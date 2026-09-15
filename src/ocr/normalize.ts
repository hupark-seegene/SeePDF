/**
 * Engine result → the contract's `OcrPage` (IPC_CONTRACT §7.9).
 *
 * Every OCR engine we ship or plan to ship (tesseract.js today, macOS Vision in P1, Windows.Media.Ocr
 * in P2) produces a different tree. `ocr_apply` only understands one shape, so the conversion lives
 * here and nowhere else — `ocrJob.ts` never touches a raw engine result.
 *
 *   OcrPage { page, dpi, widthPx, heightPx, rotation, lines: OcrLine[] }
 *   OcrLine { text, bbox, baseline?, rowHeightPx?, words: OcrWord[] }
 *   OcrWord { text, bbox, confidence }        bbox = IMAGE pixels, origin top-left
 *
 * The hard part is Korean (spike §4.4): tesseract over-segments Hangul into syllables
 * (`갑은 을에게` → `갑`,`은`,`을`,`에`,`게`, 140–170 "words" for 91 real ones) while `line.text`
 * is spaced correctly. So we walk `line.text` with the word list and merge every run of words that
 * `line.text` does *not* separate with a space. Joining words with a blind space is how you get a
 * text layer that no one can search.
 *
 * This file has **no runtime imports** on purpose: `scripts/ocr-accuracy.mjs` loads it straight into
 * Node (type stripping) so the accuracy gate scores the shipped normaliser, not a copy of it.
 */
import type { Box, OcrLine, OcrPage, OcrWord, PageIndex, Point, Rect, Rotation } from "../ipc/types";

// ---------------------------------------------------------------------------
// Raw engine shapes
// ---------------------------------------------------------------------------

/** tesseract.js `data.blocks[].paragraphs[].lines[].words[]` (structural — we never import the lib here). */
export interface TessBbox { x0: number; y0: number; x1: number; y1: number }
export interface TessWord { text: string; confidence: number; bbox: TessBbox }
export interface TessRowAttributes { ascenders: number; descenders: number; rowHeight: number }
export interface TessLine {
  text: string; confidence: number; bbox: TessBbox; words: TessWord[];
  baseline?: TessBbox; rowAttributes?: TessRowAttributes;
}
export interface TessParagraph { lines: TessLine[]; bbox?: TessBbox }
export interface TessBlock { paragraphs: TessParagraph[]; bbox?: TessBbox }
export interface TessPage { blocks?: TessBlock[] | null; text?: string; confidence?: number }

/** macOS Vision (P1, `ocr_recognize_native`): normalised rect, origin **bottom-left**, 0..1. */
export interface VisionRect { x: number; y: number; width: number; height: number }
export interface VisionWord { text: string; confidence: number; box: VisionRect }
export interface VisionObservation {
  /** top candidate string of one `VNRecognizedTextObservation` (one per *line*) */
  text: string;
  /** 0..1 */
  confidence: number;
  box: VisionRect;
  /** per-word boxes from `boundingBoxForRange`; absent means "split the line for me" */
  words?: VisionWord[];
}
export interface VisionResult { observations: VisionObservation[] }

// ---------------------------------------------------------------------------
// Context + tuning
// ---------------------------------------------------------------------------

export interface NormalizeContext {
  page: PageIndex;
  dpi: number;
  widthPx: number;
  heightPx: number;
  rotation: Rotation;
  /** words below this confidence are dropped from the layer (spike §5); 0 keeps everything */
  minConfidence?: number;
}

/** FEATURES F-21 gate: the synthetic Korean page must stay at or under this Hangul CER at PSM 4. */
export const HANGUL_CER_BUDGET = 0.03;
/** Spike §5: "skip words with confidence < 30 and empty/whitespace tokens". */
export const DEFAULT_MIN_CONFIDENCE = 30;
/** Merge two neighbouring words when the gap is under this fraction of the mean character width. */
const GEOMETRIC_MERGE_GAP = 0.35;
/** Give up on `line.text` alignment when this fraction of the words could not be located in it. */
const ALIGNMENT_FAILURE_RATIO = 0.25;

