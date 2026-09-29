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
  rotation: Rotation; crop: Rect; label: string | null;  // label from /PageLabels (written by set_page_labels, P2)
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
  pageLabels?: string[];   // P2: every page's label ("" where a page has none); absent when no page has one
}
/** Stage 2: where on the page the heading is (PDF user space). Absent for a plain page jump. */
export interface OutlineDest { x?: number; y?: number; zoom?: number }
export interface OutlineNode {
  title: string; page: PageIndex | null; dest?: OutlineDest;
  url?: string;            // P2: a web node (/A /URI); page is then null
  open?: boolean;          // P2: children start expanded (/Count > 0); read back only on nodes with children
  children: OutlineNode[];
}
export interface OpenRequest { path: string; source: 'argv' | 'macos-opened' | 'drop' | 'dialog' | 'recent' }

// Stage 8: `displayName` — what `DocInfo.name` reports instead of the file name (a recovered copy
// `<uuid>.pdf` opens under the original name; also the `{{filename}}` stamp token). Blank = file name.
// A document with more than 65,535 pages (PageIndex is u16 in the engine) is `unsupported`.
open_document(a: { path: string; password?: string; displayName?: string }): Promise<DocInfo>
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
| `get_outline` | depth-first `FPDFBookmark_GetFirstChild` / `GetNextSibling` walk (P2: raw, so it can read `/Count`'s sign → `open` and a URI action → `url`; cycle- and depth-guarded) | S0, P2 | F-05 |
| `take_pending_opens` | `PendingOpens` queue drain | S0 | F-01 |
| `open_in_new_window`, `window_bind_document` | `app/windows.rs` | S0 | F-01 |

`open_document` errors: `passwordRequired` (no password given), `passwordWrong` (one was), `notFound`,
`pdfium` (corrupt). `DocInfo.xfa` true ⇒ the form layer is read-only with a banner.

`window_bind_document` (Stage 8) also tells the native macOS 편집 menu which document the window shows: its
실행 취소 / 다시 실행 items name the focused window's top undo / redo step (`menu.edit.undoAction` with the
`undo.*` step name, e.g. `실행 취소: 워터마크` / `Undo Watermark`), updated on every `doc-changed`, on window
focus and on bind; the strings come from `src/i18n/{ko,en}.json` (compiled in). A window that is never bound
shows the last document that changed.

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
tiles never use it — v0.3 (V1): 스냅샷 renders the marquee's `rect` at 2× the on-screen device scale (both edges
≤ 8 192 px) and turns it by the view rotation in the webview. Owner: S0 (render). Features F-02, F-26.

`engine_stats` is **dev / diagnostics only** (v0.3, H5): the devtools console and bug reports read it; no UI
calls it, and its shape may change without notice.

**분할 보기 (P2).** The engine has one viewport per process, so the two panes of a split send **one** hint:
`firstPage` / `lastPage` span both panes' on-screen pages, `centrePage` is the focused pane's, and `scaleKey` /
`rotation` are those of the pane that settled. A render queued for either pane's pages is therefore never dropped
as "scrolled away" (`Viewport::keeps`). No command changed.

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
| `get_web_links` (v0.3) | `FPDFLink_LoadWebLinks` on a text page of our own (`engine/raw/weblinks.rs`) | pkg6 | V5 |
| `get_reading_order` (v0.3) | `FPDF_StructTree_*` MCID walk × `FPDFText_GetTextObject` → `FPDFPageObj_GetMarkedContentID` | pkg6 | V6 |

**v0.3 (pkg6-viewer-accessibility-settings).**

```ts
export interface WebLink { url: string; rects: Rect[]; charStart: number; charCount: number }
get_web_links(a: { docId: DocId; page: PageIndex }): Promise<WebLink[]>          // V5
export interface ReadingOrder { tagged: boolean; runs: [number, number][] }
get_reading_order(a: { docId: DocId; page: PageIndex }): Promise<ReadingOrder>   // V6
```

`get_web_links` returns the addresses PDFium's own detector finds in the page **text** — `http(s)://…`, `www.…`
(returned as `http://www.…`) and e-mail addresses (returned as `mailto:…`) — not Link annotations; `rects` are one
per line the address spans (PDF user space), `charStart` / `charCount` index the text layer (§10.1). Nothing is
written. The 읽기-mode link layer draws them as hit boxes and follows a click through the same 웹 주소 열기 confirm
(http(s) / mailto only) as a Link annotation's URI; an address a Link annotation already covers is not doubled.

`get_reading_order` gives the page's text-layer char ranges `[start, end)` in reading order. On a page with a
structure tree (a tagged PDF) the runs follow the MCID sequence of a depth-first `/K` walk (a kid element's subtree in
place, a marked-content kid as its MCID, so interleaved kids keep their order); each char's MCID is that of the text
object that drew it; a generated `\r\n` stays with the run before it; content the tree does not reference
(artifacts: running heads, page numbers) follows every tagged run in content order — nothing is dropped. `tagged:
false` = no structure tree on the page (or none referencing its content): one run `[0, charCount]`, content order.
Read aloud and the screen-reader text region use it; the frontend only asks when `DocInfo.tagged` is true. PDFium
reads tags but cannot write them, so this is read-only.

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
  uri?: string;                         // link: a web address (/A /URI)
  dest?: LinkDest;                      // link: a go-to-page target (/Dest or a GoTo /A) — P2
  inReplyTo?: AnnotId;                  // P2 threads: /IRT → the parent's /NM (/RT /R or absent; /RT /Group is not a reply)
  replies?: Annot[];                    // P2, FRONTEND ONLY — never sent: the thread under a top-level annotation (src/annot/threads.ts)
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
  | { kind: 'stamp'; rect: Rect; image: { path: string } | { builtin: string }; rotate?: number;
      signature?: boolean };
    // builtin (Stage 6b): '결재' | '승인' | '기밀' (인주 red, Hangul label from the bundled subset) |
    // 'approved' | 'final' | 'draft' | 'confidential' (label upper-cased); /Subj = the id = Annot.stampKind
    // signature (Stage 8): a typed / image signature — written with /Subj "SeePDF:Signature" and read
    // back as kind 'signature' (stampKind "SeePDF:Signature"), like the drawn one (Ink + the same /Subj)

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
reply_annotation(a: { docId: DocId; page: PageIndex; parentId: AnnotId; contents: string; author?: string | null }): Promise<AnnotResult>  // P2

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
| `set_annotations_hidden` | `FPDFAnnot_SetFlags` HIDDEN, transient: bumps `viewNonce`, **not** `docGeneration`, never dirties. Stage 6b: the viewer puts the nonce in that page's URLs (`vn`, §9) while a 선택-tool drag hides the annotation; the frontend unhides **before** the drag's `update_annotation`, so the HIDDEN bit is never in an undo snapshot | (a) | P1 |
| `reply_annotation` | `engine/annot/reply.rs` through `registry::mutate_bytes` (lopdf — PDFium cannot write an indirect reference into an annotation), below | P2 | P2 threads |

**Threads (P2).** A reply is a `Text` annotation with `/IRT` = an **indirect reference** to the parent's
dictionary and `/RT /R` (ISO 32000-1 §12.5.6.2) — what Acrobat, Preview and pdf.js group into a thread. The
engine reads `/IRT` with `FPDFAnnot_GetLinkedAnnot` and reports the parent's `/NM` as `inReplyTo` (list_page
assigns missing `/NM`s in a first pass, so a reply listed before its parent still resolves). `reply_annotation`:
refuses an encrypted document (`unsupported`, like `set_metadata`: lopdf would need the owner password), an
unknown parent or one on another page (`notFound`), a form field or link parent (`invalidArgument`) — all before
any work; then one `mutate_bytes` step (`undo.annotReply`): serialise → lopdf adds the reply (a parent that sits
directly inside `/Annots` becomes an object of its own first) → PDFium reopens it → replace, generation + 1,
`doc-changed { changedPages: [page] }`. The reply: `/Rect` a 20 pt square at the parent's top-left, `/F 28`
(Print | NoZoom | NoRotate), `/Name /Comment`, `/Open false`, `/NM` uuid, `/T` author (blank = none), `/Contents`,
`/CreationDate` = `/M`, `/P`, the parent's `/C` and `SeePDFC` colour mirror, and an **empty** normal appearance
(`/AP << /N <empty Form XObject> >>`) — a conforming reader shows replies inside the thread, never as icons, and
PDFium (which does not know threads) would otherwise draw a note icon over the parent. `update_annotation` on a
reply writes the empty appearance back after its `SetAP(NULL)` step. `result.annot` is the reply.
`delete_annotations` removes every reply below a deleted annotation (transitively, matched by `/NM` through
`/IRT`) with its popups — one undo step; `update_annotation`'s delete + re-create of a text box / markup keeps the
thread (the old dictionary with the same `/NM` stays in the file, so `/IRT` still resolves to that id).

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
`/AcroForm`). `merge_documents` creates an untitled in-memory document and is the only importer; it has
never been saved, so it opens **dirty** (`path: null`, `dirty: true`) — close / quit ask, autosave keeps it.
`duplicate` puts each copy **right after its source** (`[a, b, c]` duplicate `[0, 2]` → `[a, a′, b, c, c′]`;
repeated indices count once). An op that would take the document past 65,535 pages is `unsupported` and
rolled back.
Owner (b). Feature F-16.

