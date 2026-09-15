/**
 * 페이지 organizer state (UI_SPEC §9). Owner from Stage 1: (e).
 * Page geometry itself lives in `docStore.info.pages` — this slice only holds the grid's own state.
 */
import { create } from "zustand";
import type { PageIndex } from "../ipc/types";

export const THUMB_SIZES = [80, 120, 160, 220] as const;
export type ThumbSize = (typeof THUMB_SIZES)[number];

export interface PagesState {
  selected: PageIndex[];
  lastAnchor: PageIndex | null;
  thumbSize: ThumbSize;
  /** insertion caret between cells while dragging, or null */
  dropAt: number | null;

  setSelected(pages: PageIndex[]): void;
  toggle(page: PageIndex, additive: boolean): void;
  selectRange(to: PageIndex): void;
  selectAll(count: number): void;
  clear(): void;
  setThumbSize(size: ThumbSize): void;
  setDropAt(at: number | null): void;
}

export const usePagesStore = create<PagesState>((set, get) => ({
  selected: [],
  lastAnchor: null,
  thumbSize: 120,
  dropAt: null,

  setSelected(selected) {
    set({ selected, lastAnchor: selected[selected.length - 1] ?? null });
  },
  toggle(page, additive) {
    const { selected } = get();
    if (!additive) return set({ selected: [page], lastAnchor: page });
    const next = selected.includes(page) ? selected.filter((p) => p !== page) : [...selected, page];
    set({ selected: next, lastAnchor: page });
  },
  selectRange(to) {
    const anchor = get().lastAnchor ?? to;
    const [from, until] = anchor <= to ? [anchor, to] : [to, anchor];
    const range: PageIndex[] = [];
    for (let p = from; p <= until; p++) range.push(p);
    set({ selected: range });
  },
  selectAll(count) {
    set({ selected: Array.from({ length: count }, (_, i) => i) });
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
}));
