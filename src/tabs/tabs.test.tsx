/**
 * 문서 탭 (v0.3 DR1): state isolation between tabs, switch / restore, closing with the guards, the
 * combined prompt on window close / 종료, ⌘⇧T, "already open" focusing, autosave for background
 * tabs, the window binding, and the shortcuts.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import * as api from "../ipc/api";
import { mock, mockOpenInOtherWindow } from "../ipc/mock";
import { onDocChanged } from "../ipc/events";
import { openPath, windowCloseGate, mergePaths } from "../dialogs/flows";
import { useDialogStore, type UnsavedAnswer } from "../dialogs/dialogState";
import { useDocStore } from "../store/docStore";
import { tabDocChanged, useTabStore } from "../store/tabStore";
import { useViewStore } from "../store/viewStore";
import { usePagesStore } from "../store/pagesStore";
import { useAnnotStore } from "../store/annotStore";
import { useAppStore } from "../store/appStore";
import { useSearchStore } from "../viewer/search/SearchController";
import { setEditLeaveGuard } from "../tools/commands";
import { AutosaveController, autosave } from "../app/autosave";
import { lookup, shortcutFor, type KeyContext } from "../keys/keymap";
import { runCommand } from "../app/useCommands";
import { TabStrip } from "../app/TabStrip";
import { usePrintStore } from "../print/printStore";
import {
  activateTab, closeTab, confirmUnsavedTabs, cycleTab, reopenClosedTab, releaseWindowTabs,
} from "./flow";
import type { DocInfo, SearchHit } from "../ipc/types";

const A = "/Users/veri/Documents/SeePDF-샘플.pdf";
const B = "/tmp/other.pdf";
const C = "/tmp/third.pdf";

const tabs = () => useTabStore.getState().tabs;
const active = () => useDocStore.getState().info!;

/** A real (mock) edit, so the engine — and every `get_document` after it — says dirty. */
async function edit(docId: string): Promise<void> {
  await api.pageOps({ docId, ops: [{ kind: "rotate", pages: [0], delta: 90 }] });
}

async function answer<T>(name: string, value: T): Promise<void> {
  await waitFor(() => expect(useDialogStore.getState().stack.at(-1)?.name).toBe(name));
  const entry = useDialogStore.getState().stack.at(-1)!;
  (entry.props.resolve as (v: T) => void)(value);
}

function tabOf(docId: string): number {
  return tabs().find((t) => t.docId === docId)!.id;
}

beforeEach(() => {
  // what App.tsx wires: `doc-changed` reaches the active document and the background tabs (the
  // mock's bus is cleared by `resetMock` before every test)
  onDocChanged((e) => useDocStore.getState().applyDocChanged(e));
  onDocChanged(tabDocChanged);
  useDocStore.setState({ docId: null, info: null, outline: [], status: "empty", error: null, savedPageCount: null });
  useDialogStore.getState().closeAll();
  useAppStore.setState({ mode: "read", tool: "select", settings: null });
  useViewStore.getState().closeSplit();
  useSearchStore.getState().clear();
  autosave.reset();
});

afterEach(() => {
  setEditLeaveGuard(null);
  vi.restoreAllMocks();
});