### 7.3a Page boxes and page size — 자르기 / 페이지 크기 변경 (P2)

```ts
export interface Margins { top: number; right: number; bottom: number; left: number }  // pt, as SEEN
export type CropSpec = Rect | { margins: Margins };
export interface SetPageBoxesArgs {
  docId: DocId; pages: PageIndex[] | 'all';
  crop?: CropSpec | null;   // absent = unchanged, null = reset (crop box = media box)
  media?: Rect | null;      // absent = unchanged, null = invalidArgument
}
set_page_boxes(a: { args: SetPageBoxesArgs }): Promise<DocInfo>       // one struct argument, see below
export type ResizeTarget = 'A4' | 'Letter' | 'A3' | { w: number; h: number };   // { w, h } = as seen, pt
export type ResizeMode = 'scaleContent' | 'centerContent';
resize_pages(a: { docId: DocId; pages: PageIndex[] | 'all'; size: ResizeTarget; mode: ResizeMode }): Promise<DocInfo>
```

Engine: `engine/pages/boxes.rs`. Each call is one `registry::mutate` over the named pages = one undo step,
`undo.pageCrop` / `undo.pageResize`, `reason: 'pages'`, `changedPages` = the pages; the returned `DocInfo`
already carries the new geometry of every changed page (`mutate` refines ≤ 32 named pages, the command the
rest), so the frontend `adopt`s it. Any error rolls the whole batch back (no generation, no undo entry).

* `set_page_boxes` takes **one struct argument** `args` (like `export_images`): a top-level `crop: null`
  would reach Rust as "absent" (tauri deserialises a missing key and `null` alike for `Option`).
* `crop: Rect` — unrotated user space, the same rect for every page, clipped to each media box.
  `crop: { margins }` — inset from **each page's own** crop box as the page is seen (`/Rotate` applied,
  `top` = the displayed top edge), mapped with `stamp::visual_to_user`; this is what the 자르기 tool sends.
  `crop: null` (원래대로) sets the crop box to the media box — PDFium cannot delete a `/CropBox`, and an
  equal one is the same page. A new `media` clips an explicit crop box that sticks out of it. Written with
  `FPDFPage_SetCropBox` / `SetMediaBox` (page dictionary only; the content stream is untouched, so a crop
  never deletes anything). A page that inherits its media box from the page tree is measured through
  `FPDF_GetPageBoundingBox` with the crop box widened first.
* `resize_pages`: the page becomes `[0 0 w h]` (media = crop; trim / bleed / art boxes follow the content,
  clipped). A named size keeps each page's orientation (a landscape page gets landscape A4); `{ w, h }` is the
  size as seen. The content is mapped from the old crop box by one uniform matrix — scaled to fit and centred
  (`scaleContent`) or at 100 % and centred (`centerContent`, a smaller page clips) — with
  `FPDFPage_TransFormWithClip`, which wraps the content streams in `q <old crop> re W* n <m> cm … Q` and
  transforms the page's shading patterns; every object stays an ordinary, editable page object. (A Form
  XObject wrapper was rejected: it makes every object read-only and loses the annotations.) Annotations follow:
  `FPDFPage_TransformAnnots` maps every `/Rect` without touching appearance `BBox`es (appearances scale with
  their rect), then markup `/QuadPoints` (appearance cleared, regenerated on the next render), link quads,
  `/InkList` strokes and `/Popup` rects are mapped by hand. `/L`, `/Vertices`, `/CL` of third-party
  annotations keep their values (their appearance moves).
* Errors: `invalidArgument` for an empty / out-of-range page list, a non-finite box, a side < 1 pt or
  > 14 400 pt, margins < 0 or leaving < 1 pt, a crop outside the media box, `media: null`.
* Known limits: a shading pattern shared by several resized pages is transformed once per page; after a resize
  the page's content is three streams (PDFium's content generator handles that on later edits).

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
// Stage 8: copies moved by `offset` (PDF points), onto `targetPage` when given; `objects` and
// `newObjectIds` belong to `targetPage ?? page`. One undo step `undo.objectDuplicate`.
duplicate_objects(a: { docId: DocId; page: PageIndex; objectIds: ObjectId[]; expectGeneration: DocGeneration;
  offset: [dx: number, dy: number]; targetPage?: PageIndex }):
  Promise<{ objects: PageObject[]; docGeneration: DocGeneration; newObjectIds: ObjectId[] }>
```

`duplicate_objects` (Stage 8): the copies come from a second, throw-away parse of the source page (complete
objects sharing the document's fonts / images / Form XObjects by reference, glyphs written from their original
char codes) moved onto the target page — clip path translated too — and the target's content is regenerated
once. Text, image, path and Form XObject objects copy on the same page **and across pages**; a shading or
`other` object → `unsupported` (detail `unsupportedObjectType`: PDFium's content writer drops them), and a
copy PDFium could not write (an inline image) → `unsupported` (detail `notWritten`), rolled back. A copy lands
at the end of its original's content stream, so on a page with several streams it may sit below later
objects. `newObjectIds` are matched by type and expected bounds in the re-listed page, in request order
(sorted, deduplicated ids). `stale` / `notFound` as for the other object commands.

v0.3 (H5): `probe_text_edit` is **superseded** in the UI — 문단 편집 uses `probe_paragraph` / `edit_paragraph`, and a
크기 / 색상 change (`edit_text_object` with no `text`) takes the engine's `fontCoverage` answer as its probe: the
frontend then asks 글꼴이 대체됩니다 and retries with `allowFontSubstitution: true`. The command stays for scripts and
tests.

Engine: `engine/objects/`. `probe_text_edit` runs the trial coverage check (apply `set_text`, reopen
`page.text()`, require `loose_bounds().width() > 0` for every non-generated char **including spaces**,
then revert). `edit_text_object` with `allowFontSubstitution: false` returns `fontCoverage` instead of
silently substituting. Every command here regenerates the page content once. Owner (b).
Features F-17, F-18, F-19.

### 7.4b Paragraph probe + reflowing edit (Stage 7)

```ts
export interface ParagraphProbe {
  objectIds: ObjectId[];          // every text object of the paragraph, reading order
  rect: Rect;                     // union of their bounds
  text: string;                   // soft wraps joined with ' ' (line-end '-' + lowercase continuation de-hyphenated); no '\n'
  fontName: string; fontSizePt: number; color: Rgb;       // dominant style (most characters); size = rendered size
  mixedStyles: boolean;           // >1 font / size / colour inside → the edit merges them into the dominant one
  lineHeightPt: number;           // baseline-to-baseline; single line = fontSize × 1.2
  align: 'left' | 'center' | 'right' | 'justify';
  firstLineIndentPt: number;
  lines: number;
  strategy: 'inPlace' | 'replaceFont' | 'refused';        // for the CURRENT text, as TextEditProbe
  substituteFont?: string;
  reason?: PageObject['reason'] | 'glyphsMissing' | 'rotatedText'
    | 'unwritableContent';        // Stage 9: the edit would drop an inline image / shading PDFium cannot write back
  docGeneration: DocGeneration;   // additive: the generation objectIds belong to (use as expectGeneration)
}
probe_paragraph(a: { docId: DocId; page: PageIndex; at: Point }): Promise<ParagraphProbe | null>   // null = no text there

export type ParagraphFlow = 'push' | 'overlap' | 'fit';                 // Stage 9
export interface ParagraphEdit {
  objectIds: ObjectId[];          // from the probe
  text: string;                   // '\n' = hard line break; empty / whitespace-only = delete the paragraph
                                  // (round 2: with 'push' what follows moves up into its place; lines 0)
  width?: number;                 // box width in pt, measured from rect.l (default: rect width)
  fontSizePt?: number; color?: Rgb; align?: ParagraphProbe['align'];
  flow?: ParagraphFlow;           // Stage 9, default 'push'
  dryRun?: boolean;               // Stage 9, default false: compute the layout + flow plan, change nothing
}
export interface ParagraphEditResult {
  objects: { objects: PageObject[]; docGeneration: DocGeneration };   // dry run: objects EMPTY (page unchanged,
                                  // not re-listed), docGeneration = the current one
  rect: Rect;                     // box the new text occupies (union of the new objects' bounds);
                                  // deleted: a zero-height box at the paragraph's top
  lines: number;                  // blank lines (from '\n\n') included
  overflowPt: number;             // how far the NEW text still extends over content below after the flow;
                                  // 0 when push made the room it needed (the paragraph spacing counts),
                                  // and 0 when NOTHING is below (see pastBottomPt)
  shiftedPt: number;              // Stage 9: content below moved > 0 down, < 0 up, 0 none
  movedObjects: number;           // Stage 9: page objects moved by the flow
  movedAnnotations: number;       // Stage 9: annotations moved with them
  roomPt: number;                 // Stage 9: how far the content below could move down (page bottom margin /
                                  // obstacle); with nothing movable, how far the paragraph's line grid may grow
  blocked?: 'pageBottom' | 'obstacle';   // Stage 9, push only: set only when something still overlaps
                                  // (overflowPt > 0) — or, with nothing below, the text runs past the floor
  fitScale?: number;              // Stage 9, fit only: factor applied to font size + leading (0.7 … 1)
  pastBottomPt: number;           // Stage 9: nothing below the paragraph — how far the new text runs past the
                                  // page's bottom margin (the floor); 0 otherwise
  movedBand?: Rect;               // Stage 9, when content moved: the region it came from (column × original
                                  // bottom + ¼ size … stack bottom − ¼ size); the UI moves pending 영역 표시
                                  // marks inside it by −shiftedPt, as the engine moves annotations
}
edit_paragraph(a: { docId: DocId; page: PageIndex; expectGeneration: DocGeneration; edit: ParagraphEdit;
  allowFontSubstitution: boolean }): Promise<ParagraphEditResult>
