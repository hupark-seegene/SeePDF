/**
 * Pure logic behind 페이지 크기 변경 (P2): the form → the `resize_pages` target, in points. Custom
 * sizes are typed in millimetres (what Korean users think in; A4 = 210 × 297).
 */
import type { PageGeom, ResizeMode, ResizeTarget } from "../ipc/types";
import { MAX_SIDE_PT, mmToPt, ptToMm, visualSize, type PaperName } from "../organize/cropGeometry";

export type SizeChoice = PaperName | "custom";
export const SIZE_CHOICES: SizeChoice[] = ["A4", "Letter", "A3", "custom"];
export const RESIZE_MODES: ResizeMode[] = ["scaleContent", "centerContent"];

export interface ResizeForm {
  size: SizeChoice;
  /** custom width / height, millimetres as typed */
  widthMm: string;
  heightMm: string;
  mode: ResizeMode;
}

/** Largest custom side in mm (14 400 pt). */
export const MAX_SIDE_MM = Math.floor(ptToMm(MAX_SIDE_PT));

export function initialResizeForm(geom: PageGeom | undefined): ResizeForm {
  const seen = geom ? visualSize(geom) : { w: 595.28, h: 841.89 };
  return {
    size: "A4",
    widthMm: ptToMm(seen.w).toFixed(0),
    heightMm: ptToMm(seen.h).toFixed(0),
    mode: "scaleContent",
  };
}

/** The `size` argument, or `null` when a custom size is not a number in 1 … 5080 mm. */
export function resizeTarget(form: ResizeForm): ResizeTarget | null {
  if (form.size !== "custom") return form.size;
  const w = Number(form.widthMm.replace(",", "."));
  const h = Number(form.heightMm.replace(",", "."));
  const ok = (v: number) => Number.isFinite(v) && v >= 1 && v <= MAX_SIDE_MM;
  if (!ok(w) || !ok(h)) return null;
  return { w: Math.round(mmToPt(w) * 100) / 100, h: Math.round(mmToPt(h) * 100) / 100 };
}

/** `210 × 297` — a page's size as seen, in whole millimetres. */
export function sizeLabelMm(geom: PageGeom): { w: string; h: string } {
  const seen = visualSize(geom);
  return { w: ptToMm(seen.w).toFixed(0), h: ptToMm(seen.h).toFixed(0) };
}
