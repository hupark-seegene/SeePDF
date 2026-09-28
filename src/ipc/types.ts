/**
 * SeePDF IPC contract (v1) — the TypeScript half of `docs/IPC_CONTRACT.md`, verbatim.
 *
 * This file is FROZEN (WORKPLAN §0.2): it is changed only through the integrator, together with
 * `src-tauri/src/ipc/types.rs`. It contains types only — no `invoke`, no runtime code.
 *
 * Coordinates (§3): every Rect/Point/ink path is PDF user space — points (1/72"), origin bottom-left,
 * y-up, UNROTATED. Device pixels appear only in tile URLs (§9), OCR boxes (§7.9) and the
 * `X-Image-Width/Height` headers.
 */

// ---------------------------------------------------------------------------
// 2. Errors
// ---------------------------------------------------------------------------

export type ErrorCode =
  | 'passwordRequired'   // document is encrypted and no password was supplied
  | 'passwordWrong'      // supplied password rejected
  | 'notFound'           // unknown docId / page / annotId / objectId / path
  | 'invalidArgument'    // range string, page list, geometry validation failed
  | 'unsupported'        // the operation is impossible on this document (XFA, XObject text, Type3 font…)
  | 'permissionDenied'   // the PDF's own permission bits forbid it
  | 'readOnly'           // target file or volume is not writable -> caller should offer Save As
  | 'stale'              // expectGeneration mismatch, or a render whose viewport moved on
  | 'cancelled'          // job cancelled by the user
  | 'busy'               // engine queue bound exceeded; retry with backoff
  | 'fontCoverage'       // the requested text cannot be rendered by the target font
  | 'verifyFailed'       // save or redaction post-condition failed; the document was rolled back
  | 'pdfium'             // PdfiumError that maps to nothing more specific
  | 'io';                // filesystem error

export interface EngineError { code: ErrorCode; message: string; page?: number; detail?: string }

// ---------------------------------------------------------------------------
// 3. Shared types and coordinate conventions
// ---------------------------------------------------------------------------

export type DocId = string;          // "d1", "d2" — process-unique, never reused
export type DocGeneration = number;  // u32, +1 on every mutation of that document
export type PageIndex = number;      // 0-based
export type AnnotId = string;        // the annotation's /NM (uuid v4); stable across generations
export type ObjectId = number;       // page-object index; valid only within one docGeneration
export type JobId = number;
export type Rotation = 0 | 90 | 180 | 270;
export type Rgb = [r: number, g: number, b: number];            // 0..255
export interface Rect { l: number; b: number; r: number; t: number }   // PDF points, y-up
export type Mat6 = [a: number, b: number, c: number, d: number, e: number, f: number];
export type Point = [x: number, y: number];

// ---------------------------------------------------------------------------
// 4. Documents
// ---------------------------------------------------------------------------

export interface PageGeom {
  index: PageIndex; widthPt: number; heightPt: number;   // display size (rotation applied)
  rotation: Rotation; crop: Rect; label: string | null;  // label from /PageLabels, read-only in v1
}
export interface Permissions {
  print: boolean; modify: boolean; extractText: boolean; annotate: boolean;
  fillForms: boolean; assemble: boolean; revision: 'unprotected' | 'r2' | 'r3' | 'r4' | 'r5' | 'r6' | 'unknown';
}
export interface DocMeta {
  title?: string; author?: string; subject?: string; keywords?: string;
  creator?: string; producer?: string; created?: string; modified?: string;   // raw "D:YYYYMMDD…" strings
}
export interface DocInfo {
  docId: DocId; path: string | null; name: string; bytes: number;
  pageCount: number; pages: PageGeom[];
  docGeneration: DocGeneration; dirty: boolean;
  canUndo: boolean; canRedo: boolean; undoLabel: string | null; redoLabel: string | null;  // i18n keys
  encrypted: boolean; permissions: Permissions; hasForm: boolean; xfa: boolean;
  hasOutline: boolean; meta: DocMeta; pdfVersion: string; tagged: boolean;
}
/** Where on the page a `/Dest` points, PDF user space. Absent for a plain page reference. */
export interface OutlineDest { x?: number; y?: number; zoom?: number }
export interface OutlineNode {
  title: string; page: PageIndex | null;
  dest?: OutlineDest;                     // Stage 2: scroll to the heading, not to the page top
  children: OutlineNode[];
}
export interface OpenRequest { path: string; source: 'argv' | 'macos-opened' | 'drop' | 'dialog' | 'recent' }

