/**
 * `normalize.tesseract` and `normalize.vision` (WORKPLAN row (f) DoD).
 *
 * The Hangul cases are the real ones from the spike: tesseract splits `갑은 을에게` into five
 * "words" while `line.text` spaces it correctly (docs/spikes/ocr.md §4.4).
 */
import { describe, expect, it } from "vitest";
import {
  countWords, devicePixelToPdfPoint, imageBoxToPdfRect, meanConfidence, normalizeTesseract,
  normalizeVision, ocrScale, rebuildLineWords, hasHangul,
  type NormalizeContext, type TessLine, type TessPage, type VisionResult,
} from "./normalize";

const CTX: NormalizeContext = { page: 0, dpi: 300, widthPx: 2480, heightPx: 3508, rotation: 0 };

function word(text: string, x0: number, x1: number, confidence = 90) {
  return { text, confidence, bbox: { x0, y0: 100, x1, y1: 160 } };
}

function line(text: string, words: ReturnType<typeof word>[], extra: Partial<TessLine> = {}): TessLine {
  const x0 = Math.min(...words.map((w) => w.bbox.x0));
  const x1 = Math.max(...words.map((w) => w.bbox.x1));
  return {
    text,
    confidence: 90,
    bbox: { x0, y0: 100, x1, y1: 160 },
    words,
    rowAttributes: { ascenders: 12, descenders: -6, rowHeight: 60 },
    ...extra,
  };
}

function page(lines: TessLine[]): TessPage {
  return { blocks: [{ paragraphs: [{ lines }], bbox: { x0: 0, y0: 0, x1: 2480, y1: 3508 } }], text: "" };
}

