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
import { isDragHiding, resetDragGate, trackWrite, whenDragIdle } from "./dragGate";
import { getSnapshot } from "./snapshots";
import { undoWithAnnots } from "./sync";
import { AutosaveController } from "../app/autosave";

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
    resetDragGate();
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

  it("an image stamp is hidden too: its pixels are snapshotted first and the ghost paints them (Stage 8)", async () => {
    const annot = await setup({ kind: "stamp", rect: { l: 10, b: 10, r: 60, t: 60 }, image: { path: "/tmp/seal.png" } });
    expect(canRepaint(annot)).toBe(true);
    const real = api.renderPageRaw;
    const raw = vi.spyOn(api, "renderPageRaw").mockImplementation(async (a) => {
      calls.push(`snapshot:${a.page}`);
      return real(a);
    });
    spyHidden();
    spyUpdate();
    dragPatch(0, [{ id: annot.id, patch: movePatch(annot, 3, 0) }], true);
    await vi.waitFor(() => expect(calls).toContain(`hidden:true:${annot.id}`));
    // the snapshot of the stamp's own rect comes before the hide, so it still shows the stamp
    expect(calls.slice(0, 2)).toEqual(["snapshot:0", `hidden:true:${annot.id}`]);
    expect(raw.mock.calls[0][0].rect).toEqual(annot.rect);
    expect(getSnapshot(annot.id)?.url).toMatch(/^(data:image\/png|blob:)/);
    expect(useAnnotStore.getState().ghosts.map((g) => g.annot.id)).toEqual([annot.id]);

    dragPatch(0, [{ id: annot.id, patch: movePatch(annot, 4, 0) }], false);
    await endDrag();
    expect(calls.slice(1)).toEqual([`hidden:true:${annot.id}`, `hidden:false:${annot.id}`, `update:${annot.id}`]);
  });

  it("a failed snapshot still hides; the ghost falls back to an outline", async () => {
    const annot = await setup({ kind: "stamp", rect: { l: 10, b: 10, r: 60, t: 60 }, image: { path: "/tmp/seal.png" } });
    vi.spyOn(api, "renderPageRaw").mockRejectedValue(new Error("render failed"));
    spyHidden();
    spyUpdate();
    dragPatch(0, [{ id: annot.id, patch: movePatch(annot, 3, 0) }], true);
    await vi.waitFor(() => expect(calls).toContain(`hidden:true:${annot.id}`));
    expect(getSnapshot(annot.id)).toBeUndefined();
    dragPatch(0, [{ id: annot.id, patch: movePatch(annot, 4, 0) }], false);
    await endDrag();
    expect(calls).toEqual([`hidden:true:${annot.id}`, `hidden:false:${annot.id}`, `update:${annot.id}`]);
  });

  it("back-to-back drags: drag 1's finish sends nothing while drag 2 hides, and leaves drag 2's hold on", async () => {
    const annot = await setup();
    // the engine's HIDDEN bit, as the calls reach it (the answer to drag 1's unhide is slow)
    let hidden = false;
    let unhides = 0;
    let answerUnhide: () => void = () => undefined;
    spyHidden(async (a) => {
      hidden = a.hidden;
      if (!a.hidden && ++unhides === 1) await new Promise<void>((r) => (answerUnhide = r));
      return { viewNonce: ++nonce };
    });
    const real = api.updateAnnotation;
    const sentWhileHidden: boolean[] = [];
    vi.spyOn(api, "updateAnnotation").mockImplementation(async (a) => {
      sentWhileHidden.push(hidden);
      return real(a);
    });

    // drag 1, dropped; its unhide has not answered yet
    dragPatch(0, [{ id: annot.id, patch: movePatch(annot, 5, 0) }], true);
    await tick();
    dragPatch(0, [{ id: annot.id, patch: movePatch(annot, 10, 0) }], false);
    await tick();
    // the user grabs it again at once
    dragPatch(0, [{ id: annot.id, patch: movePatch(annot, 20, 0) }], true);
    await tick();
    answerUnhide();
    // drag 2 sits still past the coalescing delay: nothing may be sent while it is hidden
    await new Promise((r) => setTimeout(r, PATCH_COALESCE_MS + 40));
    expect(sentWhileHidden.every((h) => !h)).toBe(true);
    expect(hasPendingPatches()).toBe(true);
    expect(dragHidePage()).toBe(0);

    dragPatch(0, [{ id: annot.id, patch: movePatch(annot, 30, 0) }], false);
    await endDrag();
    // one update per drag, none of them with the HIDDEN bit set
    expect(sentWhileHidden).toEqual([false, false]);
    expect(hasPendingPatches()).toBe(false);
    expect(hidden).toBe(false);
    expect(useAnnotStore.getState().byPage[0].find((a) => a.id === annot.id)?.rect.l).toBe(40);
  });

  it("repaints built-in, image and foreign stamps, drawn ink and image signatures — not optimistic ghosts", () => {
    const base = { id: "a", page: 0, subtype: "Stamp", rect: { l: 0, b: 0, r: 1, t: 1 }, color: [0, 0, 0], fillColor: null, opacity: 1, borderWidth: 1, contents: "", author: null, created: null, modified: null, hidden: false, printed: true, locked: false, editable: "full" } as unknown as Annot;
    expect(canRepaint({ ...base, kind: "stamp", stampKind: "결재" })).toBe(true);
    expect(canRepaint({ ...base, kind: "stamp", stampKind: "image" })).toBe(true);
    expect(canRepaint({ ...base, kind: "stamp", stampKind: "Approved" })).toBe(true);
    expect(canRepaint({ ...base, kind: "signature", stampKind: "image" })).toBe(true);
    expect(canRepaint({ ...base, kind: "ink", inkPaths: [[0, 0, 1, 1]] })).toBe(true);
    expect(canRepaint({ ...base, kind: "ink" })).toBe(false);
    expect(canRepaint({ ...base, kind: "link" })).toBe(false);
    expect(canRepaint({ ...base, kind: "square", id: "ghost-3" })).toBe(false);
  });
});

