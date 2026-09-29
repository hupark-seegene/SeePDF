/**
 * Document layout and virtualisation (ARCHITECTURE §10, UI_SPEC §5).
 *
 * The layout comes from `DocInfo.pages[]` alone — no pixel has to exist before a 500-page
 * document has its final geometry, which is what keeps "open → first paint" at ≤ 400 ms and
 * stops the scrollbar from jumping while thumbnails arrive.
 *
 * Rows, not pages, are the unit: 단일 = one row (the current page), 연속 = one page per row,
 * 두 쪽 = spreads of two, 두 쪽 (표지 따로) = the cover alone, then spreads (v0.3). Only rows inside
 * `[scrollTop − 1.5·vh, scrollTop + 2.5·vh]` are mounted.
 */
import type { PageGeom, PageIndex, Rotation, ViewLayout } from "../ipc/types";
import { pageBoxCss, scaleKeyFor, type Size } from "./geometry";

export const PAGE_GAP = 16;
export const PAD_X = 16;
export const PAD_Y = 24;
/** Mount window, in screens, before and after the viewport (ARCHITECTURE §10). */
export const OVERSCAN_BEFORE = 1.5;
export const OVERSCAN_AFTER = 2.5;

export interface LayoutItem {
  page: PageIndex;
  /** CSS px inside the scroll content */
  x: number;
  y: number;
  w: number;
  h: number;
  row: number;
}

export interface LayoutRow {
  index: number;
  y: number;
  h: number;
  pages: PageIndex[];
}

export interface DocLayout {
  items: LayoutItem[];
  rows: LayoutRow[];
  byPage: Map<PageIndex, LayoutItem>;
  /** total scrollable content size in CSS px */
  width: number;
  height: number;
  zoomPercent: number;
  rotation: Rotation;
  mode: ViewLayout;
  scaleKey: number;
}

export interface LayoutOptions {
  pages: PageGeom[];
  zoomPercent: number;
  rotation: Rotation;
  mode: ViewLayout;
  viewport: Size;
  /** the page 단일 mode shows; ignored by 연속 / 두 쪽 */
  currentPage?: PageIndex;
  dpr?: number;
  gap?: number;
  padX?: number;
  padY?: number;
}

/**
 * Which pages share a row. 두 쪽 pairs from an even index, so page 1 sits left of page 2; 두 쪽
 * (표지 따로, `twoCover`) shows page 1 alone and pairs from there — [1], [2, 3], [4, 5] … — the way a
 * printed book opens (V5).
 */
export function pageRows(pageCount: number, mode: ViewLayout, currentPage = 0): PageIndex[][] {
  if (pageCount <= 0) return [];
  if (mode === "single") {
    const page = Math.max(0, Math.min(pageCount - 1, currentPage));
    return [[page]];
  }
  if (mode === "two" || mode === "twoCover") {
    const rows: PageIndex[][] = [];
    let i = 0;
    if (mode === "twoCover") {
      rows.push([0]);
      i = 1;
    }
    for (; i < pageCount; i += 2) {
      rows.push(i + 1 < pageCount ? [i, i + 1] : [i]);
    }
    return rows;
  }
  const rows: PageIndex[][] = [];
  for (let i = 0; i < pageCount; i++) rows.push([i]);
  return rows;
}

/**
 * Absolute CSS boxes for every mounted-able page. Rows are centred inside the content width, so
 * the scroller needs no flex and the content coordinate of a page never depends on the viewport
 * except through that centring.
 */