describe("normalize.tesseract", () => {
  it("rebuilds Hangul spacing from line.text and merges over-segmented syllables", () => {
    // `갑은 을에게` comes back as five syllable "words"; line.text has the real spacing.
    const l = line("갑은 을에게", [
      word("갑", 100, 160), word("은", 160, 220),
      word("을", 260, 320), word("에", 320, 380), word("게", 380, 440),
    ]);
    const words = rebuildLineWords(l);
    expect(words.map((w) => w.text)).toEqual(["갑은", "을에게"]);
    // The merged box spans every syllable it swallowed.
    expect(words[0].bbox).toEqual([100, 100, 220, 160]);
    expect(words[1].bbox).toEqual([260, 100, 440, 160]);
  });

  it("never joins words the line text separates", () => {
    const l = line("대한민국의 수도는 서울", [
      word("대한민국의", 100, 400), word("수도는", 440, 640), word("서울", 680, 800),
    ]);
    expect(rebuildLineWords(l).map((w) => w.text)).toEqual(["대한민국의", "수도는", "서울"]);
  });

  it("keeps mixed Korean/Latin lines intact", () => {
    const l = line("SeePDF 광학 문자 인식", [
      word("SeePDF", 100, 300), word("광", 340, 380), word("학", 380, 420),
      word("문자", 460, 540), word("인식", 580, 660),
    ]);
    const words = rebuildLineWords(l);
    expect(words.map((w) => w.text)).toEqual(["SeePDF", "광학", "문자", "인식"]);
  });

  it("merges by horizontal gap when line.text and the word list disagree", () => {
    // line.text is garbage (an engine that dropped the text but kept boxes) → geometric fallback.
    const l = line("", [
      word("을", 260, 320), word("에", 322, 380), word("게", 382, 440), word("금액", 600, 720),
    ]);
    expect(rebuildLineWords(l).map((w) => w.text)).toEqual(["을에게", "금액"]);
  });

  it("drops low-confidence and whitespace-only words", () => {
    const l = line("좋은 나쁜", [word("좋은", 100, 200, 95), word(" ", 210, 215, 90), word("나쁜", 240, 340, 12)]);
    expect(rebuildLineWords(l).map((w) => w.text)).toEqual(["좋은"]);
    // …unless the caller asks for everything.
    expect(rebuildLineWords(l, 0).map((w) => w.text)).toEqual(["좋은", "나쁜"]);
  });

  it("weights the merged confidence by syllable count", () => {
    const l = line("가나다", [word("가", 100, 140, 60), word("나다", 140, 220, 90)]);
    // (60×1 + 90×2) / 3 = 80
    expect(rebuildLineWords(l)[0].confidence).toBe(80);
  });

  it("produces the contract's OcrPage shape", () => {
    const result = normalizeTesseract(page([line("가나 다라", [word("가나", 100, 200), word("다라", 240, 340)])]), CTX);
    expect(result).toMatchObject({ page: 0, dpi: 300, widthPx: 2480, heightPx: 3508, rotation: 0 });
    expect(result.lines).toHaveLength(1);
    expect(result.lines[0].text).toBe("가나 다라");
    expect(result.lines[0].rowHeightPx).toBe(60);
    expect(result.lines[0].words[0]).toEqual({ text: "가나", bbox: [100, 100, 200, 160], confidence: 90 });
    expect(countWords(result)).toBe(2);
    expect(meanConfidence(result)).toBe(90);
  });

  it("keeps an absolute baseline and lifts a box-relative one", () => {
    const words = [word("가나", 100, 200)];
    const absolute = normalizeTesseract(
      page([line("가나", words, { baseline: { x0: 100, y0: 152, x1: 200, y1: 152 } })]), CTX,
    );
    expect(absolute.lines[0].baseline).toEqual([100, 152, 200, 152]);

    const relative = normalizeTesseract(
      page([line("가나", words, { baseline: { x0: 0, y0: -8, x1: 100, y1: -8 } })]), CTX,
    );
    // bbox is y0=100..y1=160, so -8 from the bottom is 152 in image pixels.
    expect(relative.lines[0].baseline).toEqual([100, 152, 200, 152]);
  });

  it("sorts the lines of a block into reading order", () => {
    const top = line("위", [word("위", 100, 200)]);
    const bottom = line("아래", [word("아래", 100, 220)]);
    bottom.bbox = { x0: 100, y0: 400, x1: 220, y1: 460 };
    bottom.words[0].bbox = { x0: 100, y0: 400, x1: 220, y1: 460 };
    const result = normalizeTesseract(page([bottom, top]), CTX);
    expect(result.lines.map((l) => l.text)).toEqual(["위", "아래"]);
  });

  it("survives an empty result", () => {
    expect(normalizeTesseract({ blocks: null }, CTX).lines).toEqual([]);
    expect(normalizeTesseract({ blocks: [] }, CTX).lines).toEqual([]);
  });
});

