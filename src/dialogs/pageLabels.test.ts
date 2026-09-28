import { describe, expect, it } from "vitest";
import {
  formatNumber, labelsFor, newRow, normalizeRanges, rangesFromRows, rowError, rowsFromRanges, sameRanges, toLetters, toRoman,
} from "./pageLabels";
import { displayLabel, pageForEntry } from "../viewer/pageLabel";

describe("pageLabels.format", () => {
  it("roman and letters follow ISO 32000 / PDFium", () => {
    expect([1, 2, 3, 4, 9, 14, 40, 90, 400, 1994, 3999].map(toRoman)).toEqual([
      "i", "ii", "iii", "iv", "ix", "xiv", "xl", "xc", "cd", "mcmxciv", "mmmcmxcix",
    ]);
    expect(toRoman(4000)).toBe("mmmm");
    // letters repeat, they do not count in base 26: 27 = aa, 28 = bb, 53 = aaa
    expect([1, 2, 26, 27, 28, 52, 53].map(toLetters)).toEqual(["a", "b", "z", "aa", "bb", "zz", "aaa"]);
    expect(formatNumber(3, "romanUpper")).toBe("III");
    expect(formatNumber(28, "alphaUpper")).toBe("BB");
    expect(formatNumber(7, "none")).toBe("");
    expect(formatNumber(12, "decimal")).toBe("12");
  });

  it("labels every page, with a leading decimal range when the first one starts later", () => {
    // the generated fixture's tree: i, ii, 1, 2, App-A, App-B
    expect(
      labelsFor(
        [
          { start: 0, style: "roman" },
          { start: 2, style: "decimal", first: 1 },
          { start: 4, style: "alphaUpper", prefix: "App-" },
        ],
        6,
      ),
    ).toEqual(["i", "ii", "1", "2", "App-A", "App-B"]);
    expect(labelsFor([{ start: 2, style: "roman", first: 5 }], 5)).toEqual(["1", "2", "v", "vi", "vii"]);
    expect(labelsFor([{ start: 0, style: "none", prefix: "Cover" }, { start: 1, style: "decimal" }], 3)).toEqual(["Cover", "1", "2"]);
    expect(labelsFor([], 3)).toEqual([]);
    // unsorted, duplicated and out-of-range starts are normalised away
    expect(normalizeRanges([{ start: 9, style: "roman" }, { start: 1, style: "alpha" }, { start: 1, style: "roman" }], 4)).toEqual([
      { start: 0, style: "decimal" },
      { start: 1, style: "alpha" },
    ]);
  });
});

describe("pageLabels.rows", () => {
  it("round-trips ranges through the editable rows", () => {
    const ranges = [
      { start: 0, style: "roman" as const },
      { start: 3, style: "decimal" as const, prefix: "P-", first: 4 },
    ];
    const rows = rowsFromRanges(ranges);
    expect(rows.map((r) => [r.start, r.style, r.prefix, r.first])).toEqual([["1", "roman", "", ""], ["4", "decimal", "P-", "4"]]);
    expect(rangesFromRows(rows)).toEqual(ranges);
    expect(sameRanges(rangesFromRows(rows), ranges)).toBe(true);
    expect(sameRanges([{ start: 0, style: "roman", first: 1 }], [{ start: 0, style: "roman" }])).toBe(true);
    expect(sameRanges([{ start: 0, style: "roman" }], [{ start: 0, style: "alpha" }])).toBe(false);
  });

  it("names the first problem of a row", () => {
    const a = newRow(1);
    const b = newRow(1);
    expect(rowError(a, [a], 5)).toBeNull();
    expect(rowError({ ...a, start: "0" }, [a], 5)).toBe("start");
    expect(rowError({ ...a, start: "6" }, [a], 5)).toBe("start");
    expect(rowError({ ...a, start: "" }, [a], 5)).toBe("start");
    expect(rowError(b, [a, b], 5)).toBe("duplicate");
    expect(rowError({ ...a, first: "0" }, [a], 5)).toBe("first");
  });
});

describe("pageLabels.display", () => {
  const labels = ["i", "ii", "1", "2", "App-A", ""];
  it("shows the label, or the number when a page has none", () => {
    expect(displayLabel(labels, 0)).toBe("i");
    expect(displayLabel(labels, 5)).toBe("6");
    expect(displayLabel(undefined, 3)).toBe("4");
  });

  it("the page box takes a label first, then a number", () => {
    expect(pageForEntry("ii", labels, 6)).toBe(1);
    expect(pageForEntry("app-a", labels, 6)).toBe(4);
    // "1" is page 3's label, which wins over the physical page 1
    expect(pageForEntry("1", labels, 6)).toBe(2);
    expect(pageForEntry("6", labels, 6)).toBe(5);
    expect(pageForEntry("7", labels, 6)).toBeNull();
    expect(pageForEntry("zz", labels, 6)).toBeNull();
    expect(pageForEntry("3", undefined, 6)).toBe(2);
  });
});
