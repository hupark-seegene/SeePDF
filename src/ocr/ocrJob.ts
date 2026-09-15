/**
 * The OCR job: page range → `/ocr` bitmaps → tesseract.js → `OcrPage` → `ocr_apply`, with progress
 * in `jobStore` and a cancel that actually stops (FEATURES F-21, IPC_CONTRACT §7.9).
 *
 * Shape of one run (spike §6, "batch, progress, cancel"):
 *
 *   ocr_page_status(all selected pages)        one call; drives "skip pages that already have text"
 *   for each batch of `workers` pages:
 *       fetch seepdf:///ocr?doc,gen,page,dpi   PNG gray8 at 300 DPI — pixels never cross a command
 *       pool.recognize(blob)  ×N in parallel   the only CPU-heavy part
 *       normalizeTesseract(...)                → OcrPage (image px, origin top-left)
 *       ocr_apply({ pages: [one] })  in order  one page per call, so cancelling at page 3 of 14
 *                                              leaves pages 1–2 applied and 4–14 untouched
 *
 * Why one page per `ocr_apply` and not one call for the whole document: F-21 wants a cancel that
 * leaves a coherent document, and the contract's batch form would either apply everything or nothing.
 * Each call is its own `registry::mutate`, so it is also its own undo step — the dialog says how many
 * pages were recognised and the user can undo them one at a time.
 *
 * Why the batch boundary exists at all: every `ocr_apply` bumps `docGeneration`, and a `/ocr` request
 * that carries an older `gen` is answered `410 Gone` (IPC_CONTRACT §9). So bitmaps are fetched for one
 * batch at the *current* generation, and the generation is re-read from the `DocInfo` each apply
 * returns.
 */
import * as api from "../ipc/api";
import { ocrImageUrl } from "../ipc/protocol";
import type { DocGeneration, DocId, DocInfo, JobEvent, JobId, OcrPage, PageGeom, PageIndex, Rotation } from "../ipc/types";
import { localJobId, registerCanceller, useJobStore } from "../store/jobStore";
import { countWords, meanConfidence, normalizeTesseract } from "./normalize";
import {
  DEFAULT_DPI, DEFAULT_LANGS, DEFAULT_LAYOUT, OcrCancelledError, TesseractPool,
  defaultWorkerCount, type OcrLayout,
} from "./tesseractPool";

// ---------------------------------------------------------------------------
// Page ranges
// ---------------------------------------------------------------------------

/**
 * `"1-3,5"` (1-based, as typed) → `[0, 1, 2, 4]` (0-based `PageIndex`, sorted, de-duplicated).
 * Returns `null` for anything malformed or out of range, which the dialog shows as
 * `ocr.range.invalid`. Empty input is `null` too — "no pages" is never what the user meant.
 */
export function parsePageRange(spec: string, pageCount: number): PageIndex[] | null {
  if (pageCount <= 0) return null;
  const parts = spec.split(/[,，]/).map((s) => s.trim()).filter((s) => s.length > 0);
  if (parts.length === 0) return null;
  const pages = new Set<PageIndex>();
  for (const part of parts) {
    const m = /^(\d+)(?:\s*[-–~]\s*(\d+))?$/.exec(part);
    if (!m) return null;
    const from = Number(m[1]);
    const to = m[2] === undefined ? from : Number(m[2]);
    if (!Number.isInteger(from) || !Number.isInteger(to)) return null;
    if (from < 1 || to < 1 || from > pageCount || to > pageCount) return null;
    const lo = Math.min(from, to);
    const hi = Math.max(from, to);
    for (let p = lo; p <= hi; p++) pages.add(p - 1);
  }
  return [...pages].sort((a, b) => a - b);
}

/** `[0,1,2,4]` → `"1-3, 5"` — the dialog echoes a normalised range back at the user. */
export function formatPageRange(pages: PageIndex[]): string {
  const sorted = [...new Set(pages)].sort((a, b) => a - b);
  const out: string[] = [];
  let i = 0;
  while (i < sorted.length) {
    let j = i;
    while (j + 1 < sorted.length && sorted[j + 1] === sorted[j] + 1) j++;
    out.push(i === j ? String(sorted[i] + 1) : `${sorted[i] + 1}-${sorted[j] + 1}`);
    i = j + 1;
  }
  return out.join(", ");
}

