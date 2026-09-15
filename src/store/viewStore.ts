/**
 * View state: zoom, layout, view rotation, current page, night mode.
 * Owner from Stage 1: (c) viewer.
 *
 * What is deliberately NOT here (ARCHITECTURE §10): scroll position, the in-gesture zoom scale, the
 * tile inventory and pointer state — those live in refs/classes so React never re-renders per frame.
 */
import { create } from "zustand";
import type { PageIndex, Rotation, ViewLayout } from "../ipc/types";

export type ZoomMode = "fit-width" | "fit-page" | "actual" | "custom";
export type NightMode = "off" | "dark" | "sepia";

/** The numeric steps of the status-bar zoom combo (UI_SPEC §8). */
export const ZOOM_STEPS = [25, 50, 75, 100, 125, 150, 200, 400] as const;
export const MIN_ZOOM = 25;
export const MAX_ZOOM = 400;

export interface ViewState {
  zoomPercent: number;
  zoomMode: ZoomMode;
  layout: ViewLayout;
  rotation: Rotation;
  night: NightMode;
  currentPage: PageIndex;
  /** bumped when something asks the scroller to jump; the scroller consumes it */
  scrollRequest: { page: PageIndex; nonce: number } | null;

  setZoom(percent: number): void;
  setZoomMode(mode: ZoomMode, percent?: number): void;
  zoomIn(): void;
  zoomOut(): void;
  setLayout(layout: ViewLayout): void;
  rotate(delta: 90 | -90): void;
  setNight(night: NightMode): void;
  setCurrentPage(page: PageIndex): void;
  goToPage(page: PageIndex): void;
}

function clamp(percent: number): number {
  return Math.max(MIN_ZOOM, Math.min(MAX_ZOOM, Math.round(percent)));
}

export const useViewStore = create<ViewState>((set, get) => ({
  zoomPercent: 100,
  zoomMode: "fit-width",
  layout: "continuous",
  rotation: 0,
  night: "off",
  currentPage: 0,
  scrollRequest: null,

  setZoom(percent) {
    set({ zoomPercent: clamp(percent), zoomMode: "custom" });
  },
  setZoomMode(mode, percent) {
    set({ zoomMode: mode, zoomPercent: percent !== undefined ? clamp(percent) : mode === "actual" ? 100 : get().zoomPercent });
  },
  zoomIn() {
    const z = get().zoomPercent;
    set({ zoomPercent: clamp(ZOOM_STEPS.find((s) => s > z) ?? MAX_ZOOM), zoomMode: "custom" });
  },
  zoomOut() {
    const z = get().zoomPercent;
    const below = [...ZOOM_STEPS].reverse().find((s) => s < z) ?? MIN_ZOOM;
    set({ zoomPercent: clamp(below), zoomMode: "custom" });
  },
  setLayout(layout) {
    set({ layout });
  },
  rotate(delta) {
    set({ rotation: (((get().rotation + delta + 360) % 360) as Rotation) });
  },
  setNight(night) {
    set({ night });
  },
  setCurrentPage(page) {
    if (page !== get().currentPage) set({ currentPage: page });
  },
  goToPage(page) {
    set({ currentPage: page, scrollRequest: { page, nonce: (get().scrollRequest?.nonce ?? 0) + 1 } });
  },
}));
