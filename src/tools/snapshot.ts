/**
 * 스냅샷 (V1, UI_SPEC §6 / §12 / §14.6): drag a marquee over a page — in any mode — and the region
 * goes to the clipboard as a PNG, ready to paste into KakaoTalk, a messenger or a slide.
 *
 * The viewer owns the drag (`Scroller.tsx`: the marquee works over every layer and every mode, so it
 * cannot live on the 주석 tool surface) and calls {@link captureSnapshot} on release. The region is
 * rendered by the engine through `render_page_raw` (IPC_CONTRACT §5 / §10.2 — it exists for exactly
 * this) at **2× the device scale on screen**, capped so neither edge passes 8 192 px and the area stays
 * under the engine's 40 M px ceiling, then turned by
 * the view rotation (the engine renders it unrotated), encoded as PNG and written with
 * `navigator.clipboard.write(ClipboardItem{'image/png'})`. The clipboard call is made synchronously,
 * inside the pointer-up gesture, with the PNG as a *promise* — WebKit refuses a clipboard write made
 * after an `await`. When the clipboard refuses anyway, a toast offers **PNG로 저장…** instead.
 *
 * The geometry is pure and unit-tested; nothing here imports React.
 */
import * as api from "../ipc/api";
import { encodePng } from "../ipc/png";
import type { DocId, PageIndex, Point, Rect, Rotation } from "../ipc/types";
import { toast } from "../app/toastStore";
import { useAppStore } from "../store/appStore";
import { useDocStore } from "../store/docStore";
import { copyForbidden, reasonKey } from "../app/permissions";
import type { ToolModule } from "./ToolController";

/** The snapshot is rendered at this multiple of the device scale on screen… */
export const SNAPSHOT_OVERSAMPLE = 2;
/** …with neither edge longer than this many pixels… */
export const SNAPSHOT_MAX_PX = 8192;
/**
 * …and no more pixels in all than this. The engine refuses a `render_page_raw` region over
 * `geometry::HARD_MAX_PX` = 40 000 000 px (src-tauri/src/engine/render/geometry.rs) — the edge cap
 * alone still allows 8 192 × 8 192 ≈ 67 M px — so the area is capped a little below it, leaving room
 * for the engine's per-edge rounding.
 */
export const ENGINE_HARD_MAX_PX = 40_000_000;
export const SNAPSHOT_MAX_AREA_PX = 36_000_000;
/** A marquee smaller than this (CSS px, either edge) is a click, not a snapshot. */
export const SNAPSHOT_MIN_CSS_PX = 4;

/**
 * The page rectangle a marquee covers: its two corners in CSS px inside the page box, mapped to PDF
 * user space with the page's `toPage` (any view rotation — all four corners are mapped), normalised
 * and clipped to the crop box. `null` when nothing of the page is inside.
 */
export function marqueeToPageRect(
  a: Point,
  b: Point,
  toPage: (x: number, y: number) => Point,
  crop: Rect,
): Rect | null {
  const corners = [toPage(a[0], a[1]), toPage(b[0], a[1]), toPage(a[0], b[1]), toPage(b[0], b[1])];
  const xs = corners.map((p) => p[0]);
  const ys = corners.map((p) => p[1]);
  const l = Math.max(crop.l, Math.min(...xs));
  const r = Math.min(crop.r, Math.max(...xs));
  const bottom = Math.max(crop.b, Math.min(...ys));
  const t = Math.min(crop.t, Math.max(...ys));
  if (r - l < 0.5 || t - bottom < 0.5) return null;
  return { l, b: bottom, r, t };
}

/** The inverse, for the marquee a rectangle occupies on screen (tests, and the page ↔ screen check). */
export function pageRectToBox(rect: Rect, toDevice: (x: number, y: number) => Point): { x: number; y: number; w: number; h: number } {
  const corners = [toDevice(rect.l, rect.b), toDevice(rect.r, rect.b), toDevice(rect.l, rect.t), toDevice(rect.r, rect.t)];
  const xs = corners.map((p) => p[0]);
  const ys = corners.map((p) => p[1]);
  const x = Math.min(...xs);
  const y = Math.min(...ys);
  return { x, y, w: Math.max(...xs) - x, h: Math.max(...ys) - y };
}