```

Engine: `engine/objects/paragraph.rs`. Detection: upright visible text objects (matrix `b ≈ 0`, `a, d > 0`)
→ lines (baseline ± 0.25·size, split at gaps > 1·size; superscript/subscript bands fold into their line)
→ paragraph around the hit: same size ± 8 %, leading ≤ 2.2·size and ± 20 % of the first leading, overlapping
x-range; a short left-flush line whose successor's first word would have fitted ends it; a line indented
by > 0.5·size starts a new one (not when centred, or flush right with an inset > 3·size). Alignment from edge
variance (justify needs ≥ 3 lines). A click on rotated text → `refused/rotatedText`, on a Form XObject with
text → `refused/insideXObject`, on the invisible OCR layer → `refused/invisible`; a `noUnicode` run refuses
the paragraph. SeePDF's own stamps (`SeePDF:Stamp`: headers, footers, watermarks) never join a paragraph —
a header 4 pt above a first line used to be folded into it and deleted by the edit — and an edit naming a
stamp's object is `invalidArgument`. Coverage + advance widths come from one **trial object** in the candidate font (every distinct
glyph, read back through a fresh text page, placed off-page — PDFium's fake-bold filter drops a repeated
glyph near the same position) on a scratch page that is never regenerated, so the probe does not mutate.
A font without a usable space glyph (LaTeX subsets) stays `inPlace`: words become separate objects with a
0.3 em space. Edit: `stale` on generation mismatch; `fontCoverage` when the paragraph's font cannot draw the
new text and `allowFontSubstitution` is false (substitute: Helvetica for Latin-1, else SeePDF Hangul);
greedy breaking at spaces, by character only for a word wider than the box; first line honours the indent;
justify = one object per word (last line left), the stretched word gap capped at 0.75 × size (a line that
would need more stays short of the right edge, so it never reads back as two lines — the probe splits at
1 × size); a paragraph whose non-last lines are ≥ 75 % flush right still detects as justify, and a line
whose word gaps are all ≥ 0.55 × size counts as flush (a justified line that stopped short at the cap,
long words), so a justified paragraph stays justified edit after edit. New objects
are appended, the old ones removed (PDFium writes objects it did not parse into a new content stream at
the end of the page, so the edited paragraph is read — copied, searched — after the rest of the page; the
public API cannot place it back in its stream); one `registry::mutate` = one undo step `undo.paragraphEdit` (text **and** every flow
move). `fontSizePt` scales the line height proportionally. Known gaps: hanging indents / bulleted lists detect
line by line; kerning, horizontal scaling, shear (synthetic italic), stroke colour and per-run styles are
not preserved (mixedStyles merges to the dominant style); right-aligned paragraphs with small ragged-left
insets may split.

**Flow (Stage 9, `engine/objects/flow.rs`).** A longer paragraph used to be drawn over what follows it.
All geometry is in user space: detected text is upright there and lines are laid out at `baseline − row ×
leading`, so "below" is −y — on a `/Rotate`d page that is the paragraph's own reading direction.
* **Growth** compares line grids, not ink: bottom = last baseline − 0.25 × size (original: detected lines;
  new: `firstBaseline − (lines − 1) × leading`). A descender typed into the last line moves nothing; one
  more line moves the content below by exactly one leading.
* **Column** = x-range of the original rect ∪ the new rect, **widened to the text block**: a text line
  below that overlaps the column and starts (or ends) within 2.5 × size of its left (right) edge, or wraps
  it on both sides, widens it — the body text under a one-line, ragged or block-quote paragraph follows it
  instead of being an obstacle — unless that line also reaches into another column's content (content
  beside the column, from the paragraph's top down to the line, that does not hug the column's edge): a
  caption spanning two columns stays an obstacle. **Below** = top ≤ original ink bottom + 0.25 × size.
  **In-column** = overlap with the column ≥ 60 % of the object's width and ≤ 15 % of the column width
  sticking out on either side (a right-column paragraph never takes in left-column content).
  **Continuation lines**: a text line below that starts within 0.5 × size of the left edge of a movable
  line 0.8 … 2.2 × size above it is movable too (a paragraph's short last line is never an obstacle inside
  its own paragraph). **List markers**: a narrow object (≤ 3 × size) just left of a movable line's text (≤
  2 × size away), on its baseline band, that belongs to no other column moves with it.
* **Obstacles**: objects below that overlap the column but are not in it (full-width figures / tables),
  AcroForm widgets below that overlap it, SeePDF **header / footer stamps** below that overlap it (a
  stamped page number), the **running footer**, the bottom edge of a **container** (a non-text object
  overlapping ≥ half the column that starts above the paragraph's bottom and ends below it — a frame or
  shaded box, a table grid drawn as one path; its bounds' bottom + 3 pt, clear of the stroke; never an
  object taller than half the page — a page background or page frame), and the top
  of an in-column object that **straddles** an obstacle's top (a line wrapped beside a figure that sticks
  out of the column cannot move, so it is in the way itself).
  **Running footer** = among the body content overlapping the column (not the whole page: on two columns
  the other column's lines would hide the gap), the lowest band, separated from everything above by ≥ 2 ×
  the page's **line pitch** (1.5 × for a band narrower than a quarter of the column — a page number at
  LaTeX's `\footskip` or Word's footer distance; 0.75 × for such a band inside the bottom 72 pt) and
  starting in the bottom 20 % of the crop box — **and looking like a footer**: short (a page number),
  inside the bottom 72 pt, centred in the column, set smaller than the body above it (≤ 0.9 × its median
  size: footnotes, copyright blocks), or starting with a thin rule (a footnote separator). A letter's
  signature block (left-aligned, body size, above the bottom inch) is body content and follows the text.
  The pitch is the median baseline step of the text in the column (the paragraph's own leading when there
  are fewer than 3 steps), so it does not depend on which paragraph is edited. Never moved; not a footer
  when the edited paragraph is in it.
  **Movable** = below, in-column, not footer, bottom at or above the first (highest) obstacle's top: text,
  image, path, Form XObject and shading objects, translated in place with their clip paths. Never moved and
  never an obstacle: watermark stamps (and stamps without a role), invisible text (an OCR layer stays on
  its scan), anything above the paragraph.
* **Floor** (page bottom margin) = `max(crop.b, min(lowest body bottom, crop.b + 18))` — content may move
  into the bottom margin down to 18 pt above the crop box edge, never below content that already sits lower,
  never out of the crop box (body = not stamp / footer, not taller than half the page).
  **roomPt** = from the bottom of the movable stack (the paragraph's own line-grid bottom when nothing is
  movable) down to the higher of the first obstacle's top and the floor; `blocked` names which one limits.
  **gap** / **free** are measured from the paragraph's **line grid** (the same reference as the growth, not
  its ink) and keep a line's **clearance** (`leading − size`): gap = grid bottom − top of the highest
  movable object − clearance; free = grid bottom − top of the first content or obstacle below − clearance
  (or down to the floor, without clearance), both ≥ 0. **reach** = how far the new text's ink reaches below
  the original grid bottom = `max(growth, grid bottom − new ink bottom)` (a descender deeper than the
  nominal ¼ size reaches further).
* **push**: growth > 0 → `need = max(0, reach − gap)` (the least shift that keeps the new ink a clearance
  above what follows); the movable set moves down by `min(max(growth, need), roomPt)`; `remaining = need −
  shift`; only `remaining > 0` is `blocked` with `overflowPt = remaining` (it includes the clearance). With
  nothing movable but something in the way, `overflowPt = reach − free`. growth < 0 → it moves up by
  |growth|. **Nothing below** (no content, footer, header / footer stamp or widget below in the column — a
  frame's edge does not count): nothing moves; reach > free → `blocked` (`'pageBottom'`, or `'obstacle'`
  when a frame's bottom edge limits), `overflowPt = 0` and `pastBottomPt = reach − free` — the text runs
  past the margin, it overlaps nothing. **overlap**: nothing moves; `overflowPt = max(0, reach − free)`
  (with nothing below, that amount is `pastBottomPt` instead). **fit**: nothing moves; font size and
  leading × the largest factor in {1, 0.98, …, 0.70} whose grid fits the original bottom (`fitScale`); if
  0.70 does not fit, the remainder as for overlap. **Empty text** deletes the paragraph; with push, what
  follows moves up by `paragraph top − top of the first movable object` (it takes the paragraph's place).
* **Annotations** in the column below the paragraph whose vertical **centre** lies in the moved band (top
  ≤ original bottom + tol, bottom ≥ stack bottom − tol) move by the same distance — a box drawn around the
  last moved line reaches below the band and still moves: `/Rect` always (an existing appearance follows it through the
  `/Rect`↔`/BBox` matrix); text markup also moves its `/QuadPoints` and has its appearance cleared (PDFium
  regenerates it at the next render — the pre-save render included); links move their `/QuadPoints`; ink
  moves its `/InkList` and keeps its appearance. Widgets and popups never move. Keys PDFium cannot write
  (`/L`, `/Vertices`, `/CL`) keep their old values; the appearance moves.
* **dryRun**: the same layout, plan and result without mutating — no generation bump, no undo entry, the
  trial objects dropped with a never-regenerated scratch page; a substitute font (Helvetica / SeePDF Hangul)
  is measured in a throwaway document so nothing is embedded; the page is not re-listed (`objects.objects`
  is empty — on a dense page the listing costs more than the plan). `stale`, `fontCoverage`,
  `unwritableContent` and every validation error are reported exactly as for the real edit. The
  serialised document is byte-identical afterwards (except the random second half of the trailer `/ID`,
  which differs between any two saves).
* **Unwritable content / content-stream rehearsal** (`raw::page::rewrite_set`): PDFium's content
  generator skips inline images (`BI … ID … EI`) and shading objects (`sh`) in every content stream it
  regenerates, and the public API cannot tell an inline image from an XObject one. It also writes each
  object whole (state, clip, matrix) — but a page's content streams are one stream cut into pieces, and a
  producer may cut inside an object or a `q … Q` block (160F-2019.pdf: a `Tm` ends one piece and its `TJ`
  starts the next; a clip opened in one piece is closed in the next), so regenerating one piece used to
  draw the next piece's text at another object's matrix and leave a clip open over the rest of the page.
  So the probe, the dry run and the write rehearse the rewrite on a copy of the page in a throw-away
  document (`FPDF_ImportPagesByIndex`; the streams of the paragraph's objects and of the objects the flow
  moves regenerated, the copy re-parsed, every object's matrix, font size and clip compared): an object
  that comes back moved, clipped or missing is regenerated too (its stream writes it whole); when that does
  not settle, every stream of the page is regenerated (then no piece depends on another — at the price
  PDFium's generator always has for rewritten text: `TJ` kerning and `Tc` / `Tw` are not written back, so
  long kerned lines drift by about a glyph). Refused (`probe: refused/unwritableContent`; edit:
  `unsupported`, `detail "unwritableContent"`) only when the paragraph's own streams would lose an inline
  image or a shading; when only the streams of the content **below** would, that content stays where it is
  and is in the way (push: nothing moves, `blocked: 'obstacle'`, `overflowPt` as for overlap), so the
  prompt still offers fit / overlap. The write re-parses the page and rolls the mutation back if anything
  went missing. Content in a stream the edit does not touch (a header drawn in a stream of its own) is safe.
* Known gaps: third-party artifacts that are not `SeePDF:Stamp` (e.g. an Acrobat watermark wider than the
  column below the paragraph) are treated as content / obstacles; the new text is written into a stream of
  its own at the end of the page (reading order, above).

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
  // P2 Bates numbering, all optional: {{bates}} = batesPrefix + (batesStart + n, zero-padded to batesDigits)
  // + batesSuffix, n counting the STAMPED pages in ascending order (a range 3-5 from 1 gives 1, 2, 3)
  batesStart?: number;     // >= 0, default 1
  batesDigits?: number;    // 1..12, default 6; a wider number is written in full, never cut
  batesPrefix?: string;    // <= 64 characters, default ''
  batesSuffix?: string;    // <= 64 characters, default ''
}
export interface StampResult { info: DocInfo; pagesStamped: number }
add_stamp(a: { docId: DocId; spec: StampSpec }): Promise<StampResult>
// Stage 8 — 워터마크 제거. Every role when `role` is omitted; `removed == 0` is not an error (no undo step).
export interface RemoveStampsResult { info: DocInfo; removed: number }
remove_stamps(a: { docId: DocId; pages?: PageIndex[] | 'all'; role?: StampRole }): Promise<RemoveStampsResult>
```

