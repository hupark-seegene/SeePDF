# SeePDF v1 — Proposal "Product": a polished v1 with the fewest moving parts

Author angle: product / simplicity-first. Ship a v1 that feels like Preview.app with Korean competence,
built from the smallest set of concepts that the verified spikes (`docs/spikes/*.md`) prove will work.
Everything below cites those spikes; where a claim is *not* yet verified it is marked **[unverified]**
and appears in the risk table and in a module's definition-of-done.

Ground truth used: pdfium-render **0.9.4** (`pdfium_7881` bindings) driving PDFium **chromium/8057**
(`src-tauri/resources/pdfium/`), tauri **2.11.5**, image **0.25.10**, png **0.18.1**, crossbeam-channel
**0.5.17**, `@tauri-apps/api` **2.11.1**, react **19.3**, vite **8.3**, typescript **6.0.3**, tesseract.js **7.0.0**.

---

## 0. Thesis and the ten decisions

SeePDF v1 has exactly **one pdfium thread, one document per window, one pixel transport, one undo
mechanism, one persistence model for annotations, one OCR contract**. Every feature is expressed in
those terms or it is not in v1.

| # | Decision | Why (spike evidence) |
|---|---|---|
| D1 | **Fork pdfium-render 0.9.4 with a ~30-line visibility patch** (`pub fn handle()` on document/page/form/annotation, `pub fn bindings()`, `save_to_bytes_with_flags`). One `PdfDocument` per open file; raw FFI only for the six verified gaps. **No** second `bind_to_library` / parallel raw documents. | pages.md §0.1, annotations.md §1.1: every needed raw call exists in the bindings, only the handle is `pub(crate)`. Two documents for one file is the single largest source of accidental complexity in the alternatives. |
| D2 | **Drop the `thread_safe` feature.** pdfium types become `!Send`, so the compiler enforces "every pdfium call on the engine thread". Enable `flatten`. | render.md §5: `thread_safe` locks only 289/481 FFI functions and gives 0 % parallel speed-up; the engine thread design needs no `Send`. |
| D3 | **Two-crate Cargo workspace**: `crates/engine` (pdfium only, no tauri, closure-based actor) + the tauri app crate. Engine is integration-tested headless through the real actor API. | Keeps `cargo test -p seepdf-engine` at seconds, keeps tauri out of 90 % of the Rust code, lets 2 agents work on the engine without touching the same `match`. |
| D4 | **All pixels go through `seepdf://` with immutable, generation-keyed URLs** (PNG `Fast`+`Sub`), loaded with `fetch()` + `createImageBitmap`. **No Rust-side bitmap cache. No binary IPC in v1.** | render.md §4: PNG 2.3 ms/page @2x, ~0.2 ms/512 px tile; a tile re-render is 0.01–0.8 ms, cheaper than any cache logic. tauri.md §1: WebKit caches custom-scheme responses; `fetch` gets headers + abort. |
| D5 | **Undo = whole-document snapshots** (`save_to_bytes()`, 5 ms/MB) with a byte budget and disk spill. One user edit = one engine command = one snapshot. | text.md §7, pages.md §8: no incremental API in the safe layer; snapshots are universal across annotations, forms, pages, OCR and text objects with zero per-op inverse logic. |
| D6 | **Annotations live in pdfium; the SVG overlay is transient only** (in-progress strokes, drag ghosts, selection handles). Annotation ids are `(page, index)` re-issued by every mutating command. | annotations.md §2/§5: pdfium renders every type it can create; APs are generated on render; enumeration is 1.6–5 ms per page. |
| D7 | **Korean text box = Stamp annotation + text object with a bundled subset Korean font**; line/arrow = Ink annotation with `/InkList`; ellipse = raw Circle. FreeText is not used. | annotations.md §0: FreeText renders only with `/DA` and Helvetica (no Hangul); Stamp+text object "handles 한글", persists, renders in every viewer; Line/Polygon cannot be created by PDFium. |
| D8 | **Forms are filled through `FORM_*` events on pdfium-render's own form handle**, with an HTML `<input>` overlay in the webview. | annotations.md §3.4: high-level `set_value` leaves the field visually empty; the raw event path regenerates the AP and persists (Korean OK). |
| D9 | **OCR: macOS Vision primary, tesseract.js (kor+eng) everywhere else; one `OcrPage` contract; invisible text objects with the bundled font.** No runtime font subsetter, no glyphless CID font in v1. | ocr.md §1/§5: Vision CER 0.21 % Hangul vs 1.2–12 %; the invisible-layer chain is verified end-to-end; pdfium embeds whole fonts, so ship a pre-subsetted font (≈1–1.5 MB once per document). |
| D10 | **Frontend = React 19 + zustand + lucide-react + a 60-line i18n hook + plain CSS tokens + a hand-rolled virtual scroller. One window per document.** | ux-research.md §4/§6: 373 flat keys, two locales, one accent colour; a generic list virtualiser cannot do variable page heights + placeholder bitmaps well. |

What is **not** in v1 (see §1.4): in-place editing of existing text, set/change password, writing metadata,
page labels or bookmarks, compression, document tabs, link creation, headers/footers, compare, batch, form
authoring, Office/hwp export, auto-update (v1.1).

---

## 1. Scope cut

### 1.1 P0 — v1 does not ship without these

| # | Feature | Acceptance criteria (each is a test or a manual checklist item) |
|---|---|---|
| P0-1 | Open PDF (dialog, drag & drop, Finder/Explorer double-click, argv), password prompt, close, unsaved guard | Every file in `fixtures/` opens and paints page 0 ≤ 250 ms (release). A wrong password shows the `security.password.wrong` message and re-prompts. Quitting with a dirty doc shows 저장/저장 안 함/취소. |
| P0-2 | Continuous / single / two-page layout, zoom 25–400 % + fit page/width, pinch zoom, view rotation | Zoom step paints (CSS-scaled) within one frame; sharp tiles within 250 ms; no white gaps while scrolling tracemonkey at 100 %. View rotation keeps text selection and annotation hit-testing aligned (unit test on all 4 rotations). |
| P0-3 | Thumbnail rail (lazy), outline panel | 500-page synthetic doc: rail placeholders ≤ 100 ms, visible thumbs ≤ 500 ms. Outline renders the 7-node generated outline fixture. |
| P0-4 | Text selection + copy; search with streamed results | Selecting "Trace-based" on tracemonkey p1 copies exactly that string; search "monkey" reports 62 hits over 14 pages; first result ≤ 150 ms on the 500-page doc. |
| P0-5 | Annotations: highlight, underline, strikeout, sticky note, ink, eraser (whole-stroke), rectangle, ellipse, line, arrow, text box (Korean OK); select/move/resize/delete; colour/opacity/thickness; annotation list | Each type created on tracemonkey p0, saved, reopened with pdfium **and** rendered by macOS Preview: visible and at the right place. Recolouring an annotation loaded from disk does not crash (SetAP(NULL) recipe). |
| P0-6 | Undo/redo for everything | 20 random edits → 20 undos → document bytes equal the original snapshot (test). |
| P0-7 | Save, Save As, recent files with restored reading position, welcome screen | Save writes to a temp file, verifies page count, renames atomically; a read-only target falls through to Save As. Recents show thumbnails and reopen at the last page/zoom. |
| P0-8 | Organize pages: reorder (drag), delete, rotate, duplicate, insert blank, insert from file, extract, merge | 160F-2019.pdf keeps `form()==Some` after split-by-delete; page reorder [3,2]→1 gives [A,D,C,B]; all undoable. |
| P0-9 | Form filling: text, checkbox, radio, combo, list; values visible after save/reopen | 160F-2019.pdf text field typed "Raw FORM 입력" is rendered by pdfium and Preview after save. |
| P0-10 | OCR ko+en, page range, skip-pages-with-text, progress + cancel, offline, one undo step | Synthetic Korean page: Vision Hangul CER ≤ 1 %, tesseract PSM 4 ≤ 2 %; `search("검색가능한")` hits after apply; cancel at page 3 of 14 leaves the document unchanged. |
| P0-11 | Export PNG/JPEG (DPI, range), plain text, flattened PDF; print | 14 pages @150 DPI PNG ≤ 400 ms release; flattened output has 0 annotations and identical pixels ±AA. |
| P0-12 | ko/en i18n (373-key catalogue), light/dark, Pretendard | `scripts/check-i18n.mjs` passes (identical key sets); no string literal in JSX outside `i18n/`. |
| P0-13 | Performance targets of ux-research §7 for open, scroll, zoom, search | `scripts/perf-baseline.mjs` numbers recorded in `docs/perf/baseline.md`; regressions > 20 % fail CI (P1 gate). |

### 1.2 P1 — in v1 if the schedule holds, else v1.1

Signature (draw → Ink annotation with `/Subj Signature`, image → Stamp), **redaction (true object removal +
rasterised image regions)**, remove password (`FPDF_REMOVE_SECURITY`), night mode (dark/sepia filter on the
bitmap layer only), Korean stamp set (결재/승인/기밀 as Stamp + bundled-font text), split by range / every N,
per-tool default styles, reading mode + full screen, add text / add image objects (select, move, delete).

### 1.3 P2 — after v1

In-place editing of existing text runs → reflow; document tabs; link create/edit; headers/footers/page
numbers/watermarks; bookmark editing (needs `lopdf`); batch OCR/export; compare; crop/resize/page labels;
form authoring; Word/hwp export; annotation replies; multi-file search; auto-update (`tauri-plugin-updater`).

### 1.4 Explicitly out of v1, and why

| Cut | Reason (spike) |
|---|---|
| In-place editing of existing text | text.md §5: subset fonts drop glyphs silently, XObject text is not editable, no reflow. The P1 "add text" object covers the common "insert a note/date" case. |
| Set / change password, permission flags | pages.md §5: impossible with pdfium; needs an encryption writer crate. |
| Write metadata, page labels, outline | pages.md §5/§6: no `FPDF_SetMetaText`, no outline write API; needs `lopdf`. |
| Compress / optimise | pages.md §8: `set_image()` always writes Flate; a real optimiser needs `lopdf` stream replacement. |
| Incremental save | pages.md §8: 25× faster but grows the file; full rewrite is 5 ms/MB and already meets the ≤ 300 ms target up to ~60 MB. |
| Document tabs | one window per document gives the same capability with zero tab-state code. |
| Go-to-page links, popup linking, FreeText | annotations.md §5.11–12/§0. |
| Binary IPC | tauri.md: pixels through the protocol are equally fast and need no bookkeeping. |

---

## 2. Repository layout and module ownership

