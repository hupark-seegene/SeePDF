/**
 * 분할 보기 (P2) end to end against the mock: ⌥⌘S opens a second pane of the same document, the
 * second pane scrolls to page 5 on its own while the first stays put, the annotation tools live in
 * the focused pane only (a click moves them), and Esc closes the split.
 */
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import App from "../App";
import { useAppStore } from "../store/appStore";
import { useDocStore } from "../store/docStore";
import { paneView, useViewStore } from "../store/viewStore";
import { shortcutFor, MENU_IDS } from "../keys/keymap";

/** 12 pages in the mock's recents. */
const CONTRACT = "/Users/veri/Documents/계약서 초안.pdf";

function panes(): HTMLElement[] {
  return [...document.querySelectorAll<HTMLElement>(".canvas.viewer")];
}

function surfaces(pane: HTMLElement): number {
  return pane.querySelectorAll(".annot-surface").length;
}

beforeEach(() => {
  useAppStore.setState({ os: "macos", mode: "read", tool: "select" });
  useViewStore.setState({
    split: null, parked: null, focusedPane: "main", currentPage: 0, scrollRequest: null,
    zoomPercent: 100, zoomMode: "custom", rotation: 0, layout: "continuous",
  });
});

afterEach(() => {
  useViewStore.getState().closeSplit();
});

async function openContract() {
  render(<App />);
  await waitFor(() => expect(useAppStore.getState().ready).toBe(true), { timeout: 3000 });
  await act(async () => {
    await useDocStore.getState().open(CONTRACT);
  });
  await waitFor(() => expect(document.querySelectorAll(".page-shell").length).toBeGreaterThan(0), { timeout: 3000 });
}

describe("view.split", () => {
  it("is 보기 ▸ 분할 보기, ⌥⌘S / Ctrl+Alt+S", () => {
    expect(shortcutFor("view.split", "macos")).toBe("⌥⌘S");
    expect(shortcutFor("view.split", "windows")).toBe("Ctrl+Alt+S");
    expect(MENU_IDS).toContain("view.split");
  });

  it("opens a second pane that scrolls on its own; tools follow the focused pane; Esc closes it", async () => {
    await openContract();
    act(() => useAppStore.getState().setMode("annotate"));
    act(() => useAppStore.getState().setTool("pen"));
    await waitFor(() => expect(surfaces(panes()[0])).toBeGreaterThan(0), { timeout: 4000 });

    fireEvent.keyDown(window, { key: "s", code: "KeyS", metaKey: true, altKey: true });
    await waitFor(() => expect(panes()).toHaveLength(2), { timeout: 3000 });
    const [main, second] = panes();
    expect(main.dataset.pane).toBe("main");
    expect(second.dataset.pane).toBe("second");
    expect(main).toHaveAttribute("data-focused");
    expect(screen.getByRole("region", { name: "분할 보기의 두 번째 창" })).toBe(second);
    await waitFor(() => expect(second.querySelectorAll(".page-shell").length).toBeGreaterThan(0), { timeout: 3000 });

    // the annotation surfaces (the pen's pointer layer) are in the focused pane only
    expect(surfaces(main)).toBeGreaterThan(0);
    expect(surfaces(second)).toBe(0);

    // a click in the second pane focuses it: the tools move there
    fireEvent.pointerDown(second, { button: 0, clientX: 5, clientY: 5 });
    await waitFor(() => expect(useViewStore.getState().focusedPane).toBe("second"), { timeout: 3000 });
    await waitFor(() => expect(surfaces(second)).toBeGreaterThan(0), { timeout: 3000 });
    expect(surfaces(main)).toBe(0);
    expect(second).toHaveAttribute("data-focused");
    expect(main).not.toHaveAttribute("data-focused");

    // page 5 in the focused (second) pane, through the status bar's page field
    const mainTop = main.scrollTop;
    const field = screen.getByLabelText("페이지 번호");
    fireEvent.change(field, { target: { value: "5" } });
    fireEvent.submit(field.closest("form") as HTMLFormElement);
    await waitFor(() => expect(second.scrollTop).toBeGreaterThan(0), { timeout: 3000 });
    await waitFor(() => expect(second.querySelector('.page-shell[data-page="4"]')).not.toBeNull(), { timeout: 3000 });
    const state = useViewStore.getState();
    expect(paneView(state, "second").currentPage).toBe(4);
    // …and the first pane stayed where it was
    expect(main.scrollTop).toBe(mainTop);
    expect(paneView(state, "main").currentPage).toBe(0);
    expect(main.querySelector('.page-shell[data-page="0"]')).not.toBeNull();

    // zoom is per pane too: ⌘+ zooms the focused pane only
    const mainZoom = paneView(useViewStore.getState(), "main").zoomPercent;
    fireEvent.keyDown(window, { key: "=", code: "Equal", metaKey: true });
    await waitFor(() => expect(paneView(useViewStore.getState(), "second").zoomPercent).not.toBe(mainZoom), { timeout: 3000 });
    expect(paneView(useViewStore.getState(), "main").zoomPercent).toBe(mainZoom);

    // Esc with a tool armed disarms it first; the next Esc closes the split
    second.focus();
    fireEvent.keyDown(second, { key: "Escape", code: "Escape" });
    await waitFor(() => expect(useAppStore.getState().tool).toBe("select"), { timeout: 3000 });
    expect(useViewStore.getState().split).not.toBeNull();
    fireEvent.keyDown(second, { key: "Escape", code: "Escape" });
    await waitFor(() => expect(panes()).toHaveLength(1), { timeout: 3000 });
    expect(useViewStore.getState()).toMatchObject({ split: null, focusedPane: "main", currentPage: 0 });
    // the main pane was never remounted: it is the same element
    expect(panes()[0]).toBe(main);
  });

  it("the status bar toggles the split, its orientation and 동기화 스크롤", async () => {
    await openContract();
    fireEvent.click(screen.getByRole("button", { name: "분할 보기" }));
    await waitFor(() => expect(panes()).toHaveLength(2), { timeout: 3000 });
    expect(document.querySelector(".viewer-panes")).toHaveAttribute("data-split", "side");
    fireEvent.click(screen.getByRole("button", { name: "위아래로 나누기" }));
    await waitFor(() => expect(document.querySelector(".viewer-panes")).toHaveAttribute("data-split", "stacked"), { timeout: 3000 });
    const sync = screen.getByRole("button", { name: "동기화 스크롤" });
    expect(sync).toHaveAttribute("aria-pressed", "false");
    fireEvent.click(sync);
    expect(useViewStore.getState().split?.sync).toBe(true);
    fireEvent.click(screen.getByRole("button", { name: "분할 보기" }));
    await waitFor(() => expect(panes()).toHaveLength(1), { timeout: 3000 });
  });

  it("closes when another document replaces this one", async () => {
    await openContract();
    act(() => useViewStore.getState().openSplit());
    await waitFor(() => expect(panes()).toHaveLength(2), { timeout: 3000 });
    await act(async () => {
      await useDocStore.getState().open("/Users/veri/Documents/SeePDF-샘플.pdf");
    });
    await waitFor(() => expect(panes()).toHaveLength(1), { timeout: 3000 });
    expect(useViewStore.getState().split).toBeNull();
  });
});
