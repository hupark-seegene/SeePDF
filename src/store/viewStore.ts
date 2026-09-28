/**
 * View state: zoom, layout, view rotation, current page, night mode.
 * Owner from Stage 1: (c) viewer.
 *
 * What is deliberately NOT here (ARCHITECTURE §10): scroll position, the in-gesture zoom scale, the
 * tile inventory and pointer state — those live in refs/classes so React never re-renders per frame.
 */
import { create } from "zustand";
import type { PageIndex, Rotation, ViewLayout } from "../ipc/types";
import { useAppStore } from "./appStore";

export type ZoomMode = "fit-width" | "fit-page" | "actual" | "custom";
export type NightMode = "off" | "dark" | "sepia";

/** 야간 모드 order (P1-10): ⌃⌘N, the 보기 menu item and the status-bar moon all step through it. */
export const NIGHT_MODES: readonly NightMode[] = ["off", "dark", "sepia"];

/** 끄기 → 어둡게 → 세피아 → 끄기. */
export function nextNight(night: NightMode): NightMode {
  return NIGHT_MODES[(NIGHT_MODES.indexOf(night) + 1) % NIGHT_MODES.length];
}

/** `Settings.night`, defended against a file written before Stage 8 or edited by hand. */
export function readNight(value: unknown): NightMode {
  return NIGHT_MODES.includes(value as NightMode) ? (value as NightMode) : "off";
}

/** 야간 모드 is remembered across launches (Stage 8): every change goes to `Settings.night`. */
function persistNight(night: NightMode): void {
  void useAppStore.getState().patchSettings({ night });
}

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
  /**
   * Bumped when something asks the scroller to jump; the scroller consumes it.
   * `yPt` is an optional PDF-user-space y on that page (an outline destination), so 목차 lands
   * on the heading rather than on the top of the page.
   */
  scrollRequest: { page: PageIndex; nonce: number; yPt?: number } | null;

  setZoom(percent: number): void;
  setZoomMode(mode: ZoomMode, percent?: number): void;
  zoomIn(): void;
  zoomOut(): void;
  setLayout(layout: ViewLayout): void;
  rotate(delta: 90 | -90): void;
  setNight(night: NightMode): void;
  /** the next of 끄기 / 어둡게 / 세피아 */
  cycleNight(): void;
  setCurrentPage(page: PageIndex): void;
  goToPage(page: PageIndex, yPt?: number): void;
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
    if (night === get().night) return;
    set({ night });
    persistNight(night);
  },
  cycleNight() {
    const night = nextNight(get().night);
    set({ night });
    persistNight(night);
  },
  setCurrentPage(page) {
    if (page !== get().currentPage) set({ currentPage: page });
  },
  goToPage(page, yPt) {
    set({
      currentPage: page,
      scrollRequest: { page, nonce: (get().scrollRequest?.nonce ?? 0) + 1, yPt },
    });
  },
}));

// Settings arrive once per window (`appStore.bootstrap`): start in the remembered 야간 모드.
useAppStore.subscribe((s, prev) => {
  if (s.settings && !prev.settings) useViewStore.setState({ night: readNight(s.settings.night) });
});