```
SeePDF/
├─ package.json                    # + zustand, lucide-react, vitest, jsdom,
│                                  #   @tauri-apps/plugin-{dialog,store,window-state,opener}
├─ vite.config.ts                  # + `test` block (vitest, environment jsdom)
├─ public/
│  ├─ fonts/PretendardVariable-ko.woff2        # OFL, KS X 1001 subset, ~350 kB
│  └─ ocr/                                     # copied by scripts/prepare-ocr.mjs (postinstall), gitignored
│     ├─ worker.min.js  tesseract-core-simd-lstm.wasm.js  tesseract-core-lstm.wasm.js
│     └─ eng.traineddata.gz  kor.traineddata.gz            # 4.0.0_best_int, 8.4 MB total
├─ scripts/  fetch-pdfium.mjs  prepare-ocr.mjs  check-i18n.mjs  check-bundle-size.mjs  perf-baseline.mjs
├─ fixtures/                        # + fixtures/gen/ (outline-labels.pdf, encrypted-rc4-40.pdf from the spike generators)
├─ docs/  spikes/  design/  perf/baseline.md
├─ src/
│  ├─ main.tsx  App.tsx
│  ├─ ipc/        types.ts  api.ts  events.ts  protocol.ts          # THE contract (§8); owner: A0/A3
│  ├─ stores/     appStore.ts (A6)  docStore.ts (A4)  jobStore.ts (A7)
│  ├─ i18n/       index.ts  ko.json  en.json                         # also include_str!'d by src-tauri/src/app/menu.rs
│  ├─ styles/     tokens.css  base.css
│  ├─ keys/       keymap.ts  useKeymap.ts
│  ├─ app/        TitleBar  ToolStrip  Sidebar  Inspector  StatusBar  Toasts  ContextMenu  useOpenFiles.ts
│  ├─ welcome/    Welcome  RecentCard  recents.ts
│  ├─ viewer/     Viewer  Scroller  useVirtualPages  PageView  PageBitmap  tiles.ts  geometry.ts
│  │              TextLayer  selection.ts  useTextSelection  SearchPanel  ThumbnailRail  OutlinePanel  zoom.ts  PrintView
│  ├─ tools/      AnnotationLayer  ToolOverlay  annotModel.ts  AnnotationList  InspectorAnnot  NotePopover
│  │              tools/{highlight,ink,shape,note,textbox,line,stamp,signature}.ts
│  ├─ forms/      FormLayer  FieldInput
│  ├─ pages/      PagesGrid  PageTile  dnd.ts
│  ├─ ocr/        OcrDialog  ocrJob.ts  tesseractPool.ts  normalize.ts  types.ts
│  ├─ dialogs/    ExportDialog  PasswordDialog  UnsavedDialog  SettingsDialog  MergePrompt
│  └─ test/       setup.ts  mockInvoke.ts
├─ src-tauri/
│  ├─ Cargo.toml                    # [workspace] members = [".", "crates/engine"]; [patch.crates-io] pdfium-render fork
│  ├─ crates/engine/                # crate `seepdf-engine`  (NO tauri dependency)
│  │  ├─ Cargo.toml
│  │  ├─ src/  lib.rs  error.rs  types.rs  geom.rs  raw.rs  thread.rs  registry.rs  render.rs  encode.rs
│  │  │        text.rs  search.rs  annot.rs  forms.rs  pages.rs  objects.rs  redact.rs  history.rs  save.rs
│  │  │        export.rs  fonts.rs  ocr/{mod.rs,layer.rs,vision.rs}  test_support.rs
│  │  └─ tests/  smoke.rs  render.rs  text.rs  annot.rs  forms.rs  pages.rs  history.rs  ocr_layer.rs
│  ├─ src/  main.rs  lib.rs
│  │  └─ app/  mod.rs  protocol.rs  jobs.rs  files.rs  windows.rs  menu.rs  recents.rs  pdfium_path.rs
│  │           commands/{docs,text,annot,forms,pages,objects,ocr,history,save,files}.rs
│  ├─ resources/  pdfium/ (fetched)  fonts/SeePDF-Hangul.ttf (committed, OFL, ~1–1.5 MB)
│  ├─ tauri.conf.json  tauri.macos.conf.json  tauri.windows.conf.json  capabilities/default.json
└─ .github/workflows/  ci.yml  release.yml
```

**Ownership rule:** one agent per directory (or per file inside `stores/`), no exceptions; cross-module needs go
through `src/ipc/types.ts` (frontend) and `crates/engine/src/types.rs` (Rust), both frozen on day 0 by A0
and changed only by pull request to A0.

---

## 3. Process / thread architecture

### 3.1 The pdfium-render fork (D1, D2)

Fork `ajrcarey/pdfium-render` at tag `0.9.4` into `github.com/<org>/pdfium-render`, branch `seepdf-0.9.4`,
pinned by commit in `src-tauri/Cargo.toml`:

```toml
[workspace]
members = [".", "crates/engine"]

[patch.crates-io]
pdfium-render = { git = "https://github.com/<org>/pdfium-render", rev = "<sha>" }

# crates/engine/Cargo.toml
[dependencies]
pdfium-render = { version = "0.9.4", default-features = false, features = ["pdfium_7881", "image_025", "flatten"] }
#                                                   ^ no `thread_safe`: pdfium types are !Send, the compiler enforces D2
```

The patch (all verified locations in the 0.9.4 source):

| File | Change |
|---|---|
| `src/pdfium.rs:74,81` | `fn bindings(&self)` → `pub fn bindings(&self) -> &'a dyn PdfiumLibraryBindings` |
| `src/pdf/document.rs:206` | `pub(crate) fn handle` → `pub fn handle(&self) -> FPDF_DOCUMENT` |
| `src/pdf/document/page.rs:229,235` | `page_handle()`, `document_handle()` → `pub` |
| `src/pdf/document/form.rs:181` | `handle()` → `pub fn handle(&self) -> FPDF_FORMHANDLE` |
| `src/pdf/document/page/annotation.rs` | add `pub fn raw_handle(&self) -> FPDF_ANNOTATION { self.handle() }` on `PdfPageAnnotation` (the `PdfPageAnnotationPrivate` trait stays private) |
| `src/pdf/document/page/object.rs` | add `pub fn raw_handle(&self) -> FPDF_PAGEOBJECT` on `PdfPageObject` |
| `src/pdf/document.rs` | add `pub fn save_to_bytes_with_flags(&self, flags: u32) -> Result<Vec<u8>, PdfiumError>` (copy of `save_to_writer`, `flags` parameter instead of the hard-coded `0`) |

The raw calls we then make on the shared handles, all present in `src/bindings.rs` (verified by grep):
`FPDF_MovePages`, `FPDF_ImportPagesByIndex` (duplicate, src == dest), `FPDFPage_CreateAnnot` (Circle),
`FPDFAnnot_SetAP` (NULL → safe recolour), `FPDFAnnot_SetColor/SetBorder/SetFlags/AddInkStroke/GetInkListPath/GetLine/GetNumberValue/SetStringValue`,
`FPDFAnnot_RemoveObject` (rebuild text-box stamp), `FPDFPage_Flatten` (`FLAT_NORMALDISPLAY`),
`FORM_OnAfterLoadPage/OnBeforeClosePage/OnLButtonDown/OnLButtonUp/SetFocusedAnnot/SelectAllText/ReplaceSelection/SetIndexSelected/ForceToKillFocus`,
`FPDFAnnot_IsChecked/GetFormFieldValue`. Everything else stays high-level. `crates/engine/src/raw.rs` is the
only file that contains `unsafe` blocks; it exposes safe functions taking `&PdfDocument`/`&PdfPage`/`&PdfPageAnnotation`.

Why not vendor by path: the crate is 24 MB (9.9 MB bindgen). GitHub is reachable from this network and from CI.

### 3.2 Threads

```
main thread (tauri/wry)        : window, native menu, protocol handler entry (must not block)
seepdf-engine (std::thread)    : Pdfium + every PdfDocument/PdfPage; the ONLY thread that calls pdfium
seepdf-encode-N (std::thread)  : PNG/JPEG encoding for export jobs (N = cores-1, scoped, per job)
seepdf-ocr (std::thread, mac)  : Vision requests (GPU/ANE); receives BGRA bytes, returns OcrPage
tokio (tauri::async_runtime)   : #[tauri::command] async fns awaiting Reply::oneshot()
WebWorkers (webview)           : tesseract.js pool (≤ min(4, cores-2))
```

The interactive protocol path is: WKWebView → `handle()` on the main thread (parses the URL, ~10 µs) →
`EngineHandle::render(..)` on the engine thread → `UriSchemeResponder::respond` **from the engine thread**
(verified: responder is `Send`, tauri.md §1).

### 3.3 Engine crate public API (`crates/engine/src/lib.rs`)

```rust
pub use types::*;   // every DTO in one file, `serde::Serialize/Deserialize`, mirrored 1:1 by src/ipc/types.ts

pub struct EngineHandle {                       // Clone + Send + Sync; the only thing tauri manages
    hi: crossbeam_channel::Sender<Cmd>,          // interactive: commands, visible tiles
    lo: crossbeam_channel::Sender<Cmd>,          // background: thumbnails, prefetch, job chunks
    pub index: Arc<parking_lot::RwLock<HashMap<DocId, DocSummary>>>, // mirror for non-engine readers (protocol 404s)
}

pub enum Lane { Interactive, Background }

pub struct Cmd {
    pub lane: Lane,
    pub label: &'static str,                     // for tracing
    pub view_gen: Option<(DocId, u64)>,          // renders only: dropped if stale (§3.5)
    pub run: Box<dyn FnOnce(&mut EngineState) + Send + 'static>,
}

impl EngineHandle {
    pub fn spawn(lib: PathBuf, fonts_dir: PathBuf) -> Result<EngineHandle, EngineError>;
    /// Run `f` on the engine thread and await its result (used by every tauri command).
    pub async fn call<T: Send + 'static>(&self, lane: Lane, label: &'static str,
        f: impl FnOnce(&mut EngineState) -> Result<T, EngineError> + Send + 'static) -> Result<T, EngineError>;
    /// Fire-and-forget with a callback (protocol handlers pass the UriSchemeResponder into `reply`).
    pub fn send(&self, lane: Lane, label: &'static str, view_gen: Option<(DocId, u64)>,
        f: impl FnOnce(&mut EngineState) + Send + 'static) -> Result<(), EngineError>;
    pub fn latest_view_gen(&self, doc: &DocId) -> u64;   // AtomicU64 per doc, bumped by `send` when view_gen is newer
}

pub struct EngineState<'p> {                     // lives on the engine thread's stack; !Send
    pub pdfium: &'p Pdfium,                      // Box::leak'd once (Pdfium::drop never calls FPDF_DestroyLibrary)
    pub raw: &'p dyn PdfiumLibraryBindings,      // pdfium.bindings() from the fork
    pub docs: registry::Registry<'p>,
    pub fonts: fonts::BundledFonts,              // bytes of SeePDF-Hangul.ttf, loaded once
    pub text_cache: text::TextCache,             // (doc, gen, page) -> Arc<TextLayer>, LRU 64 entries
    pub thumb_cache: render::ThumbCache,         // (doc, gen, page, width) -> Arc<Vec<u8>> PNG, 8 MiB cap
}
```

Module functions are plain `pub fn xyz(st: &mut EngineState, args) -> Result<T, EngineError>`; the tauri
command layer is one line each: `engine.call(Lane::Interactive, "create_annotation", move |st| annot::create(st, doc, page, spec)).await`.
This is what lets A1 and A2 add functionality without editing a shared `match`.

`Reply`/oneshot: `tauri::async_runtime::channel::<T>(1)` + `try_send` (verified, no tokio dependency needed) —
but because the engine crate must not depend on tauri, `call` is generic over a tiny `Oneshot` trait implemented
in the app crate (`app/mod.rs`, 15 lines) and, in tests, by `std::sync::mpsc`.

### 3.4 Document registry, ids, generations, page LRU (`registry.rs`)

