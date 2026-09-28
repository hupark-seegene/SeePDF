/**
 * The only file in the frontend that imports `invoke` (IPC_CONTRACT §1).
 * One typed wrapper per command; every call is routed to the engine, or to `mock.ts` when
 * `VITE_SEEPDF_MOCK=1` / we are outside Tauri. Callers cannot tell the difference.
 *
 * Errors: the engine rejects with an `EngineError`; every wrapper re-throws it as `SeePdfError`,
 * an `Error` carrying `code`, so `catch (e) { if (isSeePdfError(e) && e.code === 'passwordRequired') … }`
 * works and `t(errorKey(e))` renders the Korean message (UI_SPEC §15.17).
 */
import { Channel, invoke } from "@tauri-apps/api/core";
import { useMock } from "./env";
import { parseRawPage, parseTextLayer, type RawPage, type TextLayerView } from "./binary";
import type {
  Annot, AnnotList, AnnotPatch, AnnotResult, AnnotScanEvent, AnnotSpec, CompareOptions, CompressOptions, DocGeneration, DocId, DocInfo, DocMeta,
  EngineError, EngineStats, ErrorCode, ExportImagesArgs, FieldValue, FormField, JobEvent, JobId, ObjectId,
  ObjectsResult, OcrEngine, OcrPage, OpenRequest, OutlineNode, PageIndex, PageOp, Permissions, RecentEntry, RecoveryEntry, Rect,
  RedactPreview, Rgb, SaveResult, SearchEvent, Settings, StampResult, StampSpec, TextEditProbe, ViewportHint,
} from "./types";

export { parseTextLayer, parseRawPage };
export type { RawPage, TextLayerView };

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

export class SeePdfError extends Error {
  readonly code: ErrorCode;
  readonly page?: number;
  readonly detail?: string;
  constructor(e: EngineError) {
    super(e.message);
    this.name = "SeePdfError";
    this.code = e.code;
    this.page = e.page;
    this.detail = e.detail;
  }
}

export function isSeePdfError(e: unknown): e is SeePdfError {
  return e instanceof SeePdfError;
}

function toSeePdfError(e: unknown): SeePdfError {
  if (e instanceof SeePdfError) return e;
  if (e && typeof e === "object" && "code" in e && "message" in e) return new SeePdfError(e as EngineError);
  return new SeePdfError({ code: "pdfium", message: e instanceof Error ? e.message : String(e) });
}

/** `error.*` i18n key for an engine error code (UI_SPEC §15.17). */
export function errorKey(e: unknown): string {
  const code = isSeePdfError(e) ? e.code : "pdfium";
  switch (code) {
    case "passwordRequired":
    case "passwordWrong":
      return "security.password.wrong";
    case "notFound":
      return "error.fileMissing";
    case "permissionDenied":
      return "error.permissionDenied";
    case "readOnly":
      return "error.readOnlyFile";
    case "busy":
      return "error.busy";
    case "verifyFailed":
      return "error.saveFailed";
    case "io":
      return "error.saveFailed";
    case "unsupported":
    case "invalidArgument":
    case "stale":
    case "cancelled":
    case "fontCoverage":
    case "pdfium":
    default:
      return "error.generic";
  }
}

// ---------------------------------------------------------------------------
// Transport
// ---------------------------------------------------------------------------

/**
 * The mock adapter is loaded on demand so neither it nor its fixtures reach the production bundle.
 * It is kicked off at module load in mock mode, long before any URL builder needs it.
 */
type MockAdapter = (typeof import("./mock"))["mock"];
let mockPromise: Promise<MockAdapter> | null = null;

function loadMock(): Promise<MockAdapter> {
  return (mockPromise ??= import("./mock").then((m) => m.mock));
}

if (useMock()) void loadMock();

/** Run `mockImpl` in mock mode, otherwise `invoke(command, args)`. */
async function call<T>(command: string, args: object, mockImpl: (mock: MockAdapter) => Promise<T>): Promise<T> {
  try {
    if (useMock()) return await mockImpl(await loadMock());
    return await invoke<T>(command, args as Record<string, unknown>);
  } catch (e) {
    throw toSeePdfError(e);
  }
}

