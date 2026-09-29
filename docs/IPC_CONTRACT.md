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
  | 'io'                 // filesystem error
  | 'engineCrashed'      // v0.3 pkg5 (H4): the command panicked on the engine thread (see below)
  | 'fileChangedOnDisk'; // v0.3 H8: save_document found the file changed since open / last save (§7.11)

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

**`open_document`'s `detail`** (v0.3 pkg5, H3): a failed open says why — `notPdf` (no `%PDF-` in the first
1 KiB: a renamed text or image file), `corrupted` (PDFium's `FPDF_ERR_FORMAT`), `outOfMemory` (the file read ran
out of memory; any `io` error from `std::io::ErrorKind::OutOfMemory` carries it). Password errors carry none.
`api.openErrorKey` maps them to `error.notPdf` / `error.corrupted` / `error.outOfMemory`.

**`engineCrashed`** (v0.3 pkg5, H4): the release profile unwinds, and the engine loop runs every command under
`catch_unwind`. A panicking command answers its caller `engineCrashed` (`message`: `<label> panicked: …`); every
document the command looked up (`EngineState::doc` / `doc_mut`) is closed — its PDFium state may be half-edited
— and `engine-crashed` (§8) names them; the engine thread keeps serving every other document. A crash inside
PDFium's own C++ is not a panic and still ends the process.

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
  signatures?: SignatureInfo[];   // v0.3 S1: detected digital signatures (never validated); absent when none (§7.11)
  incrementalSave?: boolean;      // v0.3 S1: signed only — whether the next save appends (signatures stay intact)
  attachmentCount?: number;       // v0.3 S4: document-level attachments; absent when 0
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
≤ 8 192 px, ≤ 36 M px in all — the engine refuses a region over 40 M px) and turns it by the view rotation in the webview. Owner: S0 (render). Features F-02, F-26.

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

### 7.2a Form authoring, form data, flattening (v0.3, pkg2)

```ts
export type NewFieldType = 'text' | 'checkbox' | 'radio' | 'combo' | 'signature';
export interface FormFieldSpec {
  page: PageIndex; rect: Rect; type: NewFieldType; name: string;
  options?: string[];      // combo: the choices; radio: this button's export value (first entry)
  maxLen?: number; required?: boolean; multiline?: boolean;
}
export interface FormFieldPatch { name?: string; options?: string[]; required?: boolean; maxLen?: number }  // maxLen 0 removes
export interface FormEditResult { info: DocInfo; fields: FormField[]; field?: FormField }
export type FormDataFormat = 'csv' | 'xfdf';
export interface FormDataResult { fields: number; unknown: string[]; docGeneration?: DocGeneration }

create_form_field(a: { docId: DocId; spec: FormFieldSpec }): Promise<FormEditResult>                 // undo.formFieldCreate
update_form_field(a: { docId: DocId; page: PageIndex; index: number; patch: FormFieldPatch }): Promise<FormEditResult>  // undo.formFieldEdit
delete_form_field(a: { docId: DocId; page: PageIndex; index: number }): Promise<FormEditResult>        // undo.formFieldDelete
export_form_data(a: { docId: DocId; format: FormDataFormat; outPath: string }): Promise<FormDataResult>  // no document change
import_form_data(a: { docId: DocId; path: string; format?: FormDataFormat }): Promise<FormDataResult>     // undo.formImport
flatten_form(a: { docId: DocId }): Promise<DocInfo>                                                   // undo.formFlatten
```

* **Authoring** is a lopdf rewrite (`registry::mutate_bytes`, one undo step, reopened by PDFium): PDFium can
  fill a field but not make one. A new field is one widget + field dictionary (`/FT /Tx|/Btn|/Ch|/Sig`, `/T`,
  `/Ff` required 2 / multiline 4096 / combo 131072, `/MaxLen`, `/Opt`, `/DA (/Helv 0 Tf 0 g)` or `/ZaDb` for
  toggles, `/F 4`, `/P`, a grey `/MK /BC` border and a basic `/AP`); a radio button is a kid widget of a group
  field (`/Ff 49152`) whose on-state is its export value — a button named after an existing radio group joins
  it (an export value the group already has is renumbered: `선택 1` → `선택 2`). `update_form_field` renaming a
  radio group after another radio group **merges** them: its buttons join that group (clashing export values
  renumbered), the target keeps its value. `/AcroForm` gains `/Fields`, `/DA` and `/DR /Font` (`/Helv`, `/ZaDb`) as needed; a document that had no
  form gets its form handle on the reload. `field` in the result is the created / edited field (PDFium's
  listing). Errors: `invalidArgument` for an empty name or one with `.`, a name another root field has (except a
  radio joining its group), a rect under 4 × 4 pt, a page out of range, a combo without choices;
  `unsupported` on an encrypted or XFA document; `notFound` for a `(page, index)` that is not a widget.
  `delete_form_field` removes the widget, and its field (and any ancestor left empty) with it.
