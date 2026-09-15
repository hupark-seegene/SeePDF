/**
 * `ocr.range.parse`, `ocr.cancel`, `ocr.skipText` (WORKPLAN row (f) DoD).
 *
 * The worker pool is replaced with a fake: these tests are about the *orchestration* — which pages
 * are fetched, which are skipped, what `ocr_apply` is called with, and what a cancel leaves behind.
 * The recogniser itself is measured by `scripts/ocr-accuracy.mjs` and the in-bundle probe.
 */
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { DocInfo, JobEvent, OcrPage, PageIndex } from "../ipc/types";
import { useJobStore } from "../store/jobStore";

// --------------------------------------------------------------------------- mocks

const ocrPageStatus = vi.fn();
const ocrApply = vi.fn();
const cancelJob = vi.fn();

vi.mock("../ipc/api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../ipc/api")>();
  return {
    ...actual,
    ocrPageStatus: (...a: unknown[]) => ocrPageStatus(...a),
    ocrApply: (...a: unknown[]) => ocrApply(...a),
    cancelJob: (...a: unknown[]) => cancelJob(...a),
  };
});

// In vitest `useMock()` is on, so `ocrImageUrl` would answer with a 1×1 `data:` URL that carries no
// query at all. Build the real route URL instead — these tests assert on `page` and `gen`.
vi.mock("../ipc/protocol", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../ipc/protocol")>();
  return {
    ...actual,
    ocrImageUrl: (a: { doc: string; gen: number; page: number; dpi?: number }) =>
      `seepdf://localhost/ocr?doc=${a.doc}&gen=${a.gen}&page=${a.page}&dpi=${a.dpi ?? 300}`,
  };
});

/** One fake tesseract line per page, so `normalizeTesseract` has something real to chew on. */
const recognize = vi.fn();

vi.mock("./tesseractPool", async (importOriginal) => {
  const actual = await importOriginal<typeof import("./tesseractPool")>();
  return {
    ...actual,
    TesseractPool: class {
      constructor(readonly options: Record<string, unknown>) {}
      get maxWorkers() { return (this.options.workers as number) ?? 1; }
      recognize(image: unknown, req: { signal?: AbortSignal } = {}) {
        if (req.signal?.aborted) throw new actual.OcrCancelledError();
        return recognize(image, req);
      }
      terminate() { return Promise.resolve(); }
    },
  };
});

import { bitmapScaleMismatch, formatPageRange, parsePageRange, readPngSize, resolveDpi, runOcrJob } from "./ocrJob";

// --------------------------------------------------------------------------- helpers

function tessPage(text = "가나 다라") {
  return {
    blocks: [{
      bbox: { x0: 0, y0: 0, x1: 2480, y1: 3508 },
      paragraphs: [{
        lines: [{
          text,
          confidence: 90,
          bbox: { x0: 100, y0: 100, x1: 400, y1: 160 },
          rowAttributes: { ascenders: 12, descenders: -6, rowHeight: 60 },
          words: text.split(" ").map((w, i) => ({
            text: w, confidence: 90,
            bbox: { x0: 100 + i * 150, y0: 100, x1: 220 + i * 150, y1: 160 },
          })),
        }],
      }],
    }],
    text,
  };
}

function docInfo(generation: number): DocInfo {
  return {
    docId: "d1", path: null, name: "t.pdf", bytes: 1, pageCount: 5, pages: [],
    docGeneration: generation, dirty: true, canUndo: true, canRedo: false,
    undoLabel: "ocr.title", redoLabel: null, encrypted: false,
    permissions: {
      print: true, modify: true, extractText: true, annotate: true,
      fillForms: true, assemble: true, revision: "unprotected",
    },
    hasForm: false, xfa: false, hasOutline: false, meta: {}, pdfVersion: "1.7", tagged: false,
  };
}

const fetched: number[] = [];

function stubFetch() {
  vi.stubGlobal("fetch", vi.fn(async (url: string) => {
    fetched.push(Number(/page=(\d+)/.exec(url)?.[1] ?? -1));
    return {
      ok: true,
      status: 200,
      headers: { get: (k: string) => (k === "X-Image-Width" ? "2480" : k === "X-Image-Height" ? "3508" : null) },
      arrayBuffer: async () => new Uint8Array([1, 2, 3, 4]).buffer,
    } as unknown as Response;
  }));
}