Engine: `engine/stamp.rs` (Rust type names `PageStampSpec` / `PageStampSource`, because `StampSpec` is
already the stamp *annotation*; the wire shape is the one above). One `mutate` = one undo step, label
`undo.watermark` (role `watermark`) or `undo.headerFooter`; `changedPages` is `'all'` or the list.
* Text: `{{page}}` (1-based), `{{total}}`, `{{date}}` (local `YYYY-MM-DD`), `{{filename}}` (file name
  without extension, `Untitled` for an unsaved document) and (P2) `{{bates}}` are replaced per page in **one
  pass** (a file name or prefix that contains `{{page}}` is written as typed); unknown `{{x}}` stay literal;
  `\n` splits lines. The Bates fields are flattened into `PageStampSpec` (`BatesOptions`) and are not
  serialised when they hold their defaults. `remove_stamps` treats a Bates header / footer like any other
  (its role). One text object per line, Helvetica for Latin-1 and the bundled Hangul
  subset otherwise (embedded once per document). Box = widest line × (lines × size × 1.2); lines are
  left / centred / right-aligned by the anchor's column.
* Image: drawn once on a scratch page and copied in as **one Form XObject** that every stamped page
  references (a logo on 300 pages is stored once). Opacity is baked into the image's alpha (`/SMask`).
* Geometry is computed on the crop box *as displayed*: `/Rotate` is honoured, so `tl` is top-left and
  upright on screen for any page rotation. The box is rotated about its centre and its **rotated bounding
  box** is placed by anchor + margin (Stage 8), so a rotated stamp at a corner or edge anchor stays inside
  the margin.
* Text opacity = fill + stroke alpha (`/ExtGState /ca /CA`).
* Every object carries the marked-content tag `SeePDF:Stamp` (`FPDFPageObj_AddMark`); since Stage 8 with a
  string param `role` = `watermark` | `header` | `footer` (`/SeePDF:Stamp <</role (watermark)>> BDC`).
* `remove_stamps` (Stage 8) deletes the top-level objects carrying that mark on `pages` (default all), of
  `role` only when given — stamps written before Stage 8 have no `role` and go only when `role` is omitted.
  One `mutate` over the affected pages = one undo step `undo.removeStamps`; nothing found → `{ removed: 0 }`
  with the unchanged `DocInfo` (same generation, no undo entry). Out-of-range page → `invalidArgument`.
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
// Stage 8: every marked page in ONE undo step (`undo.redact`); a verifyFailed on any page rolls back all.
export interface RedactBatchMark { page: PageIndex; rects: Rect[] }
export interface RedactBatchResult { removedObjects: number; verified: boolean; docGeneration: DocGeneration; pages: PageIndex[] }
apply_redactions_batch(a: { docId: DocId; marks: RedactBatchMark[]; options: { fill: Rgb; overlayText?: string } }):
  Promise<RedactBatchResult>

remove_password(a: { docId: DocId; outPath: string }): Promise<{ bytes: number }>                 // P1, implemented
set_password(a: { docId: DocId; outPath: string; userPassword?: string; ownerPassword: string;
  permissions: Partial<Permissions> }): Promise<{ bytes: number }>                                 // P1, implemented (Stage 3, lopdf AES-256 R6)
remove_metadata(a: { docId: DocId }): Promise<DocInfo>                                             // P1, implemented (Stage 3, lopdf)
set_metadata(a: { docId: DocId; meta: DocMeta }): Promise<DocInfo>                                 // P1, implemented (Stage 3, lopdf)
```

`apply_redactions` is **legacy** (v0.3, H5): the UI applies every mark through `apply_redactions_batch` (one undo
step for all pages); the single-page command stays for scripts and tests. `apply_redactions` re-extracts the page
text after `regenerate_content()` and fails with `verifyFailed`
(rolling back to the snapshot) if any marked string survives — a fake redaction is never shipped.
`apply_redactions_batch` (Stage 8) merges the marks per page (entries with no rects are ignored), checks the
form-field refusal (`unsupported`) on **every** page before touching any, then runs the per-page apply for each
page in ascending order inside one `mutate`: one generation, one undo step `undo.redact`, `pages` = the pages
redacted. Any failure — `verifyFailed` on a later page included — restores the whole document. No rects at all or
a page out of range → `invalidArgument`.
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
path_exists(a: { path: string }): Promise<boolean>   // Stage 6a (P1-7)
```

`path_exists` is `true` when anything (file or directory) exists at `path`, and also when that cannot be
determined (a permission error counts as taken). 여러 파일 OCR asks it before each `save_document_as`, so a
`<name>-ocr.pdf` that exists becomes `<name>-ocr (2).pdf` instead of being overwritten. No engine call.