const HANGUL = /[ᄀ-ᇿ㄰-㆏ꥠ-꥿가-힯ힰ-퟿]/;

export function hasHangul(s: string): boolean {
  return HANGUL.test(s);
}

// ---------------------------------------------------------------------------
// Geometry helpers
// ---------------------------------------------------------------------------

function union(a: Box, b: Box): Box {
  return [Math.min(a[0], b[0]), Math.min(a[1], b[1]), Math.max(a[2], b[2]), Math.max(a[3], b[3])];
}

function toBox(b: TessBbox): Box {
  return [b.x0, b.y0, b.x1, b.y1];
}

/** Device scale of an OCR render: `/ocr?dpi=300` renders the page at 300/72 ≈ 4.1667 px per point. */
export function ocrScale(dpi: number): number {
  return dpi / 72;
}

/**
 * One OCR image pixel (origin top-left, `/Rotate` already applied by the renderer) → PDF user space
 * (points, y-up, **unrotated**) — the exact inverse of `pageToDevice` in IPC_CONTRACT §3.
 *
 * `ocr_apply` itself takes pixels (the engine calls `pixels_to_points`, spike §4.16); this exists for
 * the review overlay, for tests, and for anything that wants the points without a round trip.
 */
export function devicePixelToPdfPoint(
  px: number, py: number, crop: Rect, rotation: Rotation, scale: number,
): Point {
  const { l, b, r, t } = crop;
  switch (rotation) {
    case 90: return [py / scale + l, px / scale + b];
    case 180: return [r - px / scale, py / scale + b];
    case 270: return [r - py / scale, t - px / scale];
    default: return [px / scale + l, t - py / scale];
  }
}

/** An OCR word box (image px) as a PDF-points rect in unrotated user space. */
export function imageBoxToPdfRect(box: Box, crop: Rect, rotation: Rotation, scale: number): Rect {
  const a = devicePixelToPdfPoint(box[0], box[1], crop, rotation, scale);
  const c = devicePixelToPdfPoint(box[2], box[3], crop, rotation, scale);
  return {
    l: Math.min(a[0], c[0]), b: Math.min(a[1], c[1]),
    r: Math.max(a[0], c[0]), t: Math.max(a[1], c[1]),
  };
}

// ---------------------------------------------------------------------------
// Hangul spacing / over-segmentation
// ---------------------------------------------------------------------------

interface Aligned { word: TessWord; spaceBefore: boolean; matched: boolean }

/**
 * Line up `words` with `lineText` and record, for each word, whether the line text puts a space in
 * front of it. Tesseract's word list and its `line.text` agree on order but not on tokenisation, so
 * a small forward search absorbs the difference (an inserted punctuation mark, a dropped glyph).
 */
function alignWithLineText(lineText: string, words: TessWord[]): Aligned[] {
  const text = lineText.normalize("NFC");
  const out: Aligned[] = [];
  let cursor = 0;
  for (let i = 0; i < words.length; i++) {
    const word = words[i];
    const wt = word.text.normalize("NFC");
    let spaceBefore = false;
    while (cursor < text.length && /\s/.test(text[cursor])) { spaceBefore = true; cursor++; }
    if (wt.length > 0 && text.startsWith(wt, cursor)) {
      cursor += wt.length;
      out.push({ word, spaceBefore, matched: true });
      continue;
    }
    // Forward search in a short window: tolerate one or two characters of drift.
    const window = text.slice(cursor, cursor + wt.length + 8);
    const at = wt.length > 0 ? window.indexOf(wt) : -1;
    if (at >= 0) {
      const skipped = window.slice(0, at);
      if (/\s/.test(skipped)) spaceBefore = true;
      cursor += at + wt.length;
      out.push({ word, spaceBefore, matched: true });
      continue;
    }
    out.push({ word, spaceBefore, matched: false });
  }
  return out;
}

