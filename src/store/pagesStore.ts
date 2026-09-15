/**
 * 페이지 organizer state (UI_SPEC §9). Owner: Stage 1 (e).
 * Page geometry itself lives in `docStore.info.pages` — this slice only holds the grid's own state.
 *
 * `pendingOrder` is the optimistic drag-reorder: the grid paints it immediately, the `page_ops`
 * response (and the `doc-changed` refresh it triggers) reconciles it away. Nothing else in the app
 * reads it, so a failed move simply snaps back.
 */
import { create } from "zustand";
import type { PageIndex } from "../ipc/types";
import { clickSelect, normalizeSelection, rangeBetween, type SelectionState } from "../organize/selection";

export const THUMB_SIZES = [80, 120, 160, 220] as const;
export type ThumbSize = (typeof THUMB_SIZES)[number];

export interface PagesState {
  selected: PageIndex[];
  lastAnchor: PageIndex | null;
  thumbSize: ThumbSize;
  /** insertion caret between cells while dragging, or null */
  dropAt: number | null;
  /** optimistic display order (page indices) while a reorder is in flight */
  pendingOrder: PageIndex[] | null;
  /** roving keyboard focus */
  focus: PageIndex | null;

  setSelected(pages: PageIndex[]): void;
  toggle(page: PageIndex, additive: boolean): void;
  /** click / ⇧click / ⌘click in one call (UI_SPEC §9) */
  click(page: PageIndex, mods?: { shift?: boolean; meta?: boolean }): void;
  selectRange(to: PageIndex): void;
  selectAll(count: number): void;
  clear(): void;
  setThumbSize(size: ThumbSize): void;
  setDropAt(at: number | null): void;
  setPendingOrder(order: PageIndex[] | null): void;
  setFocus(page: PageIndex | null): void;
  /** a new document, or a structural change: forget everything transient */
  reset(): void;
}

export const usePagesStore = create<PagesState>((set, get) => ({
  selected: [],
  lastAnchor: null,
  thumbSize: 120,
  dropAt: null,
  pendingOrder: null,
  focus: null,

  setSelected(selected) {
    const next = normalizeSelection(selected);
    set({ selected: next, lastAnchor: next[next.length - 1] ?? null, focus: next[next.length - 1] ?? get().focus });
  },
  toggle(page, additive) {
    get().click(page, { meta: additive });
  },
  click(page, mods = {}) {
    const state: SelectionState = { selected: get().selected, anchor: get().lastAnchor };
    const next = clickSelect(state, page, mods);
    set({ selected: next.selected, lastAnchor: next.anchor, focus: page });
  },
  selectRange(to) {
    const anchor = get().lastAnchor ?? to;
    set({ selected: rangeBetween(anchor, to), focus: to });
  },
  selectAll(count) {
    set({ selected: Array.from({ length: count }, (_, i) => i), lastAnchor: count > 0 ? count - 1 : null });
  },
  clear() {
    set({ selected: [], lastAnchor: null });
  },
  setThumbSize(thumbSize) {
    set({ thumbSize });
  },
  setDropAt(dropAt) {
    set({ dropAt });
  },
  setPendingOrder(pendingOrder) {
    set({ pendingOrder });
  },
  setFocus(focus) {
    set({ focus });
  },
  reset() {
    set({ selected: [], lastAnchor: null, dropAt: null, pendingOrder: null, focus: null });
  },
}));
