/**
 * Pure logic behind the 문서 비교 view (P1-6): rows (one per `ComparePage`, so A and B stay aligned
 * by construction), change navigation, and the PDF-point → CSS-pixel conversion of the diff rects.
 */
import { MAX_WHOLE_PAGE_PX, MAX_WHOLE_PAGE_SCALE, pageToDevice, rectToBox, type Box } from "../viewer/geometry";
import type { CompareReport, PageGeom, PageIndex, Rect } from "../ipc/types";

export interface CompareRow {
  /** index into `report.pages` */
  index: number;
  pageA: PageIndex | null;
  pageB: PageIndex | null;
  changed: boolean;
  /** delete + replace runs, PDF points on page A */
  rectsA: Rect[];
  /** insert + replace runs, PDF points on page B */
  rectsB: Rect[];
  inserted: number;
  deleted: number;
}

/** Words in a diff run's text (whitespace-separated, like the engine's tokeniser). */
export function wordCount(text: string | undefined): number {
  if (!text) return 0;
  const trimmed = text.trim();
  return trimmed ? trimmed.split(/\s+/u).length : 0;
}

export function buildRows(report: CompareReport): CompareRow[] {
  return report.pages.map((p, index) => {
    const rectsA: Rect[] = [];
    const rectsB: Rect[] = [];
    let inserted = 0;
    let deleted = 0;
    for (const op of p.ops) {
      if (op.kind === "delete" || op.kind === "replace") {
        rectsA.push(...(op.rectsA ?? []));
        deleted += op.words;
      }
      if (op.kind === "insert" || op.kind === "replace") {
        rectsB.push(...(op.rectsB ?? []));
        inserted += op.kind === "insert" ? op.words : wordCount(op.textB);
      }
    }
    return { index, pageA: p.pageA, pageB: p.pageB, changed: p.changed, rectsA, rectsB, inserted, deleted };
  });
}

export function visibleRows(rows: CompareRow[], changedOnly: boolean): CompareRow[] {
  return changedOnly ? rows.filter((r) => r.changed) : rows;
}

/**
 * The next (`dir = 1`) or previous (`dir = -1`) changed row relative to row `from` (a `CompareRow.index`;
 * `-1` = before the first row). `null` when there is none in that direction — no wrap-around.
 */
export function nextChanged(rows: CompareRow[], from: number, dir: 1 | -1): number | null {
  if (dir === 1) {
    for (const r of rows) if (r.changed && r.index > from) return r.index;
    return null;
  }
  for (let i = rows.length - 1; i >= 0; i--) if (rows[i].changed && rows[i].index < from) return rows[i].index;
  return null;
}

/** Row offsets (top, in scroller px) → the row the viewport's top edge is in. `-1` when there are none. */
export function rowAtOffset(offsets: { index: number; top: number }[], scrollTop: number): number {
  let current = -1;
  for (const o of offsets) {
    if (o.top <= scrollTop + 1) current = o.index;
    else break;
  }
  return current === -1 && offsets.length ? offsets[0].index : current;
}

/** CSS scale that fits a page into `widthPx`. */
export function fitScale(geom: PageGeom, widthPx: number): number {
  return Math.max(0.05, widthPx / Math.max(1, geom.widthPt));
}

/**
 * `sk` for the `/page` route: the CSS scale times the display density, capped so the bitmap stays a
 * whole-page render (the viewer's own `s ≤ 2`, ≤ 2.5 Mpx rule) — this view never tiles.
 */
export function pageScaleKey(geom: PageGeom, cssScale: number, dpr = 1): number {
  const want = cssScale * dpr;
  const areaCap = Math.sqrt(MAX_WHOLE_PAGE_PX / Math.max(1, geom.widthPt * geom.heightPt));
  const s = Math.min(want, MAX_WHOLE_PAGE_SCALE, areaCap);
  return Math.max(1, Math.floor(s * 100 + 1e-6));
}

/** A diff rect (PDF points, y-up, unrotated) → a CSS box on the page drawn at `cssScale`. */
export function rectToCss(rect: Rect, geom: PageGeom, cssScale: number): Box {
  return rectToBox(rect, pageToDevice(geom, 0, cssScale));
}
