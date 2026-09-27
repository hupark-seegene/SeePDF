# SeePDF — IPC contract (v1)

This is **the seam** between the Rust engine and the React frontend. It is frozen at the end of Stage 0
and changed only through the integrator (see `WORKPLAN.md`). Every command listed here has an owner
module in `WORKPLAN.md` and a feature id in `FEATURES.md`; §12 is the index that proves it.

Files that implement this document:
`src/ipc/types.ts` (the TypeScript below, verbatim), `src/ipc/api.ts` (one typed wrapper per command —
the only file that imports `invoke`), `src/ipc/events.ts`, `src/ipc/protocol.ts` (URL builders),
`src/ipc/mock.ts` (fixture-backed adapter so frontend modules run in plain `vite dev`),
`src-tauri/src/ipc/types.rs` (the same structs with serde), `src-tauri/src/commands/*.rs`.

---

## 1. Conventions

* Commands are `snake_case` Rust functions registered once in `lib.rs`'s `generate_handler!`; arguments
  and fields cross the wire as **camelCase** (`#[serde(rename_all = "camelCase")]`, and
  `rename_all_fields = "camelCase"` on tagged enums — a tauri serde gotcha).
* Every command takes exactly one object argument and returns `Result<T, EngineError>`; JS receives the
  error object on rejection. `api.ts` re-throws it as `SeePdfError` (an `Error` carrying `code`).
* Every command that touches pdfium is `async` and forwards to the engine thread
  (`engine.call(lane, label, move |st| …).await`). Async tauri commands that borrow `State` **must**
  return `Result` (tauri macro restriction).
* **Pixels never travel through a command.** They travel over `seepdf://` (§9). Binary data the frontend
  must index into (`get_text_layer`, `render_page_raw`) travels as `tauri::ipc::Response` →
  `ArrayBuffer` (§10).
* Progress travels on a per-invocation `tauri::ipc::Channel<T>`; broadcasts travel on `emit`/`listen` (§8).
* Any command that mutates a document returns the new `docGeneration` (usually inside a fresh `DocInfo`
  or a `*Result`), so the frontend never needs a follow-up "refresh" round trip.

---

## 2. Errors

```ts
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
```

```rust
// src-tauri/src/ipc/types.rs
#[derive(Debug, Clone, thiserror::Error, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EngineError { pub code: ErrorCode, pub message: String, pub page: Option<u16>, pub detail: Option<String> }
```

`message` is an English developer string. The UI never shows it directly: it maps `code` to an i18n key
(`error.*` in `UI_SPEC.md` §12) and puts `message` behind 자세히.

---

## 3. Shared types and coordinate conventions

```ts
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
```

**Coordinates.** Every rectangle, point and ink path in this contract is in **PDF user space**:
points (1/72 inch), origin bottom-left, y-up, **unrotated** (the page's `/Rotate` is *not* applied).
Device pixels appear in exactly three places: tile URLs (§9), OCR boxes (§7.9, image pixels, origin
**top-left**), and `X-Image-Width/Height` response headers.

The conversion is one matrix per page, built from the **crop box** (not `page_size()`, which is already
rotated) plus `/Rotate` plus the view rotation plus the scale. Verified against `FPDF_PageToDevice` for
0° and 90° (text spike §2); both sides implement it and a cross-test compares them:

```ts
// src/viewer/geometry.ts — page user space -> device px (y-down), for scale s
export function pageToDevice(g: PageGeom, viewRotation: Rotation, s: number): Mat6 {
  const deg = (g.rotation + viewRotation) % 360;
  const { l, b, r, t } = g.crop;
  switch (deg) {
    case 90:  return [0,  s,  s,  0, -b * s, -l * s];
    case 180: return [-s, 0,  0,  s,  r * s, -b * s];
    case 270: return [0, -s, -s,  0,  t * s,  r * s];
    default:  return [s,  0,  0, -s, -l * s,  t * s];
  }
}
// dx = a*x + c*y + e ; dy = b*x + d*y + f   (row-vector convention)
```

`PageGeom.widthPt/heightPt` are the **display** (rotated) size — what the layout uses — while `crop` is
unrotated user space — what the matrix uses.

**Quads** never cross the IPC boundary. The frontend sends line rectangles; the engine builds
`FS_QUADPOINTSF` in PDFium's TL,TR,BL,BR order (`PdfQuadPoints::from_rect()` is the wrong order and
renders a 1-px sliver).

**Generations.** `DocInfo.docGeneration` increments on every mutation. It appears in every tile URL, in
every cache key, and in `expectGeneration` on commands that address a page object by index. A protocol
request for an older generation is answered `410 Gone`; a command with a mismatching `expectGeneration`
fails with `stale` and the frontend re-lists and retries.

---

## 4. Documents

