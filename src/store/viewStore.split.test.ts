/**
 * 분할 보기 (P2) — the viewStore refactor: zoom, zoom mode, rotation, current page and the scroll
 * request exist once per pane, layout and night mode once per document, and the top-level fields
 * are always the focused pane's (so every existing reader keeps acting on the pane in use).
 */
import { beforeEach, describe, expect, it } from "vitest";
import { PANE_FIELDS, paneView, useViewStore } from "./viewStore";

beforeEach(() => {
  useViewStore.getState().closeSplit();
  useViewStore.setState({
    zoomPercent: 100, zoomMode: "custom", rotation: 0, layout: "continuous", currentPage: 0, scrollRequest: null,
    night: "off", split: null, focusedPane: "main", parked: null,
  });
});

const view = (pane: "main" | "second") => paneView(useViewStore.getState(), pane);

describe("viewStore — split panes", () => {
  it("per-pane fields are exactly zoom, zoom mode, rotation, current page and the scroll request", () => {
    expect([...PANE_FIELDS].sort()).toEqual(["currentPage", "rotation", "scrollRequest", "zoomMode", "zoomPercent"]);
  });

  it("without a split both ids read the one view", () => {
    useViewStore.getState().setZoom(150);
    expect(view("main").zoomPercent).toBe(150);
    expect(view("second").zoomPercent).toBe(150);
    // an action aimed at a pane that does not exist changes nothing
    useViewStore.getState().setZoom(50, "second");
    expect(useViewStore.getState().zoomPercent).toBe(150);
  });

  it("opening copies the focused pane into the second one, which scrolls to the same page", () => {
    const s = useViewStore.getState();
    s.setZoom(175);
    s.goToPage(3);
    s.openSplit();
    const after = useViewStore.getState();
    expect(after.split).toEqual({ orientation: "side", sync: false });
    expect(after.focusedPane).toBe("main");
    expect(view("second")).toMatchObject({ zoomPercent: 175, currentPage: 3, scrollRequest: { page: 3 } });
    // a second open only changes the orientation
    after.openSplit("stacked");
    expect(useViewStore.getState().split?.orientation).toBe("stacked");
    expect(view("second").zoomPercent).toBe(175);
  });

  it("per-pane actions leave the other pane alone; document-level ones are shared", () => {
    const s = useViewStore.getState();
    s.openSplit();
    s.goToPage(5, undefined, "second");
    s.setZoom(60, "second");
    s.rotate(90, "second");
    expect(view("second")).toMatchObject({ currentPage: 5, zoomPercent: 60, rotation: 90, scrollRequest: { page: 5 } });
    expect(view("main")).toMatchObject({ currentPage: 0, zoomPercent: 100, rotation: 0 });
    // the top level is the focused (main) pane
    expect(useViewStore.getState().currentPage).toBe(0);
    s.setLayout("two");
    s.setNight("dark");
    expect(useViewStore.getState()).toMatchObject({ layout: "two", night: "dark" });
    // an `onClick={zoomIn}` hands a MouseEvent as the pane: it means the focused pane
    s.zoomIn({ type: "click" } as unknown as "main");
    expect(view("main").zoomPercent).toBe(125);
    expect(view("second").zoomPercent).toBe(60);
  });

  it("focusing swaps which pane the top level describes, and loses nothing", () => {
    const s = useViewStore.getState();
    s.setZoom(120);
    s.openSplit();
    s.goToPage(7, undefined, "second");
    s.setZoom(80, "second");
    s.focusPane("second");
    let now = useViewStore.getState();
    expect(now.focusedPane).toBe("second");
    expect(now).toMatchObject({ currentPage: 7, zoomPercent: 80 });
    expect(view("main")).toMatchObject({ currentPage: 0, zoomPercent: 120 });
    // what the keys do now lands in the second pane
    now.zoomIn();
    now.goToPage(9);
    expect(view("second")).toMatchObject({ zoomPercent: 100, currentPage: 9 });
    expect(view("main")).toMatchObject({ zoomPercent: 120, currentPage: 0 });
    s.focusPane("main");
    now = useViewStore.getState();
    expect(now).toMatchObject({ focusedPane: "main", zoomPercent: 120, currentPage: 0 });
    expect(view("second")).toMatchObject({ zoomPercent: 100, currentPage: 9 });
  });

  it("closing keeps the main pane (even from the second) and focuses it", () => {
    const s = useViewStore.getState();
    s.goToPage(2);
    s.openSplit();
    s.focusPane("second");
    useViewStore.getState().goToPage(11);
    useViewStore.getState().closeSplit();
    const now = useViewStore.getState();
    expect(now).toMatchObject({ split: null, parked: null, focusedPane: "main", currentPage: 2 });
    expect(useViewStore.getState().focusPane("second")).toBeUndefined();
    expect(useViewStore.getState().focusedPane).toBe("main");
  });

  it("toggle, orientation and 동기화 스크롤", () => {
    const s = useViewStore.getState();
    s.toggleSplit();
    expect(useViewStore.getState().split).not.toBeNull();
    s.setSplitOrientation("stacked");
    s.setSyncScroll(true);
    expect(useViewStore.getState().split).toEqual({ orientation: "stacked", sync: true });
    s.toggleSplit();
    expect(useViewStore.getState().split).toBeNull();
    // without a split these are no-ops
    s.setSyncScroll(true);
    expect(useViewStore.getState().split).toBeNull();
  });
});