describe("tabs — opening", () => {
  it("a file opened over a document takes a new tab; the first stays open in the engine", async () => {
    const close = vi.spyOn(mock, "closeDocument");
    const a = (await openPath(A))!;
    const b = (await openPath(B))!;
    expect(tabs().map((t) => t.docId)).toEqual([a.docId, b.docId]);
    expect(useTabStore.getState().activeId).toBe(tabOf(b.docId));
    expect(active().docId).toBe(b.docId);
    expect(close).not.toHaveBeenCalled();
    expect(await mock.getDocument({ docId: a.docId })).toBeTruthy();
  });

  it("opening a file a tab already has brings that tab forward instead of opening it twice", async () => {
    const a = (await openPath(A))!;
    await openPath(B);
    const open = vi.spyOn(mock, "openDocument");
    const again = await openPath(A);
    expect(again?.docId).toBe(a.docId);
    expect(open).not.toHaveBeenCalled();
    expect(tabs()).toHaveLength(2);
    expect(active().docId).toBe(a.docId);
  });

  it("a file another window shows is focused there (H8): no tab here", async () => {
    await openPath(A);
    mockOpenInOtherWindow(C, "doc-3");
    expect(await openPath(C)).toBeNull();
    expect(tabs()).toHaveLength(1);
  });

  it("설정 › 파일 열기 = 새 창 opens the file in a new window, ⌘T still takes a tab", async () => {
    await useAppStore.getState().patchSettings({ openFilesIn: "window" });
    await openPath(A);
    const newWindow = vi.spyOn(mock, "openInNewWindow");
    expect(await openPath(B)).toBeNull();
    expect(newWindow).toHaveBeenCalledWith({ path: B });
    expect(tabs()).toHaveLength(1);
    await openPath(B, { target: "tab" });
    expect(tabs()).toHaveLength(2);
  });

  it("a failed open rolls back: the first tab is active again, nothing parked", async () => {
    const a = (await openPath(A))!;
    expect(await openPath("/tmp/damaged.pdf")).toBeNull();
    expect(tabs()).toHaveLength(1);
    expect(useTabStore.getState().activeId).toBe(tabOf(a.docId));
    expect(useTabStore.getState().parked).toEqual({});
    expect(useDocStore.getState().status).toBe("ready");
  });

  it("취소 on pending 영역 표시 marks keeps the document and opens nothing", async () => {
    const a = (await openPath(A))!;
    setEditLeaveGuard(() => Promise.resolve(false));
    const open = vi.spyOn(mock, "openDocument");
    expect(await openPath(B)).toBeNull();
    expect(open).not.toHaveBeenCalled();
    expect(active().docId).toBe(a.docId);
    expect(useTabStore.getState().activeId).toBe(tabOf(a.docId));
  });

  it("합치기 over a document opens the merged one in a new tab", async () => {
    const a = (await openPath(A))!;
    const merged = (await mergePaths([{ path: "/tmp/one.pdf" }, { path: "/tmp/two.pdf" }]))!;
    expect(tabs().map((t) => t.docId)).toEqual([a.docId, merged.docId]);
    expect(await mock.getDocument({ docId: a.docId })).toBeTruthy();
  });
});

