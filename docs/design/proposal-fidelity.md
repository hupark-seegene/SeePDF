# SeePDF v1 — Proposal "Fidelity": an editing-fidelity-first architecture

**Author:** proposal agent C (editing-fidelity angle) · **Date:** 2026-09-15 · **Status:** proposal for judging/synthesis
**Ground truth:** `docs/spikes/{render,text,annotations,pages,tauri,ocr,ux-research}.md` (compile-verified signatures and
measurements against pdfium-render 0.9.4 / PDFium chromium-8057 / tauri 2.11.5). Where this document goes beyond the
spikes it says so explicitly and names the verification test the implementing agent must add first.

---

## 0. Thesis and the five invariants

A PDF editor earns trust exactly once: the first time a user saves a contract, opens it in Acrobat or Preview, and sees
what they saw in our app. Every other feature is negotiable; that one is not. So this proposal organises the whole
system around **one document model whose edits are deterministic, replayable and verifiable**, and treats rendering,
UI and OCR as clients of that model.

Five invariants that every module must keep (they are restated as tests in §9):

| # | Invariant | How it is enforced |
|---|---|---|
| I1 | **The original file is never modified in place.** A save either atomically replaces it with a verified file or fails leaving it untouched. | `engine/save/atomic.rs`: temp file in the same directory → fsync → verify by reopening → backup → `rename`. No other code path opens the original for writing. |
| I2 | **The live document == base bytes + journal.** Every mutation is a serialisable `Command` applied on the engine thread; undo is a journal cursor, not an inverse operation. | `engine/journal.rs` owns the only `&mut` path to a `PdfDocument`. Commands have `apply` + `verify`; there is no `undo`. |
| I3 | **What pdfium renders is what is saved.** Annotations, form values and text edits are drawn by pdfium (with their persisted appearance streams), never by a parallel React model that could drift. SVG is used for gestures, selection and handles only. | Pre-flight render before save forces AP generation (§2.6 step 1); the viewer renders with `render_annotations(true)` + `render_form_data(true)`. |
| I4 | **Every edit type has an explicit editability boundary** that is computed, shown in the UI, and refused when not met — never a silent substitution. | `engine/edit/editability.rs` returns a `Capability` per object/annotation/page; the UI greys out what is not allowed and says why (i18n keys in `error.*`/`prop.*`). |
| I5 | **Redaction removes content or refuses.** Verification re-extracts text and images from the *saved* file. | `engine/edit/redact.rs` + `save/verify.rs` (§4.8). |

Everything conventional (rendering, transport, UI shell, OCR engines) follows the spike recommendations and is
specified fully but briefly; §2 and §4 go deep.

---

## 1. Process and thread architecture

### 1.1 Threads