```ts
export interface PageGeom {
  index: PageIndex; widthPt: number; heightPt: number;   // display size (rotation applied)
  rotation: Rotation; crop: Rect; label: string | null;  // label from /PageLabels, read-only in v1
}
export interface Permissions {
  print: boolean; modify: boolean; extractText: boolean; annotate: boolean;
  fillForms: boolean; assemble: boolean; revision: 'unprotected' | 'r2' | 'r3' | 'r4' | 'r5' | 'r6' | 'unknown';   // r5/r6 = AES-256 (Stage 4)
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
/** Stage 2: where on the page the heading is (PDF user space). Absent for a plain page jump. */
export interface OutlineDest { x?: number; y?: number; zoom?: number }
export interface OutlineNode {
  title: string; page: PageIndex | null; dest?: OutlineDest; children: OutlineNode[];
}
export interface OpenRequest { path: string; source: 'argv' | 'macos-opened' | 'drop' | 'dialog' | 'recent' }

open_document(a: { path: string; password?: string }): Promise<DocInfo>
close_document(a: { docId: DocId }): Promise<void>
get_document(a: { docId: DocId }): Promise<DocInfo>
get_outline(a: { docId: DocId }): Promise<OutlineNode[]>
take_pending_opens(): Promise<OpenRequest[]>
open_in_new_window(a: { path?: string }): Promise<string>              // returns the window label
window_bind_document(a: { label: string; docId: DocId | null }): Promise<void>
```

```rust
#[tauri::command]
pub async fn open_document(engine: State<'_, EngineHandle>, path: String, password: Option<String>)
    -> Result<DocInfo, EngineError> {
    let bytes = io::read(&path).await?;                       // seepdf-io thread
    engine.call(Lane::Edit, "open_document", move |st| registry::open(st, path, bytes, password)).await
}
```

| Command | Engine op | Owner | Feature |
|---|---|---|---|
| `open_document` | `registry::open` → `load_pdf_from_byte_vec` + `page_sizes()` | S0 engine-core | F-01 |
| `close_document` | `registry::close` (pages → form → doc) | S0 | F-01 |
| `get_document` | registry read | S0 | F-01 |
| `get_outline` | `bookmarks().iter()` (depth-first; `root()` is the first top-level node, not a synthetic root) | S0 | F-05 |
| `take_pending_opens` | `PendingOpens` queue drain | S0 | F-01 |
| `open_in_new_window`, `window_bind_document` | `app/windows.rs` | S0 | F-01 |

`open_document` errors: `passwordRequired` (no password given), `passwordWrong` (one was), `notFound`,
`pdfium` (corrupt). `DocInfo.xfa` true ⇒ the form layer is read-only with a banner.

---

## 5. View, render control, statistics

```ts
export interface ViewportHint {
  docId: DocId; scaleKey: number;                 // round(zoomPercent * devicePixelRatio)
  rotation: Rotation; centrePage: PageIndex; firstPage: PageIndex; lastPage: PageIndex;
  velocityPxPerMs: number;
}
set_viewport(a: ViewportHint): Promise<void>       // fire-and-forget; bumps the engine's viewport generation
render_page_raw(a: { docId: DocId; page: PageIndex; scale: number; rect?: Rect }): Promise<ArrayBuffer>  // §10.2
engine_stats(): Promise<EngineStats>

export interface EngineStats {
  docs: number; queueDepth: Record<'interactive'|'edit'|'prefetch'|'thumb'|'background', number>;
  droppedStale: number; tileP50Ms: number; tileP95Ms: number; encodeP50Ms: number;
  tileCacheBytes: number; tileCacheHitRate: number; pageLruLen: number; rssBytes: number;
}
```

`render_page_raw` exists for the 스냅샷 tool and the print path (regions the frontend composites itself);
tiles never use it. Owner: S0 (render). Features F-02, F-26.

---

## 6. Text, selection, search

```ts
get_text_layer(a: { docId: DocId; page: PageIndex }): Promise<ArrayBuffer>   // binary layout in §10.1
get_page_text(a: { docId: DocId; page: PageIndex }): Promise<string>

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

search_start(a: { docId: DocId; query: string; matchCase: boolean; wholeWord: boolean; fromPage: PageIndex },
             onEvent: Channel<SearchEvent>): Promise<JobId>
cancel_job(a: { jobId: JobId }): Promise<boolean>
```

```rust
#[tauri::command]
pub async fn search_start(engine: State<'_, EngineHandle>, jobs: State<'_, Jobs>, doc_id: String,
    query: String, match_case: bool, whole_word: bool, from_page: u16,
    on_event: tauri::ipc::Channel<SearchEvent>) -> Result<u64, EngineError>
```

| Command | Engine op | Owner | Feature |
|---|---|---|---|
| `get_text_layer` | `text::layer(st, doc, page)` (cached per generation) | S0 | F-06 |
| `get_page_text` | `page.text()?.all()` (cached) | S0 | F-06, F-25 |
| `search_start` | one `Lane::Background` command per page, outward from `fromPage`, shared `JobToken` | S0 | F-07 |
| `cancel_job` | flips the job's `AtomicBool` | S0 | F-07, F-21, F-24 |

---

## 7. Editing commands

### 7.1 Annotations

