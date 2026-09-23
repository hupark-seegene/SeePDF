import { describe, expect, it } from "vitest";
import { formatPdfDate, metaChanged, metaFromForm } from "./docInfo";

describe("docInfo.formatPdfDate", () => {
  it("formats a full date with a zone", () => {
    const s = formatPdfDate("D:20240102030405+09'00'", "en");
    expect(s).toMatch(/2024|2023/); // local zone may shift the day
    expect(s).not.toContain("D:");
  });
  it("formats a zoneless date as written", () => {
    expect(formatPdfDate("D:20240102030405", "en")).toMatch(/January 2, 2024/);
    expect(formatPdfDate("D:20240102", "ko")).toMatch(/2024년 1월 2일/);
  });
  it("tolerates a year alone", () => {
    expect(formatPdfDate("D:2024", "en")).toBe("2024");
  });
  it("accepts Z and a missing D: prefix", () => {
    expect(formatPdfDate("20240615Z", "en")).toMatch(/June 15, 2024/);
  });
  it("returns garbage unchanged and a dash for nothing", () => {
    expect(formatPdfDate("yesterday", "en")).toBe("yesterday");
    expect(formatPdfDate("D:20241399", "en")).toBe("D:20241399");
    expect(formatPdfDate(undefined, "en")).toBe("—");
  });
});

describe("docInfo.metaFromForm", () => {
  it("trims and keeps empties as blank strings (= remove)", () => {
    expect(metaFromForm({ title: "  a ", author: "", subject: "   ", keywords: "x, y" })).toEqual({
      title: "a",
      author: "",
      subject: "",
      keywords: "x, y",
    });
  });
  it("detects changes", () => {
    const meta = { title: "a", author: "b" };
    expect(metaChanged(meta, { title: "a ", author: "b", subject: "", keywords: "" })).toBe(false);
    expect(metaChanged(meta, { title: "a", author: "", subject: "", keywords: "" })).toBe(true);
  });
});