| Thread | Owns | Talks to |
|---|---|---|
| `seepdf-engine` (std::thread, 16 MB stack) | `Pdfium` (leaked → `&'static`), every `PdfDocument`, `PdfPage` LRU, journals, caches, form-fill environments. **The only thread that calls pdfium.** | receives `Request`s over one `crossbeam_channel::unbounded`, answers via `Reply<T>` callbacks (the spike's `Reply` type, kept verbatim). |
| `seepdf-encode-{0,1}` (2 threads) | PNG/JPEG encoding of tiles and export pages | fed `EncodeJob { rgba: Vec<u8>, w, h, format, reply }` from the engine over a bounded(8) channel; answers the `UriSchemeResponder` or export writer directly. |
| `seepdf-ocr-mac` (macOS only) | `VNRecognizeTextRequest` (Vision uses GPU/ANE; must not sit on the engine thread) | receives BGRA page bitmaps from the engine, returns `OcrPage`. |
| `seepdf-io` (1 thread) | backups (file copy), recovery-journal fsync, lopdf fix-up passes, file hashing | engine hands it `IoJob`s; results come back as `Request::IoDone`. |
| tauri async runtime | `#[tauri::command]`s, protocol handlers (protocol handler runs on macOS main thread: parse URL, forward, return) | `EngineHandle` (Clone, Send+Sync) in managed state. |
| WebView JS | UI, tesseract.js worker pool (≤ 4 Web Workers) | `invoke`, `Channel`, `seepdf://` |

Why one pdfium thread and no `Mutex<Engine>`: the render spike proved `thread_safe` locks only 289/481 FFI functions
(`FPDF_RenderPageBitmapWithMatrix`, `FPDFText_*`, `FORM_On*`, many `FPDFAnnot_*` are unlocked) and that two threads give
0 % speed-up (58.9 vs 59.2 ms). The pages spike proved page mutation compiles with `&PdfDocument`, so the borrow
checker offers no single-writer guarantee either. The engine thread is the guarantee.

### 1.2 The pdfium access layer: a 40-line fork of pdfium-render (decision)

Editing needs ~25 FFI functions that pdfium-render 0.9.4 does not wrap, and all of them need a raw handle:
`FPDF_MovePages`, `FPDF_SaveAsCopy` with flags, `FPDFAnnot_SetAP(NULL)` (mandatory before recolouring — the crate's
`set_fill_color` **segfaults** on annotations with an AP), `FPDFAnnot_SetBorder/AddInkStroke/SetFlags/SetStringValue`
(`/DA`, `/Subj`), `FPDFPage_CreateAnnot(CIRCLE)`, `FPDFPage_Flatten(FLAT_NORMALDISPLAY)`, `FORM_SetFocusedAnnot`,
`FORM_SelectAllText`, `FORM_ReplaceSelection`, `FORM_OnLButtonDown/Up`, `FORM_SetIndexSelected`, `FORM_ForceToKillFocus`,
`FPDFAnnot_GetFormFieldValue`, `FPDFText_LoadCidType2Font`, `FPDFTextObj_GetFont` + `FPDFPageObj_CreateTextObj`
(redaction run splitting), `FPDFFormObj_RemoveObject`, `FPDFAnnot_GetLinkedAnnot`, `FPDFAnnot_GetNumberValue("CA")`,
`FPDFAnnot_GetInkListPath`, `FPDFAnnot_GetLine`, `FPDFBookmark_GetCount` (sign = open/closed).

Verified in the registry source (`~/.cargo/registry/src/*/pdfium-render-0.9.4`): `PdfDocument::handle()`
(`src/pdf/document.rs:206`), `PdfPage::page_handle()` / `document_handle()` (`page.rs:229`), `PdfForm::handle()`
(`form.rs:181`), annotation `handle()` (private trait `PdfPageAnnotationPrivate`, `annotation/private.rs:131`) and page
object `object_handle()` (`object/private.rs:44`) are all `pub(crate)`. The "bind twice" trick from the spikes gives raw
access only to a *second* document handle, which is useless for editing one document, and each bindings instance has its
own `thread_safe` mutex.

**Decision:** depend on a fork via `[patch.crates-io]` (GitHub is reachable; the mirror only replaces crates.io):

```toml
# src-tauri/Cargo.toml (implementation phase; Cargo.toml is frozen during the proposal phase)
[patch.crates-io]
pdfium-render = { git = "https://github.com/hupark-seegene/pdfium-render", rev = "<sha>" }   # branch seepdf/0.9.4-raw
```

The fork adds exactly one file, `src/raw_access.rs`, re-exported from the prelude, and nothing else changes:

```rust
// pdfium-render fork: src/raw_access.rs — extension traits, no behaviour change
pub trait RawDocumentHandle { fn raw_document_handle(&self) -> FPDF_DOCUMENT; }
impl RawDocumentHandle for PdfDocument<'_> { fn raw_document_handle(&self) -> FPDF_DOCUMENT { self.handle() } }
pub trait RawPageHandle { fn raw_page_handle(&self) -> FPDF_PAGE; }
impl RawPageHandle for PdfPage<'_> { fn raw_page_handle(&self) -> FPDF_PAGE { self.page_handle() } }
pub trait RawFormHandle { fn raw_form_handle(&self) -> FPDF_FORMHANDLE; }
impl RawFormHandle for PdfForm<'_> { fn raw_form_handle(&self) -> FPDF_FORMHANDLE { self.handle() } }
pub trait RawAnnotationHandle { fn raw_annotation_handle(&self) -> FPDF_ANNOTATION; }
impl RawAnnotationHandle for PdfPageAnnotation<'_> { fn raw_annotation_handle(&self) -> FPDF_ANNOTATION { self.handle() } }
pub trait RawObjectHandle { fn raw_object_handle(&self) -> FPDF_PAGEOBJECT; }
impl RawObjectHandle for PdfPageObject<'_> { fn raw_object_handle(&self) -> FPDF_PAGEOBJECT { self.object_handle() } }
pub trait RawFontHandle { fn raw_font_handle(&self) -> FPDF_FONT; }   // PdfFont, for FPDFPageObj_CreateTextObj
pub trait RawBindings { fn raw_bindings(&self) -> &dyn PdfiumLibraryBindings; }  // the SAME bindings instance (one mutex)
impl RawBindings for PdfDocument<'_> { fn raw_bindings(&self) -> &dyn PdfiumLibraryBindings { self.bindings() } }
```

Rules for raw use (enforced by code review + `engine/pdfium/raw.rs` being the only module that imports the traits):
every raw call goes through `engine::pdfium::raw::Raw<'a>` helpers with safe signatures (`set_ap_none(annot)`,
`set_flags(annot, AnnotFlags)`, `move_pages(doc, &[i32], dest)`, `save_as_copy(doc, &mut impl Write, SaveFlags)`,
`form_replace_text(form, page, annot, &str)` …). Constants live in `engine/pdfium/consts.rs`, copied from
`bindgen/pdfium_7881.rs` (the annotation spike already lists them): annotation subtypes `TEXT=1 LINK=2 FREETEXT=3
LINE=4 SQUARE=5 CIRCLE=6 POLYGON=7 POLYLINE=8 HIGHLIGHT=9 UNDERLINE=10 SQUIGGLY=11 STRIKEOUT=12 STAMP=13 CARET=14 INK=15
POPUP=16 FILEATTACHMENT=17 … WIDGET=20 … REDACT=28`; flags `HIDDEN=2 PRINT=4 NOVIEW=32 READONLY=64 LOCKED=128`;
`FPDF_ANNOT_APPEARANCEMODE_NORMAL=0`; colour types `Color=0 InteriorColor=1`; `FLAT_NORMALDISPLAY=0 FLAT_PRINT=1`;
save flags `FPDF_INCREMENTAL=1 FPDF_NO_INCREMENTAL=2 FPDF_REMOVE_SECURITY=4` (3 is the deprecated value);
form field types `PUSHBUTTON=1 CHECKBOX=2 RADIOBUTTON=3 COMBOBOX=4 LISTBOX=5 TEXTFIELD=6 SIGNATURE=7`. The first task of
the engine-core agent is `consts.rs` + a `#[test]` that compares each constant with the bindgen file.

Fallback if the fork is vetoed: keep `Pdfium::bind_to_library` twice and run **all** editing on raw handles with
pdfium-render used only for rendering/text of a *separate* read-only handle refreshed from `save_to_bytes()` after each
commit (adds 5 ms/MB per edit and two copies of every document). Strictly worse; listed for completeness only.

### 1.3 Engine protocol: queries, renders, commands, jobs

One channel, one priority heap, drop-stale semantics:

```rust
// src-tauri/src/engine/protocol.rs
pub enum Request {
    Query   { doc: DocId, q: Query, reply: Reply<Result<QueryResult, EngineError>> },     // read-only, cheap (< 10 ms)
    Render  { doc: DocId, spec: RenderSpec, gen: Generation, prio: RenderPrio, reply: Reply<Result<Rendered, EngineError>> },
    Command { doc: DocId, cmds: Vec<Command>, label: String, reply: Reply<Result<CommitInfo, EngineError>> }, // one undo step
    History { doc: DocId, op: HistoryOp /* Undo | Redo | Jump(usize) */, reply: Reply<Result<CommitInfo, EngineError>> },
    Job     { spec: JobSpec, events: tauri::ipc::Channel<JobEvent>, cancel: CancelToken, reply: Reply<Result<JobId, EngineError>> },
    JobStep { id: JobId },                                     // background continuation, re-enqueued by the engine itself
    IoDone  { id: IoJobId, result: IoResult },                 // from seepdf-io
    Open    { source: OpenSource, password: Option<String>, reply: Reply<Result<DocInfo, EngineError>> },
    Close   { doc: DocId, reply: Reply<()> },
    Shutdown,
}
pub enum RenderPrio { VisibleTile = 0, Placeholder = 1, Prefetch = 2, Thumbnail = 3, Export = 4 }
```

Loop (`engine/thread.rs`): `drain()` pulls everything currently queued with `try_recv`; `Render`s whose `gen` is older
than the newest generation seen for that `(doc, viewport)` are answered `Err(EngineError::Stale)` without touching
pdfium; the rest are ordered `Command/History < Query < Render(prio, centre-distance) < JobStep`; execute one; repeat;
`recv()` blocks only when empty. Jobs (search, OCR render, export, save-verify) are pre-chunked: each `JobStep` does at
most ~10 ms of pdfium work (one page) and re-enqueues itself, so tiles interleave with a 500-page export. Cancellation
= `CancelToken(Arc<AtomicBool>)` checked at every step; the frontend calls `job_cancel`.

### 1.4 Document registry and identity

```rust
// src-tauri/src/engine/registry.rs
pub struct DocId(pub u32);              // process-unique, never reused
pub struct Revision(pub u64);           // +1 on every commit, undo, redo, reload; part of every ETag and cache key
pub struct PageId(pub u64);             // stable across page moves/deletes; derived deterministically (see §2.3)
pub struct AnnotId(pub String);         // == /NM (uuid v4)

pub struct OpenDoc<'p> {
    pub id: DocId,
    pub origin: Origin,                       // File { path, len, mtime, hash: u64 } | Untitled | Import
    pub password: Option<String>,
    pub base: BaseBytes,                      // Arc<Vec<u8>> or on-disk path (> 256 MB); what replay starts from
    pub doc: PdfDocument<'p>,                 // == base + journal[..cursor]   (I2)
    pub form: Option<FormEnv>,                // raw FPDF_FORMHANDLE + per-page OnAfterLoadPage bookkeeping
    pub pages: PageTable,                     // Vec<PageId> in document order + PageId -> meta (size, rotation, label)
    pub journal: Journal,                     // Vec<Committed>, cursor, save boundary index
    pub checkpoints: CheckpointStore,         // (journal index -> bytes), budgeted
    pub page_lru: PageLru<'p>,                // ≤ 32 open PdfPage handles; MUST be cleared before structural commands
    pub text_layers: LruMap<PageId, Arc<TextLayer>>,      // ≤ 64 pages
    pub annot_index: HashMap<PageId, Vec<AnnotSummary>>,  // rebuilt lazily from pdfium after each commit touching the page
    pub intents: StructuralIntents,           // things pdfium cannot write; applied by the lopdf fix-up at save (§2.6)
    pub revision: Revision,
    pub dirty: bool,
    pub transient: TransientState,            // annotations hidden during a drag, focused field; never saved
}
```

Drop order is enforced by `impl Drop for OpenDoc` (page LRU, text layers, form env, then `doc`) — the render spike
showed `drop(doc)` with a live `PdfPage` compiles and is a UAF.

### 1.5 Memory budgets (hard, enforced)

| Budget | Value | Where |
|---|---|---|
| Raw RGBA render cache | 192 MiB (LRU by bytes, furthest-from-viewport first) | `engine/render/cache.rs` |
| Open `PdfPage` handles | 32 per document, 64 total | `PageLru` |
| Text layers | 64 pages/doc, ~200 KB each | `text_layers` |
| Checkpoints | min(2 × base size, 256 MiB) in RAM, then spill to `$CACHE/seepdf/ckpt/` | `CheckpointStore` |
| Largest single bitmap | full page ≤ 4× device scale (29.6 MiB); above that only 512² tiles | `RenderSpec::validate` |
| Base bytes kept after Save for undo-across-save | only if base ≤ 256 MiB, else undo stops at the save boundary | `Journal::rebase` |
| OCR workers (tesseract.js) | min(4, hardwareConcurrency − 2), ~110 MB each | `src/ocr/tesseractPool.ts` |
| Thumbnails (96 px, all pages) | ~2 MB / 500 pages, resident | `render/thumbs.rs` |

Idle RSS target for a 500-page doc: ≤ 400 MB (spike baseline: libpdfium + doc + 14 pages ≈ 40 MiB; the rest is the caches
above; macOS never returns freed bitmap memory, so the *largest single allocation* is what matters).

### 1.6 Rust module layout (`src-tauri/src/`)

```
lib.rs                      builder, plugins, protocol registration, invoke_handler (thin)
commands/                   #[tauri::command] wrappers only: doc.rs, history.rs, query.rs, jobs.rs, files.rs, settings.rs
protocol.rs                 seepdf:// routes: /tile /page /thumb /ocr-image /raw
model/                      serde types shared with TS (mirrored by hand in src/ipc/types.ts): geometry.rs, command.rs,
                            annot.rs, text.rs, page.rs, form.rs, ocr.rs, save.rs, events.rs
engine/
  mod.rs, thread.rs (loop), protocol.rs (Request), registry.rs, reply.rs (from spike), cancel.rs, errors.rs
  pdfium/  bind.rs (resolve + bind once), raw.rs (safe wrappers over raw FFI via the fork traits), consts.rs, filewrite.rs
  doc/     open.rs, identity.rs (PageId/AnnotId assignment), page_lru.rs, form_env.rs, info.rs (outline, metadata, perms)
  journal/ mod.rs (Journal, Committed, cursor), checkpoint.rs, replay.rs, recovery.rs (autosave jsonl + blobs)
  command/ mod.rs (Command enum, apply/verify dispatch), effects.rs, annot.rs, form.rs, page_ops.rs, objects.rs,
           text_edit.rs, image.rs, ocr_layer.rs, redact.rs, flatten.rs, intents.rs
  edit/    editability.rs, fonts.rs (bundled fonts, subsetting, coverage), layout.rs (line breaking), raster.rs
  text/    layer.rs (chars/words/lines), matrix.rs (page→device), search.rs, select.rs
  annot/   summary.rs, appearance.rs (create/edit pipelines), quads.rs, ghost.rs (transient hide)
  render/  spec.rs, tiles.rs, cache.rs, thumbs.rs, encode.rs (encoder pool), export.rs
  save/    pipeline.rs, writer.rs (streaming FPDF_FILEWRITE), atomic.rs, backup.rs, verify.rs, fixup.rs (lopdf), policy.rs
  ocr/     mod.rs (OcrPage), vision.rs (cfg macos), cidfont.rs (glyphless font + ToUnicode), apply.rs
```

The `engine` module has **no tauri dependency** except `tauri::ipc::Channel` in `protocol.rs` (feature-gated behind a
trait `EventSink` so `cargo test` can run the engine headless).

---

## 2. The document model: base + journal + verified save (the core of this proposal)

### 2.1 Four layers

```
   ┌────────────────────────────┐   immutable   ┌─────────────────────────────┐
   │ base bytes (file as opened │ ───────────▶  │ journal: Vec<Committed>      │  cursor ──▶ undo/redo position
   │ or as last saved)          │               │ [c0, c1, c2 | c3, c4]        │  save boundary index
   └────────────────────────────┘               └─────────────────────────────┘
                 │  load_pdf_from_byte_vec (0.15 ms, lazy)          │ apply(cmd) on the engine thread
                 ▼                                                  ▼
   ┌─────────────────────────────────────────────────────────────────────────┐
   │ live PdfDocument  ==  replay(base, journal[..cursor])    (invariant I2)  │
   └─────────────────────────────────────────────────────────────────────────┘
                 │ pdfium renders / text / annotation summaries
                 ▼
   ┌─────────────────────────────────────────────────────────────────────────┐
   │ projections: tiles, TextLayer, AnnotSummary[], FieldSummary[], PageTable │  keyed by (DocId, Revision, PageId)
   └─────────────────────────────────────────────────────────────────────────┘
```

pdfium has no undo and cannot resurrect a deleted page, a removed text object or a redacted glyph. Inverse operations
would have to snapshot exactly the state they cannot recover — so this design does not implement inverses at all.
**A command only knows how to apply itself and how to verify its post-condition.** Undo is "cursor − 1 and rebuild from
the nearest checkpoint" (§2.4). This is cheaper to implement for eight parallel agents (one code path per command,
no inverse bookkeeping), it is deterministic, and it gives crash recovery for free (§2.5).

