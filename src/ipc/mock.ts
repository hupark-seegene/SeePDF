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
import { marginsToUser, resizedVisualSize, visualSize, visualToUserSize } from "../organize/cropGeometry";
import type {
  Annot, AnnotList, AnnotPatch, AnnotResult, AnnotScanEvent, AnnotSpec, DocGeneration, DocId, DocInfo, EngineError,
  EngineStats, ExportImagesArgs, FieldValue, FormField, JobEvent, JobId, Mat6, MergeWarning, OcrApplyPage, OcrPage, OutlineNode,
  PageGeom, PageIndex, Permissions, PageObject, PageOp, RecentEntry, Rect, RedactPreview, SaveResult, SearchEvent, SearchHit,
  Settings, StampResult, StampSpec, StampRole, RemoveStampsResult, TextEditProbe, ViewportHint, CompressOptions, CompressReport,
  ObjectId, ObjectsResult, ParagraphAlign, ParagraphEdit, ParagraphEditResult, ParagraphProbe, Point, Rgb,
  CompareOptions, CompareReport, ComparePage, DiffOp, RecoveryEntry,
  DuplicateObjectsResult, RedactBatchMark, RedactBatchResult, AnnotationSummaryResult, ResizeMode,
  ResizeTarget, SetPageBoxesArgs, SummaryFormat, TtsStatus, LinkTarget, PageLabelRange,
} from "./types";
import { labelsFor, normalizeRanges } from "../dialogs/pageLabels";

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

/** A page object without its id: ids are array positions, renumbered on every list (like PDFium). */
type MockObj = Omit<PageObject, "objectId">;

interface Snapshot {
  info: DocInfo; annots: [PageIndex, Annot[]][]; fields: FormField[]; objects: [PageIndex, MockObj[]][];
  outline: OutlineNode[]; labelRanges: PageLabelRange[];
}

interface MockDoc {
  info: DocInfo;
  outline: OutlineNode[];
  /** P2: the /PageLabels ranges as written (`info.pageLabels` is derived from them) */
  labelRanges: PageLabelRange[];
  annots: Map<PageIndex, Annot[]>;
  fields: FormField[];
  /** page objects (편집 mode), seeded lazily from the fake text layer on first list */
  objects: Map<PageIndex, MockObj[]>;
  undo: Snapshot[];
  redo: Snapshot[];
}

const docs = new Map<DocId, MockDoc>();
const jobs = new Map<JobId, { cancel: () => void }>();
/** 압축 예상 results waiting for 적용 / 취소: at most one per document, like the engine. */
const pendingCompress = new Map<
  DocId,
  {
    token: number; beforeBytes: number; afterBytes: number; imagesDownsampled: number; baseGeneration: DocGeneration;
    /** v0.3 pkg8 (X5) */
    optimized?: boolean;
  }
>();
let nextCompressToken = 1;
/** Recovery copies (P1-8): what `$APPDATA/SeePDF/recovery` would hold, and each open doc's id. */
const recoveryFiles = new Map<string, RecoveryEntry>();
const recoveryIdOf = new Map<DocId, string>();
let nextRecovery = 1;
let nextDoc = 1;
let nextJob = 1;
let nextAnnot = 1;
/** Files the mock "wrote" (Save As): `path_exists` answers true for them (여러 파일 OCR). */
const writtenFiles = new Set<string>();
/** v0.3 pkg8: sheet counts of the n-up files `make_nup` "wrote", so opening one gives its sheets. */
const nupPageCounts = new Map<string, number>();
let recents: RecentEntry[] = structuredClone(recentsFixture) as unknown as RecentEntry[];
let settings = seedSettings();
/**
 * Stamps `add_stamp` put on each document (Stage 8 `remove_stamps`), one entry per page. Not part
 * of the undo snapshot: the mock only needs the counts to be plausible.
 */
const stampsOf = new Map<DocId, { role: StampRole; page: PageIndex }[]>();
const pendingOpens: { path: string; source: "argv" | "macos-opened" | "drop" | "dialog" | "recent" }[] = [];
/**
 * P2 읽어 주기: one fake voice for the whole app. It "speaks" for a while proportional to the text
 * (so the bar can be seen in `vite dev`) unless stopped; tests call `ttsStop` or read `mockTts`.
 */
export const mockTts = { speaking: false, text: "", rate: 1, lang: undefined as string | undefined, timer: 0 as ReturnType<typeof setTimeout> | 0 };
/** The media box of a mock page (P2 자르기): its crop box when first touched. */
type MockGeom = PageGeom & { media?: Rect };

function err(code: EngineError["code"], message: string, extra: Partial<EngineError> = {}): EngineError {
  return { code, message, ...extra };
}

/**
 * The fixture settings with the engine's serde defaults for the fields a settings file written before
 * them lacks (`types.rs` `Settings`): what `get_settings` answers at launch — and after `resetMock()`.
 */
function seedSettings(): Settings {
  const seed = structuredClone(settingsFixture) as unknown as Partial<Settings>;
  return {
    ...seed,
    recentsCount: seed.recentsCount ?? 20,
    autosaveSec: seed.autosaveSec ?? 60,
    signatures: seed.signatures ?? [],
    night: lenientNight(seed.night),
    checkUpdates: seed.checkUpdates ?? true,
  } as Settings;
}

/** `lenient_night`: an unknown 야간 모드 reads as `off` instead of failing the whole file. */
function lenientNight(value: unknown): Settings["night"] {
  return value === "dark" || value === "sepia" ? value : "off";
}

const SETTINGS_ENUMS: Partial<Record<keyof Settings, readonly unknown[]>> = {
  locale: ["ko", "en"],
  theme: ["system", "light", "dark"],
  defaultLayout: ["single", "continuous", "two"],
  renderQuality: ["balanced", "high"],
  ocrDpi: ["auto", 200, 300, 400],
};
const SETTINGS_COUNTS: (keyof Settings)[] = ["tileCacheMb", "recentsCount", "autosaveSec"];
const SETTINGS_FLAGS: (keyof Settings)[] = ["restorePosition", "backupsEnabled", "checkUpdates"];

/**
 * `set_settings` deserialises the merged value into `Settings` and answers `invalidArgument` (saving
 * nothing) when a field does not fit its type; `night` alone is lenient.
 */
function checkSettingsPatch(patch: Partial<Settings>): void {
  const bad = (key: string) => err("invalidArgument", `settings patch: invalid ${key}`);
  for (const [key, value] of Object.entries(patch) as [keyof Settings, unknown][]) {
    const allowed = SETTINGS_ENUMS[key];
    if (allowed && !allowed.includes(value)) throw bad(key);
    if (SETTINGS_COUNTS.includes(key) && !(Number.isInteger(value) && (value as number) >= 0)) throw bad(key);
    if (SETTINGS_FLAGS.includes(key) && typeof value !== "boolean") throw bad(key);
    if (key === "defaultZoom" && !(typeof value === "number" || ["fit-width", "fit-page", "actual"].includes(value as string))) {
      throw bad(key);
    }
    if (key === "author" && typeof value !== "string") throw bad(key);
  }
}

function docsByPath(path: string): boolean {
  for (const d of docs.values()) if (d.info.path === path) return true;
  return false;
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
    objects: [...d.objects.entries()].map(([p, o]) => [p, structuredClone(o)] as [PageIndex, MockObj[]]),
    outline: structuredClone(d.outline),
    labelRanges: structuredClone(d.labelRanges),
  };
}

function restore(d: MockDoc, s: Snapshot): void {
  d.info = structuredClone(s.info);
  d.annots = new Map(s.annots.map(([p, a]) => [p, structuredClone(a)]));
  d.fields = structuredClone(s.fields);
  d.objects = new Map(s.objects.map(([p, o]) => [p, structuredClone(o)]));
  d.outline = structuredClone(s.outline);
  d.labelRanges = structuredClone(s.labelRanges);
}

/** Like the engine: `pages[i].label` and `pageLabels` follow the /PageLabels ranges. */
function applyLabels(d: MockDoc): void {
  const labels = labelsFor(d.labelRanges, d.info.pageCount);
  d.info.pages = d.info.pages.map((p, i) => ({ ...p, label: labels.length && labels[i] ? labels[i] : null }));
  if (labels.length) d.info.pageLabels = labels;
  else delete d.info.pageLabels;
}

/** The engine refuses the lopdf rewrites (outline, page labels, page links) on an encrypted file. */
function refuseEncrypted(d: MockDoc): void {
  if (d.info.encrypted) throw err("unsupported", "this document is encrypted: remove the password first");
}

function outlineHas(nodes: OutlineNode[]): boolean {
  return nodes.length > 0;
}

function validateOutline(d: MockDoc, nodes: OutlineNode[], depth = 0): void {
  if (depth > 32) throw err("invalidArgument", "outline deeper than 32 levels");
  for (const n of nodes) {
    if (n.page !== null && n.page !== undefined && (n.page < 0 || n.page >= d.info.pageCount)) {
      throw err("invalidArgument", `outline page ${n.page} out of range`);
    }
    validateOutline(d, n.children ?? [], depth + 1);
  }
}

/** What `get_outline` reads back after a write: `open` only on nodes with children. */
function normalizeOutline(nodes: OutlineNode[]): OutlineNode[] {
  return nodes.map((n) => {
    const children = normalizeOutline(n.children ?? []);
    const out: OutlineNode = { title: n.title, page: n.url ? null : n.page ?? null, children };
    if (n.dest && out.page !== null && (n.dest.x !== undefined || n.dest.y !== undefined || n.dest.zoom !== undefined)) {
      out.dest = { ...n.dest };
    }
    if (n.url) out.url = n.url;
    if (children.length) out.open = n.open ?? true;
    return out;
  });
}

let openedUrls: string[] = [];

/** Test seam: the web addresses `openUrl` was asked to open. */
export function mockOpenedUrls(): string[] {
  return [...openedUrls];
}

function linkAnnot(page: PageIndex, id: string, rect: Rect, target: LinkTarget): Annot {
  const r = { l: Math.min(rect.l, rect.r), b: Math.min(rect.b, rect.t), r: Math.max(rect.l, rect.r), t: Math.max(rect.b, rect.t) };
  const now = new Date().toISOString();
  return {
    id, page, kind: "link", subtype: "Link", rect: r, color: [0, 0, 0], fillColor: null, opacity: 1, borderWidth: 0,
    contents: "", author: null, created: now, modified: now,
    ...("url" in target ? { uri: target.url } : { dest: { ...target } }),
    hidden: false, printed: true, locked: false, editable: "full",
  };
}

function validateLink(d: MockDoc, page: PageIndex, rect: Rect | undefined, target: LinkTarget | undefined): void {
  if (page < 0 || page >= d.info.pageCount) throw err("notFound", `page ${page}`);
  if (rect && (Math.abs(rect.r - rect.l) < 1 || Math.abs(rect.t - rect.b) < 1)) throw err("invalidArgument", "link rect too small");
  if (!target) return;
  if ("url" in target) {
    if (!target.url.trim()) throw err("invalidArgument", "empty url");
  } else if (target.page < 0 || target.page >= d.info.pageCount) {
    throw err("invalidArgument", `target page ${target.page} out of range`);
  }
}