```rust
pub type DocId = String;                          // "d1", "d2", … allocated by Registry::open
pub struct OpenDoc<'p> {
    pub doc: PdfDocument<'p>,                     // owns its Vec<u8> (load_pdf_from_byte_vec)
    pub bytes: Arc<[u8]>,                         // the bytes `doc` was loaded from (history base, "reload after save")
    pub path: Option<PathBuf>,
    pub generation: u32,                          // bumped by every mutation; in every tile URL and DocInfo
    pub saved_generation: u32,                    // == generation  <=>  not dirty
    pub pages: PageLru<'p>,                       // ≤ 32 open PdfPage handles, keyed by index (FPDF_LoadPage is 0.5–26 ms)
    pub touched_pages: BTreeSet<PageIndex>,       // pages with annotations created/edited since load (AP generation before save)
    pub history: history::History,                // undo/redo snapshots (§6.8)
    pub sizes: Vec<PageGeom>,                     // from pages().page_sizes() + rotation + crop (0.23 ms / 14 pages)
}
```

Rules enforced inside `Registry` (they are the drop-order and cache gotchas from render.md/pages.md turned into code):

1. `Registry::page(doc, i) -> &PdfPage` opens through the LRU; eviction runs `FORM_OnBeforeClosePage` when the doc has a form; opening runs `FORM_OnAfterLoadPage` (pdfium-render does **not** call it; verified by grep) and `set_content_regeneration_strategy(Manual)`.
2. **`Registry::mutate(doc, f)`** is the only way to change a document: it flushes the page LRU *before* any structural op that uses raw handles (`FPDF_MovePages`, `FPDF_ImportPagesByIndex` invalidate pdfium-render's `PdfPageIndexCache`), runs `f`, bumps `generation`, refreshes `sizes`, marks touched pages and pushes a history snapshot. Every editing command goes through it, so "one edit = one snapshot = one generation" holds by construction.
3. `Registry::replace(doc, bytes)` (undo/redo/save): drop all pages, drop the old `PdfDocument`, load the new one — **pages before documents, always** (render.md §5: `drop(doc)` with a live page compiles and is a UAF).
4. `Registry::close(doc)` = same drop order, then remove from `index`.
5. `Pdfium` is bound exactly once, on the engine thread, and leaked; the fonts token for the bundled Korean font is loaded lazily per document (`fonts_mut()` needs `&mut PdfDocument` and no live page — done inside `mutate` after the LRU flush).

### 3.5 Priorities, stale drops, cancellation

* Two channels + `crossbeam_channel::select_biased!` (hi first). Long jobs (OCR apply, export, search) are
  submitted as **one `Cmd` per page** on the `lo` lane, so visible tiles interleave (tauri.md gotcha: a single
  queue made tiles wait ~30 ms behind a full-page render).
* Every tile request carries the webview's **viewport generation** in the `x-vg` request header (not in the URL,
  so URLs stay cacheable). `EngineHandle::send` stores the max per doc in an `AtomicU64`; before running a
  render `Cmd` the loop checks `view_gen >= latest - 1` and otherwise answers `409 Conflict` immediately
  (a dropped `UriSchemeResponder` would make the fetch hang until the webview times out — always respond).
* Jobs: `JobId = u64`; `Arc<AtomicBool>` per job in `app/jobs.rs`; each per-page `Cmd` checks the flag first.
  Progress goes over `tauri::ipc::Channel<JobEvent>` (42k–100k msg/s verified, scoped to the caller).

### 3.6 Memory limits (release, measured bases from render.md §6)

| Budget | Value | Mechanism |
|---|---|---|
| libpdfium + doc + open pages | ≈ 25 MiB for a 14-page doc | `PageLru` ≤ 32 pages |
| Rendered bitmaps in Rust | 0 retained | tiles are rendered, encoded, sent, dropped (peak = one 512×512 BGRA = 1 MiB, or one full page ≤ 2× = 7.4 MiB) |
| Webview canvases | ≤ 6 visible pages × 7.4 MiB @2× ≈ 45 MiB | virtualiser keeps viewport ± 1.5 screens |
| WebKit HTTP cache | managed by WebKit | immutable tile URLs (§4.3) |
| Text layers | ~100 KB/page, LRU 64 | `TextCache` |
| Thumbnails | ≤ 8 MiB PNG | `ThumbCache`, cleared on generation change per page |
| Undo snapshots | ≤ 256 MiB RAM, then spill to `<temp>/seepdf-history/<doc>/<n>.pdf`, ≤ 50 steps | `History` (§6.8) |
| OCR workers | ≤ 4 × ~110 MB | `tesseractPool` cap `min(4, cores-2)` |
| Export encode | N bitmaps in flight | bounded channel of N = cores-1 |

Target: idle RSS ≤ 400 MB with a 500-page document open (ux-research §7). macOS RSS is a high-water mark, so
the largest single allocation is bounded by tiling above 2× (render.md §6).

---

## 4. Rendering pipeline and transport

### 4.1 Protocol routes (`app/protocol.rs`, scheme `seepdf`)

| Route | Query | Response | Lane |
|---|---|---|---|
| `/tile` | `doc, gen, page, scale, rot(0/90/180/270), x, y, size` | `image/png` (RGBA, `Fast`+`Sub`), headers `Cache-Control: private, max-age=31536000, immutable`, `Access-Control-Allow-Origin: *`, `X-Render-Ms` | hi (visible) / lo (`&prefetch=1`) |
| `/thumb` | `doc, gen, page, w` | PNG, same caching; served from `ThumbCache` when present | lo |
| `/ocr` | `doc, gen, page, dpi` | PNG gray8 (300 DPI page 12–21 ms render) | lo |
| `/recent-thumb` | `id` | PNG from `<app_data>/thumbs/<id>.png`, read on a blocking `std::thread` (never on main) | — |
| anything else | | 404 | — |

`gen` in every URL makes URLs immutable: WebKit never revalidates, no ETag logic, no 304 gotcha (tauri.md gotcha 3).
A page edit bumps `gen` → new URLs → old cache entries are simply never requested again.
On Windows the origin is `http://seepdf.localhost/`; JS always builds URLs with `convertFileSrc('', 'seepdf')`
(`src/ipc/protocol.ts`, kept from the spike). CSP already lists both origins.

### 4.2 Render config (verified flags, render.md §2/§3)

```rust
fn view_config(scale: f32, rot: PdfPageRenderRotation, origin: Option<(Pixels, Pixels)>) -> PdfRenderConfig {
    let mut c = PdfRenderConfig::new()
        .scale_page_by_factor(scale)                 // page px = round(pt * scale), exactly what the JS side computes
        .rotate(rot, true)
        .render_form_data(true).render_annotations(true)
        .use_lcd_text_rendering(false)               // subpixel AA is wrong once composited in a canvas
        .set_format(PdfBitmapFormat::BGRA)           // never BGR (83.6 ms conversion)
        .set_reverse_byte_order(true)                // pdfium writes RGBA -> as_raw_bytes() needs no swizzle (0.16 ms)
        .set_clear_color(PdfColor::WHITE);
    if let Some((tx, ty)) = origin { c = c.set_origin(-tx, -ty); }   // tile path that KEEPS form fields
    c
}
```

* **Never** `translate/scale/clip` on the config (silently disables form drawing).
* Tile: `PdfBitmap::empty(size, size, BGRA)` + `render_into_bitmap_with_config` (pdfium clips; blank tiles cost 0.00 ms).
* `scale <= 2.0` → the "grid" is one tile of the whole page (≤ 2.3 ms). `scale > 2.0` → 512 px tiles. The JS
  `tiles.ts` has one function `tileGrid(pagePx, scale)` returning 1×1 when `scale <= 2`; there is one code path.
* Encoding: the `png` crate directly (`Compression::Fast`, `Filter::Sub`: 2.29 ms / 1.4 MB per full 2× page,
  ~0.2 ms per tile), never `image::PngEncoder` (Adaptive filter doubles the time). Dev builds get
  `[profile.dev.package."*"] opt-level = 3` so this is not 141–260 ms in `tauri dev`.
* Thumbnails: `scale_page_by_factor(w / page_width_pt)` — never `thumbnail(n)` (squashes to n×n and disables
  annotation rendering, pages.md §4).
* OCR image: `scale_page_by_factor(dpi / 72.0)`, `use_grayscale_rendering(true)`, format Gray.

### 4.3 Webview side (`src/viewer/PageBitmap.tsx`, `tiles.ts`)

One `<canvas>` per mounted page, CSS size = page display size × zoom, backing size × `devicePixelRatio`.
`useTiles(page, scale, rot)`:

1. Compute `grid = tileGrid(round(pt*scale), scale)` and the visible subset (viewport ∩ page, centre-out order).
2. For each tile: `fetch(tileUrl, { headers: { 'x-vg': String(viewGen) }, signal })` → `blob()` →
   `createImageBitmap(blob)` → `ctx.drawImage(bmp, tx, ty)` → `bmp.close()`. The AbortController is aborted when
   the page unmounts or `scale` changes.
3. On a scale change the old canvas content is **kept and CSS-scaled** until every visible tile of the new
   scale has drawn (the "placeholder ≤ 120 ms before sharp" rule); the first paint of a page that has never
   rendered draws the 200 px sidebar thumbnail scaled up if it is already in the JS `Map`.
4. Prefetch ±1 page on the `lo` lane after the visible set settles (120 ms debounce).

Measured basis: a 2560×1440 viewport at 4× needs ≈ 20 tiles ≈ 5–15 ms of engine time; at ≤ 2× one full page is 2.3 ms.

### 4.4 Coordinates (one matrix per page)

All IPC rectangles are **PDF user space, unrotated, points, y-up** (`Rect { l, b, r, t }`). `PageGeom`
carries `crop: Rect`, `rotation: 0|90|180|270`, `width_pt/height_pt` (display, rotated). The frontend has
exactly one function (unit-tested for all four rotations against the numbers in text.md §2):

```ts
// src/viewer/geometry.ts — page user space -> canvas pixels; matches FPDF_PageToDevice within 1 px (text.md §2)
export function pageToDevice(g: PageGeom, viewRot: 0|90|180|270, scale: number): Mat6 {
  const r = (g.rotation + viewRot) % 360, s = scale, { l, b, r: cr, t } = g.crop;
  switch (r) {
    case 90:  return [0,  s,  s, 0, -b * s, -l * s];
    case 180: return [-s, 0,  0, s,  cr * s, -b * s];
    case 270: return [0, -s, -s, 0,  t * s,  cr * s];
    default:  return [s,  0,  0, -s, -l * s,  t * s];
  }
}
```

Mouse → page space is the inverse; every tool, the text layer and the annotation layer use these two functions.

---

## 5. Frontend architecture

### 5.1 Dependencies (added when `package.json` is unfrozen)

`zustand@5` (state, 1.2 kB), `lucide-react` (icons, per-icon imports, stroke 1.75), `@tauri-apps/plugin-dialog`,
`@tauri-apps/plugin-store`, `@tauri-apps/plugin-window-state` (typings; the raw `invoke('plugin:…')` calls
already work), `vitest` + `jsdom` (dev). **Not** added: i18next (custom hook), react-window (custom scroller),
any CSS framework, any component kit. `tesseract.js` stays.

