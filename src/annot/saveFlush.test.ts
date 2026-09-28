/**
 * ⌘S right after a nudge or an inspector slider (bug hunt 2026-09): the coalesced patch must reach
 * the engine before the file is written, or the saved file lacks the edit and the document goes
 * dirty again the moment the late `update_annotation` lands.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import * as api from "../ipc/api";
import type { AnnotSpec } from "../ipc/types";
import { useAnnotStore } from "../store/annotStore";
import { useDocStore } from "../store/docStore";
import { movePatch } from "../tools/hit";
import { createAnnotation, hasPendingPatches, patchAnnotation, resetPatchQueue } from "./actions";
import { resetDragGate } from "./dragGate";
import { confirmUnsaved, saveAsFlow, saveFlow } from "../dialogs/flows";

const SQUARE: AnnotSpec = { kind: "square", rect: { l: 10, b: 10, r: 60, t: 60 }, color: [245, 83, 61], fillColor: null, width: 2, opacity: 1 };

let order: string[] = [];

async function setup() {
  await useDocStore.getState().open("/tmp/sample.pdf");
  const annot = await createAnnotation(0, SQUARE);
  if (!annot) throw new Error("create failed");
  order = [];
  const update = api.updateAnnotation;
  vi.spyOn(api, "updateAnnotation").mockImplementation(async (a) => {
    order.push("update_annotation");
    return update(a);
  });
  const saved = (a: { docId: string; path?: string }) => ({
    docId: a.docId, path: a.path ?? "/tmp/sample.pdf", bytes: 1, docGeneration: 1, elapsedMs: 1,
  });
  vi.spyOn(api, "saveDocument").mockImplementation(async (a) => {
    order.push(`save_document(pending=${hasPendingPatches()})`);
    return saved(a);
  });
  vi.spyOn(api, "saveDocumentAs").mockImplementation(async (a) => {
    order.push(`save_document_as(pending=${hasPendingPatches()})`);
    return saved(a);
  });
  return annot;
}

describe("save drains the coalesced annotation patches first", () => {
  beforeEach(() => {
    useAnnotStore.getState().reset();
    resetPatchQueue();
    resetDragGate();
  });
  afterEach(() => {
    resetPatchQueue();
    vi.restoreAllMocks();
  });

  it("⌘S after an arrow-key nudge saves the nudge", async () => {
    const annot = await setup();
    patchAnnotation(0, annot.id, movePatch(annot, 1, 0), true);
    expect(hasPendingPatches()).toBe(true);
    await saveFlow();
    expect(order).toEqual(["update_annotation", "save_document(pending=false)"]);
  });

  it("다른 이름으로 저장 after a slider edit saves the edit", async () => {
    const annot = await setup();
    vi.spyOn(api, "saveFileDialog").mockResolvedValue("/tmp/copy.pdf");
    patchAnnotation(0, annot.id, { opacity: 0.5 }, true);
    await saveAsFlow();
    expect(order).toEqual(["update_annotation", "save_document_as(pending=false)"]);
  });

  it("closing right after a nudge sees the document as changed", async () => {
    const annot = await setup();
    useDocStore.setState({ info: { ...useDocStore.getState().info!, dirty: false } });
    patchAnnotation(0, annot.id, movePatch(annot, 0, 1), true);
    // the prompt is answered 취소 by replacing it (no dialog host here): what matters is that it asks
    const answer = confirmUnsaved();
    await vi.waitFor(() => expect(order).toContain("update_annotation"));
    expect(hasPendingPatches()).toBe(false);
    const { useDialogStore } = await import("../dialogs/dialogState");
    await vi.waitFor(() => expect(useDialogStore.getState().stack.map((e) => e.name)).toEqual(["unsaved"]));
    useDialogStore.getState().closeAll();
    expect(await answer).toBe(false);
  });
});
