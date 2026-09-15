/**
 * The optimism and the coalescing of `src/annot/actions.ts` (ARCHITECTURE §10, IPC_CONTRACT §7.1):
 * a created annotation is on screen before the command resolves, its ghost survives until the page
 * has been re-rendered, and a dragged slider produces **one** `update_annotation`, not thirty.
 */
import { beforeEach, describe, expect, it, vi } from "vitest";
import * as api from "../ipc/api";
import type { AnnotSpec } from "../ipc/types";
import { useAnnotStore } from "../store/annotStore";
import { useDocStore } from "../store/docStore";
import {
  PATCH_COALESCE_MS,
  annotsOnPage,
  createAnnotation,
  deleteAnnotations,
  duplicateAnnotations,
  flushPatches,
  ghostFromSpec,
  patchAnnotation,
  resetPatchQueue,
  specFromAnnot,
} from "./actions";

const SQUARE: AnnotSpec = { kind: "square", rect: { l: 10, b: 10, r: 60, t: 60 }, color: [245, 83, 61], fillColor: null, width: 2, opacity: 1 };

async function open() {
  const info = await useDocStore.getState().open("/tmp/sample.pdf");
  if (!info) throw new Error("mock open failed");
  return info;
}

describe("annot.actions — optimistic create", () => {
  beforeEach(() => {
    useAnnotStore.getState().reset();
    resetPatchQueue();
  });

  it("draws a ghost before the command resolves and keeps it until the page is re-rendered", async () => {
    const info = await open();
    const generationBefore = info.docGeneration;

    const pending = createAnnotation(0, SQUARE);
    // within the same tick the shape is already in the store
    expect(useAnnotStore.getState().ghosts).toHaveLength(1);
    expect(useAnnotStore.getState().ghosts[0].generation).toBeNull();

    const annot = await pending;
    expect(annot).not.toBeNull();
    const ghost = useAnnotStore.getState().ghosts[0];
    expect(ghost.annot.id).toBe(annot?.id);
    expect(ghost.generation).toBeGreaterThan(generationBefore);
    // the engine's list landed too
    expect(useAnnotStore.getState().byPage[0].some((a) => a.id === annot?.id)).toBe(true);

    // an older generation must not drop it …
    useAnnotStore.getState().pageRendered(0, generationBefore);
    expect(useAnnotStore.getState().ghosts).toHaveLength(1);
    // … the one that contains it does (this is the tile `onload`)
    useAnnotStore.getState().pageRendered(0, ghost.generation as number);
    expect(useAnnotStore.getState().ghosts).toHaveLength(0);
  });

  it("removes the ghost again when the command fails", async () => {
    await open();
    vi.spyOn(api, "createAnnotation").mockRejectedValueOnce(new Error("unsupported"));
    const annot = await createAnnotation(0, SQUARE);
    expect(annot).toBeNull();
    expect(useAnnotStore.getState().ghosts).toHaveLength(0);
  });

  it("exposes ghosts to hit-testing before the engine confirms them", async () => {
    await open();
    const ghost = ghostFromSpec(0, SQUARE, "박현우");
    useAnnotStore.getState().addGhost(ghost);
    expect(annotsOnPage(0).some((a) => a.id === ghost.id)).toBe(true);
  });
});