/**
 * Progress channel. Callers pass a plain callback; in Tauri it becomes a `tauri::ipc::Channel<T>`
 * (per-invocation, ordered), in mock mode the callback is invoked directly.
 */
function channel<T>(onEvent: (e: T) => void): Channel<T> | ((e: T) => void) {
  if (useMock()) return onEvent;
  const ch = new Channel<T>();
  ch.onmessage = onEvent;
  return ch;
}

// ---------------------------------------------------------------------------
// 4. Documents
// ---------------------------------------------------------------------------

export function openDocument(a: { path: string; password?: string }): Promise<DocInfo> {
  return call("open_document", a, (mock) => mock.openDocument(a));
}

export function closeDocument(a: { docId: DocId }): Promise<void> {
  return call("close_document", a, (mock) => mock.closeDocument(a));
}

export function getDocument(a: { docId: DocId }): Promise<DocInfo> {
  return call("get_document", a, (mock) => mock.getDocument(a));
}

export function getOutline(a: { docId: DocId }): Promise<OutlineNode[]> {
  return call("get_outline", a, (mock) => mock.getOutline(a));
}

export function takePendingOpens(): Promise<OpenRequest[]> {
  return call("take_pending_opens", {}, (mock) => mock.takePendingOpens());
}

export function openInNewWindow(a: { path?: string } = {}): Promise<string> {
  return call("open_in_new_window", a, (mock) => mock.openInNewWindow(a));
}

export function windowBindDocument(a: { label: string; docId: DocId | null }): Promise<void> {
  return call("window_bind_document", a, (mock) => mock.windowBindDocument(a));
}

// ---------------------------------------------------------------------------
// 5. View, render control, statistics
// ---------------------------------------------------------------------------

export function setViewport(a: ViewportHint): Promise<void> {
  // The Rust command takes ONE parameter named `hint`, so the payload has to be `{ hint }` —
  // spreading the fields made every call fail with "missing required key hint" (Stage 2).
  return call("set_viewport", { hint: a }, (mock) => mock.setViewport(a));
}

/** §10.2 "SPRX" — decoded into `{ width, height, stride, pixels }` ready for `ImageData`. */
export async function renderPageRaw(a: { docId: DocId; page: PageIndex; scale: number; rect?: Rect }): Promise<RawPage> {
  const buffer = await call<ArrayBuffer>("render_page_raw", a, (mock) => mock.renderPageRaw(a));
  return parseRawPage(buffer);
}

export function engineStats(): Promise<EngineStats> {
  return call("engine_stats", {}, (mock) => mock.engineStats());
}

// ---------------------------------------------------------------------------
// 6. Text, selection, search
// ---------------------------------------------------------------------------

/** §10.1 "STXL" — the raw buffer, for callers that keep it (the viewer's `TextLayer`). */
export function getTextLayerBuffer(a: { docId: DocId; page: PageIndex }): Promise<ArrayBuffer> {
  return call("get_text_layer", a, (mock) => mock.getTextLayer(a));
}

/** §10.1 decoded: typed-array views over the same buffer — no DOM node per character. */
export async function getTextLayer(a: { docId: DocId; page: PageIndex }): Promise<TextLayerView> {
  return parseTextLayer(await getTextLayerBuffer(a));
}

export function getPageText(a: { docId: DocId; page: PageIndex }): Promise<string> {
  return call("get_page_text", a, (mock) => mock.getPageText(a));
}

export function searchStart(
  a: { docId: DocId; query: string; matchCase: boolean; wholeWord: boolean; fromPage: PageIndex },
  onEvent: (e: SearchEvent) => void,
): Promise<JobId> {
  return call("search_start", { ...a, onEvent: channel(onEvent) }, (mock) => mock.searchStart(a, onEvent));
}

export function cancelJob(a: { jobId: JobId }): Promise<boolean> {
  return call("cancel_job", a, (mock) => mock.cancelJob(a));
}

// ---------------------------------------------------------------------------
// 7.1 Annotations
// ---------------------------------------------------------------------------

export function listAnnotations(a: { docId: DocId; page: PageIndex }): Promise<AnnotList> {
  return call("list_annotations", a, (mock) => mock.listAnnotations(a));
}

export function scanAnnotations(a: { docId: DocId }, onEvent: (e: AnnotScanEvent) => void): Promise<JobId> {
  return call("scan_annotations", { ...a, onEvent: channel(onEvent) }, (mock) => mock.scanAnnotations(a, onEvent));
}

