/**
 * One test per tool state machine (WORKPLAN (d) DoD): pointer down / move / up in, `AnnotSpec` out.
 * Everything here runs without React, without the DOM and without the IPC layer — that is the
 * whole point of `ToolModule` being pure.
 */
import { describe, expect, it } from "vitest";
import type { Annot, Rect } from "../ipc/types";
import { toolController, type PagePoint, type ToolContext } from "./ToolController";
import { registerTools, resetToolRegistry } from "./registry";
import { makeMarkupTool, markupSpec, usableRects } from "./markup";
import { inkTool } from "./ink";
import { makeShapeTool } from "./shapes";
import { makeLineTool } from "./line";
import { noteTool } from "./note";
import { textBoxTool } from "./textbox";
import { eraserTool, eraserHits } from "./eraser";
import { makeStampTool, setStampImage, resetStampImages } from "./stamp";
import { selectTool } from "./select";
import { smoothPath, snapAngle, rectFrom } from "./geometry";

const STYLE: ToolContext["style"] = {
  color: [255, 216, 77],
  opacity: 0.4,
  width: 2,
  fontSize: 12,
  fillColor: null,
  heads: [false, true],
  align: "left",
  eraserSize: 12,
};

function ctx(over: Partial<ToolContext> = {}): ToolContext {
  return {
    docId: "d1",
    docGeneration: 1,
    style: STYLE,
    modifiers: { shift: false, alt: false, meta: false, ctrl: false },
    scale: 1,
    ...over,
  };
}

const at = (x: number, y: number, page = 0): PagePoint => ({ page, pt: [x, y] });

/** Drive a whole gesture through a module and return the last result. */
function gesture<S>(
  module: { init(c: ToolContext): S; onDown(s: S, p: PagePoint, c: ToolContext): { state: S }; onMove(s: S, p: PagePoint, c: ToolContext): { state: S }; onUp(s: S, p: PagePoint, c: ToolContext): unknown },
  points: PagePoint[],
  c = ctx(),
) {
  let state = module.init(c);
  state = module.onDown(state, points[0], c).state;
  for (const p of points.slice(1, -1)) state = module.onMove(state, p, c).state;
  return module.onUp(state, points[points.length - 1], c) as {
    commit?: { page: number; spec: import("../ipc/types").AnnotSpec };
    edit?: { page: number; rect?: Rect };
    erase?: { page: number; ids: string[] };
    select?: { ids: string[] };
    patch?: { page: number; edits: { id: string; patch: unknown }[] };
  };
}

function annot(over: Partial<Annot>): Annot {
  return {
    id: "a1",
    page: 0,
    kind: "square",
    subtype: "Square",
    rect: { l: 10, b: 10, r: 50, t: 50 },
    color: [0, 0, 0],
    fillColor: null,
    opacity: 1,
    borderWidth: 2,
    contents: "",
    author: null,
    created: null,
    modified: null,
    hidden: false,
    printed: true,
    locked: false,
    editable: "full",
    ...over,
  };
}

describe("tools.markup", () => {
  const RECTS: Rect[] = [
    { l: 72, b: 690, r: 300, t: 702 },
    { l: 72, b: 676, r: 220, t: 688 },
  ];

  it("turns a two-line text selection into one quad per line", () => {
    const tool = makeMarkupTool("highlight");
    const c = ctx({ selectionRects: () => RECTS });
    const result = gesture(tool, [at(80, 700), at(220, 680)], c);
    expect(result.commit?.spec).toEqual({
      kind: "highlight",
      rects: RECTS,
      color: STYLE.color,
      opacity: 0.4,
    });
    // one rectangle per line run — merging them would paint over the gutter (F-08)
    expect(result.commit?.spec.kind === "highlight" && result.commit.spec.rects).toHaveLength(2);
  });

  it("commits nothing when the selection is empty", () => {
    const tool = makeMarkupTool("underline");
    const result = gesture(tool, [at(80, 700), at(80, 700)], ctx({ selectionRects: () => [] }));
    expect(result.commit).toBeUndefined();
  });

  it("drops pdfium's zero-area whitespace rectangles", () => {
    expect(usableRects([{ l: 10, b: 10, r: 10, t: 10 }, RECTS[0]])).toEqual([RECTS[0]]);
  });

  it("keeps the three line markups opaque and only 형광펜 translucent", () => {
    const opacityOf = (kind: "highlight" | "strikeout" | "squiggly") => {
      const spec = markupSpec(kind, RECTS, ctx());
      return spec && "opacity" in spec ? spec.opacity : null;
    };
    expect(opacityOf("highlight")).toBe(0.4);
    expect(opacityOf("strikeout")).toBe(1);
    expect(opacityOf("squiggly")).toBe(1);
  });
});