* **Data**: CSV is UTF-8 with a BOM, header `name,value`, one row per field (a multi-select list: one row per
  selected option), RFC 4180 quoting; XFDF is `<xfdf><fields><field name><value>` with dotted names nested
  (import accepts nested and dotted). Values are what `list_form_fields` reports (a checkbox / radio: its state
  name, `Off` or the export value — read with lopdf as UTF-8, so a Hangul export value such as `여` or `선택2`
  is written and matched exactly as typed; `FormField.value` of a checkbox / radio reports it the same way,
  where PDFium's own reading garbles it); push buttons and signatures are skipped. Import writes every matching field
  in **one** undo step; `unknown` lists names with no field. `format` absent = sniffed (`<` first → XFDF).
* **`reset_form`** (changed in v0.3): every writable field goes back to its **`/DV`** — text / combo to the `/DV`
  string or empty, a list to the options `/DV` names, a checkbox on only when `/DV` names its on-state, a radio
  group to the button whose export value `/DV` names, or **switched off** when it has none — except where no
  group can be switched off (a lopdf rewrite): a document whose permissions forbid "modify" (v0.3 integration:
  since S2 / S5 an encrypted document opened with full rights is rewritten and re-encrypted, so there it is
  switched off) or a signed document that still saves incrementally (S1 `pristine`: the rewrite would invalidate
  its signatures). Radio groups are then left as they are and every other field is still reset (import likewise
  skips a radio set to `Off`). `/DV` and `/MaxLen`
  are read with lopdf from the bytes the document was loaded from (matched by field name + rect).
* **`set_form_field_value { checked: false }` on a radio button that is on** (v0.3) switches its whole group off
  (`/V /Off`, every kid `/AS /Off`, a lopdf rewrite after the form-fill environment wrote the rest) instead of
  answering `unsupported`; refused (`permissionDenied`, v0.3 integration) on a document that forbids "modify".
* **`FormField.maxLen`** is now reported for text fields (lopdf `/MaxLen`, inherited through `/Parent`).
* **`flatten_form`**: every widget's normal appearance (`/AP /N`, the `/AS` state for a toggle) is drawn into its
  page as a Form XObject mapped from `/BBox` × `/Matrix` onto `/Rect`; hidden (`/F` 2) and no-view (`/F` 32)
  widgets are dropped undrawn; the page's old content is wrapped in `q … Q`; the widgets leave `/Annots`
  (**every other annotation stays** — not `FPDFPage_Flatten`, which flattens all of them) and `/AcroForm`
  leaves the catalog. `invalidArgument` when there is no field; the reopened document must have no form.

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

#### v0.3 additions (pkg2)

```ts
split_document(a: { docId; mode: { everyN } | { ranges } | { byOutline: { level: number } }; outDir }, onProgress)
import_pages_from_doc(a: { srcDocId: DocId; pages: PageIndex[]; dstDocId: DocId; at: PageIndex }): Promise<DocInfo>  // undo.pageImport
write_temp_image(body: Uint8Array /* raw request body: PNG or JPEG bytes */): Promise<string>   // a temp file path
```

* **Merge and insert-from-file carry the source's structure** (P1). `FPDF_ImportPages*` copies page
  dictionaries but not the catalog, and leaves the keys `Parent` / `Prev` / `First` of what it copies pointing at
  the *source's* object numbers — a widget's field and a popup's parent come out dangling. After the import the
  bytes are rewritten with lopdf (`engine/pages/carry.rs`): widgets are re-parented to copies of their source
  fields (values, flags, `/DA`, `/Opt`, `/AA`; `/Kids` rebuilt from the imported widgets only), the new root
  fields join `/AcroForm /Fields` — a root whose `/T` is taken is renamed `name_2`, `name_3`, … — and the
  source's `/DR` fonts and `/DA` join the destination's; popups get their `/Parent` back; the source's outline
  (read with PDFium) goes under a new top-level node named after the file, pages mapped to their new indices (a
  node whose page was not imported keeps its title only when a child survives); page labels are recomputed page
  by page from each document's own ranges and compressed back into ranges, so the destination's pages after the
  insertion keep their numbers. `merge_documents` adds one top-level node per file when any file has an outline
  and reports `formsDropped` / `outlineDropped` only if the rewrite could not be done (PDFium's plain import is
  kept then); `metadataDropped` is unchanged. `page_ops [insertFrom]` takes this path when it is the **only** op
  of the batch and the document is not encrypted (a byte-level rewrite through
  `registry::mutate_bytes_resized` — still one undo step `undo.pageInsertFrom`); a multi-op batch, an
  encrypted document and (v0.3 integration, S1) a signed document that still saves incrementally keep the plain
  import, so its signatures survive the next save. `import_pages_from_doc` follows the same rule.
* **`import_pages_from_doc`** (P3): pages of one open document copied into another at `at`, in the order given
  (duplicates dropped); the source's unsaved edits are included and it is not changed; the target gets one undo
  step `undo.pageImport`; widgets keep their fields (renamed on a clash) and popups their parent (fields only —
  no outline or labels). Same document on both sides → `invalidArgument`. An encrypted target, or a signed one
  that still saves incrementally, takes PDFium's plain import. v0.3 integration (S5): a **restricted source**
  (opened without every permission) → `permissionDenied`, detail `security`, like a restricted file given to
  `insertFrom` / `merge_documents`. The signed-document gate (`MUTATING_COMMANDS`) asks about the target.
* **`split_document { byOutline: { level } }`** (P4): one file per outline node of `level` (1 = top level) that
  points at a page, from its page to the page before the next such node; pages before the first node are a part
  named after the document; nodes on the same page fold into the first. Files are `NN 제목.pdf` (the title with
  `\ / : * ? " < > |` and control characters replaced by `_`, ≤ 80 characters, `(2)` on a clash).
  `invalidArgument` when the outline has no node with a page at that level.

### 7.3b Images to PDF — 이미지로 PDF 만들기 (v0.3 D1, pkg2)

```ts
export type ImagePageSize = 'original' | 'a4' | 'letter';
export type ImageFit = 'contain' | 'actual';
create_from_images(a: { paths: string[]; pageSize: ImagePageSize; margin?: number /* pt, 0…200 */; fit?: ImageFit },
                   onProgress: Channel<JobEvent>): Promise<DocInfo>
```

A new **untitled, dirty** document (like a merge), named `<first image>.pdf`, one page per image in order.
The images are read on a blocking worker (no PDFium): format by magic bytes (PNG, JPEG; anything else →
`unsupported`), resolution from PNG `pHYs`, JPEG JFIF density or EXIF `XResolution`/`ResolutionUnit` (96 DPI
when none), EXIF orientation applied (the page follows the displayed orientation). A JPEG without an EXIF
rotation is embedded **as its own bytes** (`/DCTDecode`, `FPDFImageObj_LoadJpegFileInline`); PNGs and rotated
JPEGs as bitmaps. `original` = the image at its resolution plus `margin` on each side (scaled down to the
14 400 pt limit); `a4` / `letter` = portrait, landscape for a landscape image, the image centred in the page less
its margins — `contain` scales it to fill that box, `actual` keeps its size unless it does not fit. Progress:
`started` (total = images + 1), one `progress` per image read, one for the build, `done`; `cancel_job` between
images → `cancelled`. `write_temp_image` writes clipboard bytes (the raw request body) to
`$TMPDIR/seepdf-clipboard/<uuid>.png|jpg` for 클립보드에서 새로 만들기 and for pasting an image in 편집 (then
`add_image_object`).

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
variance (justify needs ≥ 3 lines). A click on rotated text → `refused/rotatedText` (v0.3: probed in its
own frame instead, see below), on a Form XObject with text → `refused/insideXObject` (v0.3: the UI offers
`ungroup_object`, §7.4c), on the invisible OCR layer → `refused/invisible`; a `noUnicode` run refuses
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
are appended, the old ones removed; one `registry::mutate` = one undo step `undo.paragraphEdit` (text **and** every flow
move).

**v0.3 (pkg1, R5) — reading order.** PDFium writes objects it did not parse into a new content stream at
the end of the page, so the edited paragraph used to be read (copied, searched, read aloud) after the rest
of the page. Now the write leaves a placeholder in the old paragraph's stream — an emptied copy of its first
run, moved out of a second parse of the page (so it keeps that run's stream), marked `/SeePDFOrderAnchor` —
and marks the new objects `/SeePDFOrderPara`; after the mutation a `lopdf` pass over the serialised file
(`flow::restore_reading_order`) moves the marked blocks to the placeholder (marks stripped, placeholder
dropped, a stream left with nothing drawn removed), PDFium re-verifies the bytes and the document is
replaced **without** a history entry of its own — still one undo step, and the page renders pixel-identical.
Only streams holding a marker are decoded and rewritten (an inline image elsewhere is never touched).
Skipped (the paragraph stays last, as before) for an encrypted document (lopdf cannot write it back), a
signed document that can still be saved incrementally (S1 `pristine`: the rewrite would force a full save and
invalidate the signatures — v0.3 integration, `tests/integration_v03.rs`), a
deleted paragraph, and whenever the pass fails (logged; the edit itself stands). Cost: one serialise +
lopdf round trip + reload per committed edit (dry runs are unaffected).

**v0.3 (pkg1, R5) — rotated text.** A click on text whose baseline is rotated by θ (a caption up the
margin, a landscape table on a portrait page) probes again in a frame rotated by θ, where that text is
upright: detection, `analyze` and the layout run unchanged there, `rect` is the page-aligned box around the
frame rect, and the new objects are drawn rotated back onto the page (`[cos θ, sin θ, −sin θ, cos θ, x, y]`).
The flow never moves anything for a rotated paragraph (`shiftedPt` 0; the content "below" it is not below
it on the page). Runs at another angle inside that frame stay obstacles. `fontSizePt` scales the line height proportionally. Known gaps: hanging indents / bulleted lists detect
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

### 7.4c Groups — `ungroup_object` (v0.3, pkg1 R4)

```ts
export interface UngroupResult { docGeneration: DocGeneration; objects: PageObject[]; newObjectIds: ObjectId[] }
ungroup_object(a: { docId: DocId; page: PageIndex; objectId: ObjectId; expectGeneration: DocGeneration }):
  Promise<UngroupResult>
```

Engine: `engine/objects/ungroup.rs`. Replaces the Form XObject `objectId` (a top-level `type: 'form'` object)
by its children, in place: each child is detached (`FPDFFormObj_RemoveObject`), transformed by the form
matrix (form ∘ child, clip path included), its fill / stroke colour re-set as RGB (PDFium's content writer
only writes DeviceRGB / DeviceGray colours and drops an ICC / `/CalRGB` / `/Separation` one, which would
paint it black), and inserted at the form's index (`FPDFPage_InsertObjectAtIndex`); the emptied form object
is removed and the page regenerated — one undo step `undo.ungroup`. `newObjectIds` = the children, in drawing
order (`objectId … objectId + n − 1`). Errors: `stale` on generation mismatch; `notFound` for an index past
the end; `invalidArgument` (detail `notAGroup`) for anything else; `unsupported` (detail
`groupTransparency`) for a form drawn with transparency (group alpha, blend mode, soft mask), whose children
would come out opaque. Not carried over (PDFium limits): the text-state operators `Tc` / `Tw` / `Tz` of the
children (PDFium never writes them for text it did not create — a line justified by character spacing
tightens; `TAMReview.pdf` p.2 has three) and a clip path set on the page before `/Form Do` (the form's
`/BBox` clip is kept, it is part of every child's clip path).

### 7.5 Redaction and security

```ts
export interface RedactPreview {
  page: PageIndex;
  textObjects: { objectId: ObjectId; text: string; rect: Rect; fullyInside: boolean;
    split: boolean }[];              // v0.3 (R2): partly marked, only the marked characters go
  imageObjects: { objectId: ObjectId; rect: Rect; fullyInside: boolean;
    blank: boolean }[];              // v0.3 (R1): partly marked, its pixels under the marks are blanked
  annotations: AnnotId[];
  formFields: string[];              // non-empty ⇒ the apply will refuse
  collateral: string[];              // text that will be removed although it is outside the marks
  groups: ObjectId[];                // v0.3 (R4): Form XObjects holding marked text / images under a mark
}
redact_preview(a: { docId: DocId; page: PageIndex; rects: Rect[] }): Promise<RedactPreview>
apply_redactions(a: { docId: DocId; page: PageIndex; rects: Rect[];
  options: { fill: Rgb; overlayText?: string; ungroup?: boolean } }):
  Promise<{ removedObjects: number; verified: boolean; docGeneration: DocGeneration }>
// Stage 8: every marked page in ONE undo step (`undo.redact`); a verifyFailed on any page rolls back all.
export interface RedactBatchMark { page: PageIndex; rects: Rect[] }
export interface RedactBatchResult { removedObjects: number; verified: boolean; docGeneration: DocGeneration; pages: PageIndex[];
  collateral?: string[] }            // v0.3: runs the preview promised to split that went whole after all
apply_redactions_batch(a: { docId: DocId; marks: RedactBatchMark[];
  options: { fill: Rgb; overlayText?: string; ungroup?: boolean } }): Promise<RedactBatchResult>

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

**v0.3 (pkg1) redaction** (`engine/redact/`: `mod.rs`, `split.rs`, `image.rs`, `raw.rs`; round 2:
`survivors.rs`, `contents.rs`, `spacing.rs`). A character is
*marked* when its tight box overlaps a mark with positive area (every step uses this one rule).
* **Text (R2)** — a text object the marks cover only partly is **split**: its characters are read from a
  text page (`FPDFText_GetTextObject`), cut into runs of unmarked characters; the object is rewritten to the
  first run (`set_text` through the font's own `/ToUnicode`, origin moved to the run's first glyph, every
  glyph pinned with `FPDFText_SetPositions`, which PDFium then writes as a `TJ`) and keeps its place in the
  stream; each further run is a copy moved out of a
  second parse of the page (clip path, colours, marks kept), inserted right after it. Runs are trimmed of
  leading / trailing whitespace; a ligature PDFium reports as several characters at one origin and box
  ("fi") is written back as its presentation form (U+FB01), and a line-end hyphen (reported as U+0002) as
  `-`. Every run must read back from a fresh text page with the same visible (non-whitespace) characters at
  the same tight boxes (± 0.5 pt) before the content is regenerated, and again from a re-parse after it; a
  run that fails afterwards makes the whole batch roll back and retry with that object split without its
  spaces (a font with no `/ToUnicode` entry for U+0020 has `set_text` write the space as code 0, which moves
  the rest of the run once saved), then removed whole (its text then in `collateral`).
  **`redact_preview` is a dry run of the apply** (v0.3 round 2): the static plan, then the whole apply —
  split, regeneration, re-parse, every post-condition, the retries — on a throw-away second open copy of the
  document (groups ungrouped as after the UI's confirm, image pixels untouched), closed again. So
  `split: true` is reported only when it survives the save, every fallback is in `collateral` before the
  confirm, and a page the apply would refuse makes the preview fail with the same `verifyFailed`. `split: false` (whole-run removal, `collateral` named first)
  for Type3 fonts (no font program — PDFium's writer drops Type3 text), fonts without a usable
  `/ToUnicode`, text carrying `/ActualText` (extraction reads the replacement, not the glyphs), runs not on
  one baseline, and runs that fail the trial (a glyph the font cannot re-encode from its Unicode value).
  **Tagged content:** every string parameter (`/Alt`, `/E`, `/ActualText`, …) of the marks on a split or
  removed text object is dropped (PDFium shares a mark item between the objects of one `BDC`, so siblings
  are cleaned too; `/MCID` stays), as is any mark string on the page that contains a marked string
  (whitespace- and case-insensitive); after regeneration a mark string still holding one is `verifyFailed`.
* **Images (R1)** — entirely inside a mark: removed. Partly covered: every image pixel whose footprint on
  the page (the unit-square pixel mapped through the image matrix — rotated / flipped images included)
  overlaps a mark is set to black in the object's own bitmap, written back into the same object (matrix,
  clip, graphics state kept): `/DCTDecode` / `/JPXDecode` re-encoded as JPEG q 92 via
  `FPDFImageObj_LoadJpegFileInline` (as `compress.rs`), anything else via `FPDFImageObj_SetBitmap` (Flate).
  Verified on the data read back: exactly 0 for Flate, ≤ 96 per channel and mean ≤ 16 for JPEG, else
  `verifyFailed` (rendering the rect would prove nothing, the fill box is on top). A rasterisation of the
  image alone (mask applied, one pixel per image pixel) before and after may differ in about as many pixels
  as were blanked; more ⇒ PDFium handed back palette indices or an inverted `/Decode`, and the image is
  removed whole instead. `blank: false` (removed whole) for images with an `/SMask` / `/Mask` (found through
  that rasterisation's alpha — `FPDFPageObj_HasTransparency` does not see an image's own mask), 1-bit
  images that are not gray, `/Indexed`, `/Separation`, `/DeviceN` and pattern images. The old
  stream is not written to the saved file.
* **Paths** — entirely inside: removed; partly covered: removed when small (≥ ¼ of its box under the marks,
  or no side > 36 pt) or curved (outlined glyphs, logos); a large straight-segment path (table grid, frame,
  background) stays under the fill box — PDFium has no per-object clip, and a rule carries no content.
* **Groups (R4)** — marked text or an image under a mark inside a top-level Form XObject: listed in
  `groups`. Without `options.ungroup` the apply refuses as before (`verifyFailed`, the stuck text named; detail
  `groups` when only images are inside); with it those groups are ungrouped first (§7.4c, lossy: a group's
  transparency is not a reason to refuse here), nested groups up to 8 levels, inside the same undo step.
* **Everything else stays (round 2).** PDFium's content generator writes no `Tc` / `Tw` and computes the
  `TJ` adjustments without them, so every justified or letter-spaced run of a regenerated stream moves
  (untouched ones included). Each regeneration is therefore followed by a re-parse in which every text object
  that no longer draws its characters at the boxes it had in memory is put back (glyph positions pinned on
  its own char codes, else re-encoded; tried on a throw-away parse first) and the page regenerated again;
  after a successful batch, the repeating adjustment of those `TJ`s is written back as `Tc` / `Tw` with
  `lopdf` (only when the reopened file draws every character at the same box), so they also extract as
  before. Then a **page-wide post-condition**: every visible character that was on the page, is not under a
  mark and was not drawn by an object removed whole (`collateral`) must still be extractable at its box
  (± 0.5 pt), else `verifyFailed` (detail `lost`, the text named) — e.g. a Type3 run elsewhere on the page,
  which PDFium cannot write at all. A page whose `/Contents` streams break mid-object (`160F-2019.pdf`:
  `…(que)]` ends one stream, `TJ ET EMC` starts the next — regenerating one leaves a dangling fragment that
  breaks the rest of the page) has its streams joined into one with `lopdf` and the batch retried; the old
  streams leave the file; undo returns the document as it was, its own streams included.
* **Stencil masks (R1, round 2)** — a 1-bit `/ImageMask` scan is blanked in place too: it is replaced by
  its own rasterisation (fill colour where it paints, transparent elsewhere → RGB + `/SMask`) with the
  marked pixels opaque black, verified like the others; the mask stream leaves the file.
* `removedObjects` counts objects removed, split or blanked.

Stage 3 semantics (`docs/STAGE3_SECURITY_NOTES.md`):
* `set_password.permissions` may be partial: a missing flag means **allowed**; `revision` is ignored.
  The open document is untouched; the copy is verified (opens with user and owner password, refuses
  no password when a user password is set) before anything is written. Empty `ownerPassword` or a
  password over 127 UTF-8 bytes → `invalidArgument`; a failed check → `verifyFailed`.
* `set_metadata` / `remove_metadata` are **one undo step** each and return the reloaded `DocInfo`.
  In `set_metadata` an omitted field keeps its value, a blank string removes the key, and `modified`
  omitted means "now". Both drop the catalog XMP `/Metadata` stream. ~~An encrypted document →
  `unsupported`~~ — since v0.3 (S2, §7.11) an encrypted document is rewritten and **stays encrypted**
  with the same passwords and permissions; a document whose permissions forbid modification →
  `permissionDenied` (detail `modify`), no undo entry.

### 7.6 Save

```ts
export interface SaveResult { docId: DocId; path: string; bytes: number; docGeneration: DocGeneration; elapsedMs: number }
save_document(a: { docId: DocId; force?: boolean }, onProgress: Channel<JobEvent>): Promise<SaveResult>   // force: v0.3 H8 (§7.11)
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
* **v0.3 pkg8 (X5)** — `CompressOptions.optimize?: boolean` (default `false`) and three changes:
  * **Shared top-level images** (one stream drawn by several objects / pages) are no longer skipped: when every
    occurrence is above the threshold they go through the Stage 8 splice (re-encoded once, written into the one
    stream object; every occurrence counts in `imagesDownsampled`). Never on an encrypted document, and never
    for a stream something the scan does not see can draw — a page outside `pages`, or an annotation's appearance
    stream (lopdf resource reachability) — so a page range cannot shrink an image on the other pages.
  * **Masks**: PDFium's `FPDFPageObj_HasTransparency` does not see an image's own `/SMask` / `/Mask`, and
    `FPDFImageObj_SetBitmap` drops them, so the image dictionaries are read with lopdf first. An `/SMask` image is
    spliced and its 8-bit gray mask resampled by the same factor (kept at its resolution when lopdf cannot decode
    it); an all-255 mask counts as none; a `/Mask` (colour key / stencil) image, or one drawn with transparent
    graphics state, is left alone. Encrypted documents: soft-masked images are left alone.
  * **`optimize`** (unencrypted only): page resource entries (`/XObject`, `/Font`) whose name appears nowhere in
    the page's content are removed — a lexical scan of every `/Name` token (`#xx` decoded), not lopdf's content
    parser, which stops silently at a token it cannot read (form feed, NUL, a comment between operands). Streams
    without `/Resources` that PDFium draws with the page's — forms, annotation appearance streams, Type 3 glyph
    procedures, tiling patterns, soft-mask groups — count as the page's content. A resource dictionary shared by
    pages keeps the union; one referenced by anything but a page, or inherited from the page tree, is left alone.
    Empty content streams leave `/Contents`, unreferenced objects are pruned, and the file is written with object
    streams + an xref stream (`lopdf::save_modern`). Kept only when smaller, PDFium still opens it, **and every page
    that lost a resource entry renders identically** (160 px, annotations on) before and after; then a result with
    `imagesDownsampled == 0` but `afterBytes < beforeBytes` **is** applicable (the dialog passes the flag to its
    `applyBlock`).

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
* **v0.3 pkg8 (X4)** — `CompareOptions.visual?: boolean` (default `true`) and `DiffKind` `'visual'`:
  * every pair with both pages is also rendered at 50 DPI (`render_page_raw`'s render: annotations and form values
    included) on the engine thread; the text lines of both pages (grown by 2 px) are masked out; 8 × 8-px tiles with
    ≥ 3 pixels differing by more than 40 in a channel are changed; 8-connected tiles group into regions (at most 64,
    the largest). Any region adds **one** op `{ kind: 'visual', words: 0, rectsA, rectsB }` — the same regions in
    points on each page (a larger page is compared against white) — and makes the row `changed`. Not counted in
    `inserted` / `deleted`.
  * with `alignPages`, a candidate page without words is keyed by a 16 × 16 gray thumbnail signature (18 DPI render)
    instead of an empty word set: two scans score `1 − mean|Δ| / 24` (≤ 2 → 1), a scan against a text page 0 — so an
    inserted scanned page becomes a null-sided row like an inserted text page.

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
* **Owners** (v0.3 pkg5, H2): the sidecar also records `owner: { pid, started }` (the writing process; `started`
  = Unix ms of its first recovery write), and that process holds an exclusive lock on
  `.owner-<pid>-<started>.lock` in the directory while it runs. `list_recovery` skips a copy whose owner is
  another process that still holds its lock (a second SeePDF's live autosave); the OS drops the lock with the
  process, so a crashed owner's copies are offered and its stale lock file is removed. Sidecars without an
  owner (written before v0.3) are always listed. `RecoveryEntry` itself is unchanged.

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

**v0.3 pkg8** additions to §7.7:

```ts
print_prepare(a: { docId: DocId; pages?: PageIndex[]; annots?: PrintAnnots }): Promise<{ tempPath: string }>
export_text(a: { docId: DocId; pages: PageIndex[]; outPath: string; preserveLayout?: boolean }): Promise<{ chars: number }>
export type PrintAnnots = 'all' | 'none' | 'stamps';
```

* `print_prepare` (X7) flattens with **`FLAT_PRINT`**: what reaches paper is what the annotations' `/F` flags say
  prints (NoView + Print is baked in, an annotation without Print is dropped). `annots`: `all` (default) · `none` —
  every non-widget annotation removed first (form values still print) · `stamps` — only Stamp annotations and
  SeePDF signatures (`/Subj SeePDF:Signature`) kept. X8: the file is written to `$TMPDIR/seepdf-print/` and deleted
  10 minutes later (a detached timer), and any file older than 10 minutes is swept on every write;
  `engine::export::cleanup_print_temp()` empties the directory and is meant to run at startup and at exit (lib.rs).
* `export_text.preserveLayout` (X3): characters bucketed into rows by baseline, each word placed at the monospace
  column its x maps to (one column = the median advance of the page's narrow characters; East Asian wide characters
  take two), at least one space between words, a vertical gap of more than 1.5 rows becomes up to two blank lines.

### 7.7b 모아찍기 / 소책자 — `make_nup` (v0.3 pkg8, X2)

```ts
export interface NupOptions {
  perSheet: 1 | 2 | 4 | 6 | 9; order?: 'across' | 'down'; booklet?: boolean;
  paper?: 'auto' | 'a4' | 'letter'; annots?: PrintAnnots;
}
make_nup(a: { docId: DocId; pages?: PageIndex[]; options: NupOptions; outPath?: string }): Promise<{ path: string; pageCount: number }>
```

Engine `engine/export/nup.rs`, `Lane::Background`. The selected pages are first flattened for paper (`print_prepare`'s
bytes with `annots`), then imported in cell order into an intermediate document — 세로 방향 permutes each sheet so a
row-major fill reads down the columns; 소책자 pads to a multiple of 4 with blank pages and uses the saddle-stitch
order (8 pages → 8,1,2,7,6,3,4,5), always 2 per side on a landscape sheet — and laid out with
`FPDF_ImportNPagesToOne`. The grid (cols × rows = perSheet, portrait or landscape sheet) is the one that prints the
first page largest (A4 portrait pages: 2-up 2 × 1 landscape, 4-up 2 × 2 portrait, 6-up 3 × 2 landscape, 9-up 3 × 3).
`paper: 'auto'` = the first page's size. Written to `outPath` (내보내기) or, without one, to a print temp file (the
print path opens it with `open_document` and closes it after printing; deleted as above). Errors:
`invalidArgument` (perSheet not 1/2/4/6/9, page out of range), `permissionDenied` detail `print` (v0.3 integration:
the sheets are built from `print_prepare`'s bytes, which check the document's print permission — for 내보내기 ▸
모아찍기 PDF too). ⚠️ `FPDF_ImportNPagesToOne` ignores `/Rotate`.

### 7.7c Image export additions (v0.3 pkg8, X3)

```ts
export interface StitchStart { jobId: JobId; dpi: number; width: number; height: number; lowered: boolean }
export_embedded_images(a: { docId: DocId; pages: PageIndex[]; outDir: string; baseName: string },
                       onProgress: Channel<JobEvent>): Promise<JobId>
export_stitched_image(a: { docId: DocId; pages: PageIndex[]; dpi: number; format: 'png' | 'jpeg'; outPath: string },
                      onProgress: Channel<JobEvent>): Promise<StitchStart>
export_tiff(a: { docId: DocId; pages: PageIndex[]; dpi: number; outPath: string },
            onProgress: Channel<JobEvent>): Promise<JobId>
```

All three are page jobs (`engine/export/pagejob.rs`: one `Lane::Background` command per page in order, one
`JobToken`, Cancel within a page, one terminal event; state carried from page to page, `done.outputs` lists the files).
`pages: []` = every page. `export_embedded_images`: every image object as shown (soft mask / colour conversion
applied) → `<baseName>-p<page>-<n>.png`, 1-based. `export_stitched_image`: pages one under the other, centred on a
white canvas as wide as the widest page; the DPI is lowered (whole steps) until the canvas is ≤ 64 000 000 px and
≤ 65 000 px a side — the answer says `lowered` and the DPI used — JPEG quality 85. `export_tiff`: RGB, Deflate, the
DPI as the resolution tag, one IFD per page, written to `<out>.part` and renamed at the end (removed on cancel /
error). Errors: `invalidArgument` (page out of range, DPI outside 36–1200, too many pages for one image even at
36 DPI), `io`; `export_embedded_images` also `permissionDenied` detail `extractText` (v0.3 integration, §7.11 S5:
pulling the images out is "extract text and graphics"; checked before the job starts and on every page), like
`export_text` — with or without `preserveLayout`. The two page renders (stitched, TIFF) are not gated, like
`export_images`.

### 7.7d Text flow — DOCX / HWPX / HTML / Markdown (v0.3 pkg8, X6)

```ts
export type TextFlowFormat = 'docx' | 'hwpx' | 'html' | 'md';
export_text_flow(a: { docId: DocId; pages: PageIndex[]; format: TextFlowFormat; outPath: string },
                 onProgress: Channel<JobEvent>): Promise<JobId>
```

Engine `engine/export/textflow.rs`, a page job. Lossy by design (no conversion engine): each page's text-layer lines
join into paragraphs (same size ±15 %, baseline ≤ 1.8 × size below, no jump up, left edge within 3 × size; a trailing
hyphen before a lower-case letter is dropped, otherwise lines join with a space); the size with the most characters
is the body, and a paragraph of ≤ 200 characters at ≥ 1.6 / 1.3 / 1.12 × body is heading 1 / 2 / 3. Images
(`extract_images`, PNG, on-page size) are placed before the first paragraph below their top. DOCX: minimal OOXML
(`[Content_Types].xml`, `_rels/.rels`, `word/document.xml`, `word/styles.xml` with Heading1–3 and 맑은 고딕,
`word/_rels/document.xml.rels`, `word/media/imageN.png`, pages separated by page breaks). HWPX: OWPML — `mimetype`
(stored, first), `version.xml`, `META-INF/container.xml` + `manifest.xml`, `Contents/content.hpf`,
`Contents/header.xml` (함초롬바탕, 본문 10 pt + 제목 18/15/13 pt bold, 바탕글), `Contents/section0.xml` (the first
paragraph carries the A4 `secPr`), `settings.xml`; **no images** in HWPX. HTML: one UTF-8 file, images as `data:`
URLs, `<hr>` between pages. Markdown: `#` headings, images written to `<name>_images/image-N.png` beside it, `---`
between pages. `done.outputs` lists every file written. No tables, columns, fonts or positions. Errors:
`permissionDenied` detail `extractText` (v0.3 integration, §7.11 S5; checked before the job starts and on every page).

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

**v0.3 pkg7-ocr** (O1, O2, O3, O5):

```ts
export interface OcrEngineLanguages { tesseract: string[]; vision: string[]; windows: string[] }
ocr_capabilities(): Promise<{ engines: OcrEngine[]; languages: string[]; engineLanguages: OcrEngineLanguages }>
export type OcrApplyPage = OcrPage | { page: PageIndex; ocr: OcrPage; setRotation?: Rotation };
ocr_recognize_native(a: { docId; page; dpi; languages: string[]; rotate?: Rotation }): Promise<OcrPage>
export interface OrientationScore { rotation: Rotation; confidence: number; words: number }
ocr_detect_orientation(a: { docId: DocId; page: PageIndex; languages: string[] }):
  Promise<{ rotation: Rotation; scores: OrientationScore[] }>          // macOS Vision only
```

* `engineLanguages` (O1) lists, per engine, the app language codes it reads — `kor`, `eng`, `jpn`, `chi_sim`
  (tesseract's spelling, also `Settings.ocrLanguages`'). Tesseract is always `kor` + `eng` from the backend; the
  frontend adds what `public/ocr/tessdata/languages.json` (written by `prepare-ocr --langs`) says is staged. Vision
  lists what `supportedRecognitionLanguages` has (`ja-JP` → `jpn`, `zh-Hans` → `chi_sim`); an engine not in
  `engines` has `[]`. `languages` stays the tesseract baseline.
* `windows` (O3) is listed where `Windows.Media.Ocr` has a recogniser for one of those languages
  (`AvailableRecognizerLanguages`); `ocr_recognize_native` then runs it on a blocking thread (the first requested
  language that is installed; Korean first for `kor+eng`), maps lines and word rectangles through Vision's
  normaliser (so the `OcrPage` is the same shape), gives every word confidence 90 (Windows reports none), merges
  adjacent CJK single-character words, and reads pages larger than `MaxImageDimension` at an integer fraction.
* `rotate` (O2, default 0) turns the rendered image clockwise before it is read; the result's `rotation` is
  `(page /Rotate + rotate) % 360`. `ocr_detect_orientation` renders the page at 100 DPI and answers the clockwise
  turn that makes it upright (0 = leave it): every line Vision finds votes with its reading direction (the
  `bottomLeft → bottomRight` angle of its quadrilateral), weighted by characters; a turn wins only with ≥ 3 words
  and 1.15 × the runner-up and the page as it is. Vision reads sideways text as confidently as upright text, so
  confidence cannot decide this — elsewhere the command answers `unsupported` and the frontend reads the 100 DPI
  `/ocr` image four ways with tesseract (the same pick, on mean word confidence).
* `setRotation` (O2) on the wrapped form sets that page's `/Rotate` **inside the same `mutate`** as its layer (one
  undo step, `structure: true` when a page actually turns); `ocr.rotation` must equal it, else `invalidArgument`.
* The layer (O5) writes every non-Latin-1 word in a **glyphless CID font** loaded once per document with
  `FPDFText_LoadCidType2Font` (~0.9 KB TrueType of three box glyphs, CID = BMP code point, identity `/ToUnicode`,
  `/CIDToGIDMap` → ½ em / 1 em) and reused by every later `ocr_apply`: the file grows by ~4 KB for the font
  where the Hangul subset cost ~260 KB, and any script (日本語, 中文) is written — the subset covered KS X 1001
  only. Latin-1 words stay base-14 Helvetica.

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

#### v0.3 additions (pkg2, P5)

```ts
export interface LinkBorder { width: number; color: Rgb }   // width 0 = no border
create_link(a: { docId; page; rect; target; quads?: Rect[]; border?: LinkBorder }): Promise<AnnotResult>
update_link(a: { docId; page; id; rect?; target?; border?: LinkBorder }): Promise<AnnotResult>
```

* `quads` — a link made from a text selection, one rectangle per line: written as `/QuadPoints` (8 numbers per
  quad, top-left, top-right, bottom-left, bottom-right) and the click area `/Rect` is their union (`rect` is then
  ignored); read back as `Annot.quads`. Works for web links (PDFium) and page links (lopdf).
* `border` — `/Border [0 0 w]` and `/C` (0…1 per channel); `width` 0…12 pt, `0` writes `/Border [0 0 0]` and
  drops `/C`. Absent on `create_link` = invisible (as before); absent on `update_link` = unchanged; a border-only
  `update_link` is valid (it no longer needs a rect or a target). Read back as `Annot.borderWidth` / `color`.
  A visible border is also written as a small `/AP /N` (a `w`-wide stroke in `/C` inside the click area, one box
  per quad) because PDFium — SeePDF's own view — draws no `/Border` of a link without an appearance; the stream is
  rebuilt when a bordered link moves and removed with `width: 0` (verification round 1).
* `set_outline` (v0.3) keeps what the contract cannot carry for **untouched nodes**: the old items are walked
  in document order beside PDFium's reading of them, and a new node whose title, page, view and url equal an old
  item's gets that item's original target object back (a named `/Dest`, a GoTo `/A` with a named `/D`, or any
  other action — Launch, JavaScript, GoToR) and its `/C` / `/F`. A renamed or re-targeted node is written fresh
  (explicit `/Dest`, no colour). When the two walks disagree (a malformed tree) nothing is kept.

---

### 7.11 Security and save integrity (v0.3, pkg3)

```ts
export interface SignatureInfo { fieldName?: string; reason?: string; time?: string; subFilter?: string }
export interface SanitizeOptions { javascript: boolean; attachments: boolean; actions: boolean; metadata: boolean; hiddenLayers: boolean }
export type SanitizeCounts = { [K in keyof SanitizeOptions]: number };
export interface SanitizeResult { removed: SanitizeCounts; info: DocInfo }
export interface AttachmentInfo { index: number; name: string; size: number }

unlock_document(a: { docId: DocId; password: string }): Promise<DocInfo>                    // S5
sanitize_document(a: { docId: DocId; options?: Partial<SanitizeOptions> }): Promise<SanitizeResult>   // S3
list_attachments(a: { docId: DocId }): Promise<AttachmentInfo[]>                           // S4
save_attachment(a: { docId: DocId; index: number; path: string }): Promise<{ bytes: number }>
add_attachment(a: { docId: DocId; path: string; name?: string }): Promise<AttachmentInfo[]>
delete_attachment(a: { docId: DocId; index: number }): Promise<AttachmentInfo[]>
focus_document_window(a: { path: string }): Promise<string | null>                         // H8
backup_folder(): Promise<string>                                                           // U2
```

**S1 — digital signatures.** `DocInfo.signatures` lists every top-level `/FT /Sig` field that carries a
signature (`FPDF_GetSignatureCount` / `FPDFSignatureObj_*`; an empty signature field is not listed), with the
field's `/T` (read with lopdf, best effort), `/Reason`, `/M` (raw `D:` string) and `/SubFilter`. Nothing is
validated. The engine keeps a signed file's bytes (`OpenDoc::incremental_base`) and whether the in-memory
document still derives from them by PDFium edits alone (`pristine`: any lopdf rewrite clears it). The undo /
redo snapshots of a pristine signed document are written with `FPDF_INCREMENTAL` (`OpenDoc::snapshot_bytes`),
so they start with the signed bytes and an undo / redo / rollback to one keeps (or restores) `pristine` —
including the first edit after `list_annotations` stamped `/NM` ids.
`incrementalSave` reports it. `save_document` / `save_document_as` of a signed, pristine document use
`FPDF_INCREMENTAL`: the output **starts with the signed file byte for byte** (checked; a full rewrite is the
fallback) and the saved file becomes the new base. Anything else is a full rewrite, which the UI announces
first (the signatures become invalid).

**S5 — permission flags.** Every mutation passes `security::ensure_permitted` inside `registry::mutate` /
`mutate_bytes`, before any undo entry is pushed; the permission comes from the edit itself
(`security::perm_for`): `undo.annot*` → annotate, `undo.form*` → fill forms, a structural or `pages` edit →
assemble, everything else → modify. `print_prepare` checks print and `export_text` checks extract-text. A
refusal is `permissionDenied` with `detail` = the permission (`print` / `modify` / `extractText` / `annotate` /
`fillForms` / `assemble`). `probe_text_edit` answers `reason: 'permissions'`. The flags are those of the
**current** open: an unencrypted document, or one opened with its owner password, grants everything.
`unlock_document` reopens the document in place with `password`, which must open it with every permission
(the owner password): same docId, path, generation, dirty state and undo / redo history (every snapshot opens
with the owner password too). Wrong password, or the open (user) password → `passwordWrong`; an unencrypted
document → `invalidArgument`. The view-only page route cannot tell printing from viewing, so the 인쇄 dialog's
print-only DOM is gated in the frontend (`runPrint`); the clipboard is gated in the frontend as well.
A file used as a page *source* — `merge_documents` inputs and `page_ops` `insertFrom` — must open with every
permission (unencrypted, or its owner password as the input's `password`), since its pages land in a document
without its restrictions: otherwise `permissionDenied`, detail `security`, and nothing changes.

**S2 — lopdf rewrites of an encrypted document** (metadata, outline, page labels, page links, replies,
sanitize). `registry::mutate_bytes` hands the closure PDFium's **decrypted** serialisation
(`FPDF_REMOVE_SECURITY`), then re-encrypts its output with the document's own security-handler state, decoded
by lopdf from the encrypted serialisation (same `/O /U /OE /UE /Perms /P /V /R`, crypt filters, `/ID` and file
key — RC4 40/128, AES-128, AES-256), and the reopened result must report the same revision and permissions
(else `verifyFailed`). No owner password is needed. The derived file key is checked against PDFium's own
decryption on a few streams before anything is written; when it cannot be derived (a non-standard handler, or
an R2–R4 file opened with its owner password while it also has an open password) → `unsupported` with that
reason. `get_page_labels` reads an encrypted document decrypted. `compress_apply` returns bytes PDFium already
encrypted and skips the round trip.

**S3 — `sanitize_document`** (문서 정리): one undo step `undo.sanitize`, or none when a dry run finds nothing
(the result then carries the unchanged `DocInfo`). `javascript`: `/Names /JavaScript` and every JavaScript
action (`/S /JavaScript` or a `/JS` entry) in `/A`, `/OpenAction`, `/Next` and — when `actions` is off — the
JavaScript entries of `/AA`. `attachments`: `/Names /EmbeddedFiles`, the catalog `/AF` and every
`FileAttachment` annotation with its popup. `actions`: an action `/OpenAction` (a plain destination is kept)
and every `/AA`. `metadata`: trailer `/Info`, every `/Metadata` and `/PieceInfo`. `hiddenLayers`: optional
content off in the default configuration (an OCG in `/D`, or an OCMD whose `/P` or `/VE` evaluates hidden) —
its marked-content sections and `/OC`-tagged XObject draws are deleted from the page content, from every Form
XObject the page draws (recursively, inheriting the caller's resources when a form has none) and from
annotation appearance streams, annotations tagged with it are dropped from `/Annots`, then `/OCProperties` is
removed; if anything a hidden layer draws could not be rewritten (unparsable content, a tiling pattern or Type 3
font using it, an undecidable OCMD) the layers stay hidden (count 0). Unreferenced objects are pruned. Counts are entries removed per category.

**S4 — attachments** go through PDFium (`FPDFDoc_*Attachment*`): `add_attachment` (name defaults to the file
name; a taken name gets ` (2)` before the extension; ≤ 256 MiB) and `delete_attachment` are one undo step each
(`undo.attachmentAdd` / `undo.attachmentDelete`, modify permission); `index` is PDFium's list order (name-tree
key order) and valid for the generation it was listed in; unknown index → `notFound`.

**H8 — one window per file / file changed on disk.** `focus_document_window` canonicalises `path`, finds the
window bound (`window_bind_document`) to a document with that path, un-minimises, shows and focuses it, and
returns its label (`null` when none). `openPath` calls it first and opens nothing when another window answers.
The engine records the file's size and modification time at open and after every save; `save_document` over the
document's own file compares them first and answers `fileChangedOnDisk` on a mismatch (nothing written);
`force: true` overwrites and re-records. `save_document_as` never checks — not even onto the document's own
file, which the user picked and confirmed (Replace?) in the save panel — and re-records the file it wrote.

**U2 — backups.** `save_document` / `save_document_as` write a backup of the file being replaced only when
`Settings.backupsEnabled` is on, under `<app data>/backups/<stem>/<millis>-<name>` (the newest three kept;
the temp directory only without an app). `backup_folder` returns that folder (created if missing).

### 7.12 v0.3 pkg4-annotations-stamps-objects — additions to §7.1, §7.4, §7.4a, §7.7a, §11

> v0.3 integration (with pkg3 S1): on a **signed file that still saves incrementally** (`DocInfo.incrementalSave:
> true`) the optional lopdf steps below are skipped like on an encrypted one — a line / arrow is created as the Ink
> line, a dashed square / circle solid, and the paragraph-flow / page-resize geometry pass does not run — so the
> save keeps the signatures. A polygon, polyline or callout is still written there (it has no PDFium form), and the
> file then saves as a full rewrite.

```ts
// §7.1 — kinds, fields and specs
export type AnnotKind = /* … §7.1 … */ | 'polygon' | 'polyline' | 'callout';
export type MeasureUnit = 'mm' | 'pt';
export type StampShape = 'rect' | 'round' | 'none';
export interface Annot {
  /* … §7.1 … */
  align?: 'left' | 'center' | 'right';   // textbox / callout (the `SeePDFQ` mirror, or a foreign FreeText's /Q)
  heads?: [start: boolean, end: boolean];   // line / arrow
  vertices?: number[];                   // polygon / polyline (/Vertices)
  cloudy?: boolean;                      // polygon with /BE << /S /C >>
  callout?: number[];                    // callout: /CL, 4 or 6 numbers, tip first; `rect` is then the TEXT BOX
  dashed?: boolean;                      // /BS << /S /D >>
  measure?: MeasureUnit;                 // line / polygon / polyline drawn with a length / area label
}
export type AnnotSpec = /* … §7.1 … */
  | { kind: 'square' | 'circle'; /* … */ dashed?: boolean }   // v0.3 pkg4: a copy of a dashed shape stays dashed
  | { kind: 'line' | 'arrow'; /* … */ measure?: MeasureUnit; dashed?: boolean }
  | { kind: 'stamp'; rect: Rect; image: { path: string } | { builtin: string }
      | { text: string; color: Rgb; shape?: StampShape };   // text stamp: {{date}} / {{author}} expand at placement
      rotate?: number; signature?: boolean }
  | { kind: 'polygon' | 'polyline'; vertices: number[]; color: Rgb; fillColor: Rgb | null; width: number;
      opacity: number; cloudy?: boolean; dashed?: boolean; measure?: MeasureUnit }
  | { kind: 'callout'; rect: Rect; text: string; fontSize: number; color: Rgb;
      align: 'left' | 'center' | 'right'; fillColor: Rgb | null; callout: number[] };
    // builtin (v0.3 T2) also 'check' | 'cross' | 'dot' — quick marks ✓ ✗ ●, drawn as vector paths
export interface AnnotPatch {
  /* … §7.1 … */
  align?: 'left' | 'center' | 'right'; heads?: [boolean, boolean]; printed?: boolean;
  dashed?: boolean; vertices?: number[]; callout?: number[];
}

// §7.4 — objects
restack_objects(a: { docId: DocId; page: PageIndex; objectIds: ObjectId[]; expectGeneration: DocGeneration;
  toFront: boolean }): Promise<{ objects: PageObject[]; docGeneration: DocGeneration }>

// §7.4a — page stamps
export type StampSource = /* … */ | { kind: 'background'; color: Rgb };
export interface StampSpec { /* … */ behind?: boolean }

// §11 — app, settings
export interface Settings { /* … */ stamps: CustomStamp[] }       // 내 도장, ≤ 30, newest last; absent → []
export type CustomStamp =
  | { kind: 'image'; id: string; path: string; aspect: number; createdAt: string }
  | { kind: 'text'; id: string; text: string; color: Rgb; shape: StampShape; createdAt: string };
export type SavedSignature = /* … */ | { kind: 'image'; id: string; path: string; aspect: number; createdAt: string };
image_preview(a: { path: string; maxPx?: number }): Promise<ArrayBuffer>   // u32 LE width, u32 LE height, PNG
copy_library_image(a: { path: string; library: 'stamp' | 'signature' }): Promise<{ path: string; width: number; height: number }>
remove_library_image(a: { path: string; library: 'stamp' | 'signature' }): Promise<boolean>
```

**Author (A9).** `create_annotation` reads 설정 ▸ 주석 작성자 on the **command side**
(`app::store::get_settings`, never from the webview) and writes it as `/T` on every new annotation, stamp and
signature (trimmed; blank = no `/T`). On the first run of a settings file with an empty author the app fills it
once from the OS user name (`USERNAME` / `USER`, else `whoami`; `DOMAIN\user` keeps the user); a store flag
`authorPrefilled` records it, so an author the user cleared on purpose is never filled in again.

**Alignment, heads, 인쇄 (A1, A10).** A text box stores its alignment in the private `SeePDFQ` key ("0" / "1" /
"2"): `/Q` is an integer PDFium cannot write, and the text box is a Stamp, where `/Q` means nothing to other
viewers. `update_annotation` rebuilds a text box with `patch.align`, then the previous alignment, then left (so a
colour edit keeps a centred box centred) and a line / arrow with `patch.heads` (any head → `/Subj
SeePDF:Arrow`, none → `SeePDF:Line`); the heads are mirrored in `SeePDFHeads` ("0 1"). `printed` sets or clears
the `/F` Print bit; a rebuild (text box, markup rects) keeps the old Print / Locked flags.

**lopdf kinds (A2).** A line / arrow is now a real **`/Line`** (`/L`, `/LE [/None|/OpenArrow …]`), and
`/Polygon` (`/Vertices`, `/IC`, `/BE << /S /C /I 1 >>` for 구름), `/PolyLine` and a text box with a leader
(`FreeText /IT /FreeTextCallout /CL /RD /LE /OpenArrow`) exist — written with lopdf through
`registry::mutate_bytes` (`engine/annot/lopdf_annots.rs`): one undo step, PDFium reopens the result before it
replaces the document. PDFium cannot create these subtypes or write their geometry and never generates their
appearance, so each carries **its own `/AP`**: a Form XObject with `/BBox` = `/Rect`, an `ExtGState` for the
opacity, the border as `/BS << /W /S /D >>` (dash `[4 3]`), and — with `measure` — a Helvetica (WinAnsi) label
("76.2 mm", "354.6 pt", "645.2 mm²"; `/IT /LineDimension` / `/PolygonDimension` / `/PolyLineDimension`).
PDFium reads `/L` and `/Vertices` back itself; what it cannot read is mirrored like the colours: `SeePDFHeads`,
`SeePDFDash`, `SeePDFCloud`, `SeePDFMeasure`, `SeePDFCL` and `SeePDFBox` (a callout's text box, reported as its
`rect`). A callout is the text box built by PDFium (its Hangul glyphs need the bundled font), then converted by
lopdf in a **coalesced** second step (`History::refresh_last` + `MutateOpts::coalesced`, one undo entry however
long the first step took); the leader is appended to its appearance and `/BBox` grows to the new `/Rect`.
`update_annotation` on a SeePDF lopdf annotation redraws it **in place** (same object number, so a reply's
`/IRT` and a popup's `/Parent` still resolve; `/NM`, `/T`, `/CreationDate` kept) from the previous state + the
patch; a callout is rebuilt and converted again (one step); another application's line / polygon (moveOnly) is
moved with its appearance kept and `/Rect`, `/L`, `/Vertices`, `/CL` mapped. An **encrypted** document cannot
take a lopdf rewrite: a line / arrow falls back to SeePDF's Ink line there, a polygon / polyline / callout and an
edit of a lopdf annotation are `unsupported`.

**Dashed border (A6).** `patch.dashed` (or a width change on a dashed annotation) on a square / circle / ink:
the PDFium part first, then a coalesced lopdf step writes `/BS << /W w /S /D /D [4 3] >>` (or `/S /S`), mirrors
`SeePDFDash` and drops the `/AP`, which PDFium regenerates dashed on the next render.
`dashed: true` in a square / circle **spec** (복제 / 붙여넣기 of a dashed shape) takes the same coalesced lopdf
step right after the create — one undo entry; a line / arrow spec's `dashed` goes into its lopdf `/Line`. An
encrypted document draws them solid (no lopdf rewrite).

**Stamps (T1, T2).** A picked image is letterboxed into the stamp rect when their aspects differ (the UI sizes
the rect to the image, so this is a safety net). `{ text, color, shape }` is a text stamp: `{{date}}`
(local `yyyy.MM.dd`) and `{{author}}` (설정 ▸ 작성자) expand at placement, the label is drawn by the built-in
stamp generator (bundled Hangul subset, coverage-checked → `fontCoverage`) inside a square / rounded / no border,
`/Subj SeePDF:TextStamp` (= `stampKind`), `/Contents` = the expanded text. `check` / `cross` / `dot` are quick
marks drawn as one vector path (no font), `/Subj` = the id.

**`image_preview`** (T1): decodes a PNG / JPEG on `spawn_blocking` (never the engine thread) and returns its own
pixel size and a PNG thumbnail (longest side ≤ `maxPx`, default 512, clamped 16…1024) as raw bytes; the webview
turns the PNG into a `blob:` URL (`img-src blob:` is already allowed). No asset protocol, no fs scope.
Errors: `io`, `invalidArgument` (not an image). **`copy_library_image`** (T2 / T3): copies a picked PNG / JPEG
(≤ 20 MB) to `$APPDATA/SeePDF/stamps/` (내 도장) or `…/signatures/` (저장된 서명) as `img-<fnv64>.<ext>` —
content-addressed, never pruned (unlike the `sig-*.png` typed-signature cache) — and returns the copy's path and
size. **`remove_library_image`** deletes such a copy when its entry is removed; anything that is not an `img-*`
file directly inside that directory is left alone (`false`). `Settings.stamps` and the new `SavedSignature` kind
are read leniently like `signatures` (a malformed entry is dropped on its own; `stamps` capped at 30).

**Objects (A6).** `restack_objects` moves `objectIds` to the end (`toFront`) or the start of the page's paint
order with `FPDFPage_RemoveObject` + `FPDFPage_InsertObjectAtIndex` (nothing is copied), keeping their relative
order; one undo step `undo.objectArrange`; `stale` / `notFound` as for the other object commands.
`replace_image` (Stage 2) is now reached from 편집 ▸ 이미지 바꾸기 (E1).

**Page stamps (T4).** `behind: true` moves what the stamp adds to the **start** of each page's content (index 0 —
painted first, under the text; the text stays extractable). `{ kind: 'background', color }` fills each page's crop
box with a solid rectangle at index 0 (anchor, margin and rotation do not apply; opacity does), marked like every
stamp (`SeePDF:Stamp`, role), so `remove_stamps` takes it away.

**Page resize (A8).** After `resize_pages`' PDFium transform, a coalesced lopdf pass maps `/L`, `/Vertices`, `/CL`
(and SeePDF's `SeePDFCL` / `SeePDFBox`) through the same matrix as `/Rect` on the pages that have Line, Polygon,
PolyLine or FreeText annotations — still one undo step. Popups were already mapped. The paragraph flow
(`edit_paragraph`) moves a moved annotation's `/Popup` with it, and translates the `/L` / `/Vertices` / `/CL` (and
mirrors) of the moved Line, Polygon, PolyLine and callout annotations by the same `dy` in a coalesced lopdf pass —
still one `undo.paragraphEdit` step. On an encrypted document neither pass runs (only the appearance moves).

**`annotation_batch`** (A3 / A4) `{ docId, page, ops: AnnotOp[] } → { list: AnnotList, created: AnnotId[] }`, where
`AnnotOp = { op: 'create', spec, id? } | { op: 'update', id, patch } | { op: 'delete', ids }`. The ops run in order
(each like its single command; creates carry 작성자) and fold into **one** undo step — `undo.annotCreate` when all
create, `undo.annotDelete` when all delete, else `undo.annotEdit`. All or nothing: when an op fails, the earlier ones
are undone (no undo or redo entry is left) and its error is returned. Used by the partial eraser (one scrub's patches
and deletes) and by a pen stroke whose pressure is baked into several ink annotations. `created` lists the `/NM` of
each create op, in order. The batch is folded after every op and depth trimming waits until it ends, so this
holds for any number of ops and on a large document (undo depth 3) too.

**Annotation summary (A7).** Rows are in **thread order**: each top-level annotation followed by its replies
(depth-first, each level oldest first). CSV gains two columns at the end, `ID` (the `/NM`) and `답글 대상` /
`Reply to` (the parent's id for a reply, empty otherwise); TXT indents a reply under its parent (`↳`, four
spaces per level), Markdown nests it as a sub-item.

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
// v0.3 pkg5
'engine-crashed'   { docIds: DocId[]; label: string }   // H4: a panicking command closed these; their windows reopen them
'theme-changed'    { theme: 'system' | 'light' | 'dark' } // U4: emitted by the window that changed 테마; every window follows
// v0.3 pkg2 (P3), emitted by the frontend to every window: pages released outside their window
'pages-drop'       { srcDocId: DocId; pages: PageIndex[]; from: string /* window label */; screen: { x: number; y: number } }
```

v0.3 pkg2: the webview's native drag-drop event now also feeds `onFileDragState` (`over` / `leave` / `drop`,
the Welcome highlight — H7) and `onFileDrop` carries the drop `position` in CSS pixels (the 페이지 grid inserts
there — P2).

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

Channels are used by `search_start` (own event type), `export_images`, `export_flattened`, (v0.3) `export_embedded_images`, `export_stitched_image`, `export_tiff`, `export_text_flow`,
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
| `/page` | `doc, gen, page, sk, rot[, night][, hl][, forms][, vn][, print]` | `image/png`, whole page — also the placeholder (`sk` small) |
| `/thumb` | `doc, gen, page, w[, rot]` | `image/png`, `set_target_width(w).set_maximum_height(w*2)` |
| `/ocr` | `doc, gen, page, dpi` | `image/png` gray8 at `dpi` (300 default) |
| `/recent-thumb` | `id` | `image/png` from `$APPDATA/SeePDF/thumbs/<id>.png` (read on the io thread) |
| `/raw` | `doc` | `application/pdf`, honours `Range:` → 206 |

* `sk` = `scaleKey` = `round(zoomPercent × devicePixelRatio)`; device scale `s = sk / 100`.
* **v0.3 pkg8 (X7)** `/page?…&print=all|none|stamps` is the **print variant** (the print-only DOM's images): rendered
  with `FPDF_PRINTING`, so annotation Print / NoView flags are honoured as on paper; `none` drops `FPDF_ANNOT`
  (form widgets still drawn); `stamps` hides every annotation but Stamp / SeePDF signature / widget for that one
  render (the Hidden flags are restored before the command ends). Its own cache entry: the ETag's kind is `print`,
  `print-none` or `print-stamps`. Scheduled on `Lane::Prefetch` in page order and never dropped as stale (`409`),
  since a printed sheet is never "off screen". Any other `print` value is `400`.
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

### 11b. Diagnostics, about, licences (v0.3 pkg5)

```ts
export interface AppInfo {
  version: string; os: string; arch: string; debug: boolean;
  pdfiumVersion: string; pdfiumDir: string; locale: 'ko' | 'en'; theme: 'system' | 'light' | 'dark';
}
export interface ProblemReport { text: string; path: string }
app_info(): Promise<AppInfo>                    // H1: Welcome's version, SeePDF 정보, 문제 보고
problem_report(): Promise<ProblemReport>        // H10: 도움말 › 문제 보고…
open_log_folder(): Promise<string>              // H10: 도움말 › 로그 폴더 열기 → the folder's path
third_party_notices(): Promise<string>          // H11: SeePDF 정보 › 오픈 소스 라이선스
```

`commands/app.rs` + `app/diagnostics.rs`; none of them touches PDFium (the file work runs on `spawn_blocking`).
* `app_info`: synchronous, cheap; loaded once by the `appStore` bootstrap.
* Logs: `run()` installs stderr plus a daily rolling file `seepdf.YYYY-MM-DD.log` in the app log directory
  (macOS `~/Library/Logs/com.seepdf.desktop/`, Windows `%LOCALAPPDATA%\com.seepdf.desktop\logs\`), the newest 7
  kept, and a panic hook that logs every panic. A setup failure shows a native message box (라이브러리를 불러올 수
  없습니다 — 재설치하거나 백신 예외를 추가하세요 + the error + the log folder) and exits with code 1.
* `problem_report`: `SeePDF <version>`, `OS: <os> / <arch>`, `PDFium: <version>`, the locale and the last 200 log
  lines (across the newest files); also written to `<log dir>/problem-report.txt`, returned as `path`. The
  frontend copies `text` to the clipboard and reveals `path`. Errors: `io`.
* `open_log_folder`: creates the log directory if needed and opens it in Finder / Explorer. Errors: `io`.
* `third_party_notices`: `<resources>/notices/THIRD_PARTY_NOTICES.txt` (generated by `scripts/gen-notices.mjs`
  before every release bundle); without it (a dev build) the bundled PDFium and font licences; neither →
  `notFound`.

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
| `ungroup_object` | v0.3 pkg1, `engine/objects/ungroup.rs` | F-17, F-22 |
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
| `make_nup`, `export_embedded_images`, `export_stitched_image`, `export_tiff`, `export_text_flow` (v0.3) | pkg8, `commands/export.rs` + `engine/export/{nup,pagejob,textflow}.rs` | X2, X3, X6 |
| `write_recovery`, `clear_recovery`, `list_recovery`, `discard_recovery` | Stage 5, `engine/recovery.rs` | P1-8 |
| `set_page_boxes`, `resize_pages` | P2, `engine/pages/boxes.rs` | P2 crop / resize |
| `export_annotation_summary` | P2, `engine/export/summary.rs` | P2 annotation summary |
| `tts_speak`, `tts_stop`, `tts_status` | P2, `app/tts.rs` | P2 read aloud |
| `set_outline`, `create_link`, `update_link`, `delete_link`, `set_page_labels`, `get_page_labels` | P2, `engine/structure/` + `commands/structure.rs` | P2 outline / links / labels |
| `reply_annotation` | P2, `engine/annot/reply.rs` (lopdf via `registry::mutate_bytes`) | P2 threads |
| `list_pdf_files` | P2, `commands/save.rs` | P2 multi-file search |
| `app_info`, `problem_report`, `open_log_folder`, `third_party_notices` | v0.3 pkg5, `commands/app.rs` + `app/diagnostics.rs` | H1, H10, H11 |
| `get_web_links`, `get_reading_order` | v0.3 pkg6, `engine/text/{weblinks,structtree}.rs` + `engine/raw/` | V5, V6 |
| `get_default_settings`, `clear_render_cache`, `save_snapshot_png` | v0.3 pkg6, `commands/app.rs` | U3, V1 |
| `unlock_document`, `sanitize_document`, `list_attachments`, `save_attachment`, `add_attachment`, `delete_attachment` | v0.3 pkg3, `engine/security.rs`, `engine/sanitize.rs`, `engine/attachments.rs` + `commands/security.rs` | S5, S3, S4 |
| `focus_document_window`, `backup_folder` (+ `save_document { force }`) | v0.3 pkg3, `app/windows.rs`, `commands/save.rs`, `engine/save/mod.rs` | H8, U2 |
| `create_from_images`, `write_temp_image` | v0.3 pkg2, `engine/pages/create.rs` + `commands/documents.rs` | D1 images → PDF |
| `import_pages_from_doc` (+ merge / insert carry-over, `split_document { byOutline }`) | v0.3 pkg2, `engine/pages/{mod,carry}.rs` | P1, P3, P4 |
| `create_form_field`, `update_form_field`, `delete_form_field`, `export_form_data`, `import_form_data`, `flatten_form` | v0.3 pkg2, `engine/form/{author,data,flatten,extras}.rs` + `commands/forms.rs` | F1, F2 |
| `image_preview`, `copy_library_image`, `remove_library_image` | v0.3 pkg4, `commands/images.rs` + `app/signatures.rs` | v0.3 T1 / T2 / T3 |
| `restack_objects` | v0.3 pkg4, `engine/objects/mod.rs` | v0.3 A6 |
| `annotation_batch` | v0.3 pkg4, `engine/annot/update.rs` (`batch_in`, `History::squash_since`) | v0.3 A3 / A4 |

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
