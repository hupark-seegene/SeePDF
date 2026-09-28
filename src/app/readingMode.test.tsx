/**
 * 읽기 모드 + 전체 화면 (P1-12): ⌃⌘R (and the native menu's `view.readingMode`) hides the title
 * bar, sidebar, tool strip, inspector and status bar and leaves the pages; Esc brings them back.
 * ⌃⌘F / `view.fullScreen` toggles the window's full screen (the DOM Fullscreen API in the mock),
 * and Esc in 읽기 모드 leaves full screen too.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import App from "../App";
import { useAppStore } from "../store/appStore";
import { useDocStore } from "../store/docStore";
import { mockEvents } from "../ipc/mock";

const SAMPLE = "/Users/veri/Documents/SeePDF-샘플.pdf";

let fullscreen: Element | null = null;
const request = vi.fn(async () => {
  fullscreen = document.documentElement;
});
const exit = vi.fn(async () => {
  fullscreen = null;
});

beforeEach(() => {
  fullscreen = null;
  request.mockClear();
  exit.mockClear();
  Object.defineProperty(document, "fullscreenElement", { configurable: true, get: () => fullscreen });
  document.documentElement.requestFullscreen = request as unknown as Element["requestFullscreen"];
  document.exitFullscreen = exit as unknown as Document["exitFullscreen"];
  useAppStore.setState({ os: "macos", mode: "annotate", tool: "select", readingMode: false, sidebarOpen: true, inspectorOpen: true });
});

afterEach(() => {
  useAppStore.setState({ readingMode: false });
});

async function openApp() {
  render(<App />);
  await waitFor(() => expect(useAppStore.getState().ready).toBe(true));
  await useDocStore.getState().open(SAMPLE);
  await screen.findByRole("tab", { name: "주석" });
  // the viewer is its own chunk (App.tsx): wait for it, the test compares chrome around it
  await screen.findByRole("region", { name: "문서 보기 영역" });
  useAppStore.setState({ mode: "annotate", inspectorOpen: true });
}

const chrome = () => ({
  modeTabs: screen.queryByRole("tab", { name: "주석" }),
  status: screen.queryByText("저장됨"),
  canvas: screen.queryByRole("region", { name: "문서 보기 영역" }),
  sidebar: document.querySelector("aside.sidebar"),
  inspector: document.querySelector("aside.inspector"),
});

describe("읽기 모드", () => {
  it("⌃⌘R hides every piece of chrome and Esc brings it back", async () => {
    await openApp();
    await waitFor(() => expect(chrome().inspector).not.toBeNull());
    expect(chrome().sidebar).not.toBeNull();
    expect(chrome().status).not.toBeNull();

    fireEvent.keyDown(window, { key: "r", code: "KeyR", metaKey: true, ctrlKey: true });
    await waitFor(() => expect(useAppStore.getState().readingMode).toBe(true));
    const on = chrome();
    expect(on.modeTabs).toBeNull();
    expect(on.status).toBeNull();
    expect(on.inspector).toBeNull();
    expect(on.sidebar).toBeNull();
    expect(screen.queryByRole("toolbar", { name: "도구" })).toBeNull();
    expect(on.canvas).not.toBeNull();
    expect(document.querySelector(".app-shell")).toHaveAttribute("data-reading");

    fireEvent.keyDown(window, { key: "Escape", code: "Escape" });
    await waitFor(() => expect(useAppStore.getState().readingMode).toBe(false));
    expect(await screen.findByRole("tab", { name: "주석" })).toBeInTheDocument();
    expect(screen.getByText("저장됨")).toBeInTheDocument();
  });

  it("answers the native menu id `view.readingMode`", async () => {
    await openApp();
    act(() => mockEvents.emit("menu:view/readingMode", {}));
    await waitFor(() => expect(useAppStore.getState().readingMode).toBe(true));
    act(() => mockEvents.emit("menu:view/readingMode", {}));
    await waitFor(() => expect(useAppStore.getState().readingMode).toBe(false));
  });

  it("closing the document leaves 읽기 모드", async () => {
    await openApp();
    act(() => useAppStore.getState().setReadingMode(true));
    act(() => useDocStore.setState({ docId: null, info: null, outline: [], status: "empty" }));
    await waitFor(() => expect(useAppStore.getState().readingMode).toBe(false));
  });
});

describe("전체 화면", () => {
  it("⌃⌘F and `view.fullScreen` toggle full screen; Esc in 읽기 모드 leaves both", async () => {
    await openApp();
    fireEvent.keyDown(window, { key: "f", code: "KeyF", metaKey: true, ctrlKey: true });
    await waitFor(() => expect(request).toHaveBeenCalledTimes(1));
    act(() => mockEvents.emit("menu:view/fullScreen", {}));
    await waitFor(() => expect(exit).toHaveBeenCalledTimes(1));

    // combined: full screen + 읽기 모드, then one Esc
    act(() => mockEvents.emit("menu:view/fullScreen", {}));
    await waitFor(() => expect(fullscreen).not.toBeNull());
    fireEvent.keyDown(window, { key: "r", code: "KeyR", metaKey: true, ctrlKey: true });
    await waitFor(() => expect(useAppStore.getState().readingMode).toBe(true));
    fireEvent.keyDown(window, { key: "Escape", code: "Escape" });
    await waitFor(() => expect(useAppStore.getState().readingMode).toBe(false));
    await waitFor(() => expect(exit).toHaveBeenCalledTimes(2));
    expect(fullscreen).toBeNull();
  });
});