// ---------------------------------------------------------------------------
// 5. View, render control, statistics
// ---------------------------------------------------------------------------

export interface ViewportHint {
  docId: DocId; scaleKey: number;                 // round(zoomPercent * devicePixelRatio)
  rotation: Rotation; centrePage: PageIndex; firstPage: PageIndex; lastPage: PageIndex;
  velocityPxPerMs: number;
}

export interface EngineStats {
  docs: number; queueDepth: Record<'interactive' | 'edit' | 'prefetch' | 'thumb' | 'background', number>;
  droppedStale: number; tileP50Ms: number; tileP95Ms: number; encodeP50Ms: number;
  tileCacheBytes: number; tileCacheHitRate: number; pageLruLen: number; rssBytes: number;
}

// ---------------------------------------------------------------------------
// 6. Text, selection, search
// ---------------------------------------------------------------------------

export interface SearchHit {
  page: PageIndex; charStart: number; charLength: number;
  rects: Rect[];                    // one per line the hit spans
  context: string; contextMatch: [start: number, length: number];   // ±40 chars for the results list
}
export type SearchEvent =
  | { type: 'page'; page: PageIndex; hits: SearchHit[]; scanned: number; total: number }
  | { type: 'done'; total: number; pagesScanned: number; elapsedMs: number }
  | { type: 'cancelled'; scanned: number }
  | { type: 'error'; error: EngineError };

// ---------------------------------------------------------------------------
// 7.1 Annotations
// ---------------------------------------------------------------------------

export type AnnotKind =
  | 'highlight' | 'underline' | 'strikeout' | 'squiggly'
  | 'ink' | 'square' | 'circle' | 'note' | 'textbox' | 'stamp' | 'signature'
  | 'line' | 'arrow'                          // stored as Ink + /Subj (PDFium cannot create /Line)
  | 'link' | 'widget' | 'other';

export interface Annot {
  id: AnnotId; page: PageIndex; kind: AnnotKind; subtype: string;   // raw PDF subtype, e.g. "Ink"
  rect: Rect;
  quads?: Rect[];                       // markup: one rect per line run (engine converts to TL,TR,BL,BR quads)
  inkPaths?: number[][];                // ink/line/arrow: [x0,y0,x1,y1,…] per stroke
  linePoints?: [number, number, number, number];   // line/arrow convenience: x1,y1,x2,y2
  color: Rgb; fillColor: Rgb | null; opacity: number;   // opacity 0..1 == /CA
  borderWidth: number;
  contents: string; author: string | null; created: string | null; modified: string | null;
  text?: string; fontSize?: number;     // textbox
  stampKind?: string; imageId?: string; // stamp/signature
  uri?: string;                         // link
  hidden: boolean; printed: boolean; locked: boolean;
  editable: 'full' | 'moveOnly' | 'readOnly';   // moveOnly = third-party AP we would regenerate
}
export interface AnnotList { docId: DocId; page: PageIndex; docGeneration: DocGeneration; annots: Annot[] }
export interface AnnotResult { list: AnnotList; annot: Annot | null; previous: Annot | null }

