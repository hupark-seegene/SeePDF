/**
 * The tesseract.js worker pool — the OCR engine SeePDF ships on both platforms (ARCHITECTURE D10).
 *
 * Everything it needs is served **same-origin** from `public/ocr/` (`scripts/prepare-ocr.mjs`):
 * tesseract.js otherwise points `workerPath`, `corePath` and `langPath` at jsdelivr, and "no network
 * request is made at any point" is an acceptance criterion of F-21, not a nicety.
 *
 * Two CSP-relevant choices (spike §4.6/§4.7, probed in a real bundle — docs/STAGE1F_NOTES.md §1):
 *   * `workerBlobURL: false` — the default wraps the worker in a `blob:` URL that `importScripts` the
 *     real one; `new Worker('/ocr/worker.min.js')` needs no `blob:` in `worker-src`.
 *   * `corePath` names one **file**, not a directory: left to itself the worker asks
 *     `wasm-feature-detect` and requests `tesseract-core-relaxedsimd-lstm.wasm.js` on Chromium
 *     (WebView2), which we deliberately do not ship. We do the SIMD check here instead.
 *
 * Memory (spike §3a): ~95–120 MB RSS per worker, so the default pool is `min(4, cores/2)` and it is
 * terminated when a job finishes rather than kept warm — warm init is only 70–170 ms.
 */
import { createWorker, OEM, PSM, type Worker as TesseractWorker } from "tesseract.js";
import type { TessPage } from "./normalize";

// ---------------------------------------------------------------------------
// Assets
// ---------------------------------------------------------------------------

/** Where `scripts/prepare-ocr.mjs` puts the offline bundle, relative to the app origin. */
export const OCR_ASSET_DIR = "/ocr";
const CORE_SIMD = "tesseract-core-simd-lstm.wasm.js";
const CORE_PLAIN = "tesseract-core-lstm.wasm.js";

/**
 * Absolute URL of an OCR asset. tesseract.js resolves relative paths against `window.location.href`
 * itself, but the worker then fetches `langPath` from *inside* the worker, so we hand it an absolute
 * URL and stop guessing. In the bundle the origin is `tauri://localhost`, in `vite dev`
 * `http://localhost:1420`, and `'self'` covers both.
 */
export function ocrAssetUrl(path: string): string {
  const base = typeof document !== "undefined" && document.baseURI
    ? document.baseURI
    : typeof location !== "undefined" ? location.href : "http://localhost/";
  return new URL(`${OCR_ASSET_DIR}/${path}`.replace(/\/{2,}/g, "/"), base).href;
}

/**
 * `wasm-feature-detect`'s fixed-width SIMD probe (`v128` return + `i8x16.splat`), inlined so the
 * check costs one `WebAssembly.validate` and no import. CSP-safe: `validate` compiles nothing.
 */
const SIMD_MODULE = Uint8Array.from([
  0, 97, 115, 109, 1, 0, 0, 0, 1, 5, 1, 96, 0, 1, 123, 3, 2, 1, 0, 10, 10, 1, 8, 0, 65, 0, 253, 15, 253, 98, 11,
]);

let simdSupported: boolean | null = null;

/** True when the WebView runs WASM SIMD (WKWebView ≥ Safari 16.4, every WebView2). */
export function hasWasmSimd(): boolean {
  if (simdSupported === null) {
    try {
      simdSupported = typeof WebAssembly !== "undefined" && WebAssembly.validate(SIMD_MODULE);
    } catch {
      simdSupported = false;
    }
  }
  return simdSupported;
}

/** The core file this WebView must load. `--with-fallback-core` ships the non-SIMD one. */
export function coreFileName(): string {
  return hasWasmSimd() ? CORE_SIMD : CORE_PLAIN;
}

// ---------------------------------------------------------------------------
// Options
// ---------------------------------------------------------------------------

/** UI_SPEC §15.20 `ocr.layout.*`. Spike §4.1: PSM 3 dropped a whole paragraph on a sparse Korean page. */
export type OcrLayout = "auto" | "column" | "block";

/** Default layout. `column` (PSM 4) is the measured best for Korean and the F-21 CER gate. */
export const DEFAULT_LAYOUT: OcrLayout = "column";
/** `kor` alone reads English as digits (spike §4.2) — the Korean preset is always `kor+eng`. */
export const DEFAULT_LANGS = "kor+eng";
export const DEFAULT_DPI = 300;

export function psmFor(layout: OcrLayout): PSM {
  switch (layout) {
    case "auto": return PSM.AUTO;
    case "block": return PSM.SINGLE_BLOCK;
    case "column":
    default: return PSM.SINGLE_COLUMN;
  }
}