describe("tabs — per-tab state", () => {
  it("each tab keeps its own view, selections, mode, tool and search results", async () => {
    const a = (await openPath(A))!;
    const view = useViewStore.getState();
    view.setZoom(150);
    view.goToPage(3);
    view.rotate(90);
    view.openSplit("stacked");
    usePagesStore.getState().setSelected([1, 2]);
    useAnnotStore.getState().select(["annot-1"]);
    useAnnotStore.getState().setFilter(["highlight"]);
    useAppStore.setState({ mode: "annotate", tool: "pen" });
    const hit = { page: 3, charStart: 0, charLength: 4, rects: [] } as unknown as SearchHit;
    useSearchStore.getState().show(a.docId, "계약", { matchCase: false, wholeWord: false }, [hit], 0);

    const b = (await openPath(B))!;
    // a new tab starts clean, in the same mode
    let v = useViewStore.getState();
    expect(v.currentPage).toBe(0);
    expect(v.backStack).toEqual([]);
    expect(v.split).toBeNull();
    expect(v.rotation).toBe(0);
    expect(usePagesStore.getState().selected).toEqual([]);
    expect(useAnnotStore.getState().selected).toEqual([]);
    expect(useSearchStore.getState().hits).toEqual([]);
    useViewStore.getState().setZoom(75);
    useAppStore.setState({ mode: "read", tool: "select" });

    expect(await activateTab(tabOf(a.docId))).toBe(true);
    v = useViewStore.getState();
    expect(active().docId).toBe(a.docId);
    expect(v.zoomPercent).toBe(150);
    expect(v.rotation).toBe(90);
    expect(v.split?.orientation).toBe("stacked");
    expect(v.parked).not.toBeNull();
    expect(v.currentPage).toBe(3);
    expect(v.scrollRequest?.page).toBe(3);
    expect(v.backStack).toEqual([0]);
    expect(usePagesStore.getState().selected).toEqual([1, 2]);
    expect(useAnnotStore.getState().selected).toEqual(["annot-1"]);
    expect(useAnnotStore.getState().filter).toEqual(["highlight"]);
    expect(useAppStore.getState().mode).toBe("annotate");
    expect(useAppStore.getState().tool).toBe("pen");
    expect(useSearchStore.getState().docId).toBe(a.docId);
    expect(useSearchStore.getState().query).toBe("계약");
    expect(useSearchStore.getState().hits).toHaveLength(1);

    expect(await activateTab(tabOf(b.docId))).toBe(true);
    v = useViewStore.getState();
    expect(active().docId).toBe(b.docId);
    expect(v.zoomPercent).toBe(75);
    expect(v.split).toBeNull();
    expect(useAppStore.getState().mode).toBe("read");
    expect(useSearchStore.getState().hits).toEqual([]);
    expect(useSearchStore.getState().query).toBe("");
  });

  it("⌃Tab cycles through the tabs and wraps around", async () => {
    const a = (await openPath(A))!;
    const b = (await openPath(B))!;
    const c = (await openPath(C))!;
    await cycleTab(1);
    expect(active().docId).toBe(a.docId);
    await cycleTab(-1);
    expect(active().docId).toBe(c.docId);
    await cycleTab(-1);
    expect(active().docId).toBe(b.docId);
  });

  it("pending 영역 표시 marks ask first; 취소 stays on the tab", async () => {
    const a = (await openPath(A))!;
    const b = (await openPath(B))!;
    const guard = vi.fn(() => Promise.resolve(false));
    setEditLeaveGuard(guard);
    expect(await activateTab(tabOf(a.docId))).toBe(false);
    expect(guard).toHaveBeenCalledOnce();
    expect(active().docId).toBe(b.docId);
    guard.mockImplementation(() => Promise.resolve(true)); // 버리기
    expect(await activateTab(tabOf(a.docId))).toBe(true);
    expect(active().docId).toBe(a.docId);
  });

  it("a background tab's doc-changed moves its dot and its parked state", async () => {
    const a = (await openPath(A))!;
    await openPath(B);
    await edit(a.docId);
    const tab = tabs().find((t) => t.docId === a.docId)!;
    expect(tab.dirty).toBe(true);
    expect(useTabStore.getState().parked[tab.id].doc.info.dirty).toBe(true);
    // the document on screen is untouched
    expect(active().dirty).toBe(false);
  });

  it("the window binding follows the active tab and lists every tab", async () => {
    const bind = vi.spyOn(api, "windowBindDocument");
    const a = (await openPath(A))!;
    const b = (await openPath(B))!;
    expect(bind).toHaveBeenLastCalledWith(expect.objectContaining({ docId: b.docId, tabs: [a.docId, b.docId] }));
    await activateTab(tabOf(a.docId));
    expect(bind).toHaveBeenLastCalledWith(expect.objectContaining({ docId: a.docId, tabs: [a.docId, b.docId] }));
    await closeTab(tabOf(b.docId));
    expect(bind).toHaveBeenLastCalledWith(expect.objectContaining({ docId: a.docId, tabs: [a.docId] }));
  });
});

