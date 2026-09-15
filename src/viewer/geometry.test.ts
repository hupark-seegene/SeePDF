import { describe, expect, it } from "vitest";
import type { PageGeom, Rotation } from "../ipc/types";
import {
  MAX_SCALE_KEY,
  applyMat,
  deviceToPage,
  displaySizePt,
  pageBoxCss,
  pagePixels,
  pageToDevice,
  placeholderScaleKey,
  rectToBox,
  scaleKeyFor,
  shouldTile,
  tileGridFor,
  tilePriority,
  tileRect,
} from "./geometry";

function geom(w: number, h: number, rotation: Rotation, crop = { l: 0, b: 0, r: 612, t: 792 }): PageGeom {
  return { index: 0, widthPt: w, heightPt: h, rotation, crop, label: null };
}

/** `rotation.pdf` as the engine reports it (render spike §1, `cargo test render_rotation_sizes`). */
const ROT_P0 = geom(612, 792, 0);
const ROT_P1 = geom(792, 612, 90); // /Rotate 90 — pdfium already swapped the display size

describe("geometry", () => {
  it("rotations: page pixels match the Rust table for every view rotation", () => {
    // `geometry::page_pixels` (render/geometry.rs) — the same four rows, both sides.
    expect(pagePixels(ROT_P0, 0, 1)).toEqual({ w: 612, h: 792 });
    expect(pagePixels(ROT_P0, 90, 1)).toEqual({ w: 792, h: 612 });
    expect(pagePixels(ROT_P0, 180, 1)).toEqual({ w: 612, h: 792 });
    expect(pagePixels(ROT_P0, 270, 1)).toEqual({ w: 792, h: 612 });
    expect(pagePixels(ROT_P0, 0, 2)).toEqual({ w: 1224, h: 1584 });

    // rotation.pdf page 1: 792x612 pt already, and a 90 degree view rotation swaps it back.
    expect(displaySizePt(ROT_P1, 0)).toEqual({ w: 792, h: 612 });
    expect(pagePixels(ROT_P1, 0, 1)).toEqual({ w: 792, h: 612 });
    expect(pagePixels(ROT_P1, 90, 1)).toEqual({ w: 612, h: 792 });
  });

  it("rotations: the affine maps the crop box corners in all four orientations", () => {
    const s = 2;
    // 0 degrees — bottom-left of the crop box is the bottom-left of the image.
    const m0 = pageToDevice(ROT_P0, 0, s);
    expect(applyMat(m0, 0, 0)).toEqual([0, 1584]);
    expect(applyMat(m0, 612, 792)).toEqual([1224, 0]);

    // 90 degrees clockwise: (l, b) -> top-left, (l, t) -> top-right.
    const m90 = pageToDevice(ROT_P0, 90, s);
    expect(applyMat(m90, 0, 0)).toEqual([0, 0]);
    expect(applyMat(m90, 0, 792)).toEqual([1584, 0]);
    expect(applyMat(m90, 612, 0)).toEqual([0, 1224]);

    const m180 = pageToDevice(ROT_P0, 180, s);
    expect(applyMat(m180, 612, 792)).toEqual([0, 1584]);
    expect(applyMat(m180, 0, 0)).toEqual([1224, 0]);

    // 270 degrees: the crop box's top-right corner becomes the image origin.
    const m270 = pageToDevice(ROT_P0, 270, s);
    expect(applyMat(m270, 612, 792)).toEqual([0, 0]);
    expect(applyMat(m270, 0, 0)).toEqual([1584, 1224]);
    expect(applyMat(m270, 612, 0)).toEqual([1584, 0]);
  });

  it("rotations: the page's intrinsic /Rotate and the view rotation add up", () => {
    // page /Rotate 90 + view 270 == no rotation at all
    const m = pageToDevice(ROT_P1, 270, 1);
    expect(m).toEqual(pageToDevice(ROT_P0, 0, 1));
  });

  it("rotations: matches the text spike's device rectangles", () => {
    // text spike §2: "Trace-based" [80.52, 696.92, 173.82, 713.04] at 2x -> (161.03, 157.91) 186.61x32.24
    const box = rectToBox({ l: 80.52, b: 696.92, r: 173.82, t: 713.04 }, pageToDevice(ROT_P0, 0, 2));
    expect(box.x).toBeCloseTo(161.04, 2);
    expect(box.y).toBeCloseTo(157.92, 2);
    expect(box.w).toBeCloseTo(186.6, 2);
    expect(box.h).toBeCloseTo(32.24, 2);

    // text spike §2, rotation.pdf page 1 at 1.5x: first char -> (108, 135.5) 11x19.9, pdfium (108,136)-(119,155)
    const rotated = rectToBox({ l: 90.35, b: 72.0, r: 103.63, t: 79.33 }, pageToDevice(ROT_P1, 0, 1.5));
    expect(rotated.x).toBeCloseTo(108, 1);
    expect(rotated.y).toBeCloseTo(135.5, 1);
    expect(rotated.w).toBeCloseTo(11.0, 1);
    expect(rotated.h).toBeCloseTo(19.9, 1);
  });

  it("inverts the affine so a device point round-trips to page space", () => {
    for (const rotation of [0, 90, 180, 270] as Rotation[]) {
      const m = pageToDevice(ROT_P0, rotation, 1.5);
      const inverse = deviceToPage(m);
      const [dx, dy] = applyMat(m, 137.5, 411.25);
      const [px, py] = applyMat(inverse, dx, dy);
      expect(px).toBeCloseTo(137.5, 3);
      expect(py).toBeCloseTo(411.25, 3);
    }
  });

  it("switches to tiles exactly where the engine does", () => {
    expect(shouldTile(1, 612, 792)).toBe(false);
    expect(shouldTile(2, 1224, 1584)).toBe(false);
    expect(shouldTile(2.01, 1230, 1592)).toBe(true);
    // a huge page tiles even at 1x: 2.5 Mpx is the other half of the rule
    expect(shouldTile(1, 2000, 2000)).toBe(true);
  });

  it("clips the tile grid at the page edge (the Rust `tiles_clip_at_the_page_edge` table)", () => {
    expect(tileGridFor(1300, 1700)).toEqual({ cols: 3, rows: 4 });
    expect(tileRect(1300, 1700, 0, 0)).toEqual({ x: 0, y: 0, w: 512, h: 512 });
    expect(tileRect(1300, 1700, 2, 3)).toEqual({ x: 1024, y: 1536, w: 276, h: 164 });
    expect(tileRect(1300, 1700, 3, 0)).toBeNull();
  });

  it("orders tiles centre-out by Chebyshev distance", () => {
    expect(tilePriority(2, 2, 2, 2)).toBe(0);
    expect(tilePriority(3, 2, 2, 2)).toBe(1);
    expect(tilePriority(0, 4, 2, 2)).toBe(2);
  });

  it("quantises the scale key and caps it at the engine's ceiling", () => {
    expect(scaleKeyFor(100, 2)).toBe(200);
    expect(scaleKeyFor(118, 2)).toBe(236);
    expect(scaleKeyFor(400, 3)).toBe(MAX_SCALE_KEY);
    // the CSS box is the device bitmap divided by the density, so a tile mosaic covers it exactly
    expect(pageBoxCss(ROT_P0, 0, 200, 2)).toEqual({ w: 612, h: 792 });
  });

  it("keeps the placeholder at 640 px of page width", () => {
    expect(placeholderScaleKey(ROT_P0, 0)).toBe(100); // 612 pt wide -> s = 1
    expect(placeholderScaleKey(geom(1224, 1584, 0), 0)).toBe(52); // 640 / 1224
  });
});
