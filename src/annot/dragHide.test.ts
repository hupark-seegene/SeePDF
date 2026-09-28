/**
 * Hide-while-dragging (P1-12): the order of the IPC calls is the whole feature.
 *
 *   drag starts  → set_annotations_hidden(true)            (bitmap copy goes, overlay ghost stays)
 *   drag moves   → nothing sent (patches held)             (no undo snapshot with /F HIDDEN)
 *   drop/cancel  → set_annotations_hidden(false) → update_annotation
 *
 * and every failure on the way still ends with the annotation shown and the queue released.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import * as api from "../ipc/api";
import type { Annot, AnnotSpec } from "../ipc/types";
import { useAnnotStore } from "../store/annotStore";
import { useDocStore } from "../store/docStore";
import { toolController } from "../tools/ToolController";
import { movePatch } from "../tools/hit";
import { PATCH_COALESCE_MS, createAnnotation, hasPendingPatches, resetPatchQueue } from "./actions";
import { canRepaint, dragHidePage, dragPatch, endDrag, startDragHide } from "./dragHide";

const SQUARE: AnnotSpec = { kind: "square", rect: { l: 10, b: 10, r: 60, t: 60 }, color: [245, 83, 61], fillColor: null, width: 2, opacity: 1 };

let calls: string[] = [];
let nonce = 100;

async function setup(spec: AnnotSpec = SQUARE): Promise<Annot> {
  const info = await useDocStore.getState().open("/tmp/sample.pdf");
  if (!info) throw new Error("mock open failed");
  const annot = await createAnnotation(0, spec);
  if (!annot) throw new Error("create failed");
  useAnnotStore.getState().clearGhosts();
  calls = [];
  return annot;
}

function spyHidden(impl?: (a: { hidden: boolean }) => Promise<{ viewNonce: number }>) {
  return vi.spyOn(api, "setAnnotationsHidden").mockImplementation(async (a) => {
    calls.push(`hidden:${a.hidden}:${a.ids.join(",")}`);
    if (impl) return impl(a);
    return { viewNonce: ++nonce };
  });
}

function spyUpdate(fail = false) {
  const real = api.updateAnnotation;
  return vi.spyOn(api, "updateAnnotation").mockImplementation(async (a) => {
    calls.push(`update:${a.id}`);
    if (fail) throw { code: "stale", message: "nope" };
    return real(a);
  });
}

const tick = () => new Promise((r) => setTimeout(r, 0));

describe("dragHide — hide the bitmap copy while an annotation is dragged", () => {
  beforeEach(() => {
    useAnnotStore.getState().reset();
    resetPatchQueue();
  });
  afterEach(async () => {
    await endDrag();
    vi.restoreAllMocks();
  });

  it("hides on the first live patch, holds the patches, and unhides before the one update on drop", async () => {
    const annot = await setup();
    spyHidden();
    spyUpdate();
    const generation = useAnnotStore.getState().pageGeneration[0];

    dragPatch(0, [{ id: annot.id, patch: movePatch(annot, 5, 0) }], true);
    dragPatch(0, [{ id: annot.id, patch: movePatch(annot, 10, 0) }], true);
    await tick();
    expect(calls).toEqual([`hidden:true:${annot.id}`]);
    expect(dragHidePage()).toBe(0);
    // the page URLs switched to the hidden render, and the overlay paints the annotation in full
    expect(useAnnotStore.getState().viewNonce[0]).toBe(nonce);
    expect(useAnnotStore.getState().ghosts.map((g) => g.annot.id)).toEqual([annot.id]);
    expect(useAnnotStore.getState().ghosts[0].annot.rect.l).toBe(20);

    // an idle drag does not flush while hidden
    await new Promise((r) => setTimeout(r, PATCH_COALESCE_MS + 40));
    expect(calls).toEqual([`hidden:true:${annot.id}`]);
    expect(hasPendingPatches()).toBe(true);

    dragPatch(0, [{ id: annot.id, patch: movePatch(annot, 15, 0) }], false);
    await endDrag();
    expect(calls).toEqual([`hidden:true:${annot.id}`, `hidden:false:${annot.id}`, `update:${annot.id}`]);
    expect(dragHidePage()).toBeNull();
    expect(hasPendingPatches()).toBe(false);
    // the ghost stays until the bitmap of the new generation paints, then goes
    const ghost = useAnnotStore.getState().ghosts[0];
    expect(ghost.generation).toBeGreaterThan(generation);
    useAnnotStore.getState().pageRendered(0, ghost.generation as number);
    expect(useAnnotStore.getState().ghosts).toHaveLength(0);
    expect(useAnnotStore.getState().byPage[0].find((a) => a.id === annot.id)?.rect.l).toBe(25);
  });

  it("a drop that beats the hide's answer still unhides after it, and never shows the hidden render", async () => {
    const annot = await setup();
    let answer: (v: { viewNonce: number }) => void = () => undefined;
    spyHidden((a) => (a.hidden ? new Promise((r) => (answer = r)) : Promise.resolve({ viewNonce: ++nonce })));
    spyUpdate();

    dragPatch(0, [{ id: annot.id, patch: movePatch(annot, 5, 5) }], true);
    dragPatch(0, [{ id: annot.id, patch: movePatch(annot, 6, 6) }], false);
    await tick();
    expect(calls).toEqual([`hidden:true:${annot.id}`]); // waiting for the hide
    answer({ viewNonce: 1 });
    await endDrag();
    expect(calls).toEqual([`hidden:true:${annot.id}`, `hidden:false:${annot.id}`, `update:${annot.id}`]);
    expect(useAnnotStore.getState().viewNonce[0]).toBeUndefined();
  });

  it("pointercancel / a tool switch mid-drag restores and commits where it got to", async () => {
    const annot = await setup();
    spyHidden();
    spyUpdate();
    const stop = startDragHide();

    dragPatch(0, [{ id: annot.id, patch: movePatch(annot, 3, 0) }], true);
    await tick();
    toolController.arm("pen"); // emits; no gesture is active → the session ends
    await endDrag();
    expect(calls).toEqual([`hidden:true:${annot.id}`, `hidden:false:${annot.id}`, `update:${annot.id}`]);
    stop();
    toolController.arm("select");
  });

  it("unmounting the host mid-drag restores the annotation", async () => {
    const annot = await setup();
    spyHidden();
    spyUpdate();
    const stop = startDragHide();
    dragPatch(0, [{ id: annot.id, patch: movePatch(annot, 3, 0) }], true);
    await tick();
    stop();
    await endDrag();
    expect(calls.slice(0, 2)).toEqual([`hidden:true:${annot.id}`, `hidden:false:${annot.id}`]);
  });

  it("a failed update still leaves the annotation shown, re-rendered, and the queue released", async () => {
    const annot = await setup();
    spyHidden();
    spyUpdate(true);
    dragPatch(0, [{ id: annot.id, patch: movePatch(annot, 3, 0) }], true);
    await tick();
    const hiddenNonce = useAnnotStore.getState().viewNonce[0];
    dragPatch(0, [{ id: annot.id, patch: movePatch(annot, 4, 0) }], false);
    await endDrag();
    expect(calls).toEqual([`hidden:true:${annot.id}`, `hidden:false:${annot.id}`, `update:${annot.id}`]);
    // same generation: the page asks for a fresh render with the annotation back
    expect(useAnnotStore.getState().viewNonce[0]).toBe(nonce);
    expect(useAnnotStore.getState().viewNonce[0]).not.toBe(hiddenNonce);
    expect(hasPendingPatches()).toBe(false);
  });

  it("a failed hide still sends the unhide (the engine may have applied it) and commits", async () => {
    const annot = await setup();
    spyHidden((a) => (a.hidden ? Promise.reject(new Error("lost")) : Promise.resolve({ viewNonce: ++nonce })));
    spyUpdate();
    dragPatch(0, [{ id: annot.id, patch: movePatch(annot, 3, 0) }], true);
    dragPatch(0, [{ id: annot.id, patch: movePatch(annot, 3, 0) }], false);
    await endDrag();
    expect(calls).toEqual([`hidden:true:${annot.id}`, `hidden:false:${annot.id}`, `update:${annot.id}`]);
  });

  it("does not hide what the overlay cannot repaint (an image stamp): the old path is untouched", async () => {
    const annot = await setup({ kind: "stamp", rect: { l: 10, b: 10, r: 60, t: 60 }, image: { path: "/tmp/seal.png" } });
    expect(canRepaint(annot)).toBe(false);
    spyHidden();
    spyUpdate();
    dragPatch(0, [{ id: annot.id, patch: movePatch(annot, 3, 0) }], true);
    dragPatch(0, [{ id: annot.id, patch: movePatch(annot, 4, 0) }], false);
    await endDrag();
    expect(calls).toEqual([`update:${annot.id}`]);
    expect(useAnnotStore.getState().ghosts).toHaveLength(0);
  });

  it("repaints built-in stamps (결재) and drawn ink, not optimistic ghosts", () => {
    const base = { id: "a", page: 0, subtype: "Stamp", rect: { l: 0, b: 0, r: 1, t: 1 }, color: [0, 0, 0], fillColor: null, opacity: 1, borderWidth: 1, contents: "", author: null, created: null, modified: null, hidden: false, printed: true, locked: false, editable: "full" } as unknown as Annot;
    expect(canRepaint({ ...base, kind: "stamp", stampKind: "결재" })).toBe(true);
    expect(canRepaint({ ...base, kind: "stamp", stampKind: "SeePDF:Signature" })).toBe(false);
    expect(canRepaint({ ...base, kind: "ink", inkPaths: [[0, 0, 1, 1]] })).toBe(true);
    expect(canRepaint({ ...base, kind: "square", id: "ghost-3" })).toBe(false);
  });
});