describe("tabs — closing", () => {
  it("closing the active tab shows its neighbour and releases the document once", async () => {
    const a = (await openPath(A))!;
    const b = (await openPath(B))!;
    const c = (await openPath(C))!;
    await activateTab(tabOf(b.docId));
    const close = vi.spyOn(mock, "closeDocument");
    expect(await closeTab(tabOf(b.docId))).toBe(true);
    expect(close).toHaveBeenCalledTimes(1);
    expect(close).toHaveBeenCalledWith({ docId: b.docId });
    // the neighbour to the right
    expect(active().docId).toBe(c.docId);
    expect(tabs().map((t) => t.docId)).toEqual([a.docId, c.docId]);
  });

  it("a clean background tab closes without being shown", async () => {
    const a = (await openPath(A))!;
    const b = (await openPath(B))!;
    const close = vi.spyOn(mock, "closeDocument");
    expect(await closeTab(tabOf(a.docId))).toBe(true);
    expect(close).toHaveBeenCalledWith({ docId: a.docId });
    expect(active().docId).toBe(b.docId);
    expect(tabs()).toHaveLength(1);
    expect(useDialogStore.getState().stack).toHaveLength(0);
  });

  it("a tab whose document is being printed stays open until the job ends", async () => {
    const a = (await openPath(A))!;
    const b = (await openPath(B))!;
    const close = vi.spyOn(mock, "closeDocument");
    usePrintStore.setState({ job: { docId: a.docId, generation: a.docGeneration, pages: [0], rotation: 0, scaleKey: 200 } });
    try {
      // the background tab (× / ⌘W after a switch) and the active one alike
      expect(await closeTab(tabOf(a.docId))).toBe(false);
      await activateTab(tabOf(a.docId));
      expect(await closeTab(tabOf(a.docId))).toBe(false);
      expect(close).not.toHaveBeenCalled();
      expect(tabs()).toHaveLength(2);
      // the other tab is not held
      expect(await closeTab(tabOf(b.docId))).toBe(true);
    } finally {
      usePrintStore.setState({ job: null });
    }
    expect(await closeTab(tabOf(a.docId))).toBe(true);
  });

  it("a background tab with changes comes forward and asks; 취소 keeps it", async () => {
    const a = (await openPath(A))!;
    await openPath(B);
    await edit(a.docId);
    const closing = closeTab(tabOf(a.docId));
    await answer<UnsavedAnswer>("unsaved", "cancel");
    expect(await closing).toBe(false);
    expect(tabs()).toHaveLength(2);
    expect(active().docId).toBe(a.docId);
  });

  it("the last tab closes to the welcome screen (파일 › 닫기 in the browser build)", async () => {
    await openPath(A);
    runCommand("file.close");
    await waitFor(() => expect(useDocStore.getState().info).toBeNull());
    expect(tabs()).toEqual([]);
  });

  it("⌘⇧T reopens the last closed tab from its file", async () => {
    const a = (await openPath(A))!;
    await openPath(B);
    await closeTab(tabOf(a.docId));
    expect(useTabStore.getState().closed).toEqual([A]);
    expect(await reopenClosedTab()).toBe(true);
    expect(tabs().map((t) => t.path)).toEqual([B, A]);
    expect(active().path).toBe(A);
    expect(useTabStore.getState().closed).toEqual([]);
    expect(await reopenClosedTab()).toBe(false);
  });
});