/** `min(4, cores/2)`: ~95–120 MB RSS each, and 4 workers reach 1.5 s/page on 10 cores (spike §3a). */
export function defaultWorkerCount(hardwareConcurrency?: number): number {
  const cores = hardwareConcurrency
    ?? (typeof navigator !== "undefined" ? navigator.hardwareConcurrency : undefined)
    ?? 2;
  return Math.max(1, Math.min(4, Math.floor(cores / 2)));
}

export interface TesseractPoolOptions {
  /** `"kor+eng"`. Changing it between jobs re-initialises the workers. */
  langs?: string;
  layout?: OcrLayout;
  /** Feeds `user_defined_dpi`, which the LSTM engine uses to pick its scale. */
  dpi?: number;
  workers?: number;
  /** Sub-page progress (`status: 'recognizing text'`, 0→1) for the dialog's bar. */
  onProgress?: (e: { status: string; progress: number }) => void;
}

export interface RecognizeRequest {
  langs?: string;
  layout?: OcrLayout;
  dpi?: number;
  signal?: AbortSignal;
  onProgress?: (progress: number) => void;
}

/** Thrown when a recognition is abandoned (cancel, or the pool was terminated under it). */
export class OcrCancelledError extends Error {
  constructor(message = "OCR cancelled") {
    super(message);
    this.name = "OcrCancelledError";
  }
}

/** Anything the worker's `loadImage` understands and we actually produce: PNG bytes from `/ocr`. */
export type OcrImage = Blob | Uint8Array;

// ---------------------------------------------------------------------------
// Pool
// ---------------------------------------------------------------------------

interface Slot {
  worker: TesseractWorker;
  langs: string;
  layout: OcrLayout;
  dpi: number;
  busy: boolean;
  /** set while a recognition is in flight so `terminate()` can reject its waiter */
  reject?: (e: unknown) => void;
}

/**
 * A fixed-size pool of tesseract.js workers with a FIFO waiting queue.
 *
 * We deliberately do not use `createScheduler()`: it owns the queue, and cancelling means throwing
 * the whole scheduler away, which loses the workers that were not running the cancelled page.
 * Here a cancel terminates exactly the busy worker (tesseract.js has no per-job cancel, spike §6)
 * and the pool refills lazily on the next page.
 */
export class TesseractPool {
  private readonly opts: Required<Omit<TesseractPoolOptions, "onProgress">> & Pick<TesseractPoolOptions, "onProgress">;
  private slots: Slot[] = [];
  private waiting: ((slot: Slot) => void)[] = [];
  /** workers whose `createWorker` has not resolved yet — without this the pool over-spawns. */
  private spawning = 0;
  private terminated = false;

  constructor(options: TesseractPoolOptions = {}) {
    this.opts = {
      langs: options.langs ?? DEFAULT_LANGS,
      layout: options.layout ?? DEFAULT_LAYOUT,
      dpi: options.dpi ?? DEFAULT_DPI,
      workers: options.workers ?? defaultWorkerCount(),
      onProgress: options.onProgress,
    };
  }

  get size(): number {
    return this.slots.length;
  }

  get maxWorkers(): number {
    return this.opts.workers;
  }

  /** Create one worker pointed at `public/ocr/`. */
  private async spawn(langs: string, layout: OcrLayout, dpi: number): Promise<Slot> {
    const worker = await createWorker(langs, OEM.LSTM_ONLY, {
      workerPath: ocrAssetUrl("worker.min.js"),
      corePath: ocrAssetUrl(coreFileName()),
      langPath: ocrAssetUrl("tessdata"),
      // Our traineddata is shipped gzipped; the worker inflates it with zlibjs.
      gzip: true,
      // IndexedDB caching of files that are already on disk next to the app buys nothing.
      cacheMethod: "none",
      // CSP: no `blob:` worker (spike §4.7).
      workerBlobURL: false,
      logger: this.opts.onProgress
        ? (m) => this.opts.onProgress?.({ status: m.status, progress: m.progress })
        : () => {},
    });
    await worker.setParameters({
      tessedit_pageseg_mode: psmFor(layout),
      user_defined_dpi: String(dpi),
      // Without this Tesseract collapses runs of spaces and Hangul spacing gets worse.
      preserve_interword_spaces: "1",
    });
    return { worker, langs, layout, dpi, busy: false };
  }

