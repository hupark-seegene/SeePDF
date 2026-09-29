/**
 * v0.3 O2 페이지 회전 자동 감지: the pure pick (the twin of `engine/ocr/orientation.rs` `pick`), the
 * tesseract four-way detection, and what `runOcrJob` does with it — Vision asks the backend, every
 * other engine asks tesseract; a turned page is recognised turned and applied as
 * `{ page, ocr, setRotation }` in the same `ocr_apply` (= undo step) as its text.
 */
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { DocInfo, JobEvent, OcrPage, OrientationScore, PageIndex, Rotation } from "../ipc/types";
import { useJobStore } from "../store/jobStore";

const ocrPageStatus = vi.fn();
const ocrApply = vi.fn();
const ocrRecognizeNative = vi.fn();
const ocrDetectOrientation = vi.fn();

vi.mock("../ipc/api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../ipc/api")>();
  return {
    ...actual,
    ocrPageStatus: (...a: unknown[]) => ocrPageStatus(...a),
    ocrApply: (...a: unknown[]) => ocrApply(...a),
    ocrRecognizeNative: (...a: unknown[]) => ocrRecognizeNative(...a),
    ocrDetectOrientation: (...a: unknown[]) => ocrDetectOrientation(...a),
  };
});

vi.mock("../ipc/protocol", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../ipc/protocol")>();
  return {
    ...actual,
    ocrImageUrl: (a: { doc: string; gen: number; page: number; dpi?: number }) =>
      `seepdf://localhost/ocr?doc=${a.doc}&gen=${a.gen}&page=${a.page}&dpi=${a.dpi ?? 300}`,
  };
});

/** jsdom has no canvas: a turned image is a tagged blob, sized like the real thing. */
vi.mock("./orientation", async (importOriginal) => {
  const actual = await importOriginal<typeof import("./orientation")>();
  return {
    ...actual,
    turnImage: async (blob: Blob, widthPx: number, heightPx: number, delta: Rotation) => {
      const sideways = delta === 90 || delta === 270;
      const turned = new Blob([blob], { type: "image/png" }) as Blob & { turn?: number };
      turned.turn = ((blob as Blob & { turn?: number }).turn ?? 0) + delta;
      return { blob: turned, widthPx: sideways ? heightPx : widthPx, heightPx: sideways ? widthPx : heightPx };
    },
  };
});

/**
 * The fake recogniser reads a page scanned sideways: only a quarter turn clockwise (`upright`)
 * gives confident words.
 */
let upright = 90;
const recognized: { turn: number; dpi?: number }[] = [];

vi.mock("./tesseractPool", async (importOriginal) => {
  const actual = await importOriginal<typeof import("./tesseractPool")>();
  return {
    ...actual,
    TesseractPool: class {
      constructor(readonly options: Record<string, unknown>) {}
      get maxWorkers() { return 2; }
      async recognize(image: Blob & { turn?: number }, req: { dpi?: number } = {}) {
        const turn = (image.turn ?? 0) % 360;
        recognized.push({ turn, dpi: req.dpi });
        return turn === upright ? tessPage("검색 가능한 한글 문서", 92) : tessPage("ㅁ;; ,ㄱ Il1 ~~", 38);
      }
      terminate() { return Promise.resolve(); }
    },
  };
});

import { DETECT_DPI, detectWithTesseract, pickOrientation, scoreOf } from "./orientation";
import { runOcrJob } from "./ocrJob";

function tessPage(text: string, confidence: number) {
  return {
    blocks: [{
      bbox: { x0: 0, y0: 0, x1: 2480, y1: 3508 },
      paragraphs: [{
        lines: [{
          text, confidence,
          bbox: { x0: 100, y0: 100, x1: 900, y1: 160 },
          rowAttributes: { ascenders: 12, descenders: -6, rowHeight: 60 },
          words: text.split(" ").map((w, i) => ({
            text: w, confidence, bbox: { x0: 100 + i * 150, y0: 100, x1: 220 + i * 150, y1: 160 },
          })),
        }],
      }],
    }],
    text,
  };
}

function docInfo(generation: number): DocInfo {
  return {
    docId: "d1", path: null, name: "t.pdf", bytes: 1, pageCount: 3, pages: [],
    docGeneration: generation, dirty: true, canUndo: true, canRedo: false,
    undoLabel: "ocr.title", redoLabel: null, encrypted: false,
    permissions: {
      print: true, modify: true, extractText: true, annotate: true,
      fillForms: true, assemble: true, revision: "unprotected",
    },
    hasForm: false, xfa: false, hasOutline: false, meta: {}, pdfVersion: "1.7", tagged: false,
  };
}

