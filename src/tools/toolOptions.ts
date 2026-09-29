/**
 * v0.3 pkg4-annotations-stamps-objects — tool options that are not part of a tool's *style*
 * (colour, width…) and so do not live in `annotStore.style`:
 *
 * * 지우개 — 전체 / 부분 (A3): whole strokes (default) or only what the circle covers;
 * * 펜 — 펜으로만 그리기 (A4): touch pans and only a pen or a mouse draws. Turned on by itself the
 *   first time a pen is seen on this device (a Surface / tablet user never has to find the
 *   setting) — only while no choice is stored: once it is on or off in `localStorage`, that
 *   choice wins over every later pen, across restarts;
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
  /**
   * 펜으로만 그리기 has been decided — switched on by the first pen, or set by the user, now or
   * in an earlier session (a stored `seepdf.penOnly`): a pen no longer switches it on.
   */
  penSeen: boolean;
  polygonShape: PolygonShape;
  measure: MeasureOption;
}

const PEN_ONLY_KEY = "seepdf.penOnly";

/** The stored choice: `null` when none was ever stored on this device. */
function readPenOnly(): boolean | null {
  try {
    const v = globalThis.localStorage?.getItem(PEN_ONLY_KEY);
    return v === "1" ? true : v === "0" ? false : null;
  } catch {
    return null;
  }
}

function writePenOnly(on: boolean): void {
  try {
    globalThis.localStorage?.setItem(PEN_ONLY_KEY, on ? "1" : "0");
  } catch {
    // private mode / blocked storage: the choice lasts for this session only
  }
}

function initialOptions(): ToolOptions {
  const stored = readPenOnly();
  // v0.3 pkg4 (round 2): a stored choice — the auto-on of an earlier session, or the user's
  // own on / off — counts as decided, so the first pen of a new session does not override it.
  return { eraserMode: "whole", penOnly: stored ?? false, penSeen: stored !== null, polygonShape: "polygon", measure: "off" };
}

let options: ToolOptions = initialOptions();
const listeners = new Set<() => void>();

export function toolOptions(): ToolOptions {
  return options;
}

export function setToolOptions(patch: Partial<ToolOptions>): void {
  options = { ...options, ...patch };
  if (patch.penOnly !== undefined) {
    writePenOnly(patch.penOnly);
    // the user's own choice is a decision too: a later pen this session leaves it alone
    options.penSeen = true;
  }
  for (const l of listeners) l();
}

export function subscribeToolOptions(listener: () => void): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

/**
 * A4: called for every pointer that reaches the drawing surface. The first pen ever seen turns
 * 펜으로만 그리기 on — only while nothing is decided (see [`ToolOptions.penSeen`]): switching it
 * off again is respected, in this session and every later one.
 */
export function notePointer(pointerType: string | undefined): void {
  if (pointerType === "pen" && !options.penSeen) setToolOptions({ penSeen: true, penOnly: true });
}

/** A4: a touch pointer while 펜으로만 그리기 is on pans instead of drawing. */
export function touchPans(pointerType: string | undefined): boolean {
  return pointerType === "touch" && options.penOnly;
}

/** Test seam: a fresh device (nothing stored). */
export function resetToolOptions(): void {
  try {
    globalThis.localStorage?.removeItem(PEN_ONLY_KEY);
  } catch {
    // blocked storage: nothing stored anyway
  }
  options = initialOptions();
  for (const l of listeners) l();
}