describe("tools.ink", () => {
  it("collects a stroke and simplifies it on pointer-up", () => {
    const points = Array.from({ length: 40 }, (_, i) => at(10 + i * 2, 10 + Math.sin(i / 4) * 6));
    const result = gesture(inkTool, points);
    const spec = result.commit?.spec;
    expect(spec?.kind).toBe("ink");
    if (spec?.kind !== "ink") throw new Error("expected ink");
    expect(spec.paths).toHaveLength(1);
    expect(spec.paths[0].length).toBeLessThan(points.length * 2);
    expect(spec.paths[0].length).toBeGreaterThanOrEqual(4);
    expect(spec.width).toBe(STYLE.width);
    expect(spec.opacity).toBe(STYLE.opacity);
    // the pen starts exactly where the pointer went down
    expect(spec.paths[0].slice(0, 2)).toEqual([10, 10]);
  });

  it("⇧ turns the stroke into a straight segment", () => {
    const c = ctx({ modifiers: { shift: true, alt: false, meta: false, ctrl: false } });
    const result = gesture(inkTool, [at(0, 0), at(5, 30), at(40, 40)], c);
    const spec = result.commit?.spec;
    if (spec?.kind !== "ink") throw new Error("expected ink");
    expect(spec.paths[0]).toEqual([0, 0, 40, 40]);
  });

  it("smoothing never moves the end points", () => {
    const path = [0, 0, 10, 4, 20, 0, 30, 5, 40, 0];
    const out = smoothPath(path, 0.1);
    expect(out.slice(0, 2)).toEqual([0, 0]);
    expect(out.slice(-2)).toEqual([40, 0]);
  });
});

describe("tools.shape", () => {
  it("drags a square out of two corners", () => {
    const result = gesture(makeShapeTool("rectangle"), [at(20, 700), at(60, 660), at(120, 600)]);
    expect(result.commit?.spec).toEqual({
      kind: "square",
      rect: { l: 20, b: 600, r: 120, t: 700 },
      color: STYLE.color,
      fillColor: null,
      width: 2,
      opacity: 0.4,
    });
  });

  it("⇧ constrains it to a square and ⌥ grows it from the centre", () => {
    const shift = ctx({ modifiers: { shift: true, alt: false, meta: false, ctrl: false } });
    const r = gesture(makeShapeTool("ellipse"), [at(100, 100), at(160, 130)], shift).commit?.spec;
    if (r?.kind !== "circle") throw new Error("expected circle");
    expect(r.rect.r - r.rect.l).toBe(r.rect.t - r.rect.b);

    const alt = ctx({ modifiers: { shift: false, alt: true, meta: false, ctrl: false } });
    expect(rectFrom([100, 100], [120, 110], { fromCentre: true })).toEqual({ l: 80, b: 90, r: 120, t: 110 });
    const centred = gesture(makeShapeTool("rectangle"), [at(100, 100), at(120, 110)], alt).commit?.spec;
    if (centred?.kind !== "square") throw new Error("expected square");
    expect(centred.rect).toEqual({ l: 80, b: 90, r: 120, t: 110 });
  });

  it("a click with no drag still makes a shape", () => {
    const r = gesture(makeShapeTool("rectangle"), [at(50, 50), at(50, 50)]).commit?.spec;
    if (r?.kind !== "square") throw new Error("expected square");
    expect(r.rect).toEqual({ l: 38, b: 38, r: 62, t: 62 });
  });
});

