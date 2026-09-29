/**
 * H9 (v0.3) against the mock adapter: every mounted page carries a hidden, labelled document region
 * with its text (for NVDA / VoiceOver), page changes are announced politely, and 캐럿 탐색 (F7) lets
 * the keyboard select (⇧→ ×3 = 3 characters) and put a 메모 at the caret. Forced-colours rules exist.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, fireEvent, render, renderHook, screen, waitFor, within } from "@testing-library/react";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { mock } from "../ipc/mock";
import { useDocStore } from "../store/docStore";
import { useAppStore } from "../store/appStore";
import { useViewStore } from "../store/viewStore";
import { useCommands } from "../app/useCommands";
import { shortcutFor } from "../keys/keymap";
import { Scroller } from "./Scroller";
import { getTextLayer, resetTextLayers } from "./text/textLayers";
import { resetReadingOrders } from "./text/readingOrder";
import { selectionText, useSelectionStore } from "./text/selection";
import { caretRect, stepChar, stepLine, useCaretStore } from "./text/caret";

const SAMPLE = "/Users/veri/Documents/SeePDF-샘플.pdf";

beforeEach(() => {
  resetTextLayers();
  resetReadingOrders();
  useCaretStore.getState().set(false);
  useSelectionStore.getState().clear();
  useAppStore.setState({ mode: "read", tool: "select", os: "macos" });
  useViewStore.setState({
    zoomPercent: 100, zoomMode: "custom", layout: "continuous", rotation: 0, currentPage: 0, scrollRequest: null,
    split: null, parked: null, focusedPane: "main",
  });
});

afterEach(() => {
  vi.restoreAllMocks();
  useCaretStore.getState().set(false);
});

async function openSample() {
  const info = await useDocStore.getState().open(SAMPLE);
  if (!info) throw new Error("open failed");
  render(<Scroller info={info} />);
  await waitFor(() => expect(getTextLayer(info.docId, info.docGeneration, 0)).not.toBeNull());
  return info;
}

describe("screen readers", () => {
  it("each mounted page is a labelled document region holding its text, line by line", async () => {
    const info = await openSample();
    const regions = await screen.findAllByRole("document");
    const first = regions.find((r) => r.getAttribute("aria-label") === "1쪽")!;
    expect(first).toBeDefined();
    await waitFor(() => expect(within(first).getByText("SeePDF 샘플 문서")).toBeInTheDocument());
    expect(within(first).getByText("The mock text layer carries real word boxes in PDF points.").tagName).toBe("P");
    expect(first).toHaveClass("visually-hidden");
    // the page shell no longer duplicates the label
    expect(first.closest(".page-shell")?.getAttribute("aria-label")).toBeNull();
    // page changes are announced politely
    const announcer = screen.getByTestId("page-announcer");
    expect(announcer).toHaveAttribute("aria-live", "polite");
    expect(announcer).toHaveTextContent(`${info.pageCount}쪽 중 1쪽`);
    act(() => useViewStore.getState().setCurrentPage(2));
    expect(announcer).toHaveTextContent(`${info.pageCount}쪽 중 3쪽`);
  });

  it("a tagged page's region follows the structure tree's reading order (V6)", async () => {
    const opened = await useDocStore.getState().open(SAMPLE);
    const info = { ...opened!, tagged: true };
    const layer = await import("./text/textLayers").then((m) => m.loadTextLayer(info.docId, info.docGeneration, 0));
    const secondLine = layer.line(1);
    vi.spyOn(mock, "getReadingOrder").mockResolvedValue({
      tagged: true,
      runs: [[secondLine.firstChar, layer.charCount], [0, secondLine.firstChar]],
    });
    render(<Scroller info={info} />);
    const region = (await screen.findAllByRole("document")).find((r) => r.getAttribute("aria-label") === "1쪽")!;
    await waitFor(() => expect(region.querySelector("p")?.textContent).toBe("Lightweight PDF editor with Korean OCR"));
    expect(region.querySelector("p:last-child")?.textContent).toBe("SeePDF 샘플 문서");
  });
});

describe("캐럿 탐색 (F7)", () => {
  it("is F7 on both platforms", () => {
    expect(shortcutFor("view.caret", "macos")).toBe("F7");
    expect(shortcutFor("view.caret", "windows")).toBe("F7");
  });

  it("F7, then ⇧→ three times selects three characters; N puts a note at the caret", async () => {
    const info = await openSample();
    const { result } = renderHook(() => useCommands());
    act(() => result.current("view.caret"));
    await waitFor(() => expect(useCaretStore.getState().on).toBe(true));
    expect(useCaretStore.getState()).toMatchObject({ page: 0, offset: 0, docId: info.docId });
    expect(await screen.findByTestId("caret")).toBeInTheDocument();

    for (let i = 0; i < 3; i++) fireEvent.keyDown(window, { key: "ArrowRight", code: "ArrowRight", shiftKey: true });
    const selection = useSelectionStore.getState().selection!;
    expect(selection.anchor).toEqual({ page: 0, offset: 0 });
    expect(selection.focus).toEqual({ page: 0, offset: 3 });
    expect(selectionText(selection, info.docGeneration)).toBe("See");

    // a plain arrow moves the caret and drops the selection
    fireEvent.keyDown(window, { key: "ArrowDown", code: "ArrowDown" });
    expect(useSelectionStore.getState().selection).toBeNull();
    expect(useCaretStore.getState().offset).toBeGreaterThan(3);

    const create = vi.spyOn(mock, "createAnnotation");
    const layer = getTextLayer(info.docId, info.docGeneration, 0)!;
    const bar = caretRect(layer, useCaretStore.getState().offset)!;
    fireEvent.keyDown(window, { key: "ㅜ", code: "KeyN" });
    await waitFor(() => expect(create).toHaveBeenCalledTimes(1));
    const call = create.mock.calls[0][0];
    expect(call.page).toBe(0);
    expect(call.spec).toMatchObject({ kind: "note", at: [bar.l, bar.t], contents: "" });
    // a note is a 주석: 읽기 switched to 주석
    expect(useAppStore.getState().mode).toBe("annotate");

    act(() => result.current("view.caret"));
    await waitFor(() => expect(useCaretStore.getState().on).toBe(false));
  });

  it("moves by characters and lines, and hands over to the next page at the edge", async () => {
    const info = await openSample();
    const layer = getTextLayer(info.docId, info.docGeneration, 0)!;
    expect(stepChar(layer, 0, -1)).toBeNull();
    expect(stepChar(layer, layer.charCount, 1)).toBeNull();
    const below = stepLine(layer, 0, 1, layer.charBox(0).l)!;
    expect(layer.lineIndexOf(below)).toBe(1);
    // at the page's last character → the next page's first
    act(() => useCaretStore.getState().set(true, { docId: info.docId, page: 0, offset: layer.charCount }));
    fireEvent.keyDown(window, { key: "ArrowRight", code: "ArrowRight" });
    expect(useCaretStore.getState()).toMatchObject({ page: 1, offset: 0 });
    expect(useViewStore.getState().currentPage).toBe(1);
  });
});

describe("Windows High Contrast", () => {
  it("has forced-colors rules for the marks, the caret, focus rings and pressed buttons", () => {
    const viewer = readFileSync(join(process.cwd(), "src/viewer/viewer.css"), "utf8");
    const tokens = readFileSync(join(process.cwd(), "src/styles/tokens.css"), "utf8");
    expect(viewer).toMatch(/@media \(forced-colors: active\)[\s\S]*\.mark\.sel[\s\S]*Highlight/);
    expect(viewer).toMatch(/\.caret-bar[\s\S]*CanvasText/);
    expect(tokens).toMatch(/@media \(forced-colors: active\)[\s\S]*:focus-visible[\s\S]*outline: 2px solid Highlight/);
    expect(tokens).toMatch(/\[data-active\][\s\S]*HighlightText/);
  });
});
