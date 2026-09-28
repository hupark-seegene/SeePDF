import { describe, expect, it } from "vitest";
import type { PageGeom, PageObject, Rotation } from "../ipc/types";
import { makePageLayerContext } from "../viewer";
import {
  deltaToPage, fontStack, hitObject, imageRectAt, resizeRect, scaleArgs, textRectFor, cornerPoint,
} from "./geometry";

const PAGE: PageGeom = { index: 0, widthPt: 600, heightPt: 800, rotation: 0, crop: { l: 0, b: 0, r: 600, t: 800 }, label: null };

function ctxFor(rotation: Rotation, zoomPercent = 150, page: PageGeom = PAGE) {
  return makePageLayerContext({ docId: "d1", docGeneration: 1, page, rotation, zoomPercent, width: 0, height: 0 });
}

function obj(objectId: number, l: number, b: number, r: number, t: number, type: PageObject["type"] = "text"): PageObject {
  return { objectId, type, rect: { l, b, r, t }, matrix: [1, 0, 0, 1, l, b], ours: false, editable: "full" };
}

describe("edit geometry", () => {
  it("round-trips page points through CSS px under every rotation, and deltas follow", () => {
    for (const rotation of [0, 90, 180, 270] as Rotation[]) {
      const ctx = ctxFor(rotation);
      const [x, y] = ctx.toDevice(123, 456);
      const [px, py] = ctx.toPage(x, y);
      expect(px).toBeCloseTo(123, 6);
      expect(py).toBeCloseTo(456, 6);
      // a CSS drag from (x, y) to (x + 30, y - 12) is the page delta between the two points
      const [qx, qy] = ctx.toPage(x + 30, y - 12);
      const [dx, dy] = deltaToPage(ctx.inverse, 30, -12);
      expect(dx).toBeCloseTo(qx - px, 6);
      expect(dy).toBeCloseTo(qy - py, 6);
    }
  });

  it("maps a CSS drag to a y-up page delta at 0°", () => {
    const [dx, dy] = deltaToPage(ctxFor(0, 200).inverse, 20, 10);
    expect(dx).toBeCloseTo(10);
    expect(dy).toBeCloseTo(-5);
  });

  it("hit-tests the smallest object under the point", () => {
    const objects = [obj(0, 0, 0, 600, 800, "path"), obj(1, 100, 100, 200, 120), obj(2, 90, 90, 400, 300, "image")];
    expect(hitObject(objects, 150, 110)?.objectId).toBe(1);
    expect(hitObject(objects, 300, 200)?.objectId).toBe(2);
    expect(hitObject(objects, 10, 10)?.objectId).toBe(0);
    expect(hitObject([], 10, 10)).toBeNull();
  });

  it("resizes from a corner with the opposite corner fixed, and turns it into anchored scale + translate", () => {
    const r = { l: 100, b: 100, r: 200, t: 150 };
    const grown = resizeRect(r, "rt", 50, 25);
    expect(grown).toEqual({ l: 100, b: 100, r: 250, t: 175 });
    const fromLeft = resizeRect(r, "lb", -20, -10);
    expect(fromLeft).toEqual({ l: 80, b: 90, r: 200, t: 150 });
    const args = scaleArgs(r, fromLeft);
    expect(args.scale[0]).toBeCloseTo(1.2);
    expect(args.scale[1]).toBeCloseTo(1.2);
    expect(args.translate).toEqual([-20, -10]);
    // keep aspect: the larger relative change wins
    const kept = resizeRect(r, "rb", 100, 0, true);
    expect(kept.r - kept.l).toBeCloseTo(200);
    expect(kept.t - kept.b).toBeCloseTo(100);
    expect(kept.t).toBe(150);
    expect(cornerPoint(r, "lt")).toEqual([100, 150]);
  });

  it("sizes typed text and a clicked image box on the page", () => {
    const rect = textRectFor([72, 700], "ab\n가나다", 10);
    expect(rect.t).toBe(700);
    expect(rect.b).toBeCloseTo(700 - 24);
    expect(rect.r - rect.l).toBeGreaterThanOrEqual(30);
    const img = imageRectAt([500, 100], PAGE);
    expect(img.r).toBeLessThanOrEqual(600);
    expect(img.b).toBeGreaterThanOrEqual(0);
    expect(img.r - img.l).toBe(240);
  });

  it("approximates font families", () => {
    expect(fontStack("ABCDEF+TimesNewRomanPSMT")).toMatch(/serif$/);
    expect(fontStack("Helvetica")).toMatch(/sans-serif$/);
    expect(fontStack("NotoSansKR-Regular")).toMatch(/Apple SD Gothic Neo/);
    expect(fontStack("CourierNew")).toMatch(/monospace$/);
    expect(fontStack("HYMyeongJo")).toMatch(/serif$/);
  });
});
