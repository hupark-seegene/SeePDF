/**
 * `seepdf://` URL builders — IPC_CONTRACT §9. Pixels never travel through a command; every tile,
 * page image, thumbnail and OCR bitmap is an `<img src>` on this scheme.
 *
 * Origin (tauri spike §1): `seepdf://localhost/` on macOS/Linux, `http://seepdf.localhost/` on
 * Windows. We never sniff the OS when Tauri is present — `convertFileSrc('', 'seepdf')` returns the
 * platform-correct origin. The sniffing fallback exists only for `vite dev` / vitest, where the mock
 * adapter answers with data URLs anyway.
 */
import { detectOs, tauriInternals, useMock, type OsName } from "./env";
import type { DocGeneration, DocId, PageIndex, Rotation } from "./types";

export type QueryValue = string | number | boolean | undefined;
export type Query = Record<string, QueryValue>;

export const TILE_PX = 512;

/** The origin of the custom scheme for one platform (tauri spike §1, `scripts/core.js`). */
export function originForOs(os: OsName, useHttpsScheme = false): string {
  if (os === "windows") return `${useHttpsScheme ? "https" : "http"}://seepdf.localhost`;
  return "seepdf://localhost";
}

export function seepdfOrigin(): string {
  const convert = tauriInternals()?.convertFileSrc;
  if (convert) return convert("", "seepdf").replace(/\/$/, "");
  return originForOs(detectOs());
}

/** `seepdf://localhost/<route>?k=v…` (or the Windows form). Undefined values are dropped. */
export function seepdfUrl(route: string, query: Query = {}, origin: string = seepdfOrigin()): string {
  const qs = Object.entries(query)
    .filter(([, v]) => v !== undefined)
    .map(([k, v]) => `${encodeURIComponent(k)}=${encodeURIComponent(String(v))}`)
    .join("&");
  const path = route.startsWith("/") ? route : `/${route}`;
  return `${origin}${path}${qs ? `?${qs}` : ""}`;
}

export interface TileParams {
  doc: DocId;
  gen: DocGeneration;
  page: PageIndex;
  /** `scaleKey` = round(zoomPercent × devicePixelRatio); device scale s = sk / 100. */
  sk: number;
  rot: Rotation;
  /** tile indices — device origin is (tx·512, ty·512) */
  tx: number;
  ty: number;
  night?: boolean;
  hl?: boolean;
  /**
   * Draw the AcroForm widgets in the bitmap (default `true`). 양식 mode sets it to `false`
   * while the HTML overlay is mounted, so a field value is rendered exactly once — by the
   * input on top, not by PDFium underneath as well (F-20).
   */
  forms?: boolean;
}

export type PageParams = Omit<TileParams, "tx" | "ty">;

/** 1×1 transparent PNG: what a route answers with before the mock adapter has loaded. */
const BLANK_PNG =
  "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==";

export type MockAssetResolver = (route: string, query: Query) => string;
let mockAssetResolver: MockAssetResolver | null = null;

/** Called by `mock.ts` when it loads; keeps the mock out of the production bundle. */
export function setMockAssetResolver(fn: MockAssetResolver | null): void {
  mockAssetResolver = fn;
}

function build(route: string, query: Query): string {
  if (useMock()) return mockAssetResolver ? mockAssetResolver(route, query) : BLANK_PNG;
  return seepdfUrl(route, query);
}

export function tileUrl(p: TileParams): string {
  return build("/tile", {
    doc: p.doc, gen: p.gen, page: p.page, sk: p.sk, rot: p.rot, tx: p.tx, ty: p.ty,
    night: p.night ? 1 : undefined, hl: p.hl ? 1 : undefined, forms: p.forms === false ? 0 : undefined,
  });
}

export function pageUrl(p: PageParams): string {
  return build("/page", {
    doc: p.doc, gen: p.gen, page: p.page, sk: p.sk, rot: p.rot,
    night: p.night ? 1 : undefined, hl: p.hl ? 1 : undefined, forms: p.forms === false ? 0 : undefined,
  });
}

export function thumbUrl(a: { doc: DocId; gen: DocGeneration; page: PageIndex; w: number; rot?: Rotation }): string {
  return build("/thumb", { doc: a.doc, gen: a.gen, page: a.page, w: a.w, rot: a.rot });
}

export function ocrImageUrl(a: { doc: DocId; gen: DocGeneration; page: PageIndex; dpi?: number }): string {
  return build("/ocr", { doc: a.doc, gen: a.gen, page: a.page, dpi: a.dpi ?? 300 });
}

export function recentThumbUrl(id: string): string {
  return build("/recent-thumb", { id });
}

export function rawDocumentUrl(doc: DocId): string {
  return build("/raw", { doc });
}

/** Number of tiles across/down for a page of `widthPx × heightPx` device pixels. */
export function tileGrid(widthPx: number, heightPx: number, tile = TILE_PX): { cols: number; rows: number } {
  return { cols: Math.max(1, Math.ceil(widthPx / tile)), rows: Math.max(1, Math.ceil(heightPx / tile)) };
}

/** `sk` for a zoom percentage at the current display density (§9). */
export function scaleKey(zoomPercent: number, dpr = typeof window === "undefined" ? 1 : window.devicePixelRatio || 1): number {
  return Math.round(zoomPercent * dpr);
}
