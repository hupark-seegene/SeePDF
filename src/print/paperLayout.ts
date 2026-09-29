/**
 * v0.3.0 — how `PrintRoot` lays out a 실제 크기 job on its sheet (see `paper.ts`). Loaded with
 * `PrintRoot` only. The sheet size comes on the job (`PrintJob.sheet`, points, landscape already
 * applied).
 */

/** Points, rounded to 0.01 so the generated CSS is stable. */
function r(n: number): number {
  return Math.round(n * 100) / 100;
}

export interface ActualPlacement {
  /** The page image's printed size in points. */
  width: number;
  height: number;
  /** 1 at real size; below 1 when the page is larger than the paper and was shrunk to fit. */
  scale: number;
}

/** The page at its real size, or shrunk (never enlarged) to fit `sheet`. */
export function placeActual(page: [number, number], sheet: [number, number]): ActualPlacement {
  const scale = Math.min(1, sheet[0] / page[0], sheet[1] / page[1]);
  return { width: r(page[0] * scale), height: r(page[1] * scale), scale };
}

/**
 * The sheet is the paper less a sliver of height, so rounding in the print engine can never
 * push a blank sheet out.
 */
export const SHEET_SLACK_PT = 2;

/** The per-job print stylesheet of 실제 크기 on a `sheet` (points): `@page` size and the sheet box. */
export function actualSizeCss(sheet: [number, number]): string {
  const [w, h] = sheet;
  return [
    "@media print {",
    `  @page { size: ${r(w)}pt ${r(h)}pt; margin: 0; }`,
    `  .print-sheet[data-fit="actual"] { width: ${r(w)}pt; height: ${r(h - SHEET_SLACK_PT)}pt;` +
      " display: flex; align-items: center; justify-content: center; }",
    '  .print-sheet[data-fit="actual"] .print-page { flex: none; max-width: none; margin: 0; }',
    "}",
  ].join("\n");
}