describe("tools.line", () => {
  it("makes a line from the two drag points and keeps the arrow heads", () => {
    const r = gesture(makeLineTool("arrow"), [at(10, 10), at(40, 40), at(100, 60)]).commit?.spec;
    if (r?.kind !== "arrow") throw new Error("expected arrow");
    expect(r.p1).toEqual([10, 10]);
    expect(r.p2).toEqual([100, 60]);
    expect(r.heads).toEqual([false, true]);
  });

  it("a plain 선 carries no heads", () => {
    const r = gesture(makeLineTool("line"), [at(0, 0), at(80, 0)]).commit?.spec;
    if (r?.kind !== "line") throw new Error("expected line");
    expect(r.heads).toBeUndefined();
  });

  it("⇧ snaps to 15°", () => {
    expect(snapAngle([0, 0], [100, 10])[1]).toBeCloseTo(0, 6);
    const c = ctx({ modifiers: { shift: true, alt: false, meta: false, ctrl: false } });
    const r = gesture(makeLineTool("line"), [at(0, 0), at(100, 8)], c).commit?.spec;
    if (r?.kind !== "line") throw new Error("expected line");
    expect(r.p2[1]).toBeCloseTo(0, 6);
  });

  it("a click shorter than 3 pt is not a line", () => {
    expect(gesture(makeLineTool("line"), [at(10, 10), at(11, 10)]).commit).toBeUndefined();
  });
});

describe("tools.note", () => {
  it("places a sticky note at the click and hands control back", () => {
    const result = gesture(noteTool, [at(430, 722), at(430, 722)]);
    expect(result.commit?.spec).toEqual({ kind: "note", at: [430, 722], color: STYLE.color, contents: "" });
  });
});

describe("tools.textbox", () => {
  it("opens an editor over the dragged rectangle instead of committing", () => {
    const result = gesture(textBoxTool, [at(100, 500), at(200, 460)]);
    expect(result.commit).toBeUndefined();
    expect(result.edit).toEqual({ page: 0, rect: { l: 100, b: 460, r: 200, t: 500 } });
  });

  it("a click auto-sizes the box from the font size", () => {
    const result = gesture(textBoxTool, [at(100, 500), at(100, 500)]);
    expect(result.edit?.rect).toEqual({ l: 100, t: 500, r: 280, b: 500 - 12 * 1.6 });
  });
});

describe("tools.stamp", () => {
  it("places the picked image at a default rectangle on a click", () => {
    resetStampImages();
    setStampImage("stamp", { path: "/tmp/seal.png" });
    const result = gesture(makeStampTool("stamp"), [at(300, 400), at(300, 400)]);
    expect(result.commit?.spec).toEqual({
      kind: "stamp",
      rect: { l: 252, r: 348, b: 352, t: 448 },
      image: { path: "/tmp/seal.png" },
    });
  });

  it("commits nothing until an image has been chosen", () => {
    resetStampImages();
    const result = gesture(makeStampTool("signature"), [at(300, 400), at(300, 400)], ctx({ image: null }));
    expect(result.commit).toBeUndefined();
  });
});

describe("tools.eraser", () => {
  const ink = annot({ id: "ink1", kind: "ink", inkPaths: [[0, 0, 100, 0]], rect: { l: 0, b: -1, r: 100, t: 1 } });
  const far = annot({ id: "sq1", rect: { l: 400, b: 400, r: 450, t: 450 } });

  it("hits a stroke it passes over and leaves the rest alone", () => {
    expect(eraserHits([ink, far], 50, 2, 6)).toEqual(["ink1"]);
    expect(eraserHits([ink, far], 50, 60, 6)).toEqual([]);
  });

  it("collects every stroke of one drag and reports each id once", () => {
    const c = ctx({ annots: () => [ink] });
    let state = eraserTool.init(c);
    const down = eraserTool.onDown(state, at(0, 0), c);
    state = down.state;
    expect(down.erase).toEqual({ page: 0, ids: ["ink1"] });
    const move = eraserTool.onMove(state, at(50, 0), c);
    expect(move.erase).toBeUndefined(); // already erased, not reported twice
  });

  it("never erases a locked annotation", () => {
    expect(eraserHits([annot({ id: "l", locked: true })], 20, 20, 4)).toEqual([]);
  });
});

