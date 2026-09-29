/**
 * 다각형 (v0.3 A2) — click per vertex, double-click (a second click on the last vertex), a click
 * on the first vertex or ↵ to finish; ⌫ removes the last vertex, Esc cancels.
 *
 * What it makes depends on `toolOptions.polygonShape`: a closed `/Polygon`, an open `/PolyLine`
 * (꺾은선) or a cloudy polygon (구름, `/BE /S /C`) — all written by the engine with lopdf, each with
 * its own appearance. With 측정 on the polygon is labelled with its area, a polyline with its
 * length (mm or pt).
 *
 * Unlike the drag tools this one lives across several pointer gestures, so it keeps its state
 * between them (`keepsState`) and previews the rubber band while the pointer hovers.
 */
import type { AnnotSpec, PageIndex, Point } from "../ipc/types";
import type { ToolContext, ToolModule, ToolResult, ToolPreview } from "./ToolController";
import { measureLabel, pathLength, polygonArea } from "./geometry";
import { toolOptions } from "./toolOptions";

export interface PolygonState {
  page: PageIndex | null;
  /** flat `[x0,y0,x1,y1,…]`, PDF user space */
  vertices: number[];
  /** where the pointer hovers — the rubber band's far end */
  cursor: Point | null;
}

const EMPTY: PolygonState = { page: null, vertices: [], cursor: null };

/** A second click this close (points) to the last vertex is the double-click that finishes. */
export const FINISH_SLOP_PT = 3;
/** A click this close (CSS px) to the first vertex closes the polygon. */
export const CLOSE_PX = 8;

function near(a: Point, x: number, y: number, tolerance: number): boolean {
  return Math.hypot(a[0] - x, a[1] - y) <= tolerance;
}

/** The spec a finished set of vertices becomes, `null` when there are too few. */
export function polygonSpec(vertices: number[], ctx: ToolContext): AnnotSpec | null {
  const { polygonShape, measure } = toolOptions();
  const open = polygonShape === "polyline";
  const points = vertices.length / 2;
  if (points < (open ? 2 : 3)) return null;
  return {
    kind: open ? "polyline" : "polygon",
    vertices: [...vertices],
    color: ctx.style.color,
    fillColor: open ? null : ctx.style.fillColor,
    width: ctx.style.width,
    opacity: ctx.style.opacity,
    ...(polygonShape === "cloud" ? { cloudy: true } : {}),
    ...(measure !== "off" ? { measure } : {}),
  };
}

function preview(state: PolygonState, ctx: ToolContext): ToolPreview | null {
  if (state.page === null) return null;
  const { polygonShape, measure } = toolOptions();
  const points = state.cursor ? [...state.vertices, state.cursor[0], state.cursor[1]] : state.vertices;
  const closed = polygonShape !== "polyline";
  return {
    page: state.page,
    kind: "poly",
    points,
    closed,
    cloudy: polygonShape === "cloud",
    color: ctx.style.color,
    fillColor: closed ? ctx.style.fillColor : null,
    width: ctx.style.width,
    opacity: ctx.style.opacity,
    measureText:
      measure === "off" || points.length < 4
        ? undefined
        : closed
          ? measureLabel(polygonArea(points), measure, true)
          : measureLabel(pathLength(points), measure, false),
  };
}

function finish(state: PolygonState, ctx: ToolContext): ToolResult<PolygonState> {
  const spec = state.page === null ? null : polygonSpec(state.vertices, ctx);
  if (!spec || state.page === null) return { state: { ...EMPTY, vertices: [] }, preview: null };
  return { state: { ...EMPTY, vertices: [] }, preview: null, commit: { page: state.page, spec } };
}

export const polygonTool: ToolModule<PolygonState> = {
  id: "polygon",
  cursor: "crosshair",
  hoverPreview: true,
  keepsState: true,
  init: () => ({ ...EMPTY, vertices: [] }),

  onDown(state, p, ctx): ToolResult<PolygonState> {
    const [x, y] = p.pt;
    if (state.page === null || state.page !== p.page) {
      const next: PolygonState = { page: p.page, vertices: [x, y], cursor: null };
      return { state: next, preview: preview(next, ctx) };
    }
    const n = state.vertices.length;
    const last: Point = [state.vertices[n - 2], state.vertices[n - 1]];
    const first: Point = [state.vertices[0], state.vertices[1]];
    // the second click of a double-click lands on the vertex the first one made
    if (near(last, x, y, FINISH_SLOP_PT)) return finish(state, ctx);
    if (n >= 6 && near(first, x, y, CLOSE_PX / ctx.scale)) return finish(state, ctx);
    const next: PolygonState = { ...state, vertices: [...state.vertices, x, y], cursor: null };
    return { state: next, preview: preview(next, ctx) };
  },

  onMove(state, p, ctx): ToolResult<PolygonState> {
    if (state.page === null || state.page !== p.page) return { state };
    const next = { ...state, cursor: p.pt };
    return { state: next, preview: preview(next, ctx) };
  },

  onUp(state): ToolResult<PolygonState> {
    return { state };
  },

  onKey(state, key, ctx): ToolResult<PolygonState> {
    if (key === "Escape") return { state: { ...EMPTY, vertices: [] }, preview: null };
    if (key === "Enter") return finish(state, ctx);
    if (key === "Backspace" && state.vertices.length > 2) {
      const next = { ...state, vertices: state.vertices.slice(0, -2) };
      return { state: next, preview: preview(next, ctx) };
    }
    return { state };
  },
};

/** `true` while a polygon is being drawn — the host routes ↵ / ⌫ / Esc to it then. */
export function polygonInProgress(state: unknown): boolean {
  return !!state && (state as PolygonState).page !== null && (state as PolygonState).vertices?.length > 0;
}