/** Fallback when `line.text` and the word list disagree: merge by horizontal gap. */
function alignByGeometry(words: TessWord[]): Aligned[] {
  const widths = words
    .map((w) => {
      const chars = Array.from(w.text.normalize("NFC")).length;
      return chars > 0 ? (w.bbox.x1 - w.bbox.x0) / chars : 0;
    })
    .filter((n) => n > 0)
    .sort((a, b) => a - b);
  const meanCharWidth = widths.length ? widths[Math.floor(widths.length / 2)] : 0;
  return words.map((word, i) => {
    if (i === 0) return { word, spaceBefore: false, matched: true };
    const prev = words[i - 1];
    const gap = word.bbox.x0 - prev.bbox.x1;
    const glued = meanCharWidth > 0 && gap < meanCharWidth * GEOMETRIC_MERGE_GAP
      && (hasHangul(word.text) || hasHangul(prev.text));
    return { word, spaceBefore: !glued, matched: true };
  });
}

/** Collapse each run of words the line text does not separate into a single `OcrWord`. */
function mergeRuns(aligned: Aligned[]): OcrWord[] {
  const out: OcrWord[] = [];
  let parts: TessWord[] = [];
  const flush = () => {
    if (parts.length === 0) return;
    const text = parts.map((p) => p.text.normalize("NFC")).join("");
    let bbox = toBox(parts[0].bbox);
    for (let i = 1; i < parts.length; i++) bbox = union(bbox, toBox(parts[i].bbox));
    // Length-weighted mean confidence: a two-syllable merge should not be dragged down by one glyph.
    let weight = 0;
    let sum = 0;
    for (const p of parts) {
      const n = Math.max(1, Array.from(p.text).length);
      weight += n;
      sum += p.confidence * n;
    }
    out.push({ text, bbox, confidence: weight > 0 ? Math.round((sum / weight) * 10) / 10 : 0 });
    parts = [];
  };
  for (let i = 0; i < aligned.length; i++) {
    if (i > 0 && aligned[i].spaceBefore) flush();
    parts.push(aligned[i].word);
  }
  flush();
  return out;
}

/**
 * `line.words` → `OcrWord[]` with Hangul spacing rebuilt from `line.text`.
 * Exported for the unit tests (`normalize.tesseract`).
 */
export function rebuildLineWords(line: TessLine, minConfidence = DEFAULT_MIN_CONFIDENCE): OcrWord[] {
  // Whitespace-only tokens have to go *before* alignment, or they get merged into their neighbour
  // and leave a trailing space inside a word. The spacing itself comes from `line.text`, not them.
  const words = (line.words ?? []).filter((w) => w && w.bbox && (w.text ?? "").trim().length > 0);
  if (words.length === 0) return [];
  let aligned = alignWithLineText(line.text ?? "", words);
  const unmatched = aligned.filter((a) => !a.matched).length;
  if (unmatched > words.length * ALIGNMENT_FAILURE_RATIO) aligned = alignByGeometry(words);
  return mergeRuns(aligned).filter((w) => w.text.trim().length > 0 && w.confidence >= minConfidence);
}

// ---------------------------------------------------------------------------
// Baselines
// ---------------------------------------------------------------------------

/**
 * Tesseract's line baseline in absolute image pixels.
 *
 * `GetJSONText()` reports the baseline in page coordinates, but older builds report it relative to
 * the line box, so we accept both: a y that does not land inside (or just outside) the line box is
 * treated as an offset from the box's bottom-left corner.
 */
