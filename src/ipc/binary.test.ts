import { describe, expect, it } from "vitest";
import { CHAR_SPACE, encodeRawPage, encodeTextLayer, parseRawPage, parseTextLayer } from "./binary";

describe("§10.1 text layer", () => {
  const input = {
    pageIndex: 2,
    matrix: [1, 0, 0, -1, 0, 841.89] as [number, number, number, number, number, number],
    crop: { l: 0, b: 0, r: 595.28, t: 841.89 },
    chars: [
      { code: "한".codePointAt(0)!, box: { l: 72, b: 700, r: 83, t: 712 }, baselineY: 702, fontSizePt: 11, flags: 0, objectId: 5 },
      { code: 32, box: { l: 83, b: 700, r: 87, t: 712 }, baselineY: 702, fontSizePt: 11, flags: CHAR_SPACE, objectId: 5 },
      { code: 65, box: { l: 87, b: 700, r: 93, t: 712 }, baselineY: 702, fontSizePt: 11, flags: 0, objectId: 5 },
    ],
    words: [
      { firstChar: 0, charCount: 1, lineIndex: 0 },
      { firstChar: 2, charCount: 1, lineIndex: 0 },
    ],
    lines: [{ firstWord: 0, wordCount: 2, baselineY: 702, box: { l: 72, b: 700, r: 93, t: 712 }, firstChar: 0, charCount: 3 }],
    text: "한 A\r\n",
    hasObjectIds: true,
  };

  it("round-trips chars, words, lines and the page text", () => {
    const view = parseTextLayer(encodeTextLayer(input));
    expect(view.header.pageIndex).toBe(2);
    expect(view.header.charCount).toBe(3);
    expect(view.header.wordCount).toBe(2);
    expect(view.header.lineCount).toBe(1);
    expect(view.header.hasObjectIds).toBe(true);
    expect(view.header.crop.t).toBeCloseTo(841.89, 2);
    expect(view.header.matrix[3]).toBe(-1);

    const first = view.char(0);
    expect(String.fromCodePoint(first.code)).toBe("한");
    expect(first.box.l).toBeCloseTo(72, 3);
    expect(first.fontSizePt).toBeCloseTo(11, 3);
    expect(view.char(1).flags & CHAR_SPACE).toBe(CHAR_SPACE);
    expect(view.char(2).objectId).toBe(5);

    expect(view.word(1)).toEqual({ firstChar: 2, charCount: 1, lineIndex: 0 });
    expect(view.wordText(0)).toBe("한");
    expect(view.line(0).box.r).toBeCloseTo(93, 3);
    expect(view.text).toBe("한 A\r\n");
  });

  it("keeps every section 4-byte aligned", () => {
    const buffer = encodeTextLayer(input);
    expect(buffer.byteLength % 4).toBe(0);
  });

  it("rejects a foreign buffer", () => {
    expect(() => parseTextLayer(new ArrayBuffer(64))).toThrow(/STXL/);
  });
});

describe("§10.2 raw page", () => {
  it("round-trips RGBA pixels", () => {
    const px = new Uint8ClampedArray(2 * 2 * 4).fill(200);
    const raw = parseRawPage(encodeRawPage(2, 2, px));
    expect(raw.width).toBe(2);
    expect(raw.height).toBe(2);
    expect(raw.stride).toBe(8);
    expect(raw.pixels.length).toBe(16);
    expect(raw.pixels[0]).toBe(200);
  });
});