Engine: `engine/save/` — pre-flight AP render → `FPDF_SaveAsCopy(flags = 0)` → temp + fsync → verify by
reopening → backup → `rename` → reload (ARCHITECTURE §8). Errors: `readOnly` (offer Save As), `io`,
`verifyFailed`. A new file's directory is probed by creating a file there, never judged by its
read-only *attribute* (on Windows that is the shell-folder marker on Documents / Desktop / Pictures).
A save keeps `docGeneration` (nothing a cache is keyed on changed) and emits only `doc-saved` — no
`doc-changed`. Its job id is for progress only: a save cannot stop half-way, so `cancel_job` answers
`false` for it. Owner (b). Feature F-23.

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
  transparency (`/SMask`, `/Mask`, soft-mask state), 1-bit / stencil images, and images whose encoded
  stream appears more than once on the selected pages (a shared XObject — per-object replacement would
  store one copy per page; an occurrence inside a Form XObject counts too).
* Stage 8 — images inside Form XObjects (nested forms too, 8 levels) are counted in `imagesTotal` and are
  candidates when every occurrence on the selected pages is above the threshold, measured through the
  combined matrix (their size on the page; the lowest DPI sets the scale). PDFium cannot repoint a form's
  content, so the re-encoded data (JPEG q80 for DCT/JPX, else 8-bit samples Flated) is written into the
  **same image stream object** of the serialised copy with `lopdf`, matched by the encoded stream's hash and
  length — every form (on every page) that draws it gets the smaller image, and the occurrences count in
  `imagesDownsampled`. Only when smaller; never on an encrypted document (its streams are encrypted), so
  those nested images are left alone there. An image outside `pages` that shares the stream is replaced too.
* Encrypted documents compress like any other (top-level images); the result keeps the password.
* Re-encoding: `/DCTDecode` / `/JPXDecode` originals → JPEG q80 via `FPDFImageObj_LoadJpegFileInline` on
  the existing object, only when smaller; everything else → `FPDFImageObj_SetBitmap` (24-bit BGR or
  8-bit gray; PDFium writes Flate, which can come out larger — `afterBytes` shows it).
* The result is verified (`save::verify_bytes`) and kept as the document's single pending entry
  (a new estimate replaces it; close, undo/redo, save or any reload drop it).
* `compress_apply`: unknown/spent token → `notFound`; a result that saves nothing (`imagesDownsampled
  == 0` or `afterBytes >= beforeBytes` — the dialog disables 적용 for exactly these) is spent and the
  current `DocInfo` comes back unchanged (no undo step, not dirty, same generation); the document
  changed since the estimate → `stale`; otherwise the document is replaced from the pending bytes
  (`mutate_bytes`), one undo step, and the returned `DocInfo.undoLabel` is `undo.compress`. `compress_discard` of an unknown token is a
  no-op. Cancel with `cancel_job`.

### 7.6b Compare two documents (P1-6, Stage 5)

```ts
export interface CompareOptions {
  pagesA?: PageIndex[];        // default all pages of A
  pagesB?: PageIndex[];        // default all pages of B; the candidates, in order
  ignoreCase?: boolean;        // default false; whitespace runs are always normalised
  alignPages?: boolean;        // Stage 8, default true: pair pages by text similarity (below); false = by
                               // position, the longer list's extra pages becoming rows with a null side
}
export type DiffKind = 'equal' | 'insert' | 'delete' | 'replace';
export interface DiffOp {
  kind: DiffKind;
  words: number;               // words on the A side for equal/delete/replace, B side for insert
  textA?: string;              // omitted for equal and insert (words joined by one space)
  textB?: string;              // omitted for equal and delete
  rectsA?: Rect[];             // PDF points on page A (y-up), one rect per line fragment; omitted for equal/insert
  rectsB?: Rect[];             // same on page B; omitted for equal/delete
}
export interface ComparePage {
  pageA: PageIndex | null; pageB: PageIndex | null;
  changed: boolean;            // any op that is not equal; since Stage 8 a row with a null side is always changed
  wordsA: number; wordsB: number;
  ops: DiffOp[];
}
export interface CompareReport {
  docA: DocId; docB: DocId;
  pages: ComparePage[];
  changedPages: number; inserted: number; deleted: number;   // word counts (replace counts on both sides)
  elapsedMs: number;
}
compare_documents(a: { docA: DocId; docB: DocId; options: CompareOptions }, onProgress: Channel<JobEvent>): Promise<JobId>
```

Engine: `engine/compare.rs`. Both documents must already be open (the frontend opens B with
`open_document`). Validation runs before the job id is returned: unknown doc → `notFound`, docA == docB or
a page out of range → `invalidArgument` (promise rejected, no job). Events with `alignPages: false`:
`started{total = pairs}`, one `progress{done, page = index into pages[]}` per pair, then
`done{elapsedMs, compare}` — or `cancelled` / `error`. One `Lane::Background` command per pair; Cancel
(`cancel_job`) is observed between pairs. **A document closed mid-job ends it with `cancelled`**, not `error`
(Stage 8).
* `alignPages` (Stage 8, default): `started{total = |A| + |B| + max(|A|, |B|)}`, one page-less `progress` per
  scanned candidate page, then an align step that sets the exact total (`|A| + |B| + rows`, carried by the
  following `progress` events), then one `progress{page = index into pages[]}` per row. Pages pair by
  sequence alignment over per-page word **sets** (Jaccard similarity; identical head and tail pages pair
  directly; the middle is a Needleman–Wunsch with free gaps and a tiny per-pair bonus, so a rewritten page
  still pairs with the page in its place). An inserted / deleted page becomes one null-sided row instead of
  shifting every later pair. A middle larger than 250 000 page pairs falls back to positional pairing.
* Words: text-layer chars split on Unicode whitespace and PDFium-generated chars; with `ignoreCase` each
  char is case-folded (`text::layer::fold`) before comparison; reported text is the original.
* Diff: Myers O(ND) on interned words after trimming the common prefix/suffix; adjacent delete+insert
  collapse into `replace`. Beyond 2,000 edits on one page pair the untrimmed middle is reported as one
  `replace` (bounded memory).
* Rects: `TextLayer::range_rects` over the run's code-point span — one rect per line fragment.
* A page without text (scanned) has 0 words, no error. Neither document is modified.

### 7.6c Autosave / crash recovery (P1-8, Stage 5)

```ts
export interface RecoveryEntry {
  id: string;                   // uuid v4, also the file stem in the recovery dir
  originalPath: string | null;  // null for a never-saved document
  name: string;                 // file name, or "제목 없음" for a never-saved document
  savedAt: string;              // ISO-8601 / RFC 3339, UTC with milliseconds ("2026-09-28T01:02:03.456Z")
  bytes: number; pages: number;
  recoveryPath: string;         // absolute path of the .pdf copy
}
write_recovery(a: { docId: DocId }): Promise<RecoveryEntry>
clear_recovery(a: { docId: DocId }): Promise<void>
list_recovery(): Promise<RecoveryEntry[]>
discard_recovery(a: { id: string }): Promise<void>
```

Engine: `engine/recovery.rs`, commands `commands/recovery.rs`. The directory is `app_data_dir()/recovery/`
(macOS `~/Library/Application Support/com.seepdf.desktop/recovery/`), created lazily by the first write.
* `write_recovery`: `save::serialize` on the engine thread (appearance streams included, encryption kept,
  generation / dirty unchanged), then on the command task `<id>.pdf` and then `<id>.json` (the sidecar is
  the `RecoveryEntry`), each atomically (temp + fsync + rename). One id per open document (kept on the
  `OpenDoc`, assigned on first write); repeated calls overwrite the pair. The user's file is never touched.
  Allowed on a clean document. Unknown doc → `notFound`.
* `clear_recovery`: removes this document's pair; no-op if it never wrote one. Also works after
  `close_document` (the command remembers docId → id). `close_document` itself never deletes recovery files.
* `list_recovery`: sidecars newest first (by `savedAt`); a sidecar whose pdf is missing, or that does not
  parse, is deleted and skipped. Missing directory → `[]`. Copies that belong to a document open right
  now (any window) are live autosaves and are left out.
* `discard_recovery`: deletes one pair; unknown id → no-op; an id that is not a uuid → `invalidArgument`
  (it would otherwise name a path).

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
annotations without the Print flag. `export_flattened`'s two switches are independent: `forms` bakes the
AcroForm widgets, `annotations` every other annotation; what is not baked stays live (fields fillable,
comments editable — parked outside `/Annots` for the flatten and restored in an incremental update, so
an encrypted file stays encrypted). `print_prepare` is the fallback path for the OS print handler; the
primary print path is the frontend's print-only DOM plus `getCurrentWebview().print()`.
Owner (b). Features F-24, F-25, F-26.

### 7.7a Annotation summary export — 주석 목록 내보내기 (P2)

```ts
export type SummaryFormat = 'txt' | 'csv' | 'md';
export interface AnnotationSummaryResult { count: number; bytes: number }
export_annotation_summary(a: { docId: DocId; path: string; format: SummaryFormat; pages?: PageIndex[];
                               locale?: 'ko' | 'en' }): Promise<AnnotationSummaryResult>
```

