/**
 * v0.3 pkg4-annotations-stamps-objects — the pure tool state machines and geometry:
 * A3 (partial eraser), A4 (pen / touch / pressure), A2 (polygon, callout, measuring labels).
 */
import { beforeEach, describe, expect, it } from "vitest";
import type { Annot, AnnotSpec } from "../ipc/types";
import type { ToolContext } from "./ToolController";
import { measureLabel, pathLength, polygonArea, splitPathByCircle } from "./geometry";
import { eraserTool, partialDab } from "./eraser";
import { inkSpecs, inkTool, pressureWidth } from "./ink";
import { polygonTool, polygonSpec, type PolygonState } from "./polygon";
import { calloutGeometry, calloutTool } from "./callout";
import { makeLineTool } from "./line";
import { resetToolOptions, setToolOptions } from "./toolOptions";
import { baseStyleFor } from "../store/toolStyles";

function ctx(extra: Partial<ToolContext> = {}): ToolContext {
  return {
    docId: "d1",
    docGeneration: 1,
    style: baseStyleFor("pen"),
    modifiers: { shift: false, alt: false, meta: false, ctrl: false },
    scale: 1,
    ...extra,
  };
}

const INK: Annot = {
  id: "ink1", page: 0, kind: "ink", subtype: "Ink", rect: { l: 0, b: 90, r: 200, t: 110 },
  inkPaths: [[0, 100, 100, 100, 200, 100]], color: [0, 0, 0], fillColor: null, opacity: 1, borderWidth: 2,
  contents: "", author: null, created: null, modified: null, hidden: false, printed: true, locked: false, editable: "full",
};

beforeEach(() => resetToolOptions());

describe("A3 — splitting a stroke by the eraser circle", () => {
  it("a circle over the middle of a polyline leaves two pieces, cut exactly on the circle", () => {
    const pieces = splitPathByCircle([0, 0, 100, 0, 200, 0], 100, 0, 10);
    expect(pieces).toHaveLength(2);
    expect(pieces[0]).toEqual([0, 0, 90, 0]);
    expect(pieces[1]).toEqual([110, 0, 200, 0]);
  });

  it("a circle away from the stroke leaves it whole; one covering it leaves nothing", () => {
    expect(splitPathByCircle([0, 0, 100, 0], 50, 50, 10)).toEqual([[0, 0, 100, 0]]);
    expect(splitPathByCircle([0, 0, 10, 0], 5, 0, 50)).toEqual([]);
    // an end cut off
    expect(splitPathByCircle([0, 0, 100, 0], 100, 0, 20)).toEqual([[0, 0, 80, 0]]);
  });

  it("a partial scrub across the middle of an ink stroke patches it into 2 strokes on pointer-up", () => {
    setToolOptions({ eraserMode: "partial" });
    const c = ctx({ style: { ...baseStyleFor("eraser"), eraserSize: 10 }, annots: () => [INK] });
    let r = eraserTool.onDown(eraserTool.init(c), { page: 0, pt: [100, 120] }, c);
    expect(r.erase).toBeUndefined();
    r = eraserTool.onMove(r.state, { page: 0, pt: [100, 105] }, c);
    r = eraserTool.onMove(r.state, { page: 0, pt: [100, 100] }, c);
    r = eraserTool.onMove(r.state, { page: 0, pt: [100, 90] }, c);
    const up = eraserTool.onUp(r.state, { page: 0, pt: [100, 90] }, c);
    expect(up.erase).toBeUndefined();
    expect(up.partialErase?.edits).toHaveLength(1);
    const edit = up.partialErase!.edits[0];
    expect(edit.id).toBe("ink1");
    expect(edit.paths).toHaveLength(2);
    expect(edit.paths[0][0]).toBe(0);
    expect(edit.paths[1].at(-2)).toBe(200);
  });

  it("whole-stroke mode (the default) still deletes the stroke; a stroke wiped out entirely is deleted", () => {
    const c = ctx({ style: { ...baseStyleFor("eraser"), eraserSize: 10 }, annots: () => [INK] });
    const r = eraserTool.onDown(eraserTool.init(c), { page: 0, pt: [100, 100] }, c);
    expect(r.erase).toEqual({ page: 0, ids: ["ink1"] });
    const tiny: Annot = { ...INK, id: "tiny", inkPaths: [[100, 100, 101, 100]] };
    expect(partialDab([tiny], {}, 100, 100, 10)).toEqual({ tiny: [] });
  });
});