describe("normalize.vision", () => {
  const ctx: NormalizeContext = { page: 2, dpi: 300, widthPx: 1000, heightPx: 2000, rotation: 0 };

  it("maps normalised bottom-left boxes to top-left image pixels", () => {
    const result: VisionResult = {
      observations: [{
        text: "검색가능한 Searchable",
        confidence: 0.95,
        box: { x: 0.1, y: 0.8, width: 0.5, height: 0.05 },
        words: [
          { text: "검색가능한", confidence: 0.96, box: { x: 0.1, y: 0.8, width: 0.3, height: 0.05 } },
          { text: "Searchable", confidence: 0.94, box: { x: 0.42, y: 0.8, width: 0.18, height: 0.05 } },
        ],
      }],
    };
    const page = normalizeVision(result, ctx);
    expect(page).toMatchObject({ page: 2, dpi: 300, widthPx: 1000, heightPx: 2000, rotation: 0 });
    expect(page.lines).toHaveLength(1);
    // y: 1 - (0.8 + 0.05) = 0.15 → 300 px top, 1 - 0.8 = 0.2 → 400 px bottom.
    expect(page.lines[0].bbox).toEqual([100, 300, 600, 400]);
    expect(page.lines[0].words[0]).toEqual({ text: "검색가능한", bbox: [100, 300, 400, 400], confidence: 96 });
    expect(page.lines[0].text).toBe("검색가능한 Searchable");
    // Hangul sits lower in the box than Latin (spike §5).
    expect(page.lines[0].baseline?.[1]).toBeCloseTo(390, 5);
  });

  it("splits an observation that has no word boxes", () => {
    const page = normalizeVision(
      { observations: [{ text: "one two", confidence: 0.9, box: { x: 0, y: 0.5, width: 1, height: 0.05 } }] },
      ctx,
    );
    expect(page.lines[0].words.map((w) => w.text)).toEqual(["one", "two"]);
    expect(page.lines[0].words[0].bbox[0]).toBe(0);
    expect(page.lines[0].words[1].bbox[2]).toBeCloseTo(1000, 5);
    expect(page.lines[0].words[0].confidence).toBe(90);
  });

  it("orders observations top to bottom, then left to right", () => {
    const page = normalizeVision({
      observations: [
        { text: "b", confidence: 0.9, box: { x: 0.5, y: 0.9, width: 0.1, height: 0.02 } },
        { text: "a", confidence: 0.9, box: { x: 0.1, y: 0.9, width: 0.1, height: 0.02 } },
        { text: "c", confidence: 0.9, box: { x: 0.1, y: 0.2, width: 0.1, height: 0.02 } },
      ],
    }, ctx);
    expect(page.lines.map((l) => l.text)).toEqual(["a", "b", "c"]);
  });

  it("produces the same OcrPage shape as the tesseract path", () => {
    const vision = normalizeVision(
      { observations: [{ text: "가나", confidence: 0.9, box: { x: 0, y: 0.5, width: 0.2, height: 0.05 } }] },
      ctx,
    );
    const tess = normalizeTesseract(page([line("가나", [word("가나", 0, 200)])]), ctx);
    expect(Object.keys(vision).sort()).toEqual(Object.keys(tess).sort());
    expect(Object.keys(vision.lines[0].words[0]).sort()).toEqual(Object.keys(tess.lines[0].words[0]).sort());
  });
});

describe("normalize.geometry", () => {
  const crop = { l: 0, b: 0, r: 595, t: 842 };

  it("inverts pageToDevice for every rotation", () => {
    const s = ocrScale(300);
    expect(ocrScale(300)).toBeCloseTo(4.16667, 4);
    // 0°: dx = s(x - l), dy = s(t - y)  →  (0,0) px is the top-left, i.e. (l, t) in points.
    expect(devicePixelToPdfPoint(0, 0, crop, 0, s)).toEqual([0, 842]);
    expect(devicePixelToPdfPoint(595 * s, 842 * s, crop, 0, s)).toEqual([595, 0]);
    // 90°: dx = s(y - b), dy = s(x - l)
    expect(devicePixelToPdfPoint(0, 0, crop, 90, s)).toEqual([0, 0]);
    expect(devicePixelToPdfPoint(842 * s, 595 * s, crop, 90, s)).toEqual([595, 842]);
    // 180°: dx = s(r - x), dy = s(y - b)
    expect(devicePixelToPdfPoint(0, 0, crop, 180, s)).toEqual([595, 0]);
    // 270°: dx = s(t - y), dy = s(r - x)
    expect(devicePixelToPdfPoint(0, 0, crop, 270, s)).toEqual([595, 842]);
  });

  it("turns a word box into a PDF rect", () => {
    const s = ocrScale(72);   // 1 px per point keeps the arithmetic readable
    expect(imageBoxToPdfRect([10, 20, 110, 40], crop, 0, s)).toEqual({ l: 10, b: 802, r: 110, t: 822 });
  });

  it("detects Hangul", () => {
    expect(hasHangul("서울")).toBe(true);
    expect(hasHangul("Seoul")).toBe(false);
  });
});