export function createAnnotation(a: { docId: DocId; page: PageIndex; spec: AnnotSpec; id?: string }): Promise<AnnotResult> {
  return call("create_annotation", a, (mock) => mock.createAnnotation(a));
}

export function updateAnnotation(a: { docId: DocId; page: PageIndex; id: string; patch: AnnotPatch }): Promise<AnnotResult> {
  return call("update_annotation", a, (mock) => mock.updateAnnotation(a));
}

export function deleteAnnotations(a: { docId: DocId; page: PageIndex; ids: string[] }): Promise<AnnotResult> {
  return call("delete_annotations", a, (mock) => mock.deleteAnnotations(a));
}

export function setAnnotationsHidden(
  a: { docId: DocId; page: PageIndex; ids: string[]; hidden: boolean },
): Promise<{ viewNonce: number }> {
  return call("set_annotations_hidden", a, (mock) => mock.setAnnotationsHidden(a));
}

// ---------------------------------------------------------------------------
// 7.2 Forms
// ---------------------------------------------------------------------------

export function listFormFields(a: { docId: DocId; page?: PageIndex }): Promise<FormField[]> {
  return call("list_form_fields", a, (mock) => mock.listFormFields(a));
}

export function setFormFieldValue(
  a: { docId: DocId; page: PageIndex; index: number; value: FieldValue },
): Promise<{ field: FormField; previous: FormField["value"]; docGeneration: DocGeneration }> {
  return call("set_form_field_value", a, (mock) => mock.setFormFieldValue(a));
}

export function resetForm(a: { docId: DocId }): Promise<DocInfo> {
  return call("reset_form", a, (mock) => mock.resetForm(a));
}

// ---------------------------------------------------------------------------
// 7.3 Pages
// ---------------------------------------------------------------------------

export function pageOps(a: { docId: DocId; ops: PageOp[] }): Promise<DocInfo> {
  return call("page_ops", a, (mock) => mock.pageOps(a));
}

export function extractPages(
  a: { docId: DocId; pages: PageIndex[]; outPath: string; removeAfter: boolean },
): Promise<{ bytes: number; docGeneration: DocGeneration }> {
  return call("extract_pages", a, (mock) => mock.extractPages(a));
}

export function splitDocument(
  a: { docId: DocId; mode: { everyN: number } | { ranges: string[] }; outDir: string },
  onProgress: (e: JobEvent) => void,
): Promise<JobId> {
  return call("split_document", { ...a, onProgress: channel(onProgress) }, (mock) => mock.splitDocument(a, onProgress));
}

export function mergeDocuments(
  a: { inputs: { path: string; range?: string; password?: string }[] },
): Promise<{ info: DocInfo; warnings: ("formsDropped" | "outlineDropped" | "metadataDropped")[] }> {
  return call("merge_documents", a, (mock) => mock.mergeDocuments(a));
}

// ---------------------------------------------------------------------------
// 7.4 Page objects
// ---------------------------------------------------------------------------

export function listPageObjects(a: { docId: DocId; page: PageIndex }): Promise<ObjectsResult> {
  return call("list_page_objects", a, (mock) => mock.listPageObjects(a));
}

export function probeTextEdit(
  a: { docId: DocId; page: PageIndex; objectId: ObjectId; text: string },
): Promise<TextEditProbe> {
  return call("probe_text_edit", a, (mock) => mock.probeTextEdit(a));
}

export function editTextObject(a: {
  docId: DocId; page: PageIndex; objectId: ObjectId; expectGeneration: DocGeneration;
  patch: { text?: string; fontSizePt?: number; color?: Rgb }; allowFontSubstitution: boolean;
}): Promise<ObjectsResult> {
  return call("edit_text_object", a, (mock) => mock.editTextObject(a));
}

export function addTextObject(a: {
  docId: DocId; page: PageIndex; rect: Rect; text: string; fontSizePt: number; color: Rgb;
  align: "left" | "center" | "right";
}): Promise<ObjectsResult> {
  return call("add_text_object", a, (mock) => mock.addTextObject(a));
}

