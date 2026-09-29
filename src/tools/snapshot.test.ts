/**
 * 스냅샷 (V1, v0.3): the marquee ↔ page-rectangle maths, the render scale and its cap, the view
 * rotation of the pixels, and the clipboard write (a PNG blob, handed over synchronously) with its
 * PNG로 저장… fallback — against the mock adapter.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import * as api from "../ipc/api";
import { mock, mockSnapshots } from "../ipc/mock";
import { makePageLayerContext } from "../viewer/PageShell";
import type { PageGeom, Rect, Rotation } from "../ipc/types";
import { useToastStore } from "../app/toastStore";
import { useAppStore } from "../store/appStore";
import { useDocStore } from "../store/docStore";
import {
  captureSnapshot, copySnapshot, marqueeToPageRect, pageRectToBox, rotatePixels, snapshotScale, SNAPSHOT_MAX_PX,
  ENGINE_HARD_MAX_PX,
} from "./snapshot";

const A4: PageGeom = { index: 0, widthPt: 595, heightPt: 842, rotation: 0, crop: { l: 0, b: 0, r: 595, t: 842 }, label: null };

function ctxFor(rotation: Rotation, zoomPercent = 150) {
  const swap = rotation === 90 || rotation === 270;
  const s = zoomPercent / 100;
  return makePageLayerContext({
    docId: "d1", docGeneration: 1, page: A4, rotation, zoomPercent,
    width: (swap ? A4.heightPt : A4.widthPt) * s, height: (swap ? A4.widthPt : A4.heightPt) * s,
  });
}

/** A fake async clipboard that keeps what it was given. */
class FakeClipboardItem {
  constructor(readonly items: Record<string, Promise<Blob> | Blob>) {}
}

describe("snapshot geometry", () => {
  it("a marquee maps to the page rectangle under it, at every view rotation (round trip)", () => {
    const rect: Rect = { l: 100, b: 500, r: 300, t: 700 };
    for (const rotation of [0, 90, 180, 270] as Rotation[]) {
      const ctx = ctxFor(rotation);
      const box = pageRectToBox(rect, ctx.toDevice);
      // the marquee may be dragged from any corner
      const back = marqueeToPageRect([box.x + box.w, box.y + box.h], [box.x, box.y], ctx.toPage, A4.crop)!;
      expect(back.l).toBeCloseTo(rect.l, 3);
      expect(back.b).toBeCloseTo(rect.b, 3);
      expect(back.r).toBeCloseTo(rect.r, 3);
      expect(back.t).toBeCloseTo(rect.t, 3);
    }
    // at 0°, 150 %: the top-left 30 × 15 CSS px of the page
    expect(marqueeToPageRect([0, 0], [30, 15], ctxFor(0).toPage, A4.crop)).toEqual({ l: 0, b: 832, r: 20, t: 842 });
  });

  it("clips to the crop box, and nothing on the page is no rectangle", () => {
    const ctx = ctxFor(0, 100);
    expect(marqueeToPageRect([-50, -50], [60, 20], ctx.toPage, A4.crop)).toEqual({ l: 0, b: 822, r: 60, t: 842 });
    expect(marqueeToPageRect([-50, -50], [-10, -10], ctx.toPage, A4.crop)).toBeNull();
  });

  it("renders at 2× the on-screen device scale, capped at 8 192 px", () => {
    const small: Rect = { l: 0, b: 0, r: 200, t: 100 };
    expect(snapshotScale(150, 2, small)).toBeCloseTo(6, 6);
    expect(snapshotScale(100, 1, small)).toBeCloseTo(2, 6);
    // a narrow strip: the edge cap binds (8 192 × ~973 px is well under the area cap)
    const strip: Rect = { l: 0, b: 0, r: 100, t: 842 };
    expect(snapshotScale(400, 2, strip) * 842).toBeCloseTo(SNAPSHOT_MAX_PX, 3);
  });

  // V1 verification round 2: the edge cap alone let 8 000 × 5 320 (42.6 M px) through, and the
  // engine's render_page_raw refuses anything over geometry::HARD_MAX_PX = 40 M px.
  it.each([
    { zoom: 327, dpr: 2, cssW: 2000, cssH: 1330 }, // fit-width letter page on a 'looks like 2560' dpr-2 screen
    { zoom: 400, dpr: 2, cssW: 1700, cssH: 1700 },
    { zoom: 400, dpr: 2, cssW: 2380, cssH: 3368 }, // a whole A4 page at 400 %
    { zoom: 1600, dpr: 3, cssW: 3000, cssH: 3000 },
  ])("stays under the engine's pixel ceiling: zoom $zoom %, dpr $dpr, $cssW × $cssH CSS px", ({ zoom, dpr, cssW, cssH }) => {
    const k = zoom / 100;
    const rect: Rect = { l: 0.3, r: 0.3 + cssW / k, t: 792, b: 792 - cssH / k };
    const s = snapshotScale(zoom, dpr, rect);
    // the engine rounds each device-space edge on its own: allow a pixel of growth per edge
    const w = Math.ceil((rect.r - rect.l) * s) + 1;
    const h = Math.ceil((rect.t - rect.b) * s) + 1;
    expect(w).toBeLessThanOrEqual(SNAPSHOT_MAX_PX + 1);
    expect(h).toBeLessThanOrEqual(SNAPSHOT_MAX_PX + 1);
    expect(w * h).toBeLessThanOrEqual(ENGINE_HARD_MAX_PX);
    // still as sharp as the ceiling allows: well over the on-screen device resolution
    expect(w * h).toBeGreaterThan(cssW * cssH * Math.min(dpr, 2));
  });

  it("turns the pixels clockwise by the view rotation", () => {
    // 2 × 1: red, green
    const px = new Uint8ClampedArray([255, 0, 0, 255, 0, 255, 0, 255]);
    const r90 = rotatePixels(2, 1, 8, px, 90);
    expect([r90.width, r90.height]).toEqual([1, 2]);
    expect([...r90.pixels.slice(0, 4)]).toEqual([255, 0, 0, 255]); // red on top
    const r180 = rotatePixels(2, 1, 8, px, 180);
    expect([...r180.pixels.slice(0, 4)]).toEqual([0, 255, 0, 255]); // green first
    const r270 = rotatePixels(2, 1, 8, px, 270);
    expect([...r270.pixels.slice(0, 4)]).toEqual([0, 255, 0, 255]); // green on top
  });
});

