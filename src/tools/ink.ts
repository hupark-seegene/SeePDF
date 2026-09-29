/**
 * 펜 (F-09) — freehand ink. One gesture = one stroke; ⇧ constrains the stroke to the straight line
 * from where it started (UI_SPEC §6).
 *
 * v0.3 A4 — pen and touch:
 *
 * * **펜으로만 그리기** (`toolOptions.penOnly`, switched on by itself when a pen is first seen):
 *   a touch pointer does not draw (the surface pans the page instead, `ToolSurface.tsx`);
 * * **pressure** modulates the width. A PDF `/InkList` has one width per annotation, so the width
 *   is *baked*: a stroke of even pressure is one annotation at `pressureWidth(width, mean)`; a
 *   stroke whose pressure really varies becomes a few consecutive annotations, one per pressure
 *   band, sharing their joints (`inkSpecs`), created together by one `annotation_batch` — one
 *   undo step, selected together (`commits` → `ToolSink.commitMany`). A mouse (pressure 0.5 while pressed) draws at the
 *   style's width, as before;
 * * the surface feeds `getCoalescedEvents()` samples, so a fast pen stroke stays smooth.
 *
 * Raw samples are kept in the tool's own state (never in a store — ARCHITECTURE §10) and are
 * simplified + smoothed once, on pointer-up, so a 2-second scribble writes ~60 points instead of
 * ~600 and the annotation's `/InkList` stays small.
 */
import type { AnnotSpec } from "../ipc/types";
import type { PagePoint, ToolContext, ToolModule, ToolResult } from "./ToolController";
import { boundsOfPaths, smoothPath } from "./geometry";
import { touchPans } from "./toolOptions";

export interface InkState {
  page: number | null;
  /** flat `[x0,y0,x1,y1,…]` in PDF user space */
  points: number[];
  /** pressure per point (0..1); `null` = not a pen (a mouse or a test) */
  pressures: number[] | null;
}

/** Samples closer than this (points) to the previous one are dropped before smoothing. */
const MIN_STEP_PT = 0.7;
/** A stroke whose pressure spans less than this is drawn at one width. */
const EVEN_PRESSURE = 0.25;
/** Width bands of a stroke of varying pressure. */
const BANDS = 3;

const EMPTY: InkState = { page: null, points: [], pressures: null };

/** A4: the stroke width at `pressure` (0..1): 40 % of the style's width at a feather touch, 130 % pressed hard. */
export function pressureWidth(width: number, pressure: number): number {
  const p = Math.min(1, Math.max(0, pressure));
  return Math.round(width * (0.4 + 0.9 * p) * 100) / 100;
}

function penPressure(p: PagePoint): number | null {
  if (p.pointerType !== "pen" || p.pressure === undefined) return null;
  return p.pressure;
}

export function inkSpec(points: number[], ctx: ToolContext, width = ctx.style.width): AnnotSpec | null {
  const path = smoothPath(points);
  if (path.length < 4) return null;
  return {
    kind: "ink",
    paths: [path],
    color: ctx.style.color,
    width,
    opacity: ctx.style.opacity,
  };
}

/**
 * A4: the annotations one stroke becomes. Without pressure (mouse) or with an even pressure,
 * one; otherwise one per run of points in the same pressure band (runs of fewer than 3 points
 * join the previous run), consecutive runs sharing their joint so the stroke has no gaps.
 */
