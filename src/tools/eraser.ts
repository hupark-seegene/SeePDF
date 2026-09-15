/**
 * 지우개 (F-09) — removes **whole ink strokes** it touches, never parts of one: a PDF `/InkList`
 * is a set of strokes, and splitting one would mean rewriting the annotation's appearance stream.
 * Dragging accumulates the ids and deletes them in one `delete_annotations` call, so a scrub over
 * five strokes is one undo step.
 *
 * The circle the cursor draws is `style.eraserSize` in points (UI_SPEC §7 "지우개 크기").
 */
import type { Annot, AnnotId, PageIndex } from "../ipc/types";
import type { PagePoint, ToolModule, ToolResult } from "./ToolController";
import { distanceToPath, rectContains } from "./geometry";

export interface EraserState {
  page: PageIndex | null;
  hit: AnnotId[];
}

const EMPTY: EraserState = { page: null, hit: [] };

/** Which annotations a `radius`-wide dab at (x, y) touches. Ink by path, everything else by rect. */
export function eraserHits(annots: Annot[], x: number, y: number, radius: number): AnnotId[] {
  const out: AnnotId[] = [];
  for (const a of annots) {
    if (a.locked || a.editable === "readOnly") continue;
    if (a.inkPaths?.length) {
      if (a.inkPaths.some((path) => distanceToPath(path, x, y) <= radius + a.borderWidth / 2)) out.push(a.id);
      continue;
    }
    if (a.quads?.length) {
      if (a.quads.some((q) => rectContains(q, x, y, radius))) out.push(a.id);
      continue;
    }
    if (rectContains(a.rect, x, y, radius)) out.push(a.id);
  }
  return out;
}

export const eraserTool: ToolModule<EraserState> = {
  id: "eraser",
  cursor: "none",
  hoverPreview: true,
  init: () => ({ ...EMPTY }),

  onDown(_state, p, ctx): ToolResult<EraserState> {
    const radius = ctx.style.eraserSize / 2;
    const hit = eraserHits(ctx.annots?.(p.page) ?? [], p.pt[0], p.pt[1], radius);
    return {
      state: { page: p.page, hit },
      preview: { page: p.page, kind: "eraser", rect: dab(p, radius), ids: hit },
      erase: hit.length ? { page: p.page, ids: hit } : undefined,
    };
  },

  onMove(state, p, ctx): ToolResult<EraserState> {
    const radius = ctx.style.eraserSize / 2;
    const preview = { page: p.page, kind: "eraser" as const, rect: dab(p, radius) };
    if (state.page === null || state.page !== p.page) return { state, preview };
    const hit = eraserHits(ctx.annots?.(p.page) ?? [], p.pt[0], p.pt[1], radius).filter(
      (id) => !state.hit.includes(id),
    );
    if (hit.length === 0) return { state, preview: { ...preview, ids: state.hit } };
    const next = { page: state.page, hit: [...state.hit, ...hit] };
    return { state: next, preview: { ...preview, ids: next.hit }, erase: { page: p.page, ids: hit } };
  },

  onUp(): ToolResult<EraserState> {
    return { state: { ...EMPTY, hit: [] }, preview: null };
  },

  onKey(state, key): ToolResult<EraserState> {
    // Esc reverts to 펜, not to 선택 (UI_SPEC §6).
    if (key === "Escape") return { state: { ...EMPTY, hit: [] }, preview: null, done: true };
    return { state };
  },
};

function dab(p: PagePoint, radius: number) {
  return { l: p.pt[0] - radius, r: p.pt[0] + radius, b: p.pt[1] - radius, t: p.pt[1] + radius };
}