Engine: `engine/export/summary.rs`, `Lane::Background`. One row per annotation of `pages` (default all), pages
ascending, within a page in `/Annots` order (what the 주석 sidebar lists); links and widgets are left out,
popups never appear on their own. Columns: page number, page label (`/PageLabels`), kind (the sidebar's name,
from `src/i18n/*.json` in `locale` — default the app language), author, created, modified (PDF dates shown in
local time `YYYY-MM-DD HH:MM:SS`; unparseable ones as found), colour `#RRGGBB`, contents (a text box's text
when `/Contents` is empty), and for text markup the **quoted text**: the characters of the page's cached text
layer whose box centre lies in one of the quads, reading order, whitespace folded. CSV is RFC 4180 (CRLF,
`"` doubled, quoted when needed) in UTF-8 **with a BOM** so Excel reads Hangul; a free-text cell starting with
`=`, `+`, `-` or `@` gets a leading `'` (formula injection). TXT is blocks per annotation, Markdown a heading
per page and a bullet per annotation (markup characters escaped); both UTF-8 without a BOM. The file is written
atomically; `count == 0` still writes the (empty) list. Errors: `invalidArgument` (empty path, page out of
range), `io`.

### 7.8 History

```ts
undo(a: { docId: DocId }): Promise<DocInfo>
redo(a: { docId: DocId }): Promise<DocInfo>
```

Engine: load the top snapshot → `registry::replace` → only then move the stacks → generation bump
(ARCHITECTURE §7). Transactional: a snapshot that cannot be read or reloaded fails the call and leaves
both stacks, the generation and the document as they were. The snapshot RAM budget (256 MiB) is shared
by all open documents. `DocInfo.undoLabel` /
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
export type OcrApplyPage = OcrPage | { page: PageIndex; ocr: OcrPage };   // Stage 8: both forms, mixable
ocr_apply(a: { docId: DocId; pages: OcrApplyPage[]; replaceExisting: boolean }, onProgress: Channel<JobEvent>): Promise<DocInfo>
ocr_recognize_native(a: { docId: DocId; page: PageIndex; dpi: number; languages: string[] }): Promise<OcrPage>  // P1-11, macOS 13+
```

The page image for the tesseract.js workers comes from the protocol route `/ocr` (§9), never from a
command. `ocr_apply` is **one** `registry::mutate` for the whole batch = one undo step; per-page work runs
as separate `Lane::Background` commands so tiles interleave. Owner: (b) engine layer, (f) worker pipeline.
Feature F-21.

`ocr_recognize_native` (P1-11) renders the same `/ocr` image on the engine thread and runs Apple Vision
(`VNRecognizeTextRequest`, accurate, language correction on) on a blocking thread; it returns the page already
normalised (the Rust twin of `normalizeVision`: image pixels, origin top-left, confidence 0..100, synthetic
baseline/row height). `languages` are Vision's (`ko-KR`, `en-US`); tesseract codes (`kor`, `eng`) are mapped,
and Korean always brings English. `ocr_capabilities` lists `vision` only on macOS 13+ where Vision reports
`ko-KR`; elsewhere the command answers `unsupported`. `ocr_apply` with a wrapped page whose `page` differs
from `ocr.page` is `invalidArgument`. 여러 파일 OCR applies each file's pages in one call (one snapshot).

### 7.10 Document structure — outline, links, page labels (P2)

```ts
// OutlineNode (§4) gains `url?` and `open?`; Annot (§7.1) gains `dest?`; DocInfo (§4) gains `pageLabels?`.
export interface LinkDest { page: PageIndex; x?: number; y?: number; zoom?: number }   // /XYZ left, top, zoom factor
export type LinkTarget = LinkDest | { url: string };
export type PageLabelStyle = 'decimal' | 'roman' | 'romanUpper' | 'alpha' | 'alphaUpper' | 'none';
export interface PageLabelRange { start: PageIndex; style: PageLabelStyle; prefix?: string; first?: number }

set_outline(a: { docId: DocId; nodes: OutlineNode[] }): Promise<DocInfo>                 // [] removes /Outlines
create_link(a: { docId: DocId; page: PageIndex; rect: Rect; target: LinkTarget }): Promise<AnnotResult>
update_link(a: { docId: DocId; page: PageIndex; id: AnnotId; rect?: Rect; target?: LinkTarget }): Promise<AnnotResult>
delete_link(a: { docId: DocId; page: PageIndex; id: AnnotId }): Promise<AnnotResult>
set_page_labels(a: { docId: DocId; ranges: PageLabelRange[] }): Promise<DocInfo>        // [] removes /PageLabels
get_page_labels(a: { docId: DocId }): Promise<PageLabelRange[]>                         // the ranges as written
```

PDFium reads all three structures but writes none of them (no `FPDFBookmark_*` setter, no `/PageLabels`
writer, no way to give a Link a `/Dest`), so they go through the Stage 3 byte-level path
(`STAGE3_SECURITY_NOTES.md` §2) on the engine thread: undo snapshot → `save::serialize` → rewrite the bytes
with **lopdf** → reopen with PDFium and **check the result with PDFium's own readers** → `replace` (generation
+ 1, `doc-changed`) — one undo step. A check that fails is `verifyFailed` with the snapshot restored and no
undo entry. Module `engine/structure/` (`outline.rs`, `links.rs`, `labels.rs`), commands in
`commands/structure.rs`.

| Command | Engine op | Undo label | Feature |
|---|---|---|---|
| `set_outline` | deletes every old outline object, writes `/Outlines` + items (`/Title` UTF-16BE for Hangul, `/Parent` `/Prev` `/Next` `/First` `/Last`, `/Count` = +visible descendants when open / −descendants when closed, root `/Count` = visible items) with `/Dest [page /XYZ x y zoom]` (`/Fit` when x, y and zoom are all absent) or `/A << /S /URI >>`; check: the `FPDFBookmark_*` walk reads back the same titles, pages, urls, open flags and shape | `undo.outlineEdit` | P2 outline |
| `create_link` | web target: **PDFium** — `FPDFPage_CreateAnnot(LINK)` + `SetRect` + `FPDFAnnot_SetURI` + `/Border [0 0 0]` + `/F 4` + `/NM`, inside `registry::mutate`; page target: **lopdf** — a `/Link` dictionary with `/Dest`, `/Border [0 0 0]`, `/F 4`, `/NM`, `/P` appended to the page's `/Annots`; check: `FPDFLink_GetDest` resolves to the requested page | `undo.linkCreate` | P2 links |
| `update_link` | rect only, or web → web on a link without `/Dest`: PDFium (`SetRect` / `SetURI`); anything that adds or removes a `/Dest`: lopdf (sets `/Dest` and drops `/A`, or sets `/A /URI` and drops `/Dest`) | `undo.linkEdit` | P2 links |
| `delete_link` | `FPDFPage_RemoveAnnot` (PDFium) | `undo.linkDelete` | P2 links |
| `set_page_labels` | writes `/PageLabels << /Nums [start << /S /P /St >> …] >>` (sorted; a leading `0 << /S /D >>` is added when the first range starts later — the tree must cover page 0; `/St` omitted when 1; no `/S` for `none`); check: `FPDF_GetPageLabel` of every page equals the labels the engine computes (letters repeat: 27 = `aa`) | `undo.pageLabels` | P2 labels |
| `get_page_labels` | lopdf read of the catalog's `/PageLabels` number tree (`/Nums` and `/Kids`) from the document's bytes | — | P2 labels |

Semantics:
* `set_outline` replaces the **whole** tree. `page` out of range or a tree deeper than 32 levels →
  `invalidArgument`. A node with neither `page` nor `url` is written with no destination; `open` missing on a
  node with children means open. `get_outline` reads back exactly what was written: `dest` only when x, y or
  zoom is set, `open` only on nodes with children, `url` nodes with `page: null`. What a rewrite loses: named
  destinations become explicit ones; actions other than GoTo and URI (Launch, JavaScript, GoToR) become
  title-only nodes; item colour and style (`/C`, `/F`) are dropped. The old items are deleted from the file, not
  just unlinked.
* Links: `rect` is normalised; smaller than 1 pt, an empty `url`, a target page out of range, an `update_link`
  with neither `rect` nor `target` → `invalidArgument`; the link's own `page` out of range or an unknown `id` →
  `notFound`; an id that is not a Link → `invalidArgument`. A URI is 7-bit: non-ASCII bytes and spaces are
  percent-encoded (`https://예.kr/a b` reads back `https://%EC%98%88.kr/a%20b`; the frontend already sends it
  encoded, so it reads back as sent). A page target with no x, y or zoom is written `/Fit` and reads back with
  no position; otherwise `/XYZ` with `null` for a missing value (zoom ≤ 0 dropped). New links carry
  `/Border [0 0 0]` and the Print flag. `AnnotResult.list` is the
  page's annotations after the change, `annot` the link (`null` after a delete), `previous` the link before an
  update / delete. `doc-changed` names only that page. Links are listed by `list_annotations` /
  `scan_annotations` as `kind: 'link'` with `uri` or `dest`.
