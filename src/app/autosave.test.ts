import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { AutosaveController, autosave, autosaveSecOf, offerRecovery } from "./autosave";
import { mock, seedRecovery } from "../ipc/mock";
import * as api from "../ipc/api";
import { useDocStore } from "../store/docStore";
import { useDialogStore } from "../dialogs/dialogState";
import type { DocInfo, Settings } from "../ipc/types";

function docInfo(patch: Partial<DocInfo> = {}): DocInfo {
  return { docId: "d1", dirty: true, docGeneration: 5, name: "a.pdf" , ...patch } as DocInfo;
}

function harness(initial: DocInfo | null) {
  let current = initial;
  const write = vi.fn(async (_docId: string) => undefined as unknown);
  const clear = vi.fn(async (_docId: string) => undefined as unknown);
  const onFail = vi.fn();
  const ctl = new AutosaveController({ current: () => current, write, clear, onFail });
  return {
    ctl, write, clear, onFail,
    set(patch: Partial<DocInfo> | null) {
      current = patch === null ? null : { ...(current as DocInfo), ...patch };
      ctl.sync(current, 60);
    },
  };
}

describe("autosave.timer", () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => vi.useRealTimers());

  it("writes once per generation change, only while dirty", async () => {
    const h = harness(docInfo());
    h.ctl.sync(docInfo(), 60);
    await vi.advanceTimersByTimeAsync(59_000);
    expect(h.write).not.toHaveBeenCalled();
    await vi.advanceTimersByTimeAsync(1_000);
    expect(h.write).toHaveBeenCalledTimes(1);
    expect(h.write).toHaveBeenCalledWith("d1");

    // same generation: nothing to write
    await vi.advanceTimersByTimeAsync(180_000);
    expect(h.write).toHaveBeenCalledTimes(1);

    // an edit bumps the generation → the next beat writes again (the timer is not restarted)
    h.set({ docGeneration: 6 });
    await vi.advanceTimersByTimeAsync(60_000);
    expect(h.write).toHaveBeenCalledTimes(2);

    // clean → the timer stops
    h.set({ dirty: false, docGeneration: 7 });
    await vi.advanceTimersByTimeAsync(600_000);
    expect(h.write).toHaveBeenCalledTimes(2);
    h.ctl.stop();
  });

  it("an undo back to clean clears the copy (Stage 8); a redo writes it again", async () => {
    const h = harness(docInfo());
    h.ctl.sync(docInfo(), 60);
    await vi.advanceTimersByTimeAsync(60_000);
    expect(h.write).toHaveBeenCalledTimes(1);
    // undo → the document is clean again: the copy describes changes that are gone
    h.set({ dirty: false, docGeneration: 6 });
    await vi.advanceTimersByTimeAsync(0);
    expect(h.clear).toHaveBeenCalledTimes(1);
    expect(h.clear).toHaveBeenCalledWith("d1");
    // a second clean sync has nothing left to clear
    h.set({ dirty: false, docGeneration: 6 });
    await vi.advanceTimersByTimeAsync(0);
    expect(h.clear).toHaveBeenCalledTimes(1);
    // redo → dirty at a new generation → the next beat writes again
    h.set({ dirty: true, docGeneration: 7 });
    await vi.advanceTimersByTimeAsync(60_000);
    expect(h.write).toHaveBeenCalledTimes(2);
    h.ctl.stop();
  });

  it("a clean document that never had a copy clears nothing", async () => {
    const h = harness(docInfo({ dirty: false }));
    h.ctl.sync(docInfo({ dirty: false }), 60);
    await vi.advanceTimersByTimeAsync(0);
    expect(h.clear).not.toHaveBeenCalled();
  });

  it("does nothing when autosave is off or no document is open", async () => {
    const h = harness(docInfo());
    h.ctl.sync(docInfo(), 0);
    await vi.advanceTimersByTimeAsync(600_000);
    h.ctl.sync(null, 60);
    await vi.advanceTimersByTimeAsync(600_000);
    expect(h.write).not.toHaveBeenCalled();
  });

  it("clears the copy after a save and does not write it straight back", async () => {
    const h = harness(docInfo());
    h.ctl.sync(docInfo(), 30);
    await vi.advanceTimersByTimeAsync(30_000);
    expect(h.write).toHaveBeenCalledTimes(1);

    // saved: generation moves, the doc-changed that clears `dirty` has not arrived yet
    h.set({ docGeneration: 6 });
    await h.ctl.clear("d1");
    expect(h.clear).toHaveBeenCalledWith("d1");
    await vi.advanceTimersByTimeAsync(60_000);
    expect(h.write).toHaveBeenCalledTimes(1);

    // a second clear has nothing on disk to remove
    await h.ctl.clear("d1");
    expect(h.clear).toHaveBeenCalledTimes(1);
    h.ctl.stop();
  });

  it("never clears a document it never wrote", async () => {
    const h = harness(docInfo());
    await h.ctl.clear("d1");
    expect(h.clear).not.toHaveBeenCalled();
  });

  it("toasts a failing write once per document", async () => {
    const h = harness(docInfo());
    h.write.mockRejectedValue(new Error("disk full"));
    h.ctl.sync(docInfo(), 60);
    await vi.advanceTimersByTimeAsync(60_000);
    h.set({ docGeneration: 6 });
    await vi.advanceTimersByTimeAsync(60_000);
    h.set({ docGeneration: 7 });
    await vi.advanceTimersByTimeAsync(60_000);
    expect(h.write).toHaveBeenCalledTimes(3);
    expect(h.onFail).toHaveBeenCalledTimes(1);
    h.ctl.stop();
  });

  it("reads the interval with the pre-Stage-5 default", () => {
    expect(autosaveSecOf(null)).toBe(60);
    expect(autosaveSecOf({} as Settings)).toBe(60);
    expect(autosaveSecOf({ autosaveSec: 0 } as Settings)).toBe(0);
    expect(autosaveSecOf({ autosaveSec: 300 } as Settings)).toBe(300);
  });
});

