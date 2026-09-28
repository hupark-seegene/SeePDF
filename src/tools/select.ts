/**
 * 선택 inside 주석 mode (UI_SPEC §6: "click selects an object, drag moves, handles resize").
 *
 * It is a tool module like every other one, which keeps it out of React: the annotations it works
 * on arrive through `ctx.annots(page)` and the current selection through `ctx.selected`, and what
 * it produces is `select` / `patch` results rather than an `AnnotSpec`.
 *
 * Move and resize emit `patch … live: true` on every pointer move — the sink coalesces those into
 * a single `update_annotation` (and therefore a single undo step) and flushes on pointer-up.
 */
import type { Annot, AnnotId, AnnotPatch, PageIndex, Point } from "../ipc/types";
import type { ToolModule, ToolResult } from "./ToolController";
import { rectFrom } from "./geometry";
import { annotAt, annotsIn, handleAt, isMovable, isResizable, movePatch, resizePatch, resizeRect, type HandleId } from "./hit";

export interface SelectState {
  mode: "idle" | "press" | "move" | "resize" | "marquee";
  page: PageIndex | null;
  from: Point | null;
  /** the annotations the gesture started on, captured so a move is always relative to the origin */
  targets: Annot[];
  handle: HandleId | null;
  /** the selection as it was before a marquee started (⇧ adds to it) */
  base: AnnotId[];
  moved: boolean;
}

const EMPTY: SelectState = { mode: "idle", page: null, from: null, targets: [], handle: null, base: [], moved: false };

/** Grab radius / handle radius, CSS px — divided by `ctx.scale` to reach points. */
export const GRAB_PX = 6;
export const HANDLE_PX = 7;
/** A drag under this many points is a click, not a move. */
const DRAG_SLOP_PT = 1.5;

export const selectTool: ToolModule<SelectState> = {
  id: "select",
  cursor: "default",
  init: () => ({ ...EMPTY, targets: [], base: [] }),

  onDown(_state, p, ctx): ToolResult<SelectState> {
    const annots = ctx.annots?.(p.page) ?? [];
    const selected = ctx.selected ?? [];
    const grab = GRAB_PX / ctx.scale;
    const handleGrab = HANDLE_PX / ctx.scale;

    // A handle of an already-selected annotation wins over everything below it.
    for (const id of selected) {
      const a = annots.find((x) => x.id === id);
      if (!a || !isResizable(a)) continue;
      const handle = handleAt(a, p.pt[0], p.pt[1], handleGrab);
      if (handle && handle !== "body") {
        return {
          state: { ...EMPTY, mode: "resize", page: p.page, from: p.pt, targets: [a], handle, base: selected },
          preview: null,
        };
      }
    }

    const hit = annotAt(annots, p.pt[0], p.pt[1], grab);
    if (hit) {
      const additive = ctx.modifiers.shift || ctx.modifiers.meta;
      const inSelection = selected.includes(hit.id);
      const ids = additive
        ? inSelection ? selected.filter((x) => x !== hit.id) : [...selected, hit.id]
        : inSelection ? selected : [hit.id];
      const targets = annots.filter((a) => ids.includes(a.id) && isMovable(a));
      return {
        state: { ...EMPTY, mode: "press", page: p.page, from: p.pt, targets, base: ids },
        preview: null,
        select: { ids },
      };
    }

    const base = ctx.modifiers.shift ? selected : [];
    return {
      state: { ...EMPTY, mode: "marquee", page: p.page, from: p.pt, base },
      preview: { page: p.page, kind: "marquee", rect: rectFrom(p.pt, p.pt) },
      select: ctx.modifiers.shift ? undefined : { ids: [] },
    };
  },

  onMove(state, p, ctx): ToolResult<SelectState> {
    if (state.page === null || !state.from) return { state };
    const dx = p.pt[0] - state.from[0];
    const dy = p.pt[1] - state.from[1];

    if (state.mode === "marquee") {
      const rect = rectFrom(state.from, p.pt);
      const ids = annotsIn(ctx.annots?.(state.page) ?? [], rect);
      const merged = [...new Set([...state.base, ...ids])];
      return { state, preview: { page: state.page, kind: "marquee", rect, ids: merged }, select: { ids: merged } };
    }

    if (state.mode === "resize" && state.handle && state.targets[0]) {
      // like a move, a resize starts only past the slop: a jitter on a handle is still a click
      if (!state.moved && Math.hypot(dx, dy) <= DRAG_SLOP_PT / ctx.scale) return { state };
      const target = state.targets[0];
      const rect = resizeRect(target.rect, state.handle, p.pt[0], p.pt[1], ctx.modifiers.shift, 6 / ctx.scale);
      return {
        state: { ...state, moved: true },
        patch: { page: state.page, edits: [{ id: target.id, patch: resizePatch(target, rect) }], live: true },
      };
    }

    const moving = state.mode === "move" || Math.hypot(dx, dy) > DRAG_SLOP_PT / ctx.scale;
    if (!moving || state.targets.length === 0) return { state };
    return {
      state: { ...state, mode: "move", moved: true },
      patch: { page: state.page, edits: movePatches(state.targets, dx, dy), live: true },
    };
  },

  onUp(state, p, ctx): ToolResult<SelectState> {
    if (state.page === null || !state.from) return { state: { ...EMPTY }, preview: null };
    const dx = p.pt[0] - state.from[0];
    const dy = p.pt[1] - state.from[1];

    if (state.mode === "marquee") {
      const rect = rectFrom(state.from, p.pt);
      const ids = annotsIn(ctx.annots?.(state.page) ?? [], rect);
      const merged = [...new Set([...state.base, ...ids])];
      return { state: { ...EMPTY }, preview: null, select: { ids: merged } };
    }

    if (state.mode === "resize" && state.handle && state.targets[0]) {
      // A press on a handle that never became a drag changes nothing: no update_annotation, so
      // no 주석 편집 undo step and no dirty document for a mis-click.
      if (!state.moved && Math.hypot(dx, dy) <= DRAG_SLOP_PT / ctx.scale) return { state: { ...EMPTY }, preview: null };
      const target = state.targets[0];
      const rect = resizeRect(target.rect, state.handle, p.pt[0], p.pt[1], ctx.modifiers.shift, 6 / ctx.scale);
      return {
        state: { ...EMPTY },
        preview: null,
        patch: { page: state.page, edits: [{ id: target.id, patch: resizePatch(target, rect) }], live: false },
      };
    }

    if (state.moved && state.targets.length) {
      return {
        state: { ...EMPTY },
        preview: null,
        patch: { page: state.page, edits: movePatches(state.targets, dx, dy), live: false },
      };
    }

    // A plain click on a 메모 or a 텍스트 상자 opens its editor (UI_SPEC §12 "메모 열기").
    const target = state.targets[0];
    if (target && (target.kind === "note" || target.kind === "textbox")) {
      return { state: { ...EMPTY }, preview: null, edit: { page: state.page, id: target.id } };
    }
    return { state: { ...EMPTY }, preview: null };
  },

  onKey(state, key): ToolResult<SelectState> {
    if (key === "Escape") return { state: { ...EMPTY }, preview: null, select: { ids: [] } };
    return { state };
  },
};

/** A multi-selection move patches every target with the same delta, in one coalesced call. */
export function movePatches(targets: Annot[], dx: number, dy: number): { id: AnnotId; patch: AnnotPatch }[] {
  return targets.map((a) => ({ id: a.id, patch: movePatch(a, dx, dy) }));
}