```ts
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
  | { kind: 'line' | 'arrow'; p1: Point; p2: Point; color: Rgb; width: number; opacity: number;
      heads?: [start: boolean, end: boolean] }
  | { kind: 'textbox'; rect: Rect; text: string; fontSize: number; color: Rgb;
      align: 'left' | 'center' | 'right'; fillColor: Rgb | null }
  | { kind: 'stamp'; rect: Rect; image: { path: string } | { builtin: string }; rotate?: number };

export interface AnnotPatch {
  rect?: Rect; rects?: Rect[]; paths?: number[][]; p1?: Point; p2?: Point;
  color?: Rgb; fillColor?: Rgb | null; opacity?: number; borderWidth?: number;
  contents?: string; author?: string; text?: string; fontSize?: number; locked?: boolean;
}

list_annotations(a: { docId: DocId; page: PageIndex }): Promise<AnnotList>
scan_annotations(a: { docId: DocId }, onEvent: Channel<AnnotScanEvent>): Promise<JobId>
create_annotation(a: { docId: DocId; page: PageIndex; spec: AnnotSpec; id?: AnnotId }): Promise<AnnotResult>
update_annotation(a: { docId: DocId; page: PageIndex; id: AnnotId; patch: AnnotPatch }): Promise<AnnotResult>
delete_annotations(a: { docId: DocId; page: PageIndex; ids: AnnotId[] }): Promise<AnnotResult>
set_annotations_hidden(a: { docId: DocId; page: PageIndex; ids: AnnotId[]; hidden: boolean }): Promise<{ viewNonce: number }>  // P1

export type AnnotScanEvent =
  | { type: 'page'; page: PageIndex; annots: Annot[] }
  | { type: 'done'; total: number } | { type: 'cancelled' };
```

`create_annotation` accepts an explicit `id` so redo re-creates an annotation with the same `/NM`.
`update_annotation` returns `previous` so the frontend can show a precise undo label; the actual undo is
the engine snapshot (§7.8).

| Command | Engine op (`engine/annot/`) | Owner | Feature |
|---|---|---|---|
| `list_annotations` | high-level enumeration + raw `GetNumberValue("CA")`, `GetBorder`, `GetInkListPath`, `GetLine`, `GetAP` length; assigns `/NM` where missing | (a) | F-14 |
| `scan_annotations` | one `Lane::Background` command per page | (a) | F-14 |
| `create_annotation` | `raw::create_annot` pipeline (ARCHITECTURE §6.1) inside `registry::mutate` | (a) | F-08…F-13 |
| `update_annotation` | `SetAP(NULL)` → setters → next render regenerates | (a) | F-14 |
| `delete_annotations` | `annotations_mut().get(i)` → `delete_annotation` (+ linked popup) | (a) | F-14 |
| `set_annotations_hidden` | `FPDFAnnot_SetFlags` HIDDEN, transient: bumps `viewNonce`, **not** `docGeneration`, never dirties | (a) | P1 |

### 7.2 Forms

```ts
export type FieldType = 'text' | 'checkbox' | 'radio' | 'combo' | 'list' | 'button' | 'signature' | 'unknown';
export interface FormField {
  page: PageIndex; index: number; name: string; type: FieldType; rect: Rect;
  value: string | null; checked?: boolean; options?: { label: string; selected: boolean }[];
  readOnly: boolean; required: boolean; multiline?: boolean; comb?: boolean; maxLen?: number; fontSizePt?: number;
}
export type FieldValue = { text: string } | { checked: boolean } | { selected: number[] };

list_form_fields(a: { docId: DocId; page?: PageIndex }): Promise<FormField[]>
set_form_field_value(a: { docId: DocId; page: PageIndex; index: number; value: FieldValue }):
  Promise<{ field: FormField; previous: FormField['value']; docGeneration: DocGeneration }>
reset_form(a: { docId: DocId }): Promise<DocInfo>           // P1
```

Engine: `engine/form/` — `FORM_SetFocusedAnnot` → `FORM_SelectAllText` → `FORM_ReplaceSelection`
(**unverified**; the module ships with the verified `FORM_OnLButtonDown/Up` + per-character `FORM_OnChar`
fallback behind a unit test) → `FORM_ForceToKillFocus`. Owner (a). Feature F-20.
Field highlight is **not** a command: it is the `hl=1` tile-URL parameter (§9). Neither is
suppressing the widget the overlay replaces: that is `forms=0` on the same URLs (§9, F-20).

### 7.3 Pages

```ts
export type PageOp =
  | { kind: 'move'; pages: PageIndex[]; to: PageIndex }        // FPDF_MovePages ordered-list semantics
  | { kind: 'delete'; pages: PageIndex[] }
  | { kind: 'rotate'; pages: PageIndex[]; delta: 90 | 180 | 270 }
  | { kind: 'insertBlank'; at: PageIndex; size: { widthPt: number; heightPt: number } | 'a4' | 'letter' | 'sameAs' }
  | { kind: 'duplicate'; pages: PageIndex[] }
  | { kind: 'insertFrom'; at: PageIndex; path: string; range?: string; password?: string }   // "1-3,5", 1-based
  | { kind: 'reverse' };

page_ops(a: { docId: DocId; ops: PageOp[] }): Promise<DocInfo>
extract_pages(a: { docId: DocId; pages: PageIndex[]; outPath: string; removeAfter: boolean }):
  Promise<{ bytes: number; docGeneration: DocGeneration }>
split_document(a: { docId: DocId; mode: { everyN: number } | { ranges: string[] }; outDir: string },
               onProgress: Channel<JobEvent>): Promise<JobId>
merge_documents(a: { inputs: { path: string; range?: string; password?: string }[] }):
  Promise<{ info: DocInfo; warnings: ('formsDropped' | 'outlineDropped' | 'metadataDropped')[] }>
```