### 5.2 Stores (zustand, three files)

```ts
// src/stores/appStore.ts (A6) — per app, persisted through plugin:store `settings.json`
interface AppState {
  locale: 'ko' | 'en'; theme: 'system' | 'light' | 'dark'; os: 'macos' | 'windows' | 'linux';
  sidebar: { open: boolean; tab: 'thumbs' | 'outline' | 'annots' | 'search'; width: number };
  inspectorOpen: boolean;
  mode: Mode;                        // 'read' | 'annotate' | 'edit' | 'pages' | 'form'
  tool: Tool;                        // §5.6
  toolDefaults: Record<Tool, ToolStyle>;
  author: string;
  recents: RecentEntry[];            // mirrored from recents.json
}

// src/stores/docStore.ts (A4) — the document shown in THIS window
interface DocState {
  doc: DocInfo | null;
  view: { zoom: number | 'fit-page' | 'fit-width'; layout: 'single' | 'continuous' | 'two'; rot: 0|90|180|270; page: number; nightMode: 'off'|'dark'|'sepia' };
  viewGen: number;                   // bumped on scroll/zoom settle; sent as x-vg
  textLayers: Map<number, TextLayer>;
  annots: Map<number, AnnotInfo[]>;  // per page, replaced wholesale by every mutation result
  fields: FormField[];
  selection: TextSelection | AnnotSelection | ObjectSelection | null;
  search: { query: string; hits: SearchHit[]; current: number; running: boolean };
  pagesGrid: { selected: Set<number>; order: number[] /* model during drag */ };
}

// src/stores/jobStore.ts (A7) — running jobs shown in the status bar
interface JobState { jobs: Record<JobId, { kind: 'ocr'|'export'|'search'|'save'; done: number; total: number; page?: number }> }
```

`src/ipc/api.ts` wraps every command as a typed function (`api.createAnnotation(docId, page, spec): Promise<AnnotList>`);
components never call `invoke` directly. `mockInvoke.ts` swaps the transport for vitest.

### 5.3 Component tree

```
<App>                                   // decides Welcome vs Document from docStore.doc
 ├─ <TitleBar>   drag region · sidebar toggle · undo/redo · title menu · <ModeSwitcher/> · search · export · overflow
 ├─ <ToolStrip>  only in annotate/edit/form modes (40 px)
 ├─ <Body>
 │   ├─ <Sidebar>  <ThumbnailRail/> | <OutlinePanel/> | <AnnotationList/> | <SearchPanel/>
 │   ├─ mode === 'pages' ? <PagesGrid/> :
 │   │   <Viewer>
 │   │     <Scroller>                   // virtualised; owns scroll, zoom, layout, keyboard nav
 │   │       <PageView i>               // absolutely positioned; mounted for viewport ± 1.5 screens
 │   │         <PageBitmap/>            // <canvas>, tiles
 │   │         <TextLayer/>             // <svg> selection + search highlight rects (no spans)
 │   │         <AnnotationLayer/>       // <svg> hit-rects, handles, drag ghost, in-progress shape
 │   │         <FormLayer/>             // <input>/<select> overlays (form mode or when focused)
 │   │         <ToolOverlay/>           // pointer capture for the active tool
 │   └─ <Inspector/>                    // right panel, content by selection/tool
 ├─ <StatusBar>  page nav · layout · rotate view · zoom · save state · <JobProgress/>
 ├─ <Dialogs/>   OCR · Export · Password · Unsaved · Settings · MergePrompt (one mounted at a time)
 ├─ <ContextMenu/> <Toasts/> <NotePopover/>
 └─ <PrintView/>  @media print only
<Welcome>  actions column · recents grid (thumbnails via /recent-thumb) · drop zone
```

### 5.4 Virtualised scroller (`Scroller.tsx`, `useVirtualPages.ts`, ~200 lines)

* Layout is computed from `DocInfo.pages[].{widthPt,heightPt}` alone (available at open time, 0.23 ms for 14
  pages, no pixels needed): `layoutPages(pages, layout, zoom, viewRot, gap = 16) -> { tops[], lefts[], total }`.
* The scroll container is one `div` with `height: total`; `PageView`s are absolutely positioned; the mounted
  set is `pages whose [top, bottom] intersects [scrollTop - 1.5*vh, scrollTop + 2.5*vh]`.
* Zoom keeps the point under the cursor fixed (`scrollTop' = (scrollTop + y) * z'/z - y`); pinch = `wheel` with
  `ctrlKey` in WKWebView/WebView2; keyboard ⌘+/− steps through 25/50/75/100/125/150/200/400.
* "fit-width"/"fit-page" recompute on resize (`ResizeObserver`).
* Scroll settle (120 ms) bumps `viewGen`, updates `view.page` (the page covering the viewport centre) and
  triggers prefetch.
* Space taps page-down, Space-hold + drag pans (200 ms threshold) — the ux-research conflict resolution.

### 5.5 Text layer, selection, search (`TextLayer.tsx`, `selection.ts`)

`TextLayer` = `{ text: string; boxes: number[] /* 4 per char: l,b,r,t in page space; 0-size for generated chars */; lines: number[] /* [start,end) pairs */ }`
fetched lazily per mounted page (`get_text_layer`, 5.6 ms/page incl. grouping, cached both sides).
`Array.from(text)` gives code-point indexing that matches `boxes` 1:1.

* Selection = `{ page, start, end }` char range (multi-page = one range per page). Hit-testing: nearest char by
  baseline line then x, against `boxes` transformed by `pageToDevice` — pure JS, no round trips, ≤ 16 ms/update.
* Highlight rects = union of loose boxes per line run (`lines` makes this a loop, ~40 µs).
* Copy = `Array.from(text).slice(start, end).join('')` with `\r\n` → `\n`.
* Search runs in Rust over cached text layers (char indices, case-folded on `char`, 65 µs/page), pages ordered
  from the current page outward, streamed per page over a `Channel<SearchEvent>`; the panel renders results as
  they arrive; `⌘G` steps. No pdfium search (rect-only results, text.md §3).
* Text-markup tools (highlight/underline/strikeout) turn the selection's line-run rects into quads
  (TL,TR,BL,BR order — annotations.md gotcha 2 — built in Rust from the rects, the frontend never sees quads order).

### 5.6 Tool modes and the annotation overlay

`Mode = 'read' | 'annotate' | 'edit' | 'pages' | 'form'`;
`Tool = 'select' | 'hand' | 'highlight' | 'underline' | 'strikeout' | 'note' | 'ink' | 'eraser' | 'rect' | 'ellipse' | 'line' | 'arrow' | 'textbox' | 'stamp' | 'signature' | 'addText' | 'addImage' | 'redact'`.