### 2.2 `Command`: the contract every editing module implements

```rust
// src-tauri/src/model/command.rs — serde, versioned, stored in the recovery journal; mirrored in src/ipc/types.ts
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum Command {
    // ---- annotations (§4.1) ----
    AnnotCreate  { page: PageId, spec: AnnotSpec },                        // spec.id is the /NM uuid, chosen by the caller
    AnnotUpdate  { page: PageId, id: AnnotId, patch: AnnotPatch },        // rect, quads, ink, colour, opacity, width, contents, author…
    AnnotDelete  { page: PageId, ids: Vec<AnnotId> },
    // ---- forms (§4.2) ----
    FormSetValue { page: PageId, field: FieldRef, value: FieldValue },    // Text(String) | Checked(bool) | Selected(Vec<u32>)
    // ---- pages (§4.6) ----
    PageInsertBlank    { at: u32, size_pt: [f32; 2], id: PageId },
    PageInsertFromFile { at: u32, source: BlobRef, source_pages: Vec<u32>, ids: Vec<PageId> },   // BlobRef = content hash in the blob store
    PageDuplicate      { id: PageId, new_id: PageId },
    PageDelete         { ids: Vec<PageId> },
    PageMove           { ids: Vec<PageId>, before: Option<PageId> },       // None = end; contiguous or not
    PageRotate         { ids: Vec<PageId>, delta_deg: i32 },               // ±90, 180
    // ---- page objects (§4.4, §4.5) ----
    ObjectTransform  { page: PageId, obj: ObjectRef, matrix: [f32; 6] },   // post-multiplied, page space
    ObjectSetColor   { page: PageId, obj: ObjectRef, fill: Option<Rgba>, stroke: Option<Rgba> },
    ObjectDelete     { page: PageId, objs: Vec<ObjectRef> },
    TextSetContent   { page: PageId, obj: ObjectRef, text: String, strategy: TextEditStrategy },   // InPlace | ReplaceWithFont(BundledFont)
    TextSetSize      { page: PageId, obj: ObjectRef, size_pt: f32 },
    TextBoxCreate    { page: PageId, block: BlockId, spec: TextBoxSpec },   // one object per laid-out line
    TextBoxUpdate    { page: PageId, block: BlockId, spec: TextBoxSpec },   // = delete old objects + create new ones
    ImageInsert      { page: PageId, blob: BlobRef, rect: RectPt, id: ObjectTag },
    ImageReplace     { page: PageId, obj: ObjectRef, blob: BlobRef },
    // ---- OCR (§4.7) ----
    OcrApplyLayer    { pages: Vec<OcrPageLayer>, replace_existing: bool },
    // ---- redaction (§4.8) ----
    RedactApply      { page: PageId, regions: Vec<RectPt>, options: RedactOptions },
    // ---- document-level / intents (§2.6.3) ----
    MetadataSet      { fields: BTreeMap<MetaKey, Option<String>> },
    SecurityRemove,
    SecuritySet      { user_pw: Option<String>, owner_pw: String, perms: Permissions },
    Flatten          { pages: Option<Vec<PageId>>, annots: bool, forms: bool },
}
```

```rust
// src-tauri/src/engine/command/mod.rs
pub trait Apply {
    /// Mutates the live document. MUST be deterministic given (doc state, cmd, BlobStore). MUST NOT touch pdfium
    /// through any path other than `doc` and `raw`. Returns which projections are invalid now.
    fn apply(&self, doc: &mut OpenDoc<'_>, cx: &mut ApplyCx<'_>) -> Result<Effects, EngineError>;
    /// Post-condition on the live document, run after apply (debug: always; release: for Redact/Ocr/Text edits).
    fn verify(&self, doc: &OpenDoc<'_>) -> Result<(), EngineError>;
    /// Estimated replay cost, used by the checkpoint policy (§2.4).
    fn cost(&self) -> ReplayCost;        // Trivial (< 1 ms) | Page (≈ regenerate_content) | Heavy (OCR/raster/insert-from-file)
    /// Human label for the undo menu, i18n key + args.
    fn label(&self) -> (&'static str, serde_json::Value);
}
pub struct Effects {
    pub pages_content: Vec<PageId>,      // tiles + text layer + annot index invalid
    pub pages_annots: Vec<PageId>,       // tiles + annot index invalid (text layer still valid)
    pub structure: bool,                 // page table changed: everything invalid, page LRU cleared
    pub needs_ap_render: Vec<PageId>,    // pages whose annotations need one render before save (§2.6.1)
}
```

`ApplyCx` gives the command the raw wrappers, the blob store (image bytes, inserted PDFs), the bundled font table and a
deterministic id source (`IdSeed`: xxhash of `(doc.origin.hash, journal index)`) so that replay assigns the same ids.

**Transactions.** `Request::Command { cmds: Vec<Command>, label }` is one journal entry (`Committed { cmds, label,
effects, at }`) and one undo step: "delete 5 pages", "OCR 148 pages", "apply text box edit" (= delete + create).
If any `apply` fails, the engine rebuilds from the last checkpoint (same as undo), so a failed transaction leaves the
document exactly as before — there is no half-applied state.

### 2.3 Identity: PageId, AnnotId, ObjectRef

pdfium has no ids for pages or page objects, and only `/NM` for annotations. Replay needs stable references.

* **PageId.** On open, base pages get `PageId(hash(origin.hash, "page", index))`; inserted pages carry their id in the
  command (chosen by the frontend from the `IdSeed` it received with `DocInfo`, or by the engine when absent).
  `PageTable` maps `PageId ↔ current index` and is updated by every structural command; it is *derived* by replay, so it
  is identical after undo/reload. All IPC and all commands use `PageId`; only the render/protocol layer uses indices.
* **AnnotId = /NM.** On open, annotations without `/NM` receive one (`uuid-from(hash(origin.hash, page, index))`) via
  raw `FPDFAnnot_SetStringValue(annot, "NM", …)`. This is not a user edit (dirty stays false) and it is idempotent on
  replay; the value is written on the next save, which is spec-conformant (Acrobat does the same). Lookup: per-page
  `HashMap<AnnotId, usize>` rebuilt whenever `pages_annots` says so (enumeration is 1.6–5 ms/page).
* **ObjectRef** for page objects: `{ index: u32, fp: u64 }` where `fp = hash(object_type, matrix×1000 rounded, bounds×10
  rounded, text-for-text-objects)`. Resolution: `objects().get(index)`, check fingerprint; on mismatch, linear search by
  fingerprint; not unique → `EngineError::ObjectMoved` (the UI re-queries `page_objects` and retries). Because replay is
  deterministic, indices match on replay; the fingerprint only guards against stale frontends.
* **BlockId / ObjectTag** for text boxes and inserted images we created: tracked in `OpenDoc.blocks: HashMap<BlockId,
  Vec<ObjectRef>>`, rebuilt by replay (each `TextBoxCreate` records the refs it produced).

### 2.4 Undo/redo = journal cursor + checkpoints

```rust
pub struct Journal { entries: Vec<Committed>, cursor: usize, save_boundary: usize }
pub struct CheckpointStore { points: Vec<(usize /*journal index*/, CheckpointBytes)>, budget: usize }
enum CheckpointBytes { Ram(Arc<Vec<u8>>), Disk(PathBuf) }
```

* **Redo** = `apply(entries[cursor])`, `cursor += 1` (no rebuild).
* **Undo** = `cursor -= 1`, then `rebuild(cursor)`: pick the greatest checkpoint index `k ≤ cursor` (index 0 == base
  bytes), `doc = load_pdf_from_byte_vec(bytes.clone(), password)`, re-create `FormEnv`, re-assign identities, replay
  `entries[k..cursor]`. `pdfium::load` is lazy (0.15 ms for 1 MB; parse cost lands on first page access), so a rebuild is
  dominated by the replayed commands and the re-render of visible pages.
* **Checkpoint policy** (`journal/checkpoint.rs`): take one (`save_to_bytes()`, 5 ms/MB) after any `Heavy` command,
  after every 24 `Page`-cost commands, and never for `Trivial` runs (annotation edits replay at ~0.1 ms each). Budget as
  in §1.5; evict the checkpoint furthest from the cursor first; checkpoint 0 (base) is never evicted.