Engine: `engine/pages/`. One `page_ops` call is one `registry::mutate` = one generation = one undo step.
`extract_pages` and `split_document` **copy the original and delete the other pages** (import drops
`/AcroForm`). `merge_documents` creates an untitled in-memory document and is the only importer.
Owner (b). Feature F-16.

### 7.4 Page objects (text and image editing)

```ts
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

list_page_objects(a: { docId: DocId; page: PageIndex }): Promise<{ docGeneration: DocGeneration; objects: PageObject[] }>
probe_text_edit(a: { docId: DocId; page: PageIndex; objectId: ObjectId; text: string }): Promise<TextEditProbe>
edit_text_object(a: { docId: DocId; page: PageIndex; objectId: ObjectId; expectGeneration: DocGeneration;
  patch: { text?: string; fontSizePt?: number; color?: Rgb }; allowFontSubstitution: boolean }):
  Promise<{ objects: PageObject[]; docGeneration: DocGeneration }>
add_text_object(a: { docId: DocId; page: PageIndex; rect: Rect; text: string; fontSizePt: number;
  color: Rgb; align: 'left' | 'center' | 'right' }): Promise<{ objects: PageObject[]; docGeneration: DocGeneration }>
add_image_object(a: { docId: DocId; page: PageIndex; rect: Rect; path: string; keepAspect: boolean }):
  Promise<{ objects: PageObject[]; docGeneration: DocGeneration }>
// Stage 2: swap the bitmap of an existing image object, keeping its matrix (이미지 바꾸기).
replace_image(a: { docId: DocId; page: PageIndex; objectId: ObjectId; expectGeneration: DocGeneration;
  path: string }): Promise<{ objects: PageObject[]; docGeneration: DocGeneration }>
transform_object(a: { docId: DocId; page: PageIndex; objectId: ObjectId; expectGeneration: DocGeneration;
  translate?: Point; scale?: [number, number]; rotateDeg?: number }):
  Promise<{ objects: PageObject[]; docGeneration: DocGeneration }>
delete_objects(a: { docId: DocId; page: PageIndex; objectIds: ObjectId[]; expectGeneration: DocGeneration }):
  Promise<{ objects: PageObject[]; docGeneration: DocGeneration }>
```

Engine: `engine/objects/`. `probe_text_edit` runs the trial coverage check (apply `set_text`, reopen
`page.text()`, require `loose_bounds().width() > 0` for every non-generated char **including spaces**,
then revert). `edit_text_object` with `allowFontSubstitution: false` returns `fontCoverage` instead of
silently substituting. Every command here regenerates the page content once. Owner (b).
Features F-17, F-18, F-19.

### 7.4a Page stamps — watermark, header, footer (P1-4, Stage 4)

```ts
export type StampAnchor = 'tl' | 'tc' | 'tr' | 'ml' | 'mc' | 'mr' | 'bl' | 'bc' | 'br';
export type StampRole = 'watermark' | 'header' | 'footer';
export type StampSource =
  | { kind: 'text'; text: string; fontSizePt: number; color: Rgb }
  | { kind: 'image'; path: string; widthPt: number };          // height follows the image aspect
export interface StampSpec {
  role: StampRole; source: StampSource; anchor: StampAnchor;
  marginPt: number;        // >= 0, from the page edge for non-centre anchors
  rotateDeg: number;       // -180..180, counter-clockwise as seen, about the stamp centre
  opacity: number;         // 0..1
  pages: PageIndex[] | 'all';
}
export interface StampResult { info: DocInfo; pagesStamped: number }
add_stamp(a: { docId: DocId; spec: StampSpec }): Promise<StampResult>
```

Engine: `engine/stamp.rs` (Rust type names `PageStampSpec` / `PageStampSource`, because `StampSpec` is
already the stamp *annotation*; the wire shape is the one above). One `mutate` = one undo step, label
`undo.watermark` (role `watermark`) or `undo.headerFooter`; `changedPages` is `'all'` or the list.
* Text: `{{page}}` (1-based), `{{total}}`, `{{date}}` (local `YYYY-MM-DD`), `{{filename}}` (file name
  without extension, `Untitled` for an unsaved document) are replaced per page; unknown `{{x}}` stay
  literal; `\n` splits lines. One text object per line, Helvetica for Latin-1 and the bundled Hangul
  subset otherwise (embedded once per document). Box = widest line × (lines × size × 1.2); lines are
  left / centred / right-aligned by the anchor's column.
* Image: drawn once on a scratch page and copied in as **one Form XObject** that every stamped page
  references (a logo on 300 pages is stored once). Opacity is baked into the image's alpha (`/SMask`).
* Geometry is computed on the crop box *as displayed*: `/Rotate` is honoured, so `tl` is top-left and
  upright on screen for any page rotation. The box is placed by anchor + margin, then rotated about its
  centre (a rotated box at a corner anchor can therefore extend past the margin).
* Text opacity = fill + stroke alpha (`/ExtGState /ca /CA`).
* Every object carries the marked-content tag `SeePDF:Stamp` (`FPDFPageObj_AddMark`).
* Errors: blank text, font size ∉ (0, 1638], `widthPt` ∉ (0, 14400], `marginPt` < 0, `rotateDeg` ∉
  [-180, 180], `opacity` ∉ [0, 1], empty or out-of-range page list → `invalidArgument`; image file
  missing → `notFound`; undecodable image → `invalidArgument`; Hangul outside the bundled subset →
  `fontCoverage`. Encrypted documents are stamped like any edit (the password survives the save).