function nativePage(page: PageIndex, rotation: Rotation, dpi = 300): OcrPage {
  const sideways = rotation === 90 || rotation === 270;
  return {
    page, dpi, widthPx: sideways ? 3508 : 2480, heightPx: sideways ? 2480 : 3508, rotation,
    lines: [{
      text: "검색", bbox: [100, 100, 220, 160], rowHeightPx: 60,
      words: [{ text: "검색", bbox: [100, 100, 220, 160], confidence: 100 }],
    }],
  };
}

const fetchedDpi: number[] = [];

beforeEach(() => {
  upright = 90;
  recognized.length = 0;
  fetchedDpi.length = 0;
  let gen = 1;
  ocrPageStatus.mockReset().mockImplementation(async (a: { pages: PageIndex[] }) =>
    a.pages.map((page) => ({ page, hasText: false, charCount: 0 })));
  ocrApply.mockReset().mockImplementation(async (_a: unknown, onProgress: (e: JobEvent) => void) => {
    onProgress({ type: "started", jobId: 7, total: 1 });
    onProgress({ type: "done", jobId: 7, elapsedMs: 1 });
    return docInfo(++gen);
  });
  ocrRecognizeNative.mockReset().mockImplementation(
    async (a: { page: PageIndex; dpi: number; rotate?: Rotation }) => nativePage(a.page, (a.rotate ?? 0) as Rotation, a.dpi));
  ocrDetectOrientation.mockReset().mockImplementation(async () => ({ rotation: 90, scores: [] }));
  useJobStore.setState({ jobs: [], active: null });
  vi.stubGlobal("fetch", vi.fn(async (url: string) => {
    fetchedDpi.push(Number(/dpi=(\d+)/.exec(url)?.[1] ?? -1));
    return {
      ok: true, status: 200,
      headers: { get: (k: string) => (k === "X-Image-Width" ? "2480" : k === "X-Image-Height" ? "3508" : null) },
      arrayBuffer: async () => new Uint8Array([1, 2, 3, 4]).buffer,
    } as unknown as Response;
  }));
});

const s = (rotation: Rotation, confidence: number, words: number): OrientationScore => ({ rotation, confidence, words });

describe("ocr.orientation.pick", () => {
  it("a clear winner turns the page; an upright page stays", () => {
    expect(pickOrientation([s(0, 40, 12), s(90, 91, 60), s(180, 35, 9), s(270, 42, 14)])).toBe(90);
    expect(pickOrientation([s(0, 92, 80), s(90, 30, 5), s(180, 45, 20), s(270, 33, 6)])).toBe(0);
  });

  it("needs a 15 % margin over the runner-up and over the page as it is", () => {
    expect(pickOrientation([s(0, 80, 40), s(90, 20, 4), s(180, 88, 40), s(270, 10, 3)])).toBe(0);
    expect(pickOrientation([s(0, 0, 0), s(90, 80, 10), s(180, 20, 5), s(270, 85, 10)])).toBe(0);
    expect(pickOrientation([s(0, 60, 10), s(180, 70, 10)])).toBe(180);
  });

  it("too few words is no evidence", () => {
    expect(pickOrientation([s(0, 0, 0), s(90, 95, 2), s(180, 0, 0), s(270, 0, 0)])).toBe(0);
    expect(pickOrientation([])).toBe(0);
  });

  it("the Rust twin agrees (tests/ocr.rs ocr_orientation_pick_matches_the_frontend_rule)", () => {
    expect(pickOrientation([s(0, 31, 6), s(90, 88, 40), s(180, 29, 4), s(270, 35, 7)])).toBe(90);
    expect(pickOrientation([s(0, 88, 40), s(90, 31, 6), s(180, 29, 4), s(270, 35, 7)])).toBe(0);
    expect(pickOrientation([s(0, 80, 40), s(90, 85, 40)])).toBe(0);
  });

  it("scores a page by mean word confidence", () => {
    expect(scoreOf(90, nativePage(0, 90))).toEqual({ rotation: 90, confidence: 100, words: 1 });
  });
});

