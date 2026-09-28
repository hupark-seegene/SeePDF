import { describe, expect, it } from "vitest";
import { buildRows, fitScale, nextChanged, pageScaleKey, rectToCss, rowAtOffset, visibleRows, wordCount } from "./model";
import type { CompareReport, PageGeom } from "../ipc/types";

const A4: PageGeom = { index: 0, widthPt: 595, heightPt: 842, rotation: 0, crop: { l: 0, b: 0, r: 595, t: 842 }, label: null };

const REPORT: CompareReport = {
  docA: "d1",
  docB: "d2",
  changedPages: 3,
  inserted: 5,
  deleted: 4,
  elapsedMs: 12,
  pages: [
    { pageA: 0, pageB: 0, changed: false, wordsA: 10, wordsB: 10, ops: [{ kind: "equal", words: 10 }] },
    {
      pageA: 1, pageB: 1, changed: true, wordsA: 10, wordsB: 11,
      ops: [
        { kind: "equal", words: 4 },
        { kind: "replace", words: 1, textA: "old", textB: "new words", rectsA: [{ l: 1, b: 1, r: 2, t: 2 }], rectsB: [{ l: 3, b: 3, r: 4, t: 4 }] },
        { kind: "insert", words: 1, textB: "added", rectsB: [{ l: 5, b: 5, r: 6, t: 6 }] },
        { kind: "equal", words: 5 },
      ],
    },
    { pageA: 2, pageB: 2, changed: false, wordsA: 3, wordsB: 3, ops: [{ kind: "equal", words: 3 }] },
    {
      pageA: 3, pageB: null, changed: true, wordsA: 3, wordsB: 0,
      ops: [{ kind: "delete", words: 3, textA: "a b c", rectsA: [{ l: 0, b: 0, r: 9, t: 9 }, { l: 0, b: 10, r: 9, t: 19 }] }],
    },
    {
      pageA: null, pageB: 3, changed: true, wordsA: 0, wordsB: 2,
      ops: [{ kind: "insert", words: 2, textB: "x y", rectsB: [{ l: 0, b: 0, r: 1, t: 1 }] }],
    },
  ],
};

describe("compare.model", () => {
  it("pairs one row per ComparePage and splits the rects by side", () => {
    const rows = buildRows(REPORT);
    expect(rows.map((r) => [r.pageA, r.pageB, r.changed])).toEqual([
      [0, 0, false], [1, 1, true], [2, 2, false], [3, null, true], [null, 3, true],
    ]);
    expect(rows[1].rectsA).toEqual([{ l: 1, b: 1, r: 2, t: 2 }]);
    expect(rows[1].rectsB).toHaveLength(2);
    // replace counts its B-side words from textB; insert from `words`
    expect(rows[1].inserted).toBe(3);
    expect(rows[1].deleted).toBe(1);
    expect(rows[3].rectsA).toHaveLength(2);
    expect(rows[3].rectsB).toEqual([]);
    expect(rows[4].inserted).toBe(2);
    expect(visibleRows(rows, true).map((r) => r.index)).toEqual([1, 3, 4]);
    expect(visibleRows(rows, false)).toHaveLength(5);
  });

  it("wordCount splits on whitespace runs", () => {
    expect(wordCount(undefined)).toBe(0);
    expect(wordCount("  ")).toBe(0);
    expect(wordCount(" 한국어  문서\nPDF ")).toBe(3);
  });

  it("next / previous change walk the changed rows without wrapping", () => {
    const rows = buildRows(REPORT);
    expect(nextChanged(rows, -1, 1)).toBe(1);
    expect(nextChanged(rows, 1, 1)).toBe(3);
    expect(nextChanged(rows, 3, 1)).toBe(4);
    expect(nextChanged(rows, 4, 1)).toBeNull();
    expect(nextChanged(rows, 4, -1)).toBe(3);
    expect(nextChanged(rows, 2, -1)).toBe(1);
    expect(nextChanged(rows, 1, -1)).toBeNull();
    expect(nextChanged(rows, -1, -1)).toBeNull();
    // on the filtered list too (indices stay the report's)
    expect(nextChanged(visibleRows(rows, true), 1, 1)).toBe(3);
    expect(nextChanged(buildRows({ ...REPORT, pages: [REPORT.pages[0]] }), -1, 1)).toBeNull();
  });

  it("rowAtOffset finds the row under the top edge", () => {
    const offsets = [{ index: 0, top: 0 }, { index: 1, top: 500 }, { index: 3, top: 1000 }];
    expect(rowAtOffset(offsets, 0)).toBe(0);
    expect(rowAtOffset(offsets, 498)).toBe(0);
    expect(rowAtOffset(offsets, 499)).toBe(1); // 1 px tolerance for fractional offsets
    expect(rowAtOffset(offsets, 500)).toBe(1);
    expect(rowAtOffset(offsets, 5000)).toBe(3);
    expect(rowAtOffset([], 10)).toBe(-1);
  });

  it("converts y-up PDF rects to CSS boxes like the viewer (crop box + /Rotate)", () => {
    const s = fitScale(A4, 297.5); // 0.5
    expect(s).toBeCloseTo(0.5);
    const box = rectToCss({ l: 100, b: 700, r: 200, t: 742 }, A4, s);
    expect(box).toEqual({ x: 50, y: 50, w: 50, h: 21 });

    // crop box offset
    const cropped: PageGeom = { ...A4, crop: { l: 50, b: 40, r: 645, t: 882 } };
    expect(rectToCss({ l: 150, b: 740, r: 250, t: 782 }, cropped, 0.5)).toEqual({ x: 50, y: 50, w: 50, h: 21 });

    // a /Rotate 90 page: display size swapped, top-left of the display is the crop's bottom-left
    const rotated: PageGeom = { ...A4, widthPt: 842, heightPt: 595, rotation: 90 };
    const r = rectToCss({ l: 0, b: 0, r: 100, t: 50 }, rotated, 1);
    expect(r).toEqual({ x: 0, y: 0, w: 50, h: 100 });
  });

  it("caps the render scale to a whole-page bitmap", () => {
    expect(pageScaleKey(A4, 0.5, 2)).toBe(100);
    expect(pageScaleKey(A4, 1.2, 2)).toBeLessThanOrEqual(200);
    const huge: PageGeom = { ...A4, widthPt: 3000, heightPt: 3000 };
    const sk = pageScaleKey(huge, 1, 2);
    expect((3000 * sk) / 100 * ((3000 * sk) / 100)).toBeLessThanOrEqual(2_500_000 + 1e4);
    expect(pageScaleKey(A4, 0.001, 1)).toBe(1);
  });
});