describe("annot.actions — coalescing", () => {
  beforeEach(() => {
    useAnnotStore.getState().reset();
    resetPatchQueue();
  });

  it("merges a slider drag into one update_annotation", async () => {
    await open();
    const annot = await createAnnotation(0, SQUARE);
    if (!annot) throw new Error("create failed");
    const spy = vi.spyOn(api, "updateAnnotation");

    for (let i = 1; i <= 20; i++) patchAnnotation(0, annot.id, { opacity: i / 20 }, true);
    expect(spy).not.toHaveBeenCalled();
    // the overlay already shows the last value
    expect(useAnnotStore.getState().byPage[0].find((a) => a.id === annot.id)?.opacity).toBe(1);

    await flushPatches();
    expect(spy).toHaveBeenCalledTimes(1);
    expect(spy.mock.calls[0][0].patch).toEqual({ opacity: 1 });
  });

  it("merges different properties of the same gesture into one payload", async () => {
    await open();
    const annot = await createAnnotation(0, SQUARE);
    if (!annot) throw new Error("create failed");
    const spy = vi.spyOn(api, "updateAnnotation");
    patchAnnotation(0, annot.id, { color: [77, 184, 255] }, true);
    patchAnnotation(0, annot.id, { opacity: 0.5 }, true);
    patchAnnotation(0, annot.id, { borderWidth: 8 }, false); // the end of the gesture flushes
    await flushPatches();
    expect(spy).toHaveBeenCalledTimes(1);
    expect(spy.mock.calls[0][0].patch).toEqual({ color: [77, 184, 255], opacity: 0.5, borderWidth: 8 });
  });

  it("flushes on its own after the coalescing window", async () => {
    // The document is opened with real timers: the mock adapter answers through `setTimeout`.
    const info = await open();
    useAnnotStore.getState().setPage(0, [{ ...ghostFromSpec(0, SQUARE, null), id: "x1" }], info.docGeneration);
    const spy = vi.spyOn(api, "updateAnnotation").mockResolvedValue({
      list: { docId: info.docId, page: 0, docGeneration: info.docGeneration + 1, annots: [] },
      annot: null,
      previous: null,
    });
    patchAnnotation(0, "x1", { opacity: 0.25 }, true);
    expect(spy).not.toHaveBeenCalled();
    await new Promise((r) => setTimeout(r, PATCH_COALESCE_MS + 30));
    expect(spy).toHaveBeenCalledTimes(1);
  });
});

describe("annot.actions — delete and duplicate", () => {
  beforeEach(() => {
    useAnnotStore.getState().reset();
    resetPatchQueue();
  });

  it("removes the shape immediately and then from the file", async () => {
    await open();
    const annot = await createAnnotation(0, SQUARE);
    if (!annot) throw new Error("create failed");
    const pending = deleteAnnotations(0, [annot.id]);
    expect(useAnnotStore.getState().byPage[0].some((a) => a.id === annot.id)).toBe(false);
    await pending;
    expect(useAnnotStore.getState().byPage[0].some((a) => a.id === annot.id)).toBe(false);
  });

  it("duplicates a selection offset from the original", async () => {
    await open();
    const annot = await createAnnotation(0, SQUARE);
    if (!annot) throw new Error("create failed");
    const before = useAnnotStore.getState().byPage[0].length;
    await duplicateAnnotations(0, [annot.id], 12);
    const list = useAnnotStore.getState().byPage[0];
    expect(list).toHaveLength(before + 1);
    const copy = list[list.length - 1];
    // right and down by 12 pt, so the copy is visibly a copy (y-up: down is -12)
    expect(copy.rect).toEqual({ l: 22, b: -2, r: 72, t: 48 });
    expect(useAnnotStore.getState().selected).toEqual([copy.id]);
  });

  it("round-trips every P0 kind through specFromAnnot", async () => {
    await open();
    const kinds: AnnotSpec[] = [
      SQUARE,
      { kind: "highlight", rects: [{ l: 1, b: 2, r: 3, t: 4 }], color: [1, 2, 3], opacity: 0.4 },
      { kind: "note", at: [10, 20], color: [1, 2, 3], contents: "메모" },
      { kind: "ink", paths: [[0, 0, 1, 1]], color: [1, 2, 3], width: 2, opacity: 1 },
      { kind: "arrow", p1: [0, 0], p2: [10, 10], color: [1, 2, 3], width: 2, opacity: 1 },
      { kind: "textbox", rect: { l: 0, b: 0, r: 40, t: 20 }, text: "한글", fontSize: 12, color: [0, 0, 0], align: "left", fillColor: null },
    ];
    for (const spec of kinds) {
      const ghost = ghostFromSpec(0, spec, null);
      expect(specFromAnnot(ghost)?.kind).toBe(spec.kind === "signature" ? "ink" : spec.kind);
    }
  });
});
