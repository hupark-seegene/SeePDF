/**
 * Zoom: the ladder, the wheel/pinch response curve, and the anchor that keeps the point under the
 * cursor still (F-02: "the point under the cursor stays within 2 px across a pinch").
 *
 * The anchor is stored in *page* space (page index + fraction of the page box), not in content
 * pixels, so it survives a relayout that also re-centres the row — which is exactly what happens
 * when the content stops being narrower than the viewport.
 */
import type { PageIndex } from "../ipc/types";
import { MAX_ZOOM, MIN_ZOOM, ZOOM_STEPS } from "../store/viewStore";
import type { DocLayout } from "./layout";

export interface Vec2 {
  x: number;
  y: number;
}

export interface ViewAnchor {
  page: PageIndex;
  /** fraction of the page box, 0..1 (clamped outside the page, so margins still anchor sanely) */
  fx: number;
  fy: number;
}

export function clampZoom(percent: number): number {
  return Math.max(MIN_ZOOM, Math.min(MAX_ZOOM, Math.round(percent)));
}

export function nextZoomStep(percent: number): number {
  return clampZoom(ZOOM_STEPS.find((s) => s > percent + 0.5) ?? MAX_ZOOM);
}

export function previousZoomStep(percent: number): number {
  return clampZoom([...ZOOM_STEPS].reverse().find((s) => s < percent - 0.5) ?? MIN_ZOOM);
}

/**
 * ctrl/⌘ + wheel and trackpad pinch (which WKWebView and WebView2 both deliver as a wheel event
 * with `ctrlKey`). Exponential so the feel is the same at 25 % and at 400 %.
 */
export function zoomForWheel(zoomPercent: number, deltaY: number, deltaMode = 0): number {
  // 0 = pixels, 1 = lines, 2 = pages
  const px = deltaMode === 1 ? deltaY * 16 : deltaMode === 2 ? deltaY * 400 : deltaY;
  const factor = Math.exp(-px * 0.01);
  return clampZoom(zoomPercent * factor);
}

/** The page-space point under a viewport point, for the zoom that is about to happen. */
export function anchorAt(layout: DocLayout, scroll: Vec2, cursor: Vec2): ViewAnchor | null {
  if (layout.items.length === 0) return null;
  const cx = scroll.x + cursor.x;
  const cy = scroll.y + cursor.y;
  let best = layout.items[0];
  let bestDistance = Infinity;
  for (const item of layout.items) {
    const dx = cx < item.x ? item.x - cx : cx > item.x + item.w ? cx - (item.x + item.w) : 0;
    const dy = cy < item.y ? item.y - cy : cy > item.y + item.h ? cy - (item.y + item.h) : 0;
    const distance = dx * dx + dy * dy;
    if (distance < bestDistance) {
      bestDistance = distance;
      best = item;
      if (distance === 0) break;
    }
  }
  return {
    page: best.page,
    fx: best.w > 0 ? (cx - best.x) / best.w : 0,
    fy: best.h > 0 ? (cy - best.y) / best.h : 0,
  };
}

/** Scroll offsets that put `anchor` back under `cursor` in a freshly computed layout. */
export function scrollForAnchor(
  layout: DocLayout,
  anchor: ViewAnchor,
  cursor: Vec2,
  viewport: { w: number; h: number },
): Vec2 {
  const item = layout.byPage.get(anchor.page) ?? layout.items[0];
  if (!item) return { x: 0, y: 0 };
  const maxX = Math.max(0, layout.width - viewport.w);
  const maxY = Math.max(0, layout.height - viewport.h);
  return {
    x: Math.max(0, Math.min(maxX, item.x + anchor.fx * item.w - cursor.x)),
    y: Math.max(0, Math.min(maxY, item.y + anchor.fy * item.h - cursor.y)),
  };
}

/**
 * Scroll offset for a *single* axis when the content simply scales about the scroll origin —
 * the maths behind a zoom step that has no cursor (⌘+ / ⌘−, the slider), where the viewport
 * centre is the anchor.
 */
export function anchoredScroll(a: {
  scroll: number;
  anchor: number;
  originBefore: number;
  originAfter: number;
  ratio: number;
  max: number;
}): number {
  const content = a.scroll + a.anchor - a.originBefore;
  return Math.max(0, Math.min(a.max, a.originAfter + content * a.ratio - a.anchor));
}
