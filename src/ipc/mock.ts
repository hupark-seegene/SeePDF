/**
 * Mock adapter (IPC_CONTRACT §13) — the whole command surface against fixture JSON, in memory.
 * Selected by `VITE_SEEPDF_MOCK=1` (and automatically outside Tauri), so every frontend module runs
 * and is testable in plain `vite dev` before the backend lands.
 *
 * What is real here: document/page model, generations + dirty + undo/redo, a binary text layer with
 * genuine word boxes (so selection and search can be exercised), streaming search, annotations,
 * form fields, page operations, job progress with cancel, recents and settings.
 * What is faked: pixels (generated PNG data URLs instead of `seepdf://`), timings, file system.
 */
import { appBus } from "./bus";
import { setMockAssetResolver } from "./protocol";
import { encodeRawPage, encodeTextLayer, CHAR_SPACE, type TextChar, type TextLine, type TextWord } from "./binary";
import { pngDataUrl, solidPngDataUrl, type RgbaPixel } from "./png";
import type {
  Annot, AnnotList, AnnotPatch, AnnotResult, AnnotScanEvent, AnnotSpec, DocGeneration, DocId, DocInfo, EngineError,
  EngineStats, ExportImagesArgs, FieldValue, FormField, JobEvent, JobId, Mat6, MergeWarning, OcrPage, OutlineNode,
  PageGeom, PageIndex, PageObject, PageOp, RecentEntry, Rect, RedactPreview, SaveResult, SearchEvent, SearchHit,
  Settings, TextEditProbe, ViewportHint,
} from "./types";

import documentFixture from "../test/ipc-samples/document.json";
import textFixture from "../test/ipc-samples/text.json";
import outlineFixture from "../test/ipc-samples/outline.json";
import annotsFixture from "../test/ipc-samples/annots.json";
import fieldsFixture from "../test/ipc-samples/form-fields.json";
import recentsFixture from "../test/ipc-samples/recents.json";
import settingsFixture from "../test/ipc-samples/settings.json";

// Broadcasts go through the shared bus (`events.ts` listens on it in mock mode).
export const mockEvents = appBus;

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

interface Snapshot { info: DocInfo; annots: [PageIndex, Annot[]][]; fields: FormField[] }

interface MockDoc {
  info: DocInfo;
  outline: OutlineNode[];
  annots: Map<PageIndex, Annot[]>;
  fields: FormField[];
  undo: Snapshot[];
  redo: Snapshot[];
}

const docs = new Map<DocId, MockDoc>();
const jobs = new Map<JobId, { cancel: () => void }>();
let nextDoc = 1;
let nextJob = 1;
let nextAnnot = 1;
let recents: RecentEntry[] = structuredClone(recentsFixture) as unknown as RecentEntry[];
let settings: Settings = structuredClone(settingsFixture) as unknown as Settings;
const pendingOpens: { path: string; source: "argv" | "macos-opened" | "drop" | "dialog" | "recent" }[] = [];

function err(code: EngineError["code"], message: string, extra: Partial<EngineError> = {}): EngineError {
  return { code, message, ...extra };
}

function doc(docId: DocId): MockDoc {
  const d = docs.get(docId);
  if (!d) throw err("notFound", `unknown document ${docId}`);
  return d;
}

function delay<T>(value: T, ms = 12): Promise<T> {
  return new Promise((resolve) => setTimeout(() => resolve(value), ms));
}

function snapshot(d: MockDoc): Snapshot {
  return {
    info: structuredClone(d.info),
    annots: [...d.annots.entries()].map(([p, a]) => [p, structuredClone(a)] as [PageIndex, Annot[]]),
    fields: structuredClone(d.fields),
  };
}

function restore(d: MockDoc, s: Snapshot): void {
  d.info = structuredClone(s.info);
  d.annots = new Map(s.annots.map(([p, a]) => [p, structuredClone(a)]));
  d.fields = structuredClone(s.fields);
}

type ChangeReason = "edit" | "undo" | "redo" | "save" | "pages" | "ocr" | "redact";

/** One mutation = one generation = one undo step (IPC_CONTRACT §3). */
function mutate<T>(
  d: MockDoc,
  opts: { reason: ChangeReason; pages: PageIndex[] | "all"; structure?: boolean; undoLabel?: string; dirty?: boolean },
  body: () => T,
): T {
  d.undo.push(snapshot(d));
  d.redo.length = 0;
  const out = body();
  d.info.docGeneration += 1;
  d.info.dirty = opts.dirty ?? true;
  d.info.canUndo = d.undo.length > 0;
  d.info.canRedo = d.redo.length > 0;
  d.info.undoLabel = opts.undoLabel ?? "menu.edit.undo";
  d.info.redoLabel = null;
  mockEvents.emit("doc-changed", {
    docId: d.info.docId,
    docGeneration: d.info.docGeneration,
    changedPages: opts.pages,
    structure: opts.structure ?? false,
    dirty: d.info.dirty,
    reason: opts.reason,
    canUndo: d.info.canUndo,
    canRedo: d.info.canRedo,
  });
  return out;
}

// ---------------------------------------------------------------------------
// Document construction
// ---------------------------------------------------------------------------

const BASE_DOC = documentFixture as unknown as DocInfo;

function baseName(path: string): string {
  const parts = path.split(/[\\/]/);
  return parts[parts.length - 1] || path;
}

function dirName(path: string): string {
  const parts = path.split(/[\\/]/);
  parts.pop();
  return parts.join("/") || "/";
}

function makePages(count: number): PageGeom[] {
  const template = BASE_DOC.pages[0];
  return Array.from({ length: count }, (_, index) => ({ ...structuredClone(template), index }));
}

