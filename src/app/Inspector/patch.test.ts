/**
 * Inspector patch mapping (WORKPLAN (d) DoD): a control change → the `update_annotation` payload,
 * with the coalescing a dragged slider needs, and the "no selection edits the default" rule.
 */
import { beforeEach, describe, expect, it, vi } from "vitest";
import * as api from "../../ipc/api";
import type { AnnotSpec } from "../../ipc/types";
import { useAnnotStore } from "../../store/annotStore";
import { useDocStore } from "../../store/docStore";
import { createAnnotation, flushPatches, resetPatchQueue } from "../../annot/actions";
import { makeApply } from "./apply";
import { isLive, patchFor, styleFor } from "./patch";

const SQUARE: AnnotSpec = { kind: "square", rect: { l: 10, b: 10, r: 60, t: 60 }, color: [245, 83, 61], fillColor: null, width: 2, opacity: 1 };

describe("Inspector.patch — control → AnnotPatch", () => {
  it("maps every control to the field of the contract", () => {
    expect(patchFor("color", [77, 184, 255])).toEqual({ color: [77, 184, 255] });
    expect(patchFor("fillColor", null)).toEqual({ fillColor: null });
    expect(patchFor("opacity", 0.5)).toEqual({ opacity: 0.5 });
    // 굵기 is `borderWidth` on the wire, not `width` — the two names are a classic mix-up
    expect(patchFor("width", 8)).toEqual({ borderWidth: 8 });
    expect(patchFor("fontSize", 18)).toEqual({ fontSize: 18 });
    expect(patchFor("contents", "메모")).toEqual({ contents: "메모" });
    expect(patchFor("author", "박현우")).toEqual({ author: "박현우" });
    // a free-text annotation keeps its string in both places
    expect(patchFor("text", "한글 테스트")).toEqual({ text: "한글 테스트", contents: "한글 테스트" });
  });

  it("has no wire field for the controls that only exist in the tool defaults", () => {
    expect(patchFor("align", "center")).toBeNull();
    expect(patchFor("heads", [true, false])).toBeNull();
    expect(patchFor("eraserSize", 20)).toBeNull();
    expect(styleFor("align", "center")).toEqual({ align: "center" });
    expect(styleFor("heads", [true, false])).toEqual({ heads: [true, false] });
    expect(styleFor("eraserSize", 20)).toEqual({ eraserSize: 20 });
  });

  it("coalesces exactly the controls that are dragged", () => {
    expect(isLive("opacity")).toBe(true);
    expect(isLive("width")).toBe(true);
    expect(isLive("fontSize")).toBe(true);
    expect(isLive("color")).toBe(false);
    expect(isLive("contents")).toBe(false);
  });
});

describe("Inspector.patch — apply", () => {
  beforeEach(() => {
    useAnnotStore.getState().reset();
    resetPatchQueue();
  });

  it("edits the tool default when nothing is selected", () => {
    const setStyle = vi.fn();
    const apply = makeApply([], setStyle);
    apply("color", [123, 214, 74]);
    expect(setStyle).toHaveBeenCalledWith({ color: [123, 214, 74] });
  });

  it("edits the selection — and only the selection — when there is one", async () => {
    const info = await useDocStore.getState().open("/tmp/sample.pdf");
    if (!info) throw new Error("open failed");
    const annot = await createAnnotation(0, SQUARE);
    if (!annot) throw new Error("create failed");
    const setStyle = vi.fn();
    const spy = vi.spyOn(api, "updateAnnotation");
    const apply = makeApply([annot], setStyle);

    apply("color", [123, 214, 74]);
    await flushPatches();
    expect(setStyle).not.toHaveBeenCalled();
    expect(spy).toHaveBeenCalledTimes(1);
    expect(spy.mock.calls[0][0]).toMatchObject({ docId: info.docId, page: 0, id: annot.id, patch: { color: [123, 214, 74] } });
  });

  it("a slider drag over a selection produces one command with the last value", async () => {
    const info = await useDocStore.getState().open("/tmp/sample.pdf");
    if (!info) throw new Error("open failed");
    const annot = await createAnnotation(0, SQUARE);
    if (!annot) throw new Error("create failed");
    const spy = vi.spyOn(api, "updateAnnotation");
    const apply = makeApply([annot], vi.fn());

    for (const v of [90, 80, 70, 60, 55]) apply("opacity", v / 100);
    expect(spy).not.toHaveBeenCalled();
    await flushPatches();
    expect(spy).toHaveBeenCalledTimes(1);
    expect(spy.mock.calls[0][0].patch).toEqual({ opacity: 0.55 });
  });

  it("applies one patch per annotation of a multi-selection", async () => {
    const info = await useDocStore.getState().open("/tmp/sample.pdf");
    if (!info) throw new Error("open failed");
    const a = await createAnnotation(0, SQUARE);
    const b = await createAnnotation(0, { ...SQUARE, rect: { l: 100, b: 100, r: 140, t: 140 } });
    if (!a || !b) throw new Error("create failed");
    const spy = vi.spyOn(api, "updateAnnotation");
    makeApply([a, b], vi.fn())("width", 12);
    await flushPatches();
    expect(spy).toHaveBeenCalledTimes(2);
    expect(spy.mock.calls.map((c) => c[0].id).sort()).toEqual([a.id, b.id].sort());
    expect(spy.mock.calls[0][0].patch).toEqual({ borderWidth: 12 });
  });
});
