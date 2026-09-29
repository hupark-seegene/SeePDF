/**
 * V3 (v0.3), 분할 보기 against the mock: the divider drags (20–80 %, remembered per orientation,
 * keyboard-operable as a `role="separator"`), and the first click in the other pane reaches the
 * armed drawing tool — a 메모 is created by that click, not by a second one.
 *
 * V1 rides along: the 스냅샷 marquee dragged on the canvas copies that page region.
 */
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import App from "../App";
import { mock } from "../ipc/mock";
import { useAppStore } from "../store/appStore";
import { useAnnotStore } from "../store/annotStore";
import { useDocStore } from "../store/docStore";
import { clampSplitRatio, useViewStore } from "../store/viewStore";
import { useDialogStore } from "../dialogs/dialogState";
import { warmLazyChunks } from "../test/warmLazy";
import { ratioAt } from "./Viewer";

/** 12 pages in the mock's recents. */
const CONTRACT = "/Users/veri/Documents/계약서 초안.pdf";

beforeAll(() => {
  // jsdom has no pointer capture; the tool surfaces call it on every press
  const proto = Element.prototype as unknown as Record<string, unknown>;
  proto.setPointerCapture ??= () => undefined;
  proto.releasePointerCapture ??= () => undefined;
  proto.hasPointerCapture ??= () => false;
  return warmLazyChunks();
});

beforeEach(() => {
  localStorage.clear();
  useAppStore.setState({ os: "macos", mode: "read", tool: "select" });
  useDialogStore.getState().closeAll();
  useAnnotStore.getState().reset();
  useViewStore.setState({
    split: null, parked: null, focusedPane: "main", currentPage: 0, scrollRequest: null,
    zoomPercent: 100, zoomMode: "custom", rotation: 0, layout: "continuous", splitRatio: { side: 0.5, stacked: 0.5 },
  });
});

afterEach(() => {
  useViewStore.getState().closeSplit();
  vi.restoreAllMocks();
});

async function openContract() {
  render(<App />);
  await waitFor(() => expect(useAppStore.getState().ready).toBe(true), { timeout: 3000 });
  await act(async () => {
    await useDocStore.getState().open(CONTRACT);
  });
  await waitFor(() => expect(document.querySelectorAll(".page-shell").length).toBeGreaterThan(0), { timeout: 3000 });
}

function panes(): HTMLElement[] {
  return [...document.querySelectorAll<HTMLElement>(".canvas.viewer")];
}

describe("분할 보기 divider", () => {
  it("the ratio math clamps to 20–80 %", () => {
    const box = { left: 100, top: 50, width: 1000, height: 400 } as DOMRect;
    expect(ratioAt("side", box, 400, 0)).toBeCloseTo(0.3, 6);
    expect(ratioAt("stacked", box, 0, 150)).toBeCloseTo(0.25, 6);
    expect(clampSplitRatio(0.05)).toBe(0.2);
    expect(clampSplitRatio(0.95)).toBe(0.8);
    expect(clampSplitRatio(Number.NaN)).toBe(0.5);
  });

  it("dragging sets the ratio, the keyboard nudges it, and it is remembered per orientation", async () => {
    await openContract();
    act(() => useViewStore.getState().openSplit());
    const divider = await screen.findByRole("separator", { name: "분할 보기 경계" });
    expect(divider).toHaveAttribute("aria-valuenow", "50");
    expect(divider).toHaveAttribute("aria-orientation", "vertical");
    const container = divider.parentElement!;
    vi.spyOn(container, "getBoundingClientRect").mockReturnValue({
      left: 0, top: 0, width: 1000, height: 800, right: 1000, bottom: 800, x: 0, y: 0, toJSON: () => ({}),
    } as DOMRect);

    fireEvent.pointerDown(divider, { button: 0, clientX: 500, clientY: 10, pointerId: 1 });
    fireEvent.pointerMove(divider, { clientX: 300, clientY: 10, pointerId: 1 });
    fireEvent.pointerUp(divider, { clientX: 300, clientY: 10, pointerId: 1 });
    expect(useViewStore.getState().splitRatio.side).toBeCloseTo(0.3, 6);
    expect(divider).toHaveAttribute("aria-valuenow", "30");
    expect(panes()[0].style.flexGrow).toBe("0.3");
    expect(panes()[1].style.flexGrow).toBe("0.7");

    // past the limit: clamped
    fireEvent.pointerDown(divider, { button: 0, clientX: 300, clientY: 10, pointerId: 1 });
    fireEvent.pointerMove(divider, { clientX: 20, clientY: 10, pointerId: 1 });
    fireEvent.pointerUp(divider, { pointerId: 1 });
    expect(useViewStore.getState().splitRatio.side).toBe(0.2);
    // a moved pointer after the release does nothing
    fireEvent.pointerMove(divider, { clientX: 900, clientY: 10, pointerId: 1 });
    expect(useViewStore.getState().splitRatio.side).toBe(0.2);

    // keyboard: → +5 %, End = 80 %
    fireEvent.keyDown(divider, { key: "ArrowRight" });
    expect(useViewStore.getState().splitRatio.side).toBeCloseTo(0.25, 6);
    fireEvent.keyDown(divider, { key: "End" });
    expect(useViewStore.getState().splitRatio.side).toBe(0.8);
    expect(JSON.parse(localStorage.getItem("seepdf.splitRatio")!)).toEqual({ side: 0.8, stacked: 0.5 });

    // 위아래 has its own ratio
    act(() => useViewStore.getState().setSplitOrientation("stacked"));
    expect(screen.getByRole("separator", { name: "분할 보기 경계" })).toHaveAttribute("aria-valuenow", "50");
    expect(screen.getByRole("separator", { name: "분할 보기 경계" })).toHaveAttribute("aria-orientation", "horizontal");
  });

  it("the first click in the other pane with the 메모 tool creates the note", async () => {
    await openContract();
    act(() => useAppStore.getState().setMode("annotate"));
    act(() => useAppStore.getState().setTool("note"));
    act(() => useViewStore.getState().openSplit());
    await waitFor(() => expect(panes()).toHaveLength(2), { timeout: 3000 });
    const second = panes()[1];
    await waitFor(() => expect(second.querySelector(".page-shell")).not.toBeNull(), { timeout: 3000 });
    expect(second.querySelector(".annot-surface")).toBeNull(); // not the focused pane yet
    const shell = second.querySelector<HTMLElement>(".page-shell")!;
    const page = Number(shell.dataset.page);
    const create = vi.spyOn(mock, "createAnnotation");

    fireEvent.pointerDown(shell, { button: 0, buttons: 1, clientX: 60, clientY: 80, pointerId: 7 });
    expect(useViewStore.getState().focusedPane).toBe("second");
    // the same press reached the tool: the surface is there and the note is committed on release
    const surface = second.querySelector<HTMLElement>(`.page-shell[data-page="${page}"] .annot-surface`)!;
    expect(surface).not.toBeNull();
    fireEvent.pointerUp(surface, { button: 0, clientX: 60, clientY: 80, pointerId: 7 });
    await waitFor(() => expect(create).toHaveBeenCalledTimes(1));
    expect(create.mock.calls[0][0]).toMatchObject({ page, spec: { kind: "note" } });
  });
});