describe("tools.select", () => {
  const square = annot({ id: "sq", rect: { l: 100, b: 100, r: 200, t: 200 } });

  it("clicking an annotation selects it", () => {
    const c = ctx({ annots: () => [square], selected: [] });
    const down = selectTool.onDown(selectTool.init(c), at(150, 150), c);
    expect(down.select).toEqual({ ids: ["sq"] });
  });

  it("dragging the body moves it, and the final patch is not live", () => {
    const c = ctx({ annots: () => [square], selected: ["sq"] });
    let state = selectTool.onDown(selectTool.init(c), at(150, 150), c).state;
    const moved = selectTool.onMove(state, at(160, 170), c);
    expect(moved.patch?.live).toBe(true);
    state = moved.state;
    const up = selectTool.onUp(state, at(160, 170), c);
    expect(up.patch?.live).toBe(false);
    expect(up.patch?.edits[0].patch.rect).toEqual({ l: 110, b: 120, r: 210, t: 220 });
  });

  it("dragging a corner handle resizes it", () => {
    const c = ctx({ annots: () => [square], selected: ["sq"] });
    const down = selectTool.onDown(selectTool.init(c), at(200, 200), c);
    const up = selectTool.onUp(down.state, at(240, 260), c);
    expect(up.patch?.edits[0].patch.rect).toEqual({ l: 100, b: 100, r: 240, t: 260 });
  });

  it("a marquee over empty space selects what it touches", () => {
    const c = ctx({ annots: () => [square], selected: [] });
    const down = selectTool.onDown(selectTool.init(c), at(10, 10), c);
    expect(down.preview?.kind).toBe("marquee");
    const up = selectTool.onUp(down.state, at(300, 300), c);
    expect(up.select).toEqual({ ids: ["sq"] });
  });

  it("clicking a 메모 opens its editor", () => {
    const note = annot({ id: "n", kind: "note", rect: { l: 10, b: 10, r: 32, t: 32 } });
    const c = ctx({ annots: () => [note], selected: [] });
    const down = selectTool.onDown(selectTool.init(c), at(20, 20), c);
    const up = selectTool.onUp(down.state, at(20, 20), c);
    expect(up.edit).toEqual({ page: 0, id: "n" });
  });

  it("locked annotations cannot be dragged", () => {
    const locked = annot({ id: "lk", locked: true, rect: { l: 0, b: 0, r: 40, t: 40 } });
    const c = ctx({ annots: () => [locked], selected: [] });
    const down = selectTool.onDown(selectTool.init(c), at(20, 20), c);
    const up = selectTool.onUp(down.state, at(60, 60), c);
    expect(up.patch).toBeUndefined();
  });
});

describe("ToolController", () => {
  it("routes a gesture to the armed module and hands the spec to the sink", () => {
    resetToolRegistry();
    registerTools();
    const commits: { page: number; kind: string }[] = [];
    toolController.setSink({
      commit: (page, spec) => commits.push({ page, kind: spec.kind }),
      erase: () => undefined,
      patch: () => undefined,
      select: () => undefined,
      edit: () => undefined,
      done: () => undefined,
    });
    toolController.arm("rectangle");
    expect(toolController.cursor()).toBe("crosshair");
    const c = ctx();
    toolController.pointerDown(at(10, 10), c);
    toolController.pointerMove(at(60, 60), c);
    expect(toolController.preview()?.kind).toBe("rect");
    toolController.pointerUp(at(60, 60), c);
    expect(commits).toEqual([{ page: 0, kind: "square" }]);
    expect(toolController.preview()).toBeNull();
    toolController.setSink(null);
  });

  it("cancel drops the in-flight gesture without committing", () => {
    resetToolRegistry();
    registerTools();
    const commits: unknown[] = [];
    toolController.setSink({
      commit: (_page, spec) => commits.push(spec),
      erase: () => undefined,
      patch: () => undefined,
      select: () => undefined,
      edit: () => undefined,
      done: () => undefined,
    });
    toolController.arm("pen");
    const c = ctx();
    toolController.pointerDown(at(0, 0), c);
    toolController.pointerMove(at(20, 20), c);
    toolController.cancel();
    toolController.pointerUp(at(20, 20), c);
    expect(commits).toEqual([]);
    toolController.setSink(null);
  });
});