* **Measured expectations** (from spikes): 1 MB text PDF, 40 annotation edits since last checkpoint → undo ≈ 0.15 ms
  load + 40 × 0.1 ms + form init (≈ 1 ms) + 2 visible pages × 2.3 ms render ≈ **10 ms** (target ≤ 50 ms). 100 MB scan
  with an OCR checkpoint right before → load is lazy, so the same order of magnitude.
* **Frontend optimism.** For `AnnotUpdate` drags the SVG proxy already shows the result; undo of a *selection-visible*
  annotation edit re-shows the ghost from the frontend's last-known summary until fresh tiles arrive, so the perceived
  undo latency is one frame.

`HistoryState { entries: [{label, args}], cursor, saveBoundary }` is projected to the frontend after every commit for
the Edit menu ("실행 취소: 형광펜 추가").

### 2.5 Recovery journal (autosave that never touches the user's file)

Because every mutation is already a serialisable `Command`, crash recovery is an append-only log:

```
$APPDATA/SeePDF/recovery/<doc-key>/manifest.json   { origin: {path, len, mtime, hash}, password_protected: bool, created }
$APPDATA/SeePDF/recovery/<doc-key>/journal.jsonl   one Committed per line (cmds + label + timestamp)
$APPDATA/SeePDF/recovery/<doc-key>/blobs/<xxhash>  bytes referenced by BlobRef (inserted images/PDFs, OCR layers)
```

`doc-key = hex(hash(path)) + "-" + len + "-" + mtime`. The engine appends after each commit; `seepdf-io` fsyncs at most
every 2 s and on window blur; the directory is deleted after a successful Save/Save As or a clean close without dirty
state. On launch, `commands::files::list_recoverable()` returns manifests whose origin still matches (same len+mtime,
hash re-checked lazily) and the welcome screen offers "복구". Restore = open origin + replay journal (heavy commands
re-read their blobs). Passwords are never stored; a protected document asks for its password again before replay.

### 2.6 Save pipeline (`engine/save/pipeline.rs`)

```
 preflight ─▶ write tmp ─▶ fix-up (lopdf, only if intents) ─▶ fsync ─▶ verify (reopen) ─▶ backup original ─▶ atomic replace ─▶ rebase + reload ─▶ notify
```

**Step 1 — pre-flight (engine thread).** pdfium generates appearance streams for Highlight/Underline/StrikeOut/
Squiggly/Square/Circle/Text/FreeText-with-DA **only when a page is rendered with annotations** (annotation spike §3.2:
the un-rendered file has no `/AP`, i.e. it is invisible in Preview/pdf.js). So for every page in the union of
`Effects::needs_ap_render` since the last save, render once with `PdfRenderConfig::new().set_target_width(32)
.render_annotations(true).render_form_data(true)` (≈ 0.3 ms/page) and discard the bitmap. Then `FORM_ForceToKillFocus`
(commits any focused field), drop all `PdfPage` handles from the LRU (regeneration on drop under `AutomaticOnDrop` is
disabled — we use `Manual` everywhere and every command already regenerated what it touched).

**Step 2 — write (engine thread, streaming).** Raw `FPDF_SaveAsCopy(doc, &mut FileWriter, flags)` (or
`FPDF_SaveWithVersion(…, 17)` when `intents.min_version` demands it) with a `#[repr(C)] FileWriter { version: 1,
write_block: Some(cb), out: BufWriter<File> }` that streams into `tmp = dir.join(format!(".{stem}.seepdf-{pid}-{nonce}.tmp"))`
— never into a `Vec` (100 MB scans). Flags from `save/policy.rs`:

| Condition | Flags | Why |
|---|---|---|
| default | `FPDF_NO_INCREMENTAL` (= full rewrite, drops unreferenced objects) | clean file; 5 ms/MB measured |
| document contains a signature field or `/ByteRange` (scan of base bytes at open) | `FPDF_INCREMENTAL` | keeps the signed revision byte-identical; Acrobat shows "signed, then modified" instead of "invalid" |
| `intents.remove_security` | `FPDF_REMOVE_SECURITY` (=4) | verified: reopens without a password |
| user preference "원본 구조 유지" | `FPDF_INCREMENTAL` | 25× faster on huge files; file grows |

Incremental save with xref-stream originals is **unverified** (tracemonkey is classic xref): `save/tests/incremental_xref_stream.rs`
must pass on a generated fixture before the signed-document rule is enabled; until then signed documents fall back to
full rewrite with a warning (`error.save.signatureInvalidated`).

**Step 3 — fix-up (seepdf-io thread, lopdf 0.45).** Only when `intents` is non-empty. `lopdf::IncrementalDocument::load(tmp)`
(verified present in 0.45.0 `src/incremental_document.rs`; `opt_clone_object_to_new_document`, `save_to`) appends one
revision that pdfium cannot produce:

| Intent | What the fix-up writes |
|---|---|
| Line / Arrow annotations (P0!) — PDFium cannot create `/Line` (`FPDFPage_CreateAnnot` returns NULL) | The engine creates them as **Ink** with a real `/InkList` and a pdfium-generated AP that already includes the arrow head strokes; the fix-up rewrites `/Subtype /Ink` → `/Line`, adds `/L [x1 y1 x2 y2]`, `/LE [/None /OpenArrow]`, `/BS`, removes `/InkList`, keeps the AP. Acrobat then edits it as a line. Without lopdf the file still opens everywhere; it is just an ink stroke. |
| Sticky-note popups | adds a `/Popup` annotation dict with `/Parent` and links both ways (PDFium cannot link). Optional; Acrobat/Preview synthesise popups anyway. |
| `MetadataSet` | `/Info` dictionary (Title/Author/Subject/Keywords/Creator/Producer, dates) — no `FPDF_SetMetaText` exists |
| `SecuritySet` | `lopdf::Document::encrypt(&EncryptionState)` (AES-256, present in 0.45 `src/encryption/algorithms.rs`) — full lopdf save, not incremental |
| AcroForm after `PageInsertFromFile` of a form document into a non-form document | copies `/AcroForm` (`/Fields` filtered to imported widgets, `/DR`, `/DA`) — `FPDF_ImportPages` drops it (pages spike) |
| Page labels (P2) | `/PageLabels` |

If lopdf fails (parse error, unsupported encryption) the pass is skipped, the pdfium output is kept, and the result
carries `SaveWarning::FixupSkipped { intents }` which the UI shows ("선/화살표가 자유형 주석으로 저장되었습니다").
The lopdf pass on a 100 MB scan is expected to take ~1–3 s and double memory transiently (unmeasured — `save/tests/fixup_large.rs`
records it; if > 5 s the fix-up runs only for files ≤ 64 MB).

**Step 4 — fsync tmp.**

**Step 5 — verify (engine thread, chunked as a job on large files).** Open `tmp` as a *scratch* `PdfDocument` and check:
page count and every page size against `PageTable`; every page loads (`pages().get(i)`); annotation count per page and
every `AnnotId` we created present with `/AP` (raw `FPDFAnnot_GetAP` length > 0) and `/F & PRINT`; every
`FormSetValue` since base reads back via `FPDFAnnot_GetFormFieldValue`; every `RedactApply` since base: no char
`tight_bounds()` intersects its regions and no image object bounds intersect (re-extracted, not trusted from memory);
`OcrApplyLayer`: three sampled words per page found by `text().search`; document opens with the same password (or none
after `SecurityRemove`). Any failure → `SaveError::Verification(details)`, the original is untouched, `tmp` is moved to
`$LOGS/SeePDF/failed-save-<ts>.pdf` for diagnosis, and the UI offers Save As.

**Step 6 — backup (seepdf-io).** With `settings.backup.enabled` (default on): copy original →
`$APPDATA/SeePDF/backups/<doc-key>/<yyyymmdd-hhmmss>-<name>.pdf`, keep 5 per document and 500 MB total (oldest first),
skip with a notice above 200 MB. Also `settings.backup.sidecar` (default off) writes `<name>.bak.pdf` next to the file
for users who want it visible.

**Step 7 — atomic replace.** `fs::set_permissions(tmp, original.permissions())`; `std::fs::rename(tmp, original)`
(macOS `rename(2)` and Windows `MoveFileExW(MOVEFILE_REPLACE_EXISTING)` replace atomically on the same volume; the temp
lives in the same directory precisely so that `EXDEV` cannot happen); on Windows retry 5× over 500 ms on sharing
violations (antivirus/OneDrive); then fsync the directory on Unix. Extended attributes (Finder tags, quarantine) are
lost by the inode swap — `xattr` 1.6 on the mirror can copy them (P1, listed in §12).

**Step 8 — rebase + reload.** `journal.save_boundary = cursor`; if `base.len() ≤ 256 MiB` the old base bytes are kept
(undo across save stays possible), else the journal before the boundary is frozen; the new base = the saved file. Then
the live document is **reloaded from the saved file** (page LRU cleared, form env re-created, revision +1): after
"저장" the screen shows exactly the bytes on disk, including anything the fix-up changed (I3), and pdfium's internal
page-index cache is fresh.

**Step 9 — notify.** `doc:saved { docId, path, revision, warnings }`; dirty=false; recovery dir deleted; recent-files
entry updated with the current reading position.