* `set_page_labels`: a start outside the document, two ranges on one page, or `first` outside 1–1 000 000 →
  `invalidArgument`. `DocInfo.pageLabels` (and `pages[i].label`) are re-read from PDFium after the reload.
  `get_page_labels` normalises what it reads (`first: 1` and an empty `prefix` come back absent) and reads the
  bytes the document was last loaded from — PDFium never writes `/PageLabels`, so no later edit can have
  changed them.
* `set_outline`: a node with both `url` and `page` is written as a web node (the url wins).
* **Encrypted documents** (opened with a password, or `encrypted`): `set_outline`, `set_page_labels`,
  `get_page_labels` and every link change that needs a `/Dest` (a page `create_link`, an `update_link` that adds
  or removes one) answer `unsupported` ("remove the password first") and push no undo entry — re-encrypting
  would need the owner password, exactly as for `set_metadata`. Web links, rect moves and `delete_link` go
  through PDFium and work on encrypted documents.

---

## 8. Events and progress

```ts
// src/ipc/events.ts — app-wide broadcasts (tauri emit/listen)
'open-file'        { path: string; source: OpenRequest['source'] }
'doc-changed'      { docId: DocId; docGeneration: DocGeneration; changedPages: PageIndex[] | 'all';
                     structure: boolean; dirty: boolean; reason: 'edit'|'undo'|'redo'|'pages'|'ocr'|'redact';
                     canUndo: boolean; canRedo: boolean }   // Stage 2: no get_document per edit
'doc-saved'        { docId: DocId; path: string; docGeneration: DocGeneration }   // a save: generation unchanged, no doc-changed
'recents-changed'  {}
'engine-pressure'  { level: 'normal' | 'high' }         // budgets halved; the frontend lowers MAX_MOUNTED_TILES
'tts-progress'     { sentenceIndex: number | null }     // v0.3: read aloud started sentence i; null = the queue ended
'menu:<id>'        {}                                   // native macOS menu item -> the focused window
```

```ts
// per-invocation progress (tauri::ipc::Channel<JobEvent>)
export type JobEvent =
  | { type: 'started'; jobId: JobId; total: number }
  | { type: 'progress'; jobId: JobId; done: number; total: number; page?: PageIndex; note?: string }
  | { type: 'done'; jobId: JobId; elapsedMs: number; outputs?: string[]; report?: CompressReport;
      compare?: CompareReport }   // report: compress_estimate only; compare: compare_documents only
  | { type: 'cancelled'; jobId: JobId; done: number }
  | { type: 'error'; jobId: JobId; error: EngineError };
```

**`engine-pressure`** (emitted since v0.3, H5) comes from the tile cache (`engine/render/cache.rs`), never from
the engine thread: `high` when either load exceeds 85 % of its budget, `normal` again only below 70 % (hysteresis).
The two loads: the tile cache's **churn** — bytes evicted in the last 10 s against the cache budget (an LRU sits at
~100 % of its budget in steady state, so fullness is no signal; a working set that no longer fits is; evictions
caused by lowering 설정 › 캐시 크기 are not churn, and a budget change restarts the window) — and the
process RSS against 1 GiB (the 400 MB idle budget of ARCHITECTURE §13 plus the 256 MiB undo RAM budget), sampled
every 3 s by a monitor thread, which is also what brings the level back while nothing renders. The viewer halves
its in-flight and mounted-tile budgets while `high` (`TileManager.setPressure`).

**`tts-progress`** (v0.3, V4): see §11a.

Channels are used by `search_start` (own event type), `export_images`, `export_flattened`,
`split_document`, `ocr_apply`, `scan_annotations`, `save_document`, `compress_estimate`, `compare_documents`. Every job id can be cancelled with
`cancel_job`, except `save_document` / `save_document_as`'s, which is for progress only (`cancel_job`
answers `false`). `ocr_apply` stops between pages; the batch is one undo step, so a cancelled batch is
rolled back whole and ends with `cancelled`. Measured headroom: Channel ≥ 42k msg/s, `emit` ≥ 60k msg/s — both far above the ≤ 100 msg/s
this contract produces.

---

## 9. `seepdf://` protocol

Origin: `seepdf://localhost/` (macOS/Linux) or `http://seepdf.localhost/` (Windows). The frontend builds
every URL with `convertFileSrc('', 'seepdf')` — it never sniffs the OS.

| Route | Query | Response |
|---|---|---|
| `/tile` | `doc, gen, page, sk, rot, tx, ty[, night][, hl][, forms][, vn]` | `image/png` (512×512 or clipped edge) |
| `/page` | `doc, gen, page, sk, rot[, night][, hl][, forms][, vn]` | `image/png`, whole page — also the placeholder (`sk` small) |
| `/thumb` | `doc, gen, page, w[, rot]` | `image/png`, `set_target_width(w).set_maximum_height(w*2)` |
| `/ocr` | `doc, gen, page, dpi` | `image/png` gray8 at `dpi` (300 default) |
| `/recent-thumb` | `id` | `image/png` from `$APPDATA/SeePDF/thumbs/<id>.png` (read on the io thread) |
| `/raw` | `doc` | `application/pdf`, honours `Range:` → 206 |

* `sk` = `scaleKey` = `round(zoomPercent × devicePixelRatio)`; device scale `s = sk / 100`.
* `tx`,`ty` are **tile indices** (device origin = `tx·512, ty·512`).
* `vn` (Stage 6b, P1-12) is the `viewNonce` that `set_annotations_hidden` returned, on the **one page**
  it changed. The engine ignores it (it is not part of the cache key or the ETag; `set_annotations_hidden`
  already dropped the document's cached tiles); it only makes the URL new, because every image response
  is `Cache-Control: immutable` and the webview would otherwise keep painting the bitmap from before the
  annotation was hidden. A page with no nonce sends no `vn`, so its URLs are unchanged.
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
  layout: 'single' | 'continuous' | 'two' | 'twoCover'; pinned: boolean; thumbId: string | null;
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
  defaultLayout: 'single' | 'continuous' | 'two' | 'twoCover';   // v0.3: twoCover = 두 쪽 (표지 따로)
  defaultZoom: 'fit-width' | 'fit-page' | 'actual' | number;
  restorePosition: boolean; author: string; renderQuality: 'balanced' | 'high';
  tileCacheMb: number; recentsCount: number;   // Stage 2: first-class, was `toolDefaults.recentsCount`
  backupsEnabled: boolean; ocrLanguages: string[]; ocrDpi: 'auto' | 200 | 300 | 400;
  toolDefaults: Record<string, unknown>;   // Stage 6b: per-tool style overrides, see below
  autosaveSec: number;          // Stage 5: autosave interval in seconds, 0 = off; default 60 (absent in old files → 60)
  signatures: SavedSignature[]; // Stage 6b (P1-9): 서명 보관함, ≤ 10, newest last; absent in old files → []
  night: 'off' | 'dark' | 'sepia';   // Stage 8 (P1-10): persisted 야간 모드; absent (or unknown) in old files → 'off'
  checkUpdates: boolean;        // v0.2.0: 시작할 때 업데이트 확인; absent in old files → true
}
export type SavedSignature =
  | { kind: 'drawn'; id: string; paths: number[][]; aspect: number; createdAt: string }  // unit space, y-down
  | { kind: 'typed'; id: string; text: string; style: string; createdAt: string };      // style: script | hand | formal