  /** Take a free slot, growing the pool up to `workers`, otherwise queue. */
  private async acquire(): Promise<Slot> {
    if (this.terminated) throw new OcrCancelledError("pool terminated");
    const free = this.slots.find((s) => !s.busy);
    if (free) {
      free.busy = true;
      return free;
    }
    // `slots.length + spawning`: N concurrent `recognize` calls all reach here before the first
    // `createWorker` resolves, and a naive length check would spawn N workers for a cap of 2.
    if (this.slots.length + this.spawning < this.opts.workers) {
      this.spawning += 1;
      let slot: Slot;
      try {
        slot = await this.spawn(this.opts.langs, this.opts.layout, this.opts.dpi);
      } finally {
        this.spawning -= 1;
      }
      if (this.terminated) {
        await slot.worker.terminate().catch(() => {});
        throw new OcrCancelledError("pool terminated");
      }
      slot.busy = true;
      this.slots.push(slot);
      return slot;
    }
    return new Promise<Slot>((resolve) => this.waiting.push(resolve));
  }

  private release(slot: Slot): void {
    slot.busy = false;
    slot.reject = undefined;
    const next = this.waiting.shift();
    if (next) {
      slot.busy = true;
      next(slot);
    }
  }

  /** Replace a worker we had to kill (cancel) so the pool keeps its size. */
  private async replace(slot: Slot): Promise<void> {
    this.slots = this.slots.filter((s) => s !== slot);
    await slot.worker.terminate().catch(() => {});
    if (this.terminated) return;
    const next = this.waiting.shift();
    if (!next) return;
    try {
      const fresh = await this.spawn(this.opts.langs, this.opts.layout, this.opts.dpi);
      fresh.busy = true;
      this.slots.push(fresh);
      next(fresh);
    } catch {
      this.waiting.unshift(next);
    }
  }

  /**
   * Recognise one page image. Returns the raw tesseract page — `normalize.ts` turns it into an
   * `OcrPage`; keeping the two apart is what lets the accuracy script score the normaliser alone.
   */
  async recognize(image: OcrImage, req: RecognizeRequest = {}): Promise<TessPage> {
    if (req.signal?.aborted) throw new OcrCancelledError();
    const slot = await this.acquire();
    const langs = req.langs ?? this.opts.langs;
    const layout = req.layout ?? this.opts.layout;
    const dpi = req.dpi ?? this.opts.dpi;

    let killed = false;
    const onAbort = () => {
      killed = true;
      // tesseract.js cannot cancel a running `recognize`; killing the worker is the only way out.
      slot.reject?.(new OcrCancelledError());
      void this.replace(slot);
    };

    try {
      if (langs !== slot.langs) {
        await slot.worker.reinitialize(langs, OEM.LSTM_ONLY);
        slot.langs = langs;
        slot.layout = "auto";  // reinitialize resets the parameters
      }
      if (layout !== slot.layout || dpi !== slot.dpi) {
        await slot.worker.setParameters({
          tessedit_pageseg_mode: psmFor(layout),
          user_defined_dpi: String(dpi),
          preserve_interword_spaces: "1",
        });
        slot.layout = layout;
        slot.dpi = dpi;
      }
      if (req.signal?.aborted) throw new OcrCancelledError();
      req.signal?.addEventListener("abort", onAbort, { once: true });

      const blob = image instanceof Uint8Array
        ? new Blob([image.buffer.slice(image.byteOffset, image.byteOffset + image.byteLength) as ArrayBuffer], { type: "image/png" })
        : image;

      const result = await new Promise<TessPage>((resolve, reject) => {
        slot.reject = reject;
        slot.worker
          // `{ blocks: true }` is what carries the word boxes; v6+ dropped `data.words` (spike §3a).
          .recognize(blob, {}, { text: true, blocks: true })
          .then((r) => resolve(r.data as unknown as TessPage), reject);
      });
      return result;
    } finally {
      req.signal?.removeEventListener("abort", onAbort);
      if (!killed) this.release(slot);
    }
  }

  /** Terminate every worker and reject anything still waiting. Safe to call twice. */
  async terminate(): Promise<void> {
    this.terminated = true;
    const slots = this.slots;
    this.slots = [];
    for (const s of slots) s.reject?.(new OcrCancelledError("pool terminated"));
    const waiting = this.waiting;
    this.waiting = [];
    // A queued caller can never be served now; unblock it with a slot that immediately throws.
    for (const w of waiting) {
      w({
        busy: true, langs: this.opts.langs, layout: this.opts.layout, dpi: this.opts.dpi,
        worker: rejectingWorker(),
      });
    }
    await Promise.all(slots.map((s) => s.worker.terminate().catch(() => {})));
  }
}

/** A stand-in handed to callers that were queued when the pool was terminated. */
function rejectingWorker(): TesseractWorker {
  const fail = () => Promise.reject(new OcrCancelledError("pool terminated"));
  return new Proxy({} as TesseractWorker, { get: () => fail });
}