**Save As** runs the same pipeline against the chosen path, then re-points `origin` (like Preview). **Export copies**
(flattened PDF, extracted pages, compressed) run steps 1–5 into the target path with a *scratch* document derived from
the live one (`save_to_bytes` → load → apply `Flatten`/`PageDelete` → save) and never rebase. **Dirty tracking:**
`dirty = journal.cursor != journal.save_boundary`; the window title shows `• name` on Windows and macOS (tauri 2.11.5
has no `set_document_edited`; a native dot via `objc2-app-kit` is P2). Closing/quitting with `dirty` shows the
저장/저장 안 함/취소 sheet; "저장 안 함" deletes the recovery journal.

### 2.7 What "regenerate" means and when it runs

Every page is opened with `set_content_regeneration_strategy(PdfPageContentRegenerationStrategy::Manual)`. The text
spike proved `set_text`, `set_render_mode`, `set_fill_color`, `set_unscaled_font_size` never regenerate even under the
default strategy, and the annotation spike proved the default regenerates the whole content stream on every annotation
create. So: page-object commands call `page.regenerate_content()` exactly once at the end of `apply`; annotation and
form commands never do (they do not touch the content stream). A page that was regenerated gets `pages_content`
effects (text layer rebuilt) because pdfium may change generated whitespace (5087 → 5112 chars after regenerate in the
spike).

---

## 3. Rendering pipeline and transport (conventional; follows the render + tauri spikes)

### 3.1 Render specification

```rust
pub struct RenderSpec { pub page: PageId, pub scale_key: ScaleKey, pub tile: Option<TileXY>, pub rotate: UserRotation,
                        pub flags: RenderFlags /* annots: bool, forms: bool, transient: TransientNonce */ }
pub struct ScaleKey(u16);  // ladder index: 0.25,0.33,0.5,0.67,0.75,1,1.25,1.5,2,3,4,6,8,12,16 (device px per pt)
```

* Device scale `s = zoom × devicePixelRatio`, snapped **up** to the ladder; the frontend CSS-scales the tile down. This
  keeps cache keys finite and makes pinch zoom a pure CSS transform until the gesture settles (120 ms debounce).
* `s ≤ 2` → one whole-page bitmap (≤ 2.3 ms). `s > 2` → 512×512 device-pixel tiles via `PdfRenderConfig::new()
  .scale_page_by_factor(s).set_origin(-tx, -ty).render_form_data(true).render_annotations(true)
  .set_reverse_byte_order(true).set_clear_color(PdfColor::WHITE)` into `PdfBitmap::empty(512, 512, PdfBitmapFormat::BGRA)`
  with `render_into_bitmap_with_config` (0.01–0.8 ms per tile, empty tiles free). **Never** the matrix/`clip()` path:
  it silently disables form-field drawing. Page pixel size is `round(points × s)` exactly as pdfium-render computes it.
* User rotation: `.rotate(PdfPageRenderRotation::DegreesN, true)`; intrinsic `/Rotate` is already applied.
* LCD text off, BGRA always, `use_print_quality(true)` only for export.
* Transient state: while an annotation is being dragged the engine sets its `HIDDEN` flag (`FPDFAnnot_SetFlags`) and
  renders the page with `transient` nonce ≠ 0; on drop the flag is restored and the nonce returns to 0 (§4.1.4).

### 3.2 Transport: `seepdf://` for pixels, binary IPC for data

| Route | Serves | Cache headers |
|---|---|---|
| `GET /tile?doc&rev&page&s&tx&ty&rot&t` | PNG (Fast + Sub, ≈0.2 ms/tile) or JPEG q85 for pages classified as scans (single image object covering ≥ 90 % of the crop box: 19× smaller) | `ETag: "<doc>:<rev>:<page>:<s>:<tx>:<ty>:<rot>:<t>"`, `Cache-Control: private, max-age=0, must-revalidate`; 304 only for WebKit-sent `If-None-Match` |
| `GET /page?doc&rev&page&s&rot` | whole-page PNG for `s ≤ 2` and placeholders (`s` = 1) | same |
| `GET /thumb?doc&rev&page&w` | `set_target_width(w).set_maximum_height(w)` (aspect-correct; **not** `thumbnail(n)`, which squashes to n×n and disables annotations) | same, plus a resident 96 px engine cache |
| `GET /ocr-image?doc&rev&page&dpi` | gray8 PNG at 300 DPI for the tesseract.js worker | no-store |
| `GET /raw?doc` | original bytes with `Range` (for "Finder에서 보기"-style features and debugging) | — |

`<img>` elements consume tiles (WebKit decodes off the JS thread, revalidates on its own, keeps a memory cache);
`ImageBitmap`/canvas is used only for the print path. Every request carries `rev` so a stale `<img>` can never show
pre-edit pixels, and the engine's own raw-RGBA LRU is keyed by the same tuple. Data that JS must process (text layers,
search hits, OCR images for Vision are Rust-only) goes over `tauri::ipc::Response::new(Vec<u8>)` (141 MB/s measured)
with a tiny binary header; never JSON/base64 for anything ≥ 100 KB.

Encoding runs on `seepdf-encode-*`: the engine copies `as_raw_bytes()` (0.16 ms for a full 2× page) and hands the
`UriSchemeResponder` along in the `EncodeJob`, so the engine thread never encodes and the main thread never blocks
(verified pattern: protocol handler on `main`, response from a worker).

### 3.3 Scheduling, placeholders, prefetch

The frontend sends `render_hint { docId, gen, visible: [{page, s, tiles[]}], near: [pages] }` on every scroll/zoom
settle (throttled to 60 Hz); the engine derives the priority order: visible tiles centre-out → 1× placeholder for pages
entering the viewport (≈1 ms, upscaled by CSS until tiles arrive) → ±1 page prefetch → thumbnails for the visible
sidebar rows. `gen` increments per hint; older renders are dropped before touching pdfium. Result: a 500-page fling
never renders a page that scrolled past.

### 3.4 Dark/night mode

Because pdfium paints annotations into the same bitmap (I3), night mode applies `filter: invert(1) hue-rotate(180deg)`
to the page bitmap **including** its annotations (hue is preserved, lightness flips: a yellow highlight stays yellowish).
The UX spec's "separate non-inverted annotation layer" is deliberately not implemented: pdfium has no
annotations-only render (`FPDF_FFLDraw` draws widgets and popups only) and a React-drawn annotation layer would violate
I3. Accepted trade-off, documented in the UX notes.

---

## 4. Editing features and their exact pdfium calls

### 4.1 Annotations

#### 4.1.1 Create pipeline (all on the engine thread, page in `Manual` regeneration)

```rust
// engine/annot/appearance.rs — one function per subtype; all share this skeleton
fn create(page: &mut PdfPage, raw: &Raw, doc: &PdfDocument, spec: &AnnotSpec) -> Result<AnnotId> {
    let mut a = match spec.kind { /* high-level constructor or raw FPDFPage_CreateAnnot for Circle */ };
    // 1. rect FIRST (AP BBox = Rect at creation for Ink/Stamp; objects outside are clipped)
    a.set_bounds(PdfRect::new_from_values(bottom, left, top, right))?;         // note the argument order
    // 2. colours with IDENTICAL alpha (every FPDFAnnot_SetColor rewrites /CA)
    a.set_stroke_color(PdfColor::new(r, g, b, alpha))?;                        // /C + /CA   (only valid BEFORE an AP exists)
    if let Some(ic) = spec.interior { a.set_fill_color(PdfColor::new(ic.r, ic.g, ic.b, alpha))?; }   // /IC
    // 3. geometry: quads TL,TR,BL,BR / ink list / objects (see table)
    // 4. flags + metadata
    raw.set_flags(h, AnnotFlags::PRINT)?;                                      // pdfium-render never sets /F 4; flatten would delete it
    raw.set_string(h, "NM", &spec.id.0)?; raw.set_string(h, "T", &author)?; raw.set_string(h, "Subj", subj)?;
    a.set_contents(&spec.contents)?;                                           // /Contents; /CreationDate, /M set by pdfium-render
    Ok(spec.id.clone())
}
```

| Type (UX name) | Construction | Geometry | Appearance source |
|---|---|---|---|
| 형광펜 / 밑줄 / 취소선 (Highlight/Underline/StrikeOut) | `annotations_mut().create_highlight_annotation()` etc. | one quad per selected *line* from the text layer, `PdfQuadPoints::new_from_values(l,t, r,t, l,b, r,b)` (**TL,TR,BL,BR**; `from_rect()` is the wrong order and renders a 1-px sliver); `/Rect` = union | pdfium AP at pre-flight render (multiply blend, `/CA` honoured) |
| 메모 (Text) | `create_text_annotation(text)` | 20×20 pt rect at click | pdfium "note" icon AP |
| 펜 (Ink) | `create_ink_annotation()` then raw `FPDFAnnot_AddInkStroke(h, points.as_ptr(), n)` per stroke + `FPDFAnnot_SetBorder(h, 0, 0, width)` | points in page space; rect = stroke bounds + width | pdfium AP (real `/InkList`, editable in Acrobat) |
| 사각형 (Square) | `create_square_annotation()` + `SetBorder(0,0,width)` | rect | pdfium AP |
| 타원 (Circle) | raw `FPDFPage_CreateAnnot(page, FPDF_ANNOT_CIRCLE)` (no high-level constructor in 0.9.4) + `SetRect/SetColor/SetBorder` | rect | pdfium AP (verified: interior colour, `/CA 0.5`, border 3) |
| 선 / 화살표 (Line/Arrow) | **Ink** with 1 stroke of 2 points (+2 strokes for the arrow head), `SetBorder(0,0,width)`, `intents.line_annots.push(LineIntent{ id, l: [x1,y1,x2,y2], le: [None, OpenArrow] })` | 2 points | pdfium ink AP; `/Subtype /Line` written by the save fix-up (§2.6.3). PDFium cannot create Line annotations. |
| 텍스트 상자 (FreeText) | `create_free_text_annotation(text)` + raw `FPDFAnnot_SetStringValue(h, "DA", "r g b rg /Helv size Tf")` **mandatory** (no DA → invisible after save) | rect | pdfium AP (border from `/C`, text in Helv). For Hangul contents use the Stamp route below (Helv has no Hangul). |
| 텍스트 상자 with Hangul, 도장 (Stamp), 서명 (Signature image) | `create_stamp_annotation()`; `set_bounds` **before** `objects_mut().add_text_object/add_image_object`; transform objects **before** adding (pdfium-render never calls `FPDFAnnot_UpdateObject`); text uses the bundled subset Hangul font (§4.4.4); `Subj` = "Stamp" / "Signature" | rect | AP created at `FPDFAnnot_AppendObject` time; portable (verified: 한글 rendered, persisted) |
| 링크 (URI, P2) | `create_link_annotation(uri)` + quads | quads | invisible by design. Go-to-page links: fix-up only (P2). |

