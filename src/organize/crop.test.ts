import { describe, expect, it } from "vitest";
import {
  boxFromMargins, boxFromPoints, clampBox, contentBounds, detectCropBox, dragBox, fullBox, isFullPage, marginsFromBox,
} from "./crop";
import { marginsToUser, mmToPt, ptToMm, resizedVisualSize, visualSize, visualToUserSize } from "./cropGeometry";
import { initialResizeForm, resizeTarget, sizeLabelMm } from "../dialogs/resize";
import type { PageGeom } from "../ipc/types";

const W = 600;
const H = 800;

function geom(rotation: PageGeom["rotation"], w = 612, h = 792): PageGeom {
  return { index: 0, widthPt: rotation % 180 ? h : w, heightPt: rotation % 180 ? w : h, rotation, crop: { l: 0, b: 0, r: w, t: h }, label: null };
}

describe("organize.crop box", () => {
  it("margins and boxes round-trip", () => {
    const m = { top: 10, right: 20, bottom: 30, left: 40 };
    const box = boxFromMargins(m, W, H);
    expect(box).toEqual({ x: 40, y: 10, w: 540, h: 760 });
    expect(marginsFromBox(box, W, H)).toEqual(m);
    expect(isFullPage(marginsFromBox(fullBox(W, H), W, H))).toBe(true);
    expect(isFullPage(m)).toBe(false);
  });

  it("edges stop at the page and at the minimum size, never flipping", () => {
    const start = { x: 100, y: 100, w: 200, h: 300 };
    expect(dragBox(start, "se", 50, 20, W, H)).toEqual({ x: 100, y: 100, w: 250, h: 320 });
    expect(dragBox(start, "nw", -500, -500, W, H)).toEqual({ x: 0, y: 0, w: 300, h: 400 });
    // dragging the left edge far right stops MIN_BOX_PT (18) before the right edge
    expect(dragBox(start, "w", 1000, 0, W, H)).toEqual({ x: 282, y: 100, w: 18, h: 300 });
    expect(dragBox(start, "e", 1000, 0, W, H).x + dragBox(start, "e", 1000, 0, W, H).w).toBe(W);
    expect(dragBox(start, "n", 0, 50, W, H)).toEqual({ x: 100, y: 150, w: 200, h: 250 });
    expect(dragBox(start, "s", 0, -1000, W, H).h).toBe(18);
  });

  it("move slides the box and keeps it on the page", () => {
    const start = { x: 100, y: 100, w: 200, h: 300 };
    expect(dragBox(start, "move", 50, -20, W, H)).toEqual({ x: 150, y: 80, w: 200, h: 300 });
    expect(dragBox(start, "move", 1000, 1000, W, H)).toEqual({ x: 400, y: 500, w: 200, h: 300 });
    expect(clampBox({ x: -5, y: 790, w: 10, h: 40 }, W, H)).toEqual({ x: 0, y: 760, w: 18, h: 40 });
    expect(boxFromPoints(300, 400, 100, 50, W, H)).toEqual({ x: 100, y: 50, w: 200, h: 350 });
    expect(boxFromPoints(-20, -20, 700, 900, W, H)).toEqual({ x: 0, y: 0, w: W, h: H });
  });
});