export function addImageObject(
  a: { docId: DocId; page: PageIndex; rect: Rect; path: string; keepAspect: boolean },
): Promise<ObjectsResult> {
  return call("add_image_object", a, (mock) => mock.addImageObject(a));
}

export function transformObject(a: {
  docId: DocId; page: PageIndex; objectId: ObjectId; expectGeneration: DocGeneration;
  translate?: [number, number]; scale?: [number, number]; rotateDeg?: number;
}): Promise<ObjectsResult> {
  return call("transform_object", a, (mock) => mock.transformObject(a));
}

export function deleteObjects(
  a: { docId: DocId; page: PageIndex; objectIds: ObjectId[]; expectGeneration: DocGeneration },
): Promise<ObjectsResult> {
  return call("delete_objects", a, (mock) => mock.deleteObjects(a));
}

// ---------------------------------------------------------------------------
// 7.5 Redaction and security
// ---------------------------------------------------------------------------

export function redactPreview(a: { docId: DocId; page: PageIndex; rects: Rect[] }): Promise<RedactPreview> {
  return call("redact_preview", a, (mock) => mock.redactPreview(a));
}

export function applyRedactions(
  a: { docId: DocId; page: PageIndex; rects: Rect[]; options: { fill: Rgb; overlayText?: string } },
): Promise<{ removedObjects: number; verified: boolean; docGeneration: DocGeneration }> {
  return call("apply_redactions", a, (mock) => mock.applyRedactions(a));
}

export function removePassword(a: { docId: DocId; outPath: string }): Promise<{ bytes: number }> {
  return call("remove_password", a, (mock) => mock.removePassword(a) as Promise<{ bytes: number }>);
}

export function setPassword(a: {
  docId: DocId; outPath: string; userPassword?: string; ownerPassword: string; permissions: Partial<Permissions>;
}): Promise<{ bytes: number }> {
  return call("set_password", a, (mock) => mock.setPassword(a) as Promise<{ bytes: number }>);
}

export function removeMetadata(a: { docId: DocId }): Promise<DocInfo> {
  return call("remove_metadata", a, (mock) => mock.removeMetadata(a));
}

export function setMetadata(a: { docId: DocId; meta: DocMeta }): Promise<DocInfo> {
  return call("set_metadata", a, (mock) => mock.setMetadata(a));
}

// ---------------------------------------------------------------------------
// 7.5b Stamp and compress (Stage 4: P1-4 / P1-5)
// ---------------------------------------------------------------------------

/** 워터마크 / 머리글·바닥글 — one undo step (`undo.watermark` / `undo.headerFooter`). */
export function addStamp(a: { docId: DocId; spec: StampSpec }): Promise<StampResult> {
  return call("add_stamp", a, (mock) => mock.addStamp(a));
}

/**
 * 압축 예상: a job on a scratch copy. The `CompressReport` rides on the final `done` event
 * (`e.report`); its `token` is what `compressApply` / `compressDiscard` take.
 */
export function compressEstimate(
  a: { docId: DocId; options: CompressOptions },
  onEvent: (e: JobEvent) => void,
): Promise<JobId> {
  return call("compress_estimate", { ...a, onProgress: channel(onEvent) }, (mock) => mock.compressEstimate(a, onEvent));
}

export function compressApply(a: { docId: DocId; token: number }): Promise<DocInfo> {
  return call("compress_apply", a, (mock) => mock.compressApply(a));
}

export function compressDiscard(a: { docId: DocId; token: number }): Promise<void> {
  return call("compress_discard", a, (mock) => mock.compressDiscard(a));
}

// ---------------------------------------------------------------------------
// 7.5c Compare and crash recovery (Stage 5: P1-6 / P1-8)
// ---------------------------------------------------------------------------

/**
 * 문서 비교: a job over the page pairs of two **open** documents. The `CompareReport` rides on the
 * final `done` event (`e.compare`).
 */
export function compareDocuments(
  a: { docA: DocId; docB: DocId; options: CompareOptions },
  onEvent: (e: JobEvent) => void,
): Promise<JobId> {
  return call("compare_documents", { ...a, onProgress: channel(onEvent) }, (mock) => mock.compareDocuments(a, onEvent));
}

/** Serialise the current state of `docId` to the recovery dir (the user's file is never touched). */
export function writeRecovery(a: { docId: DocId }): Promise<RecoveryEntry> {
  return call("write_recovery", a, (mock) => mock.writeRecovery(a));
}

