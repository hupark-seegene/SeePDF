/**
 * v0.3 pkg4 verification round 1: a gesture that makes several annotation edits is ONE undo
 * step — A3 (one partial-eraser scrub across several strokes) and A4 (one pen stroke whose
 * pressure varies, baked into several ink annotations). Both go through `annotation_batch`.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { waitFor } from "@testing-library/react";
import * as api from "../ipc/api";
import type { AnnotSpec } from "../ipc/types";
import { resetMock } from "../ipc/mock";
import { useAnnotStore } from "../store/annotStore";
import { useAppStore } from "../store/appStore";
import { useDocStore } from "../store/docStore";
import { createAnnotation, flushPatches, resetPatchQueue } from "./actions";
import { host } from "./AnnotationHost";
import { toolController } from "../tools/ToolController";
import { resetToolOptions, setToolOptions } from "../tools/toolOptions";
import { baseStyleFor } from "../store/toolStyles";

const SAMPLE = "/Users/veri/Documents/SeePDF-샘플.pdf";
let stop: (() => void) | null = null;

beforeEach(() => {
  resetMock();
  resetPatchQueue();
  resetToolOptions();
  useAnnotStore.getState().reset();
  useAnnotStore.setState({ toolDefaults: {}, styleTool: null, selected: [] });
  useAppStore.setState({ mode: "annotate", tool: "select" });
});
afterEach(() => {
  stop?.();
  stop = null;
  toolController.arm("select");
  vi.restoreAllMocks();
});

const ink = (y: number): AnnotSpec => ({ kind: "ink", paths: [[0, y, 100, y, 200, y]], color: [0, 0, 0], width: 2, opacity: 1 });
const modifiers = { shift: false, alt: false, meta: false, ctrl: false };

async function inkPathCounts(docId: string): Promise<number[]> {
  const list = await api.listAnnotations({ docId, page: 0 });
  return list.annots.filter((a) => a.kind === "ink").map((a) => a.inkPaths?.length ?? 0);
}

describe("one gesture = one undo step", () => {
  it("A3: one partial scrub across two ink strokes is undone in one step", async () => {
    const info = await useDocStore.getState().open(SAMPLE);
    stop = host.start();
    await createAnnotation(0, ink(100));
    await createAnnotation(0, ink(120));
    const batch = vi.spyOn(api, "annotationBatch");
    const update = vi.spyOn(api, "updateAnnotation");
    setToolOptions({ eraserMode: "partial" });
    useAppStore.setState({ tool: "eraser" });
    toolController.arm("eraser");
    const ctx = {
      docId: info!.docId, docGeneration: 1, style: { ...baseStyleFor("eraser"), eraserSize: 10 }, modifiers, scale: 1,
      annots: (p: number) => useAnnotStore.getState().byPage[p] ?? [],
    };
    toolController.pointerDown({ page: 0, pt: [100, 140] }, ctx);
    for (let y = 135; y >= 85; y -= 5) toolController.pointerMove({ page: 0, pt: [100, y] }, ctx);
    toolController.pointerUp({ page: 0, pt: [100, 85] }, ctx);
    await waitFor(() => expect(batch).toHaveBeenCalledTimes(1));
    await flushPatches();
    expect(update).not.toHaveBeenCalled();
    expect(await inkPathCounts(info!.docId)).toEqual([2, 2]);

    const undone = await api.undo({ docId: info!.docId });
    expect(undone.redoLabel).toBe("undo.annotEdit");
    expect(await inkPathCounts(info!.docId)).toEqual([1, 1]);
  });

  it("A3: a scrub that erases one stroke whole and splits another is one step too", async () => {
    const info = await useDocStore.getState().open(SAMPLE);
    stop = host.start();
    await createAnnotation(0, { kind: "ink", paths: [[98, 110, 102, 110]], color: [0, 0, 0], width: 2, opacity: 1 });
    await createAnnotation(0, ink(120));
    setToolOptions({ eraserMode: "partial" });
    useAppStore.setState({ tool: "eraser" });
    toolController.arm("eraser");
    const batch = vi.spyOn(api, "annotationBatch");
    const ctx = {
      docId: info!.docId, docGeneration: 1, style: { ...baseStyleFor("eraser"), eraserSize: 10 }, modifiers, scale: 1,
      annots: (p: number) => useAnnotStore.getState().byPage[p] ?? [],
    };
    toolController.pointerDown({ page: 0, pt: [100, 130] }, ctx);
    for (let y = 125; y >= 105; y -= 5) toolController.pointerMove({ page: 0, pt: [100, y] }, ctx);
    toolController.pointerUp({ page: 0, pt: [100, 105] }, ctx);
    await waitFor(() => expect(batch).toHaveBeenCalledTimes(1));
    await waitFor(async () => expect(await inkPathCounts(info!.docId)).toEqual([2]));
    await api.undo({ docId: info!.docId });
    expect(await inkPathCounts(info!.docId)).toEqual([1, 1]);
  });

  it("A4: one pen stroke of varying pressure is created and undone as one step", async () => {
    const info = await useDocStore.getState().open(SAMPLE);
    stop = host.start();
    const batch = vi.spyOn(api, "annotationBatch");
    const single = vi.spyOn(api, "createAnnotation");
    useAppStore.setState({ tool: "pen" });
    toolController.arm("pen");
    const ctx = { docId: info!.docId, docGeneration: 1, style: baseStyleFor("pen"), modifiers, scale: 1 };
    const pressures = [0.1, 0.1, 0.1, 0.1, 0.5, 0.5, 0.5, 0.5, 1, 1, 1, 1];
    toolController.pointerDown({ page: 0, pt: [10, 10], pointerType: "pen", pressure: pressures[0] }, ctx);
    pressures.slice(1).forEach((p, i) =>
      toolController.pointerMove({ page: 0, pt: [10 + (i + 1) * 10, 10 + (i % 2) * 3], pointerType: "pen", pressure: p }, ctx),
    );
    toolController.pointerUp({ page: 0, pt: [120, 10], pointerType: "pen", pressure: 1 }, ctx);
    await waitFor(() => expect(batch).toHaveBeenCalledTimes(1));
    expect(single).not.toHaveBeenCalled();
    const ops = batch.mock.calls[0][0].ops;
    expect(ops.length).toBeGreaterThan(1);
    expect(ops.every((o) => o.op === "create")).toBe(true);
    await waitFor(() => expect(useAnnotStore.getState().selected.length).toBe(ops.length));
    expect(await inkPathCounts(info!.docId)).toHaveLength(ops.length);

    const undone = await api.undo({ docId: info!.docId });
    expect(undone.redoLabel).toBe("undo.annotCreate");
    expect(await inkPathCounts(info!.docId)).toEqual([]);
  });

  it("annotation_batch in the mock is all or nothing", async () => {
    const info = await useDocStore.getState().open(SAMPLE);
    const made = await api.createAnnotation({ docId: info!.docId, page: 0, spec: ink(100) });
    const before = await api.listAnnotations({ docId: info!.docId, page: 0 });
    await expect(
      api.annotationBatch({
        docId: info!.docId, page: 0,
        ops: [{ op: "update", id: made.annot!.id, patch: { paths: [[0, 100, 10, 100]] } }, { op: "delete", ids: ["nope"] }],
      }),
    ).rejects.toMatchObject({ code: "notFound" });
    const after = await api.listAnnotations({ docId: info!.docId, page: 0 });
    expect(after).toEqual(before);
  });
});
