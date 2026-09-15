import { describe, expect, it } from "vitest";
import type { PageGeom } from "../ipc/types";
import {
  PAGE_GAP,
  computeLayout,
  currentPageAt,
  fitZoomPercent,
  onScreenRange,
  pageRows,
  scrollTopForPage,
  visibleRange,
} from "./layout";

const A4 = (index: number): PageGeom => ({
  index,
  widthPt: 595,
  heightPt: 842,
  rotation: 0,
  crop: { l: 0, b: 0, r: 595, t: 842 },
  label: null,
});

function pages(n: number): PageGeom[] {
  return Array.from({ length: n }, (_, i) => A4(i));
}

const VIEWPORT = { w: 900, h: 800 };

describe("layout", () => {
  it("visibleRange mounts only the ±1.5/2.5-screen window", () => {
    const layout = computeLayout({
      pages: pages(10),
      zoomPercent: 100,
      rotation: 0,
      mode: "continuous",
      viewport: VIEWPORT,
      dpr: 1,
    });
    // row i sits at 24 + i * (842 + 16)
    expect(layout.rows[0].y).toBe(24);
    expect(layout.rows[1].y).toBe(24 + 842 + PAGE_GAP);

    // at the top: the window is [-1200, 2000] -> rows 0..2
    const top = visibleRange(layout, 0, VIEWPORT.h);
    expect([top.first, top.last]).toEqual([0, 2]);
    expect(top.pages).toEqual([0, 1, 2]);

    // four screens down: the window has moved with it, and nothing before it is mounted
    const deep = visibleRange(layout, 3200, VIEWPORT.h);
    expect(deep.first).toBeGreaterThan(0);
    expect(deep.pages).not.toContain(0);
    for (const page of deep.pages) {
      const item = layout.byPage.get(page)!;
      expect(item.y + item.h).toBeGreaterThan(3200 - 1.5 * VIEWPORT.h);
      expect(item.y).toBeLessThan(3200 + 2.5 * VIEWPORT.h);
    }

    // what is actually on screen is a much smaller set — that is what the viewport hint uses
    expect(onScreenRange(layout, 0, VIEWPORT.h).pages).toEqual([0]);
  });

  it("visibleRange survives an empty document and a zero-height viewport", () => {
    const empty = computeLayout({
      pages: [],
      zoomPercent: 100,
      rotation: 0,
      mode: "continuous",
      viewport: VIEWPORT,
      dpr: 1,
    });
    expect(visibleRange(empty, 0, VIEWPORT.h).pages).toEqual([]);

    const layout = computeLayout({
      pages: pages(3),
      zoomPercent: 100,
      rotation: 0,
      mode: "continuous",
      viewport: { w: 900, h: 0 },
      dpr: 1,
    });
    expect(visibleRange(layout, 0, 0).pages).toEqual([0]);
  });

  it("twoPage pairs from an even index and lays the spread out side by side", () => {
    expect(pageRows(5, "two")).toEqual([[0, 1], [2, 3], [4]]);
    expect(pageRows(1, "two")).toEqual([[0]]);
    expect(pageRows(4, "continuous")).toEqual([[0], [1], [2], [3]]);
    expect(pageRows(4, "single", 2)).toEqual([[2]]);

    const layout = computeLayout({
      pages: pages(5),
      zoomPercent: 100,
      rotation: 0,
      mode: "two",
      viewport: { w: 1600, h: 800 },
      dpr: 1,
    });
    expect(layout.rows).toHaveLength(3);
    const [left, right] = layout.items;
    expect(left.row).toBe(0);
    expect(right.row).toBe(0);
    expect(right.x - (left.x + left.w)).toBe(PAGE_GAP);
    expect(left.y).toBe(right.y);
    // the spread is centred in the content
    expect(left.x + left.w + PAGE_GAP + right.w + left.x).toBeCloseTo(layout.width, 0);
    // the last, unpaired page is a row of its own
    expect(layout.rows[2].pages).toEqual([4]);
  });

  it("single mode lays out exactly the current page", () => {
    const layout = computeLayout({
      pages: pages(10),
      zoomPercent: 100,
      rotation: 0,
      mode: "single",
      viewport: VIEWPORT,
      currentPage: 7,
      dpr: 1,
    });
    expect(layout.items).toHaveLength(1);
    expect(layout.items[0].page).toBe(7);
    expect(scrollTopForPage(layout, 7)).toBe(12);
  });

  it("tracks the current page from the scroll offset", () => {
    const layout = computeLayout({
      pages: pages(6),
      zoomPercent: 100,
      rotation: 0,
      mode: "continuous",
      viewport: VIEWPORT,
      dpr: 1,
    });
    expect(currentPageAt(layout, 0, VIEWPORT.h)).toBe(0);
    expect(currentPageAt(layout, scrollTopForPage(layout, 3), VIEWPORT.h)).toBe(3);
    expect(currentPageAt(layout, layout.height, VIEWPORT.h)).toBe(5);
  });

  it("rotation swaps the page box and the fit maths follows", () => {
    const layout = computeLayout({
      pages: pages(2),
      zoomPercent: 100,
      rotation: 90,
      mode: "continuous",
      viewport: VIEWPORT,
      dpr: 1,
    });
    expect(layout.items[0].w).toBe(842);
    expect(layout.items[0].h).toBe(595);

    expect(fitZoomPercent(A4(0), "fit-width", 0, "continuous", VIEWPORT)).toBe(
      Math.round(((900 - 32) / 595) * 100),
    );
    expect(fitZoomPercent(A4(0), "fit-width", 90, "continuous", VIEWPORT)).toBe(
      Math.round(((900 - 32) / 842) * 100),
    );
    // 페이지 맞춤 never exceeds 너비 맞춤
    const fitPage = fitZoomPercent(A4(0), "fit-page", 0, "continuous", VIEWPORT)!;
    const fitWidth = fitZoomPercent(A4(0), "fit-width", 0, "continuous", VIEWPORT)!;
    expect(fitPage).toBeLessThanOrEqual(fitWidth);
    // 두 쪽 halves the available width
    expect(fitZoomPercent(A4(0), "fit-width", 0, "two", VIEWPORT)!).toBeLessThan(fitWidth);
    expect(fitZoomPercent(undefined, "fit-width", 0, "continuous", VIEWPORT)).toBeNull();
  });
});
