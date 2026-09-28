/**
 * UI_SPEC §12 "Text selection" in the canvas context menu (Stage 8): the items and their order,
 * and each one against the mock adapter — 복사 (inside the click), 형광펜 · 밑줄 · 취소선 (one
 * markup per page, the tool's style), 메모 추가 (a note after the selection, opened), 영역 표시로
 * 표시 (→ 편집 · 영역 표시), 검색 (a closed panel opens with the query; an open one gets it typed).
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import { mock } from "../ipc/mock";
import { ensureTextLayer, getTextLayer, selectionText, useSearchStore, useSelectionStore } from "../viewer";
import { useAppStore } from "../store/appStore";
import { useAnnotStore } from "../store/annotStore";
import { useDocStore } from "../store/docStore";
import { styleFor } from "../store/toolStyles";
import { useEditStore } from "../edit/editStore";
import { SearchPanel } from "../sidebar/SearchPanel";
import { isSeparator, useContextMenuStore, type MenuItem } from "./contextMenuStore";
import { openPageContextMenu } from "./pageMenus";

const TEXT_IDS = [
  "copyText", "highlightSelection", "underlineSelection", "strikeoutSelection", "noteSelection",
  "redactSelection", "searchSelection",
];

async function openWithSelection(chars = 60) {
  const info = await useDocStore.getState().open("/tmp/sample.pdf");
  if (!info) throw new Error("mock open failed");
  useAppStore.setState({ mode: "read", tool: "select", sidebarTab: "thumbnails" });
  ensureTextLayer(info.docId, info.docGeneration, 0);
  await waitFor(() => expect(getTextLayer(info.docId, info.docGeneration, 0)).not.toBeNull());
  const layer = getTextLayer(info.docId, info.docGeneration, 0)!;
  const selection = { docId: info.docId, anchor: { page: 0, offset: 0 }, focus: { page: 0, offset: Math.min(layer.charCount, chars) } };
  useSelectionStore.getState().setSelection(selection);
  return { info, text: selectionText(selection, info.docGeneration) };
}

function menuItems(): MenuItem[] {
  openPageContextMenu(0, "canvas", 10, 10);
  return (useContextMenuStore.getState().menu?.items ?? []).filter((i): i is MenuItem => !isSeparator(i));
}

function choose(id: string) {
  const item = menuItems().find((i) => i.id === id);
  if (!item) throw new Error(`no menu item ${id}`);
  item.onSelect?.();
}

beforeEach(() => {
  useEditStore.getState().reset();
  useSearchStore.getState().clear();
  useAnnotStore.setState({ editing: null });
});

afterEach(() => {
  useContextMenuStore.getState().close();
  useSelectionStore.getState().clear();
  vi.restoreAllMocks();
});

describe("canvas menu · text selection", () => {
  it("lists 복사 · 형광펜 · 밑줄 · 취소선 · 메모 추가 · 영역 표시로 표시 · 검색 first, only with a selection", async () => {
    await openWithSelection();
    const ids = menuItems().map((i) => i.id);
    expect(ids.slice(0, TEXT_IDS.length)).toEqual(TEXT_IDS);
    expect(menuItems().find((i) => i.id === "copyText")?.shortcut).toBeTruthy();

    useSelectionStore.getState().clear();
    expect(menuItems().some((i) => TEXT_IDS.includes(i.id))).toBe(false);
  });

  it("복사 writes the selected text inside the click", async () => {
    const { text } = await openWithSelection();
    let copied = "";
    const exec = vi.fn(() => {
      copied = (document.activeElement as HTMLTextAreaElement).value;
      return true;
    });
    Object.defineProperty(document, "execCommand", { value: exec, configurable: true, writable: true });
    choose("copyText");
    expect(exec).toHaveBeenCalledWith("copy");
    expect(copied).toBe(text);
    expect(text.length).toBeGreaterThan(10);
  });

  it.each([
    ["highlightSelection", "highlight"],
    ["underlineSelection", "underline"],
    ["strikeoutSelection", "strikeout"],
  ] as const)("%s makes a %s markup from the selection's lines in the tool's style", async (id, kind) => {
    await openWithSelection(120);
    const create = vi.spyOn(mock, "createAnnotation");
    choose(id);
    await waitFor(() => expect(create).toHaveBeenCalledTimes(1));
    const { page, spec } = create.mock.calls[0][0];
    expect(page).toBe(0);
    expect(spec.kind).toBe(kind);
    const style = styleFor(kind, useAnnotStore.getState().toolDefaults);
    expect(spec).toMatchObject({ color: style.color, opacity: kind === "highlight" ? style.opacity : 1 });
    expect("rects" in spec && spec.rects.length).toBeGreaterThanOrEqual(1);
    // the markup replaces the selection it was made from
    expect(useSelectionStore.getState().selection).toBeNull();
  });

  it("메모 추가 puts a note just after the selection and opens it", async () => {
    await openWithSelection();
    const create = vi.spyOn(mock, "createAnnotation");
    choose("noteSelection");
    await waitFor(() => expect(create).toHaveBeenCalledTimes(1));
    const { spec } = create.mock.calls[0][0];
    expect(spec).toMatchObject({ kind: "note", contents: "" });
    await waitFor(() => expect(useAnnotStore.getState().editing?.page).toBe(0));
    expect(useSelectionStore.getState().selection).toBeNull();
  });

  it("영역 표시로 표시 marks the lines and arms 편집 · 영역 표시", async () => {
    await openWithSelection();
    choose("redactSelection");
    await waitFor(() => expect(useEditStore.getState().marks.length).toBeGreaterThan(0));
    expect(useAppStore.getState()).toMatchObject({ mode: "edit", tool: "redact" });
  });

  it("검색 opens a closed 검색 panel with the selection as its query", async () => {
    const { text } = await openWithSelection(40);
    choose("searchSelection");
    await waitFor(() => expect(useAppStore.getState().sidebarTab).toBe("search"));
    expect(useSearchStore.getState().query).toBe(text.replace(/\s+/g, " ").trim());
  });

  it("검색 types the query into an open panel, which runs it", async () => {
    const { text } = await openWithSelection(40);
    const start = vi.spyOn(mock, "searchStart");
    useAppStore.setState({ sidebarTab: "search", sidebarOpen: true });
    render(<SearchPanel />);
    const field = screen.getByRole("searchbox") as HTMLInputElement;
    choose("searchSelection");
    const query = text.replace(/\s+/g, " ").trim();
    await waitFor(() => expect(field.value).toBe(query));
    await waitFor(() => expect(useSearchStore.getState().query).toBe(query));
    expect(start).toHaveBeenCalled();
    expect(start.mock.calls.at(-1)![0]).toMatchObject({ query });
  });
});