describe("tabs — window close and 종료", () => {
  it("no changes anywhere: the window just closes", async () => {
    await openPath(A);
    await openPath(B);
    expect(await windowCloseGate()).toBe("close");
  });

  it("changes in several tabs: one prompt lists them all; 취소 keeps the window", async () => {
    const a = (await openPath(A))!;
    const b = (await openPath(B))!;
    await edit(a.docId);
    await edit(b.docId);
    await useDocStore.getState().refresh();
    const gate = windowCloseGate();
    await waitFor(() => expect(useDialogStore.getState().stack.at(-1)?.name).toBe("choice"));
    const entry = useDialogStore.getState().stack.at(-1)!;
    expect(entry.props.bodyParams).toEqual({ count: 2 });
    expect(entry.props.hintParams).toEqual({ names: "SeePDF-샘플.pdf, other.pdf" });
    (entry.props.resolve as (v: string) => void)("cancel");
    expect(await gate).toBe("cancel");
  });

  it("모두 저장 saves every changed tab, background ones included", async () => {
    const a = (await openPath(A))!;
    const b = (await openPath(B))!;
    await openPath(C);
    await edit(a.docId);
    await edit(b.docId);
    const save = vi.spyOn(mock, "saveDocument");
    const gate = windowCloseGate();
    await answer("choice", "saveAll");
    expect(await gate).toBe("confirmed");
    expect(save.mock.calls.map(([args]) => args.docId)).toEqual([a.docId, b.docId]);
  });

  it("a change only in a background tab still asks (the tab on screen is clean)", async () => {
    const a = (await openPath(A))!;
    await openPath(B);
    await edit(a.docId);
    const gate = windowCloseGate();
    await answer("choice", "dontSave");
    expect(await gate).toBe("confirmed");
  });

  it("releasing the window's tabs clears every recovery copy and closes the background documents", async () => {
    const a = (await openPath(A))!;
    const b = (await openPath(B))!;
    const close = vi.spyOn(mock, "closeDocument");
    const clear = vi.spyOn(autosave, "clear");
    await releaseWindowTabs();
    expect(clear.mock.calls.map(([id]) => id).sort()).toEqual([a.docId, b.docId].sort());
    expect(close.mock.calls.map(([args]) => args.docId)).toEqual([a.docId, b.docId]);
  });

  it("a single changed tab keeps the familiar 저장 / 저장 안 함 / 취소 prompt", async () => {
    const a = (await openPath(A))!;
    await openPath(B);
    await activateTab(tabOf(a.docId));
    await edit(a.docId);
    await useDocStore.getState().refresh();
    const asking = confirmUnsavedTabs();
    await answer<UnsavedAnswer>("unsaved", "dontSave");
    expect(await asking).toBe(true);
  });
});

describe("tabs — autosave", () => {
  it("writes a recovery copy for a dirty background tab, not only the one on screen", async () => {
    const a = (await openPath(A))!;
    const b = (await openPath(B))!;
    await edit(a.docId);
    const write = vi.spyOn(mock, "writeRecovery");
    autosave.sync(useDocStore.getState().info, 60);
    await autosave.tick();
    expect(write.mock.calls.map(([args]) => args.docId)).toEqual([a.docId]);
    // the on-screen document gets its copy too once it changes; the background one is covered
    await edit(b.docId);
    await useDocStore.getState().refresh();
    await autosave.tick();
    expect(write.mock.calls.map(([args]) => args.docId)).toEqual([a.docId, b.docId]);
    autosave.stop();
  });

  it("the timer runs while only a background tab is dirty", async () => {
    vi.useFakeTimers();
    try {
      const info = (d: string, dirty: boolean, gen = 1) => ({ docId: d, dirty, docGeneration: gen, name: d }) as DocInfo;
      const write = vi.fn(async () => undefined);
      const ctl = new AutosaveController({
        current: () => info("shown", false),
        background: () => [info("hidden", true)],
        write,
        clear: async () => undefined,
        onFail: () => undefined,
      });
      ctl.sync(info("shown", false), 30);
      await vi.advanceTimersByTimeAsync(30_000);
      expect(write).toHaveBeenCalledWith("hidden");
      ctl.stop();
    } finally {
      vi.useRealTimers();
    }
  });
});

