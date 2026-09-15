import { describe, expect, it } from "vitest";
import type { PageGeom } from "../ipc/types";
import { computeLayout } from "./layout";
import {
  anchorAt,
  anchoredScroll,
  clampZoom,
  nextZoomStep,
  previousZoomStep,
  scrollForAnchor,
  zoomForWheel,
} from "./zoom";

const pages = (n: number): PageGeom[] =>
  Array.from({ length: n }, (_, index) => ({
    index,
    widthPt: 595,
    heightPt: 842,
    rotation: 0 as const,
    crop: { l: 0, b: 0, r: 595, t: 842 },
    label: null,
  }));

const VIEWPORT = { w: 900, h: 800 };

function layoutAt(zoomPercent: number) {
  return computeLayout({
    pages: pages(12),
    zoomPercent,
    rotation: 0,
    mode: "continuous",
    viewport: VIEWPORT,
    dpr: 1,
  });
}

describe("zoom", () => {
  it("anchor: the page point under the cursor stays under the cursor (F-02, ≤ 2 px)", () => {
    const before = layoutAt(100);
    const scroll = { x: 0, y: 2000 };
    const cursor = { x: 450, y: 300 };

    const anchor = anchorAt(before, scroll, cursor)!;
    expect(anchor).toBeTruthy();

    for (const zoom of [50, 125, 200, 400]) {
      const after = layoutAt(zoom);
      const next = scrollForAnchor(after, anchor, cursor, VIEWPORT);
      const item = after.byPage.get(anchor.page)!;
      const onScreenX = item.x + anchor.fx * item.w - next.x;
      const onScreenY = item.y + anchor.fy * item.h - next.y;
      expect(Math.abs(onScreenX - cursor.x)).toBeLessThanOrEqual(2);
      expect(Math.abs(onScreenY - cursor.y)).toBeLessThanOrEqual(2);
    }
  });

  it("anchor: identifies the page under the cursor and the fraction inside it", () => {
    const layout = layoutAt(100);
    const item = layout.byPage.get(3)!;
    const cursor = { x: 100, y: 200 };
    const scroll = { x: item.x - cursor.x + item.w / 2, y: item.y - cursor.y + item.h / 4 };
    const anchor = anchorAt(layout, scroll, cursor)!;
    expect(anchor.page).toBe(3);
    expect(anchor.fx).toBeCloseTo(0.5, 5);
    expect(anchor.fy).toBeCloseTo(0.25, 5);
  });

  it("anchor: clamps to the scrollable range instead of overscrolling", () => {
    const layout = layoutAt(100);
    const top = scrollForAnchor(layout, { page: 0, fx: 0, fy: 0 }, { x: 900, y: 700 }, VIEWPORT);
    expect(top).toEqual({ x: 0, y: 0 });
    const bottom = scrollForAnchor(layout, { page: 11, fx: 1, fy: 1 }, { x: 0, y: 0 }, VIEWPORT);
    expect(bottom.y).toBeLessThanOrEqual(layout.height - VIEWPORT.h);
  });

  it("anchoredScroll keeps a point fixed on one axis", () => {
    // content point 400 sits 300 px into the viewport; at 2x it must still sit there
    expect(anchoredScroll({ scroll: 100, anchor: 300, originBefore: 0, originAfter: 0, ratio: 2, max: 10000 })).toBe(500);
    expect(anchoredScroll({ scroll: 0, anchor: 0, originBefore: 0, originAfter: 0, ratio: 0.5, max: 10000 })).toBe(0);
    // centring offsets are part of the maths
    expect(anchoredScroll({ scroll: 200, anchor: 100, originBefore: 50, originAfter: 10, ratio: 1, max: 10000 })).toBe(160);
    // and it never overscrolls
    expect(anchoredScroll({ scroll: 0, anchor: 100, originBefore: 50, originAfter: 10, ratio: 1, max: 10000 })).toBe(0);
    expect(anchoredScroll({ scroll: 900, anchor: 0, originBefore: 0, originAfter: 0, ratio: 4, max: 1000 })).toBe(1000);
  });

  it("wheel zoom is exponential, symmetric and clamped to 25–400 %", () => {
    expect(zoomForWheel(100, -10)).toBeGreaterThan(100);
    expect(zoomForWheel(100, 10)).toBeLessThan(100);
    expect(zoomForWheel(100, 0)).toBe(100);
    expect(zoomForWheel(400, -100)).toBe(400);
    expect(zoomForWheel(25, 100)).toBe(25);
    // a line-mode wheel (Windows) moves further than a pixel-mode one
    expect(zoomForWheel(100, -1, 1)).toBeGreaterThan(zoomForWheel(100, -1, 0));
  });

  it("steps through the status-bar ladder", () => {
    expect(nextZoomStep(100)).toBe(125);
    expect(nextZoomStep(118)).toBe(125);
    expect(previousZoomStep(100)).toBe(75);
    expect(previousZoomStep(25)).toBe(25);
    expect(nextZoomStep(400)).toBe(400);
    expect(clampZoom(9999)).toBe(400);
    expect(clampZoom(1)).toBe(25);
  });
});