describe("스냅샷 marquee", () => {
  it("a drag on the canvas copies that page region, then 선택 is back", async () => {
    await openContract();
    class Item {
      constructor(readonly items: Record<string, unknown>) {}
    }
    vi.stubGlobal("ClipboardItem", Item);
    const write = vi.fn(async () => undefined);
    Object.defineProperty(navigator, "clipboard", { configurable: true, value: { write } });
    const raw = vi.spyOn(mock, "renderPageRaw");
    act(() => useAppStore.getState().setTool("snapshot"));
    const canvas = panes()[0];
    const first = canvas.querySelector<HTMLElement>(".page-shell[data-page='0']")!;
    const left = parseFloat(first.style.left);
    const top = parseFloat(first.style.top);

    fireEvent.pointerDown(canvas, { button: 0, clientX: left + 20, clientY: top + 30, pointerId: 3 });
    fireEvent.pointerMove(canvas, { clientX: left + 220, clientY: top + 130, pointerId: 3 });
    const marquee = canvas.querySelector<HTMLElement>(".snapshot-marquee")!;
    expect(marquee.hidden).toBe(false);
    expect(marquee.style.width).toBe("200px");
    expect(marquee.style.height).toBe("100px");
    fireEvent.pointerUp(canvas, { clientX: left + 220, clientY: top + 130, pointerId: 3 });

    expect(write).toHaveBeenCalledTimes(1);
    expect(marquee.hidden).toBe(true);
    await waitFor(() => expect(raw).toHaveBeenCalledTimes(1));
    const { page, rect, scale } = raw.mock.calls[0][0];
    const info = useDocStore.getState().info!;
    expect(page).toBe(0);
    // 200 × 100 CSS px at the page's scale, from 20 / 30 px in from its top-left
    const s = first.getBoundingClientRect().width > 0 ? 0 : parseFloat(first.style.width) / info.pages[0].widthPt;
    expect(rect!.r - rect!.l).toBeCloseTo(200 / s, 1);
    expect(rect!.t - rect!.b).toBeCloseTo(100 / s, 0);
    expect(rect!.l).toBeCloseTo(20 / s, 1);
    expect(rect!.t).toBeCloseTo(info.pages[0].crop.t - 30 / s, 0);
    expect(scale).toBeCloseTo(2 * (useViewStore.getState().zoomPercent / 100) * (window.devicePixelRatio || 1), 6);
    expect(useAppStore.getState().tool).toBe("select");

    // any mode: in 편집 the press lands on the edit surface, and still starts the marquee
    act(() => useAppStore.getState().setMode("edit"));
    act(() => useAppStore.getState().setTool("snapshot"));
    const surface = await waitFor(() => {
      const el = canvas.querySelector<HTMLElement>(".page-shell[data-page='0'] .edit-surface");
      expect(el).not.toBeNull();
      return el!;
    });
    fireEvent.pointerDown(surface, { button: 0, clientX: left + 40, clientY: top + 40, pointerId: 4 });
    fireEvent.pointerMove(canvas, { clientX: left + 140, clientY: top + 90, pointerId: 4 });
    expect(marquee.hidden).toBe(false);
    fireEvent.pointerUp(canvas, { clientX: left + 140, clientY: top + 90, pointerId: 4 });
    expect(write).toHaveBeenCalledTimes(2);
    expect(useAppStore.getState().tool).toBe("select");
    vi.unstubAllGlobals();
  });
});
