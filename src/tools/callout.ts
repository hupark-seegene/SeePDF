/**
 * 설명선 (v0.3 A2) — the callout variant of the 텍스트 상자: press where the arrow should point,
 * drag to where the box goes, release, type. The box is the text box's auto-sized one at the
 * release point; the leader runs from the press point to the nearer side of the box
 * (`FreeText /IT /FreeTextCallout /CL`, written by the engine with lopdf). A click without a
 * drag puts the box up and to the right of the tip.
 *
 * Like the text box it commits nothing itself: it hands the host the rectangle **and the
 * leader** through `edit`, and the editor creates the annotation with its text.
 */
import type { PageIndex, Point, Rect } from "../ipc/types";
import type { ToolModule, ToolResult } from "./ToolController";
import { autoRect } from "./textbox";

export interface CalloutState {
  page: PageIndex | null;
  tip: Point | null;
}

const EMPTY: CalloutState = { page: null, tip: null };
/** A drag shorter than this is a click. */
const CLICK_PT = 8;
/** Where a clicked callout's box goes, relative to the tip. */
const CLICK_OFFSET: Point = [36, 36];

/** The box at `at` and the leader from `tip` to the middle of its nearer vertical side. */
export function calloutGeometry(tip: Point, at: Point, fontSize: number): { rect: Rect; callout: number[] } {
  const rect = autoRect(at, fontSize);
  const midY = (rect.b + rect.t) / 2;
  const x = tip[0] <= (rect.l + rect.r) / 2 ? rect.l : rect.r;
  return { rect, callout: [tip[0], tip[1], x, midY] };
}

export const calloutTool: ToolModule<CalloutState> = {
  id: "callout",
  cursor: "crosshair",
  init: () => ({ ...EMPTY }),

  onDown(_state, p): ToolResult<CalloutState> {
    return { state: { page: p.page, tip: p.pt }, preview: null };
  },

  onMove(state, p, ctx): ToolResult<CalloutState> {
    if (!state.tip || state.page === null) return { state };
    const { rect, callout } = calloutGeometry(state.tip, p.pt, ctx.style.fontSize);
    return {
      state,
      preview: { page: state.page, kind: "callout", rect, points: callout, color: ctx.style.color, fillColor: ctx.style.fillColor, width: 1 },
    };
  },

  onUp(state, p, ctx): ToolResult<CalloutState> {
    if (!state.tip || state.page === null) return { state: { ...EMPTY }, preview: null };
    const dragged = Math.hypot(p.pt[0] - state.tip[0], p.pt[1] - state.tip[1]) >= CLICK_PT;
    const at: Point = dragged ? p.pt : [state.tip[0] + CLICK_OFFSET[0], state.tip[1] + CLICK_OFFSET[1]];
    const { rect, callout } = calloutGeometry(state.tip, at, ctx.style.fontSize);
    return { state: { ...EMPTY }, preview: null, edit: { page: state.page, rect, callout }, done: true };
  },

  onKey(state, key): ToolResult<CalloutState> {
    if (key === "Escape") return { state: { ...EMPTY }, preview: null, done: true };
    return { state };
  },
};
