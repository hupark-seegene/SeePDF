/**
 * Annotations of the open document, per page, plus the tool style defaults.
 * Owner from Stage 1: (d) tools/properties. Stage 0 defines the shape only.
 *
 * Optimism (ARCHITECTURE §10): a created annotation is pushed here immediately (`ghosts`), drawn in
 * SVG within 16 ms, and removed from `ghosts` when the new generation's tile paints it.
 */
import { create } from "zustand";
import type { Annot, AnnotId, PageIndex, Rgb } from "../ipc/types";

/** The 8 swatches of UI_SPEC §14.4, with the highlight alpha column. */
export const PALETTE: { key: string; rgb: Rgb; highlightAlpha: number }[] = [
  { key: "color.yellow", rgb: [255, 216, 77], highlightAlpha: 0.4 },
  { key: "color.green", rgb: [123, 214, 74], highlightAlpha: 0.38 },
  { key: "color.blue", rgb: [77, 184, 255], highlightAlpha: 0.38 },
  { key: "color.pink", rgb: [255, 127, 176], highlightAlpha: 0.38 },
  { key: "color.purple", rgb: [176, 123, 255], highlightAlpha: 0.35 },
  { key: "color.orange", rgb: [255, 154, 61], highlightAlpha: 0.38 },
  { key: "color.red", rgb: [245, 83, 61], highlightAlpha: 0.32 },
  { key: "color.black", rgb: [43, 47, 51], highlightAlpha: 1 },
];

export interface ToolStyle {
  color: Rgb;
  opacity: number;
  width: number;
  fontSize: number;
  fillColor: Rgb | null;
}

export interface AnnotState {
  byPage: Record<PageIndex, Annot[]>;
  selected: AnnotId[];
  ghosts: Annot[];
  style: ToolStyle;

  setPage(page: PageIndex, annots: Annot[]): void;
  upsert(annot: Annot): void;
  remove(page: PageIndex, ids: AnnotId[]): void;
  select(ids: AnnotId[]): void;
  addGhost(annot: Annot): void;
  clearGhosts(page?: PageIndex): void;
  setStyle(patch: Partial<ToolStyle>): void;
  reset(): void;
}

const INITIAL_STYLE: ToolStyle = {
  color: PALETTE[0].rgb,
  opacity: PALETTE[0].highlightAlpha,
  width: 2,
  fontSize: 12,
  fillColor: null,
};

export const useAnnotStore = create<AnnotState>((set, get) => ({
  byPage: {},
  selected: [],
  ghosts: [],
  style: INITIAL_STYLE,

  setPage(page, annots) {
    set({ byPage: { ...get().byPage, [page]: annots } });
  },
  upsert(annot) {
    const list = get().byPage[annot.page] ?? [];
    const next = list.some((a) => a.id === annot.id)
      ? list.map((a) => (a.id === annot.id ? annot : a))
      : [...list, annot];
    set({ byPage: { ...get().byPage, [annot.page]: next } });
  },
  remove(page, ids) {
    const list = (get().byPage[page] ?? []).filter((a) => !ids.includes(a.id));
    set({ byPage: { ...get().byPage, [page]: list }, selected: get().selected.filter((id) => !ids.includes(id)) });
  },
  select(selected) {
    set({ selected });
  },
  addGhost(annot) {
    set({ ghosts: [...get().ghosts, annot] });
  },
  clearGhosts(page) {
    set({ ghosts: page === undefined ? [] : get().ghosts.filter((g) => g.page !== page) });
  },
  setStyle(patch) {
    set({ style: { ...get().style, ...patch } });
  },
  reset() {
    set({ byPage: {}, selected: [], ghosts: [] });
  },
}));