Popups: PDFium cannot link `/Popup` ↔ `/Parent`, and unlinked popups are never drawn — note contents are shown by the
React popover from `contents()/creator()/modification_date()`; the optional fix-up adds real popup dicts for Acrobat.

#### 4.1.2 Edit pipeline

`AnnotUpdate { patch }` resolves the annotation by `/NM`, then:

| Patch | Calls | Notes |
|---|---|---|
| move / resize (`rect`) | `set_bounds(rect)`; for markup also `set_attachment_point_at_index(i, quad)` per line | pdfium re-maps its own generated APs (`/PDFIUM_HasGeneratedAP`) through Rect/BBox; verified 0 px at old place. For APs from other producers: `raw.set_ap_none(h)` first so pdfium regenerates (the old custom look is lost — the UI marks such annotations "외부 앱에서 만든 주석: 이동 시 모양이 다시 생성됩니다"). |
| colour / opacity / width | **`raw.set_ap_none(h)`** → `FPDFAnnot_SetColor` (both `/C` and `/IC`, same alpha) → `FPDFAnnot_SetBorder` → `needs_ap_render.push(page)` | Never `set_fill_color`/`set_stroke_color` from pdfium-render on an annotation that has an AP: SIGSEGV (annotation spike §5.1). |
| ink stroke edit (eraser) | rebuild: `FPDFAnnot_RemoveInkList(h)` then `AddInkStroke` for the surviving strokes, `set_ap_none`, rect | eraser semantics = whole-stroke removal (P0) |
| contents / author / subject | `set_contents`, `set_creator`, `raw.set_string("Subj")` | `/M` updated automatically |
| text of FreeText | `set_contents` + `set_ap_none` (DA kept) | Stamp-based boxes: `TextBoxUpdate`-style rebuild of objects |
| flags (hidden/locked/print) | `raw.set_flags` | |

`AnnotDelete` = `annotations_mut().get(i)` (mutable borrow required) → `delete_annotation(a)`; also deletes a linked
popup (`FPDFAnnot_GetLinkedAnnot(h, "Popup")` → index → delete).

#### 4.1.3 Read model

`AnnotSummary { id, kind, rect, quads?, inkPaths?, line?, color, interiorColor?, opacity, borderWidth, contents,
author, created, modified, flags, editable: Editability, hasCustomAp: bool }` per page, from the high-level getters plus
raw reads (`GetNumberValue("CA")`, `GetBorder`, `GetInkListPath`, `GetLine`, `GetAP` length). Line annotations from
other producers arrive as `PdfPageAnnotation::Unsupported` — common props still work, `GetLine` supplies geometry.
`editable`: `Full` for annotations we created or pdfium-generated APs; `MoveOnly` for third-party APs of unsupported
subtypes (Polygon, Caret, FileAttachment, Redact); `ReadOnly` when `/F & LOCKED` or the document forbids annotation
edits (`PdfPermissions::can_add_or_modify_text_annotations()` false and opened without the owner password).

#### 4.1.4 Gesture ⇄ pdfium: how the UI stays optimistic without a second model (I3)

1. **Creating.** The gesture (drag over text, freehand, rubber-band rect) is drawn in the `AnnotOverlay` SVG using the
   same colour/opacity the annotation will get. On mouse-up the frontend sends `doc_apply [AnnotCreate]`; the SVG
   ghost stays until `doc:changed { revision }` arrives and the affected tiles have loaded (`onload` on the new
   `<img>` src), then it is removed. Engine cost: create 0.05 ms + one page render (2–6 ms) + PNG. Visible latency ≈
   one frame with the ghost; no flash because the ghost is removed only after the sharp tile is in the DOM.
2. **Selecting.** Click → hit-test against `AnnotSummary` rects/quads/ink paths in JS → handles drawn in SVG over the
   pdfium pixels.
3. **Dragging/resizing.** On drag start: `annot_set_transient { hidden: [id] }` → engine sets `HIDDEN`, bumps the
   transient nonce, re-renders the page (one render); the SVG shows a proxy: for markup/shape/ink types a vector
   proxy from the summary; for stamps/free-text a cropped `<img>` of the annotation's rect taken from the previous
   tiles. On drop: `doc_apply [AnnotUpdate{rect}]` + `annot_set_transient { hidden: [] }`. Undo/redo re-render via
   revision.
4. **Popover edits** (colour, opacity): apply immediately (`AnnotUpdate`), debounced at 60 ms while a slider moves;
   pdfium regenerates the AP on the next tile render, so the colour on screen is the saved colour.

### 4.2 Forms (AcroForm fill)

**Read.** `FieldSummary { id: FieldRef{page, annotIndex, name}, kind, rect, value, options[], readOnly, required,
maxLen?, multiline, comb }` from `PdfPageAnnotation::as_form_field()` + raw `FPDFAnnot_GetFormFieldFlags`. XFA forms
(`form_type()` is Xfa*) are read-only with a banner (XFA is not built into this pdfium).

**Write** (`FormSetValue`) — the high-level `set_value()`/`set_checked()` write `/V` only and the value is invisible in
every viewer; the working path is the form-fill environment (annotation spike §3.4), deterministic variant:

```rust
// engine/command/form.rs (raw handles from the fork)
let form = doc.raw_form_handle(); let page = page.raw_page_handle(); let annot = annotation.raw_annotation_handle();
raw.FORM_OnAfterLoadPage(page, form);                                   // once per opened page (FormEnv tracks it)
match value {
    FieldValue::Text(s) => {
        raw.FORM_SetFocusedAnnot(form, annot);                          // instead of a click at a coordinate
        raw.FORM_SelectAllText(form, page);
        raw.FORM_ReplaceSelection(form, page, wide(&s).as_ptr());       // UTF-16LE, NUL-terminated
        raw.FORM_ForceToKillFocus(form);                                // commits /V AND regenerates /AP
    }
    FieldValue::Checked(want) => {                                       // checkbox / radio: click the widget centre, verify
        raw.FORM_OnLButtonDown(form, page, 0, cx, cy); raw.FORM_OnLButtonUp(form, page, 0, cx, cy);
        if raw.FPDFAnnot_IsChecked(form, annot) != want { /* click again */ }
    }
    FieldValue::Selected(idx) => { raw.FORM_SetFocusedAnnot(form, annot); for i in all { raw.FORM_SetIndexSelected(form, page, i, idx.contains(&i)); } raw.FORM_ForceToKillFocus(form); }
}
```

`FORM_SetFocusedAnnot` + `FORM_SelectAllText` + `FORM_ReplaceSelection` are present in the bindings but were **not
exercised** by the spike (it used clicks + `FORM_OnChar`); `engine/command/form.rs` starts with a test on
`fixtures/160F-2019.pdf` that asserts `FPDFAnnot_GetFormFieldValue` and a > 0-byte AP after the call, with the verified
click + `FORM_OnChar` sequence as the fallback implementation. Korean input: the frontend uses **HTML overlay inputs**
(IME composition in a real `<input>` — never keystroke forwarding to pdfium) positioned by the device matrix, and commits
on blur/Enter; while focused the overlay covers the widget. Rendering uses `render_form_data(true)` (= `FPDF_FFLDraw`)
so the regenerated AP is what the user sees; `highlight_all_form_fields(PdfColor)` implements 필드 강조.

Push-button actions (JS, URI) are ignored (no V8); signature fields are read-only; the 양식 mode banner explains both.

### 4.3 Text layer, selection, search (read-only paths)

**TextLayer** (`engine/text/layer.rs`, 5.6 ms/page incl. grouping, cached ≤ 64 pages):

```rust
pub struct TextLayer { pub page: PageId, pub rev: Revision,
    pub matrix: [f32; 6],                 // page(unrotated user space) -> device(px at s=1); crop box + /Rotate (text spike, ±1 px vs FPDF_PageToDevice)
    pub chars: Vec<CharBox>,              // index, unicode(u32), loose box l,b,r,t (unrotated user space), baseline_y, font_size, flags(generated, hyphen), object_index(u32)
    pub words: Vec<(u32, u32)>,           // char index ranges
    pub lines: Vec<LineRun>,              // char range + union rect + word range
    pub text: String,                     // chars incl. generated \r\n, for copy/search
}
```

