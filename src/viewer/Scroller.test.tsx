/**
 * The scroller's navigation effects (bug hunt 2026-09): a jump is a one-shot request, not a state
 * the scroller keeps re-applying; 너비 맞춤 must not feed back into itself through the current page;
 * a no-op zoom must not park a scroll target; and a display-density change re-lays the pages out.
 * 분할 보기 (P2): every one of those holds per pane — each pane consumes its own request, a search
 * hit moves the focused pane only, and a density change replays nothing in either pane.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, render, waitFor } from "@testing-library/react";
import { Scroller } from "./Scroller";
import { TileManager } from "./TileManager";
import { resetPanes } from "./panes";
import { useViewStore } from "../store/viewStore";
import { useSearchStore } from "./search/SearchController";
import { setMockAssetResolver } from "../ipc/protocol";
import type { DocInfo, PageGeom, SearchHit } from "../ipc/types";

function geom(index: number, w = 595.28, h = 841.89): PageGeom {
  return { index, widthPt: w, heightPt: h, rotation: 0, crop: { l: 0, b: 0, r: w, t: h }, label: null };
}

function doc(pages: PageGeom[], extra: Partial<DocInfo> = {}): DocInfo {
  return {
    docId: "d-scroll",
    path: "/tmp/scroll.pdf",
    name: "scroll.pdf",
    bytes: 1,
    pageCount: pages.length,
    pages,
    docGeneration: 1,
    dirty: false,
    canUndo: false,
    canRedo: false,
    undoLabel: null,
    redoLabel: null,
    encrypted: false,
    permissions: {
      print: true, modify: true, extractText: true, annotate: true, fillForms: true, assemble: true,
      revision: "unprotected",
    },
    hasForm: false,
    xfa: false,
    hasOutline: false,
    meta: {},
    pdfVersion: "1.7",
    tagged: false,
    ...extra,
  };
}

const portrait = (n: number) => Array.from({ length: n }, (_, i) => geom(i));

function canvas(container: HTMLElement): HTMLDivElement {
  return container.querySelector(".canvas.viewer") as HTMLDivElement;
}

beforeEach(() => {
  useViewStore.setState({
    zoomPercent: 100, zoomMode: "custom", layout: "continuous", rotation: 0, currentPage: 0, scrollRequest: null,
    split: null, parked: null, focusedPane: "main",
  });
  useSearchStore.setState({ hits: [], current: -1, navNonce: 0 });
});

afterEach(() => {
  vi.unstubAllGlobals();
  setMockAssetResolver(null);
  useViewStore.getState().closeSplit();
  resetPanes();
});

describe("Scroller — 이동 is a one-shot request", () => {
  it("a document change (annotation, form, undo, save) does not scroll back to the last 이동 page", () => {
    const info = doc(portrait(40));
    const { container, rerender } = render(<Scroller info={info} />);
    const el = canvas(container);
    act(() => useViewStore.getState().goToPage(5));
    const page5 = el.scrollTop;
    expect(page5).toBeGreaterThan(0);

    // the user scrolls far away, then edits something
    el.scrollTop = page5 + 20_000;
    rerender(<Scroller info={{ ...info, docGeneration: 2, dirty: true }} />);
    expect(el.scrollTop).toBe(page5 + 20_000);
    rerender(<Scroller info={{ ...info, docGeneration: 3, dirty: false, canUndo: true }} />);
    expect(el.scrollTop).toBe(page5 + 20_000);

    // …while a new request still lands
    act(() => useViewStore.getState().goToPage(5));
    expect(el.scrollTop).toBe(page5);
  });

  it("a document change does not scroll back to the current search hit", () => {
    const info = doc(portrait(40));
    const { container, rerender } = render(<Scroller info={info} />);
    const el = canvas(container);
    const hit: SearchHit = {
      page: 20, charStart: 0, charLength: 6, rects: [{ l: 100, b: 400, r: 200, t: 412 }],
      context: "monkey", contextMatch: [0, 6],
    };
    act(() => useSearchStore.setState({ hits: [hit], current: 0, navNonce: 1 }));
    const hitTop = el.scrollTop;
    expect(hitTop).toBeGreaterThan(10_000);

    el.scrollTop = 100;
    rerender(<Scroller info={{ ...info, docGeneration: 2, dirty: true }} />);
    expect(el.scrollTop).toBe(100);

    // ⌘G again still scrolls
    act(() => useSearchStore.setState({ navNonce: 2 }));
    expect(el.scrollTop).toBe(hitTop);
  });
});

describe("Scroller — 너비 맞춤 with mixed page sizes", () => {
  it("does not refit when the current page changes between a portrait and a landscape page", () => {
    const pages = Array.from({ length: 12 }, (_, i) => (i % 2 ? geom(i, 842, 595) : geom(i, 595, 842)));
    useViewStore.setState({ zoomMode: "fit-width" });
    render(<Scroller info={doc(pages)} />);
    const settled = useViewStore.getState().zoomPercent;
    expect(useViewStore.getState().zoomMode).toBe("fit-width");

    // the scroll tick moves the current page onto the other orientation: the zoom must hold
    for (const page of [1, 2, 3, 0, 5]) {
      act(() => useViewStore.getState().setCurrentPage(page));
      expect(useViewStore.getState().zoomPercent).toBe(settled);
    }
    // every page fits: the widest page decides
    const fitted = useViewStore.getState().zoomPercent;
    expect((842 * fitted) / 100).toBeLessThanOrEqual(900 - 32);
  });

  it("단일 still fits the page it shows", () => {
    const pages = [geom(0, 595, 842), geom(1, 842, 595)];
    useViewStore.setState({ zoomMode: "fit-width", layout: "single" });
    render(<Scroller info={doc(pages)} />);
    const onPortrait = useViewStore.getState().zoomPercent;
    act(() => useViewStore.getState().goToPage(1));
    expect(useViewStore.getState().zoomPercent).toBeLessThan(onPortrait);
  });
});

describe("Scroller — a zoom that changes nothing", () => {
  function wheelThenRotate(noOpWheel: boolean): number {
    useViewStore.setState({ zoomPercent: 400, zoomMode: "custom", rotation: 0, currentPage: 0, scrollRequest: null });
    const { container, unmount } = render(<Scroller info={doc(portrait(20))} />);
    const el = canvas(container);
    act(() => useViewStore.getState().goToPage(3));
    if (noOpWheel) {
      // pinch-in at the 400 % ceiling: the zoom stays 400
      act(() => {
        el.dispatchEvent(new WheelEvent("wheel", { deltaY: -30, ctrlKey: true, bubbles: true, cancelable: true }));
      });
      expect(useViewStore.getState().zoomPercent).toBe(400);
    }
    el.scrollTop = el.scrollTop + 30_000;
    act(() => useViewStore.getState().rotate(90));
    const after = el.scrollTop;
    unmount();
    return after;
  }

  it("does not leave a parked scroll target for the next unrelated relayout", () => {
    const clean = wheelThenRotate(false);
    const withNoOp = wheelThenRotate(true);
    expect(withNoOp).toBe(clean);
  });
});

describe("Scroller — display density", () => {
  it("re-lays out and re-requests bitmaps when the window moves to a display with another scale", async () => {
    const requests: Record<string, unknown>[] = [];
    setMockAssetResolver((route, query) => {
      if (route === "/page") requests.push({ ...query });
      return "data:image/png;base64,iVBORw0KGgo=";
    });
    let listeners: (() => void)[] = [];
    vi.stubGlobal("matchMedia", (query: string) => ({
      media: query,
      matches: true,
      addEventListener: (_: string, cb: () => void) => listeners.push(cb),
      removeEventListener: (_: string, cb: () => void) => (listeners = listeners.filter((l) => l !== cb)),
      addListener: (cb: () => void) => listeners.push(cb),
      removeListener: (cb: () => void) => (listeners = listeners.filter((l) => l !== cb)),
    }));
    const original = window.devicePixelRatio;
    Object.defineProperty(window, "devicePixelRatio", { configurable: true, value: 1 });
    try {
      render(<Scroller info={doc(portrait(3))} />);
      await waitFor(() => expect(requests.some((q) => Number(q.sk) === 100)).toBe(true));
      expect(listeners.length).toBeGreaterThan(0);

      // dragged onto a Retina display
      Object.defineProperty(window, "devicePixelRatio", { configurable: true, value: 2 });
      act(() => listeners.forEach((l) => l()));
      await waitFor(() => expect(requests.some((q) => Number(q.sk) === 200)).toBe(true));
    } finally {
      Object.defineProperty(window, "devicePixelRatio", { configurable: true, value: original });
    }
  });
});

// ---------------------------------------------------------------------------
// 분할 보기 (P2) × the navigation fixes
// ---------------------------------------------------------------------------

/** Both panes the way `Viewer` mounts them: one shared TileManager, `focused` from the store. */
function renderSplit(info: DocInfo) {
  const tiles = new TileManager();
  const ui = (doc: DocInfo) => {
    const focusedPane = useViewStore.getState().focusedPane;
    return (
      <>
        <Scroller info={doc} paneId="main" split focused={focusedPane === "main"} tiles={tiles} />
        <Scroller info={doc} paneId="second" split focused={focusedPane === "second"} tiles={tiles} />
      </>
    );
  };
  const result = render(ui(info));
  const [main, second] = [...result.container.querySelectorAll<HTMLDivElement>(".canvas.viewer")];
  expect(main.dataset.pane).toBe("main");
  expect(second.dataset.pane).toBe("second");
  return { main, second, rerender: (doc: DocInfo) => result.rerender(ui(doc)) };
}

