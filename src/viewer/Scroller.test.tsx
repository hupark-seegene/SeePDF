/**
 * The scroller's navigation effects (bug hunt 2026-09): a jump is a one-shot request, not a state
 * the scroller keeps re-applying; 너비 맞춤 must not feed back into itself through the current page;
 * a no-op zoom must not park a scroll target; and a display-density change re-lays the pages out.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, render, waitFor } from "@testing-library/react";
import { Scroller } from "./Scroller";
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
  });
  useSearchStore.setState({ hits: [], current: -1, navNonce: 0 });
});

afterEach(() => {
  vi.unstubAllGlobals();
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