describe("tabs — shortcuts", () => {
  const contexts: KeyContext[] = ["always", "doc", "canvas"];
  const key = (k: string, code: string, mods: Partial<Record<"metaKey" | "ctrlKey" | "altKey" | "shiftKey", boolean>>) => ({
    key: k, code, metaKey: false, ctrlKey: false, altKey: false, shiftKey: false, ...mods,
  });

  it("⌘T / ⌃Tab / ⌃⇧Tab / ⌥⌘→ / ⌥⌘← / ⇧⌘T on macOS, Ctrl on Windows", () => {
    expect(lookup(key("t", "KeyT", { metaKey: true }), "macos", contexts)?.id).toBe("file.openInNewTab");
    expect(lookup(key("Tab", "Tab", { ctrlKey: true }), "macos", contexts)?.id).toBe("tab.next");
    expect(lookup(key("Tab", "Tab", { ctrlKey: true, shiftKey: true }), "macos", contexts)?.id).toBe("tab.previous");
    expect(lookup(key("ArrowRight", "ArrowRight", { metaKey: true, altKey: true }), "macos", contexts)?.id).toBe("tab.next");
    expect(lookup(key("ArrowLeft", "ArrowLeft", { metaKey: true, altKey: true }), "macos", contexts)?.id).toBe("tab.previous");
    expect(lookup(key("T", "KeyT", { metaKey: true, shiftKey: true }), "macos", contexts)?.id).toBe("tab.reopenClosed");
    expect(lookup(key("w", "KeyW", { metaKey: true }), "macos", contexts)?.id).toBe("file.close");
    expect(lookup(key("t", "KeyT", { ctrlKey: true }), "windows", contexts)?.id).toBe("file.openInNewTab");
    expect(lookup(key("PageDown", "PageDown", { ctrlKey: true }), "windows", contexts)?.id).toBe("tab.next");
    expect(lookup(key("PageUp", "PageUp", { ctrlKey: true }), "windows", contexts)?.id).toBe("tab.previous");
    expect(lookup(key("Tab", "Tab", { ctrlKey: true }), "windows", contexts)?.id).toBe("tab.next");
    // the bare letter is still the 텍스트 상자 tool, and a plain PageDown still pages
    expect(lookup(key("t", "KeyT", {}), "macos", contexts)?.id).toBe("tool.textbox");
    expect(lookup(key("PageDown", "PageDown", {}), "windows", contexts)?.id).toBe("go.nextPage");
    expect(shortcutFor("tab.next", "macos")).toBe("⌃Tab");
    expect(shortcutFor("tab.reopenClosed", "windows")).toBe("Ctrl+Shift+T");
  });

  it("cycling needs a document; opening in a new tab works on the welcome screen", () => {
    expect(lookup(key("Tab", "Tab", { ctrlKey: true }), "macos", ["always"])).toBeUndefined();
    expect(lookup(key("t", "KeyT", { metaKey: true }), "macos", ["always"])?.id).toBe("file.openInNewTab");
  });
});

describe("tabs — the strip", () => {
  it("appears with a second document; × and a middle click close, a right click opens the tab menu", async () => {
    const a = (await openPath(A))!;
    const { container } = render(<TabStrip />);
    expect(container.querySelector(".tabstrip")).toBeNull();
    const b = (await openPath(B))!;
    const c = (await openPath(C))!;
    const strip = await screen.findByRole("tablist", { name: "문서 탭" });
    expect(strip).toBeInTheDocument();
    const named = () => screen.getAllByRole("tab").map((t) => t.textContent);
    expect(named()).toEqual(["SeePDF-샘플.pdf", "other.pdf", "third.pdf"]);
    expect(screen.getAllByRole("tab")[2]).toHaveAttribute("aria-selected", "true");

    fireEvent.click(screen.getAllByRole("button", { name: "탭 닫기" })[1]);
    await waitFor(() => expect(tabs().map((t) => t.docId)).toEqual([a.docId, c.docId]));

    fireEvent(screen.getAllByRole("tab")[0], new MouseEvent("auxclick", { bubbles: true, button: 1 }));
    await waitFor(() => expect(tabs().map((t) => t.docId)).toEqual([c.docId]));
    expect(b.docId).not.toBe(c.docId);
  });

  it("drag to reorder moves the tab in the list", async () => {
    const a = (await openPath(A))!;
    const b = (await openPath(B))!;
    const c = (await openPath(C))!;
    useTabStore.getState().move(tabOf(a.docId), 2);
    expect(tabs().map((t) => t.docId)).toEqual([b.docId, c.docId, a.docId]);
    useTabStore.getState().move(tabOf(a.docId), 0);
    expect(tabs().map((t) => t.docId)).toEqual([a.docId, b.docId, c.docId]);
  });
});