function makeDoc(path: string, pageCount = BASE_DOC.pageCount): MockDoc {
  const docId = `d${nextDoc++}`;
  const info: DocInfo = {
    ...structuredClone(BASE_DOC),
    docId,
    path,
    name: baseName(path),
    pageCount,
    pages: makePages(pageCount),
    docGeneration: 1,
    dirty: false,
    canUndo: false,
    canRedo: false,
    undoLabel: null,
    redoLabel: null,
  };
  const annots = new Map<PageIndex, Annot[]>();
  for (const a of structuredClone(annotsFixture) as unknown as Annot[]) {
    if (a.page >= pageCount) continue;
    annots.set(a.page, [...(annots.get(a.page) ?? []), a]);
  }
  return {
    info,
    outline: structuredClone(outlineFixture) as unknown as OutlineNode[],
    annots,
    fields: (structuredClone(fieldsFixture) as unknown as FormField[]).filter((f) => f.page < pageCount),
    undo: [],
    redo: [],
  };
}

// ---------------------------------------------------------------------------
// Fake text layer with real word boxes
// ---------------------------------------------------------------------------

interface FixtureLine { text: string; x: number; y: number; size: number }
const TEXT_PAGES = (textFixture as { pages: { index: number; lines: FixtureLine[] }[] }).pages;

interface MockTextPage {
  chars: TextChar[];
  words: TextWord[];
  lines: TextLine[];
  text: string;
  /** index into `text` for every char record (they differ: the text carries "\r\n" line breaks) */
  charTextOffset: number[];
}

const textCache = new Map<string, MockTextPage>();

function isWide(ch: string): boolean {
  const c = ch.codePointAt(0) ?? 0;
  return c >= 0x1100 && c <= 0xffe6; // CJK / Hangul — one em wide
}

function buildTextPage(page: PageIndex): MockTextPage {
  const fixture = TEXT_PAGES[page % TEXT_PAGES.length];
  const chars: TextChar[] = [];
  const words: TextWord[] = [];
  const lines: TextLine[] = [];
  const charTextOffset: number[] = [];
  let text = "";

  for (const line of fixture.lines) {
    const firstChar = chars.length;
    const firstWord = words.length;
    let x = line.x;
    let wordStart = chars.length;
    let wordChars = 0;
    const flush = () => {
      if (wordChars > 0) words.push({ firstChar: wordStart, charCount: wordChars, lineIndex: lines.length });
      wordChars = 0;
    };
    for (const ch of [...line.text]) {
      const advance = line.size * (ch === " " ? 0.32 : isWide(ch) ? 1 : 0.52);
      charTextOffset.push(text.length);
      text += ch;
      chars.push({
        code: ch.codePointAt(0) ?? 32,
        box: { l: x, b: line.y - line.size * 0.24, r: x + advance, t: line.y + line.size * 0.78 },
        baselineY: line.y,
        fontSizePt: line.size,
        flags: ch === " " ? CHAR_SPACE : 0,
        objectId: page * 100 + lines.length,
      });
      if (ch === " ") {
        flush();
        wordStart = chars.length;
      } else {
        if (wordChars === 0) wordStart = chars.length - 1;
        wordChars += 1;
      }
      x += advance;
    }
    flush();
    lines.push({
      firstWord,
      wordCount: words.length - firstWord,
      baselineY: line.y,
      box: {
        l: line.x,
        b: line.y - line.size * 0.24,
        r: x,
        t: line.y + line.size * 0.78,
      },
      firstChar,
      charCount: chars.length - firstChar,
    });
    text += "\r\n";
  }
  return { chars, words, lines, text, charTextOffset };
}

function textPage(d: MockDoc, page: PageIndex): MockTextPage {
  const key = `${d.info.docId}:${d.info.docGeneration}:${page}`;
  const hit = textCache.get(key);
  if (hit) return hit;
  const built = buildTextPage(page);
  textCache.set(key, built);
  return built;
}

/** page user space -> device px at s = 1, rotation 0 (IPC_CONTRACT §3). */
function identityMatrix(geom: PageGeom): Mat6 {
  return [1, 0, 0, -1, -geom.crop.l, geom.crop.t];
}

// ---------------------------------------------------------------------------
// Generated page images (data URLs) — `protocol.ts` routes here in mock mode
// ---------------------------------------------------------------------------

const imageCache = new Map<string, string>();
const PAGE_BG: RgbaPixel = [255, 255, 255, 255];

function pageImage(docId: DocId, page: PageIndex, widthPx: number, gray = false): string {
  const key = `${docId}:${page}:${widthPx}:${gray ? "g" : "c"}`;
  const hit = imageCache.get(key);
  if (hit) return hit;
  const d = docs.get(docId);
  const geom = d?.info.pages[page] ?? BASE_DOC.pages[0];
  // capped: the mock encodes PNG in JS, and a data URL is ~1.33× the raw pixels
  const w = Math.max(8, Math.min(widthPx, 340));
  const h = Math.max(8, Math.round((w * geom.heightPt) / geom.widthPt));
  const tp = buildTextPage(page);
  const bars = tp.lines.map((l) => l.box);
  const url = pngDataUrl(w, h, (x, y) => {
    const px = (x / w) * geom.widthPt;
    const py = (1 - y / h) * geom.heightPt;
    for (const b of bars) {
      if (px >= b.l && px <= b.r && py >= b.b && py <= b.t) {
        const v = gray ? 90 : 96;
        return [v, v, v + (gray ? 0 : 6), 255];
      }
    }
    if (x < 1 || y < 1 || x >= w - 1 || y >= h - 1) return [222, 224, 228, 255];
    return PAGE_BG;
  });
  imageCache.set(key, url);
  return url;
}