export type AnnotSpec =
  | { kind: 'highlight' | 'underline' | 'strikeout' | 'squiggly'; rects: Rect[]; color: Rgb; opacity: number; contents?: string }
  | { kind: 'note'; at: Point; color: Rgb; contents: string }
  | { kind: 'ink' | 'signature'; paths: number[][]; color: Rgb; width: number; opacity: number }
  | { kind: 'square' | 'circle'; rect: Rect; color: Rgb; fillColor: Rgb | null; width: number; opacity: number }
  | {
      kind: 'line' | 'arrow'; p1: Point; p2: Point; color: Rgb; width: number; opacity: number;
      heads?: [start: boolean, end: boolean];
    }
  | {
      kind: 'textbox'; rect: Rect; text: string; fontSize: number; color: Rgb;
      align: 'left' | 'center' | 'right'; fillColor: Rgb | null;
    }
  | {
      kind: 'stamp'; rect: Rect; image: { path: string } | { builtin: string }; rotate?: number;
      /** Stage 8: written with `/Subj "SeePDF:Signature"` and read back as `kind: 'signature'` (서명, not 도장) */
      signature?: boolean;
    };

export interface AnnotPatch {
  rect?: Rect; rects?: Rect[]; paths?: number[][]; p1?: Point; p2?: Point;
  color?: Rgb; fillColor?: Rgb | null; opacity?: number; borderWidth?: number;
  contents?: string; author?: string; text?: string; fontSize?: number; locked?: boolean;
}

export type AnnotScanEvent =
  | { type: 'page'; page: PageIndex; annots: Annot[] }
  | { type: 'done'; total: number } | { type: 'cancelled' };

// ---------------------------------------------------------------------------
// 7.2 Forms
// ---------------------------------------------------------------------------

export type FieldType = 'text' | 'checkbox' | 'radio' | 'combo' | 'list' | 'button' | 'signature' | 'unknown';
export interface FormField {
  page: PageIndex; index: number; name: string; type: FieldType; rect: Rect;
  value: string | null; checked?: boolean; options?: { label: string; selected: boolean }[];
  readOnly: boolean; required: boolean; multiline?: boolean; comb?: boolean; maxLen?: number; fontSizePt?: number;
}
export type FieldValue = { text: string } | { checked: boolean } | { selected: number[] };

// ---------------------------------------------------------------------------
// 7.3 Pages
// ---------------------------------------------------------------------------

export type PageOp =
  | { kind: 'move'; pages: PageIndex[]; to: PageIndex }        // FPDF_MovePages ordered-list semantics
  | { kind: 'delete'; pages: PageIndex[] }
  | { kind: 'rotate'; pages: PageIndex[]; delta: 90 | 180 | 270 }
  | { kind: 'insertBlank'; at: PageIndex; size: { widthPt: number; heightPt: number } | 'a4' | 'letter' | 'sameAs' }
  | { kind: 'duplicate'; pages: PageIndex[] }
  | { kind: 'insertFrom'; at: PageIndex; path: string; range?: string; password?: string }   // "1-3,5", 1-based
  | { kind: 'reverse' };

export type MergeWarning = 'formsDropped' | 'outlineDropped' | 'metadataDropped';

// ---------------------------------------------------------------------------
// 7.4 Page objects (text and image editing)
// ---------------------------------------------------------------------------

export interface PageObject {
  objectId: ObjectId; type: 'text' | 'image' | 'path' | 'shading' | 'form' | 'other';
  rect: Rect; matrix: Mat6; ours: boolean;
  text?: string; fontName?: string; fontSizePt?: number; color?: Rgb;
  editable: 'full' | 'moveOnly' | 'readOnly';
  reason?: 'insideXObject' | 'noUnicode' | 'type3' | 'invisible' | 'permissions';
}
export interface TextEditProbe {
  strategy: 'inPlace' | 'replaceFont' | 'refused';
  substituteFont?: string;                 // e.g. "SeePDF Hangul" — the UI must confirm before committing
  reason?: PageObject['reason'] | 'glyphsMissing';
}
export interface ObjectsResult { objects: PageObject[]; docGeneration: DocGeneration }
/** Stage 8 `duplicate_objects`: `objects` and `newObjectIds` belong to `targetPage ?? page`. */
export type DuplicateObjectsResult = ObjectsResult & { newObjectIds: ObjectId[] };