describe("dragGate — nothing snapshots the document while an annotation is hidden (Stage 8)", () => {
  beforeEach(() => {
    useAnnotStore.getState().reset();
    resetPatchQueue();
    resetDragGate();
  });
  afterEach(async () => {
    await endDrag();
    vi.restoreAllMocks();
  });

  it("⌘Z mid-drag is refused; the gate opens only once the drop has committed", async () => {
    const annot = await setup();
    spyHidden();
    spyUpdate();
    const undo = vi.spyOn(useDocStore.getState(), "undo");
    dragPatch(0, [{ id: annot.id, patch: movePatch(annot, 5, 0) }], true);
    expect(isDragHiding()).toBe(true);
    await undoWithAnnots();
    expect(undo).not.toHaveBeenCalled();

    let idle = false;
    void whenDragIdle().then(() => (idle = true));
    dragPatch(0, [{ id: annot.id, patch: movePatch(annot, 6, 0) }], false);
    await tick();
    await endDrag();
    expect(idle).toBe(true);
    expect(isDragHiding()).toBe(false);
    // the update landed before the gate opened
    expect(calls.at(-1)).toBe(`update:${annot.id}`);
    await undoWithAnnots();
    expect(undo).toHaveBeenCalledTimes(1);
  });

  it("⌘S mid-drag is queued until the drop: the save sees the annotation shown again", async () => {
    const annot = await setup();
    spyHidden();
    spyUpdate();
    const save = vi.spyOn(api, "saveDocument").mockImplementation(async (a) => {
      calls.push("save");
      return { docId: a.docId, path: "/tmp/sample.pdf", bytes: 1, docGeneration: 9, elapsedMs: 1 };
    });
    dragPatch(0, [{ id: annot.id, patch: movePatch(annot, 5, 0) }], true);
    const { saveFlow } = await import("../dialogs/flows");
    const saving = saveFlow();
    await tick();
    expect(save).not.toHaveBeenCalled();
    dragPatch(0, [{ id: annot.id, patch: movePatch(annot, 6, 0) }], false);
    await saving;
    expect(calls).toEqual([`hidden:true:${annot.id}`, `hidden:false:${annot.id}`, `update:${annot.id}`, "save"]);
  });

  it("an autosave beat mid-drag waits for the drop; a write in flight delays the hide", async () => {
    const annot = await setup();
    spyHidden();
    spyUpdate();
    // a write already in flight when the drag starts: the hide goes only after it lands
    let land: () => void = () => undefined;
    void trackWrite(new Promise<void>((r) => (land = () => { calls.push("write:landed"); r(); })));
    dragPatch(0, [{ id: annot.id, patch: movePatch(annot, 5, 0) }], true);
    await tick();
    expect(calls).toEqual([]);
    land();
    await vi.waitFor(() => expect(calls).toEqual(["write:landed", `hidden:true:${annot.id}`]));

    // a beat now waits until the drop has settled
    const info = { ...useDocStore.getState().info!, dirty: true, docGeneration: 99 };
    const ctl = new AutosaveController({
      current: () => info,
      write: async () => void calls.push("write:copy"),
      clear: async () => undefined,
      onFail: () => undefined,
      idle: whenDragIdle,
    });
    const beat = ctl.tick();
    await tick();
    expect(calls).not.toContain("write:copy");
    dragPatch(0, [{ id: annot.id, patch: movePatch(annot, 6, 0) }], false);
    await beat;
    expect(calls.slice(-2)).toEqual([`update:${annot.id}`, "write:copy"]);
  });
});