beforeEach(() => {
  fetched.length = 0;
  let gen = 1;
  ocrPageStatus.mockReset().mockImplementation(async (a: { pages: PageIndex[] }) =>
    a.pages.map((page) => ({ page, hasText: false, charCount: 0 })));
  ocrApply.mockReset().mockImplementation(async (_a: unknown, onProgress: (e: JobEvent) => void) => {
    onProgress({ type: "started", jobId: 77, total: 1 });
    onProgress({ type: "done", jobId: 77, elapsedMs: 1 });
    return docInfo(++gen);
  });
  cancelJob.mockReset().mockResolvedValue(true);
  recognize.mockReset().mockImplementation(async () => tessPage());
  useJobStore.setState({ jobs: [], active: null });
  stubFetch();
});

// --------------------------------------------------------------------------- ocr.range.parse

describe("ocr.range.parse", () => {
  it("parses a 1-based range into 0-based page indices", () => {
    expect(parsePageRange("1-3,5", 10)).toEqual([0, 1, 2, 4]);
    expect(parsePageRange("2", 10)).toEqual([1]);
    expect(parsePageRange(" 1 , 3 - 4 ", 10)).toEqual([0, 2, 3]);
  });

  it("de-duplicates, sorts and accepts a reversed range", () => {
    expect(parsePageRange("5-3,4,3", 10)).toEqual([2, 3, 4]);
    expect(parsePageRange("9,1", 10)).toEqual([0, 8]);
  });

  it("rejects anything out of range or malformed", () => {
    expect(parsePageRange("0", 10)).toBeNull();
    expect(parsePageRange("11", 10)).toBeNull();
    expect(parsePageRange("1-11", 10)).toBeNull();
    expect(parsePageRange("", 10)).toBeNull();
    expect(parsePageRange("   ", 10)).toBeNull();
    expect(parsePageRange("abc", 10)).toBeNull();
    expect(parsePageRange("1-", 10)).toBeNull();
    expect(parsePageRange("1-2-3", 10)).toBeNull();
    expect(parsePageRange("1", 0)).toBeNull();
  });

  it("round-trips through formatPageRange", () => {
    expect(formatPageRange([0, 1, 2, 4])).toBe("1-3, 5");
    expect(formatPageRange([3])).toBe("4");
    expect(formatPageRange([])).toBe("");
    expect(parsePageRange(formatPageRange([0, 1, 2, 4]), 10)).toEqual([0, 1, 2, 4]);
  });
});

describe("ocr.dpi", () => {
  it("resolves 'auto' to 300 and passes an explicit DPI through", () => {
    expect(resolveDpi("auto")).toBe(300);
    expect(resolveDpi(undefined)).toBe(300);
    expect(resolveDpi(400)).toBe(400);
  });

  it("reads a PNG IHDR when the route headers are missing", () => {
    const png = new Uint8Array(24);
    png.set([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a], 0);
    new DataView(png.buffer).setUint32(12, 0x49484452);
    new DataView(png.buffer).setUint32(16, 2480);
    new DataView(png.buffer).setUint32(20, 3508);
    expect(readPngSize(png)).toEqual({ widthPx: 2480, heightPx: 3508 });
    expect(readPngSize(new Uint8Array(8))).toBeNull();
  });

  it("flags a bitmap whose scale does not match the page size", () => {
    const geom = { index: 0, widthPt: 595, heightPt: 842, rotation: 0 as const, crop: { l: 0, b: 0, r: 595, t: 842 }, label: null };
    const good = { blob: new Blob(), widthPx: Math.round((595 * 300) / 72), heightPx: 3508 };
    expect(bitmapScaleMismatch(good, geom, 300)).toBeNull();
    expect(bitmapScaleMismatch({ ...good, widthPx: 1000 }, geom, 300)).toBeCloseTo(0.403, 2);
    expect(bitmapScaleMismatch(good, undefined, 300)).toBeNull();
  });
});

