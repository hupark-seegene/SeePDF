/**
 * The pixel arithmetic of the viewer — the frontend half of
 * `src-tauri/src/engine/render/geometry.rs` (`ARCHITECTURE.md` §3.1).
 *
 * Every number here must agree with the engine, because the frontend lays out the page box and
 * the engine paints the bitmap that fills it:
 *
 *   * page pixel size is `round(fround(points) × s)` — pdfium-render's own arithmetic;
 *   * whole-page bitmaps while `s ≤ 2` and `w×h ≤ 2.5 Mpx`, 512 px tiles above that;
 *   * `scaleKey = round(zoomPercent × devicePixelRatio)`, `s = scaleKey / 100`, ceiling 800.
 *
 * `pageToDevice` is the contract of `IPC_CONTRACT.md` §3 verbatim: crop box + `/Rotate` + view
 * rotation + scale, row-vector convention (`dx = a·x + c·y + e`). It is **frozen** — (d) and (e)
 * build every hit-test and every handle on it.
 */
import type { Mat6, PageGeom, Point, Rect, Rotation } from "../ipc/types";

/** Device-pixel edge of one tile (IPC_CONTRACT §9, `geometry::TILE`). */
export const TILE_PX = 512;
/** Whole-page bitmaps are used while `s ≤ 2` **and** the page is at most this many pixels. */
export const MAX_WHOLE_PAGE_PX = 2_500_000;
/** Above this device scale the viewer switches to tiles. */
export const MAX_WHOLE_PAGE_SCALE = 2;
/** `scaleKey` ceiling; above it the frontend CSS-upscales. */
export const MAX_SCALE_KEY = 800;
/** Placeholder width in points — `s_ph = min(1, 640 / w_pt)` (ARCHITECTURE §3.1). */
export const PLACEHOLDER_PT = 640;

export interface Size {
  w: number;
  h: number;
}

export interface Box {
  x: number;
  y: number;
  w: number;
  h: number;
}

function clamp(v: number, lo: number, hi: number): number {
  return v < lo ? lo : v > hi ? hi : v;
}

/** `scaleKey = round(zoomPercent × devicePixelRatio)`, clamped to the engine's ceiling. */
export function scaleKeyFor(zoomPercent: number, dpr = devicePixelRatio()): number {
  return clamp(Math.round(zoomPercent * dpr), 1, MAX_SCALE_KEY);
}

/** The device scale the engine will actually render at for a `scaleKey`. */
export function scaleFromKey(scaleKey: number): number {
  return scaleKey / 100;
}

export function devicePixelRatio(): number {
  if (typeof window === "undefined") return 1;
  return window.devicePixelRatio || 1;
}

/** Display size of a page under a view rotation, in points (the intrinsic `/Rotate` is already in `widthPt`). */
export function displaySizePt(page: PageGeom, viewRotation: Rotation): Size {
  const swap = viewRotation === 90 || viewRotation === 270;
  return {
    w: Math.fround(swap ? page.heightPt : page.widthPt),
    h: Math.fround(swap ? page.widthPt : page.heightPt),
  };
}

/** Pixel size of a whole page at device scale `s` — `round(fround(pt) × s)`, exactly the engine. */
export function pagePixels(page: PageGeom, viewRotation: Rotation, s: number): Size {
  const pt = displaySizePt(page, viewRotation);
  return {
    w: Math.max(1, Math.round(pt.w * s)),
    h: Math.max(1, Math.round(pt.h * s)),
  };
}

/**
 * CSS size of the page box: the device bitmap divided by the display density, so a tile mosaic
 * covers the box exactly (no seams, no 1 px white edge).
 */
export function pageBoxCss(page: PageGeom, viewRotation: Rotation, scaleKey: number, dpr = devicePixelRatio()): Size {
  const px = pagePixels(page, viewRotation, scaleFromKey(scaleKey));
  return { w: px.w / dpr, h: px.h / dpr };
}

/** Whole page, or tiles? (ARCHITECTURE D5 / `geometry::should_tile`) */
export function shouldTile(s: number, widthPx: number, heightPx: number): boolean {
  return s > MAX_WHOLE_PAGE_SCALE || widthPx * heightPx > MAX_WHOLE_PAGE_PX;
}

