/**
 * View state: zoom, layout, view rotation, current page, night mode — and 분할 보기 (P2).
 * Owner from Stage 1: (c) viewer.
 *
 * What is deliberately NOT here (ARCHITECTURE §10): scroll position, the in-gesture zoom scale, the
 * tile inventory and pointer state — those live in refs/classes so React never re-renders per frame.
 *
 * **Two panes, one document (P2 split view).** Zoom, zoom mode, view rotation, current page, the
 * pending scroll request and the 뒤로 / 앞으로 history are per *pane* (`PaneView`); layout
 * (단일 / 연속 / 두 쪽) and night mode stay document-level. The top-level `PaneView` fields always
 * describe the **focused** pane — so the status bar, the keymap, the menus, the dialogs ("현재
 * 페이지") and every existing reader keep working unchanged and act on the pane the user last
 * clicked — while the other pane's state is parked in `parked`. `focusPane` swaps the two; nothing
 * else ever copies between them. A pane's scroller reads its own slice with `paneView(state, id)` /
 * `usePaneView(id, …)`.
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

/** 분할 보기 (P2): the always-present pane and the one the split adds. */
export type PaneId = "main" | "second";
/** 좌우 (side by side) or 위아래 (one above the other). */
export type SplitOrientation = "side" | "stacked";

export interface ScrollRequest {
  page: PageIndex;
  nonce: number;
  /** an optional PDF-user-space y on that page (an outline destination) */
  yPt?: number;
}

/** What each pane of 분할 보기 has of its own. */
export interface PaneView {
  zoomPercent: number;
  zoomMode: ZoomMode;
  rotation: Rotation;
  currentPage: PageIndex;
  /**
   * Bumped when something asks the scroller to jump; the scroller consumes it.
   * `yPt` is an optional PDF-user-space y on that page (an outline destination), so 목차 lands
   * on the heading rather than on the top of the page.
   */
  scrollRequest: ScrollRequest | null;
  /** 뒤로 / 앞으로 (⌘[ / ⌘]): the pages `goToPage` jumped away from, newest last — per pane. */
  backStack: PageIndex[];
  forwardStack: PageIndex[];
}

export interface SplitState {
  orientation: SplitOrientation;
  /** 동기화 스크롤: scrolling one pane moves the other by as many pages */
  sync: boolean;
}

export interface ViewState extends PaneView {
  layout: ViewLayout;
  night: NightMode;
  /** 분할 보기 (P2); `null` = one pane */
  split: SplitState | null;
  /** the pane the top-level `PaneView` fields describe (keys, menus and the status bar act on it) */
  focusedPane: PaneId;
  /** the unfocused pane's state while split; `null` otherwise */
  parked: PaneView | null;

  /** every per-pane action acts on the focused pane unless `pane` names the other one */
  setZoom(percent: number, pane?: PaneId): void;
  setZoomMode(mode: ZoomMode, percent?: number, pane?: PaneId): void;
  zoomIn(pane?: PaneId): void;
  zoomOut(pane?: PaneId): void;
  setLayout(layout: ViewLayout): void;
  rotate(delta: 90 | -90, pane?: PaneId): void;
  setNight(night: NightMode): void;
  /** the next of 끄기 / 어둡게 / 세피아 */
  cycleNight(): void;
  setCurrentPage(page: PageIndex, pane?: PaneId): void;
  goToPage(page: PageIndex, yPt?: number, pane?: PaneId): void;
  /** 뒤로 / 앞으로: the pane's own jump history */
  goBack(pane?: PaneId): void;
  goForward(pane?: PaneId): void;
  /** a new document starts with no 뒤로 / 앞으로 history (in either pane) */
  resetHistory(): void;

  /** 보기 ▸ 분할 보기: the second pane opens on the focused pane's page and zoom */
  openSplit(orientation?: SplitOrientation): void;
  /** Esc / the menu / ×: the second pane goes, the main pane stays where it is and is focused */
  closeSplit(): void;
  toggleSplit(): void;
  setSplitOrientation(orientation: SplitOrientation): void;
  setSyncScroll(on: boolean): void;
  /** a click in a pane: it becomes the one the keys, menus and tools act on */
  focusPane(pane: PaneId): void;
}

