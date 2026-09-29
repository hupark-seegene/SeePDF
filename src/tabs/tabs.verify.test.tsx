/**
 * v0.3.0 final verification — regressions of 문서 탭 (DR1) against the mock adapter: answers that
 * arrive after the user switched tabs, a switch while a file opens into a new tab, a save that
 * finishes in the background, the native menu under a modal sheet, S1 on a background tab,
 * 모두 저장 during 문서 비교, an engine crash of the tab on screen, and "already open" under
 * another spelling of the path.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { waitFor } from "@testing-library/react";
import * as api from "../ipc/api";
import { mock } from "../ipc/mock";
import { onDocChanged, onDocSaved } from "../ipc/events";
import { openPath, recoverFromEngineCrash, saveFlow, windowCloseGate } from "../dialogs/flows";
import { useDialogStore } from "../dialogs/dialogState";
import { useDocStore } from "../store/docStore";
import { tabDocChanged, useTabStore } from "../store/tabStore";
import { useViewStore } from "../store/viewStore";
import { useAppStore } from "../store/appStore";
import { useSearchStore } from "../viewer/search/SearchController";
import { useCompareStore } from "../compare/state";
import { setEditLeaveGuard } from "../tools/commands";
import { autosave } from "../app/autosave";
import { runCommand } from "../app/useCommands";
import { installSignatureGate, resetSignatureGate } from "../app/signatureGate";
import { closeOcrDialog, openOcrDialog } from "../ocr/dialogState";
import { activateTab, closeTab, tabDocSaved } from "./flow";
import type { CompareReport, DocInfo } from "../ipc/types";

const A = "/Users/veri/Documents/SeePDF-샘플.pdf";
const B = "/tmp/other.pdf";
const C = "/tmp/third.pdf";

const tabs = () => useTabStore.getState().tabs;
const active = () => useDocStore.getState().info!;
const tabOf = (docId: string) => tabs().find((t) => t.docId === docId)!.id;

function deferred(): { promise: Promise<void>; release: () => void } {
  let release = () => undefined as void;
  const promise = new Promise<void>((resolve) => {
    release = resolve;
  });
  return { promise, release };
}

async function edit(docId: string): Promise<void> {
  await api.pageOps({ docId, ops: [{ kind: "rotate", pages: [0], delta: 90 }] });
}

async function answer<T>(name: string, value: T): Promise<void> {
  await waitFor(() => expect(useDialogStore.getState().stack.at(-1)?.name).toBe(name));
  const entry = useDialogStore.getState().stack.at(-1)!;
  (entry.props.resolve as (v: T) => void)(value);
}

beforeEach(() => {
  onDocChanged((e) => useDocStore.getState().applyDocChanged(e));
  onDocChanged(tabDocChanged);
  onDocSaved(tabDocSaved);
  useDocStore.setState({ docId: null, info: null, outline: [], status: "empty", error: null, savedPageCount: null });
  useDialogStore.getState().closeAll();
  useAppStore.setState({ mode: "read", tool: "select", settings: null });
  useViewStore.getState().closeSplit();
  useSearchStore.getState().clear();
  useCompareStore.setState({ session: null });
  closeOcrDialog();
  autosave.reset();
});

afterEach(() => {
  setEditLeaveGuard(null);
  api.setMutationGate(null);
  vi.restoreAllMocks();
});

describe("v0.3.0 — an answer that arrives after a tab switch", () => {
  it("a late get_document of the tab just left does not overwrite the tab on screen", async () => {
    const a = (await openPath(A))!;
    const b = (await openPath(B))!;
    await activateTab(tabOf(a.docId));
    const hold = deferred();
    const real = mock.getDocument.bind(mock);
    vi.spyOn(mock, "getDocument").mockImplementation(async (args) => {
      if (args.docId === b.docId) await hold.promise;
      return real(args);
    });
    await activateTab(tabOf(b.docId)); // restore → refresh(B), held behind B's tiles
    await activateTab(tabOf(a.docId));
    hold.release();
    await new Promise((r) => setTimeout(r, 20));
    expect(useDocStore.getState().docId).toBe(a.docId);
    expect(active().docId).toBe(a.docId);
    // the parked snapshot of B still describes B, and closing A releases A (not B)
    expect(useTabStore.getState().parked[tabOf(b.docId)].doc.info.docId).toBe(b.docId);
    const close = vi.spyOn(mock, "closeDocument");
    expect(await closeTab(tabOf(a.docId))).toBe(true);
    expect(close.mock.calls.map(([x]) => x.docId)).toEqual([a.docId]);
    expect(tabs().map((t) => t.docId)).toEqual([b.docId]);
    expect(active().docId).toBe(b.docId);
  });

  it("a late undo answer for the document left behind is not shown on the other tab", async () => {
    const a = (await openPath(A))!;
    const b = (await openPath(B))!;
    await edit(b.docId);
    const hold = deferred();
    const real = mock.undo.bind(mock);
    vi.spyOn(mock, "undo").mockImplementation(async (args) => {
      await hold.promise;
      return real(args);
    });
    const undoing = useDocStore.getState().undo();
    await activateTab(tabOf(a.docId));
    hold.release();
    await undoing;
    expect(active().docId).toBe(a.docId);
  });
});

describe("v0.3.0 — switching while a file opens into a new tab", () => {
  it("the switch is refused; the new document gets its own tab and the other tab keeps its document", async () => {
    const a = (await openPath(A))!;
    await edit(a.docId);
    await useDocStore.getState().refresh();
    const hold = deferred();
    const real = mock.openDocument.bind(mock);
    vi.spyOn(mock, "openDocument").mockImplementation(async (args) => {
      if (args.path === B) await hold.promise;
      return real(args);
    });
    const opening = openPath(B);
    await waitFor(() => expect(useTabStore.getState().opening).toBe(true));
    expect(await activateTab(tabOf(a.docId))).toBe(false);
    expect(await closeTab(tabOf(a.docId))).toBe(false);
    hold.release();
    const b = (await opening)!;
    expect(tabs().map((t) => t.docId)).toEqual([a.docId, b.docId]);
    expect(useTabStore.getState().opening).toBe(false);
    expect(active().docId).toBe(b.docId);
    // A's changes are still asked about on window close
    expect(tabs().find((t) => t.docId === a.docId)!.dirty).toBe(true);
    const gate = windowCloseGate();
    await answer("choice", "cancel");
    expect(await gate).toBe("cancel");
    // and a switch works again
    expect(await activateTab(tabOf(a.docId))).toBe(true);
    expect(active().docId).toBe(a.docId);
  });

  it("a failed open into a new tab clears the flag and brings the tab back", async () => {
    const a = (await openPath(A))!;
    expect(await openPath("/tmp/damaged.pdf")).toBeNull();
    expect(useTabStore.getState().opening).toBe(false);
    expect(useTabStore.getState().activeId).toBe(tabOf(a.docId));
    expect(await openPath(B)).not.toBeNull();
    expect(await activateTab(tabOf(a.docId))).toBe(true);
  });
});

describe("v0.3.0 — a save that finishes in the background", () => {
  it("clears that tab's dot and parked state, and records its own reading position", async () => {
    const a = (await openPath(A))!;
    const b = (await openPath(B))!;
    await activateTab(tabOf(a.docId));
    useViewStore.getState().goToPage(2);
    await edit(a.docId);
    await useDocStore.getState().refresh();
    const hold = deferred();
    const real = mock.saveDocument.bind(mock);
    vi.spyOn(mock, "saveDocument").mockImplementation(async (args, onProgress) => {
      await hold.promise;
      return real(args, onProgress);
    });
    const recent = vi.spyOn(mock, "updateRecent");
    const saving = saveFlow();
    await waitFor(() => expect(mock.saveDocument).toHaveBeenCalled());
    expect(await activateTab(tabOf(b.docId))).toBe(true);
    hold.release();
    expect(await saving).toBe(true);
    expect((await mock.getDocument({ docId: a.docId })).dirty).toBe(false);
    const tab = tabs().find((t) => t.docId === a.docId)!;
    expect(tab.dirty).toBe(false);
    expect(useTabStore.getState().parked[tab.id].doc.info.dirty).toBe(false);
    expect(active().docId).toBe(b.docId);
    expect(await windowCloseGate()).toBe("close");
    const entry = recent.mock.calls.at(-1)![0].entry;
    expect(entry.path).toBe(A);
    expect(entry.lastPage).toBe(2);
  });

  it("doc-saved alone clears a background tab's dot", async () => {
    const a = (await openPath(A))!;
    await openPath(B);
    await edit(a.docId);
    expect(tabs().find((t) => t.docId === a.docId)!.dirty).toBe(true);
    await mock.saveDocument({ docId: a.docId }, () => undefined);
    expect(tabs().find((t) => t.docId === a.docId)!.dirty).toBe(false);
    expect(useTabStore.getState().parked[tabOf(a.docId)].doc.info.dirty).toBe(false);
  });
});

describe("v0.3.0 — the native menu under a modal", () => {
  it("다음 / 이전 탭 보기 from the menu do nothing while the 텍스트 인식 sheet or a dialog is open", async () => {
    const a = (await openPath(A))!;
    const b = (await openPath(B))!;
    openOcrDialog();
    runCommand("tab.previous");
    runCommand("tab.next");
    await new Promise((r) => setTimeout(r, 20));
    expect(active().docId).toBe(b.docId);
    closeOcrDialog();
    runCommand("tab.previous");
    await waitFor(() => expect(active().docId).toBe(a.docId));
  });
});

describe("v0.3.0 — S1 on a background tab", () => {
  it("a change to a signed document in a background tab still asks, naming it", async () => {
    resetSignatureGate();
    installSignatureGate();
    const signed = (await openPath("/tmp/signed-contract.pdf"))!;
    expect(signed.signatures?.length).toBe(1);
    await openPath(B);
    const change = api.pageOps({ docId: signed.docId, ops: [{ kind: "rotate", pages: [0], delta: 90 }] });
    await waitFor(() => expect(useDialogStore.getState().stack.at(-1)?.name).toBe("confirm"));
    const entry = useDialogStore.getState().stack.at(-1)!;
    expect(entry.props.bodyKey).toBe("security.signed.editBody");
    expect(entry.props.bodyParams).toEqual({ name: "signed-contract.pdf" });
    (entry.props.resolve as (v: boolean) => void)(false);
    await expect(change).rejects.toBeTruthy();
    expect((await mock.getDocument({ docId: signed.docId })).dirty).toBe(false);
  });
});

describe("v0.3.0 — 모두 저장 while 문서 비교 is open", () => {
  it("leaves the comparison and saves every changed tab", async () => {
    const a = (await openPath(A))!;
    const b = (await openPath(B))!;
    await edit(a.docId);
    await edit(b.docId);
    await useDocStore.getState().refresh();
    const other = await mock.openDocument({ path: C });
    useCompareStore.setState({ session: { report: {} as CompareReport, infoA: b, infoB: other } });
    const save = vi.spyOn(mock, "saveDocument");
    const gate = windowCloseGate();
    await answer("choice", "saveAll");
    expect(await gate).toBe("confirmed");
    expect(save.mock.calls.map(([x]) => x.docId)).toEqual([a.docId, b.docId]);
    expect(useCompareStore.getState().session).toBeNull();
  });
});

describe("v0.3.0 — an engine crash of the tab on screen", () => {
  it("reopens the document in the same place in the strip", async () => {
    const a = (await openPath(A))!;
    const b = (await openPath(B))!;
    const c = (await openPath(C))!;
    await activateTab(tabOf(b.docId));
    const fresh = (await recoverFromEngineCrash([b.docId]))!;
    expect(fresh.path).toBe(B);
    expect(tabs().map((t) => t.docId)).toEqual([a.docId, fresh.docId, c.docId]);
    expect(useTabStore.getState().activeId).toBe(tabOf(fresh.docId));
    expect(active().docId).toBe(fresh.docId);
  });

  it("a never-saved document that cannot be reopened leaves the strip", async () => {
    await openPath(A);
    const merged = (await (await import("../dialogs/flows")).mergePaths([{ path: "/tmp/one.pdf" }, { path: "/tmp/two.pdf" }]))!;
    expect(merged.path).toBeNull();
    expect(await recoverFromEngineCrash([merged.docId])).toBeNull();
    expect(tabs().map((t) => t.docId)).not.toContain(merged.docId);
  });
});

describe("v0.3.0 — already open under another spelling of the path", () => {
  it("/private/tmp/a.pdf brings forward the tab that has /tmp/a.pdf", async () => {
    const a = (await openPath("/tmp/a.pdf"))!;
    await openPath(B);
    const open = vi.spyOn(mock, "openDocument");
    const again = (await openPath("/private/tmp/a.pdf")) as DocInfo;
    expect(again.docId).toBe(a.docId);
    expect(open).not.toHaveBeenCalled();
    expect(tabs()).toHaveLength(2);
    expect(active().docId).toBe(a.docId);
  });
});