type ChangeReason = "edit" | "undo" | "redo" | "pages" | "ocr" | "redact";

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
  d.info.undoLabel = opts.undoLabel ?? "undo.objectEdit";
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
  // like `flows.dirName`: the path's own separator, and a drive root keeps its separator
  const cut = Math.max(path.lastIndexOf("/"), path.lastIndexOf("\\"));
  if (cut < 0) return ".";
  const dir = path.slice(0, cut);
  return dir === "" || /^[A-Za-z]:$/.test(dir) ? dir + path[cut] : dir;
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
    labelRanges: [],
    annots,
    fields: (structuredClone(fieldsFixture) as unknown as FormField[]).filter((f) => f.page < pageCount),
    objects: new Map(),
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
  opts: {
    stepMs?: number; note?: (done: number) => string; outputs?: string[]; onDone?: () => void;
    report?: (elapsedMs: number) => CompressReport;
    compare?: (elapsedMs: number) => CompareReport;
  } = {},
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
      const elapsedMs = Date.now() - started;
      onEvent({
        type: "done", jobId, elapsedMs, outputs: opts.outputs, report: opts.report?.(elapsedMs),
        compare: opts.compare?.(elapsedMs),
      });
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

const PAGE_OP_LABEL: Record<PageOp["kind"], string> = {
  move: "undo.pageMove", delete: "undo.pageDelete", rotate: "undo.pageRotate", insertBlank: "undo.pageInsert",
  duplicate: "undo.pageDuplicate", insertFrom: "undo.pageInsertFrom", reverse: "undo.pageReverse",
};

