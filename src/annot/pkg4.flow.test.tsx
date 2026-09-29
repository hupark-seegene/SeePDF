/**
 * v0.3 pkg4-annotations-stamps-objects — the annotation-mode flows against the mock:
 * A9 (author), A1 (정렬 / 화살표 on a selection), A10 (인쇄), A5 (quick popover), T1 (a picked
 * image keeps its aspect), T2 (내 도장 persists and comes back), A2 (a finished polygon is created).
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import * as api from "../ipc/api";
import type { AnnotSpec, CustomStamp } from "../ipc/types";
import { resetMock, setMockImageSize } from "../ipc/mock";
import { useAnnotStore } from "../store/annotStore";
import { useAppStore } from "../store/appStore";
import { useDocStore } from "../store/docStore";
import { createAnnotation, flushPatches, resetPatchQueue } from "./actions";
import InspectorBody from "../app/Inspector/InspectorBody";
import { host } from "./AnnotationHost";
import { makePageLayerContext, type PageLayerContext } from "../viewer";
import { toolController } from "../tools/ToolController";
import { resetStampImages, stampImage, stampAspect, placementSize } from "../tools/stamp";
import { closeDialog, useDialogStore } from "../dialogs/dialogState";
import StampPickerDialog from "../dialogs/StampPickerDialog";
import { resetToolOptions } from "../tools/toolOptions";

const SAMPLE = "/Users/veri/Documents/SeePDF-샘플.pdf";

const TEXTBOX: AnnotSpec = {
  kind: "textbox", rect: { l: 50, b: 500, r: 250, t: 530 }, text: "정렬", fontSize: 12, color: [0, 0, 0], align: "left", fillColor: null,
};
const SQUARE: AnnotSpec = { kind: "square", rect: { l: 10, b: 10, r: 60, t: 60 }, color: [245, 83, 61], fillColor: null, width: 2, opacity: 1 };
const HIGHLIGHT: AnnotSpec = { kind: "highlight", rects: [{ l: 100, b: 600, r: 220, t: 612 }], color: [255, 216, 77], opacity: 0.4 };

async function openSample() {
  const info = await useDocStore.getState().open(SAMPLE);
  if (!info) throw new Error("open failed");
  return info;
}

beforeEach(() => {
  resetMock();
  resetPatchQueue();
  resetStampImages();
  resetToolOptions();
  useAnnotStore.getState().reset();
  useAnnotStore.setState({ toolDefaults: {}, styleTool: null, selected: [] });
  useAppStore.setState({ mode: "annotate", tool: "select" });
  useDialogStore.getState().closeAll();
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe("A9 — 작성자", () => {
  it("the mock writes no author for a blank 작성자, the trimmed name otherwise — like the engine", async () => {
    const info = await openSample();
    await api.setSettings({ patch: { author: "   " } });
    const blank = await api.createAnnotation({ docId: info.docId, page: 0, spec: SQUARE });
    expect(blank.annot!.author).toBeNull();
    await api.setSettings({ patch: { author: " 홍길동 " } });
    const named = await api.createAnnotation({ docId: info.docId, page: 0, spec: SQUARE });
    expect(named.annot!.author).toBe("홍길동");
  });

  it("the optimistic ghost does not invent an author either", async () => {
    await openSample();
    const settings = await api.getSettings();
    useAppStore.setState({ settings: { ...settings, author: "" } });
    const pending = createAnnotation(0, SQUARE);
    expect(useAnnotStore.getState().ghosts.at(-1)?.annot.author).toBeNull();
    await pending;
  });
});

describe("A1 / A10 — the panel edits the selected annotation", () => {
  it("가운데 with a text box selected sends {align: 'center'} and leaves the tool style alone", async () => {
    const info = await openSample();
    const box = await createAnnotation(0, TEXTBOX);
    if (!box) throw new Error("create failed");
    const styleBefore = useAnnotStore.getState().style;
    const spy = vi.spyOn(api, "updateAnnotation");
    render(<InspectorBody />);
    fireEvent.click(screen.getByRole("button", { name: "가운데" }));
    await flushPatches();
    expect(spy).toHaveBeenCalledTimes(1);
    expect(spy.mock.calls[0][0]).toMatchObject({ docId: info.docId, page: 0, id: box.id, patch: { align: "center" } });
    expect(useAnnotStore.getState().style).toEqual(styleBefore);
    // the panel now shows the annotation's own alignment
    await waitFor(() => expect(screen.getByRole("button", { name: "가운데" })).toHaveAttribute("data-active", "true"));
  });

  it("시작 화살표 on a selected line sends its heads; the line reads back as an arrow", async () => {
    await openSample();
    const line = await createAnnotation(0, { kind: "line", p1: [10, 10], p2: [200, 10], color: [0, 0, 0], width: 2, opacity: 1 });
    if (!line) throw new Error("create failed");
    const spy = vi.spyOn(api, "updateAnnotation");
    render(<InspectorBody />);
    fireEvent.click(screen.getByRole("checkbox", { name: "시작 화살표" }));
    await flushPatches();
    expect(spy.mock.calls[0][0].patch).toEqual({ heads: [true, false] });
    const after = useAnnotStore.getState().byPage[0].find((a) => a.id === line.id);
    expect(after?.kind).toBe("arrow");
    expect(after?.heads).toEqual([true, false]);
  });

  it("unticking 인쇄 sends {printed: false}", async () => {
    await openSample();
    const sq = await createAnnotation(0, SQUARE);
    if (!sq) throw new Error("create failed");
    const spy = vi.spyOn(api, "updateAnnotation");
    render(<InspectorBody />);
    const box = screen.getByRole("checkbox", { name: "인쇄" });
    expect(box).toBeChecked();
    fireEvent.click(box);
    await flushPatches();
    expect(spy.mock.calls[0][0].patch).toEqual({ printed: false });
    await waitFor(() => expect(screen.getByRole("checkbox", { name: "인쇄" })).not.toBeChecked());
  });
});

describe("A5 — quick popover", () => {
  let stop: (() => void) | null = null;
  afterEach(() => {
    stop?.();
    stop = null;
    toolController.arm("select");
  });

  function pageCtx(): PageLayerContext {
    const info = useDocStore.getState().info!;
    return makePageLayerContext({
      docId: info.docId, docGeneration: info.docGeneration, page: info.pages[0], rotation: 0, zoomPercent: 100,
      width: info.pages[0].widthPt, height: info.pages[0].heightPt,
    });
  }

  function Surface() {
    return <>{host.renderLayers(pageCtx()).surface}</>;
  }

  it("selecting a highlight shows the popover; a swatch sends {color}", async () => {
    await openSample();
    stop = host.start();
    const hl = await createAnnotation(0, HIGHLIGHT, { select: false });
    if (!hl) throw new Error("create failed");
    render(<Surface />);
    expect(screen.queryByTestId("quick-popover")).toBeNull();
    act(() => useAnnotStore.getState().select([hl.id]));
    const popover = await screen.findByTestId("quick-popover");
    const spy = vi.spyOn(api, "updateAnnotation");
    fireEvent.click(within(popover).getByRole("button", { name: "색상 연두" }));
    await flushPatches();
    expect(spy.mock.calls[0][0]).toMatchObject({ id: hl.id, patch: { color: [123, 214, 74] } });
    // no thickness for a markup run; there is for a shape
    expect(within(popover).queryByRole("button", { name: "8" })).toBeNull();
    fireEvent.click(within(popover).getByRole("button", { name: /삭제/ }));
    await waitFor(() => expect(screen.queryByTestId("quick-popover")).toBeNull());
  });
});

describe("T1 / T2 — stamps", () => {
  let stop: (() => void) | null = null;
  afterEach(() => {
    stop?.();
    stop = null;
    toolController.arm("select");
  });

  it("picking a 400 × 100 image places a stamp with a 4:1 rect", async () => {
    await openSample();
    stop = host.start();
    vi.spyOn(api, "openFileDialog").mockResolvedValue(["/Users/veri/Pictures/seal.png"]);
    setMockImageSize("/Users/veri/Pictures/seal.png", 400, 100);
    toolController.arm("stamp");
    const picker = useDialogStore.getState().stack.at(-1);
    expect(picker?.name).toBe("stampPicker");
    (picker?.props.onChooseImage as () => void)();
    await waitFor(() => expect(stampImage("stamp")).toEqual({ path: "/Users/veri/Pictures/seal.png" }));
    await waitFor(() => expect(stampAspect("stamp")).toBe(4));
    const size = placementSize("stamp", stampImage("stamp"));
    expect(size.w / size.h).toBe(4);

    const spy = vi.spyOn(api, "createAnnotation");
    const ctx = {
      docId: useDocStore.getState().info!.docId, docGeneration: 1, style: useAnnotStore.getState().style,
      modifiers: { shift: false, alt: false, meta: false, ctrl: false }, scale: 1,
      image: stampImage("stamp"),
    };
    toolController.pointerDown({ page: 0, pt: [300, 400] }, ctx);
    toolController.pointerUp({ page: 0, pt: [300, 400] }, ctx);
    await waitFor(() => expect(spy).toHaveBeenCalled());
    const spec = spy.mock.calls[0][0].spec as Extract<AnnotSpec, { kind: "stamp" }>;
    expect((spec.rect.r - spec.rect.l) / (spec.rect.t - spec.rect.b)).toBeCloseTo(4, 5);
    closeDialog("stampPicker");
  });

  it("an image added to 내 도장 is kept in Settings.stamps and listed again after a reload", async () => {
    await openSample();
    useAppStore.setState({ settings: await api.getSettings() });
    vi.spyOn(api, "openFileDialog").mockResolvedValue(["/Users/veri/Pictures/도장.png"]);
    setMockImageSize("/Users/veri/Pictures/도장.png", 300, 300);
    const first = render(<StampPickerDialog onClose={vi.fn()} onPick={vi.fn()} onChooseImage={vi.fn()} />);
    fireEvent.click(screen.getByRole("button", { name: "이미지 도장 추가…" }));
    await waitFor(async () => expect((await api.getSettings()).stamps).toHaveLength(1));
    const saved = (await api.getSettings()).stamps[0] as Extract<CustomStamp, { kind: "image" }>;
    expect(saved).toMatchObject({ kind: "image", aspect: 1 });
    expect(saved.path).toMatch(/^\/mock\/app-data\/stamps\/img-[0-9a-f]+\.png$/);
    first.unmount();

    // "reload": fresh settings from the backend, a fresh picker
    useAppStore.setState({ settings: await api.getSettings() });
    const onPickCustom = vi.fn();
    render(<StampPickerDialog onClose={vi.fn()} onPick={vi.fn()} onChooseImage={vi.fn()} onPickCustom={onPickCustom} />);
    const mine = screen.getByRole("region", { name: "내 도장" });
    fireEvent.click(within(mine).getByRole("button", { name: "이미지 도장" }));
    expect(onPickCustom).toHaveBeenCalledWith(expect.objectContaining({ kind: "image", path: saved.path, aspect: 1 }));
  });

  it("a text stamp with {{date}} is added, and 오늘 날짜 / ✓ are offered", async () => {
    await openSample();
    useAppStore.setState({ settings: await api.getSettings() });
    const onPick = vi.fn();
    const onPickCustom = vi.fn();
    render(<StampPickerDialog onClose={vi.fn()} onPick={onPick} onChooseImage={vi.fn()} onPickCustom={onPickCustom} />);
    fireEvent.change(screen.getByRole("textbox", { name: /도장 문구/ }), { target: { value: "검토 완료 {{date}}" } });
    fireEvent.click(screen.getByRole("button", { name: "텍스트 도장 추가" }));
    await waitFor(async () => expect((await api.getSettings()).stamps[0]).toMatchObject({ kind: "text", text: "검토 완료 {{date}}", shape: "rect" }));
    fireEvent.click(screen.getByRole("option", { name: "확인 표시 ✓" }));
    expect(onPick).toHaveBeenCalledWith("check");
    fireEvent.click(screen.getByRole("option", { name: "오늘 날짜" }));
    expect(onPickCustom).toHaveBeenCalledWith(expect.objectContaining({ kind: "text", text: "{{date}}", shape: "none" }));
    // the engine (and the mock) expand the tokens at placement
    const info = useDocStore.getState().info!;
    const made = await api.createAnnotation({
      docId: info.docId, page: 0,
      spec: { kind: "stamp", rect: { l: 0, b: 0, r: 100, t: 40 }, image: { text: "{{date}}", color: [206, 32, 41], shape: "none" } },
    });
    expect(made.annot!.contents).toMatch(/^\d{4}\.\d{2}\.\d{2}$/);
    expect(made.annot!.stampKind).toBe("SeePDF:TextStamp");
  });
});

describe("A2 — polygon tool end to end", () => {
  let stop: (() => void) | null = null;
  afterEach(() => {
    stop?.();
    stop = null;
    toolController.arm("select");
  });

  it("three clicks and a double-click create one polygon; ↵ finishes too", async () => {
    await openSample();
    stop = host.start();
    useAppStore.setState({ tool: "polygon" });
    toolController.arm("polygon");
    const spy = vi.spyOn(api, "createAnnotation");
    const ctx = {
      docId: useDocStore.getState().info!.docId, docGeneration: 1, style: useAnnotStore.getState().style,
      modifiers: { shift: false, alt: false, meta: false, ctrl: false }, scale: 1,
    };
    for (const pt of [[100, 100], [200, 100], [150, 180], [150, 180]] as [number, number][]) {
      toolController.pointerDown({ page: 0, pt }, ctx);
      toolController.pointerUp({ page: 0, pt }, ctx);
    }
    await waitFor(() => expect(spy).toHaveBeenCalledTimes(1));
    expect(spy.mock.calls[0][0].spec).toMatchObject({ kind: "polygon", vertices: [100, 100, 200, 100, 150, 180] });
    await waitFor(() => expect(useAnnotStore.getState().byPage[0].some((a) => a.kind === "polygon")).toBe(true));

    for (const pt of [[300, 300], [400, 300]] as [number, number][]) {
      toolController.pointerDown({ page: 0, pt }, ctx);
      toolController.pointerUp({ page: 0, pt }, ctx);
    }
    fireEvent.keyDown(window, { key: "Enter" });
    // two points are not a polygon: nothing is created, the gesture is over
    await new Promise((r) => setTimeout(r, 30));
    expect(spy).toHaveBeenCalledTimes(1);
    expect(toolController.pendingState()).toMatchObject({ page: null });
  });
});
