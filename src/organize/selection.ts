/**
 * Page-grid selection arithmetic (UI_SPEC §9): click · ⇧click range · ⌘click toggle · marquee.
 * Pure functions, no store, no React — `organize.selection` tests them directly.
 */
import type { PageIndex } from "../ipc/types";

export interface SelectionState {
  selected: PageIndex[];
  /** the last page the user touched; ⇧click ranges run from here */
  anchor: PageIndex | null;
}

export interface ClickMods {
  /** ⇧ — extend from the anchor */
  shift?: boolean;
  /** ⌘ on macOS / Ctrl on Windows — toggle one cell */
  meta?: boolean;
}

/** Ascending, unique. Every selection in the app is stored in this shape. */
export function normalizeSelection(pages: readonly PageIndex[]): PageIndex[] {
  return [...new Set(pages)].sort((a, b) => a - b);
}

export function rangeBetween(a: PageIndex, b: PageIndex): PageIndex[] {
  const [from, to] = a <= b ? [a, b] : [b, a];
  const out: PageIndex[] = [];
  for (let p = from; p <= to; p++) out.push(p);
  return out;
}

/**
 * One click on `page`. ⇧ extends from the anchor (⇧⌘ unions with what is already selected),
 * ⌘ toggles, a plain click replaces the selection.
 */
export function clickSelect(state: SelectionState, page: PageIndex, mods: ClickMods = {}): SelectionState {
  if (mods.shift) {
    const anchor = state.anchor ?? page;
    const range = rangeBetween(anchor, page);
    const selected = mods.meta ? normalizeSelection([...state.selected, ...range]) : range;
    // the anchor survives a range extension, so ⇧click again re-ranges from the same place
    return { selected, anchor };
  }
  if (mods.meta) {
    const has = state.selected.includes(page);
    const selected = has ? state.selected.filter((p) => p !== page) : normalizeSelection([...state.selected, page]);
    return { selected, anchor: page };
  }
  return { selected: [page], anchor: page };
}

/**
 * Pointer-down on a cell before a possible drag: a plain press inside an existing multi-selection
 * keeps it (so the whole block can be dragged) and only collapses on pointer-up without a drag.
 */
export function pressSelect(state: SelectionState, page: PageIndex, mods: ClickMods = {}): SelectionState {
  if (!mods.shift && !mods.meta && state.selected.includes(page)) return { ...state, anchor: page };
  return clickSelect(state, page, mods);
}

/** Marquee over empty space: `hits` replaces the selection, or unions with it while ⌘/⇧ is held. */
export function marqueeSelect(base: SelectionState, hits: readonly PageIndex[], additive: boolean): SelectionState {
  const selected = additive ? normalizeSelection([...base.selected, ...hits]) : normalizeSelection(hits);
  return { selected, anchor: selected[selected.length - 1] ?? base.anchor };
}

export interface Box {
  x: number;
  y: number;
  w: number;
  h: number;
}

export function boxesIntersect(a: Box, b: Box): boolean {
  return !(b.x > a.x + a.w || b.x + b.w < a.x || b.y > a.y + a.h || b.y + b.h < a.y);
}

/** Normalised rectangle from two pointer positions. */
export function boxFromPoints(x0: number, y0: number, x1: number, y1: number): Box {
  return { x: Math.min(x0, x1), y: Math.min(y0, y1), w: Math.abs(x1 - x0), h: Math.abs(y1 - y0) };
}

/** Move the roving focus by `delta` cells, clamped to the document. */
export function stepFocus(focus: PageIndex | null, delta: number, count: number): PageIndex {
  if (count <= 0) return 0;
  const base = focus ?? 0;
  return Math.max(0, Math.min(count - 1, base + delta));
}
