import { describe, expect, it } from "vitest";
import { formatPageRange, parsePageRange, resolveRange } from "./pageRange";

describe("pageRange.parse", () => {
  it("parses the 1-based grammar people type into 0-based indices", () => {
    expect(parsePageRange("1-3,5,8-", 10)).toEqual([0, 1, 2, 4, 7, 8, 9]);
    expect(parsePageRange(" 2 , 4 ", 10)).toEqual([1, 3]);
    expect(parsePageRange("-3", 10)).toEqual([0, 1, 2]);
    expect(parsePageRange("7", 10)).toEqual([6]);
  });

  it("is forgiving about order, duplicates and separators", () => {
    expect(parsePageRange("5-3", 10)).toEqual([2, 3, 4]);
    expect(parsePageRange("1,1,2;2", 10)).toEqual([0, 1]);
    expect(parsePageRange("3–4", 10)).toEqual([2, 3]); // en dash, what macOS autocorrect produces
  });

  it("rejects anything the engine would reject", () => {
    expect(parsePageRange("", 10)).toBeNull();
    expect(parsePageRange("0", 10)).toBeNull(); // 1-based: there is no page 0
    expect(parsePageRange("11", 10)).toBeNull();
    expect(parsePageRange("1-99", 10)).toBeNull();
    expect(parsePageRange("a-b", 10)).toBeNull();
    expect(parsePageRange("1--3", 10)).toBeNull();
    expect(parsePageRange("1-3", 0)).toBeNull();
  });

  it("round-trips through the compact 1-based form the engine expects", () => {
    expect(formatPageRange([0, 1, 2, 4, 7, 8, 9])).toBe("1-3, 5, 8-10");
    expect(formatPageRange([3])).toBe("4");
    expect(formatPageRange([])).toBe("");
    expect(parsePageRange(formatPageRange([0, 2, 3]), 5)).toEqual([0, 2, 3]);
  });

  it("resolves a range picker", () => {
    const ctx = { pageCount: 4, currentPage: 2, selected: [1, 3] };
    expect(resolveRange({ mode: "all", text: "" }, ctx)).toEqual([0, 1, 2, 3]);
    expect(resolveRange({ mode: "current", text: "" }, ctx)).toEqual([2]);
    expect(resolveRange({ mode: "selected", text: "" }, ctx)).toEqual([1, 3]);
    expect(resolveRange({ mode: "custom", text: "2-3" }, ctx)).toEqual([1, 2]);
    expect(resolveRange({ mode: "custom", text: "9" }, ctx)).toBeNull();
    expect(resolveRange({ mode: "selected", text: "" }, { ...ctx, selected: [] })).toBeNull();
  });
});