/** Number of tiles across and down for a page of `widthPx × heightPx`. */
export function tileGridFor(widthPx: number, heightPx: number, tile = TILE_PX): { cols: number; rows: number } {
  return { cols: Math.max(1, Math.ceil(widthPx / tile)), rows: Math.max(1, Math.ceil(heightPx / tile)) };
}

/** Origin and size of tile `(tx, ty)` clipped to the page, or `null` when it is off the page. */
export function tileRect(widthPx: number, heightPx: number, tx: number, ty: number, tile = TILE_PX): Box | null {
  const x = tx * tile;
  const y = ty * tile;
  if (x < 0 || y < 0 || x >= widthPx || y >= heightPx) return null;
  return { x, y, w: Math.min(tile, widthPx - x), h: Math.min(tile, heightPx - y) };
}

/** Chebyshev distance from the viewport centre — the centre-out request order of ARCHITECTURE §3.3. */
export function tilePriority(tx: number, ty: number, centreTx: number, centreTy: number): number {
  return Math.max(Math.abs(tx - centreTx), Math.abs(ty - centreTy));
}

/** `scaleKey` of the low-resolution whole-page placeholder: `s = min(1, 640 / w_pt)` (ARCHITECTURE §3.1). */
export function placeholderScaleKey(page: PageGeom, viewRotation: Rotation): number {
  const pt = displaySizePt(page, viewRotation);
  const s = Math.min(1, PLACEHOLDER_PT / Math.max(1, pt.w));
  return Math.max(1, Math.round(s * 100));
}

// ---------------------------------------------------------------------------
// The affine — IPC_CONTRACT §3 (frozen; cross-tested against `geometry::page_to_device`)
// ---------------------------------------------------------------------------

/**
 * Page user space → device px (y-down) at scale `s`.
 * `dx = a·x + c·y + e ; dy = b·x + d·y + f` (row-vector convention).
 */
export function pageToDevice(g: PageGeom, viewRotation: Rotation, s: number): Mat6 {
  const deg = (g.rotation + viewRotation) % 360;
  const { l, b, r, t } = g.crop;
  switch (deg) {
    case 90:
      return [0, s, s, 0, -b * s, -l * s];
    case 180:
      return [-s, 0, 0, s, r * s, -b * s];
    case 270:
      return [0, -s, -s, 0, t * s, r * s];
    default:
      return [s, 0, 0, -s, -l * s, t * s];
  }
}

/** The inverse affine: device px → page user space. */
export function deviceToPage(m: Mat6): Mat6 {
  const [a, b, c, d, e, f] = m;
  const det = a * d - b * c;
  if (det === 0) return [1, 0, 0, 1, 0, 0];
  return [d / det, -b / det, -c / det, a / det, (c * f - d * e) / det, (b * e - a * f) / det];
}

/** Apply an affine to a point (row-vector convention). */
export function applyMat(m: Mat6, x: number, y: number): Point {
  return [m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5]];
}

/** Device box of a PDF user-space rectangle (y-up, unrotated) under `m`. */
export function rectToBox(rect: Rect, m: Mat6): Box {
  const [x0, y0] = applyMat(m, rect.l, rect.b);
  const [x1, y1] = applyMat(m, rect.r, rect.t);
  return { x: Math.min(x0, x1), y: Math.min(y0, y1), w: Math.abs(x1 - x0), h: Math.abs(y1 - y0) };
}

/** Does a user-space rectangle contain a point? (rects are y-up, `b ≤ t`). */
export function rectContains(rect: Rect, x: number, y: number): boolean {
  return x >= rect.l && x <= rect.r && y >= rect.b && y <= rect.t;
}

/** Union of two user-space rectangles; `null` operands are ignored. */
export function unionRect(a: Rect | null, b: Rect): Rect {
  if (!a) return { ...b };
  return { l: Math.min(a.l, b.l), b: Math.min(a.b, b.b), r: Math.max(a.r, b.r), t: Math.max(a.t, b.t) };
}

/** Is the rectangle empty (zero area — pdfium's generated whitespace boxes are)? */
export function isEmptyRect(rect: Rect): boolean {
  return rect.r - rect.l <= 0 || rect.t - rect.b <= 0;
}