### 7.5 Redaction and security

```ts
export interface RedactPreview {
  page: PageIndex;
  textObjects: { objectId: ObjectId; text: string; rect: Rect; fullyInside: boolean }[];
  imageObjects: { objectId: ObjectId; rect: Rect; fullyInside: boolean }[];
  annotations: AnnotId[];
  formFields: string[];              // non-empty ⇒ the apply will refuse
  collateral: string[];              // text that will be removed although it is outside the marks
}
redact_preview(a: { docId: DocId; page: PageIndex; rects: Rect[] }): Promise<RedactPreview>
apply_redactions(a: { docId: DocId; page: PageIndex; rects: Rect[];
  options: { fill: Rgb; overlayText?: string } }):
  Promise<{ removedObjects: number; verified: boolean; docGeneration: DocGeneration }>

remove_password(a: { docId: DocId; outPath: string }): Promise<{ bytes: number }>                 // P1, implemented
set_password(a: { docId: DocId; outPath: string; userPassword?: string; ownerPassword: string;
  permissions: Partial<Permissions> }): Promise<{ bytes: number }>                                 // P1, implemented (Stage 3, lopdf AES-256 R6)
remove_metadata(a: { docId: DocId }): Promise<DocInfo>                                             // P1, implemented (Stage 3, lopdf)
set_metadata(a: { docId: DocId; meta: DocMeta }): Promise<DocInfo>                                 // P1, implemented (Stage 3, lopdf)
```

`apply_redactions` re-extracts the page text after `regenerate_content()` and fails with `verifyFailed`
(rolling back to the snapshot) if any marked string survives — a fake redaction is never shipped.
Owner (a) for redaction, (b) for the metadata and security file rewrites. Features F-22, P1-1, P1-2, P1-3.

Stage 3 semantics (`docs/STAGE3_SECURITY_NOTES.md`):
* `set_password.permissions` may be partial: a missing flag means **allowed**; `revision` is ignored.
  The open document is untouched; the copy is verified (opens with user and owner password, refuses
  no password when a user password is set) before anything is written. Empty `ownerPassword` or a
  password over 127 UTF-8 bytes → `invalidArgument`; a failed check → `verifyFailed`.
* `set_metadata` / `remove_metadata` are **one undo step** each and return the reloaded `DocInfo`.
  In `set_metadata` an omitted field keeps its value, a blank string removes the key, and `modified`
  omitted means "now". Both drop the catalog XMP `/Metadata` stream. An encrypted document →
  `unsupported` ("remove the password first"), no undo entry.

### 7.6 Save

```ts
export interface SaveResult { docId: DocId; path: string; bytes: number; docGeneration: DocGeneration; elapsedMs: number }
save_document(a: { docId: DocId }, onProgress: Channel<JobEvent>): Promise<SaveResult>
save_document_as(a: { docId: DocId; path: string }, onProgress: Channel<JobEvent>): Promise<SaveResult>
```

Engine: `engine/save/` — pre-flight AP render → `FPDF_SaveAsCopy(flags = 0)` → temp + fsync → verify by
reopening → backup → `rename` → reload (ARCHITECTURE §8). Errors: `readOnly` (offer Save As), `io`,
`verifyFailed`. Owner (b). Feature F-23.

### 7.6a Compress (P1-5, Stage 4)

```ts
export type CompressPreset = 300 | 150 | 96;   // target DPI for raster images
export interface CompressOptions { targetDpi: CompressPreset; pages?: PageIndex[] }   // default all
export interface CompressReport {
  token: number;               // pending-result handle, valid until apply/discard/doc change
  beforeBytes: number; afterBytes: number;
  imagesTotal: number; imagesDownsampled: number;
  elapsedMs: number;
}
compress_estimate(a: { docId: DocId; options: CompressOptions }, onProgress: Channel<JobEvent>): Promise<JobId>
compress_apply(a: { docId: DocId; token: number }): Promise<DocInfo>      // one undo step `undo.compress`
compress_discard(a: { docId: DocId; token: number }): Promise<void>
```

Engine: `engine/compress.rs`. `compress_estimate` validates the options (preset not 300/150/96 or a page
out of range → `invalidArgument`, promise rejected, no job), serialises the document into a **scratch**
`PdfDocument` and returns the job id. Events: `started{total = pages}`, one `progress{done, page}` per
page, then `done{elapsedMs, report}` — or `cancelled` / `error`. The open document is never touched.
* Candidates: top-level image objects whose effective DPI (PDFium image metadata: pixels ÷ on-page
  size × 72) exceeds `targetDpi × 1.1` on both axes. Skipped (counted in `imagesTotal` only):
  transparency (`/SMask`, `/Mask`, soft-mask state), 1-bit / stencil images, images inside Form
  XObjects (not counted), and images whose encoded stream appears more than once on the selected pages
  (a shared XObject — per-object replacement would store one copy per page).
* Re-encoding: `/DCTDecode` / `/JPXDecode` originals → JPEG q80 via `FPDFImageObj_LoadJpegFileInline` on
  the existing object, only when smaller; everything else → `FPDFImageObj_SetBitmap` (24-bit BGR or
  8-bit gray; PDFium writes Flate, which can come out larger — `afterBytes` shows it).
