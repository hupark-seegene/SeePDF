/**
 * 선 and 화살표 — F-10. v0.3 (A2): the engine writes a real `/Line` with lopdf (`/L`, `/LE
 * /OpenArrow` for the heads, its own appearance), so other viewers see a line with arrow heads;
 * only an encrypted document still gets SeePDF's Ink line (`/Subj SeePDF:Line` / `SeePDF:Arrow`).
 * With 측정 on (`toolOptions.measure`) the line is labelled with its length in mm or pt.
 *
 * ⇧ snaps the far end to 15° (UI_SPEC §6).
 */
import type { AnnotSpec, PageIndex, Point } from "../ipc/types";
import type { ToolModule, ToolResult } from "./ToolController";
import { measureLabel, snapAngle } from "./geometry";
import { toolOptions } from "./toolOptions";

export interface LineState {
  page: PageIndex | null;
  from: Point | null;
}

const EMPTY: LineState = { page: null, from: null };
/** Shorter than this (points) and it was a click, not a line. */
const MIN_LENGTH_PT = 3;

export function makeLineTool(id: "line" | "arrow"): ToolModule<LineState> {
  return {
    id,
    cursor: "crosshair",
    init: () => ({ ...EMPTY }),

    onDown(_state, p): ToolResult<LineState> {
      return { state: { page: p.page, from: p.pt }, preview: null };
    },

    onMove(state, p, ctx): ToolResult<LineState> {
      if (!state.from || state.page === null) return { state };
      const to = ctx.modifiers.shift ? snapAngle(state.from, p.pt) : p.pt;
      const measure = toolOptions().measure;
      return {
        state,
        preview: {
          page: state.page,
          kind: "line",
          points: [state.from[0], state.from[1], to[0], to[1]],
          heads: id === "arrow" ? ctx.style.heads : [false, false],
          color: ctx.style.color,
          opacity: ctx.style.opacity,
          width: ctx.style.width,
          measureText:
            measure === "off" ? undefined : measureLabel(Math.hypot(to[0] - state.from[0], to[1] - state.from[1]), measure, false),
        },
      };
    },

    onUp(state, p, ctx): ToolResult<LineState> {
      if (!state.from || state.page === null) return { state: { ...EMPTY }, preview: null };
      const to = ctx.modifiers.shift ? snapAngle(state.from, p.pt) : p.pt;
      if (Math.hypot(to[0] - state.from[0], to[1] - state.from[1]) < MIN_LENGTH_PT) {
        return { state: { ...EMPTY }, preview: null };
      }
      const measure = toolOptions().measure;
      const spec: AnnotSpec = {
        kind: id,
        p1: state.from,
        p2: to,
        color: ctx.style.color,
        width: ctx.style.width,
        opacity: ctx.style.opacity,
        heads: id === "arrow" ? ctx.style.heads : undefined,
        ...(measure !== "off" ? { measure } : {}),
      };
      return { state: { ...EMPTY }, preview: null, commit: { page: state.page, spec } };
    },

    onKey(state, key): ToolResult<LineState> {
      if (key === "Escape") return { state: { ...EMPTY }, preview: null };
      return { state };
    },
  };
}