// ---------------------------------------------------------------------------
// Options / result
// ---------------------------------------------------------------------------

export type OcrDpi = "auto" | 200 | 300 | 400;

export interface OcrRunOptions {
  docId: DocId;
  /** current `DocInfo.docGeneration`; kept up to date from every `ocr_apply` reply */
  docGeneration: DocGeneration;
  /** 0-based pages, in the order they should be processed */
  pages: PageIndex[];
  /** page geometry from `DocInfo.pages`, used for the rotation and the px↔pt sanity check */
  pageGeom?: PageGeom[];
  langs?: string;
  layout?: OcrLayout;
  dpi?: OcrDpi;
  /** UI_SPEC `ocr.option.skipText`, on by default */
  skipPagesWithText?: boolean;
  /** re-OCR pages that already carry a text layer */
  replaceExisting?: boolean;
  workers?: number;
  /** progress of the page currently being recognised, 0..1 (tesseract's own logger) */
  onPageProgress?: (page: PageIndex, progress: number) => void;
  /** called after each page is applied */
  onPageDone?: (page: PageIndex, result: OcrPage) => void;
  /** an existing job id (the dialog allocates one so it can render the cancel button immediately) */
  jobId?: JobId;
  signal?: AbortSignal;
}

export interface OcrJobResult {
  jobId: JobId;
  status: "done" | "cancelled" | "error";
  /** pages whose text layer was applied */
  applied: PageIndex[];
  /** pages that already had text and were skipped */
  skipped: PageIndex[];
  words: number;
  /** mean word confidence over every applied page, 0..100 */
  confidence: number;
  elapsedMs: number;
  /** last `DocInfo` we saw, so the caller can refresh without another round trip */
  info?: DocInfo;
  /** i18n key for the toast/banner: `ocr.done` · `ocr.cancelled` · `ocr.failed` · `ocr.noImagePages` */
  messageKey: string;
  /** `error.*` key with the engine's reason, when there is one */
  detailKey?: string;
  error?: unknown;
}

/** Spike §3a: ~1.5 s per page at PSM 4 on this class of machine, plus a warm worker init. */
const SECONDS_PER_PAGE = 1.6;
const INIT_SECONDS = 0.3;

/** `ocr.estimate` — "예상 시간: 약 {{seconds}}초". */
export function estimateSeconds(pageCount: number, workers = defaultWorkerCount()): number {
  if (pageCount <= 0) return 0;
  return Math.max(1, Math.round(INIT_SECONDS + (pageCount * SECONDS_PER_PAGE) / Math.max(1, workers)));
}

/** `'auto'` has no native-DPI signal in the frontend, so it is 300 (spike §6 "rendering DPI"). */
export function resolveDpi(dpi: OcrDpi | undefined): number {
  return dpi === undefined || dpi === "auto" ? DEFAULT_DPI : dpi;
}

// ---------------------------------------------------------------------------
// The page bitmap
// ---------------------------------------------------------------------------

export interface PageBitmap { blob: Blob; widthPx: number; heightPx: number }

/** Width/height from a PNG IHDR — the fallback when the route's headers are not readable. */
export function readPngSize(bytes: Uint8Array): { widthPx: number; heightPx: number } | null {
  if (bytes.length < 24 || bytes[0] !== 0x89 || bytes[1] !== 0x50) return null;
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  if (view.getUint32(12) !== 0x49484452) return null; // "IHDR"
  return { widthPx: view.getUint32(16), heightPx: view.getUint32(20) };
}

function httpErrorKey(status: number): string {
  switch (status) {
    case 404: return "error.fileMissing";
    case 410: return "error.generic";       // generation moved on — the caller retries with a fresh gen
    case 503: return "error.busy";
    default: return "error.generic";
  }
}

class OcrHttpError extends Error {
  constructor(readonly status: number, readonly key: string) {
    super(`/ocr → HTTP ${status}`);
    this.name = "OcrHttpError";
  }
}

/**
 * Fetch one page image from the `/ocr` protocol route. `X-Image-Width`/`X-Image-Height` are part of
 * the route contract (§9); the PNG header is the belt-and-braces fallback (the mock adapter answers
 * with a `data:` URL that carries no headers).
 */