/** Answers the `seepdf://` routes of IPC_CONTRACT §9 with `data:` URLs. */
export function mockAssetUrl(route: string, query: Record<string, string | number | boolean | undefined>): string {
  const docId = String(query.doc ?? "");
  const page = Number(query.page ?? 0);
  switch (route.replace(/^\//, "")) {
    case "tile":
    case "page": {
      const sk = Number(query.sk ?? 100);
      return pageImage(docId, page, Math.round((sk / 100) * 220));
    }
    case "thumb":
      return pageImage(docId, page, Math.min(Number(query.w ?? 120), 160));
    case "ocr":
      return pageImage(docId, page, 200, true);
    case "recent-thumb":
      return recentThumbImage(String(query.id ?? ""));
    case "raw":
      return "data:application/pdf;base64,JVBERi0xLjcK";
    default:
      return solidPngDataUrl(1, 1, [255, 255, 255, 255]);
  }
}

function recentThumbImage(id: string): string {
  const key = `recent:${id}`;
  const hit = imageCache.get(key);
  if (hit) return hit;
  const seed = [...id].reduce((n, c) => n + c.charCodeAt(0), 0);
  const url = pngDataUrl(90, 120, (x, y) => {
    if (x < 1 || y < 1 || x >= 89 || y >= 119) return [220, 222, 226, 255];
    const line = Math.floor((y - 12) / 9);
    const width = 20 + ((seed * (line + 3)) % 50);
    if (y > 12 && (y - 12) % 9 < 3 && x > 10 && x < 10 + width) return [150, 155, 162, 255];
    return [255, 255, 255, 255];
  });
  imageCache.set(key, url);
  return url;
}

// ---------------------------------------------------------------------------
// Jobs
// ---------------------------------------------------------------------------

function runJob(
  total: number,
  onEvent: (e: JobEvent) => void,
  opts: { stepMs?: number; note?: (done: number) => string; outputs?: string[]; onDone?: () => void } = {},
): JobId {
  const jobId = nextJob++;
  const started = Date.now();
  let done = 0;
  let cancelled = false;
  const stepMs = opts.stepMs ?? 90;
  onEvent({ type: "started", jobId, total });
  const timer = setInterval(() => {
    if (cancelled) return;
    done += 1;
    if (done >= total) {
      clearInterval(timer);
      jobs.delete(jobId);
      opts.onDone?.();
      onEvent({ type: "done", jobId, elapsedMs: Date.now() - started, outputs: opts.outputs });
      return;
    }
    onEvent({ type: "progress", jobId, done, total, page: done, note: opts.note?.(done) });
  }, stepMs);
  jobs.set(jobId, {
    cancel: () => {
      cancelled = true;
      clearInterval(timer);
      jobs.delete(jobId);
      onEvent({ type: "cancelled", jobId, done });
    },
  });
  return jobId;
}

// ---------------------------------------------------------------------------
// The command surface (same names as `api.ts`)
// ---------------------------------------------------------------------------

export const mock = {
  // 4. documents -------------------------------------------------------------
  async openDocument(a: { path: string; password?: string }): Promise<DocInfo> {
    if (/encrypted/i.test(a.path) && !a.password) throw err("passwordRequired", "document is encrypted");
    const recent = recents.find((r) => r.path === a.path);
    const d = makeDoc(a.path, recent?.pages ?? BASE_DOC.pageCount);
    docs.set(d.info.docId, d);
    const entry: RecentEntry = recent ?? {
      path: a.path, name: baseName(a.path), dir: dirName(a.path), pages: d.info.pageCount, bytes: d.info.bytes,
      lastOpened: new Date().toISOString(), lastPage: 0, zoomPercent: 100, layout: "continuous", pinned: false,
      thumbId: null,
    };
    entry.lastOpened = new Date().toISOString();
    recents = [entry, ...recents.filter((r) => r.path !== a.path)];
    mockEvents.emit("recents-changed", {});
    return delay(structuredClone(d.info), 40);
  },
  async closeDocument(a: { docId: DocId }): Promise<void> {
    docs.delete(a.docId);
  },
  async getDocument(a: { docId: DocId }): Promise<DocInfo> {
    return structuredClone(doc(a.docId).info);
  },
  async getOutline(a: { docId: DocId }): Promise<OutlineNode[]> {
    return delay(structuredClone(doc(a.docId).outline), 20);
  },
  async takePendingOpens() {
    return pendingOpens.splice(0, pendingOpens.length);
  },
  async openInNewWindow(_a: { path?: string }): Promise<string> {
    return `doc-${nextDoc}`;
  },
  async windowBindDocument(_a: { label: string; docId: DocId | null }): Promise<void> {},

  // 5. view / stats ----------------------------------------------------------
  async setViewport(_a: ViewportHint): Promise<void> {},
  async renderPageRaw(a: { docId: DocId; page: PageIndex; scale: number; rect?: Rect }): Promise<ArrayBuffer> {
    const geom = doc(a.docId).info.pages[a.page];
    if (!geom) throw err("notFound", `page ${a.page}`);
    const w = Math.max(1, Math.min(Math.round(geom.widthPt * a.scale), 400));
    const h = Math.max(1, Math.round((w * geom.heightPt) / geom.widthPt));
    const px = new Uint8ClampedArray(w * h * 4).fill(255);
    return encodeRawPage(w, h, px);
  },
  async engineStats(): Promise<EngineStats> {
    return {
      docs: docs.size,
      queueDepth: { interactive: 0, edit: 0, prefetch: 0, thumb: 0, background: 0 },
      droppedStale: 0, tileP50Ms: 3.1, tileP95Ms: 9.4, encodeP50Ms: 2.3,
      tileCacheBytes: 12 * 1024 * 1024, tileCacheHitRate: 0.86, pageLruLen: 6, rssBytes: 180 * 1024 * 1024,
    };
  },

  // 6. text / search ---------------------------------------------------------
  async getTextLayer(a: { docId: DocId; page: PageIndex }): Promise<ArrayBuffer> {
    const d = doc(a.docId);
    const geom = d.info.pages[a.page];
    if (!geom) throw err("notFound", `page ${a.page}`);
    const tp = textPage(d, a.page);
    return encodeTextLayer({
      pageIndex: a.page,
      matrix: identityMatrix(geom),
      crop: geom.crop,
      chars: tp.chars,
      words: tp.words,
      lines: tp.lines,
      text: tp.text,
      hasObjectIds: true,
    });
  },
  async getPageText(a: { docId: DocId; page: PageIndex }): Promise<string> {
    return textPage(doc(a.docId), a.page).text;
  },
  async searchStart(
    a: { docId: DocId; query: string; matchCase: boolean; wholeWord: boolean; fromPage: PageIndex },
    onEvent: (e: SearchEvent) => void,
  ): Promise<JobId> {
    const d = doc(a.docId);
    const jobId = nextJob++;
    const order: PageIndex[] = [];
    for (let i = 0; i < d.info.pageCount; i++) order.push((a.fromPage + i) % d.info.pageCount);
    const started = Date.now();
    let scanned = 0;
    let total = 0;
    let cancelled = false;
    const step = () => {
      if (cancelled) return;
      const page = order[scanned];
      const hits = searchPage(d, page, a.query, a.matchCase, a.wholeWord);
      scanned += 1;
      total += hits.length;
      onEvent({ type: "page", page, hits, scanned, total });
      if (scanned >= order.length) {
        jobs.delete(jobId);
        onEvent({ type: "done", total, pagesScanned: scanned, elapsedMs: Date.now() - started });
      } else {
        timer = setTimeout(step, 24);
      }
    };
    let timer = setTimeout(step, 8);
    jobs.set(jobId, {
      cancel: () => {
        cancelled = true;
        clearTimeout(timer);
        jobs.delete(jobId);
        onEvent({ type: "cancelled", scanned });
      },
    });
    return jobId;
  },
  async cancelJob(a: { jobId: JobId }): Promise<boolean> {
    const job = jobs.get(a.jobId);
    if (!job) return false;
    job.cancel();
    return true;
  },

  // 7.1 annotations ----------------------------------------------------------
  async listAnnotations(a: { docId: DocId; page: PageIndex }): Promise<AnnotList> {
    const d = doc(a.docId);
    return {
      docId: a.docId, page: a.page, docGeneration: d.info.docGeneration,
      annots: structuredClone(d.annots.get(a.page) ?? []),
    };
  },
  async scanAnnotations(a: { docId: DocId }, onEvent: (e: AnnotScanEvent) => void): Promise<JobId> {
    const d = doc(a.docId);
    let total = 0;
    return runJob(d.info.pageCount, (e) => {
      if (e.type === "progress") {
        const page = e.done - 1;
        const annots = structuredClone(d.annots.get(page) ?? []);
        total += annots.length;
        onEvent({ type: "page", page, annots });
      } else if (e.type === "done") {
        onEvent({ type: "done", total });
      } else if (e.type === "cancelled") {
        onEvent({ type: "cancelled" });
      }
    }, { stepMs: 20 });
  },
  async createAnnotation(a: { docId: DocId; page: PageIndex; spec: AnnotSpec; id?: string }): Promise<AnnotResult> {
    const d = doc(a.docId);
    const annot = annotFromSpec(a.page, a.spec, a.id ?? `mock-${nextAnnot++}`, settings.author);
    return mutate(d, { reason: "edit", pages: [a.page], undoLabel: `tool.${annot.kind}` }, () => {
      d.annots.set(a.page, [...(d.annots.get(a.page) ?? []), annot]);
      return { list: listOf(d, a.page), annot: structuredClone(annot), previous: null };
    });
  },
  async updateAnnotation(a: { docId: DocId; page: PageIndex; id: string; patch: AnnotPatch }): Promise<AnnotResult> {
    const d = doc(a.docId);
    const list = d.annots.get(a.page) ?? [];
    const idx = list.findIndex((x) => x.id === a.id);
    if (idx < 0) throw err("notFound", `annotation ${a.id}`);
    const previous = structuredClone(list[idx]);
    return mutate(d, { reason: "edit", pages: [a.page] }, () => {
      const next: Annot = { ...list[idx], ...a.patch, modified: new Date().toISOString() } as Annot;
      list[idx] = next;
      d.annots.set(a.page, list);
      return { list: listOf(d, a.page), annot: structuredClone(next), previous };
    });
  },
  async deleteAnnotations(a: { docId: DocId; page: PageIndex; ids: string[] }): Promise<AnnotResult> {
    const d = doc(a.docId);
    return mutate(d, { reason: "edit", pages: [a.page], undoLabel: "menu.edit.delete" }, () => {
      const kept = (d.annots.get(a.page) ?? []).filter((x) => !a.ids.includes(x.id));
      d.annots.set(a.page, kept);
      return { list: listOf(d, a.page), annot: null, previous: null };
    });
  },
  async setAnnotationsHidden(_a: { docId: DocId; page: PageIndex; ids: string[]; hidden: boolean }) {
    return { viewNonce: Date.now() };
  },

  // 7.2 forms ----------------------------------------------------------------
  async listFormFields(a: { docId: DocId; page?: PageIndex }): Promise<FormField[]> {
    const d = doc(a.docId);
    return structuredClone(a.page === undefined ? d.fields : d.fields.filter((f) => f.page === a.page));
  },
  async setFormFieldValue(a: { docId: DocId; page: PageIndex; index: number; value: FieldValue }) {
    const d = doc(a.docId);
    const field = d.fields.find((f) => f.page === a.page && f.index === a.index);
    if (!field) throw err("notFound", `field ${a.index}`);
    const previous = field.value;
    return mutate(d, { reason: "edit", pages: [a.page] }, () => {
      const value = a.value;
      if ("text" in value) field.value = value.text;
      else if ("checked" in value) {
        field.checked = value.checked;
        field.value = value.checked ? "On" : "Off";
      } else if (field.options) {
        field.options = field.options.map((o, i) => ({ ...o, selected: value.selected.includes(i) }));
        field.value = field.options.find((o) => o.selected)?.label ?? null;
      }
      return { field: structuredClone(field), previous, docGeneration: d.info.docGeneration + 1 };
    });
  },
  async resetForm(a: { docId: DocId }): Promise<DocInfo> {
    const d = doc(a.docId);
    return mutate(d, { reason: "edit", pages: "all" }, () => {
      d.fields = d.fields.map((f) => ({ ...f, value: f.type === "checkbox" ? "Off" : "", checked: false }));
      return structuredClone(d.info);
    });
  },

  // 7.3 pages ----------------------------------------------------------------
  async pageOps(a: { docId: DocId; ops: PageOp[] }): Promise<DocInfo> {
    const d = doc(a.docId);
    return mutate(d, { reason: "pages", pages: "all", structure: true, undoLabel: "pages.title" }, () => {
      for (const op of a.ops) applyPageOp(d, op);
      d.info.pages = d.info.pages.map((p, index) => ({ ...p, index }));
      d.info.pageCount = d.info.pages.length;
      return structuredClone(d.info);
    });
  },
  async extractPages(a: { docId: DocId; pages: PageIndex[]; outPath: string; removeAfter: boolean }) {
    const d = doc(a.docId);
    if (a.removeAfter) await mock.pageOps({ docId: a.docId, ops: [{ kind: "delete", pages: a.pages }] });
    return { bytes: 120_000 * Math.max(1, a.pages.length), docGeneration: d.info.docGeneration };
  },
  async splitDocument(
    a: { docId: DocId; mode: { everyN: number } | { ranges: string[] }; outDir: string },
    onProgress: (e: JobEvent) => void,
  ): Promise<JobId> {
    const d = doc(a.docId);
    const parts = "everyN" in a.mode ? Math.ceil(d.info.pageCount / a.mode.everyN) : a.mode.ranges.length;
    return runJob(Math.max(1, parts), onProgress, {
      stepMs: 140,
      outputs: Array.from({ length: Math.max(1, parts) }, (_, i) => `${a.outDir}/${d.info.name}-${i + 1}.pdf`),
    });
  },
  async mergeDocuments(a: { inputs: { path: string; range?: string; password?: string }[] }) {
    const merged = makeDoc(a.inputs[0]?.path ?? "merged.pdf", a.inputs.length * BASE_DOC.pageCount);
    merged.info.path = null;
    merged.info.name = "merged.pdf";
    merged.info.dirty = true;
    docs.set(merged.info.docId, merged);
    const warnings: MergeWarning[] = ["outlineDropped"];
    return { info: structuredClone(merged.info), warnings };
  },

  // 7.4 page objects ---------------------------------------------------------
  async listPageObjects(a: { docId: DocId; page: PageIndex }) {
    const d = doc(a.docId);
    const tp = textPage(d, a.page);
    const objects: PageObject[] = tp.lines.map((l, i) => ({
      objectId: i,
      type: "text",
      rect: l.box,
      matrix: [1, 0, 0, 1, l.box.l, l.baselineY] as Mat6,
      ours: false,
      text: sliceLineText(tp, i),
      fontName: "Pretendard",
      fontSizePt: 11,
      color: [26, 28, 31],
      editable: i === 0 ? "readOnly" : "full",
      reason: i === 0 ? "insideXObject" : undefined,
    }));
    return { docGeneration: d.info.docGeneration, objects };
  },
  async probeTextEdit(a: { docId: DocId; page: PageIndex; objectId: number; text: string }): Promise<TextEditProbe> {
    if (a.objectId === 0) return { strategy: "refused", reason: "insideXObject" };
    return /[ᄀ-힯]/.test(a.text)
      ? { strategy: "replaceFont", substituteFont: "SeePDF Hangul" }
      : { strategy: "inPlace" };
  },
  async editTextObject(a: { docId: DocId; page: PageIndex; objectId: number; expectGeneration: DocGeneration;
    patch: { text?: string }; allowFontSubstitution: boolean }) {
    const d = doc(a.docId);
    if (a.expectGeneration !== d.info.docGeneration) throw err("stale", "generation moved on");
    if (!a.allowFontSubstitution && /[ᄀ-힯]/.test(a.patch.text ?? "")) {
      throw err("fontCoverage", "embedded font cannot render the requested text");
    }
    return mutate(d, { reason: "edit", pages: [a.page] }, async () => mock.listPageObjects(a));
  },
  async addTextObject(a: { docId: DocId; page: PageIndex }) {
    const d = doc(a.docId);
    return mutate(d, { reason: "edit", pages: [a.page], undoLabel: "tool.addText" }, () => mock.listPageObjects(a));
  },
  async addImageObject(a: { docId: DocId; page: PageIndex }) {
    const d = doc(a.docId);
    return mutate(d, { reason: "edit", pages: [a.page], undoLabel: "tool.addImage" }, () => mock.listPageObjects(a));
  },
  async transformObject(a: { docId: DocId; page: PageIndex }) {
    const d = doc(a.docId);
    return mutate(d, { reason: "edit", pages: [a.page] }, () => mock.listPageObjects(a));
  },
  async deleteObjects(a: { docId: DocId; page: PageIndex }) {
    const d = doc(a.docId);
    return mutate(d, { reason: "edit", pages: [a.page], undoLabel: "menu.edit.delete" }, () => mock.listPageObjects(a));
  },

  // 7.5 redaction / security -------------------------------------------------
  async redactPreview(a: { docId: DocId; page: PageIndex; rects: Rect[] }): Promise<RedactPreview> {
    const d = doc(a.docId);
    const tp = textPage(d, a.page);
    const textObjects = tp.lines
      .map((l, i) => ({ objectId: i, text: sliceLineText(tp, i), rect: l.box, fullyInside: false }))
      .filter((o) => a.rects.some((r) => intersects(r, o.rect)));
    return { page: a.page, textObjects, imageObjects: [], annotations: [], formFields: [], collateral: [] };
  },
  async applyRedactions(a: { docId: DocId; page: PageIndex; rects: Rect[] }) {
    const d = doc(a.docId);
    return mutate(d, { reason: "redact", pages: [a.page], undoLabel: "redact.title" }, () => ({
      removedObjects: a.rects.length,
      verified: true,
      docGeneration: d.info.docGeneration + 1,
    }));
  },
  async removePassword(_a: { docId: DocId; outPath: string }) {
    throw err("unsupported", "not implemented in the mock adapter");
  },
  async setPassword(_a: unknown) {
    throw err("unsupported", "not implemented in the mock adapter");
  },
  async removeMetadata(a: { docId: DocId }): Promise<DocInfo> {
    const d = doc(a.docId);
    return mutate(d, { reason: "edit", pages: "all" }, () => {
      d.info.meta = {};
      return structuredClone(d.info);
    });
  },
  async setMetadata(a: { docId: DocId; meta: DocInfo["meta"] }): Promise<DocInfo> {
    const d = doc(a.docId);
    return mutate(d, { reason: "edit", pages: "all" }, () => {
      d.info.meta = { ...a.meta };
      return structuredClone(d.info);
    });
  },

  // 7.6 save -----------------------------------------------------------------
  async saveDocument(a: { docId: DocId }, onProgress: (e: JobEvent) => void): Promise<SaveResult> {
    const d = doc(a.docId);
    if (!d.info.path) throw err("readOnly", "document has no path; use Save As");
    return mock.saveDocumentAs({ docId: a.docId, path: d.info.path }, onProgress);
  },
  async saveDocumentAs(a: { docId: DocId; path: string }, onProgress: (e: JobEvent) => void): Promise<SaveResult> {
    const d = doc(a.docId);
    const jobId = nextJob++;
    onProgress({ type: "started", jobId, total: 1 });
    await delay(null, 120);
    d.info.path = a.path;
    d.info.name = baseName(a.path);
    d.info.dirty = false;
    d.info.docGeneration += 1;
    onProgress({ type: "done", jobId, elapsedMs: 120 });
    mockEvents.emit("doc-changed", {
      docId: d.info.docId, docGeneration: d.info.docGeneration, changedPages: "all",
      structure: false, dirty: false, reason: "save",
      canUndo: d.info.canUndo, canRedo: d.info.canRedo,
    });
    mockEvents.emit("doc-saved", { docId: d.info.docId, path: a.path, docGeneration: d.info.docGeneration });
    return { docId: d.info.docId, path: a.path, bytes: d.info.bytes, docGeneration: d.info.docGeneration, elapsedMs: 120 };
  },

  // 7.7 export / print -------------------------------------------------------
  async exportImages(a: ExportImagesArgs, onProgress: (e: JobEvent) => void): Promise<JobId> {
    return runJob(Math.max(1, a.pages.length), onProgress, {
      stepMs: 110,
      outputs: a.pages.map((p) => `${a.outDir}/${a.baseName}-${p + 1}.${a.format}`),
    });
  },
  async exportText(a: { docId: DocId; pages: PageIndex[]; outPath: string }) {
    const d = doc(a.docId);
    const chars = a.pages.reduce((n, p) => n + textPage(d, p).text.length, 0);
    return { chars };
  },
  async exportFlattened(
    a: { docId: DocId; outPath: string; annotations: boolean; forms: boolean; pages?: PageIndex[] },
    onProgress: (e: JobEvent) => void,
  ): Promise<JobId> {
    const d = doc(a.docId);
    return runJob(a.pages?.length ?? d.info.pageCount, onProgress, { stepMs: 80, outputs: [a.outPath] });
  },
  async estimateExport(a: { docId: DocId; pages: PageIndex[]; format: "png" | "jpeg"; dpi: number }) {
    const perPage = (a.format === "png" ? 180_000 : 90_000) * (a.dpi / 150) ** 2;
    return { bytes: Math.round(perPage * Math.max(1, a.pages.length)), sampledPages: Math.min(3, a.pages.length) };
  },
  async printPrepare(a: { docId: DocId; pages?: PageIndex[] }) {
    return { tempPath: `/tmp/seepdf-print-${a.docId}.pdf` };
  },

  // 7.8 history --------------------------------------------------------------
  async undo(a: { docId: DocId }): Promise<DocInfo> {
    const d = doc(a.docId);
    const prev = d.undo.pop();
    if (!prev) throw err("invalidArgument", "nothing to undo");
    d.redo.push(snapshot(d));
    restore(d, prev);
    d.info.docGeneration += 1;
    d.info.canUndo = d.undo.length > 0;
    d.info.canRedo = true;
    mockEvents.emit("doc-changed", {
      docId: d.info.docId, docGeneration: d.info.docGeneration, changedPages: "all",
      structure: true, dirty: d.info.dirty, reason: "undo",
      canUndo: d.info.canUndo, canRedo: d.info.canRedo,
    });
    return structuredClone(d.info);
  },
  async redo(a: { docId: DocId }): Promise<DocInfo> {
    const d = doc(a.docId);
    const next = d.redo.pop();
    if (!next) throw err("invalidArgument", "nothing to redo");
    d.undo.push(snapshot(d));
    restore(d, next);
    d.info.docGeneration += 1;
    d.info.canUndo = true;
    d.info.canRedo = d.redo.length > 0;
    mockEvents.emit("doc-changed", {
      docId: d.info.docId, docGeneration: d.info.docGeneration, changedPages: "all",
      structure: true, dirty: d.info.dirty, reason: "redo",
      canUndo: d.info.canUndo, canRedo: d.info.canRedo,
    });
    return structuredClone(d.info);
  },

  // 7.9 OCR ------------------------------------------------------------------
  async ocrCapabilities() {
    return { engines: ["tesseract"] as ("tesseract" | "vision" | "windows")[], languages: ["kor", "eng"] };
  },
  async ocrPageStatus(a: { docId: DocId; pages: PageIndex[] }) {
    const d = doc(a.docId);
    return a.pages.map((page) => {
      const chars = textPage(d, page).text.length;
      return { page, hasText: chars > 0, charCount: chars };
    });
  },
  async ocrApply(
    a: { docId: DocId; pages: OcrPage[]; replaceExisting: boolean },
    onProgress: (e: JobEvent) => void,
  ): Promise<DocInfo> {
    const d = doc(a.docId);
    await new Promise<void>((resolve) => {
      runJob(Math.max(1, a.pages.length), onProgress, { stepMs: 120, onDone: resolve });
    });
    return mutate(d, { reason: "ocr", pages: "all", undoLabel: "ocr.title" }, () => structuredClone(d.info));
  },

  // 11. app, settings, recents ----------------------------------------------
  async getRecent(): Promise<RecentEntry[]> {
    return delay(structuredClone(recents), 10);
  },
  async updateRecent(a: { entry: RecentEntry }): Promise<void> {
    recents = [a.entry, ...recents.filter((r) => r.path !== a.entry.path)];
    mockEvents.emit("recents-changed", {});
  },
  async removeRecent(a: { path: string }): Promise<void> {
    recents = recents.filter((r) => r.path !== a.path);
    mockEvents.emit("recents-changed", {});
  },
  async setRecentPinned(a: { path: string; pinned: boolean }): Promise<void> {
    recents = recents.map((r) => (r.path === a.path ? { ...r, pinned: a.pinned } : r));
    mockEvents.emit("recents-changed", {});
  },
  async clearRecent(): Promise<void> {
    recents = [];
    mockEvents.emit("recents-changed", {});
  },
  async writeRecentThumbnail(a: { docId: DocId }) {
    return { thumbId: `thumb-${a.docId}` };
  },
  async revealInFileManager(_a: { path: string }): Promise<void> {},
  async getSettings(): Promise<Settings> {
    return structuredClone(settings);
  },
  async setSettings(a: { patch: Partial<Settings> }): Promise<Settings> {
    settings = { ...settings, ...a.patch };
    return structuredClone(settings);
  },
};

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

function listOf(d: MockDoc, page: PageIndex): AnnotList {
  return {
    docId: d.info.docId, page, docGeneration: d.info.docGeneration + 1,
    annots: structuredClone(d.annots.get(page) ?? []),
  };
}

function intersects(a: Rect, b: Rect): boolean {
  return !(b.l > a.r || b.r < a.l || b.b > a.t || b.t < a.b);
}

function sliceLineText(tp: MockTextPage, lineIndex: number): string {
  const line = tp.lines[lineIndex];
  let out = "";
  for (let i = line.firstChar; i < line.firstChar + line.charCount; i++) out += String.fromCodePoint(tp.chars[i].code);
  return out;
}

function searchPage(d: MockDoc, page: PageIndex, query: string, matchCase: boolean, wholeWord: boolean): SearchHit[] {
  if (!query) return [];
  const tp = textPage(d, page);
  const hay = matchCase ? tp.text : tp.text.toLowerCase();
  const needle = matchCase ? query : query.toLowerCase();
  const hits: SearchHit[] = [];
  let at = hay.indexOf(needle);
  while (at >= 0 && hits.length < 50) {
    const before = hay[at - 1] ?? " ";
    const after = hay[at + needle.length] ?? " ";
    const isWord = !/[\p{L}\p{N}]/u.test(before) && !/[\p{L}\p{N}]/u.test(after);
    if (!wholeWord || isWord) {
      const charStart = tp.charTextOffset.findIndex((o) => o === at);
      const rects = charStart >= 0 ? [boxOfRange(tp, charStart, [...query].length)] : [];
      hits.push({
        page,
        charStart: charStart >= 0 ? charStart : at,
        charLength: [...query].length,
        rects,
        context: tp.text.slice(Math.max(0, at - 40), at + needle.length + 40).replace(/\r\n/g, " "),
        contextMatch: [Math.min(40, at), needle.length],
      });
    }
    at = hay.indexOf(needle, at + needle.length);
  }
  return hits;
}

function boxOfRange(tp: MockTextPage, firstChar: number, count: number): Rect {
  const first = tp.chars[firstChar]?.box ?? { l: 0, b: 0, r: 0, t: 0 };
  const box = { ...first };
  for (let i = firstChar; i < Math.min(firstChar + count, tp.chars.length); i++) {
    const c = tp.chars[i].box;
    box.l = Math.min(box.l, c.l);
    box.b = Math.min(box.b, c.b);
    box.r = Math.max(box.r, c.r);
    box.t = Math.max(box.t, c.t);
  }
  return box;
}

function applyPageOp(d: MockDoc, op: PageOp): void {
  const pages = d.info.pages;
  switch (op.kind) {
    case "move": {
      const moving = op.pages.map((i) => pages[i]).filter(Boolean);
      const rest = pages.filter((p) => !op.pages.includes(p.index));
      const at = Math.max(0, Math.min(op.to, rest.length));
      d.info.pages = [...rest.slice(0, at), ...moving, ...rest.slice(at)];
      break;
    }
    case "delete":
      d.info.pages = pages.filter((p) => !op.pages.includes(p.index));
      break;
    case "rotate":
      // `PageGeom.widthPt/heightPt` are the **display** size (IPC_CONTRACT §4), so a quarter turn
      // swaps them — the organizer's cells re-aspect off exactly this (STAGE1E_NOTES §3, §5.5).
      d.info.pages = pages.map((p) => {
        if (!op.pages.includes(p.index)) return p;
        const quarter = op.delta % 180 !== 0;
        return {
          ...p,
          rotation: (((p.rotation + op.delta) % 360) as PageGeom["rotation"]),
          widthPt: quarter ? p.heightPt : p.widthPt,
          heightPt: quarter ? p.widthPt : p.heightPt,
        };
      });
      break;
    case "insertBlank": {
      const size = typeof op.size === "string"
        ? { widthPt: pages[0]?.widthPt ?? 595.28, heightPt: pages[0]?.heightPt ?? 841.89 }
        : op.size;
      const blank: PageGeom = {
        index: op.at, widthPt: size.widthPt, heightPt: size.heightPt, rotation: 0,
        crop: { l: 0, b: 0, r: size.widthPt, t: size.heightPt }, label: null,
      };
      d.info.pages = [...pages.slice(0, op.at), blank, ...pages.slice(op.at)];
      break;
    }
    case "duplicate": {
      const copies = op.pages.map((i) => structuredClone(pages[i])).filter(Boolean);
      d.info.pages = [...pages, ...copies];
      break;
    }
    case "insertFrom": {
      const count = op.range ? op.range.split(",").length : 1;
      const added = makePages(count);
      d.info.pages = [...pages.slice(0, op.at), ...added, ...pages.slice(op.at)];
      break;
    }
    case "reverse":
      d.info.pages = [...pages].reverse();
      break;
  }
}

function annotFromSpec(page: PageIndex, spec: AnnotSpec, id: string, author: string): Annot {
  const now = new Date().toISOString();
  const base: Annot = {
    id, page, kind: spec.kind as Annot["kind"], subtype: "Square",
    rect: { l: 0, b: 0, r: 0, t: 0 }, color: [47, 111, 235], fillColor: null, opacity: 1, borderWidth: 1,
    contents: "", author, created: now, modified: now, hidden: false, printed: true, locked: false, editable: "full",
  };
  switch (spec.kind) {
    case "highlight":
    case "underline":
    case "strikeout":
    case "squiggly":
      return {
        ...base, subtype: spec.kind[0].toUpperCase() + spec.kind.slice(1), quads: spec.rects,
        rect: spec.rects.reduce((acc, r) => ({
          l: Math.min(acc.l, r.l), b: Math.min(acc.b, r.b), r: Math.max(acc.r, r.r), t: Math.max(acc.t, r.t),
        }), spec.rects[0] ?? base.rect),
        color: spec.color, opacity: spec.opacity, contents: spec.contents ?? "",
      };
    case "note":
      return {
        ...base, subtype: "Text", color: spec.color, contents: spec.contents,
        rect: { l: spec.at[0], b: spec.at[1] - 22, r: spec.at[0] + 22, t: spec.at[1] },
      };
    case "ink":
    case "signature":
      return {
        ...base, subtype: "Ink", inkPaths: spec.paths, color: spec.color, borderWidth: spec.width,
        opacity: spec.opacity, rect: boundsOfPaths(spec.paths),
      };
    case "square":
    case "circle":
      return {
        ...base, subtype: spec.kind === "square" ? "Square" : "Circle", rect: spec.rect, color: spec.color,
        fillColor: spec.fillColor, borderWidth: spec.width, opacity: spec.opacity,
      };
    case "line":
    case "arrow":
      return {
        ...base, subtype: "Ink", linePoints: [spec.p1[0], spec.p1[1], spec.p2[0], spec.p2[1]],
        inkPaths: [[spec.p1[0], spec.p1[1], spec.p2[0], spec.p2[1]]], color: spec.color, borderWidth: spec.width,
        opacity: spec.opacity, rect: boundsOfPaths([[spec.p1[0], spec.p1[1], spec.p2[0], spec.p2[1]]]),
      };
    case "textbox":
      return {
        ...base, subtype: "FreeText", rect: spec.rect, text: spec.text, fontSize: spec.fontSize,
        color: spec.color, fillColor: spec.fillColor,
      };
    case "stamp":
      return { ...base, subtype: "Stamp", rect: spec.rect, stampKind: "builtin" in spec.image ? spec.image.builtin : "image" };
  }
}

function boundsOfPaths(paths: number[][]): Rect {
  const box = { l: Infinity, b: Infinity, r: -Infinity, t: -Infinity };
  for (const path of paths) {
    for (let i = 0; i + 1 < path.length; i += 2) {
      box.l = Math.min(box.l, path[i]);
      box.r = Math.max(box.r, path[i]);
      box.b = Math.min(box.b, path[i + 1]);
      box.t = Math.max(box.t, path[i + 1]);
    }
  }
  return Number.isFinite(box.l) ? box : { l: 0, b: 0, r: 0, t: 0 };
}

/** Test helper: forget every open document, job and cached image. */
export function resetMock(): void {
  docs.clear();
  for (const job of jobs.values()) job.cancel();
  jobs.clear();
  textCache.clear();
  imageCache.clear();
  appBus.clear();
  nextDoc = 1;
  nextJob = 1;
  nextAnnot = 1;
  recents = structuredClone(recentsFixture) as unknown as RecentEntry[];
  settings = structuredClone(settingsFixture) as unknown as Settings;
}

/** Test helper: queue an OS-level open so `take_pending_opens` returns something. */
export function pushPendingOpen(path: string, source: "argv" | "macos-opened" | "drop" | "dialog" | "recent" = "argv"): void {
  pendingOpens.push({ path, source });
}

// Route every `seepdf://` URL builder to the generated data URLs above while the mock is live.
setMockAssetResolver(mockAssetUrl);
