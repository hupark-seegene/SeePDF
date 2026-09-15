/**
 * 형광펜 · 밑줄 · 취소선 · 물결선 (F-08) — the four text-anchored tools.
 *
 * These four do **not** own the drag: the viewer already turns a pointer drag into a text
 * selection for them (`Scroller.selectsText`), and the text layer already produces one rectangle
 * per line run, which is exactly the shape `AnnotSpec.rects` wants — the engine turns each into a
 * `FS_QUADPOINTSF` in TL,TR,BL,BR order (IPC_CONTRACT §3, spikes/annotations §1.4).
 *
 * So the state machine is: on down, remember the page; on up, ask the host for the selection
 * rectangles of that page and commit. `ctx.selectionRects` is injected (by the annotation host
 * from `useSelectionStore` + the text layer) so the machine stays pure and testable.
 */
import type { AnnotSpec, PageIndex, Rect } from "../ipc/types";
import type { ToolContext, ToolModule, ToolResult } from "./ToolController";
import { boundsOfRects, normalizeRect } from "./geometry";

export type MarkupKind = "highlight" | "underline" | "strikeout" | "squiggly";

export interface MarkupState {
  page: PageIndex | null;
}

/** Drop the zero-area rectangles pdfium's generated whitespace produces. */
export function usableRects(rects: Rect[]): Rect[] {
  return rects.map(normalizeRect).filter((r) => r.r - r.l > 0.5 && r.t - r.b > 0.5);
}

export function markupSpec(kind: MarkupKind, rects: Rect[], ctx: ToolContext): AnnotSpec | null {
  const usable = usableRects(rects);
  if (usable.length === 0) return null;
  return {
    kind,
    rects: usable,
    color: ctx.style.color,
    // 형광펜 multiplies, so it keeps the swatch's alpha column; the three line markups are opaque
    // strokes and would look muddy at .4 (UI_SPEC §14.4).
    opacity: kind === "highlight" ? ctx.style.opacity : 1,
  };
}

export function makeMarkupTool(kind: MarkupKind): ToolModule<MarkupState> {
  return {
    id: kind,
    cursor: "text",
    init: () => ({ page: null }),

    onDown(_state, p): ToolResult<MarkupState> {
      return { state: { page: p.page }, preview: null };
    },

    onMove(state, p, ctx): ToolResult<MarkupState> {
      // The viewer already paints its own selection rectangles during the drag; this preview only
      // adds the tool's colour on top of them, so the user sees the highlight before releasing.
      const page = state.page ?? p.page;
      const rects = usableRects(ctx.selectionRects?.(page) ?? []);
      return {
        state,
        preview: rects.length
          ? { page, kind: "quads", quads: rects, color: ctx.style.color, opacity: kind === "highlight" ? ctx.style.opacity : 1 }
          : null,
      };
    },

    onUp(state, p, ctx): ToolResult<MarkupState> {
      const page = state.page ?? p.page;
      const rects = ctx.selectionRects?.(page) ?? [];
      const spec = markupSpec(kind, rects, ctx);
      if (!spec) return { state: { page: null }, preview: null };
      return { state: { page: null }, preview: null, commit: { page, spec } };
    },

    onKey(state, key): ToolResult<MarkupState> {
      if (key === "Escape") return { state: { page: null }, preview: null, done: true };
      return { state };
    },
  };
}

/** The bounding box a markup annotation gets — the union of its quads (IPC_CONTRACT §7.1). */
export { boundsOfRects as markupBounds };