`ToolOverlay` captures the pointer and runs the active tool's small state machine
(`tools/*.ts`, each ≈ 60–120 lines, pure functions `onDown/onMove/onUp(state, pt) -> { state, preview?: SvgNode, commit?: AnnotSpec }`,
unit-testable). `AnnotationLayer` draws (a) transparent hit rects for every `AnnotInfo` of the page,
(b) selection handles, (c) the tool's `preview`, (d) a 70 % drag ghost while moving/resizing.
On `commit` → `api.createAnnotation`/`updateAnnotation` (optimistic: the preview stays until the mutation result
arrives, then the store swaps in the new `annots` and the next tile fetch with the new `gen` paints the pdfium
appearance; the preview is removed on the tile's `onload`, so nothing flickers).

Sticky-note popups, note icons' hover text, and link tooltips are React (`NotePopover`), never pdfium
(annotations.md gotcha 11).

### 5.7 Undo/redo in the UI

`⌘Z`/`⇧⌘Z` → `api.undo(docId)` → new `DocInfo` (+ `generation`) → the store invalidates `annots`, `fields`,
`textLayers` and every mounted page re-fetches tiles with the new `gen`. Nothing in the frontend keeps an undo
stack; the engine is the single source of truth (D5). Selection is cleared on undo.

### 5.8 i18n (`src/i18n/index.ts`, ~60 lines)

```ts
export type Locale = 'ko' | 'en';
const catalogs: Record<Locale, Record<string, string>> = { ko, en };   // flat JSON, 373 keys from ux-research §5
export function t(key: string, params?: Record<string, string | number>): string {
  const loc = useAppStore.getState().locale;
  const k = params?.count !== undefined && loc === 'en' && params.count !== 1 && catalogs.en[`${key}_other`] ? `${key}_other` : key;
  const s = catalogs[loc][k] ?? catalogs.ko[k] ?? key;
  return s.replace(/\{\{(\w+)\}\}/g, (_, n) => String(params?.[n] ?? ''));
}
export const useT = () => { useAppStore((s) => s.locale); return t; };   // re-render on locale change
```

`scripts/check-i18n.mjs` fails CI when `ko.json`/`en.json` key sets differ or a `{{param}}` differs.
The same JSON files are `include_str!`'d by `src-tauri/src/app/menu.rs` for the native macOS menu, so there is
one catalogue. Numbers/dates via `Intl`.

### 5.9 Theme and Korean typography

`tokens.css` = ux-research §6 verbatim (light on `:root`, dark under `:root[data-theme="dark"]` and
`@media (prefers-color-scheme: dark) :root:not([data-theme="light"])`). `data-os` on `<html>` sets
`padding-left: 78px` for the traffic lights and `font-size: 14px` on Windows. Pretendard Variable Korean subset
(`public/fonts`, `font-display: swap`, system stack fallback); body text `word-break: keep-all; line-height: 1.45`;
UI weights 400/500/600 only. Night mode = `filter: invert(1) hue-rotate(180deg)` on `<canvas>` only, never on the
SVG layers.

### 5.10 Keyboard

`src/keys/keymap.ts` is a data table `{ id, mac, win, when: 'always'|'canvas'|'pages'|'textEditing', run }`
covering ux-research §4.7 in full; `useKeymap()` is one `keydown` listener on `window` that resolves the
platform chord, checks `when` (no shortcuts while an `<input>`/`contenteditable` is focused except Esc/⌘S/⌘Z),
and implements Figma-style sticky tools (tap = latch, hold > 200 ms = momentary). The native macOS menu emits
`menu:<id>` events with the same ids, so the menu and the keymap share one action table.

### 5.11 Welcome, recents, file opening, drag & drop, windows

* `useOpenFiles()` (A6) subscribes to: `take_pending_opens` on mount, the `open-file` event, `onDragDropEvent`
  drops, ⌘O (`plugin:dialog|open`, `multiple: true`), and Welcome's card clicks. Multiple files → `MergePrompt`
  (각각 열기 / 하나로 합치기). Every path goes through `api.openDocument(path)`; a `password_required` error opens
  `PasswordDialog` and retries with `{ password }`.
* **One window per document.** The first window (`main`) shows Welcome; opening a file into a window that has a
  document asks Rust (`open_in_new_window`) to create `doc-<n>` (`WebviewWindowBuilder`, `index.html?win=doc-n`);
  the capability `windows` list becomes `["main", "doc-*"]`. `app/windows.rs` tracks `label → Option<DocId>`
  (`window_bind_document` command) so Finder opens land in the empty window if one exists, else in a new one.
* Recents (`recents.json` via plugin:store): `{ path, name, dir, pages, bytes, lastOpened, lastPage, zoom, layout, pinned, thumbId }`.
  On close the engine writes `<app_data>/thumbs/<thumbId>.png` (200 px wide, first page) so the Welcome grid
  works without opening documents. Relative dates (오늘/어제/3일 전) via `Intl.RelativeTimeFormat`.
* Title shows `• name` when dirty (tauri 2.11.5 has no `set_document_edited`; `set_title` exists).

---

## 6. Document editing model

Every mutation is `Registry::mutate` (§3.4): flush page LRU → edit → bump generation → snapshot. The result of
every mutating command includes the new `generation` plus the data the UI needs to refresh (`AnnotList`,
`FormField`, or a full `DocInfo` for structural changes), so no separate "refresh" round trip is needed.

### 6.1 Annotations (`crates/engine/src/annot.rs`)

Type mapping (what the user sees → what is written):

| Tool | PDF annotation | How (verified recipe) |
|---|---|---|
| 형광펜 / 밑줄 / 취소선 | Highlight / Underline / StrikeOut | high-level `create_*_annotation`, quads TL,TR,BL,BR per line run, `set_bounds(union)`, colour with alpha via `set_stroke_color(PdfColor::new(r,g,b,a))` **before** any render, `set_is_printed(true)`, `/NM` uuid, `/T` author, `/Subj`, `/Contents` |
| 메모 | Text | `create_text_annotation(contents)`, `set_bounds(20×20 at point)`, colour |
| 펜 | Ink with `/InkList` | raw: `FPDFPage_CreateAnnot(INK)`, `SetRect`, `SetColor(C, rgba)`, `SetBorder(0,0,width)`, `AddInkStroke` per stroke, `SetFlags(PRINT)` |
| 지우개 | — | deletes whole Ink annotations under the cursor (v1 scope; partial stroke erasing is P2) |
| 사각형 | Square | high-level `create_square_annotation` + bounds + `/C`/`/IC` with identical alpha + raw `SetBorder` |
| 타원 | Circle | raw `FPDFPage_CreateAnnot(CIRCLE)` then the same setters (no high-level constructor in 0.9.4) |
| 선 / 화살표 | Ink (`/Subj SeePDF:Line` / `SeePDF:Arrow`) | one stroke [p1,p2] (+ one 3-point head stroke for arrows); PDFium cannot create Line annots |
| 텍스트 상자 | Stamp (`/Subj SeePDF:TextBox`) + text object(s) | `create_stamp_annotation`, `set_bounds(rect)` **first**, then `PdfPageTextObject::new(doc, line, font, size)` per line (Helvetica if Latin-1 only, else the bundled Korean font token), `set_fill_color`, `translate` **before** `add_text_object`; `/Contents` = the text so the annotation list and other viewers show it; edit = `FPDFAnnot_RemoveObject` all + rebuild |
| 도장 / 서명(이미지) | Stamp + image object | `PdfPageImageObject::new_with_size` + `translate` before `add_image_object` |
| 서명(그리기) | Ink (`/Subj SeePDF:Signature`) | as 펜, black, width 1.5 |

Invariants implemented once in `annot::finish_create(page, annot)`: Print flag, `/NM`, `/T`, `/CreationDate`
(pdfium-render stamps it), `touched_pages.insert(page)`. Before every save, touched pages are rendered once at
scale 0.05 (≈ 0.3 ms) so pdfium writes the `/AP` streams (annotations.md gotcha 3) — otherwise Preview/Acrobat
show nothing.

Editing existing annotations (`annot::update`): move/resize → `set_bounds` (+ `set_attachment_point_at_index`
for markup); colour/opacity/width → **raw `FPDFAnnot_SetAP(annot, 0, NULL)` first**, then `SetColor`/`SetBorder`
(never `set_fill_color`/`set_stroke_color` on an annotation with an AP: SIGSEGV, annotations.md gotcha 1);
contents/author → high-level. Deleting: `annotations_mut().get(i)` → `delete_annotation`.

Reading (`annot::list`): high-level common props + raw `GetNumberValue("CA")`, `GetBorder`, `GetInkListPath`,
`GetLine` (existing Line annots from other producers are `Unsupported` in pdfium-render but still readable and
rendered). `editable = true` for the types above; widgets/popups/others are listed as read-only rects.

### 6.2 Forms (`forms.rs`)

`list_fields`: for each page's widget annotations, `as_form_field()` → name, type, rect, value/options/checked.
`set_value` (all raw on `doc.form().handle()` + `page.page_handle()`; the page came through the LRU, so
`FORM_OnAfterLoadPage` ran):

```
text/combo-edit:  FORM_SetFocusedAnnot(form, annot)  → FORM_SelectAllText(form, page) → FORM_ReplaceSelection(form, page, utf16)  [unverified combo]
                  → FORM_ForceToKillFocus(form)                      (verified: /V + /AP regenerated, persists, Korean OK)
checkbox/radio:   FORM_OnLButtonDown/Up(form, page, 0, cx, cy) at the widget centre; read back FPDFAnnot_IsChecked
combo/list:       FORM_SetFocusedAnnot → FORM_SetIndexSelected(form, page, i, true) per index → ForceToKillFocus
```

`FORM_SelectAllText` + `FORM_ReplaceSelection` are in the bindings but were not exercised by the spike (the spike
typed with `FORM_OnChar`); `forms.rs`'s DoD test covers it, with `OnChar`-per-character as the fallback.
The webview shows a native `<input>`/`<select>` positioned by `pageToDevice(rect)` for the focused field only;
blur/Enter commits; the tile re-render (widgets are drawn by `FPDF_FFLDraw` inside the bitmap) replaces it.
Signature fields and push buttons with JS/URI actions are read-only (pdfium-render leaves the FFI callbacks `None`).

### 6.3 Page operations (`pages.rs`)

| Op | Call | Note |
|---|---|---|
| rotate | `page.set_rotation(Degrees90…)` with `Manual` strategy, one `regenerate_content()` per page | 2.8 ms/page otherwise |
| delete | `page.delete()` **descending by index** | 2.2 ms each |
| move | raw `FPDF_MovePages(doc, &indices, len, dest)` after LRU flush; validate (no dups, in range, `dest + len <= count`) in Rust; a `false` return → reload the document from the last snapshot | 0.02 ms |
| duplicate | raw `FPDF_ImportPagesByIndex(doc, doc, &[i], 1, i + 1)` | verified 14→15 |
| insert blank | `pages_mut().create_page_at_index(PdfPagePaperSize::from_points(w, h), i)` | size = neighbour page |
| insert from file | open the source with `load_pdf_from_file` into a temporary `PdfDocument` → `copy_pages_from_document(&src, "1-3,5", at)` (1-based string exactly as typed in the dialog) | drop the temp doc after |
| extract | `save_to_bytes()` → temp doc → delete unwanted pages → `save_to_file(out)` | keeps `/AcroForm`, outline, metadata (import loses them) |
| merge | `create_new_pdf()` + `copy_pages_from_document` per source | UI warns `pages.merge.formWarning` when a source `form().is_some()` |

### 6.4 Text and image objects (P1, `objects.rs`)

`add_text_object(page, x, y, text, size, color)`: one `PdfPageTextObject` per line (Helvetica or the bundled
Korean font), `Manual` strategy, `regenerate_content()`. `add_image_object`: `create_image_object(x, y, &img, Some(w), None)`.
`list_objects` returns only objects tagged as ours (`/SeePDF` content mark is P2; in v1 we tag by keeping the
object indices we created in `OpenDoc.owned_objects`), `move_object` = `translate`, `delete_object` = `remove_object_at_index`
(indices shift; re-list after). No editing of pre-existing text (§1.4).

### 6.5 OCR layer (`ocr/layer.rs`) — see §7.

### 6.6 Redaction (P1, `redact.rs`)

Input: page rects. Steps (approach A, verified): for each text object whose `bounds()` intersect a rect →
`remove_object_at_index` (descending); for each image object intersecting → `get_raw_image()`, blank the
intersecting pixel region (rect mapped through the inverse of `matrix()`), `set_image()` (matrix preserved,
verified); add `create_path_object_rect(rect, None, None, Some(BLACK))`; `regenerate_content()`; then
re-extract text and assert no char box intersects the rect (fail the command otherwise — never ship a fake
redaction). The UI shows exactly which text runs will be removed (whole objects, e.g. "Andreas Gal" for "Gal")
before 적용, because PDFium cannot split a text object; word-level re-creation is P2.

### 6.7 Save / Save As / dirty tracking (`save.rs`)

```
save(doc, path):
  1. render every page in touched_pages at scale 0.05  (AP generation)
  2. bytes = doc.save_to_bytes()                        (full rewrite, flags 0; 5 ms/MB)
  3. tmp = path + ".seepdf-tmp"; write; fsync
  4. verify: pdfium.load_pdf_from_file(tmp)?.pages().len() == doc.pages().len()
  5. std::fs::rename(tmp, path)                          (atomic on the same volume; replaces on Windows too)
  6. Registry::replace(doc, bytes); saved_generation = generation; history keeps its stack
```

Read-only target / permission error → `EngineError::Io` → the UI falls through to Save As with
`dialog.saveAs.readOnly`. Dirty = `generation != saved_generation` (in `DocInfo`). Encrypted documents opened
with a password are saved encrypted (flags 0 keeps encryption, verified); P1 "remove password" uses
`save_to_bytes_with_flags(FPDF_REMOVE_SECURITY = 4)` — note the value is 4, not 3 (pages.md §8).

### 6.8 History (`history.rs`, D5)

```rust
pub struct History { undo: Vec<Snapshot>, redo: Vec<Snapshot>, ram_bytes: usize, dir: PathBuf }
enum Snapshot { Ram(Arc<[u8]>), Disk(PathBuf) }          // spill oldest to disk above 256 MiB; ≤ 50 steps
impl History {
    pub fn push(&mut self, bytes: Arc<[u8]>);            // called by Registry::mutate BEFORE the edit (bytes = current state)
    pub fn undo(&mut self, current: Arc<[u8]>) -> Option<Arc<[u8]>>;   // moves `current` to redo
    pub fn redo(&mut self, current: Arc<[u8]>) -> Option<Arc<[u8]>>;
}
```

`mutate` obtains the "current state" bytes with `save_to_bytes()` *before* applying the edit (so the snapshot
never contains unrendered-AP surprises: pdfium regenerates APs on load anyway). Undo = `Registry::replace(doc, prev)`
(0.15 ms lazy load) + generation bump → every layer refreshes. Cost per edit: 5 ms/MB; for a 100 MB scan that is
~0.5 s of engine time per edit, listed as a known v1 limitation with the incremental-delta optimisation
(`FPDF_INCREMENTAL` output = original + small delta, 0.2 ms) as the documented follow-up behind the same `push()` seam.

### 6.9 Export and print (`export.rs`, `PrintView.tsx`)

* Images: per page `Cmd` on the `lo` lane renders (`scale = dpi/72`, RGBA), hands the buffer to a bounded
  `std::sync::mpsc` consumed by `cores-1` encoder threads (`png` Fast+Sub, or `image::codecs::jpeg::JpegEncoder`
  with alpha stripped), writes `<stem>-<nnn>.{png,jpg}`; progress per page; cancel flag checked per page.
  Measured: 14 pages @150 DPI = 65 ms render + 62 ms encode (release).
* Text: `page.text()?.all()` per page, `\r\n → \n`, form-feed between pages (1.4 ms/page).
* Flattened PDF: `save_to_bytes()` → temp doc → per page raw `FPDFPage_Flatten(page, FLAT_NORMALDISPLAY)` +
  `FPDFPage_GenerateContent` + reload page → `save_to_file(out)` (never `PdfPage::flatten()` = `FLAT_PRINT`,
  which deletes non-printable annotations).
* Print: `PrintView` renders all pages as `<img src=/tile … scale=150/72>` under `@media print`, then
  `getCurrentWebview().print()` (permission `core:webview:allow-print`, verified to exist in 2.11.5).

---

## 7. OCR pipeline (D9)

```
OcrDialog ──► ocrJob.ts (JS orchestrator, one page at a time per worker, cancellable)
   for page in range:
     if !force && (await api.ocrPageStatus(doc, page)).hasText  → skip           // page.text().len() > 50
     engine = platform === 'macos' && visionAvailable ? 'vision' : 'tesseract'
     vision:    OcrPage = await api.ocrVisionRecognize({doc, page, dpi, langs})   // Rust: render BGRA on engine thread → seepdf-ocr thread → CGImage → VNRecognizeTextRequest(.accurate, ["ko-KR","en-US"])
     tesseract: blob = await fetch(seepdf /ocr?doc&gen&page&dpi) ; bmp = createImageBitmap(blob)
                res = await pool.recognize(bmp, { psm })  → normalize.ts → OcrPage
     progress: jobStore
   await api.ocrApply({doc, pages: OcrPage[], onProgress})                       // ONE undo step, per-page Cmds on the lo lane
```

* `OcrPage` (engine-agnostic, `src/ocr/types.ts` == `crates/engine/src/types.rs`):
  `{ page, dpi, widthPx, heightPx, lines: [{ text, bbox, baseline?, words: [{ text, bbox, confidence }] }] }`,
  pixel boxes, origin top-left, in the **rendered (display-rotated)** image.
* `ocr/layer.rs` (verified chain, ocr.md §5): `pixels_to_points(x, y, &same_config)` per box (handles `/Rotate`),
  font size = line height in pt, per-word `scale(box_w / natural_w, 1.0)` clamped [0.5, 2.0] **before**
  `translate`, `set_render_mode(Invisible)`, Helvetica for Latin-1-only words, the bundled Korean font otherwise
  (one `load_true_type_from_bytes(bytes, true)` per document, ≈ +1–1.5 MB once), spaces rebuilt from `line.text`
  (Tesseract over-segments Hangul), words with confidence < 30 skipped, `Manual` regeneration + one
  `regenerate_content()` per page (2 ms for 730 words vs 81 ms automatic), rotated pages get
  `rotate_counter_clockwise_degrees(rotation)`.
* tesseract.js: `createWorker(['kor','eng'], OEM.LSTM_ONLY, { workerPath: '/ocr/worker.min.js', corePath: '/ocr/', langPath: '/ocr/', gzip: true, cacheMethod: 'none', workerBlobURL: false })`,
  `setParameters({ tessedit_pageseg_mode: psm, user_defined_dpi: '300', preserve_interword_spaces: '1' })`,
  `recognize(img, {}, { text: true, blocks: true })`. Always `kor+eng`. Layout option 자동/단일 단/단일 블록 (PSM 3/4/6)
  in the dialog's 고급 disclosure. `worker-src 'self' blob:` and `script-src 'self' 'wasm-unsafe-eval'` added to the CSP **[unverified in the Tauri window]**.
* Vision: `objc2-vision 0.3.2` + `objc2`, `objc2-foundation`, `objc2-core-foundation`, `objc2-core-graphics`
  as `[target.'cfg(target_os = "macos")'.dependencies]` **[unverified compile]**; availability =
  `supportedRecognitionLanguagesAndReturnError` contains `ko-KR` (macOS 13+). Two-column pages get a column sort
  by x-overlap clustering in `normalize.ts` (both engines).
* Dialog fields and strings: ux-research §4.11 / §5.11. Estimated time = pages × (Vision 0.5 s | Tesseract 2 s).
* Bundle impact: +8.4 MB (tesseract runtime + eng + kor) + ~1.5 MB font; Vision 0 bytes.

---

## 8. IPC contract (`src/ipc/types.ts` — the frozen interface)

Conventions: commands are `snake_case` in Rust, arguments `camelCase` (serde `rename_all`), every command
returns `Result<T, EngineError>`; JS receives the error object on rejection. Rects are page user space.

```ts
// ---------- errors ----------
export interface EngineError { code: 'password_required' | 'password_wrong' | 'not_found' | 'io' | 'pdfium' | 'unsupported' | 'invalid' | 'cancelled' | 'busy'; message: string }

// ---------- geometry ----------
export interface Rect { l: number; b: number; r: number; t: number }      // PDF points, unrotated user space
export type Mat6 = [number, number, number, number, number, number];
export type Rotation = 0 | 90 | 180 | 270;
export type RGB = [number, number, number];

// ---------- documents ----------
export interface PageGeom { index: number; widthPt: number; heightPt: number; rotation: Rotation; crop: Rect; label: string | null }
export interface Permissions { print: boolean; modify: boolean; extractText: boolean; annotate: boolean; fillForms: boolean; assemble: boolean }
export interface DocInfo {
  id: string; path: string | null; name: string; bytes: number; pageCount: number; pages: PageGeom[];
  generation: number; dirty: boolean; canUndo: boolean; canRedo: boolean;
  encrypted: boolean; permissions: Permissions; hasForm: boolean; hasOutline: boolean;
  meta: { title?: string; author?: string; subject?: string; keywords?: string; creator?: string; producer?: string; created?: string; modified?: string };
}
export interface OutlineNode { title: string; page: number | null; children: OutlineNode[] }

// open_document({ path, password? }) -> DocInfo           errors: password_required | password_wrong | not_found | pdfium
// close_document({ docId }) -> void                      (writes the recents thumbnail first)
// get_document({ docId }) -> DocInfo
// get_outline({ docId }) -> OutlineNode[]
// new_document_from_files({ paths }) -> { doc: DocInfo; warnings: string[] }   merge; warnings name sources whose /AcroForm or outline is lost
// open_in_new_window({ path }) -> string /* window label */
// window_bind_document({ label, docId: string | null }) -> void

// ---------- text ----------
export interface TextLayer { text: string; boxes: number[]; lines: number[] }
export interface SearchHit { page: number; start: number; length: number; rects: Rect[] }
export type SearchEvent = { type: 'page'; page: number; hits: SearchHit[] } | { type: 'done'; total: number } | { type: 'cancelled' };
// get_text_layer({ docId, page }) -> TextLayer
// get_page_text({ docId, page }) -> string
// search({ docId, query, matchCase, wholeWord, fromPage, onEvent: Channel<SearchEvent> }) -> JobId
// cancel_job({ jobId }) -> boolean

// ---------- annotations ----------
export type AnnotType = 'highlight' | 'underline' | 'strikeout' | 'squiggly' | 'ink' | 'square' | 'circle' | 'text' | 'freetext' | 'stamp' | 'link' | 'widget' | 'line' | 'popup' | 'other';
export interface AnnotRef { page: number; index: number }               // valid until the next generation change
export interface AnnotInfo {
  index: number; type: AnnotType; seepdf: 'textbox' | 'line' | 'arrow' | 'signature' | 'stamp' | null;   // from /Subj
  rect: Rect; quads?: Rect[]; inkPaths?: number[][] /* [x0,y0,x1,y1,…] */; linePoints?: [number, number, number, number];
  contents?: string; author?: string; created?: string; modified?: string; name?: string;
  color?: RGB; fillColor?: RGB; opacity: number; borderWidth?: number; fontSize?: number;
  hidden: boolean; printed: boolean; editable: boolean;
}
export interface AnnotList { page: number; generation: number; annots: AnnotInfo[] }
export type AnnotSpec =
  | { kind: 'highlight' | 'underline' | 'strikeout' | 'squiggly'; rects: Rect[]; color: RGB; opacity: number; contents?: string }
  | { kind: 'note'; at: [number, number]; color: RGB; contents: string }
  | { kind: 'ink'; paths: number[][]; color: RGB; width: number; opacity: number; subj?: 'signature' }
  | { kind: 'square' | 'circle'; rect: Rect; stroke: RGB; fill: RGB | null; width: number; opacity: number }
  | { kind: 'line' | 'arrow'; p1: [number, number]; p2: [number, number]; stroke: RGB; width: number; opacity: number }
  | { kind: 'textbox'; rect: Rect; text: string; fontSize: number; color: RGB; align: 'left' | 'center' | 'right' }
  | { kind: 'stamp'; rect: Rect; image: { path: string } | { builtin: string } };
export interface AnnotPatch { rect?: Rect; rects?: Rect[]; inkPaths?: number[][]; p1?: [number, number]; p2?: [number, number];
  color?: RGB; fillColor?: RGB | null; opacity?: number; borderWidth?: number; contents?: string; text?: string; fontSize?: number }
// list_annotations({ docId, page }) -> AnnotList
// create_annotation({ docId, page, spec, author }) -> AnnotList
// update_annotation({ docId, ref, patch }) -> AnnotList
// delete_annotation({ docId, ref }) -> AnnotList

// ---------- forms ----------
export type FieldType = 'text' | 'checkbox' | 'radio' | 'combo' | 'list' | 'button' | 'signature' | 'unknown';
export interface FormField { page: number; index: number; name: string; type: FieldType; rect: Rect; readOnly: boolean;
  value?: string; checked?: boolean; options?: { label: string; selected: boolean }[]; multiline?: boolean; maxLen?: number }
export type FieldValue = { text: string } | { checked: boolean } | { selected: number[] };
// list_form_fields({ docId }) -> FormField[]
// set_form_value({ docId, page, index, value: FieldValue }) -> { field: FormField; generation: number }

// ---------- pages ----------
// rotate_pages({ docId, pages, delta: 90 | -90 | 180 }) -> DocInfo
// delete_pages({ docId, pages }) -> DocInfo
// move_pages({ docId, pages, dest }) -> DocInfo                   FPDF_MovePages semantics: ordered list inserted at dest
// insert_blank_page({ docId, at, size?: { w: number; h: number } }) -> DocInfo
// insert_pages_from_file({ docId, at, path, range?: string /* "1-3,5", 1-based */ }) -> DocInfo
// duplicate_pages({ docId, pages }) -> DocInfo
// extract_pages({ docId, pages, outPath }) -> void
// split_document({ docId, mode: { every: number } | { ranges: string[] }, outDir }) -> string[]      (P1)

// ---------- objects (P1) ----------
export interface PageObject { index: number; type: 'text' | 'image' | 'path' | 'other'; rect: Rect; ours: boolean }
// add_text_object({ docId, page, x, y, text, fontSize, color }) -> { objects: PageObject[]; generation: number }
// add_image_object({ docId, page, rect, path }) -> { objects; generation }
// list_page_objects({ docId, page }) -> PageObject[]
// move_page_object({ docId, page, index, dx, dy }) -> { objects; generation }
// delete_page_object({ docId, page, index }) -> { objects; generation }
// redact_areas({ docId, page, rects }) -> { removedObjects: number; generation: number }           (P1)

// ---------- OCR ----------
export interface OcrWord { text: string; bbox: [number, number, number, number]; confidence: number }
export interface OcrLine { text: string; bbox: [number, number, number, number]; baseline?: [number, number, number, number]; words: OcrWord[] }
export interface OcrPage { page: number; dpi: number; widthPx: number; heightPx: number; lines: OcrLine[] }
// ocr_page_status({ docId, pages }) -> { page: number; hasText: boolean }[]
// ocr_vision_available() -> { available: boolean; languages: string[] }
// ocr_vision_recognize({ docId, page, dpi, languages }) -> OcrPage
// ocr_apply({ docId, pages: OcrPage[], onProgress: Channel<JobEvent> }) -> DocInfo      one undo step

// ---------- history / save / export ----------
export type JobEvent = { type: 'started'; jobId: number; total: number } | { type: 'progress'; jobId: number; done: number; total: number; page?: number }
  | { type: 'done'; jobId: number; elapsedMs: number; outputs?: string[] } | { type: 'cancelled'; jobId: number; done: number } | { type: 'error'; jobId: number; error: EngineError };
// undo({ docId }) -> DocInfo ; redo({ docId }) -> DocInfo
// save_document({ docId }) -> DocInfo ; save_document_as({ docId, path }) -> DocInfo
// export_images({ docId, pages, dir, format: 'png' | 'jpeg', dpi, quality, onProgress }) -> JobId
// export_text({ docId, pages, path }) -> void
// export_flattened({ docId, path, onProgress }) -> JobId
// remove_password({ docId, path }) -> void                                                (P1)

// ---------- files / windows / misc ----------
// take_pending_opens() -> string[]
// write_recent_thumbnail({ docId }) -> string /* thumbId */
// reveal_in_file_manager({ path }) -> void            (plugin:opener reveal_item_in_dir)
// Dialogs: invoke('plugin:dialog|open' | 'plugin:dialog|save', { options })   (verified shapes in tauri.md §4)
// Store:   invoke('plugin:store|…') through @tauri-apps/plugin-store: settings.json, recents.json, tool-defaults.json
```

Events (`src/ipc/events.ts`): `open-file { path, source }` (Finder/argv/Dock, queued by `PendingOpens`),
`doc-changed { docId, generation, reason: 'edit'|'undo'|'redo'|'save'|'pages' }` (broadcast, used by every
window that shows that doc), `menu:<id>` (native menu → focused window), `recents-changed`.
Channels (per call): `search`, `export_images`, `export_flattened`, `ocr_apply`.

Protocol (`src/ipc/protocol.ts`): `tileUrl(doc, gen, page, scale, rot, x, y, size)`, `thumbUrl(doc, gen, page, w)`,
`ocrImageUrl(doc, gen, page, dpi)`, `recentThumbUrl(id)` — all built on `convertFileSrc('', 'seepdf')`.

Rust mirror: `crates/engine/src/types.rs` holds the same structs with `#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]`; enums with data use `#[serde(tag = "kind"/"type", rename_all = "camelCase", rename_all_fields = "camelCase")]`
(the `rename_all_fields` gotcha from tauri.md). A vitest test deserialises the JSON fixtures in `src/test/ipc-samples/`
against both sides' expectations.

---

## 9. Testing and CI

### 9.1 Rust (`cargo test --workspace`, engine tests are headless and use the real actor)

`crates/engine/src/test_support.rs`: `pub fn engine() -> &'static EngineHandle` (`OnceLock`; binds
`../../resources/pdfium/<host lib>` once — `bind_to_library` works once per process — and the engine thread
serialises all tests' pdfium calls, so `cargo test` may stay parallel). `pub fn fixture(name) -> PathBuf`.

| Test file | What it asserts (all numbers from the spikes) |
|---|---|
| `tests/smoke.rs` | every `fixtures/*.pdf` + `fixtures/gen/*.pdf`: open, page count, `page_sizes`, tile (0,0) @1×, page 0 @2× PNG decodes to `round(pt*2)` px, text layer builds, annotations list, `save_to_bytes` → reopen same page count |
| `tests/render.rs` | tile at 8× equals the crop of the full render within max channel delta 1 (427/1,048,576 px); rotation.pdf page 1 renders 792×612 @1×; thumbnails keep aspect |
| `tests/text.rs` | tracemonkey p1 layer = 5,087 chars / 89 lines; `pageToDevice` for "Trace-based" @2× = (161,158)-(348,190); search "monkey" = 62 hits with char indices; rotation.pdf p2 char box matches `points_to_pixels` within 1 px |
| `tests/annot.rs` | each `AnnotSpec` kind → create → render → save → reopen → present, bounds equal, ≥ N changed pixels; recolour after reopen does not crash; delete persists; QuadPoints order TL,TR,BL,BR; text box with "한글" renders ≥ 100 dark px |
| `tests/forms.rs` | 160F-2019.pdf: 64 text / 10 button / 2 radio fields listed; `set_form_value` text "Raw FORM 입력" → `FPDFAnnot_GetFormFieldValue` equal, ≥ 100 changed px with FFLDraw, persists after save/reopen; radio click toggles `IsChecked` |
| `tests/pages.rs` | move [3,2]→1 gives [A,D,C,B] (text of page 1 == old page 3); delete descending; duplicate 14→15; split-by-delete keeps `form()==Some` on 160F; merge keeps annotations (annotation-highlight ×2); 1-based range string "1-3,5" copies pages 1,2,3,5 |
| `tests/history.rs` | 20 random edits → 20 undos → bytes == snapshot 0; redo restores; budget spills to disk at 256 MiB (test with a 4 MB budget); save then undo still works |
| `tests/ocr_layer.rs` | apply a synthetic `OcrPage` ("검색가능한 Searchable") → reopen → `text().all()` contains both words with spaces (guards the AppleGothic tab bug for the bundled font), `search` hits, 0 dark px in the boxes; rotation.pdf p2 round trip `points_to_pixels → pixels_to_points` |
| unit tests (inline) | page-range parser, quad builder, word/line grouping, `MovePages` validation, history budget arithmetic, `EngineError` mapping of `PdfiumInternalError::PasswordError` |

App crate: `tauri::test::mock_builder()` (feature `test`) drives `open_document` / `list_annotations` through
`get_ipc_response` without a webview (P1); `pdfium_path::resolve` unit test.

### 9.2 Frontend (vitest, jsdom)

`geometry.test.ts` (all 4 rotations, inverse round trip), `tiles.test.ts` (grid is 1×1 ≤ 2×, 512 above,
centre-out order), `selection.test.ts` (hit-testing and line-run union on a recorded `TextLayer` fixture),
`keymap.test.ts` (chords per platform, `when` guards, sticky-hold timing), `i18n.test.ts` (key parity, plural,
placeholders), `tools/*.test.ts` (each state machine emits the expected `AnnotSpec`), `ocr/normalize.test.ts`
(Tesseract and Vision samples → identical `OcrPage` shape, Hangul spacing rebuilt from `line.text`),
`pageRange.test.ts`, `docStore.test.ts` (mutation results replace `annots`, generation invalidates layers),
`api.test.ts` against `mockInvoke`. Component smoke: `Welcome` renders recents from a mocked store; `Viewer`
mounts pages for the virtual window.

### 9.3 Smoke and manual gates

* `scripts/perf-baseline.mjs` runs the release binary with `SEEPDF_SELFTEST=1`, which opens each fixture plus a
  generated 500-page doc (36× tracemonkey via `copy_pages_from_document`, built by `test_support`) and logs
  open/first-paint/thumbnail/search timings through the real protocol path; results land in `docs/perf/baseline.md`.
* `docs/qa/checklist.md`: the P0 acceptance rows that need a human (Preview/Acrobat rendering of saved annotations,
  Finder double-click, pinch, dark mode, Korean strings on Windows fonts). Run per release on both platforms.
* WebDriver E2E (`tauri-driver`) exists for Windows/Linux only; add a Windows-runner E2E in v1.1.

### 9.4 CI (`.github/workflows/ci.yml`)

```yaml
strategy: { matrix: { include: [ { os: macos-14, target: aarch64-apple-darwin, bundles: dmg },
                                  { os: macos-13, target: x86_64-apple-darwin, bundles: dmg },     # or macos-15-large if 13 is retired
                                  { os: windows-2022, target: x86_64-pc-windows-msvc, bundles: nsis } ] } }
steps:
  - actions/checkout ; actions/setup-node@v4 { node-version: 24, cache: npm } ; dtolnay/rust-toolchain@stable ; Swatinem/rust-cache
  - npm ci                                  # postinstall: scripts/fetch-pdfium.mjs (host lib from GitHub releases) + scripts/prepare-ocr.mjs
  - node scripts/check-i18n.mjs ; npm run typecheck ; npm test -- --run
  - cd src-tauri && cargo test --workspace --release   # release: encoder timings in tests are meaningful; ~2–4 min with cache
  - npm run tauri build -- --target ${{ matrix.target }} --bundles ${{ matrix.bundles }}
  - node scripts/check-bundle-size.mjs      # installed size ≤ 60 MB (fail > 80); prints the breakdown
  - actions/upload-artifact (bundle + docs/perf/baseline.md)
```

`release.yml` on `v*` tags: same build + macOS codesign/notarize (secrets), NSIS signing, GitHub Release upload.
Auto-update (`tauri-plugin-updater`, signed manifest) is v1.1 and needs only the release workflow to publish
`latest.json`. The crates.io mirror in `~/.cargo/config.toml` is machine-local; CI uses crates.io directly.

Bundle budget (installed): tauri shell ≈ 8 MB, libpdfium 7 MB (one arch per bundle via the platform overlays),
frontend ≈ 2 MB, Pretendard 0.35 MB, Hangul PDF font ≈ 1.5 MB, OCR 8.4 MB → ≈ 27 MB + installer overhead.
Universal macOS builds are out (they would double libpdfium); ship two DMGs.

---

## 10. Work breakdown for parallel agents (git worktrees)

Day 0 is A0 alone; everything else starts on day 1 against frozen interfaces. Each agent runs with
`CARGO_TARGET_DIR=/Users/veri/Dev/SeePDF/src-tauri/target` and `npm run tauri dev -- --no-watch`.

| Agent | Owns (exclusive) | Depends on | Delivers / definition of done |
|---|---|---|---|
| **A0 integrator** (day 0, then reviews) | pdfium-render fork + `[patch]`; `src-tauri/Cargo.toml` workspace; `crates/engine/{Cargo.toml, src/lib.rs, error.rs, types.rs, test_support.rs}`; `src/ipc/types.ts`; `package.json`; `vite.config.ts`; `.github/`; `scripts/check-*.mjs` | — | `cargo check --workspace` and `npm run typecheck` green with stub commands; fork patch compiles (`raw_handle()` used in a doctest); CI matrix green on the stubs; `types.ts` ⇄ `types.rs` sample-JSON test passes |
| **A1 engine-core** | `crates/engine/src/{thread,registry,render,encode,text,search,history,save,geom,fonts}.rs`, `tests/{smoke,render,text,history}.rs` | A0 | `EngineHandle::spawn/call/send`, lanes + stale drop, `Registry` rules 1–5, tiles + thumbs + OCR image render, text layer + search streaming, history with disk spill, save with verify+rename. Tests in §9.1 rows 1–3 and 7 pass; `tracemonkey` p0 tile @2× ≤ 4 ms release |
| **A2 engine-edit** | `crates/engine/src/{raw,annot,forms,pages,objects,redact,export}.rs`, `tests/{annot,forms,pages}.rs` | A0 (types), A1's `Registry::mutate` signature (frozen day 1) | every `AnnotSpec` kind, `AnnotPatch` via SetAP(NULL), forms via FORM_* (+ `FORM_OnAfterLoadPage` hook in the LRU agreed with A1), all page ops, export images/text/flattened. Tests rows 4–6 pass; "saved annotations visible in Preview" manual check recorded |
| **A3 app-shell (Rust)** | `src-tauri/src/{lib.rs,main.rs}`, `src/app/**`, `tauri.conf.json` + overlays, `capabilities/` | A0, A1 API | all commands of §8 wired (one line each), protocol routes with immutable caching + `x-vg`, jobs/cancel, files (argv/Opened/pending), windows, native macOS menu from `i18n/*.json`, recents thumbnails, `SEEPDF_SELFTEST`. DoD: `tauri build --debug` bundle opens a fixture from Finder; `fetch` of the same tile URL twice hits Rust once (log) |
| **A4 viewer (TS)** | `src/viewer/**`, `src/stores/docStore.ts` | `types.ts`, `mockInvoke` | scroller, layouts, zoom/pinch, tiles with placeholder swap, text layer + selection + copy, search panel with streaming, thumbnail rail, outline, view rotation, print view. Vitest rows for geometry/tiles/selection/docStore; 60 fps scroll on tracemonkey and the 500-page doc (manual, recorded) |
| **A5 tools (TS)** | `src/tools/**`, `src/forms/**`, `src/pages/**` | A4's `PageView` slots and `pageToDevice` (frozen day 2) | all P0 tools + inspector + annotation list + note popover, form overlay, pages grid with drag reorder/multi-select/insert caret. Vitest for every tool state machine; manual: create each annotation type, edit colour after reopen, fill 160F |
| **A6 shell UI (TS)** | `src/app/**`, `src/welcome/**`, `src/dialogs/**`, `src/i18n/**`, `src/styles/**`, `src/keys/**`, `src/stores/appStore.ts`, `src/App.tsx`, `src/main.tsx` | `types.ts` | title bar/mode switcher/tool strip/status bar/sidebar chrome, Welcome + recents + drop zone + merge prompt, password/unsaved/export/settings dialogs, i18n (373 keys), theme tokens, Pretendard, keymap + sticky tools, context menus, toasts, `useOpenFiles`. `check-i18n` passes; every §4.7 shortcut resolves in `keymap.test.ts` |
| **A7 OCR** | `src/ocr/**`, `src/stores/jobStore.ts`, `crates/engine/src/ocr/**`, `tests/ocr_layer.rs`, `scripts/prepare-ocr.mjs`, `src-tauri/resources/fonts/`, CSP additions (PR to A3) | A1 (render), A0 | day 1: compile probe of `objc2-vision` and tesseract.js worker inside the Tauri window (the two [unverified] items) — report before building on them; `OcrPage` normalisers, worker pool, Vision thread, `layer.rs`, OCR dialog wiring, bundled subset font (subset script + licence file + ToUnicode round-trip test). DoD: P0-10 acceptance on the synthetic Korean page and tracemonkey p1 |
| **A8 QA/perf** | `docs/qa/**`, `docs/perf/**`, `scripts/perf-baseline.mjs`, `scripts/check-bundle-size.mjs`, `fixtures/gen/**` (+ generator scripts), `src/test/**` | everyone's tests | perf harness + baseline table, bundle-size gate, generated fixtures (outline, encrypted, 500-page), manual checklist, triage of cross-module bugs |

Dependency order and merge sequence: A0 → (A1 ‖ A4 ‖ A6 ‖ A7-probe) → (A2 ‖ A3 ‖ A5) → integration branch
`integrate/v1` maintained by A0 (rebase daily; the frozen files change only through A0). Suggested milestones:
M1 "viewer" (A1+A3+A4+A6: open/scroll/zoom/search/recents), M2 "editor" (A2+A5: annotations/forms/pages/undo/save),
M3 "OCR + export" (A7 + A2 export), M4 hardening (A8 gates green on all three targets).

---

## 11. Risks and mitigations

| Risk | Likelihood / impact | Mitigation (owner) |
|---|---|---|
| `objc2-vision` stack does not compile with the CLT-only toolchain or the API differs from the source read **[unverified]** | medium / high (Korean OCR quality) | A7 day-1 probe; fallback = tesseract.js only on macOS (CER 1.2 % with PSM 4, still shippable); Vision moves to v1.1 |
| tesseract.js worker blocked by CSP / blob URL inside WKWebView or WebView2 **[unverified]** | medium / high (Windows OCR) | A7 day-1 probe with `workerBlobURL: false`, `worker-src 'self' blob:`, `script-src 'self' 'wasm-unsafe-eval'` in a `tauri build --debug` bundle (CSP is not applied in dev) |
| `FORM_SelectAllText`/`FORM_ReplaceSelection` path behaves differently from `FORM_OnChar` **[unverified]** | low / medium | `tests/forms.rs`; fallback to `FORM_OnChar` per character (verified) |
| WebKit/WebView2 do not honour `immutable` for custom-scheme responses → repeated tile fetches | low / low | re-render is 0.01–2.3 ms; A3's DoD checks the Rust log; if needed add an in-Rust PNG LRU (64 MiB) behind the same route |
| Snapshot undo on 100 MB scans costs ~0.5 s of engine time per edit | medium / medium | ship; document; follow-up = incremental-delta snapshots (`FPDF_INCREMENTAL`, 0.2 ms) behind `History::push` |
| Bundled Korean font's generated `/ToUnicode` maps space to TAB (as AppleGothic did) | low / high (search on OCR'd docs) | `tests/ocr_layer.rs` round trip is a merge gate; candidate fonts: Noto Sans KR (Arial Unicode/AppleSDGothicNeo behaved correctly) |
| Fork maintenance drift from upstream pdfium-render | low / low | patch is visibility-only + one method; rebase per upstream release; pinned by rev |
| Stamp-based text box is not editable as FreeText in Acrobat | certain / low | documented; `/Contents` carries the text; FreeText for Latin-only text can be added later without changing the UI |
| Ink-based lines/arrows appear as "ink" in other tools' annotation lists | certain / low | `/Subj` tags; acceptable for v1, matches what PDFium can create |
| No Windows machine in this environment: WebView2, NSIS, `http://seepdf.localhost` origin, custom caption buttons untested locally | certain / medium | CI windows runner from day 0; manual checklist on a Windows VM before release; Windows uses the native caption bar in v1 (ux-research open question 2 answered "native") |
| Multi-window state (menu routing, `open-file` targeting) | low / medium | `windows.rs` is ~80 lines with a unit test; v1 ships single-document-per-window, never per-window engines |
| Pdfium `FPDF_MovePages` returning `false` leaves the document "indeterminate" | low / high | validate in Rust first; on `false` reload from the pre-edit snapshot (the snapshot already exists because `mutate` pushes it before the edit) |
| 500-page performance targets missed by the React scroller | low / medium | virtualiser from day 1 (A4), perf harness gate (A8) |
| Fixture coverage: no committed outline/encrypted/CJK-scan fixtures | certain / low | `fixtures/gen/` generated deterministically by scripts (spike generators exist); add one real Korean scan (licence-clean) for OCR QA |

---

## 12. Appendix

### 12.1 Verified numbers this design leans on (release unless noted)

bind 2.1 ms · open 1 MB 0.15 ms · `page_sizes` 14 pages 0.1–0.23 ms · `FPDF_LoadPage` 0.5–26 ms · full render 1×/2×/4× 0.95/2.24/6.54 ms ·
512 px tile @8× 0.01–0.8 ms · PNG Fast+Sub 2× page 2.29 ms / 1.4 MB · text layer 5.6 ms/page · search 65 µs/page cached ·
annotation enumerate 1.6–5 ms/page · create 13 annots 0.7–1.2 ms · `save_to_bytes` 5 ms/MB · `FPDF_MovePages` 0.02 ms ·
delete page 2.2 ms · export 14 pages @150 DPI 153 ms · OCR render 300 DPI 12–21 ms · Vision Korean page 448 ms warm ·
tesseract kor+eng PSM 4 1.5 s · OCR layer 730 words 2 ms · engine channel round trip 0.3 µs · Channel progress ≥ 42k msg/s ·
protocol fetch cached by WebKit (verified If-None-Match on second request).

### 12.2 Crates / packages to add (none added by this proposal)

Rust: fork of `pdfium-render` via `[patch.crates-io]` (no new crate); macOS-only `objc2 0.6`, `objc2-foundation 0.3`,
`objc2-core-foundation 0.3`, `objc2-core-graphics 0.3`, `objc2-vision 0.3` (features per ocr.md §3b); `uuid` (v4 for `/NM`)
or a 10-line random hex helper (prefer the helper). Not needed: `lru`, `rayon`, `tokio`, `lopdf`, `ttf-parser`, `subsetter`.
npm: `zustand`, `lucide-react`, `@tauri-apps/plugin-dialog`, `@tauri-apps/plugin-store`, `@tauri-apps/plugin-window-state`,
dev `vitest`, `jsdom`. Assets: Pretendard Variable (OFL) subset woff2; a Noto Sans KR (OFL) subset TTF built once by
`scripts/build-hangul-font.*` (fonttools `pyftsubset` if Python is available, else the `subsetter` crate in a throw-away example) and committed with its licence.

### 12.3 Cargo profile and dev workflow

```toml
[profile.dev.package."*"]
opt-level = 3            # png/miniz_oxide/image in dev: 2 ms instead of 141–260 ms per page
[profile.release]        # unchanged: lto = true, codegen-units = 1, opt-level = 3, panic = "abort", strip = true
```

`npm run tauri dev -- --no-watch` (tauri's watcher restarts on any `src-tauri` change), `RUST_LOG=seepdf_lib=debug,seepdf_engine=debug`,
binary args in dev `npm run tauri dev -- --no-watch -- -- /path/x.pdf`, benchmark builds
`CARGO_PROFILE_RELEASE_LTO=false CARGO_PROFILE_RELEASE_CODEGEN_UNITS=16 cargo build --release`. Add `fixtures/out/` to `.gitignore`.

### 12.4 Config deltas (for A3)

`tauri.conf.json`: `security.csp` += `"worker-src": "'self' blob:"`, `"script-src": "'self' 'wasm-unsafe-eval'"`;
`app.windows[0]` unchanged (Overlay + hiddenTitle on macOS; the Windows overlay merges `titleBarStyle: "Visible"`);
`tauri.macos.conf.json` `bundle.resources = ["resources/pdfium/*apple-darwin.dylib", "resources/pdfium/LICENSE.pdfium", "resources/pdfium/VERSION", "resources/fonts/*"]`,
`tauri.windows.conf.json` with the dll. `capabilities/default.json`: `windows: ["main", "doc-*"]`, += `core:webview:allow-print`,
`core:window:allow-set-title`, `core:window:allow-create` (multi-window), `dialog:default`, `store:default`, `window-state:default`, `opener:default`
(`fs:*` is not needed: the engine reads files).