// --------------------------------------------------------------------------- ocr.skipText

describe("ocr.skipText", () => {
  it("skips pages that already have text and never fetches their bitmap", async () => {
    ocrPageStatus.mockImplementation(async (a: { pages: PageIndex[] }) =>
      a.pages.map((page) => ({ page, hasText: page === 1, charCount: page === 1 ? 4200 : 0 })));

    const r = await runOcrJob({
      docId: "d1", docGeneration: 1, pages: [0, 1, 2], workers: 1, skipPagesWithText: true,
    });

    expect(r.status).toBe("done");
    expect(r.skipped).toEqual([1]);
    expect(r.applied).toEqual([0, 2]);
    expect(fetched).toEqual([0, 2]);
    expect(ocrApply).toHaveBeenCalledTimes(2);
  });

  it("OCRs everything when the option is off", async () => {
    ocrPageStatus.mockImplementation(async (a: { pages: PageIndex[] }) =>
      a.pages.map((page) => ({ page, hasText: true, charCount: 4200 })));

    const r = await runOcrJob({
      docId: "d1", docGeneration: 1, pages: [0, 1], workers: 1, skipPagesWithText: false,
    });

    expect(ocrPageStatus).not.toHaveBeenCalled();
    expect(r.skipped).toEqual([]);
    expect(r.applied).toEqual([0, 1]);
  });

  it("reports ocr.noImagePages when every page already has text", async () => {
    ocrPageStatus.mockImplementation(async (a: { pages: PageIndex[] }) =>
      a.pages.map((page) => ({ page, hasText: true, charCount: 10 })));

    const r = await runOcrJob({ docId: "d1", docGeneration: 1, pages: [0, 1], workers: 1 });
    expect(r.messageKey).toBe("ocr.noImagePages");
    expect(ocrApply).not.toHaveBeenCalled();
  });
});

// --------------------------------------------------------------------------- the applied layer

describe("ocr.apply", () => {
  it("applies one page per call, in order, with the contract's OcrPage", async () => {
    await runOcrJob({ docId: "d1", docGeneration: 1, pages: [0, 1], workers: 1 });

    expect(ocrApply).toHaveBeenCalledTimes(2);
    const first = ocrApply.mock.calls[0][0] as { docId: string; pages: OcrPage[]; replaceExisting: boolean };
    expect(first.docId).toBe("d1");
    expect(first.replaceExisting).toBe(false);
    expect(first.pages).toHaveLength(1);
    expect(first.pages[0]).toMatchObject({ page: 0, dpi: 300, widthPx: 2480, heightPx: 3508, rotation: 0 });
    expect(first.pages[0].lines[0].text).toBe("가나 다라");
    expect((ocrApply.mock.calls[1][0] as { pages: OcrPage[] }).pages[0].page).toBe(1);
  });

  it("re-reads docGeneration from every apply so /ocr is never asked for a stale generation", async () => {
    const urls: string[] = [];
    vi.stubGlobal("fetch", vi.fn(async (url: string) => {
      urls.push(url);
      return {
        ok: true, status: 200,
        headers: { get: (k: string) => (k === "X-Image-Width" ? "2480" : k === "X-Image-Height" ? "3508" : null) },
        arrayBuffer: async () => new Uint8Array([1]).buffer,
      } as unknown as Response;
    }));
    await runOcrJob({ docId: "d1", docGeneration: 1, pages: [0, 1], workers: 1 });
    expect(urls[0]).toContain("gen=1");
    expect(urls[1]).toContain("gen=2");   // the first ocr_apply bumped it
  });

  it("surfaces an engine failure with an i18n key instead of throwing", async () => {
    ocrApply.mockRejectedValue(Object.assign(new Error("boom"), { name: "SeePdfError", code: "pdfium" }));
    const r = await runOcrJob({ docId: "d1", docGeneration: 1, pages: [0], workers: 1 });
    expect(r.status).toBe("error");
    expect(r.messageKey).toBe("ocr.failed");
  });
});

// --------------------------------------------------------------------------- ocr.cancel