describe("ocr.orientation.detect", () => {
  it("tesseract reads the page four ways at 100 DPI and picks the confident one", async () => {
    const { turnImage } = await import("./orientation");
    const r = await detectWithTesseract({
      page: 0, image: new Blob([new Uint8Array([1])]), widthPx: 827, heightPx: 1169, turn: turnImage,
      recognize: async (image, dpi) => {
        const turn = ((image as Blob & { turn?: number }).turn ?? 0) % 360;
        recognized.push({ turn, dpi });
        return turn === 270 ? tessPage("검색 가능한 한글 문서", 90) : tessPage("~~ ;; ||", 35);
      },
    });
    expect(r.rotation).toBe(270);
    expect(recognized.map((x) => x.turn)).toEqual([0, 90, 180, 270]);
    expect(recognized.every((x) => x.dpi === DETECT_DPI)).toBe(true);
  });
});

describe("ocr.orientation.job", () => {
  it("tesseract: detect at 100 DPI, recognise turned, apply with setRotation in the same call", async () => {
    const r = await runOcrJob({
      docId: "d1", docGeneration: 1, pages: [0], engine: "tesseract", autoRotate: true, workers: 1,
      pageGeom: [{ index: 0, widthPt: 595, heightPt: 842, rotation: 0 } as never],
    });
    expect(r.status).toBe("done");
    expect(r.rotated).toEqual([0]);
    // 4 trial reads at 100 DPI, then the 300 DPI read turned the winning way.
    expect(fetchedDpi).toEqual([100, 300]);
    expect(recognized.map((x) => x.turn)).toEqual([0, 90, 180, 270, 90]);
    expect(ocrApply).toHaveBeenCalledTimes(1);
    const [entry] = (ocrApply.mock.calls[0][0] as { pages: unknown[] }).pages as {
      page: number; setRotation: Rotation; ocr: OcrPage;
    }[];
    expect(entry.page).toBe(0);
    expect(entry.setRotation).toBe(90);
    expect(entry.ocr.rotation).toBe(90);
    // the turned image is landscape
    expect([entry.ocr.widthPx, entry.ocr.heightPx]).toEqual([3508, 2480]);
  });

  it("an upright page is applied plain, without setRotation", async () => {
    upright = 0;
    const r = await runOcrJob({ docId: "d1", docGeneration: 1, pages: [0], engine: "tesseract", autoRotate: true });
    expect(r.rotated).toEqual([]);
    const pages = (ocrApply.mock.calls[0][0] as { pages: OcrPage[] }).pages;
    expect(pages[0]).not.toHaveProperty("setRotation");
    expect(pages[0].rotation).toBe(0);
  });

  it("off by default: no detection at all", async () => {
    await runOcrJob({ docId: "d1", docGeneration: 1, pages: [0], engine: "tesseract" });
    expect(fetchedDpi).toEqual([300]);
    expect(recognized).toHaveLength(1);
  });

  it("vision: the backend detects, recognises turned, and the apply carries setRotation", async () => {
    const r = await runOcrJob({ docId: "d1", docGeneration: 1, pages: [0, 1], engine: "vision", autoRotate: true, applyOnce: true });
    expect(r.rotated).toEqual([0, 1]);
    expect(ocrDetectOrientation).toHaveBeenCalledTimes(2);
    expect(ocrRecognizeNative.mock.calls[0][0]).toMatchObject({ page: 0, rotate: 90 });
    expect(recognized).toEqual([]);   // no tesseract at all
    const pages = (ocrApply.mock.calls[0][0] as { pages: { setRotation?: number }[] }).pages;
    expect(pages.map((p) => p.setRotation)).toEqual([90, 90]);
  });

  it("a failed detection leaves the page as it is", async () => {
    ocrDetectOrientation.mockRejectedValue(new Error("Vision hiccup"));
    const r = await runOcrJob({ docId: "d1", docGeneration: 1, pages: [0], engine: "vision", autoRotate: true });
    expect(r.status).toBe("done");
    expect(r.rotated).toEqual([]);
    expect(ocrRecognizeNative.mock.calls[0][0]).not.toHaveProperty("rotate");
  });

  it("windows: recognition is native, detection is tesseract's", async () => {
    const r = await runOcrJob({ docId: "d1", docGeneration: 1, pages: [0], engine: "windows", autoRotate: true });
    expect(r.rotated).toEqual([0]);
    expect(ocrDetectOrientation).not.toHaveBeenCalled();
    expect(recognized.map((x) => x.turn)).toEqual([0, 90, 180, 270]);
    expect(ocrRecognizeNative.mock.calls[0][0]).toMatchObject({ page: 0, rotate: 90 });
  });
});
