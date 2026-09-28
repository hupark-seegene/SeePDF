/**
 * P2 자르기 / 페이지 크기 변경 — the page geometry both the dialogs and the mock adapter need, the
 * frontend twin of `src-tauri/src/engine/pages/boxes.rs`. Pure; unit-tested.
 *
 * "Visual" space is the page as SEEN: the crop box with width and height swapped for `/Rotate`
 * 90 / 270, origin bottom-left, y up — the space `stamp.rs` lays stamps out in. Margins are always
 * visual (위 is the top edge on screen whatever the rotation); the engine maps them into the page's
 * unrotated user space with the same matrix as `visualToUser` below.
 */
import type { Margins, PageGeom, Rect, ResizeTarget } from "../ipc/types";

/** Portrait width × height in points (`PaperName::size_pt`). */
export const PAPER = {
  A4: [595.28, 841.89],
  Letter: [612, 792],
  A3: [841.89, 1190.55],
} as const satisfies Record<string, readonly [number, number]>;
export type PaperName = keyof typeof PAPER;

export const PT_PER_MM = 72 / 25.4;
/** Smallest side a crop or a page may have, in points (`boxes::MIN_SIDE`). */
export const MIN_SIDE_PT = 1;
/** Largest page side PDF allows, in points (`boxes::MAX_SIDE`). */
export const MAX_SIDE_PT = 14_400;

export function ptToMm(pt: number): number {
  return pt / PT_PER_MM;
}

export function mmToPt(mm: number): number {
  return mm * PT_PER_MM;
}

/** Width × height of the page as seen. */
export function visualSize(g: Pick<PageGeom, "rotation" | "crop">): { w: number; h: number } {
  const cw = Math.abs(g.crop.r - g.crop.l);
  const ch = Math.abs(g.crop.t - g.crop.b);
  return g.rotation % 180 === 90 ? { w: ch, h: cw } : { w: cw, h: ch };
}

/** Visual `(u, v)` → unrotated user space for `/Rotate` = `rotation` (`stamp::visual_to_user`). */
export function visualToUser(rotation: number, crop: Rect, u: number, v: number): [number, number] {
  switch (((rotation % 360) + 360) % 360) {
    case 90:
      return [crop.r - v, crop.b + u];
    case 180:
      return [crop.r - u, crop.t - v];
    case 270:
      return [crop.l + v, crop.t - u];
    default:
      return [crop.l + u, crop.b + v];
  }
}

/** Margins (as seen) inset from `crop`, as a user-space rect — `null` when less than 1 pt is left. */
export function marginsToUser(rotation: number, crop: Rect, m: Margins): Rect | null {
  const { w, h } = visualSize({ rotation: rotation as PageGeom["rotation"], crop });
  const u0 = m.left;
  const v0 = m.bottom;
  const u1 = w - m.right;
  const v1 = h - m.top;
  if (u1 - u0 < MIN_SIDE_PT || v1 - v0 < MIN_SIDE_PT) return null;
  const [x0, y0] = visualToUser(rotation, crop, u0, v0);
  const [x1, y1] = visualToUser(rotation, crop, u1, v1);
  return { l: Math.min(x0, x1), b: Math.min(y0, y1), r: Math.max(x0, x1), t: Math.max(y0, y1) };
}

/** The size (as seen) a resize gives this page: a named size keeps the page's orientation. */
export function resizedVisualSize(g: Pick<PageGeom, "rotation" | "crop">, target: ResizeTarget): { w: number; h: number } {
  if (typeof target !== "string") return { w: target.w, h: target.h };
  const [pw, ph] = PAPER[target];
  const { w, h } = visualSize(g);
  return w > h ? { w: ph, h: pw } : { w: pw, h: ph };
}

/** The same size in unrotated user space. */
export function visualToUserSize(rotation: number, w: number, h: number): { w: number; h: number } {
  return rotation % 180 === 90 ? { w: h, h: w } : { w, h };
}
