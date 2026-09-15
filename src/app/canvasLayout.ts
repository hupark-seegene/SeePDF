/**
 * Layout maths for the canvas, kept out of the component file so React Fast Refresh stays clean.
 * Stage 1 (c) moves this into `src/viewer/layout.ts` together with the virtualised scroller.
 */
import type { PageGeom, Rotation, ViewLayout } from "../ipc/types";

/** CSS size of one page box at a zoom percentage, with the view rotation applied. */
export function displaySize(page: PageGeom, zoomPercent: number, rotation: Rotation): { w: number; h: number } {
  const swap = rotation === 90 || rotation === 270;
  const wPt = swap ? page.heightPt : page.widthPt;
  const hPt = swap ? page.widthPt : page.heightPt;
  const s = zoomPercent / 100;
  return { w: Math.round(wPt * s), h: Math.round(hPt * s) };
}

/** 너비 맞춤 / 페이지 맞춤 resolve to a percentage once the scroller has a size. */
export function resolveZoom(
  page: PageGeom | undefined,
  zoomMode: string,
  zoomPercent: number,
  rotation: Rotation,
  layout: ViewLayout,
  box: { w: number; h: number },
): number {
  if (!page || (zoomMode !== "fit-width" && zoomMode !== "fit-page") || box.w === 0) return zoomPercent;
  const swap = rotation === 90 || rotation === 270;
  const wPt = swap ? page.heightPt : page.widthPt;
  const hPt = swap ? page.widthPt : page.heightPt;
  const columns = layout === "two" ? 2 : 1;
  const padding = 48;
  const availableW = Math.max(120, box.w - padding - (columns - 1) * 16) / columns;
  const availableH = Math.max(120, box.h - padding);
  const pct = zoomMode === "fit-width"
    ? (availableW / wPt) * 100
    : Math.min((availableW / wPt) * 100, (availableH / hPt) * 100);
  return Math.max(25, Math.min(400, Math.round(pct)));
}
