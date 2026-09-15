/**
 * The pool's contract with tesseract.js: offline asset paths, `workerBlobURL: false`, PSM mapping,
 * worker count, queueing, and a cancel that kills exactly the busy worker.
 *
 * `tesseract.js` is mocked — spawning four real WASM workers in jsdom would take seconds and prove
 * nothing that `scripts/ocr-accuracy.mjs` and the in-bundle probe do not already prove.
 */
import { beforeEach, describe, expect, it, vi } from "vitest";

const createWorker = vi.fn();

vi.mock("tesseract.js", async (importOriginal) => {
  const actual = await importOriginal<typeof import("tesseract.js")>();
  return { ...actual, createWorker: (...a: unknown[]) => createWorker(...a) };
});

import {
  coreFileName, defaultWorkerCount, hasWasmSimd, ocrAssetUrl, psmFor, OcrCancelledError, TesseractPool,
} from "./tesseractPool";

interface FakeWorker {
  recognize: ReturnType<typeof vi.fn>;
  setParameters: ReturnType<typeof vi.fn>;
  reinitialize: ReturnType<typeof vi.fn>;
  terminate: ReturnType<typeof vi.fn>;
}

const workers: FakeWorker[] = [];

function fakeWorker(recognize = vi.fn(async () => ({ data: { blocks: [], text: "" } }))): FakeWorker {
  const w: FakeWorker = {
    recognize,
    setParameters: vi.fn(async () => ({})),
    reinitialize: vi.fn(async () => ({})),
    terminate: vi.fn(async () => ({})),
  };
  workers.push(w);
  return w;
}

beforeEach(() => {
  workers.length = 0;
  createWorker.mockReset().mockImplementation(async () => fakeWorker());
});

describe("ocr.assets", () => {
  it("resolves every asset against the app origin, never a CDN", () => {
    expect(ocrAssetUrl("worker.min.js")).toMatch(/\/ocr\/worker\.min\.js$/);
    expect(ocrAssetUrl("tessdata")).toMatch(/\/ocr\/tessdata$/);
    expect(ocrAssetUrl("worker.min.js")).not.toContain("jsdelivr");
  });

  it("picks the SIMD LSTM core when the engine supports SIMD", () => {
    expect(hasWasmSimd()).toBe(true);           // Node 24 / jsdom: WebAssembly with SIMD
    expect(coreFileName()).toBe("tesseract-core-simd-lstm.wasm.js");
  });

  it("maps the layout option to a page segmentation mode (spike §4.1)", () => {
    expect(psmFor("auto")).toBe("3");
    expect(psmFor("column")).toBe("4");
    expect(psmFor("block")).toBe("6");
  });

  it("caps the pool at min(4, cores/2)", () => {
    expect(defaultWorkerCount(2)).toBe(1);
    expect(defaultWorkerCount(8)).toBe(4);
    expect(defaultWorkerCount(10)).toBe(4);
    expect(defaultWorkerCount(1)).toBe(1);
  });
});

describe("ocr.pool", () => {
  it("creates workers pointed at public/ocr with no blob: worker", async () => {
    const pool = new TesseractPool({ workers: 1 });
    await pool.recognize(new Blob([new Uint8Array([1])]));
    expect(createWorker).toHaveBeenCalledTimes(1);
    const [langs, oem, options] = createWorker.mock.calls[0] as [string, number, Record<string, unknown>];
    expect(langs).toBe("kor+eng");
    expect(oem).toBe(1);                            // OEM.LSTM_ONLY — the 4.0.0_best_int data needs it
    expect(options.workerBlobURL).toBe(false);
    expect(options.gzip).toBe(true);
    expect(options.cacheMethod).toBe("none");
    expect(String(options.workerPath)).toMatch(/\/ocr\/worker\.min\.js$/);
    expect(String(options.corePath)).toMatch(/\/ocr\/tesseract-core-simd-lstm\.wasm\.js$/);
    expect(String(options.langPath)).toMatch(/\/ocr\/tessdata$/);
    // PSM 4 and the DPI go in as parameters, not as worker options.
    expect(workers[0].setParameters).toHaveBeenCalledWith(
      expect.objectContaining({ tessedit_pageseg_mode: "4", user_defined_dpi: "300", preserve_interword_spaces: "1" }),
    );
    await pool.terminate();
  });

  it("asks for word boxes (`blocks: true`)", async () => {
    const pool = new TesseractPool({ workers: 1 });
    await pool.recognize(new Blob([new Uint8Array([1])]));
    expect(workers[0].recognize).toHaveBeenCalledWith(expect.anything(), {}, { text: true, blocks: true });
    await pool.terminate();
  });

  it("reuses one worker for several pages and never grows past the cap", async () => {
    const pool = new TesseractPool({ workers: 2 });
    await Promise.all([0, 1, 2, 3].map(() => pool.recognize(new Blob([new Uint8Array([1])]))));
    expect(createWorker).toHaveBeenCalledTimes(2);
    expect(pool.size).toBe(2);
    await pool.terminate();
    for (const w of workers) expect(w.terminate).toHaveBeenCalled();
  });

  it("converts Uint8Array page bytes into a PNG blob for the worker", async () => {
    const pool = new TesseractPool({ workers: 1 });
    await pool.recognize(new Uint8Array([0x89, 0x50, 0x4e, 0x47]));
    const arg = workers[0].recognize.mock.calls[0][0] as Blob;
    expect(arg).toBeInstanceOf(Blob);
    expect(arg.type).toBe("image/png");
    await pool.terminate();
  });

  it("kills the busy worker on abort and rejects with OcrCancelledError", async () => {
    let recognizing: () => void = () => {};
    const inFlight = new Promise<void>((resolve) => { recognizing = resolve; });
    createWorker.mockImplementation(async () => fakeWorker(vi.fn(() => {
      recognizing();
      return new Promise(() => {});   // a recognition that never finishes on its own
    })));

    const pool = new TesseractPool({ workers: 1 });
    const controller = new AbortController();
    const pending = pool.recognize(new Blob([new Uint8Array([1])]), { signal: controller.signal });
    await inFlight;                    // the worker really is chewing on the page now
    controller.abort();

    await expect(pending).rejects.toBeInstanceOf(OcrCancelledError);
    // tesseract.js has no per-job cancel (spike §6): killing the worker is the only way out.
    expect(workers[0].terminate).toHaveBeenCalled();
    expect(pool.size).toBe(0);
    await pool.terminate();
  });

  it("refuses to start when the signal is already aborted", async () => {
    const pool = new TesseractPool({ workers: 1 });
    const controller = new AbortController();
    controller.abort();
    await expect(pool.recognize(new Blob([new Uint8Array([1])]), { signal: controller.signal }))
      .rejects.toBeInstanceOf(OcrCancelledError);
    expect(createWorker).not.toHaveBeenCalled();
  });

  it("re-initialises a worker when the language string changes", async () => {
    const pool = new TesseractPool({ workers: 1, langs: "kor+eng" });
    await pool.recognize(new Blob([new Uint8Array([1])]));
    await pool.recognize(new Blob([new Uint8Array([1])]), { langs: "eng" });
    expect(workers[0].reinitialize).toHaveBeenCalledWith("eng", 1);
    expect(createWorker).toHaveBeenCalledTimes(1);
    await pool.terminate();
  });
});
