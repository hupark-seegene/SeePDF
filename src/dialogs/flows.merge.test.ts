import { beforeEach, describe, expect, it, vi } from "vitest";
import { waitFor } from "@testing-library/react";
import { mergePaths, openPath } from "./flows";
import { useDialogStore } from "./dialogState";
import type { UnsavedAnswer } from "./dialogState";
import { useDocStore } from "../store/docStore";
import { useToastStore } from "../app/toastStore";
import { mock } from "../ipc/mock";

const A = "/Users/veri/Documents/SeePDF-샘플.pdf";
const INPUTS = [{ path: "/tmp/one.pdf" }, { path: "/tmp/two.pdf" }];

/** Is `docId` still loaded in the (mock) engine? */
async function loaded(docId: string): Promise<boolean> {
  return mock.getDocument({ docId }).then(
    () => true,
    () => false,
  );
}

/** Open A and mark it edited, so the 저장 / 저장 안 함 / 취소 gate has something to ask about. */
async function openDirtyA() {
  const a = (await openPath(A))!;
  useDocStore.setState({ info: { ...a, dirty: true } });
  return a;
}

/** Answer the unsaved prompt once it is on top of the dialog stack. */
async function answerUnsaved(answer: UnsavedAnswer): Promise<void> {
  await waitFor(() => expect(useDialogStore.getState().stack.at(-1)?.name).toBe("unsaved"));
  const entry = useDialogStore.getState().stack.at(-1)!;
  (entry.props.resolve as (v: UnsavedAnswer) => void)(answer);
}

describe("dialogs.flows.merge — the merged document replaces the window's like an open", () => {
  beforeEach(() => {
    vi.restoreAllMocks();
    useDocStore.setState({ docId: null, info: null, outline: [], status: "empty", error: null });
    useDialogStore.getState().closeAll();
    useToastStore.setState({ toasts: [] });
  });

  it("merging into an empty window closes nothing", async () => {
    const close = vi.spyOn(mock, "closeDocument");
    const merged = (await mergePaths(INPUTS))!;
    expect(merged).not.toBeNull();
    expect(close).not.toHaveBeenCalled();
    expect(useDocStore.getState().docId).toBe(merged.docId);
  });

  it("merging over A closes A in the engine, once", async () => {
    const a = (await openPath(A))!;
    const close = vi.spyOn(mock, "closeDocument");
    const merged = (await mergePaths(INPUTS))!;
    expect(merged.docId).not.toBe(a.docId);
    expect(close).toHaveBeenCalledTimes(1);
    expect(close).toHaveBeenCalledWith({ docId: a.docId });
    expect(await loaded(a.docId)).toBe(false);
    expect(await loaded(merged.docId)).toBe(true);
    const doc = useDocStore.getState();
    expect(doc.docId).toBe(merged.docId);
    expect(doc.status).toBe("ready");
    expect(useToastStore.getState().toasts.map((t) => t.messageKey)).toContain("pages.merge.done");
  });

  it("a failed merge keeps A displayed and open", async () => {
    const a = (await openPath(A))!;
    const close = vi.spyOn(mock, "closeDocument");
    vi.spyOn(mock, "mergeDocuments").mockRejectedValueOnce(new Error("damaged input"));
    expect(await mergePaths(INPUTS)).toBeNull();
    expect(close).not.toHaveBeenCalled();
    expect(await loaded(a.docId)).toBe(true);
    const doc = useDocStore.getState();
    expect(doc.docId).toBe(a.docId);
    expect(doc.status).toBe("ready");
    expect(useToastStore.getState().toasts.map((t) => t.messageKey)).toContain("error.openFailed");
  });

  it("an edited A asks first; 취소 merges nothing and keeps A", async () => {
    const a = await openDirtyA();
    const merge = vi.spyOn(mock, "mergeDocuments");
    const close = vi.spyOn(mock, "closeDocument");
    const merging = mergePaths(INPUTS);
    await answerUnsaved("cancel");
    expect(await merging).toBeNull();
    expect(merge).not.toHaveBeenCalled();
    expect(close).not.toHaveBeenCalled();
    expect(await loaded(a.docId)).toBe(true);
    expect(useDocStore.getState().docId).toBe(a.docId);
  });

  it("저장 안 함 goes ahead: merged, and A is closed", async () => {
    const a = await openDirtyA();
    const merge = vi.spyOn(mock, "mergeDocuments");
    const close = vi.spyOn(mock, "closeDocument");
    const merging = mergePaths(INPUTS);
    await answerUnsaved("dontSave");
    const merged = (await merging)!;
    expect(merge).toHaveBeenCalledTimes(1);
    expect(close).toHaveBeenCalledWith({ docId: a.docId });
    expect(await loaded(a.docId)).toBe(false);
    expect(useDocStore.getState().docId).toBe(merged.docId);
  });

  it("the guard can be skipped by a caller that already ran it", async () => {
    const a = await openDirtyA();
    const merged = (await mergePaths(INPUTS, { guard: false }))!;
    expect(useDialogStore.getState().stack.some((d) => d.name === "unsaved")).toBe(false);
    expect(merged.docId).not.toBe(a.docId);
  });
});