describe("ocr.cancel", () => {
  it("leaves the pages applied before the cancel and never touches the ones after", async () => {
    const jobId = -999;
    let applied = 0;
    ocrApply.mockImplementation(async (_a: unknown, onProgress: (e: JobEvent) => void) => {
      onProgress({ type: "started", jobId: 77, total: 1 });
      applied += 1;
      // Cancel the moment page 3 of 14 has been applied (the F-21 acceptance scenario).
      if (applied === 3) await useJobStore.getState().cancel(jobId);
      onProgress({ type: "done", jobId: 77, elapsedMs: 1 });
      return docInfo(1 + applied);
    });

    const pages = Array.from({ length: 14 }, (_, i) => i);
    const r = await runOcrJob({ docId: "d1", docGeneration: 1, pages, workers: 1, jobId });

    expect(r.status).toBe("cancelled");
    expect(r.messageKey).toBe("ocr.cancelled");
    expect(r.applied).toEqual([0, 1, 2]);
    expect(ocrApply).toHaveBeenCalledTimes(3);
    // No bitmap was fetched for a page we were never going to apply.
    expect(fetched).toEqual([0, 1, 2]);
    expect(useJobStore.getState().jobs.find((j) => j.id === jobId)?.state).toBe("cancelled");
  });

  it("cancels the engine-side job too when one is in flight", async () => {
    const jobId = -998;
    ocrApply.mockImplementation(async (_a: unknown, onProgress: (e: JobEvent) => void) => {
      onProgress({ type: "started", jobId: 42, total: 1 });
      await useJobStore.getState().cancel(jobId);      // cancel while `ocr_apply` is still running
      throw Object.assign(new Error("cancelled"), { name: "SeePdfError", code: "cancelled" });
    });
    const r = await runOcrJob({ docId: "d1", docGeneration: 1, pages: [0, 1], workers: 1, jobId });
    expect(r.status).toBe("cancelled");
    expect(r.applied).toEqual([]);
    expect(cancelJob).toHaveBeenCalledWith({ jobId: 42 });
  });

  it("stops before the first page when the caller's signal is already aborted", async () => {
    const controller = new AbortController();
    controller.abort();
    const r = await runOcrJob({ docId: "d1", docGeneration: 1, pages: [0, 1], workers: 1, signal: controller.signal });
    expect(r.status).toBe("cancelled");
    expect(ocrApply).not.toHaveBeenCalled();
    expect(fetched).toEqual([]);
  });

  it("aborts the in-flight bitmap fetch", async () => {
    const jobId = -997;
    vi.stubGlobal("fetch", vi.fn((url: string, init?: { signal?: AbortSignal }) => new Promise((_, reject) => {
      void url;
      init?.signal?.addEventListener("abort", () => reject(new DOMException("aborted", "AbortError")));
      void useJobStore.getState().cancel(jobId);
    })));
    const r = await runOcrJob({ docId: "d1", docGeneration: 1, pages: [0], workers: 1, jobId });
    expect(r.status).toBe("cancelled");
    expect(r.applied).toEqual([]);
  });
});

// --------------------------------------------------------------------------- progress

describe("ocr.progress", () => {
  it("feeds the status-bar job slot", async () => {
    const jobId = -996;
    const seen: number[] = [];
    const unsubscribe = useJobStore.subscribe((s) => {
      const j = s.jobs.find((x) => x.id === jobId);
      if (j) seen.push(j.done);
    });
    await runOcrJob({ docId: "d1", docGeneration: 1, pages: [0, 1, 2], workers: 1, jobId });
    unsubscribe();
    const job = useJobStore.getState().jobs.find((j) => j.id === jobId);
    expect(job).toMatchObject({ kind: "ocr", labelKey: "status.ocr", state: "done", done: 3, total: 3 });
    expect(seen).toContain(1);
    expect(seen).toContain(2);
  });

  it("batches by worker count", async () => {
    await runOcrJob({ docId: "d1", docGeneration: 1, pages: [0, 1, 2, 3], workers: 2 });
    expect(recognize).toHaveBeenCalledTimes(4);
    expect(fetched).toEqual([0, 1, 2, 3]);
  });
});