/** 뒤로 remembers this many jumps. */
const HISTORY_MAX = 50;

function clamp(percent: number): number {
  return Math.max(MIN_ZOOM, Math.min(MAX_ZOOM, Math.round(percent)));
}

const PANE_KEYS = [
  "zoomPercent", "zoomMode", "rotation", "currentPage", "scrollRequest", "backStack", "forwardStack",
] as const;

function pick(s: PaneView): PaneView {
  return {
    zoomPercent: s.zoomPercent,
    zoomMode: s.zoomMode,
    rotation: s.rotation,
    currentPage: s.currentPage,
    scrollRequest: s.scrollRequest,
    backStack: s.backStack,
    forwardStack: s.forwardStack,
  };
}

/** One pane's view. A pane that does not exist (no split) reads as the focused one. */
export function paneView(s: ViewState, pane: PaneId): PaneView {
  if (pane === s.focusedPane || !s.parked) return pick(s);
  return s.parked;
}

/** A component's subscription to one field of one pane (re-renders only when that value moves). */
export function usePaneView<T>(pane: PaneId, select: (v: PaneView) => T): T {
  return useViewStore((s) => select(paneView(s, pane)));
}

/**
 * A pane argument, or `undefined` for anything else — the actions are bound straight to `onClick`
 * (`onClick={zoomIn}`), so their optional `pane` may well be a MouseEvent.
 */
function asPane(pane: unknown): PaneId | undefined {
  return pane === "main" || pane === "second" ? pane : undefined;
}

/** The state change that writes `patch` into `pane` (the focused one when omitted). */
function writePane(s: ViewState, pane: unknown, patch: Partial<PaneView>): Partial<ViewState> {
  const target = asPane(pane);
  if (target === undefined || target === s.focusedPane) return patch;
  if (!s.parked) return {};
  return { parked: { ...s.parked, ...patch } };
}