function absoluteBaseline(line: TessLine): [number, number, number, number] | undefined {
  const bl = line.baseline;
  if (!bl || !Number.isFinite(bl.y0) || !Number.isFinite(bl.y1)) return undefined;
  const { x0, y0, y1 } = line.bbox;
  const h = Math.max(1, y1 - y0);
  const inside = bl.y0 >= y0 - h && bl.y0 <= y1 + h;
  if (inside) return [bl.x0, bl.y0, bl.x1, bl.y1];
  return [x0 + bl.x0, y1 + bl.y0, x0 + bl.x1, y1 + bl.y1];
}

// ---------------------------------------------------------------------------
// tesseract.js → OcrPage
// ---------------------------------------------------------------------------

/** Sort the lines of one block into reading order (top → bottom, left → right on the same row). */
function readingOrder(lines: TessLine[]): TessLine[] {
  return [...lines].sort((a, b) => {
    const tolerance = Math.max(4, Math.min(a.bbox.y1 - a.bbox.y0, b.bbox.y1 - b.bbox.y0) * 0.5);
    if (Math.abs(a.bbox.y0 - b.bbox.y0) > tolerance) return a.bbox.y0 - b.bbox.y0;
    return a.bbox.x0 - b.bbox.x0;
  });
}

export function normalizeTesseract(data: TessPage, ctx: NormalizeContext): OcrPage {
  const minConfidence = ctx.minConfidence ?? DEFAULT_MIN_CONFIDENCE;
  const lines: OcrLine[] = [];
  // Blocks and paragraphs already come in reading order (tesseract orders columns for us); we only
  // re-sort *inside* a block, where a stray line occasionally arrives out of order.
  for (const block of data.blocks ?? []) {
    const blockLines: TessLine[] = [];
    for (const paragraph of block.paragraphs ?? []) blockLines.push(...(paragraph.lines ?? []));
    for (const line of readingOrder(blockLines)) {
      const words = rebuildLineWords(line, minConfidence);
      if (words.length === 0) continue;
      let bbox = words[0].bbox;
      for (const w of words) bbox = union(bbox, w.bbox);
      const out: OcrLine = {
        text: words.map((w) => w.text).join(" "),
        bbox: line.bbox ? toBox(line.bbox) : bbox,
        words,
      };
      const baseline = absoluteBaseline(line);
      if (baseline) out.baseline = baseline;
      const rowHeight = line.rowAttributes
        ? line.rowAttributes.rowHeight
          || (line.rowAttributes.ascenders ?? 0) - (line.rowAttributes.descenders ?? 0)
        : 0;
      if (rowHeight > 0) out.rowHeightPx = rowHeight;
      lines.push(out);
    }
  }
  return {
    page: ctx.page,
    dpi: ctx.dpi,
    widthPx: ctx.widthPx,
    heightPx: ctx.heightPx,
    rotation: ctx.rotation,
    lines,
  };
}

// ---------------------------------------------------------------------------
// Vision (P1) → OcrPage
// ---------------------------------------------------------------------------

/** Sub-pixel precision is noise in an OCR box; two decimals keeps the JSON small and stable. */
const px = (n: number) => Math.round(n * 100) / 100;

/** Normalised Vision rect (0..1, origin bottom-left) → image pixels, origin top-left. */
function visionBoxToPixels(box: VisionRect, widthPx: number, heightPx: number): Box {
  const x0 = box.x * widthPx;
  const x1 = (box.x + box.width) * widthPx;
  const y0 = (1 - (box.y + box.height)) * heightPx;
  const y1 = (1 - box.y) * heightPx;
  return [px(Math.min(x0, x1)), px(Math.min(y0, y1)), px(Math.max(x0, x1)), px(Math.max(y0, y1))];
}

/**
 * Split an observation without per-word boxes into words, distributing the line box by character
 * count. Vision *can* give per-word boxes (`boundingBoxForRange`, spike §3b) — this is what the
 * fallback looks like when the native side sends only the line.
 */