get_settings(): Promise<Settings>
set_settings(a: { patch: Partial<Settings> }): Promise<Settings>
write_signature_image(a: { bytes: number[] }): Promise<string>   // Stage 6b (P1-9): PNG → absolute path
list_pdf_files(a: { dir: string; recursive?: boolean }): Promise<string[]>   // P2 여러 파일에서 검색 › 폴더 추가
// v0.3 pkg6
get_default_settings(): Promise<Settings>                        // U3 기본값으로 되돌리기
clear_render_cache(): Promise<number>                            // U3 캐시 비우기 → bytes freed
save_snapshot_png(a: { path: string; bytes: number[] }): Promise<void>   // V1 스냅샷 › PNG로 저장…
```

**v0.3 (U3).** `renderQuality` takes effect: `high` renders page bitmaps and tiles at 1.5 × the display density
(at most 3), so `sk = round(zoom × dpr × 1.5)`, and the viewer CSS-scales the denser bitmap into the same page box;
a whole-page bitmap then serves as its own placeholder (no low-resolution draft pass). `balanced` is unchanged.
`set_settings` applies a new `tileCacheMb` at once: the tile cache's budget changes and it evicts least recently used
entries down to it (`TileCache::set_budget`, no pdfium — it runs on the command thread). `get_default_settings` is
`Settings::default()`; 기본값으로 되돌리기 writes it with `set_settings` minus `author`, `signatures` and `stamps` (the
user's own data; recents are a separate file). `clear_render_cache` empties the encoded tile / page cache (no pdfium)
and returns the bytes it held. **`save_snapshot_png`** writes a PNG the 스냅샷 tool could not put on the clipboard to
the path the save panel returned: the bytes must start with the PNG signature and the path end in `.png`
(`invalidArgument` otherwise); file I/O on `spawn_blocking`, errors `io`.

**`list_pdf_files`** (P2): every `*.pdf` (extension in any case) below `dir`, sorted by path — recursive unless
`recursive: false`, at most 16 levels and 2 000 paths, hidden entries (`.name`) skipped, symbolic links not followed.
File-system only, on `spawn_blocking`; no PDFium. An unreadable `dir` is `io`; an unreadable sub-directory is
skipped. **여러 파일에서 검색** itself adds no command: the frontend opens each file with `open_document` beside the
window's document, runs `search_start { fromPage: 0 }` to `done`, and `close_document`s it (§6).

**`toolDefaults`** (Stage 6b, P1-12): keyed by tool id (`highlight`, `underline`, `strikeout`, `squiggly`,
`note`, `pen`, `eraser`, `rectangle`, `ellipse`, `line`, `arrow`, `textbox`), each a *partial*
`{ color, opacity, width, fontSize, fillColor, heads, align, eraserSize }` holding only what the user
changed; the frontend validates each field and ignores other keys (the pre-Stage-2 `recentsCount` is left
alone). **`signatures`** is read leniently in Rust: an entry that does not parse is dropped on its own and the
list is capped at 10, so one bad entry never resets every setting to the default.

**`write_signature_image`** (Stage 6b, P1-9): the webview renders a typed signature to a transparent PNG and
sends its bytes (a JSON array → `Vec<u8>`). Rust checks the PNG signature and dimensions (≤ 8 MB, ≤ 4096 px a
side), writes `$APPDATA/SeePDF/signatures/sig-<fnv64>.png` (write-then-rename; the same bytes reuse the same
file), keeps the 32 most recently used files, and returns the absolute path — which the 서명 tool places through
the existing `AnnotSpec { kind: 'stamp', image: { path } }`. File I/O only, on `spawn_blocking`; no pdfium.
Errors: `invalidArgument` (not a PNG / too large), `io`.

Backed by `tauri-plugin-store` (`settings.json`, `recents.json`) from Rust so the values are available
before the webview mounts (window state, locale, theme). Owner: S0. Features F-01, F-27, F-28.

Native dialogs are called directly (no npm wrapper needed):
`invoke('plugin:dialog|open', { options: { title, multiple, filters: [{ name: 'PDF', extensions: ['pdf'] }] } })`
and `invoke('plugin:dialog|save', { options: { defaultPath, filters } })`. The dialog plugin adds picked
paths to the fs scope automatically.

### 11a. Read aloud — 읽어 주기 (P2)

```ts
export interface TtsStatus {
  supported: boolean; speaking: boolean; engine: 'say' | 'sapi' | null; voice: string | null;
  sentenceIndex?: number | null;   // v0.3: the sentence being read (sentence mode)
}
tts_speak(a: { text?: string; lang?: string; rate?: number;
               sentences?: string[]; startIndex?: number }): Promise<TtsStatus>   // rate 0.5 … 2, default 1
tts_stop(): Promise<TtsStatus>
tts_status(): Promise<TtsStatus>
```

`app/tts.rs` + `commands/tts.rs` — no pdfium, so not on the engine thread (a blocking pool thread instead).
The operating system's own voice, offline: macOS `/usr/bin/say` with the text on **stdin** (no argv length
limit, a leading `-` cannot become an option), `-v` = `Yuna` for Korean text when installed (else the first
`ko_*` voice of `say -v ?`), `-r` = 185 × rate words per minute; Windows `powershell.exe -EncodedCommand`
running `System.Speech.Synthesis.SpeechSynthesizer` (text read from stdin as UTF-8 bytes, a voice whose culture
matches, `Rate` = round(10 · ln rate / ln 3)), no console window; elsewhere `supported: false` and `tts_speak`
answers `unsupported`. `lang` (`ko`, `en-US`) picks the voice; absent = Korean when the text has Hangul, else
the system voice. One utterance for the whole app: `tts_speak` stops the current one first; `tts_stop`, the
state being dropped and `RunEvent::Exit` kill the process. `speaking` turns false by itself when the voice
ends (the frontend polls `tts_status` every 500 ms while its bar is up). Errors: `invalidArgument` (blank text,
> 200 000 characters, a non-positive rate), `unsupported`.

**Sentences (v0.3, V4).** With `sentences` (the frontend splits the text Korean-aware — `src/tts/sentences.ts` — and
keeps each sentence's text-layer ranges) `text` is ignored: the queue is kept in Rust and spoken one utterance per
sentence from `startIndex` (default 0); a driver thread starts the next as soon as one ends (checked every 40 ms)
and emits **`tts-progress { sentenceIndex }`** as each starts, then `{ sentenceIndex: null }` when the queue ends or
is stopped. The voice's language is picked once for the whole text. `tts_status` reports `sentenceIndex` and stays
`speaking` between two sentences. A 속도 change is a new `tts_speak` with the same sentences and `startIndex` = the
current sentence, so reading continues where it was. Errors add: every sentence blank, `startIndex` past the end
(`invalidArgument`).

---

## 12. Command → owner → feature index

| Command | Owner module (WORKPLAN) | Feature |
|---|---|---|
| `open_document`, `close_document`, `get_document`, `get_outline`, `take_pending_opens`, `open_in_new_window`, `window_bind_document` | S0 engine-core / app | F-01, F-05 |
| `get_recent`, `update_recent`, `remove_recent`, `set_recent_pinned`, `clear_recent`, `write_recent_thumbnail`, `reveal_in_file_manager`, `get_settings`, `set_settings` | S0 app | F-01, F-27, F-28 |
| `write_signature_image` | Stage 6b, `commands/app.rs` + `app/signatures.rs` | P1-9 |
| `set_viewport`, `engine_stats`, `render_page_raw` | S0 render | F-02, F-26, F-30 |
| `get_text_layer`, `get_page_text`, `search_start`, `cancel_job` | S0 text | F-06, F-07 |
| `undo`, `redo` | S0 engine-core (history) | F-15 |
| `list_annotations`, `scan_annotations`, `create_annotation`, `update_annotation`, `delete_annotations`, `set_annotations_hidden` | (a) backend annotations | F-08…F-14 |
| `list_form_fields`, `set_form_field_value`, `reset_form` | (a) backend forms | F-20 |
| `redact_preview`, `apply_redactions` (legacy since v0.3: the UI uses `apply_redactions_batch`) | (a) backend redaction | F-22 |
| `apply_redactions_batch` | Stage 8, `engine/redact` | F-22 |
| `page_ops`, `extract_pages`, `split_document`, `merge_documents` | (b) backend pages | F-16 |
| `list_page_objects`, `probe_text_edit`, `edit_text_object`, `add_text_object`, `add_image_object`, `transform_object`, `delete_objects` | (b) backend objects | F-17, F-18, F-19 |
| `probe_paragraph`, `edit_paragraph` | Stage 7, `engine/objects/paragraph.rs` | F-17 |
| `duplicate_objects` | Stage 8, `engine/objects` | F-17, F-19 |
| `save_document`, `save_document_as` | (b) backend save | F-23 |
| `path_exists` | Stage 6a, `commands/save.rs` | P1-7 |
| `export_images`, `export_text`, `export_flattened`, `estimate_export`, `print_prepare` | (b) backend export | F-24, F-25, F-26 |
| `ocr_capabilities`, `ocr_page_status`, `ocr_apply` | (b) engine OCR layer + (f) worker pipeline | F-21 |
| `remove_password`, `set_password`, `remove_metadata`, `set_metadata`, `ocr_recognize_native` | (b), P1 | P1-1, P1-2, P1-3, P1-11 |
| `add_stamp`, `remove_stamps` (Stage 8) | Stage 4, `engine/stamp.rs` | P1-4 |
| `compress_estimate`, `compress_apply`, `compress_discard` | Stage 4, `engine/compress.rs` | P1-5 |
| `compare_documents` | Stage 5, `engine/compare.rs` | P1-6 |
| `write_recovery`, `clear_recovery`, `list_recovery`, `discard_recovery` | Stage 5, `engine/recovery.rs` | P1-8 |
| `set_page_boxes`, `resize_pages` | P2, `engine/pages/boxes.rs` | P2 crop / resize |
| `export_annotation_summary` | P2, `engine/export/summary.rs` | P2 annotation summary |
| `tts_speak`, `tts_stop`, `tts_status` | P2, `app/tts.rs` | P2 read aloud |
| `set_outline`, `create_link`, `update_link`, `delete_link`, `set_page_labels`, `get_page_labels` | P2, `engine/structure/` + `commands/structure.rs` | P2 outline / links / labels |
| `reply_annotation` | P2, `engine/annot/reply.rs` (lopdf via `registry::mutate_bytes`) | P2 threads |
| `list_pdf_files` | P2, `commands/save.rs` | P2 multi-file search |
| `get_web_links`, `get_reading_order` | v0.3 pkg6, `engine/text/{weblinks,structtree}.rs` + `engine/raw/` | V5, V6 |
| `get_default_settings`, `clear_render_cache`, `save_snapshot_png` | v0.3 pkg6, `commands/app.rs` | U3, V1 |

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