/** Stage 7 — paragraph editing with reflow (`probe_paragraph` / `edit_paragraph`). */
export type ParagraphAlign = 'left' | 'center' | 'right' | 'justify';
export interface ParagraphProbe {
  objectIds: ObjectId[];          // every text object of the paragraph, reading order
  rect: Rect;                     // union of their bounds
  text: string;                   // soft wraps joined with ' '; '\n' only for a hard break the detector kept
  fontName: string; fontSizePt: number; color: Rgb;       // the dominant style (most characters)
  mixedStyles: boolean;           // more than one font / size / colour inside → the UI warns the styles merge
  lineHeightPt: number;           // baseline-to-baseline; for a single line = fontSize × 1.2
  align: ParagraphAlign;
  firstLineIndentPt: number;      // may be negative (hanging)
  lines: number;
  strategy: 'inPlace' | 'replaceFont' | 'refused';        // for the CURRENT text, as TextEditProbe
  substituteFont?: string;
  reason?: PageObject['reason'] | 'glyphsMissing' | 'rotatedText';
  docGeneration?: DocGeneration;  // the generation `objectIds` belong to (the engine sends it; optional here)
}
export interface ParagraphEdit {
  objectIds: ObjectId[];          // from the probe
  text: string;                   // '\n' = hard line break inside the paragraph
  width?: number;                 // new box width in pt (default: probe rect width)
  fontSizePt?: number; color?: Rgb; align?: ParagraphAlign;
}
export interface ParagraphEditResult {
  objects: ObjectsResult;         // page objects after the edit
  rect: Rect;                     // box actually occupied by the new text
  lines: number;
  overflowPt: number;             // how far the new text extends below the original rect bottom (0 if not)
}

// ---------------------------------------------------------------------------
// 7.5 Redaction and security
// ---------------------------------------------------------------------------

/** Stage 8 `apply_redactions_batch`: one page's marks. */
export interface RedactBatchMark { page: PageIndex; rects: Rect[] }
/** Stage 8 `apply_redactions_batch`: every page in ONE undo step (`undo.redact`). */
export interface RedactBatchResult {
  removedObjects: number; verified: boolean; docGeneration: DocGeneration; pages: PageIndex[];
}
export interface RedactPreview {
  page: PageIndex;
  textObjects: { objectId: ObjectId; text: string; rect: Rect; fullyInside: boolean }[];
  imageObjects: { objectId: ObjectId; rect: Rect; fullyInside: boolean }[];
  annotations: AnnotId[];
  formFields: string[];              // non-empty ⇒ the apply will refuse
  collateral: string[];              // text that will be removed although it is outside the marks
}

// ---------------------------------------------------------------------------
// 7.5b Stamp (P1-4 워터마크 / 머리글·바닥글) and compress (P1-5 압축) — Stage 4 contract
// ---------------------------------------------------------------------------

export type StampAnchor = 'tl' | 'tc' | 'tr' | 'ml' | 'mc' | 'mr' | 'bl' | 'bc' | 'br';
export type StampRole = 'watermark' | 'header' | 'footer';
export type StampSource =
  | { kind: 'text'; text: string; fontSizePt: number; color: Rgb }
  | { kind: 'image'; path: string; widthPt: number };          // height follows the image aspect