* The result is verified (`save::verify_bytes`) and kept as the document's single pending entry
  (a new estimate replaces it; close, undo/redo, save or any reload drop it).
* `compress_apply`: unknown/spent token → `notFound`; the document changed since the estimate →
  `stale`; otherwise the document is replaced from the pending bytes (`mutate_bytes`), one undo step,
  and the returned `DocInfo.undoLabel` is `undo.compress`. `compress_discard` of an unknown token is a
  no-op. Cancel with `cancel_job`.

### 7.7 Export and print

```ts
export interface ExportImagesArgs {
  docId: DocId; pages: PageIndex[]; format: 'png' | 'jpeg'; dpi: number; quality?: number;
  outDir: string; baseName: string; transparentBackground?: boolean;
}
export_images(a: ExportImagesArgs, onProgress: Channel<JobEvent>): Promise<JobId>
export_text(a: { docId: DocId; pages: PageIndex[]; outPath: string }): Promise<{ chars: number }>
export_flattened(a: { docId: DocId; outPath: string; annotations: boolean; forms: boolean; pages?: PageIndex[] },
                 onProgress: Channel<JobEvent>): Promise<JobId>
estimate_export(a: { docId: DocId; pages: PageIndex[]; format: 'png' | 'jpeg'; dpi: number }):
  Promise<{ bytes: number; sampledPages: number }>
print_prepare(a: { docId: DocId; pages?: PageIndex[] }): Promise<{ tempPath: string }>
```

Flattening uses raw `FPDFPage_Flatten(page, FLAT_NORMALDISPLAY)` + `FPDFPage_GenerateContent` + page
reload on a scratch copy — never `PdfPage::flatten()`, which is `FLAT_PRINT` and silently deletes
annotations without the Print flag. `print_prepare` is the fallback path for the OS print handler; the
primary print path is the frontend's print-only DOM plus `getCurrentWebview().print()`.
Owner (b). Features F-24, F-25, F-26.

### 7.8 History

```ts
undo(a: { docId: DocId }): Promise<DocInfo>
redo(a: { docId: DocId }): Promise<DocInfo>
```

Engine: pop a snapshot → `registry::replace` → generation bump (ARCHITECTURE §7). `DocInfo.undoLabel` /
`redoLabel` are i18n keys (`undo.annotCreate`, `undo.pageDelete`, …) rendered in the Edit menu.
Owner: S0. Feature F-15.

### 7.9 OCR

```ts
export type Box = [x0: number, y0: number, x1: number, y1: number];   // IMAGE pixels, origin top-left
export interface OcrWord { text: string; bbox: Box; confidence: number }
export interface OcrLine { text: string; bbox: Box; baseline?: [number, number, number, number];
                           rowHeightPx?: number; words: OcrWord[] }
export interface OcrPage { page: PageIndex; dpi: number; widthPx: number; heightPx: number;
                           rotation: Rotation; lines: OcrLine[] }

ocr_capabilities(): Promise<{ engines: ('tesseract' | 'vision' | 'windows')[]; languages: string[] }>
ocr_page_status(a: { docId: DocId; pages: PageIndex[] }): Promise<{ page: PageIndex; hasText: boolean; charCount: number }[]>
ocr_apply(a: { docId: DocId; pages: OcrPage[]; replaceExisting: boolean }, onProgress: Channel<JobEvent>): Promise<DocInfo>
ocr_recognize_native(a: { docId: DocId; page: PageIndex; dpi: number; languages: string[] }): Promise<OcrPage>  // P1, macOS
```

The page image for the tesseract.js workers comes from the protocol route `/ocr` (§9), never from a
command. `ocr_apply` is **one** `registry::mutate` for the whole batch = one undo step; per-page work runs
as separate `Lane::Background` commands so tiles interleave. Owner: (b) engine layer, (f) worker pipeline.
Feature F-21.

---

## 8. Events and progress

```ts
// src/ipc/events.ts — app-wide broadcasts (tauri emit/listen)
'open-file'        { path: string; source: OpenRequest['source'] }
'doc-changed'      { docId: DocId; docGeneration: DocGeneration; changedPages: PageIndex[] | 'all';
                     structure: boolean; dirty: boolean; reason: 'edit'|'undo'|'redo'|'save'|'pages'|'ocr'|'redact';
                     canUndo: boolean; canRedo: boolean }   // Stage 2: no get_document per edit
'doc-saved'        { docId: DocId; path: string; docGeneration: DocGeneration }
'recents-changed'  {}
'engine-pressure'  { level: 'normal' | 'high' }         // budgets halved; the frontend lowers MAX_MOUNTED_TILES
'menu:<id>'        {}                                   // native macOS menu item -> the focused window
```

```ts
// per-invocation progress (tauri::ipc::Channel<JobEvent>)
export type JobEvent =
  | { type: 'started'; jobId: JobId; total: number }
  | { type: 'progress'; jobId: JobId; done: number; total: number; page?: PageIndex; note?: string }
  | { type: 'done'; jobId: JobId; elapsedMs: number; outputs?: string[]; report?: CompressReport }   // report: compress_estimate only
  | { type: 'cancelled'; jobId: JobId; done: number }
  | { type: 'error'; jobId: JobId; error: EngineError };
```

