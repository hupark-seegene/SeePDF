/**
 * 메모 (F-12) — a sticky note. One click places a `Text` annotation and opens its popover focused
 * (UI_SPEC §6); the contents are written on blur through `update_annotation`, which is why the
 * spec is committed with an empty `contents` rather than waiting for the text (the ghost has to
 * appear within a frame).
 *
 * The click is the note's **top-left**: PDFium draws its own note icon into the 22 × 22 pt rect,
 * and the engine derives that rect from `at` (IPC_CONTRACT §7.1).
 */
import type { PageIndex } from "../ipc/types";
import type { ToolModule, ToolResult } from "./ToolController";

export interface NoteState {
  page: PageIndex | null;
}

export const NOTE_SIZE_PT = 22;

export const noteTool: ToolModule<NoteState> = {
  id: "note",
  cursor: "copy",
  init: () => ({ page: null }),

  onDown(_state, p): ToolResult<NoteState> {
    return { state: { page: p.page } };
  },

  onMove(state): ToolResult<NoteState> {
    return { state };
  },

  onUp(state, p, ctx): ToolResult<NoteState> {
    const page = state.page ?? p.page;
    return {
      state: { page: null },
      preview: null,
      commit: { page, spec: { kind: "note", at: p.pt, color: ctx.style.color, contents: "" } },
      // back to 선택 so the freshly placed note can be dragged straight away (UI_SPEC §6)
      done: true,
    };
  },

  onKey(state, key): ToolResult<NoteState> {
    if (key === "Escape") return { state: { page: null }, preview: null, done: true };
    return { state };
  },
};