describe("organize.crop auto-detect", () => {
  /** A white RGBA page with one dark block at (x0, y0) … (x1, y1) px, exclusive. */
  function page(w: number, h: number, block: [number, number, number, number] | null) {
    const px = new Uint8ClampedArray(w * h * 4).fill(255);
    if (block) {
      const [x0, y0, x1, y1] = block;
      for (let y = y0; y < y1; y++) for (let x = x0; x < x1; x++) px.set([20, 20, 20, 255], (y * w + x) * 4);
    }
    return { width: w, height: h, stride: w * 4, pixels: px };
  }

  it("finds the ink and ignores paper and transparency", () => {
    const raw = page(100, 80, [10, 20, 60, 50]);
    expect(contentBounds(raw.pixels, 100, 80)).toEqual({ x0: 10, y0: 20, x1: 60, y1: 50 });
    expect(contentBounds(page(40, 40, null).pixels, 40, 40)).toBeNull();
    const clear = new Uint8ClampedArray(16 * 16 * 4); // all transparent black
    expect(contentBounds(clear, 16, 16)).toBeNull();
  });

  it("turns render pixels into a padded crop box in points", () => {
    // 2 px per pt: a 300 × 400 pt page rendered at 600 × 800
    const raw = page(600, 800, [100, 200, 500, 600]);
    const box = detectCropBox(raw, 2, 300, 400, 6);
    expect(box).toEqual({ x: 44, y: 94, w: 212, h: 212 });
    // padding never pushes past the page
    const edge = detectCropBox(page(600, 800, [0, 0, 600, 20]), 2, 300, 400, 6)!;
    expect(edge.x).toBe(0);
    expect(edge.y).toBe(0);
    expect(edge.w).toBe(300);
    expect(detectCropBox(page(60, 80, null), 2, 30, 40)).toBeNull();
  });
});

describe("organize.cropGeometry", () => {
  it("margins map to the displayed edges for every rotation (engine parity)", () => {
    const crop = { l: 0, b: 0, r: 600, t: 800 };
    const m = { top: 10, right: 20, bottom: 30, left: 40 };
    expect(marginsToUser(0, crop, m)).toEqual({ l: 40, b: 30, r: 580, t: 790 });
    // the same numbers as `boxes::tests::margins_follow_the_displayed_edges`
    expect(marginsToUser(90, crop, m)).toEqual({ l: 10, b: 40, r: 570, t: 780 });
    expect(marginsToUser(180, crop, m)).toEqual({ l: 20, b: 10, r: 560, t: 770 });
    expect(marginsToUser(270, crop, m)).toEqual({ l: 30, b: 20, r: 590, t: 760 });
    expect(marginsToUser(0, crop, { top: 400, right: 0, bottom: 400, left: 0 })).toBeNull();
  });

  it("named sizes keep the orientation; explicit sizes are as seen", () => {
    expect(resizedVisualSize(geom(0), "A4")).toEqual({ w: 595.28, h: 841.89 });
    expect(resizedVisualSize(geom(90), "A4")).toEqual({ w: 841.89, h: 595.28 });
    expect(resizedVisualSize(geom(0, 792, 612), "Letter")).toEqual({ w: 792, h: 612 });
    expect(resizedVisualSize(geom(0), { w: 300, h: 200 })).toEqual({ w: 300, h: 200 });
    expect(visualToUserSize(90, 300, 200)).toEqual({ w: 200, h: 300 });
    expect(visualSize(geom(90))).toEqual({ w: 792, h: 612 });
  });

  it("millimetres and points", () => {
    expect(ptToMm(595.28)).toBeCloseTo(210, 1);
    expect(mmToPt(297)).toBeCloseTo(841.89, 1);
  });
});

describe("dialogs.resize", () => {
  it("named sizes pass through, custom sizes are mm → pt and validated", () => {
    const form = initialResizeForm(geom(0));
    expect(form).toMatchObject({ size: "A4", widthMm: "216", heightMm: "279", mode: "scaleContent" });
    expect(resizeTarget(form)).toBe("A4");
    expect(resizeTarget({ ...form, size: "Letter" })).toBe("Letter");
    expect(resizeTarget({ ...form, size: "custom", widthMm: "210", heightMm: "297" })).toEqual({ w: 595.28, h: 841.89 });
    expect(resizeTarget({ ...form, size: "custom", widthMm: "0", heightMm: "297" })).toBeNull();
    expect(resizeTarget({ ...form, size: "custom", widthMm: "abc", heightMm: "297" })).toBeNull();
    expect(resizeTarget({ ...form, size: "custom", widthMm: "6000", heightMm: "297" })).toBeNull();
    expect(sizeLabelMm(geom(90))).toEqual({ w: "279", h: "216" });
  });
});