Channels are used by `search_start` (own event type), `export_images`, `export_flattened`,
`split_document`, `ocr_apply`, `scan_annotations`, `save_document`, `compress_estimate`. Every job id can be cancelled with
`cancel_job`. Measured headroom: Channel ≥ 42k msg/s, `emit` ≥ 60k msg/s — both far above the ≤ 100 msg/s
this contract produces.

---

## 9. `seepdf://` protocol

Origin: `seepdf://localhost/` (macOS/Linux) or `http://seepdf.localhost/` (Windows). The frontend builds
every URL with `convertFileSrc('', 'seepdf')` — it never sniffs the OS.

| Route | Query | Response |
|---|---|---|
| `/tile` | `doc, gen, page, sk, rot, tx, ty[, night][, hl][, forms]` | `image/png` (512×512 or clipped edge) |
| `/page` | `doc, gen, page, sk, rot[, night][, hl][, forms]` | `image/png`, whole page — also the placeholder (`sk` small) |
| `/thumb` | `doc, gen, page, w[, rot]` | `image/png`, `set_target_width(w).set_maximum_height(w*2)` |
| `/ocr` | `doc, gen, page, dpi` | `image/png` gray8 at `dpi` (300 default) |
| `/recent-thumb` | `id` | `image/png` from `$APPDATA/SeePDF/thumbs/<id>.png` (read on the io thread) |
| `/raw` | `doc` | `application/pdf`, honours `Range:` → 206 |

* `sk` = `scaleKey` = `round(zoomPercent × devicePixelRatio)`; device scale `s = sk / 100`.
* `tx`,`ty` are **tile indices** (device origin = `tx·512, ty·512`).
* `forms` defaults to `1`. **`forms=0` makes the engine skip `FPDF_FFLDraw`**, so the bitmap carries
  no AcroForm widget at all — no value text, no field wash, no pushbutton caption (PDFium's own
  contract: `FPDF_ANNOT` renders "all annotations except widget and popup annotations"). The 양식
  overlay requests it while it is mounted, so a field value is rendered exactly once — by the HTML
  input on top (F-20). Everything else on the page is byte-identical to a `forms=1` render, which
  `cargo test form_forms_flag_suppresses_only_the_widgets` asserts pixel by pixel.
* Response headers: `Cache-Control: private, max-age=31536000, immutable`,
  `ETag "<doc>:<gen>:<page>:<kind>:<sk>:<rot>:<tx>:<ty>:<night>:<hl>:<forms>"`,
  `Access-Control-Allow-Origin: *`, `Access-Control-Expose-Headers: *`,
  `X-Render-Ms`, `X-Encode-Ms`, `X-Cache: hit|miss`, `X-Image-Width`, `X-Image-Height`.
* Status codes: `400` bad parameters, `404` unknown document, `410` `gen` older than the document's
  current generation (a lagging `<img>` must never paint stale pixels), `409` viewport moved on
  (render dropped as stale), `500` render error, `503` engine busy (`Retry-After: 0`).
  The handler **always** responds — a dropped `UriSchemeResponder` hangs the `<img>`.
* `304` is returned only when the request actually carries `If-None-Match` (a hand-crafted 304 for a
  `fetch()` makes WebKit throw `TypeError: Load failed`).

---

## 10. Binary payload formats

### 10.1 Text layer (`get_text_layer` → `ArrayBuffer`)

All little-endian; every section starts at a 4-byte aligned offset.

```
off  size          field
0    4             magic  "STXL" (0x4C_58_54_53 LE)
4    2             version = 1
6    2             flags   bit0 = objectIds present
8    4             pageIndex (u32)
12   4             charCount (u32)
16   4             wordCount (u32)
20   4             lineCount (u32)
24   24            matrix  f32[6]   page user space -> device px at s = 1 (crop box + /Rotate)
48   16            cropBox f32[4]   l, b, r, t
64   charCount*32  chars:  u32 codepoint | f32 l,b,r,t (loose box) | f32 baselineY
                           | u16 fontSizeX10 | u8 flags (1=generated, 2=hyphen, 4=space) | u8 pad | u32 objectId
…    wordCount*12  words:  u32 firstChar | u32 charCount | u32 lineIndex
…    lineCount*36  lines:  u32 firstWord | u32 wordCount | f32 baselineY | f32 l,b,r,t | u32 firstChar | u32 charCount
…    4 + n (+pad)  text:   u32 byteLength | UTF-8 page text (includes pdfium's generated "\r\n")
```

≈ 32 B per char ⇒ ~163 kB for a 5,087-char page; built in 5.6 ms/page and cached per generation.
`src/viewer/text/TextLayer.ts` wraps the buffer in `Uint32Array`/`Float32Array` views; **no DOM node per
character** is ever created.

### 10.2 Raw page buffer (`render_page_raw` → `ArrayBuffer`)

```
off size            field
0   4               magic "SPRX"
4   2               version = 1
6   2               format: 1 = RGBA8 (only value in v1)
8   4               width  px (u32)
12  4               height px (u32)
16  4               stride bytes (u32, == width*4)
20  12              reserved
32  stride*height   pixels, top-left origin, straight alpha
```