export interface StampSpec {
  role: StampRole;            // only affects the undo label + docs; geometry comes from anchor/margins
  source: StampSource;
  anchor: StampAnchor;
  marginPt: number;           // distance from the page edge for non-centre anchors (>= 0)
  rotateDeg: number;          // -180..180, about the stamp centre; 0 for header/footer
  opacity: number;            // 0..1 (fill + stroke alpha)
  pages: PageIndex[] | 'all';
}
export interface StampResult { info: DocInfo; pagesStamped: number }
/** Stage 8 `remove_stamps`: `removed == 0` is not an error. */
export interface RemoveStampsResult { info: DocInfo; removed: number }

export type CompressPreset = 300 | 150 | 96;   // target DPI for raster images
export interface CompressOptions {
  targetDpi: CompressPreset;
  pages?: PageIndex[];         // default all
}
export interface CompressReport {
  token: number;               // pending-result handle, valid until apply/discard or doc close
  beforeBytes: number; afterBytes: number;
  imagesTotal: number; imagesDownsampled: number;
  elapsedMs: number;
}

// ---------------------------------------------------------------------------
// 7.5c Compare (P1-6 문서 비교) and autosave / crash recovery (P1-8) — Stage 5 contract
// ---------------------------------------------------------------------------

export interface CompareOptions {
  pagesA?: PageIndex[];        // default all pages of A
  pagesB?: PageIndex[];        // default all pages of B; paired with pagesA by position; the longer list's
                               // extra pages become ComparePage rows with pageA/pageB = null
  ignoreCase?: boolean;        // default false; whitespace runs are always normalised
  /**
   * Stage 8, default true: pair pages by text similarity (sequence alignment over per-page word
   * sets), so an inserted / deleted page becomes a null-sided row instead of shifting every later pair.
   */
  alignPages?: boolean;
}
export type DiffKind = 'equal' | 'insert' | 'delete' | 'replace';
export interface DiffOp {
  kind: DiffKind;
  words: number;               // words on the A side for equal/delete/replace, B side for insert
  textA?: string;              // omitted for equal and insert
  textB?: string;              // omitted for equal and delete
  rectsA?: Rect[];             // PDF points on page A (y-up), one rect per line fragment; omitted for equal/insert
  rectsB?: Rect[];             // same on page B; omitted for equal/delete
}
export interface ComparePage {
  pageA: PageIndex | null; pageB: PageIndex | null;
  changed: boolean;            // any op that is not equal (a null side counts as changed if the other has words)
  wordsA: number; wordsB: number;
  ops: DiffOp[];
}
export interface CompareReport {
  docA: DocId; docB: DocId;
  pages: ComparePage[];
  changedPages: number; inserted: number; deleted: number;   // word counts (replace counts on both)
  elapsedMs: number;
}

export interface RecoveryEntry {
  id: string;                   // uuid v4, also the file stem in the recovery dir
  originalPath: string | null;  // null for a never-saved document
  name: string;                 // display name (file name or 제목 없음)
  savedAt: string;              // ISO-8601
  bytes: number; pages: number;
  recoveryPath: string;         // absolute path of the .pdf copy
}

// ---------------------------------------------------------------------------
// 7.6 Save
// ---------------------------------------------------------------------------

export interface SaveResult { docId: DocId; path: string; bytes: number; docGeneration: DocGeneration; elapsedMs: number }

// ---------------------------------------------------------------------------
// 7.7 Export and print
// ---------------------------------------------------------------------------

export interface ExportImagesArgs {
  docId: DocId; pages: PageIndex[]; format: 'png' | 'jpeg'; dpi: number; quality?: number;
  outDir: string; baseName: string; transparentBackground?: boolean;
}

// ---------------------------------------------------------------------------
// 7.9 OCR
// ---------------------------------------------------------------------------

export type Box = [x0: number, y0: number, x1: number, y1: number];   // IMAGE pixels, origin top-left
export interface OcrWord { text: string; bbox: Box; confidence: number }
export interface OcrLine {
  text: string; bbox: Box; baseline?: [number, number, number, number];
  rowHeightPx?: number; words: OcrWord[];
}
export interface OcrPage {
  page: PageIndex; dpi: number; widthPx: number; heightPx: number;
  rotation: Rotation; lines: OcrLine[];
}
export type OcrEngine = 'tesseract' | 'vision' | 'windows';
/** One element of `ocr_apply.pages`: the page itself, or the Stage 8 `{ page, ocr }` form (same batch). */
export type OcrApplyPage = OcrPage | { page: PageIndex; ocr: OcrPage };