function splitObservation(text: string, bbox: Box, confidence: number): OcrWord[] {
  const tokens = text.split(/\s+/).filter(Boolean);
  if (tokens.length === 0) return [];
  const chars = tokens.reduce((n, w) => n + Array.from(w).length, 0);
  if (chars === 0) return [];
  const width = bbox[2] - bbox[0];
  const per = width / (chars + Math.max(0, tokens.length - 1));
  const out: OcrWord[] = [];
  let x = bbox[0];
  for (const token of tokens) {
    const w = Array.from(token).length * per;
    out.push({ text: token, bbox: [px(x), bbox[1], px(x + w), bbox[3]], confidence });
    x += w + per;
  }
  return out;
}

/**
 * macOS Vision result → the same `OcrPage` (P1, `ocr_recognize_native`).
 *
 * Placeholder in the sense that nothing produces a `VisionResult` yet — the *mapping* is real, so
 * when the native side lands the only thing to check is that its JSON matches `VisionResult`.
 * Vision returns one observation per line with no block/paragraph structure (spike §3b), so reading
 * order is a top-to-bottom, left-to-right sort here rather than the engine's own grouping.
 */
export function normalizeVision(result: VisionResult, ctx: NormalizeContext): OcrPage {
  const minConfidence = ctx.minConfidence ?? DEFAULT_MIN_CONFIDENCE;
  const lines: OcrLine[] = [];
  const observations = [...(result.observations ?? [])]
    .map((o) => ({ o, bbox: visionBoxToPixels(o.box, ctx.widthPx, ctx.heightPx) }))
    .sort((a, b) => {
      const tolerance = Math.max(4, Math.min(a.bbox[3] - a.bbox[1], b.bbox[3] - b.bbox[1]) * 0.5);
      if (Math.abs(a.bbox[1] - b.bbox[1]) > tolerance) return a.bbox[1] - b.bbox[1];
      return a.bbox[0] - b.bbox[0];
    });

  for (const { o, bbox } of observations) {
    const text = (o.text ?? "").normalize("NFC").trim();
    if (!text) continue;
    // Vision confidence is 0..1; the contract's OcrWord.confidence is 0..100 like Tesseract's.
    const confidence = Math.round(Math.max(0, Math.min(1, o.confidence ?? 0)) * 1000) / 10;
    const words: OcrWord[] = o.words?.length
      ? o.words
          .map((w) => ({
            text: w.text.normalize("NFC"),
            bbox: visionBoxToPixels(w.box, ctx.widthPx, ctx.heightPx),
            confidence: Math.round(Math.max(0, Math.min(1, w.confidence ?? o.confidence ?? 0)) * 1000) / 10,
          }))
          .filter((w) => w.text.trim().length > 0)
      : splitObservation(text, bbox, confidence);
    const kept = words.filter((w) => w.confidence >= minConfidence);
    if (kept.length === 0) continue;
    const height = bbox[3] - bbox[1];
    // Spike §5: Vision has no baseline; Hangul sits lower in the box than mixed-case Latin.
    const baselineY = bbox[3] - height * (hasHangul(text) ? 0.1 : 0.2);
    lines.push({
      text: kept.map((w) => w.text).join(" "),
      bbox,
      baseline: [bbox[0], baselineY, bbox[2], baselineY],
      rowHeightPx: height,
      words: kept,
    });
  }

  return {
    page: ctx.page,
    dpi: ctx.dpi,
    widthPx: ctx.widthPx,
    heightPx: ctx.heightPx,
    rotation: ctx.rotation,
    lines,
  };
}

/** Total words in a page — the dialog's result summary and the tests both want it. */
export function countWords(page: OcrPage): number {
  return page.lines.reduce((n, l) => n + l.words.length, 0);
}

/** Mean word confidence (0..100) of a page, 0 when it has no words. */
export function meanConfidence(page: OcrPage): number {
  let n = 0;
  let sum = 0;
  for (const line of page.lines) for (const w of line.words) { n++; sum += w.confidence; }
  return n ? Math.round((sum / n) * 10) / 10 : 0;
}