describe("snapshot clipboard", () => {
  let written: FakeClipboardItem[][] = [];

  beforeEach(async () => {
    written = [];
    useToastStore.setState({ toasts: [] });
    await useDocStore.getState().open("/Users/veri/Documents/SeePDF-샘플.pdf");
    vi.stubGlobal("ClipboardItem", FakeClipboardItem);
  });

  afterEach(() => {
    vi.unstubAllGlobals();
    vi.restoreAllMocks();
  });

  function stubClipboard(write: (items: FakeClipboardItem[]) => Promise<void>) {
    Object.defineProperty(navigator, "clipboard", { configurable: true, value: { write } });
  }

  it("copies the region as a PNG blob, rendered at 2× through render_page_raw", async () => {
    const info = useDocStore.getState().info!;
    const raw = vi.spyOn(mock, "renderPageRaw");
    stubClipboard(async (items) => {
      written.push(items);
    });
    const rect: Rect = { l: 72, b: 600, r: 272, t: 700 };
    const done = copySnapshot({ docId: info.docId, page: 0, rect, zoomPercent: 125, rotation: 0, dpr: 2 });
    // the clipboard was written synchronously — inside the gesture
    expect(written).toHaveLength(1);
    expect(await done).toBe(true);
    expect(raw).toHaveBeenCalledWith({ docId: info.docId, page: 0, scale: 5, rect });
    const blob = await (written[0][0].items["image/png"] as Promise<Blob>);
    expect(blob.type).toBe("image/png");
    const bytes = new Uint8Array(await blob.arrayBuffer());
    expect([...bytes.slice(0, 4)]).toEqual([0x89, 0x50, 0x4e, 0x47]);
    expect(useToastStore.getState().toasts.at(-1)?.messageKey).toBe("snapshot.copied");
  });

  it("a refused clipboard offers PNG로 저장…, which writes the file", async () => {
    const info = useDocStore.getState().info!;
    stubClipboard(() => Promise.reject(new Error("NotAllowedError")));
    const save = vi.spyOn(api, "saveSnapshotPng");
    const ok = await copySnapshot({
      docId: info.docId, page: 1, rect: { l: 0, b: 0, r: 100, t: 100 }, zoomPercent: 100, rotation: 90, dpr: 1, docName: "계약서.pdf",
    });
    expect(ok).toBe(false);
    const t = useToastStore.getState().toasts.at(-1)!;
    expect(t.messageKey).toBe("snapshot.copyFailed");
    expect(t.actions?.[0].labelKey).toBe("snapshot.saveAs");
    t.actions![0].onSelect();
    await vi.waitFor(() => expect(save).toHaveBeenCalledTimes(1));
    expect(save.mock.calls[0][0].path).toMatch(/계약서-2쪽\.png$/);
    expect(mockSnapshots.size).toBe(1);
  });

  it("captureSnapshot hands the pointer back to the mode's own tool", async () => {
    const info = useDocStore.getState().info!;
    stubClipboard(async () => undefined);
    useAppStore.setState({ mode: "annotate", tool: "snapshot" });
    await captureSnapshot({ docId: info.docId, page: 0, rect: { l: 0, b: 0, r: 50, t: 50 }, zoomPercent: 100, rotation: 0, dpr: 1 });
    expect(useAppStore.getState().tool).toBe("select");
    useAppStore.setState({ mode: "form", tool: "snapshot" });
    await captureSnapshot({ docId: info.docId, page: 0, rect: { l: 0, b: 0, r: 50, t: 50 }, zoomPercent: 100, rotation: 0, dpr: 1 });
    expect(useAppStore.getState().tool).toBe("fillForm");
  });
});
