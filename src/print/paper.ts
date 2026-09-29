/**
 * v0.3.0 release QA — the paper-aware layout of 인쇄 ▸ 크기 ▸ 실제 크기.
 *
 * The webview does not know which paper the system print panel will use, and while printing
 * WKWebView resolves `vw` / `vh` against the window, so the sheet cannot be sized from the
 * paper it lands on. After the X8 fix an A4 page at 실제 크기 on Letter (the panel's default
 * with no printer chosen) was taller than the sheet and spilled onto a second one.
 *
 * 실제 크기 therefore prints for a paper the user picks in the 인쇄 dialog (`paperChoice.ts`:
 * A4 by default, Letter in North America, or the document's own page size):
 * * `startDomPrint` puts the sheet size for that paper on the job (`paperSize`, here), and
 * * `PrintRoot` lays every sheet out as that paper in physical units, with a per-job `@page`
 *   size (`paperLayout.ts`) — the page centred at its real size or, when it is larger than
 *   the paper, shrunk to fit (never enlarged), which the dialog announces (`shrunkCount`).
 *
 * The three modules are split by who loads them — this one with `printFlow`, the layout with
 * `PrintRoot`, the choice with the 인쇄 dialog — so that no new shared chunk joins the entry's
 * preload map (the critical-path budget). Pure functions only.
 */
import type { PaperId } from "./paperChoice";

/** Portrait sizes in points (1/72 in). */
export const PAPER_PT: Record<Exclude<PaperId, "page">, [number, number]> = {
  a4: [595.28, 841.89],
  letter: [612, 792],
  legal: [612, 1008],
  a3: [841.89, 1190.55],
};

/** The most common size among `sizes` (ties: the first seen); Letter when there are none. */
function commonSize(sizes: [number, number][]): [number, number] {
  const counts = new Map<string, { size: [number, number]; n: number }>();
  let best: { size: [number, number]; n: number } | undefined;
  for (const s of sizes) {
    const key = `${Math.round(s[0])}x${Math.round(s[1])}`;
    const e = counts.get(key) ?? { size: s, n: 0 };
    e.n += 1;
    counts.set(key, e);
    if (!best || e.n > best.n) best = e;
  }
  return best?.size ?? [612, 792];
}

/**
 * The sheet size in points for `paper`, turned landscape when most pages are landscape (one
 * `@page` size serves the whole job: named pages are not honoured by WebKit).
 */
export function paperSize(paper: PaperId, sizes: [number, number][]): [number, number] {
  if (paper === "page") return commonSize(sizes);
  const [w, h] = PAPER_PT[paper];
  const landscape = sizes.filter((s) => s[0] > s[1]).length * 2 > sizes.length;
  return landscape ? [h, w] : [w, h];
}

/** How many of `sizes` 실제 크기 has to shrink to fit `paper` (the dialog's notice). */
export function shrunkCount(paper: PaperId, sizes: [number, number][]): number {
  const sheet = paperSize(paper, sizes);
  // A hair of tolerance: a page that *is* the paper size (A4 on A4) is not "shrunk".
  return sizes.filter((s) => s[0] > sheet[0] + 0.5 || s[1] > sheet[1] + 0.5).length;
}
