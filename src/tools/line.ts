/**
 * 선 and 화살표 — F-10. PDFium cannot create a `/Line` annotation (`FPDFPage_CreateAnnot` returns
 * NULL for it, spikes/annotations §2), so the engine writes both as **Ink with a `/Subj`**
 * (`SeePDF:Line` / `SeePDF:Arrow`) and reads them back as lines. Other viewers show ink; the UI
 * says so once through `annot.lineAsInk`.
 *
 * ⇧ snaps the far end to 15° (UI_SPEC §6).
 */
import type { AnnotSpec, PageIndex, Point } from "../ipc/types";
import type { ToolModule, ToolResult } from "./ToolController";
import { snapAngle } from "./geometry";

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
        },
      };
    },

    onUp(state, p, ctx): ToolResult<LineState> {
      if (!state.from || state.page === null) return { state: { ...EMPTY }, preview: null };
      const to = ctx.modifiers.shift ? snapAngle(state.from, p.pt) : p.pt;
      if (Math.hypot(to[0] - state.from[0], to[1] - state.from[1]) < MIN_LENGTH_PT) {
        return { state: { ...EMPTY }, preview: null };
      }
      const spec: AnnotSpec = {
        kind: id,
        p1: state.from,
        p2: to,
        color: ctx.style.color,
        width: ctx.style.width,
        opacity: ctx.style.opacity,
        heads: id === "arrow" ? ctx.style.heads : undefined,
      };
      return { state: { ...EMPTY }, preview: null, commit: { page: state.page, spec } };
    },

    onKey(state, key): ToolResult<LineState> {
      if (key === "Escape") return { state: { ...EMPTY }, preview: null };
      return { state };
    },
  };
}