/** Drop `docId`'s recovery pair — after a successful save or a clean close. No-op if none. */
export function clearRecovery(a: { docId: DocId }): Promise<void> {
  return call("clear_recovery", a, (mock) => mock.clearRecovery(a));
}

/** Recovery copies left behind by a crash, newest first. */
export function listRecovery(): Promise<RecoveryEntry[]> {
  return call("list_recovery", {}, (mock) => mock.listRecovery());
}

export function discardRecovery(a: { id: string }): Promise<void> {
  return call("discard_recovery", a, (mock) => mock.discardRecovery(a));
}

// ---------------------------------------------------------------------------
// 7.6 Save
// ---------------------------------------------------------------------------

export function saveDocument(a: { docId: DocId }, onProgress: (e: JobEvent) => void): Promise<SaveResult> {
  return call("save_document", { ...a, onProgress: channel(onProgress) }, (mock) => mock.saveDocument(a, onProgress));
}

export function saveDocumentAs(
  a: { docId: DocId; path: string },
  onProgress: (e: JobEvent) => void,
): Promise<SaveResult> {
  return call("save_document_as", { ...a, onProgress: channel(onProgress) }, (mock) => mock.saveDocumentAs(a, onProgress));
}

/** `true` when something already exists at `path` (여러 파일 OCR picks `name-ocr (2).pdf` then). */
export function pathExists(a: { path: string }): Promise<boolean> {
  return call("path_exists", a, (mock) => mock.pathExists(a));
}

// ---------------------------------------------------------------------------
// 7.7 Export and print
// ---------------------------------------------------------------------------

export function exportImages(a: ExportImagesArgs, onProgress: (e: JobEvent) => void): Promise<JobId> {
  // `export_images(args: ExportImagesArgs, on_progress: Channel<_>)` — one named struct, same as
  // `set_viewport`. Spreading made it fail with "missing required key args" (Stage 2).
  return call("export_images", { args: a, onProgress: channel(onProgress) }, (mock) =>
    mock.exportImages(a, onProgress),
  );
}

export function exportText(a: { docId: DocId; pages: PageIndex[]; outPath: string }): Promise<{ chars: number }> {
  return call("export_text", a, (mock) => mock.exportText(a));
}

export function exportFlattened(
  a: { docId: DocId; outPath: string; annotations: boolean; forms: boolean; pages?: PageIndex[] },
  onProgress: (e: JobEvent) => void,
): Promise<JobId> {
  return call("export_flattened", { ...a, onProgress: channel(onProgress) }, (mock) => mock.exportFlattened(a, onProgress));
}

export function estimateExport(
  a: { docId: DocId; pages: PageIndex[]; format: "png" | "jpeg"; dpi: number },
): Promise<{ bytes: number; sampledPages: number }> {
  return call("estimate_export", a, (mock) => mock.estimateExport(a));
}

export function printPrepare(a: { docId: DocId; pages?: PageIndex[] }): Promise<{ tempPath: string }> {
  return call("print_prepare", a, (mock) => mock.printPrepare(a));
}

// ---------------------------------------------------------------------------
// 7.8 History
// ---------------------------------------------------------------------------

export function undo(a: { docId: DocId }): Promise<DocInfo> {
  return call("undo", a, (mock) => mock.undo(a));
}

export function redo(a: { docId: DocId }): Promise<DocInfo> {
  return call("redo", a, (mock) => mock.redo(a));
}

// ---------------------------------------------------------------------------
// 7.9 OCR
// ---------------------------------------------------------------------------

export function ocrCapabilities(): Promise<{ engines: OcrEngine[]; languages: string[] }> {
  return call("ocr_capabilities", {}, (mock) => mock.ocrCapabilities());
}

export function ocrPageStatus(
  a: { docId: DocId; pages: PageIndex[] },
): Promise<{ page: PageIndex; hasText: boolean; charCount: number }[]> {
  return call("ocr_page_status", a, (mock) => mock.ocrPageStatus(a));
}

export function ocrApply(
  a: { docId: DocId; pages: OcrPage[]; replaceExisting: boolean },
  onProgress: (e: JobEvent) => void,
): Promise<DocInfo> {
  return call("ocr_apply", { ...a, onProgress: channel(onProgress) }, (mock) => mock.ocrApply(a, onProgress));
}