export function computeLayout(o: LayoutOptions): DocLayout {
  const gap = o.gap ?? PAGE_GAP;
  const padX = o.padX ?? PAD_X;
  const padY = o.padY ?? PAD_Y;
  const dpr = o.dpr ?? 1;
  const scaleKey = scaleKeyFor(o.zoomPercent, dpr);
  const rows = pageRows(o.pages.length, o.mode, o.currentPage ?? 0);

  const sizes = new Map<PageIndex, Size>();
  let widest = 0;
  for (const row of rows) {
    let rowW = 0;
    for (const page of row) {
      const geom = o.pages[page];
      if (!geom) continue;
      const size = pageBoxCss(geom, o.rotation, scaleKey, dpr);
      sizes.set(page, size);
      rowW += size.w;
    }
    rowW += gap * Math.max(0, row.length - 1);
    widest = Math.max(widest, rowW);
  }

  const contentW = Math.max(o.viewport.w, widest + padX * 2);
  const items: LayoutItem[] = [];
  const layoutRows: LayoutRow[] = [];
  let y = padY;

  rows.forEach((pages, index) => {
    let rowW = gap * Math.max(0, pages.length - 1);
    let rowH = 0;
    for (const page of pages) {
      const size = sizes.get(page);
      if (!size) continue;
      rowW += size.w;
      rowH = Math.max(rowH, size.h);
    }
    let x = Math.max(padX, Math.round((contentW - rowW) / 2));
    for (const page of pages) {
      const size = sizes.get(page);
      if (!size) continue;
      items.push({ page, x, y: y + Math.round((rowH - size.h) / 2), w: size.w, h: size.h, row: index });
      x += size.w + gap;
    }
    layoutRows.push({ index, y, h: rowH, pages });
    y += rowH + gap;
  });

  const height = Math.max(o.viewport.h, Math.max(padY, y - gap + padY));
  return {
    items,
    rows: layoutRows,
    byPage: new Map(items.map((item) => [item.page, item])),
    width: contentW,
    height,
    zoomPercent: o.zoomPercent,
    rotation: o.rotation,
    mode: o.mode,
    scaleKey,
  };
}

export interface VisibleRange {
  /** row indices, inclusive; `first > last` means nothing is mounted */
  first: number;
  last: number;
  pages: PageIndex[];
}

/** Rows intersecting `[scrollTop − before·vh, scrollTop + after·vh]` (ARCHITECTURE §10). */
export function visibleRange(
  layout: DocLayout,
  scrollTop: number,
  viewportH: number,
  before = OVERSCAN_BEFORE,
  after = OVERSCAN_AFTER,
): VisibleRange {
  const vh = Math.max(1, viewportH);
  const top = scrollTop - before * vh;
  const bottom = scrollTop + after * vh;
  let first = -1;
  let last = -2;
  for (const row of layout.rows) {
    if (row.y + row.h < top || row.y > bottom) continue;
    if (first === -1) first = row.index;
    last = row.index;
  }
  // A degenerate viewport (a hidden panel, jsdom, the first frame before the ResizeObserver fires)
  // must still mount the row the scroll offset is in — never nothing.
  if (first === -1 && layout.rows.length > 0) {
    let best = layout.rows[0];
    let bestDistance = Infinity;
    for (const row of layout.rows) {
      const distance = scrollTop < row.y ? row.y - scrollTop : Math.max(0, scrollTop - (row.y + row.h));
      if (distance < bestDistance) {
        bestDistance = distance;
        best = row;
      }
    }
    first = best.index;
    last = best.index;
  }

  const pages: PageIndex[] = [];
  if (first >= 0) {
    for (let i = first; i <= last; i++) pages.push(...layout.rows[i].pages);
  }
  return { first, last, pages };
}

/** Rows actually on screen — what the tile priority and the viewport hint are computed from. */
export function onScreenRange(layout: DocLayout, scrollTop: number, viewportH: number): VisibleRange {
  return visibleRange(layout, scrollTop, viewportH, 0, 1);
}

/**
 * The page the status bar shows: the one covering the point 35 % down the viewport, which is what
 * every other reader does — the page you are reading, not the sliver at the top edge. In 두 쪽 both
 * pages of a spread share a row, and the spread is named by its first (left) page.
 */
export function currentPageAt(layout: DocLayout, scrollTop: number, viewportH: number): PageIndex {
  const probe = scrollTop + viewportH * 0.35;
  let best: LayoutRow | null = null;
  for (const row of layout.rows) {
    if (row.pages.length === 0) continue;
    if (best === null || row.y <= probe) best = row;
    else break;
  }
  return best ? best.pages[0] : 0;
}

/** The page ◀ / ▶, ⌘↑ / ⌘↓ and Space go to (a spread at a time in 두 쪽). */
export { stepPage } from "./stepPage";