Built from `page.text()?.chars()` (`loose_bounds` for selection, `tight_bounds` for redaction hit-tests, `origin`,
`scaled_font_size`, `is_generated`, `text_object()` per char for the char→object map — the linear method (B), 84 ms
worst case on a 151-object page, cached). Serialised to the frontend as one `Response` binary: header
`{u32 magic, u32 nChars, u32 nWords, u32 nLines, f32[6] matrix}` + `Float32Array(nChars×6)` + `Uint32Array(nChars×2)`
+ ranges. 5,000 chars ≈ 160 KB; no DOM spans are created (the text layer is virtual: hit-testing is binary search over
lines then words; selection is rendered as ≤ 1 rect per line in the overlay). A hidden `<div role="document">` holds
the page text for screen readers.

**Selection** = `[pageId, charStart, charEnd]` (anchor/focus, may span pages); copy joins `text` slices with `\n`
between pages. Rect-drag selection is computed in JS against loose boxes (pdfium's `chars_inside_rect` is approximate
and off by one).

**Search** = a `Job`: per page `text().all()` (1.4 ms/page; cached in the TextLayer when present) and a
char-based case-folded matcher in Rust (65 µs/page) that yields `{page, charStart, charEnd, rects[]}` with **char
indices** (pdfium's search returns rects only); results stream over a `Channel<SearchEvent>` page by page (first hit on
a 500-page doc ≪ 150 ms since the current page is searched first, then outward). Whole-word/case options are applied
in Rust; pdfium `PdfPageText::search` is used only in the test-suite as a cross-check.

### 4.4 Text editing: explicit boundaries (I4)

#### 4.4.1 Editability matrix (computed per text object by `engine/edit/editability.rs`, shown as a badge in 편집 mode)

| Situation of the text object | Move / scale / recolour / delete | Change characters | UI |
|---|---|---|---|
| direct page child, base-14 font (Helvetica/Times/Courier/Symbol/ZapfDingbats, `is_embedded()==false`) | yes | **in place**: `set_text` after the trial coverage check (WinAnsi range only) | editable |
| direct child, embedded font **with** ToUnicode, trial check passes for the new string | yes | in place | editable |
| direct child, embedded subset lacking glyphs (trial fails: any char incl. space with `loose_bounds().width()==0`) | yes | **replace-object** with a bundled font (Pretendard for Hangul/Latin; Helvetica/Times/Courier substitute chosen by serif/fixed/italic/weight flags) — user is told "글꼴이 대체됩니다: Pretendard" before commit | editable with notice |
| direct child, no ToUnicode (raw char codes, e.g. TAMReview "Cambria") | yes (move/delete/recolour) | **refused** (`error.edit.noUnicode`) | badge 읽기 전용 |
| Type3 font | move/delete only | refused | badge |
| inside a Form XObject (tracemonkey: 268 of 419 text objects; TAMReview: 100 %) | **refused** (edits return Ok but are not persisted by `FPDFPage_GenerateContent`) | refused | badge "그룹 안의 텍스트" |
| text rendering mode Invisible (OCR layer) | delete only (re-OCR instead) | refused | hidden from 편집 mode |
| document permissions forbid content modification (opened without owner password) | refused | refused | banner |

The **trial coverage check** is the only reliable detector (`PdfFontGlyphs::len()` is 0 for subset fonts and built-ins;
`PdfFont::name()` strips the subset tag): apply `set_text(new)` on the live object, open a fresh `page.text()`, iterate
`chars_for_object` and require `loose_bounds().width() > 0` for every non-generated char including spaces; then
restore the old text if the command was only a probe (`page_text_edit_probe` query) — all on the engine thread, no
regeneration needed because `page.text()` reflects in-memory object state.

#### 4.4.2 Safe in-place edits (always persisted; verified by the text spike)

| Edit | Calls (page in `Manual`; `regenerate_content()` once at the end of `apply`) |
|---|---|
| move | `obj.translate(dx, dy)` |
| scale about the object's own origin | `translate(-x,-y); scale(k,k); translate(x,y)` (`scale()` alone scales about the page origin) |
| font size | `text.set_unscaled_font_size(pt)` — `bounds()` is stale until regeneration/reload; the TextLayer is rebuilt from `pages_content` |
| colour | `set_fill_color(PdfColor)` |
| delete | `objects_mut().remove_object_at_index(i)` — later indices shift by −1; the command carries all `ObjectRef`s and deletes descending |
| in-place text | `text.set_text(s)` (`""` becomes `" "`; never create empty objects) |

#### 4.4.3 Replace-object model (when the trial fails or the font is substituted)

`PdfPageTextObject::new(&doc, text, font_token, unscaled_size)` → `apply_matrix(old.matrix())` → `set_fill_color(old.fill_color())`
→ `set_render_mode(old.render_mode())` → `objects_mut().add_text_object(new)` → `remove_object_at_index(old_index)`
→ `regenerate_content()`. The new object lands at the end of the object list (z-order changes: text is on top of any
later-drawn path). `FPDFPage_InsertObjectAtIndex` exists in the bindings (pdfium_7881) and is used raw to keep the
original z-order when the fork lands; `objects.rs` has a test that the object index is preserved.

For redaction run-splitting (§4.8) the *same* font resource must be reused; there `FPDFTextObj_GetFont(old)` →
`FPDFPageObj_CreateTextObj(doc, font, size)` → `FPDFText_SetText` (raw) keeps the embedded subset, which by
construction covers the surviving glyphs.

#### 4.4.4 Fonts we bring (`engine/edit/fonts.rs`)

* **Pretendard Variable → static Regular/Bold** (OFL; the UX design font) as the Hangul/Latin editing font;
  **Helvetica/Times/Courier** via `PdfFonts::helvetica()` etc. (base-14, nothing embedded) when the string is WinAnsi.
* Never a system font: pdfium embeds the entire file (AppleGothic +6.2 MB, Arial Unicode +15 MB, AppleSDGothicNeo.ttc
  +31 MB per document; AppleGothic additionally maps space→TAB in the generated ToUnicode and breaks search).
* **Subsetting is mandatory for CJK**: `ttf-parser` 0.25 checks `cmap` coverage of the string first (pdfium checks
  nothing; missing glyphs vanish silently or become `ÿ`); `subsetter` 0.2.6 produces a glyph subset; the subset is loaded
  once per document per (font, glyph-set) with `fonts_mut().load_true_type_from_bytes(&bytes, /*is_cid*/ true)` and the
  `PdfFontToken` (Copy) cached in `OpenDoc.fonts`. `fonts_mut()` needs `&mut PdfDocument` — take the token **before**
  holding any `PdfPage`. Additional characters typed later → a second subset font (fonts are cheap; one per commit
  batch at most). A test asserts the subset keeps `cmap`, extraction round-trips Hangul with real spaces (not `\t`), and
  the per-document cost is ≤ 200 KB for a 300-glyph subset.
* Metrics for our own layout: `ttf-parser` advances for Pretendard; static AFM width tables for the base-14 fonts
  (`engine/edit/afm.rs`, generated once from the Adobe Core14 AFMs, ~6 KB).

#### 4.4.5 Text boxes (P1 "텍스트 추가") and in-place editing (P2 as per UX, but the model is v1)

`TextBoxSpec { rect, text, font: BundledFont, size, color, align, lineHeight }`. `engine/edit/layout.rs` breaks lines
(greedy, on spaces and after CJK characters, hyphen-free) using the metrics above, and `TextBoxCreate` emits one
`PdfPageTextObject` per line placed at the line's baseline; the block registry (`BlockId → Vec<ObjectRef>`) lets
`TextBoxUpdate` delete and re-create the block as one transaction. No reflow of existing paragraphs in v1: an in-place
edit of an existing object keeps its single line and warns when the new width exceeds the old one by > 20 %
(`prop.text.overflow`).

### 4.5 Images

`ImageInsert`: `image::load_from_memory(blob)` (png+jpeg features) → `objects_mut().create_image_object(x, y, &img,
Some(w_pt), Some(h_pt))` (5.7 ms for 160×120; ~840 ms for an 8.4 MP scan — run as a `Heavy` command, progress via the
status bar). `ImageReplace`: `as_image_object_mut().set_image(&img)` keeps the matrix (0.03 ms). Transform/delete as
page objects. JPEG sources are stored re-encoded as Flate by `set_image` (pdfium never keeps JPEG) — so for *inserting*
photos use `PdfPageImageObject::new_from_jpeg_reader(&doc, reader)` + `scale/rotate/translate` (order matters: a new
object is 1×1 pt at the origin) + `add_image_object` to keep the DCT stream. "Extract image" uses `get_processed_image`
(what the page shows) and, when `filters()` says `DCTDecode`, `get_raw_image_data()` for a byte-exact JPEG export.

### 4.6 Page operations (`engine/command/page_ops.rs`)

All structural commands first clear the page LRU and text caches for the document (`Effects::structure`), because
raw `FPDF_MovePages` bypasses pdfium-render's `PdfPageIndexCache` and `PageId` order changes.