export const useViewStore = create<ViewState>((set, get) => {
  const view = (pane?: unknown) => paneView(get(), asPane(pane) ?? get().focusedPane);
  const write = (pane: unknown, patch: Partial<PaneView>) => set(writePane(get(), pane, patch));
  return {
    zoomPercent: 100,
    zoomMode: "fit-width",
    layout: "continuous",
    rotation: 0,
    night: "off",
    currentPage: 0,
    scrollRequest: null,
    backStack: [],
    forwardStack: [],
    split: null,
    focusedPane: "main",
    parked: null,

    setZoom(percent, pane) {
      write(pane, { zoomPercent: clamp(percent), zoomMode: "custom" });
    },
    setZoomMode(mode, percent, pane) {
      const current = view(pane).zoomPercent;
      write(pane, {
        zoomMode: mode,
        zoomPercent: percent !== undefined ? clamp(percent) : mode === "actual" ? 100 : current,
      });
    },
    zoomIn(pane) {
      const z = view(pane).zoomPercent;
      write(pane, { zoomPercent: clamp(ZOOM_STEPS.find((s) => s > z) ?? MAX_ZOOM), zoomMode: "custom" });
    },
    zoomOut(pane) {
      const z = view(pane).zoomPercent;
      const below = [...ZOOM_STEPS].reverse().find((s) => s < z) ?? MIN_ZOOM;
      write(pane, { zoomPercent: clamp(below), zoomMode: "custom" });
    },
    setLayout(layout) {
      set({ layout });
    },
    rotate(delta, pane) {
      write(pane, { rotation: (((view(pane).rotation + delta + 360) % 360) as Rotation) });
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
    setCurrentPage(page, pane) {
      if (page !== view(pane).currentPage) write(pane, { currentPage: page });
    },
    goToPage(page, yPt, pane) {
      const v = view(pane);
      const from = v.currentPage;
      write(pane, {
        currentPage: page,
        scrollRequest: { page, nonce: (v.scrollRequest?.nonce ?? 0) + 1, yPt },
        ...(page !== from ? { backStack: [...v.backStack, from].slice(-HISTORY_MAX), forwardStack: [] } : {}),
      });
    },
    goBack(pane) {
      const v = view(pane);
      const page = v.backStack[v.backStack.length - 1];
      if (page === undefined) return;
      write(pane, {
        currentPage: page,
        scrollRequest: { page, nonce: (v.scrollRequest?.nonce ?? 0) + 1 },
        backStack: v.backStack.slice(0, -1),
        forwardStack: [...v.forwardStack, v.currentPage].slice(-HISTORY_MAX),
      });
    },
    goForward(pane) {
      const v = view(pane);
      const page = v.forwardStack[v.forwardStack.length - 1];
      if (page === undefined) return;
      write(pane, {
        currentPage: page,
        scrollRequest: { page, nonce: (v.scrollRequest?.nonce ?? 0) + 1 },
        forwardStack: v.forwardStack.slice(0, -1),
        backStack: [...v.backStack, v.currentPage].slice(-HISTORY_MAX),
      });
    },
    resetHistory() {
      const parked = get().parked;
      const cleared = { backStack: [], forwardStack: [] };
      set({ ...cleared, ...(parked ? { parked: { ...parked, ...cleared } } : {}) });
    },

    openSplit(requested) {
      const orientation = requested === "side" || requested === "stacked" ? requested : undefined;
      const s = get();
      if (s.split) {
        if (orientation && orientation !== s.split.orientation) set({ split: { ...s.split, orientation } });
        return;
      }
      const here = pick(s);
      set({
        split: { orientation: orientation ?? "side", sync: false },
        // the new pane starts on the same page, at the same zoom — and scrolls there on mount; its
        // 뒤로 / 앞으로 history is its own, starting empty
        parked: { ...here, scrollRequest: { page: here.currentPage, nonce: 1 }, backStack: [], forwardStack: [] },
      });
    },
    closeSplit() {
      const s = get();
      if (!s.split) return;
      if (s.focusedPane === "second" && s.parked) {
        set({ ...s.parked, focusedPane: "main", parked: null, split: null });
      } else {
        set({ focusedPane: "main", parked: null, split: null });
      }
    },
    toggleSplit() {
      if (get().split) get().closeSplit();
      else get().openSplit();
    },
    setSplitOrientation(orientation) {
      const split = get().split;
      if (split && split.orientation !== orientation) set({ split: { ...split, orientation } });
    },
    setSyncScroll(sync) {
      const split = get().split;
      if (split && split.sync !== sync) set({ split: { ...split, sync } });
    },
    focusPane(pane) {
      const s = get();
      if (pane === s.focusedPane || !s.split || !s.parked) return;
      set({ ...s.parked, parked: pick(s), focusedPane: pane });
    },
  };
});

/** Test seam: the per-pane field names (the unit tests assert both panes carry exactly these). */
export const PANE_FIELDS: readonly (keyof PaneView)[] = PANE_KEYS;

/**
 * "현재 위치" (P2 목차 / 링크 destinations): the page under the top of the viewport and the PDF
 * user-space y of that edge. Each pane's scroller registers a probe (scroll offsets live in its
 * refs, never in a store); without one — no document, tests — it is the current page, no position.
 */
export interface ViewTarget {
  page: PageIndex;
  y?: number;
}

/** One probe per pane (분할 보기): "현재 위치" is the focused pane's. */
const viewProbes: Partial<Record<PaneId, () => ViewTarget | null>> = {};

export function setViewProbe(probe: (() => ViewTarget | null) | null, pane: PaneId = "main"): void {
  if (probe) viewProbes[pane] = probe;
  else delete viewProbes[pane];
}

export function currentViewTarget(): ViewTarget {
  const s = useViewStore.getState();
  return viewProbes[s.focusedPane]?.() ?? { page: s.currentPage };
}

// Settings arrive once per window (`appStore.bootstrap`): start in the remembered 야간 모드.
useAppStore.subscribe((s, prev) => {
  if (s.settings && !prev.settings) useViewStore.setState({ night: readNight(s.settings.night) });
});