/**
 * 분할 보기 동기화 스크롤 (P2): where a pane is, in **pages** — the page at the top edge of the
 * viewport plus how far down it (a spread of two counts as two pages). Page units, not pixels, so
 * two panes at different zooms move by the same amount of document.
 */
export function positionAt(layout: DocLayout, scrollTop: number, gap = PAGE_GAP): number {
  const rows = layout.rows;
  if (rows.length === 0) return 0;
  let row = rows[0];
  for (const r of rows) {
    if (r.y <= scrollTop) row = r;
    else break;
  }
  const span = Math.max(1, row.h + gap);
  const fraction = Math.min(1, Math.max(0, (scrollTop - row.y) / span));
  return row.pages[0] + fraction * row.pages.length;
}

/** The inverse of `positionAt`; `null` when the page is not laid out (단일 shows one page). */
export function scrollTopForPosition(layout: DocLayout, position: number, gap = PAGE_GAP): number | null {
  const item = layout.byPage.get(Math.max(0, Math.floor(position)));
  const row = item ? layout.rows[item.row] : undefined;
  if (!row) return null;
  const fraction = (position - row.pages[0]) / row.pages.length;
  return Math.max(0, row.y + fraction * (row.h + gap));
}

/** Scroll offset that puts a page at the top of the viewport (minus the top padding). */
export function scrollTopForPage(layout: DocLayout, page: PageIndex, padY = PAD_Y): number {
  const item = layout.byPage.get(page);
  if (!item) return 0;
  return Math.max(0, item.y - padY / 2);
}

/** 너비 맞춤 / 페이지 맞춤 as a percentage, once the scroller has a size. */
export function fitZoomPercent(
  page: PageGeom | undefined,
  mode: "fit-width" | "fit-page",
  rotation: Rotation,
  viewLayout: ViewLayout,
  viewport: Size,
  opts: { gap?: number; padX?: number; padY?: number; min?: number; max?: number } = {},
): number | null {
  if (!page || viewport.w <= 0) return null;
  const gap = opts.gap ?? PAGE_GAP;
  const padX = opts.padX ?? PAD_X;
  const padY = opts.padY ?? PAD_Y;
  const min = opts.min ?? 25;
  const max = opts.max ?? 400;
  const swap = rotation === 90 || rotation === 270;
  const wPt = swap ? page.heightPt : page.widthPt;
  const hPt = swap ? page.widthPt : page.heightPt;
  const columns = viewLayout === "two" || viewLayout === "twoCover" ? 2 : 1;
  const availW = Math.max(40, viewport.w - padX * 2 - (columns - 1) * gap) / columns;
  const availH = Math.max(40, viewport.h - padY * 2);
  const widthPct = (availW / wPt) * 100;
  const pct = mode === "fit-width" ? widthPct : Math.min(widthPct, (availH / hPt) * 100);
  return Math.max(min, Math.min(max, Math.round(pct)));
}

/**
 * 너비 맞춤 / 페이지 맞춤 for the whole view. 단일 fits the page it shows; 연속 and 두 쪽 fit the
 * most demanding page of the document (the widest, for 너비 맞춤), so the zoom never depends on
 * which page the scroll position happens to make current — with mixed portrait and landscape pages
 * that dependency is a feedback loop (refit → re-anchor → new current page → refit …).
 */
export function fitZoomForView(
  pages: PageGeom[],
  mode: "fit-width" | "fit-page",
  rotation: Rotation,
  viewLayout: ViewLayout,
  viewport: Size,
  currentPage: PageIndex,
): number | null {
  if (viewLayout === "single") return fitZoomPercent(pages[currentPage], mode, rotation, viewLayout, viewport);
  let best: number | null = null;
  const seen = new Set<string>();
  for (const page of pages) {
    const size = `${page.widthPt}x${page.heightPt}`;
    if (seen.has(size)) continue;
    seen.add(size);
    const fit = fitZoomPercent(page, mode, rotation, viewLayout, viewport);
    if (fit !== null && (best === null || fit < best)) best = fit;
  }
  return best;
}
