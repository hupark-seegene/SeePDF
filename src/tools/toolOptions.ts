/**
 * v0.3 pkg4-annotations-stamps-objects — tool options that are not part of a tool's *style*
 * (colour, width…) and so do not live in `annotStore.style`:
 *
 * * 지우개 — 전체 / 부분 (A3): whole strokes (default) or only what the circle covers;
 * * 펜 — 펜으로만 그리기 (A4): touch pans and only a pen or a mouse draws. Turned on by itself the
 *   first time a pen is seen (a Surface / tablet user never has to find the setting); remembered;
 * * 다각형 — 다각형 / 꺾은선 / 구름 (A2);
 * * 선 / 화살표 / 다각형 — 측정 (A2): label the length / area in mm or pt.
 *
 * A tiny external store (no zustand: the tool modules stay React-free and this file stays out of
 * the entry chunk — only lazy chunks import it). The pen-only choice is kept in `localStorage`
 * (a per-device preference; it never needs to reach another device or the engine).
 */
export type EraserMode = "whole" | "partial";
export type PolygonShape = "polygon" | "polyline" | "cloud";
export type MeasureOption = "off" | "mm" | "pt";

export interface ToolOptions {
  eraserMode: EraserMode;
  penOnly: boolean;
  /** a pen has been seen this session (so 펜으로만 그리기 was switched on automatically once) */
  penSeen: boolean;
  polygonShape: PolygonShape;
  measure: MeasureOption;
}

const PEN_ONLY_KEY = "seepdf.penOnly";

function readPenOnly(): boolean {
  try {
    return globalThis.localStorage?.getItem(PEN_ONLY_KEY) === "1";
  } catch {
    return false;
  }
}

function writePenOnly(on: boolean): void {
  try {
    globalThis.localStorage?.setItem(PEN_ONLY_KEY, on ? "1" : "0");
  } catch {
    // private mode / blocked storage: the choice lasts for this session only
  }
}

let options: ToolOptions = { eraserMode: "whole", penOnly: readPenOnly(), penSeen: false, polygonShape: "polygon", measure: "off" };
const listeners = new Set<() => void>();

export function toolOptions(): ToolOptions {
  return options;
}

export function setToolOptions(patch: Partial<ToolOptions>): void {
  options = { ...options, ...patch };
  if (patch.penOnly !== undefined) writePenOnly(patch.penOnly);
  for (const l of listeners) l();
}

export function subscribeToolOptions(listener: () => void): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

/**
 * A4: called for every pointer that reaches the drawing surface. The first pen turns
 * 펜으로만 그리기 on (once per session — switching it off again is respected).
 */
export function notePointer(pointerType: string | undefined): void {
  if (pointerType === "pen" && !options.penSeen) setToolOptions({ penSeen: true, penOnly: true });
}

/** A4: a touch pointer while 펜으로만 그리기 is on pans instead of drawing. */
export function touchPans(pointerType: string | undefined): boolean {
  return pointerType === "touch" && options.penOnly;
}

/** Test seam. */
export function resetToolOptions(): void {
  options = { eraserMode: "whole", penOnly: false, penSeen: false, polygonShape: "polygon", measure: "off" };
  for (const l of listeners) l();
}