export function inkSpecs(points: number[], pressures: number[] | null, ctx: ToolContext): AnnotSpec[] {
  if (!pressures || pressures.length * 2 !== points.length) {
    const one = inkSpec(points, ctx);
    return one ? [one] : [];
  }
  const lo = Math.min(...pressures);
  const hi = Math.max(...pressures);
  if (hi - lo < EVEN_PRESSURE) {
    const mean = pressures.reduce((s, v) => s + v, 0) / pressures.length;
    const one = inkSpec(points, ctx, pressureWidth(ctx.style.width, mean));
    return one ? [one] : [];
  }
  const band = (v: number) => Math.min(BANDS - 1, Math.floor(((v - lo) / (hi - lo)) * BANDS));
  const runs: { band: number; start: number; end: number }[] = [];
  for (let i = 0; i < pressures.length; i++) {
    const b = band(pressures[i]);
    const last = runs[runs.length - 1];
    if (last && (last.band === b || last.end - last.start < 2)) last.end = i;
    else runs.push({ band: b, start: i, end: i });
  }
  const specs: AnnotSpec[] = [];
  for (const run of runs) {
    // share the joint with the next run
    const end = Math.min(run.end + 1, pressures.length - 1);
    const slice = points.slice(run.start * 2, end * 2 + 2);
    const mean = pressures.slice(run.start, end + 1).reduce((s, v) => s + v, 0) / (end - run.start + 1);
    const spec = inkSpec(slice, ctx, pressureWidth(ctx.style.width, mean));
    if (spec) specs.push(spec);
  }
  return specs;
}

export const inkTool: ToolModule<InkState> = {
  id: "pen",
  cursor: "crosshair",
  init: () => ({ ...EMPTY, points: [], pressures: null }),

  onDown(_state, p): ToolResult<InkState> {
    // A4: with 펜으로만 그리기 a finger does not draw (the surface pans instead)
    if (touchPans(p.pointerType)) return { state: { ...EMPTY, points: [] } };
    const pressure = penPressure(p);
    const state: InkState = { page: p.page, points: [p.pt[0], p.pt[1]], pressures: pressure === null ? null : [pressure] };
    return { state, preview: { page: p.page, kind: "ink", points: state.points } };
  },

  onMove(state, p, ctx): ToolResult<InkState> {
    if (state.page === null || p.page !== state.page) return { state };
    const n = state.points.length;
    let points: number[];
    let pressures = state.pressures;
    const pressure = penPressure(p);
    if (ctx.modifiers.shift) {
      // ⇧ = a straight line from the anchor: replace everything after the first sample.
      points = [state.points[0], state.points[1], p.pt[0], p.pt[1]];
      if (pressures) pressures = [pressures[0], pressure ?? pressures[0]];
    } else {
      const dx = p.pt[0] - state.points[n - 2];
      const dy = p.pt[1] - state.points[n - 1];
      if (Math.hypot(dx, dy) < MIN_STEP_PT) return { state };
      points = [...state.points, p.pt[0], p.pt[1]];
      if (pressures) pressures = [...pressures, pressure ?? pressures[pressures.length - 1]];
    }
    const next: InkState = { page: state.page, points, pressures };
    const width = pressures ? pressureWidth(ctx.style.width, pressures[pressures.length - 1]) : ctx.style.width;
    return { state: next, preview: { page: state.page, kind: "ink", points, color: ctx.style.color, width, opacity: ctx.style.opacity } };
  },

  onUp(state, p, ctx): ToolResult<InkState> {
    if (state.page === null) return { state: { ...EMPTY }, preview: null };
    const straight = ctx.modifiers.shift;
    const points = straight ? [state.points[0], state.points[1], p.pt[0], p.pt[1]] : state.points;
    const pressures = straight && state.pressures ? [state.pressures[0], state.pressures[state.pressures.length - 1]] : state.pressures;
    const specs = inkSpecs(points, pressures, ctx);
    if (specs.length === 0) return { state: { ...EMPTY }, preview: null };
    if (specs.length === 1) return { state: { ...EMPTY }, preview: null, commit: { page: state.page, spec: specs[0] } };
    const page = state.page;
    return { state: { ...EMPTY }, preview: null, commits: specs.map((spec) => ({ page, spec })) };
  },

  onKey(state, key): ToolResult<InkState> {
    if (key === "Escape") return { state: { ...EMPTY }, preview: null };
    return { state };
  },
};

export { boundsOfPaths as inkBounds };