export async function fetchPageBitmap(
  a: { docId: DocId; gen: DocGeneration; page: PageIndex; dpi: number; signal?: AbortSignal },
): Promise<PageBitmap> {
  const url = ocrImageUrl({ doc: a.docId, gen: a.gen, page: a.page, dpi: a.dpi });
  const res = await fetch(url, { signal: a.signal });
  if (!res.ok) throw new OcrHttpError(res.status, httpErrorKey(res.status));
  const buffer = await res.arrayBuffer();
  const bytes = new Uint8Array(buffer);
  const header = {
    widthPx: Number(res.headers.get("X-Image-Width") ?? 0),
    heightPx: Number(res.headers.get("X-Image-Height") ?? 0),
  };
  const size = header.widthPx > 0 && header.heightPx > 0 ? header : readPngSize(bytes);
  if (!size) throw new Error("/ocr returned an image without usable dimensions");
  return { blob: new Blob([bytes as unknown as BlobPart], { type: "image/png" }), ...size };
}

/**
 * Sanity-check the bitmap against the page geometry: `widthPx ≈ widthPt × dpi / 72`. A mismatch means
 * the renderer used a different scale than we asked for, which would put every word box in the wrong
 * place. We do not fail on it — the engine converts pixels itself with `pixels_to_points` — but the
 * caller logs it, because it is exactly the sort of drift that silently ruins a text layer.
 */
export function bitmapScaleMismatch(bitmap: PageBitmap, geom: PageGeom | undefined, dpi: number): number | null {
  if (!geom) return null;
  const expected = (geom.widthPt * dpi) / 72;
  if (expected <= 0) return null;
  const ratio = bitmap.widthPx / expected;
  return Math.abs(ratio - 1) > 0.02 ? ratio : null;
}

// ---------------------------------------------------------------------------
// The job
// ---------------------------------------------------------------------------

function chunk<T>(items: T[], size: number): T[][] {
  const out: T[][] = [];
  for (let i = 0; i < items.length; i += Math.max(1, size)) out.push(items.slice(i, i + Math.max(1, size)));
  return out;
}

/**
 * Run OCR over `pages`. Never throws for an OCR failure — the outcome is in `OcrJobResult.status`
 * and `messageKey`, which is what the dialog and the toast both render.
 */
