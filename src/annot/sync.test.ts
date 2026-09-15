/**
 * Undo / redo reconciliation (IPC_CONTRACT §7.8, §8).
 *
 * The engine never says *what* it undid — it pops a snapshot and broadcasts a new `docGeneration`
 * with a `changedPages` set. The store has to re-list those pages and, crucially, ignore a reply
 * that belongs to an older generation, or a slow `list_annotations` resurrects an annotation the
 * user has already undone.
 */
import { beforeEach, describe, expect, it, vi } from "vitest";
import * as api from "../ipc/api";
import type { AnnotSpec, DocChangedEvent } from "../ipc/types";
import { useAnnotStore } from "../store/annotStore";
import { useDocStore } from "../store/docStore";
import { createAnnotation, ghostFromSpec, resetPatchQueue } from "./actions";
import { applyDocChanged, bindDocument, pagesToReload, redoWithAnnots, resetAnnotSync, undoWithAnnots } from "./sync";

const SQUARE: AnnotSpec = { kind: "square", rect: { l: 10, b: 10, r: 60, t: 60 }, color: [245, 83, 61], fillColor: null, width: 2, opacity: 1 };

async function open() {
  const info = await useDocStore.getState().open("/tmp/sample.pdf");
  if (!info) throw new Error("mock open failed");
  bindDocument(info.docId);
  return info;
}

describe("annot.sync — which pages to re-list", () => {
  it("re-lists only the named pages, and every known page for `all`", () => {
    expect(pagesToReload([2, 3, 2], [0, 1])).toEqual([2, 3]);
    expect(pagesToReload("all", [0, 4, 9])).toEqual([0, 4, 9]);
    // a 500-page document must not fire 500 commands for one ⌘Z
    expect(pagesToReload("all", [])).toEqual([]);
  });
});

describe("annot.sync — undo / redo", () => {
  beforeEach(() => {
    useAnnotStore.getState().reset();
    resetAnnotSync();
    resetPatchQueue();
  });

  /** What `App.tsx` does with one `doc-changed`: the document store first, then (d)'s sync. */
  async function broadcast(e: DocChangedEvent): Promise<void> {
    useDocStore.getState().applyDocChanged(e);
    await applyDocChanged(e);
  }

  it("drops the annotation again after undo and brings it back on redo", async () => {
    const info = await open();
    const annot = await createAnnotation(0, SQUARE);
    if (!annot) throw new Error("create failed");
    expect(useAnnotStore.getState().byPage[0].some((a) => a.id === annot.id)).toBe(true);

    // The title bar's ⌘Z button reads `info.canUndo`, which now rides on the event itself
    // (Stage 2, IPC_CONTRACT §8): `App.tsx` feeds every `doc-changed` to *both*
    // `docStore.applyDocChanged` and the annotation sync, so the test does the same.
    await broadcast({
      docId: info.docId,
      docGeneration: useDocStore.getState().info?.docGeneration ?? 0,
      changedPages: [0],
      structure: false,
      dirty: true,
      reason: "edit", canUndo: true, canRedo: false,
    });
    await new Promise((r) => setTimeout(r, 30));
    expect(useDocStore.getState().info?.canUndo).toBe(true);

    await undoWithAnnots();
    const afterUndo = useDocStore.getState().info;
    expect(afterUndo?.docGeneration).toBeGreaterThan(info.docGeneration);
    await broadcast({
      docId: info.docId,
      docGeneration: afterUndo?.docGeneration ?? 0,
      changedPages: "all",
      structure: true,
      dirty: true,
      reason: "undo", canUndo: true, canRedo: false,
    });
    expect(useAnnotStore.getState().byPage[0].some((a) => a.id === annot.id)).toBe(false);
    // the selection follows: a selected annotation that no longer exists must not stay selected
    expect(useAnnotStore.getState().selected).not.toContain(annot.id);

    await redoWithAnnots();
    const afterRedo = useDocStore.getState().info;
    await broadcast({
      docId: info.docId,
      docGeneration: afterRedo?.docGeneration ?? 0,
      changedPages: "all",
      structure: true,
      dirty: true,
      reason: "redo", canUndo: true, canRedo: false,
    });
    expect(useAnnotStore.getState().byPage[0].some((a) => a.id === annot.id)).toBe(true);
    expect(useDocStore.getState().info?.canRedo).toBe(false);
  });

  it("ignores a `list_annotations` reply from an older generation", async () => {
    const info = await open();
    const ghost = ghostFromSpec(0, SQUARE, null);
    useAnnotStore.getState().setPage(0, [ghost], info.docGeneration + 5);
    // a slow reply for the generation before this one arrives late …
    useAnnotStore.getState().setPage(0, [], info.docGeneration + 1);
    // … and is dropped
    expect(useAnnotStore.getState().byPage[0]).toHaveLength(1);
    // the newest one wins
    useAnnotStore.getState().setPage(0, [], info.docGeneration + 6);
    expect(useAnnotStore.getState().byPage[0]).toHaveLength(0);
  });

  it("does nothing for a `doc-changed` of another document", async () => {
    const info = await open();
    const spy = vi.spyOn(api, "listAnnotations");
    const other: DocChangedEvent = {
      docId: "d999",
      docGeneration: info.docGeneration + 1,
      changedPages: [0],
      structure: false,
      dirty: true,
      reason: "edit", canUndo: true, canRedo: false,
    };
    await applyDocChanged(other);
    expect(spy).not.toHaveBeenCalled();
  });

  it("flushes a coalesced patch before popping the snapshot", async () => {
    const info = await open();
    const annot = await createAnnotation(0, SQUARE);
    if (!annot) throw new Error("create failed");
    const update = vi.spyOn(api, "updateAnnotation");
    const undo = vi.spyOn(api, "undo");
    const { patchAnnotation } = await import("./actions");
    patchAnnotation(0, annot.id, { opacity: 0.2 }, true); // still coalescing
    expect(update).not.toHaveBeenCalled();
    await undoWithAnnots();
    expect(update).toHaveBeenCalledTimes(1);
    expect(update.mock.invocationCallOrder[0]).toBeLessThan(undo.mock.invocationCallOrder[0]);
    expect(info.docId).toBeTruthy();
  });
});

describe("annot.sync — optimistic ghost lifetime", () => {
  beforeEach(() => {
    useAnnotStore.getState().reset();
    resetAnnotSync();
    resetPatchQueue();
  });

  it("keeps the ghost until the page bitmap of its generation has loaded", async () => {
    await open();
    const annot = await createAnnotation(0, SQUARE);
    if (!annot) throw new Error("create failed");
    const ghost = useAnnotStore.getState().ghosts[0];
    expect(ghost).toBeDefined();

    // the tile that is still in flight belongs to the previous generation
    useAnnotStore.getState().pageRendered(0, (ghost.generation as number) - 1);
    expect(useAnnotStore.getState().ghosts).toHaveLength(1);

    // this is the `onload` of the bitmap that contains it
    useAnnotStore.getState().pageRendered(0, ghost.generation as number);
    expect(useAnnotStore.getState().ghosts).toHaveLength(0);
  });

  it("does not drop a ghost of another page", async () => {
    await open();
    const annot = await createAnnotation(1, SQUARE);
    if (!annot) throw new Error("create failed");
    const ghost = useAnnotStore.getState().ghosts[0];
    useAnnotStore.getState().pageRendered(0, (ghost.generation as number) + 10);
    expect(useAnnotStore.getState().ghosts).toHaveLength(1);
  });
});
