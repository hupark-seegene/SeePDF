/**
 * 선택 tool geometry (bug hunt 2026-09): a click on a handle is not a resize (no spurious
 * 주석 편집 undo step), a line is hit along its stroke — not anywhere in its bounding box — and
 * ⇧ on an edge handle scales instead of doing nothing.
 */
import { describe, expect, it } from "vitest";
import type { Annot } from "../ipc/types";
import type { PagePoint, ToolContext } from "./ToolController";
import { selectTool } from "./select";
import { annotAt, hitsAnnot, resizeRect } from "./hit";

function annot(over: Partial<Annot>): Annot {
  return {
    id: "a1", page: 0, kind: "square", subtype: "Square", rect: { l: 10, b: 10, r: 50, t: 50 },
    color: [0, 0, 0], fillColor: null, opacity: 1, borderWidth: 2, contents: "", author: null,
    created: null, modified: null, hidden: false, printed: true, locked: false, editable: "full",
    ...over,
  };
}

function ctx(over: Partial<ToolContext> = {}): ToolContext {
  return {
    docId: "d1",
    docGeneration: 1,
    style: {
      color: [0, 0, 0], opacity: 1, width: 2, fontSize: 12, fillColor: null, heads: [false, true],
      align: "left", eraserSize: 12,
    },
    modifiers: { shift: false, alt: false, meta: false, ctrl: false },
    scale: 1,
    ...over,
  };
}

const at = (x: number, y: number): PagePoint => ({ page: 0, pt: [x, y] });

describe("select: a press on a resize handle without a drag", () => {
  const square = annot({ id: "sq", rect: { l: 100, b: 100, r: 200, t: 200 } });

  it("emits no patch — no update_annotation, no undo step, no dirty document", () => {
    const c = ctx({ annots: () => [square], selected: ["sq"] });
    // the se handle sits at (200, 100); the press lands a little off it, within the handle radius
    const down = selectTool.onDown(selectTool.init(c), at(203, 98), c);
    expect(down.state.mode).toBe("resize");
    // a jitter of the pointer is not a drag yet
    const jitter = selectTool.onMove(down.state, at(203.5, 98), c);
    expect(jitter.patch).toBeUndefined();
    const up = selectTool.onUp(jitter.state, at(203.5, 98), c);
    expect(up.patch).toBeUndefined();
  });

  it("a real drag of the handle still resizes", () => {
    const c = ctx({ annots: () => [square], selected: ["sq"] });
    const down = selectTool.onDown(selectTool.init(c), at(200, 100), c);
    const moved = selectTool.onMove(down.state, at(230, 80), c);
    expect(moved.patch?.live).toBe(true);
    const up = selectTool.onUp(moved.state, at(240, 70), c);
    expect(up.patch?.live).toBe(false);
    expect(up.patch?.edits[0].patch.rect).toEqual({ l: 100, b: 70, r: 240, t: 200 });
  });
});

describe("hit-testing a line or an arrow", () => {
  const highlight = annot({
    id: "hl", kind: "highlight", subtype: "Highlight", rect: { l: 380, b: 40, r: 480, t: 60 },
    quads: [{ l: 380, b: 40, r: 480, t: 60 }],
  });
  const line = annot({
    id: "ln", kind: "line", subtype: "Line", rect: { l: 0, b: 0, r: 500, t: 500 },
    linePoints: [0, 0, 500, 500], inkPaths: [[0, 0, 500, 500]],
  });

  it("a click far from the stroke falls through to the annotation underneath", () => {
    // topmost-first: the line is listed after the highlight
    expect(annotAt([highlight, line], 430, 50, 6)?.id).toBe("hl");
    expect(hitsAnnot(line, 430, 50, 6)).toBe(false);
    expect(hitsAnnot({ ...line, kind: "arrow", subtype: "Line" }, 430, 50, 6)).toBe(false);
  });

  it("a click on the stroke still grabs the line, with or without ink paths", () => {
    expect(annotAt([highlight, line], 250, 252, 6)?.id).toBe("ln");
    const bare = { ...line, inkPaths: undefined };
    expect(hitsAnnot(bare, 250, 252, 6)).toBe(true);
    expect(hitsAnnot(bare, 430, 50, 6)).toBe(false);
  });

  it("a drawn signature is still grabbable from inside its bounds", () => {
    const signature = annot({
      id: "sig", kind: "signature", subtype: "Ink", rect: { l: 0, b: 0, r: 100, t: 40 },
      inkPaths: [[0, 0, 20, 40, 40, 0]],
    });
    expect(hitsAnnot(signature, 80, 20, 2)).toBe(true);
  });
});

describe("⇧ on an edge handle", () => {
  it("n / s scale the width with the height instead of swallowing the drag", () => {
    const rect = { l: 100, b: 100, r: 200, t: 200 };
    const north = resizeRect(rect, "n", 150, 260, true);
    expect(north.t).toBe(260);
    expect(north.b).toBe(100);
    expect(north.r - north.l).toBeCloseTo(north.t - north.b, 6);
    // centred on the edge it was dragged by
    expect((north.l + north.r) / 2).toBeCloseTo(150, 6);

    const south = resizeRect(rect, "s", 150, 60, true);
    expect(south.t).toBe(200);
    expect(south.b).toBe(60);
    expect(south.r - south.l).toBeCloseTo(140, 6);
  });

  it("e / w and the corners keep working", () => {
    const rect = { l: 100, b: 100, r: 200, t: 200 };
    const east = resizeRect(rect, "e", 250, 150, true);
    expect(east.r - east.l).toBe(150);
    expect(east.t - east.b).toBeCloseTo(150, 6);
    const corner = resizeRect(rect, "ne", 300, 400, true);
    expect(corner.r - corner.l).toBeCloseTo(corner.t - corner.b, 6);
  });
});