describe("A4 — pen and touch", () => {
  it("a touch pointerdown with 펜으로만 그리기 does not start a stroke; a pen does, and turns it on by itself", async () => {
    const { notePointer, toolOptions } = await import("./toolOptions");
    expect(toolOptions().penOnly).toBe(false);
    notePointer("pen");
    expect(toolOptions().penOnly).toBe(true);
    const c = ctx();
    const touch = inkTool.onDown(inkTool.init(c), { page: 0, pt: [10, 10], pointerType: "touch" }, c);
    expect(touch.state.page).toBeNull();
    expect(touch.preview).toBeUndefined();
    const pen = inkTool.onDown(inkTool.init(c), { page: 0, pt: [10, 10], pointerType: "pen", pressure: 0.5 }, c);
    expect(pen.state.page).toBe(0);
    // switched off again, a finger draws
    setToolOptions({ penOnly: false });
    expect(inkTool.onDown(inkTool.init(c), { page: 0, pt: [10, 10], pointerType: "touch" }, c).state.page).toBe(0);
  });

  it("pressure 0.2 and 1.0 give different widths", () => {
    expect(pressureWidth(2, 0.2)).toBeLessThan(pressureWidth(2, 1));
    const c = ctx();
    const stroke = [0, 0, 10, 0, 20, 0, 30, 0, 40, 0];
    const light = inkSpecs(stroke, [0.2, 0.2, 0.2, 0.2, 0.2], c);
    const hard = inkSpecs(stroke, [1, 1, 1, 1, 1], c);
    const w = (s: AnnotSpec[]) => (s[0] as Extract<AnnotSpec, { paths: number[][] }>).width;
    expect(light).toHaveLength(1);
    expect(hard).toHaveLength(1);
    expect(w(light)).toBeLessThan(w(hard));
    // a mouse draws at the style's width
    expect(w(inkSpecs(stroke, null, c))).toBe(c.style.width);
  });

  it("a stroke whose pressure varies becomes joined pieces of different widths", () => {
    const c = ctx();
    const stroke = Array.from({ length: 12 }, (_, i) => [i * 10, 0]).flat();
    const pressures = [0.1, 0.1, 0.1, 0.1, 0.5, 0.5, 0.5, 0.5, 1, 1, 1, 1];
    const specs = inkSpecs(stroke, pressures, c) as Extract<AnnotSpec, { paths: number[][] }>[];
    expect(specs.length).toBeGreaterThan(1);
    const widths = specs.map((s) => s.width);
    expect(new Set(widths).size).toBe(specs.length);
    // consecutive pieces share their joint: no gap in the stroke
    for (let i = 1; i < specs.length; i++) {
      const prev = specs[i - 1].paths[0];
      expect(specs[i].paths[0].slice(0, 2)).toEqual(prev.slice(-2));
    }
  });
});

describe("A2 — polygon, callout, measuring", () => {
  it("collects a vertex per click, previews the rubber band and finishes on a double-click", () => {
    const c = ctx({ style: baseStyleFor("polygon") });
    let s: PolygonState = polygonTool.init(c);
    const click = (x: number, y: number) => {
      const r = polygonTool.onDown(s, { page: 2, pt: [x, y] }, c);
      s = polygonTool.onUp(r.state, { page: 2, pt: [x, y] }, c).state;
      return r;
    };
    click(10, 10);
    click(110, 10);
    const hover = polygonTool.onMove(s, { page: 2, pt: [60, 90] }, c);
    expect(hover.preview).toMatchObject({ kind: "poly", points: [10, 10, 110, 10, 60, 90], closed: true });
    click(60, 90);
    expect(s.vertices).toEqual([10, 10, 110, 10, 60, 90]);
    const done = click(60, 90.5); // the second click of the double-click
    expect(done.commit).toMatchObject({ page: 2, spec: { kind: "polygon", vertices: [10, 10, 110, 10, 60, 90] } });
    expect(done.state.vertices).toEqual([]);
  });

  it("clicking the first vertex closes it, ⌫ drops the last one, and the shape option picks the kind", () => {
    const c = ctx({ style: baseStyleFor("polygon") });
    setToolOptions({ polygonShape: "cloud", measure: "mm" });
    let s = polygonTool.init(c);
    for (const pt of [[0, 0], [100, 0], [100, 100], [0, 100]] as [number, number][]) {
      s = polygonTool.onDown(s, { page: 0, pt }, c).state;
    }
    s = polygonTool.onKey!(s, "Backspace", c).state;
    expect(s.vertices).toEqual([0, 0, 100, 0, 100, 100]);
    const done = polygonTool.onDown(s, { page: 0, pt: [3, 2] }, c);
    expect(done.commit?.spec).toMatchObject({ kind: "polygon", cloudy: true, measure: "mm" });
    setToolOptions({ polygonShape: "polyline", measure: "off" });
    const open = polygonSpec([0, 0, 50, 50], c);
    expect(open).toMatchObject({ kind: "polyline", fillColor: null });
    expect(open && "measure" in open).toBe(false);
    expect(polygonSpec([0, 0], c)).toBeNull();
  });

  it("the callout tool opens a draft with the box at the release point and a leader from the press point", () => {
    const c = ctx({ style: baseStyleFor("callout") });
    const down = calloutTool.onDown(calloutTool.init(c), { page: 0, pt: [50, 50] }, c);
    const up = calloutTool.onUp(down.state, { page: 0, pt: [200, 300] }, c);
    expect(up.edit?.rect).toMatchObject({ l: 200, t: 300 });
    expect(up.edit?.callout).toEqual([50, 50, 200, (up.edit!.rect!.b + up.edit!.rect!.t) / 2]);
    expect(calloutGeometry([500, 50], [200, 300], 12).callout[2]).toBe(calloutGeometry([500, 50], [200, 300], 12).rect.r);
  });

  it("a measuring line carries its unit and previews its length; the labels match the engine's", () => {
    setToolOptions({ measure: "mm" });
    const tool = makeLineTool("line");
    const c = ctx({ style: baseStyleFor("line") });
    const down = tool.onDown(tool.init(c), { page: 0, pt: [0, 0] }, c);
    const move = tool.onMove(down.state, { page: 0, pt: [72, 0] }, c);
    expect(move.preview?.measureText).toBe("25.4 mm");
    const up = tool.onUp(down.state, { page: 0, pt: [72, 0] }, c);
    expect(up.commit?.spec).toMatchObject({ kind: "line", measure: "mm" });
    expect(measureLabel(polygonArea([0, 0, 72, 0, 72, 72, 0, 72]), "mm", true)).toBe("645.2 mm²");
    expect(measureLabel(pathLength([0, 0, 72, 0]), "pt", false)).toBe("72.0 pt");
  });
});