function hitOn(page: number): SearchHit {
  return {
    page, charStart: 0, charLength: 6, rects: [{ l: 100, b: 400, r: 200, t: 412 }],
    context: "monkey", contextMatch: [0, 6],
  };
}

describe("Scroller — 분할 보기: 이동 and search per pane", () => {
  it("each pane consumes its own request once; a search hit moves the focused pane only", () => {
    const info = doc(portrait(40));
    act(() => useViewStore.getState().openSplit());
    const { main, second, rerender } = renderSplit(info);

    // 이동 in the focused (main) pane: the second pane stays where it opened (page 1)
    const secondOpened = second.scrollTop;
    act(() => useViewStore.getState().goToPage(5));
    const mainAt5 = main.scrollTop;
    expect(mainAt5).toBeGreaterThan(secondOpened);
    expect(second.scrollTop).toBe(secondOpened);

    // 이동 aimed at the second pane: main stays
    act(() => useViewStore.getState().goToPage(12, undefined, "second"));
    const secondAt12 = second.scrollTop;
    expect(secondAt12).toBeGreaterThan(mainAt5);
    expect(main.scrollTop).toBe(mainAt5);

    // the user scrolls both panes, then edits: neither pane scrolls back to its last request
    main.scrollTop = mainAt5 + 3000;
    second.scrollTop = secondAt12 + 5000;
    rerender({ ...info, docGeneration: 2, dirty: true });
    expect(main.scrollTop).toBe(mainAt5 + 3000);
    expect(second.scrollTop).toBe(secondAt12 + 5000);

    // a click focuses the second pane: the swap moves the requests between slices, it is not one
    act(() => useViewStore.getState().focusPane("second"));
    rerender({ ...info, docGeneration: 3, dirty: true });
    expect(main.scrollTop).toBe(mainAt5 + 3000);
    expect(second.scrollTop).toBe(secondAt12 + 5000);

    // ⌘G: the focused (second) pane goes to the hit, main does not move
    act(() => useSearchStore.setState({ hits: [hitOn(30)], current: 0, navNonce: 1 }));
    const secondAtHit = second.scrollTop;
    expect(secondAtHit).toBeGreaterThan(secondAt12 + 5000);
    expect(main.scrollTop).toBe(mainAt5 + 3000);

    // back in main, the next ⌘G lands there — and the second pane stays put
    second.scrollTop = 0;
    act(() => useViewStore.getState().focusPane("main"));
    rerender({ ...info, docGeneration: 4, dirty: true });
    expect(main.scrollTop).toBe(mainAt5 + 3000);
    act(() => useSearchStore.setState({ navNonce: 2 }));
    expect(main.scrollTop).toBe(secondAtHit);
    expect(second.scrollTop).toBe(0);
  });

  it("a display-density change re-renders both panes at their own zoom and replays nothing", async () => {
    const requests: number[] = [];
    setMockAssetResolver((route, query) => {
      if (route === "/page" || route === "/tile") requests.push(Number(query.sk));
      return "data:image/png;base64,iVBORw0KGgo=";
    });
    let listeners: (() => void)[] = [];
    vi.stubGlobal("matchMedia", (query: string) => ({
      media: query,
      matches: true,
      addEventListener: (_: string, cb: () => void) => listeners.push(cb),
      removeEventListener: (_: string, cb: () => void) => (listeners = listeners.filter((l) => l !== cb)),
      addListener: (cb: () => void) => listeners.push(cb),
      removeListener: (cb: () => void) => (listeners = listeners.filter((l) => l !== cb)),
    }));
    const original = window.devicePixelRatio;
    Object.defineProperty(window, "devicePixelRatio", { configurable: true, value: 1 });
    try {
      const info = doc(portrait(40));
      act(() => useViewStore.getState().openSplit());
      act(() => useViewStore.getState().setZoom(75, "second"));
      const { main, second, rerender } = renderSplit(info);
      await waitFor(() => expect(requests).toContain(100));
      await waitFor(() => expect(requests).toContain(75));

      // each pane has navigated, and the focused (main) pane has followed a search hit
      act(() => useViewStore.getState().goToPage(5));
      act(() => useViewStore.getState().goToPage(9, undefined, "second"));
      act(() => useSearchStore.setState({ hits: [hitOn(30)], current: 0, navNonce: 1 }));
      expect(main.scrollTop).toBeGreaterThan(10_000);

      // …then scrolled both panes elsewhere, and focused the second one
      main.scrollTop = 1234;
      second.scrollTop = 4321;
      act(() => useViewStore.getState().focusPane("second"));
      rerender(info);

      // dragged onto a Retina display
      requests.length = 0;
      Object.defineProperty(window, "devicePixelRatio", { configurable: true, value: 2 });
      act(() => listeners.forEach((l) => l()));
      // both panes re-request bitmaps at the new density, each at its own zoom
      await waitFor(() => expect(requests).toContain(200));
      await waitFor(() => expect(requests).toContain(150));
      // no pane replays its last 이동, and the now-focused second pane does not jump to the hit
      // main followed while it was in the background
      expect(main.scrollTop).toBe(1234);
      expect(second.scrollTop).toBe(4321);
    } finally {
      Object.defineProperty(window, "devicePixelRatio", { configurable: true, value: original });
    }
  });
});