export async function runOcrJob(options: OcrRunOptions): Promise<OcrJobResult> {
  const jobs = useJobStore.getState();
  const jobId = options.jobId ?? localJobId();
  const startedAt = Date.now();
  const dpi = resolveDpi(options.dpi);
  const langs = options.langs ?? DEFAULT_LANGS;
  const layout = options.layout ?? DEFAULT_LAYOUT;
  const workers = options.workers ?? defaultWorkerCount();
  const skipPagesWithText = options.skipPagesWithText ?? true;
  const replaceExisting = options.replaceExisting ?? false;

  const controller = new AbortController();
  const abort = () => controller.abort();
  if (options.signal) {
    if (options.signal.aborted) abort();
    else options.signal.addEventListener("abort", abort, { once: true });
  }
  const unregister = registerCanceller(jobId, abort);
  /** the backend job id of the `ocr_apply` currently in flight, so cancel reaches the engine too */
  let backendJobId: JobId | null = null;

  const applied: PageIndex[] = [];
  const skipped: PageIndex[] = [];
  let words = 0;
  let confidenceSum = 0;
  let gen = options.docGeneration;
  let info: DocInfo | undefined;
  let pool: TesseractPool | null = null;

  const finish = (r: Omit<OcrJobResult, "jobId" | "elapsedMs">): OcrJobResult => ({
    ...r, jobId, elapsedMs: Date.now() - startedAt,
  });

  jobs.start({ id: jobId, kind: "ocr", labelKey: "status.ocr", total: options.pages.length });

  try {
    if (options.pages.length === 0) {
      jobs.update(jobId, { state: "done" });
      return finish({ status: "done", applied, skipped, words: 0, confidence: 0, messageKey: "ocr.noImagePages" });
    }

    // 1. Which pages already have text? One call for the whole selection (§7.9).
    let todo = options.pages;
    if (skipPagesWithText && !replaceExisting) {
      const status = await api.ocrPageStatus({ docId: options.docId, pages: options.pages });
      const hasText = new Set(status.filter((s) => s.hasText && s.charCount > 0).map((s) => s.page));
      todo = options.pages.filter((p) => !hasText.has(p));
      skipped.push(...options.pages.filter((p) => hasText.has(p)));
    }
    jobs.update(jobId, { total: todo.length });
    if (todo.length === 0) {
      jobs.update(jobId, { state: "done", done: 0 });
      return finish({ status: "done", applied, skipped, words: 0, confidence: 0, messageKey: "ocr.noImagePages" });
    }

    pool = new TesseractPool({
      langs, layout, dpi, workers,
      onProgress: (e) => {
        if (e.status === "recognizing text") options.onPageProgress?.(-1, e.progress);
      },
    });

    // 2. Batch of `workers` pages: fetch + recognise in parallel, then apply strictly in order.
    for (const batch of chunk(todo, workers)) {
      if (controller.signal.aborted) throw new OcrCancelledError();
      const batchGen = gen;
      const recognised = await Promise.all(batch.map(async (page) => {
        const bitmap = await fetchPageBitmap({ docId: options.docId, gen: batchGen, page, dpi, signal: controller.signal });
        const mismatch = bitmapScaleMismatch(bitmap, options.pageGeom?.[page], dpi);
        if (mismatch !== null) {
          console.warn(`[ocr] page ${page + 1}: /ocr bitmap is ${mismatch.toFixed(3)}× the expected size at ${dpi} DPI`);
        }
        const raw = await pool!.recognize(bitmap.blob, { langs, layout, dpi, signal: controller.signal });
        const rotation: Rotation = options.pageGeom?.[page]?.rotation ?? 0;
        return normalizeTesseract(raw, {
          page, dpi, widthPx: bitmap.widthPx, heightPx: bitmap.heightPx, rotation,
        });
      }));

      for (const ocrPage of recognised) {
        if (controller.signal.aborted) throw new OcrCancelledError();
        info = await api.ocrApply(
          { docId: options.docId, pages: [ocrPage], replaceExisting },
          (e: JobEvent) => {
            if (e.type === "started") backendJobId = e.jobId;
            if (e.type === "done" || e.type === "error" || e.type === "cancelled") backendJobId = null;
          },
        );
        gen = info.docGeneration;
        applied.push(ocrPage.page);
        const n = countWords(ocrPage);
        words += n;
        confidenceSum += meanConfidence(ocrPage) * n;
        options.onPageDone?.(ocrPage.page, ocrPage);
        jobs.update(jobId, { done: applied.length, total: todo.length, note: String(ocrPage.page + 1) });
      }
    }

    jobs.update(jobId, { state: "done", done: applied.length });
    return finish({
      status: "done", applied, skipped, words,
      confidence: words ? Math.round((confidenceSum / words) * 10) / 10 : 0,
      info, messageKey: "ocr.done",
    });
  } catch (e) {
    const cancelled = controller.signal.aborted
      || e instanceof OcrCancelledError
      || (e instanceof DOMException && e.name === "AbortError")
      || (api.isSeePdfError(e) && e.code === "cancelled");
    if (cancelled) {
      // Stop the engine-side half too; the frontend half is already dead.
      if (backendJobId !== null) await api.cancelJob({ jobId: backendJobId }).catch(() => false);
      jobs.update(jobId, { state: "cancelled", done: applied.length });
      return finish({
        status: "cancelled", applied, skipped, words,
        confidence: words ? Math.round((confidenceSum / words) * 10) / 10 : 0,
        info, messageKey: "ocr.cancelled",
      });
    }
    const detailKey = e instanceof OcrHttpError ? e.key : api.isSeePdfError(e) ? api.errorKey(e) : undefined;
    jobs.update(jobId, { state: "error", error: api.isSeePdfError(e) ? e.code : "pdfium" });
    console.error("[ocr] job failed", e);
    return finish({
      status: "error", applied, skipped, words,
      confidence: words ? Math.round((confidenceSum / words) * 10) / 10 : 0,
      info, messageKey: "ocr.failed", detailKey, error: e,
    });
  } finally {
    unregister();
    options.signal?.removeEventListener("abort", abort);
    await pool?.terminate().catch(() => {});
  }
}