/**
 * Render scale for a region: 2× the on-screen device scale, capped so neither edge passes
 * {@link SNAPSHOT_MAX_PX} and the pixel count stays under {@link SNAPSHOT_MAX_AREA_PX}.
 */
export function snapshotScale(zoomPercent: number, dpr: number, rect: Rect): number {
  const wanted = SNAPSHOT_OVERSAMPLE * (zoomPercent / 100) * Math.max(1, dpr);
  const wPt = Math.max(rect.r - rect.l, 1);
  const hPt = Math.max(rect.t - rect.b, 1);
  const byEdge = SNAPSHOT_MAX_PX / Math.max(wPt, hPt);
  const byArea = Math.sqrt(SNAPSHOT_MAX_AREA_PX / (wPt * hPt));
  return Math.max(0.05, Math.min(wanted, byEdge, byArea));
}

/** RGBA pixels (top-left origin, `stride` bytes per row) turned clockwise by the view rotation. */
export function rotatePixels(
  width: number,
  height: number,
  stride: number,
  pixels: Uint8ClampedArray | Uint8Array,
  rotation: Rotation,
): { width: number; height: number; pixels: Uint8ClampedArray } {
  const swap = rotation === 90 || rotation === 270;
  const w = swap ? height : width;
  const h = swap ? width : height;
  const out = new Uint8ClampedArray(w * h * 4);
  for (let y = 0; y < height; y++) {
    for (let x = 0; x < width; x++) {
      let tx = x;
      let ty = y;
      if (rotation === 90) {
        tx = height - 1 - y;
        ty = x;
      } else if (rotation === 180) {
        tx = width - 1 - x;
        ty = height - 1 - y;
      } else if (rotation === 270) {
        tx = y;
        ty = width - 1 - x;
      }
      const s = y * stride + x * 4;
      const d = (ty * w + tx) * 4;
      out[d] = pixels[s];
      out[d + 1] = pixels[s + 1];
      out[d + 2] = pixels[s + 2];
      out[d + 3] = pixels[s + 3];
    }
  }
  return { width: w, height: h, pixels: out };
}

/** PNG bytes: a compressed canvas encode where there is one, the tiny stored encoder otherwise. */
async function encode(width: number, height: number, rgba: Uint8ClampedArray): Promise<Blob> {
  if (typeof OffscreenCanvas !== "undefined" && typeof ImageData !== "undefined") {
    try {
      const canvas = new OffscreenCanvas(width, height);
      const ctx = canvas.getContext("2d");
      if (ctx) {
        ctx.putImageData(new ImageData(new Uint8ClampedArray(rgba), width, height), 0, 0);
        return await canvas.convertToBlob({ type: "image/png" });
      }
    } catch {
      /* fall through to the JS encoder */
    }
  }
  const png = encodePng(width, height, new Uint8Array(rgba.buffer, rgba.byteOffset, rgba.byteLength));
  return new Blob([png as BlobPart], { type: "image/png" });
}

export interface SnapshotRequest {
  docId: DocId;
  page: PageIndex;
  rect: Rect;
  /** the pane's zoom and view rotation, and the display density */
  zoomPercent: number;
  rotation: Rotation;
  dpr: number;
  /** for 저장's default file name */
  docName?: string;
}

/** Renders the region and encodes it (no clipboard). */
export async function snapshotPng(req: SnapshotRequest): Promise<Blob> {
  const scale = snapshotScale(req.zoomPercent, req.dpr, req.rect);
  const raw = await api.renderPageRaw({ docId: req.docId, page: req.page, scale, rect: req.rect });
  const turned = rotatePixels(raw.width, raw.height, raw.stride, raw.pixels, req.rotation);
  return encode(turned.width, turned.height, turned.pixels);
}

