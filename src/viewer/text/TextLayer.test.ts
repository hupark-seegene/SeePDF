import { describe, expect, it } from "vitest";
import { CHAR_GENERATED, CHAR_SPACE, encodeTextLayer, parseTextLayer, type TextChar } from "../../ipc/binary";
import { PageTextLayer } from "./TextLayer";

const CHAR_W = 10;

function charAt(code: string, x: number, baseline: number, generated = false): TextChar {
  return {
    code: code.codePointAt(0)!,
    box: generated
      ? { l: x, b: baseline, r: x, t: baseline }
      : { l: x, b: baseline - 4, r: x + CHAR_W, t: baseline + 13 },
    baselineY: baseline,
    fontSizePt: 12,
    flags: generated ? CHAR_GENERATED | CHAR_SPACE : 0,
    objectId: 0,
  };
}

/**
 * Two lines of five characters plus pdfium's generated space at the end of the first line —
 * a zero-size box that must never be hit-tested but must still be copied (text spike §2, §8.14).
 */
function fixture(): PageTextLayer {
  const chars: TextChar[] = [];
  "Trace".split("").forEach((c, i) => chars.push(charAt(c, 80 + i * CHAR_W, 700)));
  chars.push(charAt(" ", 130, 700, true));
  "based".split("").forEach((c, i) => chars.push(charAt(c, 80 + i * CHAR_W, 680)));

  const buffer = encodeTextLayer({
    pageIndex: 0,
    matrix: [1, 0, 0, -1, 0, 792],
    crop: { l: 0, b: 0, r: 612, t: 792 },
    chars,
    words: [
      { firstChar: 0, charCount: 5, lineIndex: 0 },
      { firstChar: 6, charCount: 5, lineIndex: 1 },
    ],
    lines: [
      { firstWord: 0, wordCount: 1, baselineY: 700, box: { l: 80, b: 696, r: 130, t: 713 }, firstChar: 0, charCount: 6 },
      { firstWord: 1, wordCount: 1, baselineY: 680, box: { l: 80, b: 676, r: 130, t: 693 }, firstChar: 6, charCount: 5 },
    ],
    text: "Trace based",
  });
  return new PageTextLayer(parseTextLayer(buffer));
}

describe("TextLayer", () => {
  it("hitTest: finds the character under a point and which side of it", () => {
    const layer = fixture();
    expect(layer.charCount).toBe(11);

    const left = layer.hitTest(82, 705)!;
    expect(left.char).toBe(0);
    expect(left.line).toBe(0);
    expect(left.word).toBe(0);
    expect(left.offset).toBe(0);
    expect(left.inside).toBe(true);

    // past the middle of the same glyph the caret goes after it
    expect(layer.hitTest(88, 705)!.offset).toBe(1);
    expect(layer.hitTest(125, 705)!.char).toBe(4);
    expect(layer.hitTest(125, 705)!.offset).toBe(4);
    expect(layer.hitTest(129, 705)!.offset).toBe(5);
  });

  it("hitTest: snaps to the nearest line and the nearest character outside the text", () => {
    const layer = fixture();

    // far to the right of line 1 -> the caret lands after its last character
    const right = layer.hitTest(500, 705)!;
    expect(right.char).toBe(4);
    expect(right.offset).toBe(5);
    expect(right.inside).toBe(false);

    // below everything -> the lower line
    const below = layer.hitTest(85, 200)!;
    expect(below.line).toBe(1);
    expect(below.char).toBe(6);

    // above everything -> the upper line
    expect(layer.hitTest(85, 5000)!.line).toBe(0);

    // the generated space has a zero-size box and is never the hit
    for (const x of [126, 130, 134]) {
      expect(layer.hitTest(x, 705)!.char).not.toBe(5);
    }
  });

  it("hitTest: an empty layer has nothing to hit", () => {
    const empty = new PageTextLayer(
      parseTextLayer(
        encodeTextLayer({
          pageIndex: 3,
          matrix: [1, 0, 0, -1, 0, 792],
          crop: { l: 0, b: 0, r: 612, t: 792 },
          chars: [],
          words: [],
          lines: [],
          text: "",
        }),
      ),
    );
    expect(empty.page).toBe(3);
    expect(empty.hitTest(100, 100)).toBeNull();
    expect(empty.rangeRects(0, 5)).toEqual([]);
    expect(empty.rangeText(0, 5)).toBe("");
  });

  it("snaps to words and lines for double- and triple-clicks", () => {
    const layer = fixture();
    expect(layer.wordRange(2)).toEqual([0, 5]);
    expect(layer.wordRange(8)).toEqual([6, 11]);
    expect(layer.lineRange(2)).toEqual([0, 6]);
    expect(layer.lineRange(8)).toEqual([6, 11]);
    expect(layer.allRange()).toEqual([0, 11]);
  });

  it("produces one rectangle per line and the exact selected string (F-06)", () => {
    const layer = fixture();

    const one = layer.rangeRects(0, 5);
    expect(one).toHaveLength(1);
    expect(one[0]).toEqual({ l: 80, b: 696, r: 130, t: 713 });
    expect(layer.rangeText(0, 5)).toBe("Trace");

    const both = layer.rangeRects(0, 11);
    expect(both).toHaveLength(2);
    expect(layer.rangeText(0, 11)).toBe("Trace based");

    // a partial word, and a backwards range
    expect(layer.rangeText(6, 9)).toBe("bas");
    expect(layer.rangeText(9, 6)).toBe("bas");
    expect(layer.rangeRects(1, 3)[0]).toEqual({ l: 90, b: 696, r: 110, t: 713 });
  });

  it("turns pdfium's CRLF into single newlines when copying", () => {
    const chars: TextChar[] = [
      charAt("a", 80, 700),
      { ...charAt(" ", 90, 700, true), code: 13 },
      { ...charAt(" ", 90, 700, true), code: 10 },
      charAt("b", 80, 680),
    ];
    const layer = new PageTextLayer(
      parseTextLayer(
        encodeTextLayer({
          pageIndex: 0,
          matrix: [1, 0, 0, -1, 0, 792],
          crop: { l: 0, b: 0, r: 612, t: 792 },
          chars,
          words: [{ firstChar: 0, charCount: 1, lineIndex: 0 }],
          lines: [
            { firstWord: 0, wordCount: 1, baselineY: 700, box: { l: 80, b: 696, r: 90, t: 713 }, firstChar: 0, charCount: 3 },
            { firstWord: 1, wordCount: 0, baselineY: 680, box: { l: 80, b: 676, r: 90, t: 693 }, firstChar: 3, charCount: 1 },
          ],
          text: "a\r\nb",
        }),
      ),
    );
    expect(layer.rangeText(0, 4)).toBe("a\nb");
  });
});
