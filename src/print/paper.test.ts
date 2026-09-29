/**
 * v0.3.0 — 인쇄 ▸ 실제 크기 is paper-aware (release QA: an A4 page at actual size on Letter
 * paper spilled onto a second sheet). The layout is pure CSS generation, pinned here.
 */
import { describe, expect, it } from "vitest";
import { PAPER_PT, paperSize, shrunkCount } from "./paper";
import { defaultPaper, PAPERS } from "./paperChoice";
import { actualSizeCss, placeActual, SHEET_SLACK_PT } from "./paperLayout";

const A4: [number, number] = [595.28, 841.89];
const LETTER: [number, number] = [612, 792];

describe("print paper", () => {
  it("defaults to A4, Letter only in the Letter regions", () => {
    expect(defaultPaper("ko-KR")).toBe("a4");
    expect(defaultPaper("ko")).toBe("a4");
    expect(defaultPaper("en-GB")).toBe("a4");
    expect(defaultPaper("en-US")).toBe("letter");
    expect(defaultPaper("fr_CA")).toBe("letter");
    expect(defaultPaper(undefined)).toBe("a4");
    expect(PAPERS[0]).toBe("a4");
  });

  it("turns the paper landscape only when most pages are landscape", () => {
    expect(paperSize("a4", [A4, A4])).toEqual(PAPER_PT.a4);
    expect(paperSize("a4", [[842, 595], [842, 595], A4])).toEqual([841.89, 595.28]);
    expect(paperSize("letter", [[792, 612], A4])).toEqual([612, 792]); // a tie stays portrait
  });

  it("문서 페이지 크기 uses the most common page size", () => {
    expect(paperSize("page", [LETTER, A4, A4])).toEqual(A4);
    expect(paperSize("page", [])).toEqual(LETTER);
  });

  it("keeps a page that fits at its real size", () => {
    expect(placeActual(A4, PAPER_PT.a3)).toEqual({ width: 595.28, height: 841.89, scale: 1 });
    expect(placeActual(A4, PAPER_PT.a4).scale).toBe(1);
  });

  it("shrinks an A4 page on Letter to fit instead of spilling onto a second sheet", () => {
    const placed = placeActual(A4, PAPER_PT.letter);
    expect(placed.scale).toBeCloseTo(792 / 841.89, 6);
    expect(placed.height).toBe(792);
    expect(placed.width).toBeLessThanOrEqual(612);
    expect(shrunkCount("letter", [A4, LETTER, A4])).toBe(2);
    expect(shrunkCount("a4", [A4, A4])).toBe(0);
    // Never enlarged: a small page on A3 stays small.
    expect(placeActual([300, 400], PAPER_PT.a3)).toEqual({ width: 300, height: 400, scale: 1 });
  });

  it("generates a per-job @page size and a sheet as big as the paper", () => {
    const css = actualSizeCss(paperSize("letter", [A4]));
    expect(css).toContain("@media print");
    expect(css).toContain("@page { size: 612pt 792pt; margin: 0; }");
    expect(css).toContain(`.print-sheet[data-fit="actual"] { width: 612pt; height: ${792 - SHEET_SLACK_PT}pt;`);
    expect(css).toContain("justify-content: center");

    const landscape = actualSizeCss(paperSize("a4", [[842, 595]]));
    expect(landscape).toContain("@page { size: 841.89pt 595.28pt; margin: 0; }");

    expect(actualSizeCss(paperSize("page", [[500, 700]]))).toContain("@page { size: 500pt 700pt; margin: 0; }");
  });
});