describe("autosave.flows", () => {
  beforeEach(() => {
    autosave.reset();
    useDialogStore.getState().closeAll();
  });

  it("저장 drops the recovery copy; a clean close does too", async () => {
    const clear = vi.spyOn(mock, "clearRecovery");
    const info = await useDocStore.getState().open("/Users/veri/Documents/SeePDF-샘플.pdf");
    await api.pageOps({ docId: info!.docId, ops: [{ kind: "rotate", pages: [0], delta: 90 }] });
    await useDocStore.getState().refresh();
    expect(useDocStore.getState().info?.dirty).toBe(true);

    await autosave.tick();
    expect(await api.listRecovery()).toHaveLength(1);

    const flows = await import("../dialogs/flows");
    expect(await flows.saveFlow()).toBe(true);
    expect(clear).toHaveBeenCalledWith({ docId: info!.docId });
    expect(await api.listRecovery()).toHaveLength(0);

    // dirty again, copy written, then closed with nothing to save → cleared
    await api.pageOps({ docId: info!.docId, ops: [{ kind: "rotate", pages: [0], delta: 90 }] });
    await useDocStore.getState().refresh();
    await autosave.tick();
    expect(await api.listRecovery()).toHaveLength(1);
    useDocStore.setState({ info: { ...useDocStore.getState().info!, dirty: false } });
    expect(await flows.closeDocumentFlow()).toBe(true);
    expect(await api.listRecovery()).toHaveLength(0);
  });

  it("offers the 복구 dialog at launch only when copies exist", async () => {
    expect(await offerRecovery()).toBe(false);
    expect(useDialogStore.getState().stack).toHaveLength(0);
    seedRecovery([
      {
        id: "r1", originalPath: "/a/계약서.pdf", name: "계약서.pdf", savedAt: "2026-09-27T10:00:00Z",
        bytes: 2048, pages: 4, recoveryPath: "/rec/r1.pdf",
      },
    ]);
    expect(await offerRecovery()).toBe(true);
    const top = useDialogStore.getState().stack.at(-1);
    expect(top?.name).toBe("recovery");
    expect((top?.props.entries as unknown[]).length).toBe(1);
  });
});
