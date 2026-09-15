/**
 * 텍스트 상자 (F-11) — drag a box, or click for an auto-sized one, then type inline.
 *
 * The tool itself commits **nothing**: it hands the host a rectangle through `edit`, the host
 * opens a Korean-IME-safe editor over it (`src/annot/TextEditor.tsx`), and the annotation is
 * created with its text when the editor commits. Creating first and patching the text afterwards
 * would cost two undo steps and flash an empty box.
 */
import type { PageIndex, Point, Rect } from "../ipc/types";
import type { ToolModule, ToolResult } from "./ToolController";
import { rectFrom, rectIsEmpty } from "./geometry";

export interface TextBoxState {
  page: PageIndex | null;
  from: Point | null;
}

const EMPTY: TextBoxState = { page: null, from: null };

/** A click (no drag) opens a box this wide; it grows with the text while editing. */
export const AUTO_WIDTH_PT = 180;

export function autoRect(at: Point, fontSize: number): Rect {
  const height = Math.max(fontSize * 1.6, 18);
  return { l: at[0], t: at[1], r: at[0] + AUTO_WIDTH_PT, b: at[1] - height };
}

export const textBoxTool: ToolModule<TextBoxState> = {
  id: "textbox",
  cursor: "crosshair",
  init: () => ({ ...EMPTY }),

  onDown(_state, p): ToolResult<TextBoxState> {
    return { state: { page: p.page, from: p.pt }, preview: null };
  },

  onMove(state, p, ctx): ToolResult<TextBoxState> {
    if (!state.from || state.page === null) return { state };
    return {
      state,
      preview: {
        page: state.page,
        kind: "rect",
        rect: rectFrom(state.from, p.pt),
        color: ctx.style.color,
        fillColor: ctx.style.fillColor,
        opacity: 1,
        width: 1,
      },
    };
  },

  onUp(state, p, ctx): ToolResult<TextBoxState> {
    if (!state.from || state.page === null) return { state: { ...EMPTY }, preview: null };
    const dragged = rectFrom(state.from, p.pt);
    const rect = rectIsEmpty(dragged, 4) ? autoRect(state.from, ctx.style.fontSize) : dragged;
    return { state: { ...EMPTY }, preview: null, edit: { page: state.page, rect }, done: true };
  },

  onKey(state, key): ToolResult<TextBoxState> {
    if (key === "Escape") return { state: { ...EMPTY }, preview: null, done: true };
    return { state };
  },
};