export const mock = {
  // 4. documents -------------------------------------------------------------
  async openDocument(a: { path: string; password?: string; displayName?: string }): Promise<DocInfo> {
    if (/encrypted/i.test(a.path) && !a.password) throw err("passwordRequired", "document is encrypted");
    // a path that names itself damaged fails to open, so a batch can exercise its 실패 row
    if (/damaged/i.test(a.path)) throw err("pdfium", "the file is damaged or not a PDF");
    const recent = recents.find((r) => r.path === a.path);
    const d = makeDoc(a.path, nupPageCounts.get(a.path) ?? recent?.pages ?? BASE_DOC.pageCount);
    // Stage 8: a recovered copy reports the original name, not `<uuid>.pdf`
    if (a.displayName) d.info.name = a.displayName;
    // like the engine (`encrypted = revision != -1 || password.is_some()`): P2's lopdf rewrites refuse it
    if (a.password) d.info.encrypted = true;
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
    pendingCompress.delete(a.docId);
    stampsOf.delete(a.docId);
  },
  async getDocument(a: { docId: DocId }): Promise<DocInfo> {
    return structuredClone(doc(a.docId).info);
  },
  async getOutline(a: { docId: DocId }): Promise<OutlineNode[]> {
    return delay(structuredClone(doc(a.docId).outline), 20);
  },
  async setOutline(a: { docId: DocId; nodes: OutlineNode[] }): Promise<DocInfo> {
    const d = doc(a.docId);
    refuseEncrypted(d);
    validateOutline(d, a.nodes);
    mutate(d, { reason: "edit", pages: "all", undoLabel: "undo.outlineEdit" }, () => {
      d.outline = normalizeOutline(a.nodes);
      d.info.hasOutline = outlineHas(d.outline);
    });
    // after the bump: the returned DocInfo carries the new generation, like the engine's
    return structuredClone(d.info);
  },
  async setPageLabels(a: { docId: DocId; ranges: PageLabelRange[] }): Promise<DocInfo> {
    const d = doc(a.docId);
    refuseEncrypted(d);
    const starts = new Set<number>();
    for (const r of a.ranges) {
      if (r.start < 0 || r.start >= d.info.pageCount) throw err("invalidArgument", `range start ${r.start} out of range`);
      if (starts.has(r.start)) throw err("invalidArgument", `two ranges start at ${r.start}`);
      if (r.first !== undefined && (r.first < 1 || r.first > 1_000_000)) throw err("invalidArgument", "first must be 1..1000000");
      starts.add(r.start);
    }
    mutate(d, { reason: "edit", pages: "all", undoLabel: "undo.pageLabels" }, () => {
      d.labelRanges = normalizeRanges(a.ranges, d.info.pageCount);
      applyLabels(d);
    });
    return structuredClone(d.info);
  },
  async getPageLabels(a: { docId: DocId }): Promise<PageLabelRange[]> {
    const d = doc(a.docId);
    refuseEncrypted(d);
    return delay(structuredClone(d.labelRanges), 10);
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
    return mutate(d, { reason: "edit", pages: [a.page], undoLabel: "undo.annotCreate" }, () => {
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
    return mutate(d, { reason: "edit", pages: [a.page], undoLabel: "undo.annotEdit" }, () => {
      const next: Annot = { ...patchedAnnot(list[idx], a.patch), modified: new Date().toISOString() };
      list[idx] = next;
      d.annots.set(a.page, list);
      return { list: listOf(d, a.page), annot: structuredClone(next), previous };
    });
  },
  async deleteAnnotations(a: { docId: DocId; page: PageIndex; ids: string[] }): Promise<AnnotResult> {
    const d = doc(a.docId);
    const list = d.annots.get(a.page) ?? [];
    // like `annot::delete`: every id must be on the page, or nothing is deleted
    const onPage = new Set(list.map((x) => x.id));
    const missing = a.ids.filter((id) => !onPage.has(id));
    if (missing.length) {
      throw err("notFound", `annotation(s) ${JSON.stringify(missing)} are not on page ${a.page}`, { page: a.page });
    }
    // like the engine (P2 threads): a deleted annotation takes its replies, transitively
    const doomed = new Set(a.ids);
    for (let grew = true; grew; ) {
      grew = false;
      for (const x of list) {
        if (x.inReplyTo && doomed.has(x.inReplyTo) && !doomed.has(x.id)) {
          doomed.add(x.id);
          grew = true;
        }
      }
    }
    return mutate(d, { reason: "edit", pages: [a.page], undoLabel: "undo.annotDelete" }, () => {
      const kept = list.filter((x) => !doomed.has(x.id));
      d.annots.set(a.page, kept);
      return { list: listOf(d, a.page), annot: null, previous: null };
    });
  },
  /** P2 threads: a `Text` reply, invisible on the page, sharing its parent's colour. */
  async replyAnnotation(a: {
    docId: DocId; page: PageIndex; parentId: string; contents: string; author?: string | null;
  }): Promise<AnnotResult> {
    const d = doc(a.docId);
    if (d.info.encrypted) throw err("unsupported", "replies cannot be written into an encrypted document");
    const parent = (d.annots.get(a.page) ?? []).find((x) => x.id === a.parentId);
    if (!parent) throw err("notFound", `annotation '${a.parentId}' is not on page ${a.page}`);
    const now = new Date().toISOString();
    const reply: Annot = {
      id: `mock-${nextAnnot++}`, page: a.page, kind: "note", subtype: "Text",
      rect: { l: parent.rect.l, b: parent.rect.t - 20, r: parent.rect.l + 20, t: parent.rect.t },
      color: [...parent.color] as Rgb, fillColor: null, opacity: 1, borderWidth: 1,
      contents: a.contents, author: a.author?.trim() || null, created: now, modified: now,
      inReplyTo: parent.id, hidden: false, printed: true, locked: false, editable: "full",
    };
    return delay(
      mutate(d, { reason: "edit", pages: [a.page], undoLabel: "undo.annotReply" }, () => {
        d.annots.set(a.page, [...(d.annots.get(a.page) ?? []), reply]);
        return { list: listOf(d, a.page), annot: structuredClone(reply), previous: null };
      }),
      30,
    );
  },
  async setAnnotationsHidden(_a: { docId: DocId; page: PageIndex; ids: string[]; hidden: boolean }) {
    return { viewNonce: Date.now() };
  },
  // P2 links ------------------------------------------------------------------
  async createLink(a: { docId: DocId; page: PageIndex; rect: Rect; target: LinkTarget }): Promise<AnnotResult> {
    const d = doc(a.docId);
    validateLink(d, a.page, a.rect, a.target);
    // a go-to-page link is a lopdf rewrite in the engine: refused on an encrypted file
    if (!("url" in a.target)) refuseEncrypted(d);
    const annot = linkAnnot(a.page, `mock-link-${nextAnnot++}`, a.rect, a.target);
    return mutate(d, { reason: "edit", pages: [a.page], undoLabel: "undo.linkCreate" }, () => {
      d.annots.set(a.page, [...(d.annots.get(a.page) ?? []), annot]);
      return { list: listOf(d, a.page), annot: structuredClone(annot), previous: null };
    });
  },
  async updateLink(a: { docId: DocId; page: PageIndex; id: string; rect?: Rect; target?: LinkTarget }): Promise<AnnotResult> {
    const d = doc(a.docId);
    const list = d.annots.get(a.page) ?? [];
    const idx = list.findIndex((x) => x.id === a.id);
    if (idx < 0) throw err("notFound", `annotation ${a.id}`);
    if (list[idx].kind !== "link") throw err("invalidArgument", `${a.id} is not a link`);
    if (!a.rect && !a.target) throw err("invalidArgument", "update_link needs a rect or a target");
    validateLink(d, a.page, a.rect, a.target);
    const previous = structuredClone(list[idx]);
    const wasPage = previous.dest !== undefined;
    if (a.target && (!("url" in a.target) || wasPage)) refuseEncrypted(d);
    return mutate(d, { reason: "edit", pages: [a.page], undoLabel: "undo.linkEdit" }, () => {
      const target: LinkTarget = a.target ?? (previous.uri !== undefined ? { url: previous.uri } : { ...previous.dest! });
      const next = linkAnnot(a.page, a.id, a.rect ?? previous.rect, target);
      next.created = previous.created;
      list[idx] = next;
      d.annots.set(a.page, list);
      return { list: listOf(d, a.page), annot: structuredClone(next), previous };
    });
  },
  async deleteLink(a: { docId: DocId; page: PageIndex; id: string }): Promise<AnnotResult> {
    const d = doc(a.docId);
    const list = d.annots.get(a.page) ?? [];
    const found = list.find((x) => x.id === a.id);
    if (!found) throw err("notFound", `annotation ${a.id}`);
    if (found.kind !== "link") throw err("invalidArgument", `${a.id} is not a link`);
    return mutate(d, { reason: "edit", pages: [a.page], undoLabel: "undo.linkDelete" }, () => {
      d.annots.set(a.page, list.filter((x) => x.id !== a.id));
      return { list: listOf(d, a.page), annot: null, previous: structuredClone(found) };
    });
  },
  async openUrl(a: { url: string }): Promise<void> {
    openedUrls.push(a.url);
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
    return mutate(d, { reason: "edit", pages: [a.page], undoLabel: "undo.formFill" }, () => {
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
    return mutate(d, { reason: "edit", pages: "all", undoLabel: "undo.formReset" }, () => {
      d.fields = d.fields.map((f) => ({ ...f, value: f.type === "checkbox" ? "Off" : "", checked: false }));
      return structuredClone(d.info);
    });
  },

  // 7.3 pages ----------------------------------------------------------------
  async pageOps(a: { docId: DocId; ops: PageOp[] }): Promise<DocInfo> {
    const d = doc(a.docId);
    return mutate(d, { reason: "pages", pages: "all", structure: true, undoLabel: a.ops.length === 1 ? PAGE_OP_LABEL[a.ops[0].kind] : "undo.pageOps" }, () => {
      for (const op of a.ops) applyPageOp(d, op);
      d.info.pages = d.info.pages.map((p, index) => ({ ...p, index }));
      d.info.pageCount = d.info.pages.length;
      // /PageLabels are by page index and page ops do not rewrite them (neither does PDFium)
      if (d.labelRanges.length) applyLabels(d);
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

  // 7.3a crop / resize (P2) ----------------------------------------------------
  async setPageBoxes(a: SetPageBoxesArgs): Promise<DocInfo> {
    const d = doc(a.docId);
    if (a.media === null) throw err("invalidArgument", "media: null is not supported");
    const pages = mockPageList(d, a.pages);
    if (a.crop === undefined && a.media === undefined) return structuredClone(d.info);
    // validate every page first: a failure changes nothing (like the engine's rollback)
    const next = new Map<PageIndex, MockGeom>();
    for (const index of pages) {
      const g = structuredClone(d.info.pages[index]) as MockGeom;
      const media = a.media ?? g.media ?? g.crop;
      let crop: Rect | null = null;
      if (a.crop === null) crop = media;
      else if (a.crop && "margins" in a.crop) crop = marginsToUser(g.rotation, g.crop, a.crop.margins);
      else if (a.crop) crop = a.crop;
      else if (a.media) crop = g.crop;
      if (crop) crop = intersectRect(crop, media);
      if (!crop || crop.r - crop.l < 1 || crop.t - crop.b < 1) {
        throw err("invalidArgument", `the crop box of page ${index + 1} is empty`, { page: index });
      }
      const { w, h } = visualSize({ rotation: g.rotation, crop });
      next.set(index, { ...g, media, crop, widthPt: round2(w), heightPt: round2(h) });
    }
    mutate(d, { reason: "pages", pages, undoLabel: "undo.pageCrop" }, () => {
      d.info.pages = d.info.pages.map((g) => next.get(g.index) ?? g);
    });
    // like the engine: the DocInfo *after* the step (generation, history labels)
    return structuredClone(d.info);
  },
  async resizePages(a: { docId: DocId; pages: PageIndex[] | "all"; size: ResizeTarget; mode: ResizeMode }): Promise<DocInfo> {
    const d = doc(a.docId);
    if (typeof a.size !== "string" && !(a.size.w >= 1 && a.size.h >= 1 && a.size.w <= 14400 && a.size.h <= 14400)) {
      throw err("invalidArgument", "page size out of range");
    }
    const pages = mockPageList(d, a.pages);
    mutate(d, { reason: "pages", pages, undoLabel: "undo.pageResize" }, () => {
      d.info.pages = d.info.pages.map((g) => {
        if (!pages.includes(g.index)) return g;
        const seen = resizedVisualSize(g, a.size);
        const user = visualToUserSize(g.rotation, seen.w, seen.h);
        const box = { l: 0, b: 0, r: round2(user.w), t: round2(user.h) };
        return { ...g, crop: box, media: box, widthPt: round2(seen.w), heightPt: round2(seen.h) } as MockGeom;
      });
      d.info.bytes += 400 * pages.length;
    });
    return structuredClone(d.info);
  },

  // 7.4 page objects ---------------------------------------------------------
  // Stateful (Stage 7): moves, deletes, additions and paragraph edits change the list, and undo
  // restores it — enough for the 편집 flow tests. Pixels and the text layer stay fixture-based.
  async listPageObjects(a: { docId: DocId; page: PageIndex }): Promise<ObjectsResult> {
    const d = doc(a.docId);
    return listObjects(d, a.page);
  },
  async probeTextEdit(a: { docId: DocId; page: PageIndex; objectId: number; text: string }): Promise<TextEditProbe> {
    const o = pageObjects(doc(a.docId), a.page)[a.objectId];
    if (!o) throw err("notFound", `no object ${a.objectId}`);
    if (o.editable === "readOnly") return { strategy: "refused", reason: o.reason };
    return needsHangulFont(o.fontName ?? "", a.text, o.text)
      ? { strategy: "replaceFont", substituteFont: "SeePDF Hangul" }
      : { strategy: "inPlace" };
  },
  async editTextObject(a: { docId: DocId; page: PageIndex; objectId: number; expectGeneration: DocGeneration;
    patch: { text?: string; fontSizePt?: number; color?: Rgb }; allowFontSubstitution: boolean }) {
    const d = doc(a.docId);
    checkGeneration(d, a.expectGeneration);
    const o = pageObjects(d, a.page)[a.objectId];
    if (!o || o.type !== "text") throw err("notFound", `no text object ${a.objectId}`);
    if (o.editable !== "full") throw err("unsupported", "object is not editable");
    if (!a.allowFontSubstitution && needsHangulFont(o.fontName ?? "", a.patch.text ?? "", o.text)) {
      throw err("fontCoverage", "embedded font cannot render the requested text");
    }
    return mutate(d, { reason: "edit", pages: [a.page] }, () => {
      const target = pageObjects(d, a.page)[a.objectId];
      if (a.patch.text !== undefined) target.text = a.patch.text;
      if (a.patch.fontSizePt !== undefined) target.fontSizePt = a.patch.fontSizePt;
      if (a.patch.color !== undefined) target.color = a.patch.color;
      return () => listObjects(d, a.page);
    })();
  },
  async addTextObject(a: { docId: DocId; page: PageIndex; rect: Rect; text: string; fontSizePt: number; color: Rgb;
    align: "left" | "center" | "right" }) {
    const d = doc(a.docId);
    if (!a.text.trim()) throw err("invalidArgument", "empty text");
    return mutate(d, { reason: "edit", pages: [a.page], undoLabel: "undo.objectAdd" }, () => {
      const lines = a.text.split("\n");
      let baseline = a.rect.t - a.fontSizePt * 0.8;
      for (const line of lines) {
        if (line) {
          const w = measure(line, a.fontSizePt);
          pageObjects(d, a.page).push(textObj(line, a.rect.l, baseline, w, a.fontSizePt, a.color, "SeePDF Hangul", true));
        }
        baseline -= a.fontSizePt * 1.2;
      }
      return () => listObjects(d, a.page);
    })();
  },
  async addImageObject(a: { docId: DocId; page: PageIndex; rect: Rect; path: string; keepAspect: boolean }) {
    const d = doc(a.docId);
    if (!/\.(png|jpe?g)$/i.test(a.path)) throw err("unsupported", "not a PNG / JPEG");
    return mutate(d, { reason: "edit", pages: [a.page], undoLabel: "undo.objectAdd" }, () => {
      // the fake image is 4:3; keepAspect centres it inside the rect like the engine
      let r = { ...a.rect };
      if (a.keepAspect) {
        const w = r.r - r.l;
        const h = r.t - r.b;
        const s = Math.min(w / 400, h / 300);
        const cx = (r.l + r.r) / 2;
        const cy = (r.b + r.t) / 2;
        r = { l: cx - 200 * s, r: cx + 200 * s, b: cy - 150 * s, t: cy + 150 * s };
      }
      pageObjects(d, a.page).push({
        type: "image", rect: r, matrix: [r.r - r.l, 0, 0, r.t - r.b, r.l, r.b], ours: true, editable: "full",
      });
      return () => listObjects(d, a.page);
    })();
  },
  async transformObject(a: { docId: DocId; page: PageIndex; objectId: ObjectId; expectGeneration: DocGeneration;
    translate?: [number, number]; scale?: [number, number]; rotateDeg?: number }) {
    const d = doc(a.docId);
    checkGeneration(d, a.expectGeneration);
    const o = pageObjects(d, a.page)[a.objectId];
    if (!o) throw err("notFound", `no object ${a.objectId}`);
    if (o.editable === "readOnly") throw err("unsupported", "object is read-only");
    return mutate(d, { reason: "edit", pages: [a.page], undoLabel: "undo.objectTransform" }, () => {
      const t = pageObjects(d, a.page)[a.objectId];
      const { l, b } = t.rect;
      if (a.scale) {
        const [sx, sy] = a.scale;
        t.rect = { l, b, r: l + (t.rect.r - l) * sx, t: b + (t.rect.t - b) * sy };
        t.matrix = [t.matrix[0] * sx, t.matrix[1], t.matrix[2], t.matrix[3] * sy, t.matrix[4], t.matrix[5]];
        if (t.fontSizePt) t.fontSizePt = Math.round(t.fontSizePt * sy * 100) / 100;
      }
      if (a.translate) {
        const [dx, dy] = a.translate;
        t.rect = { l: t.rect.l + dx, b: t.rect.b + dy, r: t.rect.r + dx, t: t.rect.t + dy };
        t.matrix = [t.matrix[0], t.matrix[1], t.matrix[2], t.matrix[3], t.matrix[4] + dx, t.matrix[5] + dy];
      }
      return () => listObjects(d, a.page);
    })();
  },
  // Stage 8: copies are appended to the target page's list (new ids = its old length onwards).
  // Like the engine, a form XObject / shading / other object cannot be carried to another page.
  async duplicateObjects(a: { docId: DocId; page: PageIndex; objectIds: ObjectId[]; expectGeneration: DocGeneration;
    offset: [number, number]; targetPage?: PageIndex }): Promise<DuplicateObjectsResult> {
    const d = doc(a.docId);
    checkGeneration(d, a.expectGeneration);
    const target = a.targetPage ?? a.page;
    if (target < 0 || target >= d.info.pageCount) throw err("invalidArgument", `no page ${target}`);
    if (a.objectIds.length === 0) throw err("invalidArgument", "no objects");
    const list = pageObjects(d, a.page);
    for (const id of a.objectIds) {
      const o = list[id];
      if (!o) throw err("notFound", `no object ${id}`);
      if (o.editable === "readOnly") throw err("unsupported", "object is read-only");
      if (target !== a.page && (o.type === "form" || o.type === "shading" || o.type === "other")) {
        throw err("unsupported", `a ${o.type} object cannot be copied to another page`);
      }
    }
    return mutate(d, { reason: "edit", pages: [target], undoLabel: "undo.objectDuplicate" }, () => {
      const [dx, dy] = a.offset;
      const dest = pageObjects(d, target);
      const copies = a.objectIds.map((id) => structuredClone(list[id]));
      const newObjectIds: ObjectId[] = [];
      for (const c of copies) {
        c.rect = { l: c.rect.l + dx, b: c.rect.b + dy, r: c.rect.r + dx, t: c.rect.t + dy };
        c.matrix = [c.matrix[0], c.matrix[1], c.matrix[2], c.matrix[3], c.matrix[4] + dx, c.matrix[5] + dy];
        c.ours = true;
        newObjectIds.push(dest.length);
        dest.push(c);
      }
      return () => ({ ...listObjects(d, target), newObjectIds });
    })();
  },
  async deleteObjects(a: { docId: DocId; page: PageIndex; objectIds: ObjectId[]; expectGeneration: DocGeneration }) {
    const d = doc(a.docId);
    checkGeneration(d, a.expectGeneration);
    const list = pageObjects(d, a.page);
    for (const id of a.objectIds) {
      if (!list[id]) throw err("notFound", `no object ${id}`);
      if (list[id].editable === "readOnly") throw err("unsupported", "object is read-only");
    }
    return mutate(d, { reason: "edit", pages: [a.page], undoLabel: "undo.objectDelete" }, () => {
      const drop = new Set(a.objectIds);
      d.objects.set(a.page, list.filter((_, i) => !drop.has(i)));
      return () => listObjects(d, a.page);
    })();
  },
  async probeParagraph(a: { docId: DocId; page: PageIndex; at: Point }): Promise<ParagraphProbe | null> {
    return mockParagraph(doc(a.docId), a.page, a.at);
  },
  async editParagraph(a: { docId: DocId; page: PageIndex; expectGeneration: DocGeneration; edit: ParagraphEdit;
    allowFontSubstitution: boolean }): Promise<ParagraphEditResult> {
    const d = doc(a.docId);
    checkGeneration(d, a.expectGeneration);
    const list = pageObjects(d, a.page);
    const members = a.edit.objectIds.map((id) => list[id]);
    if (members.length === 0 || members.some((o) => !o || o.type !== "text")) throw err("notFound", "paragraph objects moved on");
    if (members.some((o) => o.editable !== "full")) throw err("unsupported", "paragraph is not editable");
    const first = members[0];
    const fontName = first.fontName ?? "Helvetica";
    const hangul = needsHangulFont(fontName, a.edit.text, members.map((o) => o.text ?? "").join(""));
    if (hangul && !a.allowFontSubstitution) throw err("fontCoverage", "the paragraph font cannot render the new text");
    const plan = planParagraphEdit(d, a.page, a.edit, hangul ? "SeePDF Hangul" : fontName);
    const fields = {
      rect: plan.box,
      lines: plan.lines,
      overflowPt: round2(plan.overflowPt),
      shiftedPt: round2(plan.shiftPt),
      movedObjects: plan.shiftPt !== 0 ? plan.movable.length : 0,
      movedAnnotations: plan.shiftPt !== 0 ? plan.annots.length : 0,
      roomPt: round2(plan.roomPt),
      pastBottomPt: round2(plan.pastBottomPt),
      ...(plan.blocked ? { blocked: plan.blocked } : null),
      ...(plan.fitScale !== undefined ? { fitScale: plan.fitScale } : null),
      ...(plan.shiftPt !== 0 && plan.band ? { movedBand: plan.band } : null),
    };
    // a dry run answers the same fields and leaves everything as it was: no generation, no undo step,
    // and (like the engine) no page listing
    if (a.edit.dryRun) return { objects: { docGeneration: d.info.docGeneration, objects: [] }, ...fields };
    return mutate(d, { reason: "edit", pages: [a.page], undoLabel: "undo.paragraphEdit" }, () => {
      const dy = -plan.shiftPt;
      if (dy !== 0) {
        for (const i of plan.movable) translateObj(list[i], dy);
        for (const annot of plan.annots) translateAnnot(annot, dy);
      }
      const drop = new Set(a.edit.objectIds);
      d.objects.set(a.page, [...list.filter((_, i) => !drop.has(i)), ...plan.objects]);
      return () => ({ objects: listObjects(d, a.page), ...fields });
    })();
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
    return mutate(d, { reason: "redact", pages: [a.page], undoLabel: "undo.redact" }, () => ({
      removedObjects: a.rects.length,
      verified: true,
      docGeneration: d.info.docGeneration + 1,
    }));
  },
  // Stage 8: every page in one mutation (= one undo step); text / image objects the marks touch go.
  async applyRedactionsBatch(a: { docId: DocId; marks: RedactBatchMark[]; options: { fill: Rgb; overlayText?: string } }): Promise<RedactBatchResult> {
    const d = doc(a.docId);
    const pages = [...new Set(a.marks.filter((m) => m.rects.length > 0).map((m) => m.page))].sort((x, y) => x - y);
    if (pages.length === 0) throw err("invalidArgument", "no marks");
    for (const p of pages) if (p < 0 || p >= d.info.pageCount) throw err("invalidArgument", `no page ${p}`);
    return mutate(d, { reason: "redact", pages, undoLabel: "undo.redact" }, () => {
      let removedObjects = 0;
      for (const p of pages) {
        const rects = a.marks.filter((m) => m.page === p).flatMap((m) => m.rects);
        const list = pageObjects(d, p);
        const kept = list.filter((o) => !((o.type === "text" || o.type === "image") && rects.some((r) => intersects(r, o.rect))));
        removedObjects += list.length - kept.length;
        d.objects.set(p, kept);
      }
      return () => ({ removedObjects, verified: true, docGeneration: d.info.docGeneration, pages });
    })();
  },
  // both write a copy to `outPath`; the open document is untouched
  async removePassword(a: { docId: DocId; outPath: string }): Promise<{ bytes: number }> {
    const d = doc(a.docId);
    if (!a.outPath) throw err("invalidArgument", "outPath is required");
    if (!d.info.encrypted) throw err("invalidArgument", "document is not encrypted");
    return { bytes: d.info.bytes };
  },
  async setPassword(a: {
    docId: DocId; outPath: string; userPassword?: string; ownerPassword: string; permissions: Partial<Permissions>;
  }): Promise<{ bytes: number }> {
    const d = doc(a.docId);
    if (!a.outPath) throw err("invalidArgument", "outPath is required");
    if (!a.ownerPassword) throw err("invalidArgument", "ownerPassword is required");
    return { bytes: d.info.bytes };
  },
  async removeMetadata(a: { docId: DocId }): Promise<DocInfo> {
    const d = doc(a.docId);
    return mutate(d, { reason: "edit", pages: "all", undoLabel: "undo.metadataRemove" }, () => {
      d.info.meta = {};
      return structuredClone(d.info);
    });
  },
  async setMetadata(a: { docId: DocId; meta: DocInfo["meta"] }): Promise<DocInfo> {
    const d = doc(a.docId);
    return mutate(d, { reason: "edit", pages: "all", undoLabel: "undo.metadataEdit" }, () => {
      // like the backend: `undefined` keeps a key, a blank string removes it
      const meta = { ...d.info.meta };
      for (const [k, v] of Object.entries(a.meta) as [keyof DocInfo["meta"], string | undefined][]) {
        if (v === undefined) continue;
        if (v.trim()) meta[k] = v;
        else delete meta[k];
      }
      d.info.meta = meta;
      return structuredClone(d.info);
    });
  },

  // 7.5b stamp / compress (Stage 4) ---------------------------------------------
  async addStamp(a: { docId: DocId; spec: StampSpec }): Promise<StampResult> {
    const d = doc(a.docId);
    const { spec } = a;
    if (spec.source.kind === "text" && !spec.source.text.trim()) throw err("invalidArgument", "empty stamp text");
    if (spec.source.kind === "image" && !spec.source.path) throw err("notFound", "stamp image missing");
    const pages = spec.pages === "all" ? d.info.pages.map((p) => p.index) : spec.pages;
    if (!pages.length || pages.some((p) => p < 0 || p >= d.info.pageCount)) throw err("invalidArgument", "page out of range");
    const undoLabel = spec.role === "watermark" ? "undo.watermark" : "undo.headerFooter";
    mutate(d, { reason: "edit", pages: spec.pages === "all" ? "all" : pages, undoLabel }, () => {
      d.info.bytes += 900 * pages.length;
    });
    stampsOf.set(a.docId, [...(stampsOf.get(a.docId) ?? []), ...pages.map((page) => ({ role: spec.role, page }))]);
    // like the engine: the DocInfo *after* the step (generation, history labels)
    return { info: structuredClone(d.info), pagesStamped: pages.length };
  },
  /** Stage 8 워터마크 제거: every role when `role` is omitted; nothing to remove is not an error. */
  async removeStamps(a: { docId: DocId; pages?: PageIndex[] | "all"; role?: StampRole }): Promise<RemoveStampsResult> {
    const d = doc(a.docId);
    const pages = a.pages === undefined || a.pages === "all" ? null : new Set(a.pages);
    if (pages && [...pages].some((p) => p < 0 || p >= d.info.pageCount)) throw err("invalidArgument", "page out of range");
    const all = stampsOf.get(a.docId) ?? [];
    const hit = (s: { role: StampRole; page: PageIndex }) => (!pages || pages.has(s.page)) && (!a.role || s.role === a.role);
    const removed = all.filter(hit);
    if (removed.length === 0) return { info: structuredClone(d.info), removed: 0 };
    const touched = [...new Set(removed.map((s) => s.page))].sort((x, y) => x - y);
    mutate(d, { reason: "edit", pages: touched, undoLabel: "undo.removeStamps" }, () => {
      d.info.bytes = Math.max(0, d.info.bytes - 900 * removed.length);
    });
    stampsOf.set(a.docId, all.filter((s) => !hit(s)));
    return { info: structuredClone(d.info), removed: removed.length };
  },
  async compressEstimate(
    a: { docId: DocId; options: CompressOptions },
    onEvent: (e: JobEvent) => void,
  ): Promise<JobId> {
    const d = doc(a.docId);
    const pages = a.options.pages?.length ? a.options.pages : d.info.pages.map((p) => p.index);
    pendingCompress.delete(a.docId);
    const before = d.info.bytes;
    const baseGeneration = d.info.docGeneration;
    // 300 DPI: nothing in the fixture is above it, so the rewrite comes out slightly bigger
    const ratio = a.options.targetDpi === 300 ? 1.002 : a.options.targetDpi === 150 ? 0.58 : 0.41;
    // v0.3 pkg8 (X5) 구조 최적화: unencrypted only, a further ~12 % (kept only when smaller)
    const optimized = !!a.options.optimize && !d.info.encrypted;
    const imagesTotal = pages.length * 2;
    return runJob(Math.max(1, pages.length), onEvent, {
      stepMs: 60,
      report: (elapsedMs) => {
        const token = nextCompressToken++;
        const afterBytes = Math.round(before * (optimized ? Math.min(ratio, 1) * 0.88 : ratio));
        const imagesDownsampled = a.options.targetDpi === 300 ? 0 : imagesTotal - 1;
        pendingCompress.set(a.docId, { token, beforeBytes: before, afterBytes, imagesDownsampled, baseGeneration, optimized });
        return { token, beforeBytes: before, afterBytes, imagesTotal, imagesDownsampled, elapsedMs };
      },
    });
  },
  async compressApply(a: { docId: DocId; token: number }): Promise<DocInfo> {
    const d = doc(a.docId);
    const pending = pendingCompress.get(a.docId);
    if (!pending || pending.token !== a.token) throw err("notFound", `compress result ${a.token}`);
    pendingCompress.delete(a.docId);
    // like the engine: a result that saves nothing is spent without touching the document
    if ((pending.imagesDownsampled === 0 && !pending.optimized) || pending.afterBytes >= pending.beforeBytes) {
      return structuredClone(d.info);
    }
    // …and one estimated before the latest edit is `stale` (the token is spent either way)
    if (d.info.docGeneration !== pending.baseGeneration) {
      throw err("stale", "the document changed after the estimate; estimate again");
    }
    mutate(d, { reason: "edit", pages: "all", structure: true, undoLabel: "undo.compress" }, () => {
      d.info.bytes = pending.afterBytes;
    });
    return structuredClone(d.info);
  },
  async compressDiscard(a: { docId: DocId; token: number }): Promise<void> {
    const pending = pendingCompress.get(a.docId);
    if (pending?.token === a.token) pendingCompress.delete(a.docId);
  },

  // 7.5c compare / recovery (Stage 5) -------------------------------------------
  async compareDocuments(
    a: { docA: DocId; docB: DocId; options: CompareOptions },
    onEvent: (e: JobEvent) => void,
  ): Promise<JobId> {
    const dA = doc(a.docA);
    const dB = doc(a.docB);
    if (a.docA === a.docB) throw err("invalidArgument", "cannot compare a document with itself");
    const pagesA = a.options.pagesA ?? dA.info.pages.map((_, i) => i);
    const pagesB = a.options.pagesB ?? dB.info.pages.map((_, i) => i);
    const pairs = Math.max(pagesA.length, pagesB.length);
    return runJob(Math.max(1, pairs), onEvent, {
      stepMs: 40,
      compare: (elapsedMs) =>
        mockCompare(dA, dB, pagesA, pagesB, Boolean(a.options.ignoreCase), elapsedMs, a.options.alignPages !== false),
    });
  },
  async writeRecovery(a: { docId: DocId }): Promise<RecoveryEntry> {
    const d = doc(a.docId);
    let id = recoveryIdOf.get(a.docId);
    if (!id) {
      id = `00000000-0000-4000-8000-${String(nextRecovery++).padStart(12, "0")}`;
      recoveryIdOf.set(a.docId, id);
    }
    const entry: RecoveryEntry = {
      id, originalPath: d.info.path, name: d.info.name, savedAt: new Date().toISOString(),
      bytes: d.info.bytes, pages: d.info.pageCount,
      recoveryPath: `/Users/veri/Library/Application Support/SeePDF/recovery/${id}.pdf`,
    };
    recoveryFiles.set(id, entry);
    return delay(structuredClone(entry), 8);
  },
  async clearRecovery(a: { docId: DocId }): Promise<void> {
    doc(a.docId);
    const id = recoveryIdOf.get(a.docId);
    if (id) recoveryFiles.delete(id);
  },
  async listRecovery(): Promise<RecoveryEntry[]> {
    const list = [...recoveryFiles.values()].sort((x, y) => y.savedAt.localeCompare(x.savedAt));
    return delay(structuredClone(list), 8);
  },
  async discardRecovery(a: { id: string }): Promise<void> {
    recoveryFiles.delete(a.id);
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
    writtenFiles.add(a.path);
    d.info.name = baseName(a.path);
    d.info.dirty = false;
    // Like the engine (IPC_CONTRACT §8): a save changes nothing a cache is keyed on, so the
    // generation stays and only `doc-saved` is broadcast — never `doc-changed`.
    onProgress({ type: "done", jobId, elapsedMs: 120, outputs: [a.path] });
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
  async exportText(a: { docId: DocId; pages: PageIndex[]; outPath: string; preserveLayout?: boolean }) {
    const d = doc(a.docId);
    const chars = a.pages.reduce((n, p) => n + textPage(d, p).text.length, 0);
    return { chars };
  },
  /** P2: counts what the engine would write (links / widgets are not comments). */
  async exportAnnotationSummary(
    a: { docId: DocId; path: string; format: SummaryFormat; pages?: PageIndex[]; locale?: "ko" | "en" },
  ): Promise<AnnotationSummaryResult> {
    const d = doc(a.docId);
    if (!a.path) throw err("invalidArgument", "the output path is empty");
    const pages = a.pages?.length ? a.pages : d.info.pages.map((p) => p.index);
    if (pages.some((p) => p < 0 || p >= d.info.pageCount)) throw err("invalidArgument", "page out of range");
    const count = pages.reduce(
      (n, p) => n + (d.annots.get(p) ?? []).filter((x) => x.kind !== "link" && x.kind !== "widget").length,
      0,
    );
    writtenFiles.add(a.path);
    return delay({ count, bytes: (a.format === "csv" ? 3 : 0) + 120 + 160 * count }, 20);
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
  async printPrepare(a: { docId: DocId; pages?: PageIndex[]; annots?: "all" | "none" | "stamps" }) {
    return { tempPath: `/tmp/seepdf-print-${a.docId}.pdf` };
  },
  // v0.3 pkg8: 모아찍기 / 소책자 — the engine's sheet arithmetic, no file.
  async makeNup(a: {
    docId: DocId; pages?: PageIndex[];
    options: { perSheet: number; booklet?: boolean };
    outPath?: string;
  }): Promise<{ path: string; pageCount: number }> {
    const d = doc(a.docId);
    if (![1, 2, 4, 6, 9].includes(a.options.perSheet)) throw err("invalidArgument", "perSheet must be 1, 2, 4, 6 or 9");
    const n = a.pages?.length ? a.pages.length : d.info.pageCount;
    const pageCount = a.options.booklet ? Math.ceil(n / 4) * 2 : Math.ceil(n / a.options.perSheet);
    const path = a.outPath ?? `/tmp/seepdf-print/${d.info.name.replace(/\.pdf$/i, "")}-${a.docId}-nup.pdf`;
    writtenFiles.add(path);
    nupPageCounts.set(path, pageCount);
    return delay({ path, pageCount }, 30);
  },
  /** v0.3 pkg8 (X3): the sample's pages carry one image each. */
  async exportEmbeddedImages(
    a: { docId: DocId; pages: PageIndex[]; outDir: string; baseName: string },
    onProgress: (e: JobEvent) => void,
  ): Promise<JobId> {
    const d = doc(a.docId);
    const pages = a.pages.length ? a.pages : d.info.pages.map((p) => p.index);
    return runJob(Math.max(1, pages.length), onProgress, {
      stepMs: 40,
      outputs: pages.map((p) => `${a.outDir}/${a.baseName}-p${p + 1}-1.png`),
    });
  },
  async exportStitchedImage(
    a: { docId: DocId; pages: PageIndex[]; dpi: number; format: "png" | "jpeg"; outPath: string },
    onProgress: (e: JobEvent) => void,
  ): Promise<{ jobId: JobId; dpi: number; width: number; height: number; lowered: boolean }> {
    const d = doc(a.docId);
    const pages = a.pages.length ? a.pages : d.info.pages.map((p) => p.index);
    const px = (pt: number, dpi: number) => Math.round((pt * dpi) / 72);
    const size = (dpi: number) => ({
      width: Math.max(...pages.map((p) => px(d.info.pages[p].widthPt, dpi))),
      height: pages.reduce((n, p) => n + px(d.info.pages[p].heightPt, dpi), 0),
    });
    // the engine's cap: 64 Mpx and 65 000 px a side
    let dpi = a.dpi;
    while (dpi > 36 && (size(dpi).width * size(dpi).height > 64_000_000 || size(dpi).height > 65_000)) dpi -= 1;
    writtenFiles.add(a.outPath);
    const jobId = runJob(Math.max(1, pages.length), onProgress, { stepMs: 40, outputs: [a.outPath] });
    return { jobId, dpi, ...size(dpi), lowered: dpi !== a.dpi };
  },
  async exportTiff(
    a: { docId: DocId; pages: PageIndex[]; dpi: number; outPath: string },
    onProgress: (e: JobEvent) => void,
  ): Promise<JobId> {
    const d = doc(a.docId);
    if (a.dpi < 36 || a.dpi > 1200) throw err("invalidArgument", `${a.dpi} DPI is outside 36..=1200`);
    writtenFiles.add(a.outPath);
    return runJob(Math.max(1, a.pages.length || d.info.pageCount), onProgress, { stepMs: 40, outputs: [a.outPath] });
  },
  async exportTextFlow(
    a: { docId: DocId; pages: PageIndex[]; format: "docx" | "hwpx" | "html" | "md"; outPath: string },
    onProgress: (e: JobEvent) => void,
  ): Promise<JobId> {
    const d = doc(a.docId);
    writtenFiles.add(a.outPath);
    return runJob(Math.max(1, a.pages.length || d.info.pageCount), onProgress, { stepMs: 40, outputs: [a.outPath] });
  },

  // 7.8 history --------------------------------------------------------------
  async undo(a: { docId: DocId }): Promise<DocInfo> {
    const d = doc(a.docId);
    const prev = d.undo.pop();
    if (!prev) throw err("invalidArgument", "nothing to undo");
    d.redo.push(snapshot(d));
    // The engine's generation only ever grows (`doc.generation += 1` after the restore): a
    // generation is never handed out twice, so an id pinned to it cannot name another state.
    const generation = d.info.docGeneration;
    restore(d, prev);
    d.info.docGeneration = generation + 1;
    d.info.canUndo = d.undo.length > 0;
    d.info.canRedo = true;
    d.info.redoLabel = d.redo[d.redo.length - 1]?.info.undoLabel ?? null;
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
    const generation = d.info.docGeneration;
    restore(d, next);
    d.info.docGeneration = generation + 1;
    d.info.canUndo = true;
    d.info.canRedo = d.redo.length > 0;
    d.info.redoLabel = d.redo[d.redo.length - 1]?.info.undoLabel ?? null;
    mockEvents.emit("doc-changed", {
      docId: d.info.docId, docGeneration: d.info.docGeneration, changedPages: "all",
      structure: true, dirty: d.info.dirty, reason: "redo",
      canUndo: d.info.canUndo, canRedo: d.info.canRedo,
    });
    return structuredClone(d.info);
  },

  // 7.9 OCR ------------------------------------------------------------------
  async ocrCapabilities() {
    // The browser mock plays a Mac with Vision, so 인식 엔진 can be exercised without a backend.
    return { engines: ["tesseract", "vision"] as ("tesseract" | "vision" | "windows")[], languages: ["kor", "eng"] };
  },
  /** A canned one-line Vision result in the `/ocr` image's geometry (P1-11). */
  async ocrRecognizeNative(a: { docId: DocId; page: PageIndex; dpi: number; languages: string[] }): Promise<OcrPage> {
    const d = doc(a.docId);
    const geom = d.info.pages[a.page];
    if (!geom) throw err("invalidArgument", `page ${a.page} of ${d.info.pageCount}`);
    const turned = geom.rotation === 90 || geom.rotation === 270;
    const widthPx = Math.round(((turned ? geom.heightPt : geom.widthPt) * a.dpi) / 72);
    const heightPx = Math.round(((turned ? geom.widthPt : geom.heightPt) * a.dpi) / 72);
    const y0 = Math.round(heightPx * 0.1);
    const y1 = y0 + Math.round(a.dpi / 6);
    const x0 = Math.round(widthPx * 0.1);
    const mid = Math.round(widthPx * 0.3);
    const x1 = Math.round(widthPx * 0.5);
    return delay({
      page: a.page, dpi: a.dpi, widthPx, heightPx, rotation: geom.rotation,
      lines: [{
        text: "모의 인식 결과", bbox: [x0, y0, x1, y1], rowHeightPx: y1 - y0,
        words: [
          { text: "모의", bbox: [x0, y0, mid, y1], confidence: 100 },
          { text: "인식 결과", bbox: [mid + 10, y0, x1, y1], confidence: 100 },
        ],
      }],
    }, 150);
  },
  async ocrPageStatus(a: { docId: DocId; pages: PageIndex[] }) {
    const d = doc(a.docId);
    return a.pages.map((page) => {
      const chars = textPage(d, page).text.length;
      return { page, hasText: chars > 0, charCount: chars };
    });
  },
  async ocrApply(
    a: { docId: DocId; pages: OcrApplyPage[]; replaceExisting: boolean },
    onProgress: (e: JobEvent) => void,
  ): Promise<DocInfo> {
    const d = doc(a.docId);
    await new Promise<void>((resolve) => {
      runJob(Math.max(1, a.pages.length), onProgress, { stepMs: 120, onDone: resolve });
    });
    return mutate(d, { reason: "ocr", pages: "all", undoLabel: "undo.ocrApply" }, () => structuredClone(d.info));
  },

  // 11a. read aloud (P2) --------------------------------------------------------
  async ttsSpeak(a: { text: string; lang?: string; rate?: number }): Promise<TtsStatus> {
    if (!a.text.trim()) throw err("invalidArgument", "there is no text to read");
    if (mockTts.timer) clearTimeout(mockTts.timer);
    const rate = Math.min(2, Math.max(0.5, a.rate ?? 1));
    Object.assign(mockTts, { speaking: true, text: a.text, rate, lang: a.lang });
    mockTts.timer = setTimeout(() => {
      mockTts.speaking = false;
      mockTts.timer = 0;
    }, Math.min(8000, 400 + (a.text.length * 40) / rate));
    return mockTtsStatus();
  },
  async ttsStop(): Promise<TtsStatus> {
    if (mockTts.timer) clearTimeout(mockTts.timer);
    mockTts.timer = 0;
    mockTts.speaking = false;
    return mockTtsStatus();
  },
  async ttsStatus(): Promise<TtsStatus> {
    return mockTtsStatus();
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
  async pathExists(a: { path: string }): Promise<boolean> {
    return writtenFiles.has(a.path) || docsByPath(a.path) || recents.some((r) => r.path === a.path);
  },
  /** P2 여러 파일에서 검색 › 폴더 추가: a fixed little tree under any folder, like the real listing. */
  async listPdfFiles(a: { dir: string; recursive?: boolean }): Promise<string[]> {
    const dir = a.dir.replace(/[\\/]+$/, "");
    const top = [`${dir}/보고서-2024.pdf`, `${dir}/회의록.pdf`];
    const nested = [`${dir}/2023/예산안.pdf`];
    return delay([...top, ...(a.recursive === false ? [] : nested)].sort(), 10);
  },
  async getSettings(): Promise<Settings> {
    return structuredClone(settings);
  },
  async setSettings(a: { patch: Partial<Settings> }): Promise<Settings> {
    checkSettingsPatch(a.patch);
    settings = { ...settings, ...a.patch };
    settings.night = lenientNight(settings.night);
    return structuredClone(settings);
  },
  /** P1-9: the path is content-addressed like the real one, so the same PNG gives the same path. */
  async writeSignatureImage(a: { bytes: number[] }): Promise<string> {
    if (a.bytes.length < 8 || a.bytes[0] !== 0x89 || a.bytes[1] !== 0x50) {
      throw err("invalidArgument", "signature image is not a PNG");
    }
    let hash = 0x811c9dc5;
    for (const b of a.bytes) hash = Math.imul(hash ^ b, 0x01000193) >>> 0;
    const path = `/mock/app-data/signatures/sig-${hash.toString(16).padStart(8, "0")}.png`;
    writtenFiles.add(path);
    return path;
  },
};

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

function mockTtsStatus(): TtsStatus {
  const hangul = /[\uac00-\ud7a3]/.test(mockTts.text);
  return { supported: true, speaking: mockTts.speaking, engine: "say", voice: hangul || mockTts.lang?.startsWith("ko") ? "Yuna" : null };
}

/** `PageIndex[] | 'all'` → sorted, deduplicated, validated indices. */
function mockPageList(d: MockDoc, pages: PageIndex[] | "all"): PageIndex[] {
  const list = pages === "all" ? d.info.pages.map((p) => p.index) : [...new Set(pages)].sort((x, y) => x - y);
  if (!list.length || list.some((p) => p < 0 || p >= d.info.pageCount)) throw err("invalidArgument", "page out of range");
  return list;
}

function intersectRect(a: Rect, b: Rect): Rect | null {
  const r = { l: Math.max(a.l, b.l), b: Math.max(a.b, b.b), r: Math.min(a.r, b.r), t: Math.min(a.t, b.t) };
  return r.r > r.l && r.t > r.b ? r : null;
}

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
      // like the engine: each copy lands right after its source (import at `index + 1`, highest
      // index first), and a repeated index is copied once (`check_pages` dedups)
      const next = [...pages];
      const sources = [...new Set(op.pages)].filter((i) => i >= 0 && i < pages.length).sort((x, y) => y - x);
      for (const i of sources) next.splice(i + 1, 0, structuredClone(pages[i]));
      d.info.pages = next;
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
      return {
        ...base,
        // Stage 8: `/Subj "SeePDF:Signature"` reads back as a 서명
        kind: spec.signature ? "signature" : "stamp",
        subtype: "Stamp", rect: spec.rect, stampKind: "builtin" in spec.image ? spec.image.builtin : "image",
      };
  }
}

/**
 * `update_annotation`'s patch applied the way the engine applies it (`annot/update.rs`): geometry
 * is rebuilt — `rects` become the markup's quads and their union its rect, `paths` the ink strokes
 * and their padded bounds, `p1`/`p2` the line — and only contract fields are written, never the
 * patch's own keys (`rects`, `paths`, `p1`, `p2` are not `Annot` fields).
 */
function patchedAnnot(annot: Annot, patch: AnnotPatch): Annot {
  const next: Annot = structuredClone(annot);
  const width = patch.borderWidth ?? annot.borderWidth;
  if (patch.color) next.color = patch.color;
  if (patch.fillColor !== undefined) next.fillColor = patch.fillColor;
  if (patch.opacity !== undefined) next.opacity = Math.min(1, Math.max(0, patch.opacity));
  if (patch.borderWidth !== undefined) next.borderWidth = patch.borderWidth;
  if (patch.rects) {
    next.quads = structuredClone(patch.rects);
    next.rect = patch.rects.reduce(
      (acc, r) => ({ l: Math.min(acc.l, r.l), b: Math.min(acc.b, r.b), r: Math.max(acc.r, r.r), t: Math.max(acc.t, r.t) }),
      patch.rects[0] ?? annot.rect,
    );
  }
  if (patch.paths) {
    next.inkPaths = structuredClone(patch.paths);
    next.rect = paddedBounds(patch.paths, width);
  }
  if ((annot.kind === "line" || annot.kind === "arrow") && (patch.p1 || patch.p2)) {
    const [x1, y1, x2, y2] = annot.linePoints ?? [0, 0, 0, 0];
    const p1 = patch.p1 ?? [x1, y1];
    const p2 = patch.p2 ?? [x2, y2];
    next.linePoints = [p1[0], p1[1], p2[0], p2[1]];
    next.inkPaths = [[p1[0], p1[1], p2[0], p2[1]]];
    next.rect = paddedBounds(next.inkPaths, width);
  }
  if (patch.rect) next.rect = structuredClone(patch.rect);
  if (patch.contents !== undefined) next.contents = patch.contents;
  if (patch.author !== undefined) next.author = patch.author;
  if (patch.fontSize !== undefined) next.fontSize = patch.fontSize;
  if (patch.text !== undefined) {
    next.text = patch.text;
    // a text box is rebuilt from its text, which is also its /Contents
    if (annot.kind === "textbox" && patch.contents === undefined) next.contents = patch.text;
  }
  if (patch.locked !== undefined) next.locked = patch.locked;
  return next;
}

/** `ink_bounds`: the points' bounds grown by half the stroke width (at least 1 pt). */
function paddedBounds(paths: number[][], width: number): Rect {
  const r = boundsOfPaths(paths);
  const pad = Math.max(width / 2, 1);
  return { l: r.l - pad, b: r.b - pad, r: r.r + pad, t: r.t + pad };
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

// ---------------------------------------------------------------------------
// 문서 비교 in the mock: page B is a deterministic "revision" of its fixture text
// ---------------------------------------------------------------------------

interface MockWord { text: string; rect: Rect; line: number }

function mockWords(d: MockDoc, page: PageIndex | null): MockWord[] {
  if (page === null || page < 0 || page >= d.info.pageCount) return [];
  const tp = textPage(d, page);
  return tp.words.map((w) => {
    let text = "";
    for (let i = w.firstChar; i < w.firstChar + w.charCount; i++) text += String.fromCodePoint(tp.chars[i].code);
    return { text, rect: boxOfRange(tp, w.firstChar, w.charCount), line: w.lineIndex };
  });
}

/** Pair 0, 3, 6… equal; 1, 4… one word replaced + one inserted; 2, 5… two words deleted. */
function revise(words: MockWord[], pair: number): MockWord[] {
  switch (pair % 3) {
    case 1: {
      const out = words.slice();
      if (out[1]) out[1] = { ...out[1], text: `${out[1].text}(개정)` };
      if (out.length > 4) out.splice(5, 0, { ...out[4], text: "추가됨" });
      return out;
    }
    case 2:
      return words.filter((_, i) => i !== 2 && i !== 3);
    default:
      return words;
  }
}

/** One rect per line: the union of the words' boxes on each line. */
function lineRects(words: MockWord[]): Rect[] {
  const byLine = new Map<number, Rect>();
  for (const w of words) {
    const r = byLine.get(w.line);
    byLine.set(w.line, r
      ? { l: Math.min(r.l, w.rect.l), b: Math.min(r.b, w.rect.b), r: Math.max(r.r, w.rect.r), t: Math.max(r.t, w.rect.t) }
      : { ...w.rect });
  }
  return [...byLine.values()];
}

/** Word LCS → equal / delete / insert runs, adjacent delete+insert collapsed into replace. */
function diffWords(a: MockWord[], b: MockWord[], fold: (s: string) => string): DiffOp[] {
  const A = a.map((w) => fold(w.text));
  const B = b.map((w) => fold(w.text));
  const n = A.length;
  const m = B.length;
  const lcs: number[][] = Array.from({ length: n + 1 }, () => new Array<number>(m + 1).fill(0));
  for (let i = n - 1; i >= 0; i--) {
    for (let j = m - 1; j >= 0; j--) lcs[i][j] = A[i] === B[j] ? lcs[i + 1][j + 1] + 1 : Math.max(lcs[i + 1][j], lcs[i][j + 1]);
  }
  type Run = { kind: "equal" | "delete" | "insert"; a: MockWord[]; b: MockWord[] };
  const runs: Run[] = [];
  const push = (kind: Run["kind"], wa?: MockWord, wb?: MockWord) => {
    let last = runs[runs.length - 1];
    if (!last || last.kind !== kind) runs.push((last = { kind, a: [], b: [] }));
    if (wa) last.a.push(wa);
    if (wb) last.b.push(wb);
  };
  let i = 0;
  let j = 0;
  while (i < n || j < m) {
    if (i < n && j < m && A[i] === B[j]) push("equal", a[i++], b[j++]);
    else if (j < m && (i >= n || lcs[i][j + 1] >= lcs[i + 1][j])) push("insert", undefined, b[j++]);
    else push("delete", a[i++]);
  }
  const ops: DiffOp[] = [];
  const text = (ws: MockWord[]) => ws.map((w) => w.text).join(" ");
  for (let k = 0; k < runs.length; k++) {
    const r = runs[k];
    const next = runs[k + 1];
    if ((r.kind === "delete" && next?.kind === "insert") || (r.kind === "insert" && next?.kind === "delete")) {
      const del = r.kind === "delete" ? r : next;
      const ins = r.kind === "insert" ? r : next;
      ops.push({ kind: "replace", words: del.a.length, textA: text(del.a), textB: text(ins.b), rectsA: lineRects(del.a), rectsB: lineRects(ins.b) });
      k += 1;
    } else if (r.kind === "equal") ops.push({ kind: "equal", words: r.a.length });
    else if (r.kind === "delete") ops.push({ kind: "delete", words: r.a.length, textA: text(r.a), rectsA: lineRects(r.a) });
    else ops.push({ kind: "insert", words: r.b.length, textB: text(r.b), rectsB: lineRects(r.b) });
  }
  return ops;
}

/**
 * Stage 8 `alignPages`: pair candidate pages by word-set similarity (Jaccard ≥ 0.5 may pair),
 * maximising the total similarity in order — an inserted / deleted page becomes a null-sided row.
 * Returns positions into `wordsA` / `wordsB`.
 */
function alignMockPages(wordsA: string[][], wordsB: string[][]): [number | null, number | null][] {
  const sim = (x: string[], y: string[]) => {
    const sx = new Set(x);
    const sy = new Set(y);
    let both = 0;
    for (const w of sx) if (sy.has(w)) both += 1;
    const union = sx.size + sy.size - both;
    return union === 0 ? 1 : both / union;
  };
  const n = wordsA.length;
  const m = wordsB.length;
  const score: number[][] = Array.from({ length: n + 1 }, () => new Array<number>(m + 1).fill(0));
  for (let i = n - 1; i >= 0; i--) {
    for (let j = m - 1; j >= 0; j--) {
      const s = sim(wordsA[i], wordsB[j]);
      score[i][j] = Math.max(score[i + 1][j], score[i][j + 1], s >= 0.5 ? score[i + 1][j + 1] + s : -Infinity);
    }
  }
  const out: [number | null, number | null][] = [];
  let i = 0;
  let j = 0;
  while (i < n || j < m) {
    if (i < n && j < m) {
      const s = sim(wordsA[i], wordsB[j]);
      if (s >= 0.5 && score[i][j] === score[i + 1][j + 1] + s) {
        out.push([i++, j++]);
        continue;
      }
      if (score[i][j] === score[i + 1][j]) out.push([i++, null]);
      else out.push([null, j++]);
      continue;
    }
    out.push(i < n ? [i++, null] : [null, j++]);
  }
  return out;
}

function mockCompare(
  dA: MockDoc, dB: MockDoc, pagesA: PageIndex[], pagesB: PageIndex[], ignoreCase: boolean, elapsedMs: number,
  alignPages = true,
): CompareReport {
  const fold = ignoreCase ? (s: string) => s.toLowerCase() : (s: string) => s;
  const pages: ComparePage[] = [];
  let inserted = 0;
  let deleted = 0;
  // B's text is revised per position in `pagesB`, so pairing does not change what B says
  const revisedB = pagesB.map((p, k) => revise(mockWords(dB, p), k));
  const pairs: [number | null, number | null][] = alignPages
    ? alignMockPages(pagesA.map((p) => mockWords(dA, p).map((w) => fold(w.text))), revisedB.map((ws) => ws.map((w) => fold(w.text))))
    : Array.from({ length: Math.max(pagesA.length, pagesB.length) }, (_, k) => [
        k < pagesA.length ? k : null,
        k < pagesB.length ? k : null,
      ]);
  for (const [ka, kb] of pairs) {
    const pageA = ka === null ? null : pagesA[ka];
    const pageB = kb === null ? null : pagesB[kb];
    const wa = mockWords(dA, pageA);
    const wb = kb === null ? [] : revisedB[kb];
    const ops = diffWords(wa, wb, fold);
    for (const op of ops) {
      if (op.kind === "delete" || op.kind === "replace") deleted += op.words;
      if (op.kind === "insert") inserted += op.words;
      if (op.kind === "replace") inserted += op.textB ? op.textB.split(" ").length : 0;
    }
    pages.push({ pageA, pageB, changed: ops.some((o) => o.kind !== "equal"), wordsA: wa.length, wordsB: wb.length, ops });
  }
  return {
    docA: dA.info.docId, docB: dB.info.docId, pages,
    changedPages: pages.filter((p) => p.changed).length, inserted, deleted, elapsedMs,
  };
}

// ---------------------------------------------------------------------------
// Page objects (편집 mode, Stage 7)
// ---------------------------------------------------------------------------

function checkGeneration(d: MockDoc, expect: DocGeneration): void {
  if (expect !== d.info.docGeneration) throw err("stale", "generation moved on");
}

const HANGUL = /[\u1100-\u11ff\u3130-\u318f\uac00-\ud7af]/;

/**
 * A Latin font cannot draw Hangul it has not drawn before: the fake Korean fonts cover every
 * syllable, a Latin (subset) font only the Hangul already present in the text it drew (`have`).
 */
function needsHangulFont(fontName: string, text: string, have = ""): boolean {
  if (/hangul|kr|gothic|pretendard/i.test(fontName)) return false;
  for (const ch of text) if (HANGUL.test(ch) && !have.includes(ch)) return true;
  return false;
}

/** Width of a run in points — the same advances as the fake text layer. */
function measure(text: string, size: number): number {
  let w = 0;
  for (const ch of text) w += size * (ch === " " ? 0.32 : isWide(ch) ? 1 : 0.52);
  return w;
}

/** Break at spaces, a word wider than the box by character; `\n` is a hard break. */
function wrapText(text: string, width: number, size: number): string[] {
  const out: string[] = [];
  for (const para of text.split("\n")) {
    let line = "";
    for (const word of para.split(" ")) {
      const candidate = line ? `${line} ${word}` : word;
      if (measure(candidate, size) <= width || !line) {
        line = candidate;
      } else {
        out.push(line);
        line = word;
      }
      while (measure(line, size) > width && line.length > 1) {
        let cut = line.length - 1;
        while (cut > 1 && measure(line.slice(0, cut), size) > width) cut -= 1;
        out.push(line.slice(0, cut));
        line = line.slice(cut);
      }
    }
    out.push(line);
  }
  return out;
}

function unionR(a: Rect, b: Rect): Rect {
  return { l: Math.min(a.l, b.l), b: Math.min(a.b, b.b), r: Math.max(a.r, b.r), t: Math.max(a.t, b.t) };
}

function textObj(text: string, x: number, baseline: number, width: number, size: number, color: Rgb, fontName: string, ours: boolean): MockObj {
  return {
    type: "text",
    rect: { l: x, b: baseline - size * 0.24, r: x + width, t: baseline + size * 0.78 },
    matrix: [1, 0, 0, 1, x, baseline],
    ours,
    text,
    fontName,
    fontSizePt: size,
    color,
    editable: "full",
  };
}

const round2 = (v: number) => Math.round(v * 100) / 100 || 0;

function translateObj(o: MockObj, dy: number): void {
  o.rect = { ...o.rect, b: o.rect.b + dy, t: o.rect.t + dy };
  o.matrix = [o.matrix[0], o.matrix[1], o.matrix[2], o.matrix[3], o.matrix[4], o.matrix[5] + dy];
}

function translateAnnot(a: Annot, dy: number): void {
  a.rect = { ...a.rect, b: a.rect.b + dy, t: a.rect.t + dy };
  if (a.quads) a.quads = a.quads.map((q) => ({ ...q, b: q.b + dy, t: q.t + dy }));
  if (a.inkPaths) a.inkPaths = a.inkPaths.map((path) => path.map((v, k) => (k % 2 ? v + dy : v)));
  if (a.linePoints) {
    const [x1, y1, x2, y2] = a.linePoints;
    a.linePoints = [x1, y1 + dy, x2, y2 + dy];
  }
}

interface ParagraphPlan {
  /** the new text objects, laid out downward from the paragraph's first baseline */
  objects: MockObj[];
  box: Rect;
  lines: number;
  /** indices into the page list of the objects the flow moves, and the annotations moving with them */
  movable: number[];
  annots: Annot[];
  /** > 0 down, < 0 up */
  shiftPt: number;
  roomPt: number;
  overflowPt: number;
  /** nothing below: how far the new text runs past the bottom margin */
  pastBottomPt: number;
  blocked?: "pageBottom" | "obstacle";
  fitScale?: number;
  /** the region the moved content came from (column × paragraph bottom … stack bottom) */
  band?: Rect;
}

/**
 * The fake engine's flow (Stage 9 contract; upright pages only). Content that follows the paragraph
 * = objects whose top is at or below its original bottom (± a quarter of the size) and that sit in
 * its column (the original ∪ new box, horizontally): ≥ 60 % of the object inside, sticking out by
 * ≤ 15 % of the column. Below-and-overlapping objects that are not in the column are obstacles; the
 * movable set stops at the first one. `room` = from the lowest movable bottom (or, with nothing to
 * move, the paragraph's own bottom) down to that obstacle's top or the bottom margin (crop + 18 pt).
 */
function planParagraphEdit(d: MockDoc, page: PageIndex, edit: ParagraphEdit, font: string): ParagraphPlan {
  const list = pageObjects(d, page);
  const members = edit.objectIds.map((id) => list[id]);
  const first = members[0];
  const orig = members.reduce((u, o) => unionR(u, o.rect), first.rect);
  const size = edit.fontSizePt ?? first.fontSizePt ?? 11;
  const color = edit.color ?? first.color ?? [0, 0, 0];
  const width = edit.width ?? orig.r - orig.l;
  const leading = members.length > 1 ? Math.abs(members[0].matrix[5] - members[1].matrix[5]) : size * 1.2;
  const top = first.matrix[5];
  const flow = edit.flow ?? "push";
  // no text: the paragraph is deleted and, with push, what follows moves up into its place
  const emptied = !edit.text.trim();

  const bottomAt = (f: number) => top - (wrapText(edit.text, width, size * f).length - 1) * leading * f - size * f * 0.24;
  let scale = 1;
  if (flow === "fit") {
    for (let k = 0; k <= 15; k += 1) {
      scale = round2(1 - 0.02 * k);
      if (bottomAt(scale) >= orig.b - 0.01) break;
    }
  }
  const sz = size * scale;
  const lead = leading * scale;
  const wrapped = emptied ? [] : wrapText(edit.text, width, sz);
  const objects: MockObj[] = [];
  let used: Rect | null = null;
  wrapped.forEach((line, i) => {
    if (!line) return;
    const o = textObj(line, orig.l, top - i * lead, measure(line, sz), sz, color, font, first.ours);
    used = used ? unionR(used, o.rect) : o.rect;
    objects.push(o);
  });
  const box: Rect = used ?? { l: orig.l, b: orig.t, r: orig.l, t: orig.t };
  const newBottom = bottomAt(scale);
  let delta = emptied ? 0 : orig.b - newBottom; // > 0: the paragraph grew

  const cl = Math.min(orig.l, box.l);
  const cr = Math.max(orig.r, box.r);
  const overlapOf = (r: Rect) => Math.max(0, Math.min(r.r, cr) - Math.max(r.l, cl));
  const inColumn = (r: Rect) => {
    const w = r.r - r.l;
    if (w <= 0) return r.l >= cl && r.l <= cr;
    const out = Math.max(0, cl - r.l) + Math.max(0, r.r - cr);
    return overlapOf(r) >= 0.6 * w && out <= 0.15 * (cr - cl);
  };
  const isBelow = (r: Rect) => r.t <= orig.b + 0.25 * size && (overlapOf(r) > 0 || inColumn(r));
  const drop = new Set(edit.objectIds);
  const below = list.map((o, i) => ({ o, i })).filter((e) => !drop.has(e.i) && isBelow(e.o.rect));
  const obstacles = below.filter((e) => !inColumn(e.o.rect));
  const obstacleTop = obstacles.length ? Math.max(...obstacles.map((e) => e.o.rect.t)) : null;
  const movable = below.filter((e) => inColumn(e.o.rect) && (obstacleTop === null || e.o.rect.b >= obstacleTop - 0.01));
  const margin = (d.info.pages[page]?.crop.b ?? 0) + 18;
  const byObstacle = obstacleTop !== null && obstacleTop >= margin;
  const limit = byObstacle ? (obstacleTop as number) : margin;
  const lowest = movable.length ? Math.min(...movable.map((e) => e.o.rect.b)) : orig.b;
  const roomPt = Math.max(0, lowest - limit);
  if (emptied && movable.length) delta = Math.min(0, Math.max(...movable.map((e) => e.o.rect.t)) - orig.t);

  // the paragraph spacing above the first thing that follows: a push may use it up before the new
  // text touches what follows (the engine's `gap`)
  const gap = movable.length ? Math.max(0, orig.b - Math.max(...movable.map((e) => e.o.rect.t))) : 0;
  const nothingBelow = below.length === 0;
  let shiftPt = 0;
  let overflowPt = 0;
  let pastBottomPt = 0;
  let blocked: ParagraphPlan["blocked"];
  if (flow === "push") {
    if (delta > 0.01 && nothingBelow) {
      // only the page bottom: nothing is overlapped, the text runs past the margin
      if (delta > roomPt + 0.01) {
        blocked = "pageBottom";
        pastBottomPt = delta - roomPt;
      }
    } else if (delta > 0.01) {
      if (movable.length) shiftPt = Math.min(delta, roomPt);
      const remaining = delta - roomPt - gap;
      if (remaining > 0.01) {
        blocked = byObstacle ? "obstacle" : "pageBottom";
        overflowPt = remaining;
      }
    } else if (delta < -0.01 && movable.length) {
      shiftPt = delta;
    }
  } else if (delta > 0.01 && below.length) {
    // nothing moves: how far the new text reaches over the first thing below
    const firstTop = Math.max(...below.map((e) => e.o.rect.t));
    overflowPt = Math.max(0, Math.min(delta, firstTop - newBottom));
  } else if (delta > 0.01) {
    pastBottomPt = Math.max(0, margin - newBottom);
  }

  // annotations in the moved band and in the column go with the content (never widgets)
  const bandBottom = byObstacle ? (obstacleTop as number) - 0.01 : -Infinity;
  const annots = (d.annots.get(page) ?? []).filter(
    (an) => an.subtype !== "Widget" && an.rect.t <= orig.b + 0.25 * size && an.rect.b >= bandBottom && inColumn(an.rect),
  );
  const band: Rect | undefined = movable.length
    ? { l: cl, r: cr, t: orig.b + 0.25 * size, b: lowest - 0.25 * size }
    : undefined;
  return {
    objects, box, lines: wrapped.length, movable: movable.map((e) => e.i), annots, shiftPt, roomPt, overflowPt, pastBottomPt,
    blocked, band,
    ...(flow === "fit" ? { fitScale: scale } : null),
  };
}

/** The live object list of a page, seeded from the fake text layer (one text object per line). */
function pageObjects(d: MockDoc, page: PageIndex): MockObj[] {
  let list = d.objects.get(page);
  if (!list) {
    const tp = textPage(d, page);
    list = tp.lines.map((l, i): MockObj => {
      const text = sliceLineText(tp, i);
      return {
        type: "text",
        rect: l.box,
        matrix: [1, 0, 0, 1, l.box.l, l.baselineY] as Mat6,
        ours: false,
        text,
        fontName: [...text].filter((ch) => HANGUL.test(ch)).length * 2 > text.length ? "NotoSansKR" : "Helvetica",
        fontSizePt: tp.chars[l.firstChar]?.fontSizePt ?? 11,
        color: [26, 28, 31],
        // the title sits inside a Form XObject, so one run always shows the read-only badge
        editable: i === 0 ? "readOnly" : "full",
        reason: i === 0 ? "insideXObject" : undefined,
      };
    });
    d.objects.set(page, list);
  }
  return list;
}

function listObjects(d: MockDoc, page: PageIndex): ObjectsResult {
  return {
    docGeneration: d.info.docGeneration,
    objects: pageObjects(d, page).map((o, objectId) => ({ ...structuredClone(o), objectId })),
  };
}

/**
 * The fake paragraph detector: text objects are lines (baseline = matrix f); the paragraph is the
 * hit line plus its neighbours with the same size and a steady leading ≤ 2.2 × size.
 */
function mockParagraph(d: MockDoc, page: PageIndex, [x, y]: Point): ParagraphProbe | null {
  const all = pageObjects(d, page).map((o, id) => ({ o, id })).filter((e) => e.o.type === "text");
  const lines = all.sort((a, b) => b.o.matrix[5] - a.o.matrix[5]);
  const hit = lines.findIndex((e) => x >= e.o.rect.l - 1 && x <= e.o.rect.r + 1 && y >= e.o.rect.b - 1 && y <= e.o.rect.t + 1);
  if (hit < 0) return null;
  const size = lines[hit].o.fontSizePt ?? 11;
  const joins = (upper: MockObj, lower: MockObj, leading: number | null) => {
    const gap = upper.matrix[5] - lower.matrix[5];
    return (lower.fontSizePt ?? 0) === size && gap > 0 && gap <= 2.2 * size && (leading === null || Math.abs(gap - leading) <= leading * 0.2);
  };
  let from = hit;
  let to = hit;
  let leading: number | null = null;
  while (from > 0 && joins(lines[from - 1].o, lines[from].o, leading)) {
    leading ??= lines[from - 1].o.matrix[5] - lines[from].o.matrix[5];
    from -= 1;
  }
  while (to < lines.length - 1 && joins(lines[to].o, lines[to + 1].o, leading)) {
    leading ??= lines[to].o.matrix[5] - lines[to + 1].o.matrix[5];
    to += 1;
  }
  const members = lines.slice(from, to + 1);
  const first = members[0].o;
  const rect = members.reduce((u, e) => unionR(u, e.o.rect), first.rect);
  const text = members.map((e) => e.o.text ?? "").join(" ");
  const refused = members.find((e) => e.o.editable !== "full");
  const fontName = first.fontName ?? "Helvetica";
  const align: ParagraphAlign = "left";
  return {
    objectIds: members.map((e) => e.id),
    rect,
    text,
    fontName,
    fontSizePt: size,
    color: first.color ?? [0, 0, 0],
    mixedStyles: members.some((e) => e.o.fontName !== fontName),
    lineHeightPt: leading ?? size * 1.2,
    align,
    firstLineIndentPt: 0,
    lines: members.length,
    // the paragraph's own font drew the current text, so it covers it
    strategy: refused ? "refused" : "inPlace",
    ...(refused ? { reason: refused.o.reason } : {}),
    docGeneration: d.info.docGeneration,
  };
}

/** Test helper: pretend a previous session crashed and left these recovery copies behind. */
export function seedRecovery(entries: RecoveryEntry[]): void {
  for (const e of entries) recoveryFiles.set(e.id, structuredClone(e));
}

/** Test helper: forget every open document, job and cached image. */
export function resetMock(): void {
  docs.clear();
  pendingCompress.clear();
  nextCompressToken = 1;
  recoveryFiles.clear();
  recoveryIdOf.clear();
  stampsOf.clear();
  writtenFiles.clear();
  nupPageCounts.clear();
  if (mockTts.timer) clearTimeout(mockTts.timer);
  Object.assign(mockTts, { speaking: false, text: "", rate: 1, lang: undefined, timer: 0 });
  nextRecovery = 1;
  for (const job of jobs.values()) job.cancel();
  jobs.clear();
  textCache.clear();
  imageCache.clear();
  appBus.clear();
  nextDoc = 1;
  nextJob = 1;
  nextAnnot = 1;
  openedUrls = [];
  recents = structuredClone(recentsFixture) as unknown as RecentEntry[];
  settings = seedSettings();
}

/** Test helper: queue an OS-level open so `take_pending_opens` returns something. */
export function pushPendingOpen(path: string, source: "argv" | "macos-opened" | "drop" | "dialog" | "recent" = "argv"): void {
  pendingOpens.push({ path, source });
}

// Route every `seepdf://` URL builder to the generated data URLs above while the mock is live.
setMockAssetResolver(mockAssetUrl);