| Command | Calls | Cost |
|---|---|---|
| `PageMove` | validate (no duplicates, all `< len`, `dest + n ≤ len`) → raw `FPDF_MovePages(doc, &indices, n, dest)`; on `false` → rebuild from checkpoint (the header warns of an indeterminate state) | 0.02 ms |
| `PageDelete` | `pages().get(i)?.delete()` in **descending** index order | 2 ms each |
| `PageRotate` | `page.set_rotation(PdfPageRenderRotation::…)` (not a `Result`) under `Manual` for the batch | 0.1 ms + one regeneration |
| `PageInsertBlank` | `pages_mut().create_page_at_index(PdfPagePaperSize::from_points(w,h), i)` | 0 ms |
| `PageDuplicate` | raw `FPDF_ImportPagesByIndex(doc, doc, &[i], 1, i+1)` (src == dest; verified 14 → 15 pages) | 0.4 ms |
| `PageInsertFromFile` | source bytes from the blob store → scratch `load_pdf_from_byte_vec` → `pages_mut().copy_page_range_from_document(&src, a..=b, at)` (**zero-based inclusive**; the string form `copy_pages_from_document(src, "1-3,5", at)` is one-based and is used only for user-typed ranges) → if the source has an AcroForm, `intents.restore_acroform` (widgets survive, `/AcroForm` does not) | 0.5 ms / 4 pages |
| 추출 / 분할 (export) | **copy of the original + delete unwanted pages** (keeps `/AcroForm`, outline, metadata), never "new doc + import" | 2 ms/page |
| 병합 (merge into current) | `PageInsertFromFile` per source; warning when a source carries forms/outline (`pages.merge.formsWarning`) | |

Organizer UX consequences: grid layout from `pages().page_sizes()` (0.23 ms / 14 pages) before any pixels; rotation
preview = re-render one thumbnail; drag reorder emits exactly one `PageMove`.

### 4.7 OCR text layer (apply side; engines in §5)

Input (engine-agnostic, produced by Vision or tesseract.js):

```ts
interface OcrPage { page: PageId; dpi: number; widthPx: number; heightPx: number; rotation: 0|90|180|270;
  lines: { text: string; bbox: Box; baseline?: [x0,y0,x1,y1]; rowHeightPx?: number;
           words: { text: string; bbox: Box; confidence: number }[] }[] }   // Box = image px, origin top-left
```

`OcrApplyLayer` per page (page in `Manual`; 2 ms for 730 words):
1. Skip pages that already have ≥ 50 non-generated chars unless `replace_existing` (then delete existing
   render-mode-3 objects first).
2. Font: **glyphless CID font** via raw `FPDFText_LoadCidType2Font(doc, pdf_ttf, len, &to_unicode_cmap,
   cid_to_gid.as_ptr(), cid_to_gid.len())` (signature verified in `bindings.rs:7736`; requires the raw document handle
   from the fork). `pdf_ttf` = Tesseract's 1-glyph `pdf.ttf` (Apache-2.0, < 1 KB) or our own generated 1-glyph
   TrueType; CIDs are assigned per OCR job from the set of distinct characters (`cid = 1 + rank`), `CIDToGIDMap` maps
   every CID to GID 0, `ToUnicode` is a `bfrange`/`bfchar` CMap over the same table. Language-agnostic, ~1–3 KB per
   document, the technique ocrmypdf/Tesseract use. Two things to verify in `ocr/tests/cidfont.rs` before relying on it:
   the advance of glyph 0 is > 0 (otherwise `bounds()` is degenerate and the horizontal fit divides by zero — then we
   generate our own font with advance 500) and `page.text().all()` reproduces Hangul after save+reload.
   **Fallback** (verified today): bundled Pretendard subset via `load_true_type_from_bytes(.., true)` for Hangul words,
   `helvetica()` for Latin-1-only words.
3. Geometry (all verified in the text/OCR spikes): `s = 72 / dpi`; per line `font_size_pt = rowHeightPx × s` (Vision:
   box height; ×1.15 when only word boxes exist); per word `obj = PdfPageTextObject::new(&doc, word, font, size)`,
   `set_render_mode(Invisible)`, `natural_w = obj.bounds()?.width()` (works before adding), `sx = clamp(box_w_pt /
   natural_w, 0.5, 2.0)`, `obj.scale(sx, 1.0)` **before** `obj.translate(x_pt, baseline_pt)`; baseline from Tesseract's
   `line.baseline` or `box.bottom + 0.2·h` (Latin) / `0.1·h` (Hangul). Pixel→page via `page.pixels_to_points(x, y,
   &same_render_config)` so `/Rotate` is honoured; then `rotate_counter_clockwise_degrees(rotation)` on the object.
   Spaces are reconstructed from `line.text` (Tesseract over-segments Hangul into syllables); words with confidence < 30
   are skipped; `add_text_object` each; one `regenerate_content()`.
4. `verify`: rebuild the TextLayer; for 3 sampled words assert `search` hits and that the hit rect ⊂ word box ± 2 pt.

The whole OCR job is **one transaction** ("OCR 148쪽", undoable) with a checkpoint right after it.

### 4.8 Redaction that truly removes content (I5)

`RedactApply { page, regions, options: { fill: Rgba, overlayText?: String, strict: bool, rasterDpi: 300 } }` — the
`redact_preview` query runs steps 1–3 without mutating and returns what will happen so the confirm dialog can say
"텍스트 12단어, 이미지 1개 제거 · 이 페이지는 이미지로 변환됩니다".

1. **Hit-test** using the TextLayer's *tight* boxes (ink boxes; loose boxes would over-remove neighbours). Affected
   objects = text objects with ≥ 1 intersecting char; image/path/shading/form objects with intersecting `bounds()`.
2. **Classify** each affected object:
   * *fully inside a region* → remove (`remove_object_at_index`, descending), any type;
   * *text, partial, direct child, chars have Unicode* → **split**: surviving chars grouped into runs of consecutive
     non-space chars that do not intersect any region; for each run raw `FPDFTextObj_GetFont(old)` →
     `FPDFPageObj_CreateTextObj(doc, font, unscaled_size)` → `FPDFText_SetText_str(new, run)` → matrix = old
     `[a b c d]` with `[e f]` = the run's first-char `origin()` → `FPDFPage_InsertObjectAtIndex(page, new, old_index)`;
     copy fill colour and render mode; then remove the old object. Kerning inside a run (TJ offsets) is lost — drift is
     bounded by the run length and is invisible at word level; positioning per run (not per word with space glyphs)
     avoids the zero-width-space problem of subset fonts.
   * *text inside a Form XObject, or without Unicode, or Type3* → cannot be split → **page raster fallback** (below);
     first try raw `FPDFFormObj_RemoveObject(form, child)` for fully-covered children — it exists in the bindings but
     persistence through `FPDFPage_GenerateContent` is unverified (the text spike showed `set_text` on form children is
     not persisted). `redact/tests/xobject.rs` decides: if removal persists, partial XObject text is handled by removing
     the child and re-creating surviving runs on the page (font via `FPDFTextObj_GetFont`); if not, raster.
   * *image, partial* → `get_raw_image()` → map the region into image pixel space with the inverse object matrix → fill
     the pixels with `fill` → `set_image(&img)` (matrix preserved; note Flate re-encode). Soft masks (`/SMask`) are not
     touched: if the object has one (`get_processed_image` differs from raw) → raster fallback in `strict`, else fill the
     base image and warn.
   * *path/shading, partial* → kept unless `strict` (then raster). Fully-inside paths are removed.
3. **Annotations** whose rect intersects a region → `AnnotDelete` (plus popups). Form widgets intersecting → refused
   with `error.redact.formField` (delete the field first).
4. **Fill**: `create_path_object_rect(region, None, None, Some(fill))` per region; optional overlay text as a Helvetica
   text object clipped to the region (no clipping API → size to fit).
5. `regenerate_content()`; **verify in memory**: rebuild TextLayer, assert no tight char box intersects a region; assert
   no image bounds intersect; record `removed: Vec<RemovedSnippet{ region, text }>` in the command for the save-time
   verifier (§2.6.5, position-based, not string-based, so a repeated word elsewhere does not fail the check).

**Raster fallback** (`engine/edit/raster.rs`, `Heavy`): render the page at `rasterDpi` with `.rotate(inverse of
page.rotation(), true)` so the bitmap is in unrotated user space; paint the regions with `fill` in the bitmap; remove
**all** page objects; `create_image_object(crop.left, crop.bottom, &img, Some(crop.width), Some(crop.height))`; re-add an
invisible text layer for the surviving chars of the original TextLayer (same machinery as §4.7, glyphless font) so
search/selection still work; delete intersecting annotations; `regenerate_content()`. The UI must say the page becomes
an image (and that vector quality is lost) before the user confirms. A test on `fixtures/TAMReview.pdf` (all text in
XObjects) covers this path; `rotation.pdf` covers the rotation handling (unverified reasoning — must be tested).

Metadata (`/Info`, XMP) is not scrubbed by redaction; "메타데이터 제거" is a separate `MetadataSet` intent (P1).

### 4.9 Flatten and export

* `Flatten { annots, forms }`: raw `FPDFPage_Flatten(page, FLAT_NORMALDISPLAY)` (**not** `PdfPage::flatten()`, which uses
  `FLAT_PRINT` and silently *deletes* annotations without the Print flag — and is behind a crate feature we do not
  enable) → `FPDFPage_GenerateContent` → close and re-load the page. Return 2 ("nothing to do") is fine. Exported
  flattened copies run this on a scratch document (§2.6).
* PNG/JPEG export: `Job` per page: render (`use_print_quality(true)`, DPI selector 72–600) on the engine thread, encode on
  the encoder pool (4.4 ms/page PNG at 150 DPI; 1.17 MB/page), size estimate shown before writing, cancellable.
* Text export: `text().all()` per page (1.4 ms/page), `\f` between pages.
* Print (P0): a print-only DOM of `<img>` pages at 200 DPI through `seepdf://` + `window.print()` (system dialog on both
  platforms, WYSIWYG because pdfium rendered it). Limitation: mixed page sizes are scaled to the printer page; native
  printing (`NSPrintOperation` / `ShellExecuteW "print"`) is P1.
* Attachments: `attachments_mut().create_attachment_from_bytes(name, bytes)` / `delete_at_index` — full read/write
  exists; exposed in 문서 정보 (P1).