// ---------------------------------------------------------------------------
// 8. Events and progress
// ---------------------------------------------------------------------------

export interface OpenFileEvent { path: string; source: OpenRequest['source'] }
export interface DocChangedEvent {
  docId: DocId; docGeneration: DocGeneration; changedPages: PageIndex[] | 'all';
  structure: boolean; dirty: boolean; reason: 'edit' | 'undo' | 'redo' | 'save' | 'pages' | 'ocr' | 'redact';
  /** Stage 2: history state rides along, so ⌘Z/⇧⌘Z need no `get_document` per edit. */
  canUndo: boolean; canRedo: boolean;
}
export interface DocSavedEvent { docId: DocId; path: string; docGeneration: DocGeneration }
export interface RecentsChangedEvent {}
export interface EnginePressureEvent { level: 'normal' | 'high' }

export type JobEvent =
  | { type: 'started'; jobId: JobId; total: number }
  | { type: 'progress'; jobId: JobId; done: number; total: number; page?: PageIndex; note?: string }
  | { type: 'done'; jobId: JobId; elapsedMs: number; outputs?: string[]; report?: CompressReport; compare?: CompareReport }
  | { type: 'cancelled'; jobId: JobId; done: number }
  | { type: 'error'; jobId: JobId; error: EngineError };

// ---------------------------------------------------------------------------
// 11. App, settings, recents
// ---------------------------------------------------------------------------

export interface RecentEntry {
  path: string; name: string; dir: string; pages: number; bytes: number;
  lastOpened: string; lastPage: PageIndex; zoomPercent: number;
  layout: 'single' | 'continuous' | 'two'; pinned: boolean; thumbId: string | null;
}

export interface Settings {
  locale: 'ko' | 'en'; theme: 'system' | 'light' | 'dark';
  defaultLayout: 'single' | 'continuous' | 'two'; defaultZoom: 'fit-width' | 'fit-page' | 'actual' | number;
  restorePosition: boolean; author: string; renderQuality: 'balanced' | 'high';
  tileCacheMb: number; recentsCount: number;   // Stage 2: no longer inside `toolDefaults`
  backupsEnabled: boolean; ocrLanguages: string[]; ocrDpi: 'auto' | 200 | 300 | 400;
  autosaveSec: number;          // Stage 5 (P1-8): 0 = off; default 60
  /** per-tool style overrides, keyed by tool id (Stage 6b, P1-12); see `store/toolStyles.ts` */
  toolDefaults: Record<string, unknown>;
  /** 서명 보관함, at most 10, newest last (Stage 6b, P1-9); `[]` in settings written before it */
  signatures: SavedSignature[];
  /** 야간 모드, persisted across launches (Stage 8); `'off'` in settings written before it */
  night: 'off' | 'dark' | 'sepia';
}

/** One entry of the 서명 보관함. Drawn strokes are unit space (0…1 of the drawn box, y-down). */
export type SavedSignature =
  | { kind: 'drawn'; id: string; paths: number[][]; aspect: number; createdAt: string }
  | { kind: 'typed'; id: string; text: string; style: string; createdAt: string };

// ---------------------------------------------------------------------------
// Convenience aliases used by the shell (not part of the wire format)
// ---------------------------------------------------------------------------

/** View layout as it is stored in `Settings`/`RecentEntry` (`'two'`), see UI_SPEC §8. */
export type ViewLayout = Settings['defaultLayout'];
export type ThemePref = Settings['theme'];
export type ZoomPref = Settings['defaultZoom'];