Produced by `set_reverse_byte_order(true)` + `as_raw_bytes()` (one copy, 0.16 ms for a 2× page); JS wraps
it in `ImageData` → `createImageBitmap`.

---

## 11. App, settings, recents

```ts
export interface RecentEntry {
  path: string; name: string; dir: string; pages: number; bytes: number;
  lastOpened: string; lastPage: PageIndex; zoomPercent: number;
  layout: 'single' | 'continuous' | 'two'; pinned: boolean; thumbId: string | null;
}
get_recent(): Promise<RecentEntry[]>
update_recent(a: { entry: RecentEntry }): Promise<void>
remove_recent(a: { path: string }): Promise<void>
set_recent_pinned(a: { path: string; pinned: boolean }): Promise<void>
clear_recent(): Promise<void>
write_recent_thumbnail(a: { docId: DocId }): Promise<{ thumbId: string }>
reveal_in_file_manager(a: { path: string }): Promise<void>

export interface Settings {
  locale: 'ko' | 'en'; theme: 'system' | 'light' | 'dark';
  defaultLayout: 'single' | 'continuous' | 'two'; defaultZoom: 'fit-width' | 'fit-page' | 'actual' | number;
  restorePosition: boolean; author: string; renderQuality: 'balanced' | 'high';
  tileCacheMb: number; recentsCount: number;   // Stage 2: first-class, was `toolDefaults.recentsCount`
  backupsEnabled: boolean; ocrLanguages: string[]; ocrDpi: 'auto' | 200 | 300 | 400;
  toolDefaults: Record<string, unknown>;
}
get_settings(): Promise<Settings>
set_settings(a: { patch: Partial<Settings> }): Promise<Settings>
```

Backed by `tauri-plugin-store` (`settings.json`, `recents.json`) from Rust so the values are available
before the webview mounts (window state, locale, theme). Owner: S0. Features F-01, F-27, F-28.

Native dialogs are called directly (no npm wrapper needed):
`invoke('plugin:dialog|open', { options: { title, multiple, filters: [{ name: 'PDF', extensions: ['pdf'] }] } })`
and `invoke('plugin:dialog|save', { options: { defaultPath, filters } })`. The dialog plugin adds picked
paths to the fs scope automatically.

---

## 12. Command → owner → feature index

| Command | Owner module (WORKPLAN) | Feature |
|---|---|---|
| `open_document`, `close_document`, `get_document`, `get_outline`, `take_pending_opens`, `open_in_new_window`, `window_bind_document` | S0 engine-core / app | F-01, F-05 |
| `get_recent`, `update_recent`, `remove_recent`, `set_recent_pinned`, `clear_recent`, `write_recent_thumbnail`, `reveal_in_file_manager`, `get_settings`, `set_settings` | S0 app | F-01, F-27, F-28 |
| `set_viewport`, `engine_stats`, `render_page_raw` | S0 render | F-02, F-26, F-30 |
| `get_text_layer`, `get_page_text`, `search_start`, `cancel_job` | S0 text | F-06, F-07 |
| `undo`, `redo` | S0 engine-core (history) | F-15 |
| `list_annotations`, `scan_annotations`, `create_annotation`, `update_annotation`, `delete_annotations`, `set_annotations_hidden` | (a) backend annotations | F-08…F-14 |
| `list_form_fields`, `set_form_field_value`, `reset_form` | (a) backend forms | F-20 |
| `redact_preview`, `apply_redactions` | (a) backend redaction | F-22 |
| `page_ops`, `extract_pages`, `split_document`, `merge_documents` | (b) backend pages | F-16 |
| `list_page_objects`, `probe_text_edit`, `edit_text_object`, `add_text_object`, `add_image_object`, `transform_object`, `delete_objects` | (b) backend objects | F-17, F-18, F-19 |
| `save_document`, `save_document_as` | (b) backend save | F-23 |
| `export_images`, `export_text`, `export_flattened`, `estimate_export`, `print_prepare` | (b) backend export | F-24, F-25, F-26 |
| `ocr_capabilities`, `ocr_page_status`, `ocr_apply` | (b) engine OCR layer + (f) worker pipeline | F-21 |
| `remove_password`, `set_password`, `remove_metadata`, `set_metadata`, `ocr_recognize_native` | (b), P1 | P1-1, P1-2, P1-3, P1-11 |
| `add_stamp` | Stage 4, `engine/stamp.rs` | P1-4 |
| `compress_estimate`, `compress_apply`, `compress_discard` | Stage 4, `engine/compress.rs` | P1-5 |

Frontend consumers: (c) viewer — documents, text, search, view, protocol routes; (d) tools —
annotations, forms, objects, history; (e) organizer/dialogs — pages, save, export, merge/split,
settings, recents; (f) OCR — `/ocr` route, `ocr_*`.

---

## 13. Stubs and the mock adapter

Stage 0 registers **every** command above in `generate_handler!` with a real signature and a body of
`Err(EngineError::unsupported("not implemented"))` where the owning module has not landed yet, so
`lib.rs` never changes again and the contract compiles from day one.

`src/ipc/mock.ts` implements the same surface against fixture JSON (`src/test/ipc-samples/`) and
1×1 PNG tiles, selected by `import.meta.env.VITE_SEEPDF_MOCK === '1'`, so every frontend module runs and
is testable in plain `vite dev` before the backend lands.