function bytesOf(blob: Blob): Promise<Uint8Array> {
  if (typeof blob.arrayBuffer === "function") return blob.arrayBuffer().then((b) => new Uint8Array(b));
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => resolve(new Uint8Array(reader.result as ArrayBuffer));
    reader.onerror = () => reject(reader.error);
    reader.readAsArrayBuffer(blob);
  });
}

/** PNG로 저장…: the save panel, then the engine writes the file. */
export async function saveSnapshotAs(blob: Blob, req: Pick<SnapshotRequest, "page" | "docName">): Promise<boolean> {
  const base = (req.docName ?? "SeePDF").replace(/\.pdf$/i, "");
  const path = await api
    .saveFileDialog({ defaultPath: `${base}-${req.page + 1}쪽.png`, filters: [{ name: "PNG", extensions: ["png"] }] })
    .catch(() => null);
  if (!path) return false;
  try {
    await api.saveSnapshotPng({ path: /\.png$/i.test(path) ? path : `${path}.png`, bytes: await bytesOf(blob) });
    toast("snapshot.saved", undefined, { tone: "success" });
    return true;
  } catch (e) {
    toast("snapshot.failed", undefined, { tone: "danger", detail: e instanceof Error ? e.message : String(e) });
    return false;
  }
}

/**
 * Copies the region to the clipboard. **Call it synchronously from the pointer-up handler**: the
 * clipboard write starts before anything is awaited. Resolves `true` when the clipboard took it.
 */
export function copySnapshot(req: SnapshotRequest): Promise<boolean> {
  // v0.3 integration (S5 × V1): the document's copy permission covers snapshots too
  if (copyForbidden(useDocStore.getState().info)) {
    toast(reasonKey("extractText"), undefined, { tone: "info" });
    return Promise.resolve(false);
  }
  const png = snapshotPng(req);
  // never an unhandled rejection: every path below observes it
  png.catch(() => undefined);
  const clipboard = typeof navigator !== "undefined" ? navigator.clipboard : undefined;
  let written: Promise<void>;
  try {
    if (!clipboard?.write || typeof ClipboardItem === "undefined") throw new Error("no clipboard image support");
    written = clipboard.write([new ClipboardItem({ "image/png": png })]);
  } catch (e) {
    written = Promise.reject(e);
  }
  return written.then(
    () => {
      toast("snapshot.copied", undefined, { tone: "success", timeoutMs: 2500 });
      return true;
    },
    async (e: unknown) => {
      let blob: Blob;
      try {
        blob = await png;
      } catch (renderError) {
        toast("snapshot.failed", undefined, {
          tone: "danger",
          detail: renderError instanceof Error ? renderError.message : String(renderError),
        });
        return false;
      }
      toast("snapshot.copyFailed", undefined, {
        tone: "danger",
        detail: e instanceof Error ? e.message : String(e),
        actions: [{ labelKey: "snapshot.saveAs", onSelect: () => void saveSnapshotAs(blob, req) }],
      });
      return false;
    },
  );
}

/** The tool the mode goes back to once a snapshot is taken (or Esc). */
export function toolAfterSnapshot(): "select" | "fillForm" {
  return useAppStore.getState().mode === "form" ? "fillForm" : "select";
}

/**
 * The viewer's pointer-up: copy, then hand the pointer back to the mode's own tool (a snapshot is
 * one-shot, like 메모 — arming it again is one key or one menu item away).
 */
export function captureSnapshot(req: SnapshotRequest): Promise<boolean> {
  const done = copySnapshot(req);
  useAppStore.getState().setTool(toolAfterSnapshot());
  return done;
}

/**
 * The controller's view of the tool (registered by `registry.ts`): its cursor, and Esc hands back
 * to 선택. The marquee itself is the viewer's (see the file comment).
 */
export const snapshotTool: ToolModule<null> = {
  id: "snapshot",
  cursor: "crosshair",
  init: () => null,
  onDown: (state) => ({ state }),
  onMove: (state) => ({ state }),
  onUp: (state) => ({ state }),
  onKey: (state, key) => (key === "Escape" ? { state, preview: null, done: true } : { state }),
};
