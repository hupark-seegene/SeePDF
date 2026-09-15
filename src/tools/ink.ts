/**
 * 펜 (F-09) — freehand ink. One gesture = one `AnnotSpec` with one stroke; ⇧ constrains the stroke
 * to the straight line from where it started (UI_SPEC §6). Pressure is deliberately ignored: the
 * PDF `/InkList` has no pressure channel and a variable-width AP would not survive a re-render.
 *
 * Raw samples are kept in the tool's own state (never in a store — ARCHITECTURE §10) and are
 * simplified + smoothed once, on pointer-up, so a 2-second scribble writes ~60 points instead of
 * ~600 and the annotation's `/InkList` stays small.
 */
import type { AnnotSpec } from "../ipc/types";
import type { ToolContext, ToolModule, ToolResult } from "./ToolController";
import { boundsOfPaths, smoothPath } from "./geometry";

export interface InkState {
  page: number | null;
  /** flat `[x0,y0,x1,y1,…]` in PDF user space */
  points: number[];
}

/** Samples closer than this (points) to the previous one are dropped before smoothing. */
const MIN_STEP_PT = 0.7;

const EMPTY: InkState = { page: null, points: [] };

export function inkSpec(points: number[], ctx: ToolContext): AnnotSpec | null {
  const path = smoothPath(points);
  if (path.length < 4) return null;
  return {
    kind: "ink",
    paths: [path],
    color: ctx.style.color,
    width: ctx.style.width,
    opacity: ctx.style.opacity,
  };
}

export const inkTool: ToolModule<InkState> = {
  id: "pen",
  cursor: "crosshair",
  init: () => ({ ...EMPTY, points: [] }),

  onDown(_state, p): ToolResult<InkState> {
    const state: InkState = { page: p.page, points: [p.pt[0], p.pt[1]] };
    return { state, preview: { page: p.page, kind: "ink", points: state.points } };
  },

  onMove(state, p, ctx): ToolResult<InkState> {
    if (state.page === null || p.page !== state.page) return { state };
    const n = state.points.length;
    let points: number[];
    if (ctx.modifiers.shift) {
      // ⇧ = a straight line from the anchor: replace everything after the first sample.
      points = [state.points[0], state.points[1], p.pt[0], p.pt[1]];
    } else {
      const dx = p.pt[0] - state.points[n - 2];
      const dy = p.pt[1] - state.points[n - 1];
      if (Math.hypot(dx, dy) < MIN_STEP_PT) return { state };
      points = [...state.points, p.pt[0], p.pt[1]];
    }
    const next: InkState = { page: state.page, points };
    return { state: next, preview: { page: state.page, kind: "ink", points, color: ctx.style.color, width: ctx.style.width, opacity: ctx.style.opacity } };
  },

  onUp(state, p, ctx): ToolResult<InkState> {
    if (state.page === null) return { state: { ...EMPTY }, preview: null };
    const points = ctx.modifiers.shift
      ? [state.points[0], state.points[1], p.pt[0], p.pt[1]]
      : state.points;
    const spec = inkSpec(points, ctx);
    if (!spec) return { state: { ...EMPTY }, preview: null };
    return { state: { ...EMPTY }, preview: null, commit: { page: state.page, spec } };
  },

  onKey(state, key): ToolResult<InkState> {
    if (key === "Escape") return { state: { ...EMPTY }, preview: null };
    return { state };
  },
};

export { boundsOfPaths as inkBounds };
