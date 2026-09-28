/**
 * Pure logic behind the 자르기 tool (P2): the crop box is edited as a rectangle on the page AS SEEN,
 * in points with the origin at the TOP-left (y down, like the screen), and sent to the engine as
 * margins (`set_page_boxes { crop: { margins } }`), which apply to every target page relative to its
 * own crop box. Kept out of the component so every drag and the auto-detect are unit-testable.
 */
import type { Margins } from "../ipc/types";

/** A rectangle on the visual page, in points, origin top-left. */
export interface CropBox { x: number; y: number; w: number; h: number }

/** What a pointer drag grabs: an edge, a corner, or the inside (`move`). */
export type CropHandle = "n" | "s" | "e" | "w" | "ne" | "nw" | "se" | "sw" | "move";

export const HANDLES: Exclude<CropHandle, "move">[] = ["nw", "n", "ne", "e", "se", "s", "sw", "w"];

/** The smallest box a drag may leave, in points (bigger than the engine's 1 pt, so it stays grabbable). */
export const MIN_BOX_PT = 18;

export function fullBox(pageW: number, pageH: number): CropBox {
  return { x: 0, y: 0, w: pageW, h: pageH };
}

export function boxFromMargins(m: Margins, pageW: number, pageH: number): CropBox {
  return clampBox({ x: m.left, y: m.top, w: pageW - m.left - m.right, h: pageH - m.top - m.bottom }, pageW, pageH);
}

export function marginsFromBox(b: CropBox, pageW: number, pageH: number): Margins {
  const r = (v: number) => Math.max(0, Math.round(v * 10) / 10);
  return { top: r(b.y), left: r(b.x), right: r(pageW - b.x - b.w), bottom: r(pageH - b.y - b.h) };
}

/** True when the box is the whole page (nothing to crop). */
export function isFullPage(m: Margins): boolean {
  return m.top < 0.05 && m.right < 0.05 && m.bottom < 0.05 && m.left < 0.05;
}

/** Keeps the box on the page and at least `min` points on each side. */
export function clampBox(b: CropBox, pageW: number, pageH: number, min = Math.min(MIN_BOX_PT, pageW, pageH)): CropBox {
  const w = Math.min(pageW, Math.max(min, b.w));
  const h = Math.min(pageH, Math.max(min, b.h));
  const x = Math.min(pageW - w, Math.max(0, b.x));
  const y = Math.min(pageH - h, Math.max(0, b.y));
  return { x, y, w, h };
}

/**
 * The box after dragging `handle` by `(dx, dy)` points from `start`. Edges stop at the page and at the
 * minimum size — they never flip past the opposite edge; `move` slides the box, clamped to the page.
 */
export function dragBox(
  start: CropBox,
  handle: CropHandle,
  dx: number,
  dy: number,
  pageW: number,
  pageH: number,
  min = Math.min(MIN_BOX_PT, pageW, pageH),
): CropBox {
  if (handle === "move") {
    return {
      x: Math.min(pageW - start.w, Math.max(0, start.x + dx)),
      y: Math.min(pageH - start.h, Math.max(0, start.y + dy)),
      w: start.w,
      h: start.h,
    };
  }
  let left = start.x;
  let top = start.y;
  let right = start.x + start.w;
  let bottom = start.y + start.h;
  if (handle.includes("w")) left = Math.min(right - min, Math.max(0, left + dx));
  if (handle.includes("e")) right = Math.max(left + min, Math.min(pageW, right + dx));
  if (handle.includes("n")) top = Math.min(bottom - min, Math.max(0, top + dy));
  if (handle.includes("s")) bottom = Math.max(top + min, Math.min(pageH, bottom + dy));
  return { x: left, y: top, w: right - left, h: bottom - top };
}

/** A box drawn from `(x0, y0)` to `(x1, y1)` (any direction), clamped to the page. */
export function boxFromPoints(x0: number, y0: number, x1: number, y1: number, pageW: number, pageH: number): CropBox {
  const l = Math.max(0, Math.min(x0, x1));
  const t = Math.max(0, Math.min(y0, y1));
  const r = Math.min(pageW, Math.max(x0, x1));
  const b = Math.min(pageH, Math.max(y0, y1));
  return { x: l, y: t, w: Math.max(0, r - l), h: Math.max(0, b - t) };
}

export interface PixelBounds { x0: number; y0: number; x1: number; y1: number }

/**
 * The bounding box of every pixel that is not (near-)white paper, in pixels (`x1` / `y1` exclusive),
 * or `null` for a blank page. `threshold`: a channel above it counts as paper; transparent pixels
 * count as paper too. RGBA, straight alpha, top-left origin — `render_page_raw`'s format.
 */
export function contentBounds(
  pixels: Uint8ClampedArray | Uint8Array,
  width: number,
  height: number,
  stride = width * 4,
  threshold = 235,
): PixelBounds | null {
  const ink = (o: number) => pixels[o + 3] > 16 && (pixels[o] < threshold || pixels[o + 1] < threshold || pixels[o + 2] < threshold);
  let y0 = -1;
  let y1 = -1;
  let x0 = width;
  let x1 = -1;
  for (let y = 0; y < height; y++) {
    const row = y * stride;
    let first = -1;
    let last = -1;
    for (let x = 0; x < width; x++) {
      if (ink(row + x * 4)) {
        if (first < 0) first = x;
        last = x;
      }
    }
    if (first < 0) continue;
    if (y0 < 0) y0 = y;
    y1 = y;
    if (first < x0) x0 = first;
    if (last > x1) x1 = last;
  }
  if (y0 < 0) return null;
  return { x0, y0, x1: x1 + 1, y1: y1 + 1 };
}

/**
 * 여백 자동 감지: the content bounds of a render at `scale` (device px per point) as a crop box on the
 * visual page, grown by `padPt` on each side and kept on the page. `null` for a blank page.
 */
export function detectCropBox(
  raw: { width: number; height: number; stride: number; pixels: Uint8ClampedArray | Uint8Array },
  scale: number,
  pageW: number,
  pageH: number,
  padPt = 6,
): CropBox | null {
  const b = contentBounds(raw.pixels, raw.width, raw.height, raw.stride);
  if (!b) return null;
  const box = {
    x: b.x0 / scale - padPt,
    y: b.y0 / scale - padPt,
    w: (b.x1 - b.x0) / scale + 2 * padPt,
    h: (b.y1 - b.y0) / scale + 2 * padPt,
  };
  // grow into the page rather than past it
  const l = Math.max(0, box.x);
  const t = Math.max(0, box.y);
  const r = Math.min(pageW, box.x + box.w);
  const btm = Math.min(pageH, box.y + box.h);
  return clampBox({ x: l, y: t, w: r - l, h: btm - t }, pageW, pageH);
}
