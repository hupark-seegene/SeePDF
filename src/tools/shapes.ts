/**
 * 사각형 (Square) and 타원 (Circle) — F-10. Drag a rectangle; ⇧ makes it square/circular and ⌥
 * grows it from the point the drag started (UI_SPEC §6).
 *
 * `fillColor: null` means "no interior colour"; the engine writes `/IC` only when it is set, and
 * the same alpha reaches both `/C` and `/IC` because every `FPDFAnnot_SetColor` rewrites `/CA`
 * (spikes/annotations §2).
 */
import type { AnnotSpec, PageIndex, Point } from "../ipc/types";
import type { ToolModule, ToolResult } from "./ToolController";
import { rectFrom, rectIsEmpty } from "./geometry";

export type ShapeKind = "square" | "circle";

export interface ShapeState {
  page: PageIndex | null;
  from: Point | null;
}

const EMPTY: ShapeState = { page: null, from: null };

/** A click with no drag still deserves a shape: 24 × 24 pt around the point. */
const CLICK_SIZE_PT = 24;

export function makeShapeTool(id: "rectangle" | "ellipse"): ToolModule<ShapeState> {
  const kind: ShapeKind = id === "rectangle" ? "square" : "circle";
  return {
    id,
    cursor: "crosshair",
    init: () => ({ ...EMPTY }),

    onDown(_state, p, ctx): ToolResult<ShapeState> {
      const state: ShapeState = { page: p.page, from: p.pt };
      return {
        state,
        preview: {
          page: p.page,
          kind: kind === "square" ? "rect" : "ellipse",
          rect: rectFrom(p.pt, p.pt),
          color: ctx.style.color,
          fillColor: ctx.style.fillColor,
          opacity: ctx.style.opacity,
          width: ctx.style.width,
        },
      };
    },

    onMove(state, p, ctx): ToolResult<ShapeState> {
      if (!state.from || state.page === null) return { state };
      const rect = rectFrom(state.from, p.pt, { square: ctx.modifiers.shift, fromCentre: ctx.modifiers.alt });
      return {
        state,
        preview: {
          page: state.page,
          kind: kind === "square" ? "rect" : "ellipse",
          rect,
          color: ctx.style.color,
          fillColor: ctx.style.fillColor,
          opacity: ctx.style.opacity,
          width: ctx.style.width,
        },
      };
    },

    onUp(state, p, ctx): ToolResult<ShapeState> {
      if (!state.from || state.page === null) return { state: { ...EMPTY }, preview: null };
      let rect = rectFrom(state.from, p.pt, { square: ctx.modifiers.shift, fromCentre: ctx.modifiers.alt });
      if (rectIsEmpty(rect, 2)) {
        const half = CLICK_SIZE_PT / 2;
        rect = { l: state.from[0] - half, b: state.from[1] - half, r: state.from[0] + half, t: state.from[1] + half };
      }
      const spec: AnnotSpec = {
        kind,
        rect,
        color: ctx.style.color,
        fillColor: ctx.style.fillColor,
        width: ctx.style.width,
        opacity: ctx.style.opacity,
      };
      return { state: { ...EMPTY }, preview: null, commit: { page: state.page, spec } };
    },

    onKey(state, key): ToolResult<ShapeState> {
      if (key === "Escape") return { state: { ...EMPTY }, preview: null };
      return { state };
    },
  };
}