export function ocrRecognizeNative(
  a: { docId: DocId; page: PageIndex; dpi: number; languages: string[] },
): Promise<OcrPage> {
  return call("ocr_recognize_native", a, async () => {
    throw new SeePdfError({ code: "unsupported", message: "native OCR is P1 (macOS Vision)" });
  });
}

// ---------------------------------------------------------------------------
// 11. App, settings, recents
// ---------------------------------------------------------------------------

export function getRecent(): Promise<RecentEntry[]> {
  return call("get_recent", {}, (mock) => mock.getRecent());
}

export function updateRecent(a: { entry: RecentEntry }): Promise<void> {
  return call("update_recent", a, (mock) => mock.updateRecent(a));
}

export function removeRecent(a: { path: string }): Promise<void> {
  return call("remove_recent", a, (mock) => mock.removeRecent(a));
}

export function setRecentPinned(a: { path: string; pinned: boolean }): Promise<void> {
  return call("set_recent_pinned", a, (mock) => mock.setRecentPinned(a));
}

export function clearRecent(): Promise<void> {
  return call("clear_recent", {}, (mock) => mock.clearRecent());
}

export function writeRecentThumbnail(a: { docId: DocId }): Promise<{ thumbId: string }> {
  return call("write_recent_thumbnail", a, (mock) => mock.writeRecentThumbnail(a));
}

export function revealInFileManager(a: { path: string }): Promise<void> {
  return call("reveal_in_file_manager", a, (mock) => mock.revealInFileManager(a));
}

export function getSettings(): Promise<Settings> {
  return call("get_settings", {}, (mock) => mock.getSettings());
}

export function setSettings(a: { patch: Partial<Settings> }): Promise<Settings> {
  return call("set_settings", a, (mock) => mock.setSettings(a));
}

/**
 * P1-9: a typed signature rendered to PNG → `$APPDATA/SeePDF/signatures/sig-<hash>.png`, so the
 * 서명 tool can place it as `StampImage { path }`. Resolves with the absolute path.
 */
export function writeSignatureImage(a: { bytes: Uint8Array }): Promise<string> {
  // A JSON array: `Vec<u8>` on the Rust side (a typed array would serialise as an object).
  const args = { bytes: Array.from(a.bytes) };
  return call("write_signature_image", args, (mock) => mock.writeSignatureImage(args));
}

// ---------------------------------------------------------------------------
// Native dialogs (IPC_CONTRACT §11 — the plugin commands, no npm wrapper needed)
// ---------------------------------------------------------------------------

export interface OpenDialogOptions {
  title?: string;
  multiple?: boolean;
  directory?: boolean;
  defaultPath?: string;
  filters?: { name: string; extensions: string[] }[];
}

const PDF_FILTER = [{ name: "PDF", extensions: ["pdf"] }];

export async function openFileDialog(options: OpenDialogOptions = {}): Promise<string[] | null> {
  if (useMock()) {
    // Distinct answers per shape, so 파일 합치기 can reach two inputs and 내보내기 gets a folder
    // that is not a file path (STAGE1E_NOTES §5.5).
    if (options.directory) return ["/Users/veri/Documents/SeePDF-내보내기"];
    if (options.multiple) {
      return [
        "/Users/veri/Documents/SeePDF-샘플.pdf",
        "/Users/veri/Documents/보고서-2024.pdf",
        "/Users/veri/Documents/tracemonkey.pdf",
      ];
    }
    return ["/Users/veri/Documents/SeePDF-샘플.pdf"];
  }
  const picked = await invoke<string | string[] | null>("plugin:dialog|open", {
    options: { filters: PDF_FILTER, multiple: false, directory: false, ...options },
  });
  if (picked === null) return null;
  return Array.isArray(picked) ? picked : [picked];
}

export async function saveFileDialog(options: { title?: string; defaultPath?: string } = {}): Promise<string | null> {
  if (useMock()) return "/Users/veri/Documents/SeePDF-샘플 (사본).pdf";
  return invoke<string | null>("plugin:dialog|save", { options: { filters: PDF_FILTER, ...options } });
}

/** Everything the annotation list needs to render an author string. */
export type { Annot };
