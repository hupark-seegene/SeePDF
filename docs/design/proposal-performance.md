# SeePDF v1 — Proposal C: performance-first architecture

Angle: **the app must feel faster than Acrobat** — instant open, 60 fps scroll and zoom on a 500-page
document, small memory. Everything else (editing, OCR, i18n, CI) is covered completely but
conventionally. Every number below comes from the spike reports in `docs/spikes/*.md`
(pdfium-render 0.9.4, libpdfium chromium/8057, tauri 2.11.5, Apple Silicon, release builds unless
stated); every signature quoted is one the spikes compiled.

Written for: the judges, the synthesis agent, and the 6–8 implementation agents (§10 assigns
files). Read §0 and §2–§3 first; §7 is the machine-readable-ish IPC contract.

---

## 0. The ten decisions (TL;DR)

| # | Decision | Why (measured) |
|---|---|---|
| 1 | **One `pdf-engine` thread owns pdfium** (both a pdfium-render `Pdfium` and a second raw `&'static dyn PdfiumLibraryBindings`), driven by a **priority scheduler**, not a FIFO. | `thread_safe` locks only 289/481 FFI functions; pdfium gives 0 % speed-up with 2 threads (58.9 vs 59.2 ms); a FIFO made tiles wait ~30 ms behind a full-page render. |
| 2 | **Render on the engine thread, encode on a 2–3 thread pool, respond to the webview from the pool.** The engine thread never runs a PNG/JPEG encoder. | Render 0.01–0.8 ms/tile, PNG Fast+Sub 0.2 ms/tile but 2.3 ms/page and JPEG 12.7 ms/page; encoding is the only parallelisable part. |
| 3 | **Tiles of 512 device px via `set_origin`, whole-page bitmap only when the page is ≤ 2.5 Mpx.** Never the matrix/clip path, never a full page above 4×. | Tiling overhead ≤ 25 %; empty tiles cost 0 ms; matrix path silently drops form fields; an 8× page is 118 MiB. |
| 4 | **Pixels travel over the `seepdf://` custom protocol as `<img>` tiles with `gen` in the URL and `Cache-Control: immutable`.** Binary IPC (`tauri::ipc::Response`) only for data JS must touch (text layer, OCR bitmaps). JSON/base64 never carries pixels. | Protocol = free WebKit/Chromium decode off-thread + memory cache; raw IPC 141 MB/s vs base64 17 MB/s vs JSON 10 MB/s. |
| 5 | **Two caches, both bounded in bytes: engine-side *encoded* tile LRU (64 MiB, checked on the protocol thread before the engine is touched) and the browser's own image cache.** No raw-RGBA cache: re-rendering a tile (0.01–0.8 ms) is cheaper than keeping 1 MiB of it. | macOS never returns freed bitmap memory (RSS = high-water mark); PNG tiles are 5.5× smaller than RGBA. |
| 6 | **Three-level LOD per page — thumbnail (120 px), placeholder (scale 1, ≤ 640 px wide, ~1 ms), full tiles — plus CSS transforms during zoom gestures**; sharp tiles replace the CSS-scaled layer on settle (120 ms debounce). | Zoom visual response ≤ 16 ms target; placeholder ≤ 120 ms target; a 1× page renders in 0.95 ms. |
| 7 | **Every render request carries a viewport generation and a priority lane; the engine drops stale requests before calling pdfium** and orders visible tiles centre-out. Edit commands are strictly FIFO on their own lane. | Fling-scroll must never build a backlog of stale pages. |
| 8 | **Frontend: React 19 for chrome and page-shell virtualisation only; tiles, text hit-testing and selection are imperative classes that mutate DOM directly.** State in zustand (1 kB). No spans-per-char text layer; char boxes arrive as one `ArrayBuffer` per page. | A 5,000-char page as spans is 5,000 DOM nodes; a typed-array text layer is ~140 KB and hit-tests in µs. |
| 9 | **Open = `load_pdf_from_byte_vec` (≤ 32 MB) or `load_pdf_from_reader(File)` (larger), then `page_sizes()` only; everything else (outline, text, thumbnails, form fields) is lazy.** | Open 1 MB = 0.15 ms; `page_sizes()` = 0.1 ms/14 pages → ~3.5 ms for 500 pages; `pages().get()` costs 0.5–26 ms so it is never done eagerly. |
| 10 | **Phase 0 vendors pdfium-render 0.9.4 into `src-tauri/vendor/` with a ~40-line patch exposing raw handles** (`PdfDocument::raw_handle()`, `PdfPage::raw_handle()`, `PdfPageAnnotation::raw_handle()`, `PdfForm::raw_handle()`, `PdfFonts::load_cid_type2_font`). | Three spikes independently need raw FFI on the *same* document: `FPDF_MovePages`, save flags, `FPDFAnnot_SetAP(NULL)` recolour recipe, `FORM_On*` fill, `FPDFPage_Flatten(NORMALDISPLAY)`, circle annots, progressive render. Two separate documents cannot share edits. |

Targets we commit to (from `docs/spikes/ux-research.md` §7) and the budget that meets them:

| Scenario | Target | Budget derived below |
|---|---|---|
| Cold launch → welcome | ≤ 700 ms (win ≤ 1.0 s) | Tauri shell ~350 ms + initial JS chunk ≤ 120 kB gz + engine bind 2 ms off the critical path (§2.7) |
| Open 500-page PDF → first page painted | ≤ 400 ms | read ≤ 10 ms (10 MB) + load 0.2 ms + page_sizes 3.5 ms + placeholder 1 ms + tiles ≤ 10 ms + transport ≤ 15 ms ≈ 40 ms (§2.7) |
| Scroll 100 % zoom | 60 fps, placeholder ≤ 120 ms, sharp ≤ 300 ms after stop | native compositor scroll, 0 JS per frame; tile pipeline ≈ 5–20 ms per tile row (§3.6) |
| Zoom step | ≤ 16 ms visual, sharp ≤ 250 ms | CSS transform now; 12–20 tiles × (≤ 0.8 ms render + 0.2 ms encode + ~1 ms transport) ≈ 40 ms (§3.1) |
| Page render A4 @150 DPI | p50 ≤ 30 ms, p95 ≤ 120 ms | measured 4.6 ms/page text; vector-heavy pages capped by progressive render (§3.2) |
| Search first match, 500 pages | ≤ 150 ms, full pass ≤ 2.5 s streamed | text layer 5.6 ms/page, outward-from-current-page order (§4.6) |
| Annotation create → visible | ≤ 16 ms | optimistic SVG; baked tiles follow in ~10 ms (§4.7) |
| Save, annotations only, 500 pages | ≤ 300 ms | `FPDF_INCREMENTAL` 0.2 ms + sequential write (§5.7) |
| Idle RAM, 500-page doc | ≤ 400 MB | engine ≤ 15 + file + 25 + 64 + 32 + transient 40 ≈ 190 MB; webview bounded by mounted tiles (§2.5) |
| Installed bundle | ≤ 60 MB | Tauri ~10 + libpdfium 7 + OCR 8.4 + fonts ~1.5 ≈ 27 MB per platform (§9.4) |

---

## 1. Ground rules that every module obeys

1. **No pdfium call off the engine thread.** Not from a Tauri command, not from the protocol
   handler, not from the OCR thread. The engine thread is the only place a `PdfDocument`,
   `PdfPage`, `PdfBitmap` or raw `FPDF_*` handle exists. Everything crossing the channel is
   `Send + 'static` plain data (`Vec<u8>`, `Arc<[u8]>`, structs).
2. **Drop order is a law:** `PdfPageText` → `PdfPage` → `PdfDocument` → (never) `Pdfium`.
   `PdfPage<'static>` outliving its document *compiles* and is a use-after-free (render spike,
   `SPIKE_UAF`). The `DocEntry` type in §2.4 enforces it by field order + explicit `close()`.
3. **Pages are never held across commands** except through the engine's own `PageLru`
   (§2.4), which is flushed before any raw structural call (`FPDF_MovePages`,
   `FPDF_ImportPagesByIndex`) because those bypass pdfium-render's `PdfPageIndexCache`.
4. **Every mutation bumps `DocGen`** and emits `doc:changed`. Every cache key (tiles, text
   layers, thumbnails, annotation lists) contains the gen. Nothing is invalidated by hand.
5. **Long work is chunked per page** (search, OCR render, export, flatten) into separate
   scheduler jobs sharing a `JobToken`, so interactive tiles interleave and cancellation is
   observed within one page's worth of work (≤ 30 ms).
6. **Release-profile numbers only.** Debug builds make the encoders 30–60× slower (PNG 141 ms
   vs 4.3 ms). `Cargo.toml` gets `[profile.dev.package."*"] opt-level = 3` in Phase 0 so
   `tauri dev` is representative.
7. **Korean first**: every string through `t()`, every dialog laid out against the Korean copy.

---

## 2. Process and thread architecture

### 2.1 Threads

```
main thread (Tauri/wry event loop)      ← protocol handler dispatch (µs), window events, RunEvent::Opened
tokio runtime (tauri::async_runtime)    ← #[tauri::command] async fns: forward to engine, await oneshot
pdf-engine  (std::thread, 16 MiB stack) ← owns Pdfium + raw bindings + DocRegistry + PageLru + TextCache; runs Scheduler
encode-0..N (N = clamp(cores/4, 2, 3))  ← PNG/JPEG encode, tile-cache insert, UriSchemeResponder::respond, export file writes
io          (std::thread)               ← file reads for open, atomic save (temp + fsync + rename), backups, recent-files store
ocr-native  (macOS only, std::thread)   ← Vision requests (GPU/ANE), never touches pdfium
webview     (WKWebView / WebView2)      ← React shell, TileManager, tesseract.js Web Workers (≤ 4)
```

Why not a thread per document: pdfium is one global (`FPDF_InitLibrary` once, font/image caches
per process), and the measured parallel speed-up is zero. Why a separate encode pool: encoding
is 50 % of export cost (4.4 of 9 ms/page) and 100 % parallelisable; with the pool the engine
thread's per-tile occupancy is the render time only.

### 2.2 Engine command protocol (`src-tauri/src/engine/types.rs`)

```rust
use crossbeam_channel::{Receiver, Sender};
use std::sync::{atomic::{AtomicBool, AtomicU64}, Arc};

/// Stable per-process document id (never reused within a session).
#[derive(Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct DocId(pub u32);

/// Monotonic per-document edit counter. Part of every cache key and every tile URL.
pub type DocGen = u64;
pub type PageIndex = i32;            // == pdfium_render::prelude::PdfPageIndex (c_int)
pub type ScaleKey = u32;             // round(zoom_percent * device_pixel_ratio); s = key / 100.0

/// One-shot reply. `oneshot()` uses tauri::async_runtime::channel(1) (tauri does not re-export
/// tokio::sync::oneshot); `from_fn` lets the protocol handler pass its UriSchemeResponder.
pub struct Reply<T>(Box<dyn FnOnce(T) + Send + 'static>);

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Lane { Interactive = 0, Edit = 1, Prefetch = 2, Thumb = 3, Background = 4 }

/// Cancellation + staleness. `viewport_gen` is compared against EngineShared::viewport_gen before
/// the job runs; `cancel` is polled inside chunked jobs between pages.
#[derive(Clone)]
pub struct JobToken { pub id: u64, pub cancel: Arc<AtomicBool>, pub viewport_gen: u64 }

pub struct Job {
    pub lane: Lane,
    /// Lower runs first inside a lane. For tiles: Chebyshev distance (in tiles) from the viewport
    /// centre; for pages: |page - centre_page|. Edit lane ignores it (FIFO by seq).
    pub priority: u32,
    pub seq: u64,                    // submission order, tie-breaker and FIFO key for Lane::Edit
    pub token: JobToken,
    pub kind: JobKind,
}

pub enum JobKind {
    Open      { path: std::path::PathBuf, password: Option<String>, reply: Reply<Result<DocInfo, EngineError>> },
    Close     { doc: DocId, reply: Reply<()> },
    RenderTile{ req: TileRequest, reply: Reply<Result<RenderedTile, EngineError>> },
    RenderPage{ req: PageRenderRequest, reply: Reply<Result<RenderedTile, EngineError>> }, // placeholder / thumb / export / OCR
    TextLayer { doc: DocId, page: PageIndex, reply: Reply<Result<Arc<TextLayer>, EngineError>> },
    SearchPage{ doc: DocId, page: PageIndex, query: Arc<SearchQuery>, sink: tauri::ipc::Channel<SearchEvent> },
    Annot     { doc: DocId, op: AnnotOp, reply: Reply<Result<AnnotResult, EngineError>> },
    Form      { doc: DocId, op: FormOp,  reply: Reply<Result<FormResult, EngineError>> },
    Pages     { doc: DocId, ops: Vec<PageOp>, reply: Reply<Result<DocInfo, EngineError>> },
    Save      { doc: DocId, req: SaveRequest, reply: Reply<Result<SaveResult, EngineError>> },
    ExportPage{ doc: DocId, page: PageIndex, spec: Arc<ExportSpec>, sink: tauri::ipc::Channel<JobEvent> },
    OcrApply  { doc: DocId, page: OcrPage, font: OcrFontChoice, reply: Reply<Result<u32, EngineError>> },
    Flatten   { doc: DocId, page: PageIndex, reply: Reply<Result<(), EngineError>> },
    Stats     { reply: Reply<EngineStats> },
    Shutdown,
}

/// Cloneable, Send + Sync; the only thing in tauri managed state.
#[derive(Clone)]
pub struct EngineHandle {
    tx: Sender<Job>,
    pub shared: Arc<EngineShared>,
}

pub struct EngineShared {
    pub seq: AtomicU64,
    pub viewport_gen: AtomicU64,                 // bumped by set_viewport (every scroll/zoom settle)
    pub viewport: parking_lot::Mutex<Viewport>,  // centre page, visible range, scale_key, scroll direction
    pub docs: parking_lot::RwLock<HashMap<DocId, DocSummary>>, // mirror for protocol/404 checks, no pdfium
    pub tile_cache: TileCache,                   // encoded tiles, parking_lot::Mutex<lru>, §3.5
    pub budgets: parking_lot::RwLock<Budgets>,
}
```

Submission from a Tauri command:

```rust
#[tauri::command]
pub async fn get_text_layer(engine: tauri::State<'_, EngineHandle>, doc_id: DocId, page: i32)
    -> Result<tauri::ipc::Response, EngineError>
{
    let (reply, mut rx) = Reply::oneshot();
    engine.submit(Lane::Interactive, 0, JobKind::TextLayer { doc: doc_id, page, reply })?;
    let layer = rx.recv().await.ok_or(EngineError::Gone)??;
    Ok(tauri::ipc::Response::new(layer.to_bytes()))     // binary, see §4.6
}
```

### 2.3 Scheduler (`src-tauri/src/engine/scheduler.rs`)

The engine loop is not `for job in rx`. It is:

```rust
loop {
    // 1. block for at least one job, then drain everything that is already queued (µs)
    let first = rx.recv()?;
    heap.push(first);
    while let Ok(j) = rx.try_recv() { heap.push(j); }
    // 2. pop by (lane, priority, seq); Edit lane entries keep FIFO because priority == 0 and seq ascends
    let job = heap.pop().unwrap();
    // 3. staleness: a tile whose viewport_gen is older than the current one is dropped before pdfium runs
    if job.is_render() && job.token.viewport_gen + STALE_TOLERANCE < shared.viewport_gen.load(Relaxed) {
        job.reply_err(EngineError::Stale); continue;
    }
    if job.token.cancel.load(Relaxed) { job.reply_err(EngineError::Cancelled); continue; }
    // 4. run (≤ one page of work); chunked jobs re-enqueue their next page with the same token
    run(job, &mut state);
}
```

- **Lanes**: `Interactive` (visible tiles, placeholder for pages entering the viewport, text
  layer of the page under the cursor, form focus), `Edit` (annotation/form/page/save commands,
  strictly FIFO), `Prefetch` (±1.5 screens of tiles, velocity-biased), `Thumb`, `Background`
  (search pass, export pages, OCR renders, flatten).
- **Starvation guard**: after 8 consecutive Interactive/Prefetch pops, one Edit or Background
  job is popped if present (otherwise the search pass never advances during a slow scroll).
  Edit jobs are short (annotation create 0.05 ms, page delete 2 ms, `save_to_bytes` 5 ms/MB —
  for a 100 MB scan, Save is the one job that blocks tiles for ~0.5 s; the UI shows the
  status-bar spinner and the frontend stops requesting tiles until `SaveResult`).
- **Re-prioritisation is free**: tiles are pushed with `priority = distance` computed by the
  frontend at request time; when the viewport moves, the new requests come with new distances
  and old ones are dropped by generation. `STALE_TOLERANCE = 1` so a tile requested one scroll
  frame ago still renders (it is usually still visible).
- **Cancellation**: `job_cancel(jobId)` flips the `AtomicBool`; chunked jobs check it between
  pages; `search_start` checks per page (≤ 6 ms granularity); OCR render/export per page
  (≤ 30 ms). For a single pathological vector page the P1 progressive path (§3.2) checks every
  ~8 ms via `IFSDK_PAUSE`.
- **Queue depth bound**: `EngineHandle::submit` refuses (`Err(Busy)`) render jobs when the
  heap holds > 256 render jobs; the frontend's TileManager already limits in-flight tiles to 24,
  so this is a safety net against a runaway client.

### 2.4 Document registry (`src-tauri/src/engine/registry.rs`)

```rust
pub struct DocEntry {
    // field order == drop order (Rust drops fields in declaration order):
    pages: PageLru,                        // open PdfPage<'static> handles, dropped first
    doc: PdfDocument<'static>,             // from the leaked &'static Pdfium (Pdfium::drop never destroys the library)
    raw: FPDF_DOCUMENT,                    // == doc.raw_handle() (Phase-0 patch); NOT a second load
    form: Option<FPDF_FORMHANDLE>,         // pdfium-render inits FPDFDOC_InitFormFillEnvironment at load when /AcroForm exists
    pub gen: DocGen,
    pub saved_gen: DocGen,
    pub path: Option<PathBuf>,             // None for new/merged documents
    pub source: DocSource,                 // Bytes(Vec owned by pdfium) | Reader(File) | Memory
    pub meta: DocMeta,                     // page sizes + rotations + labels, computed at open (page_sizes(): 0.1 ms/14 pages)
    pub history: EditHistory,              // kinds of edits since saved_gen: annotations/forms only? → incremental save eligible
    pub text: TextCache,                   // per-page Arc<TextLayer> LRU (32 MiB) + Vec<Option<Arc<str>>> plain text (kept)
    pub graveyard: Option<PdfDocument<'static>>, // deleted pages parked for undo (copy_page_from_document, 0.37 ms/page)
}

pub struct PageLru { map: HashMap<PageIndex, PdfPage<'static>>, order: VecDeque<PageIndex>, cap: usize /* 24 */ }
impl PageLru {
    /// FPDF_LoadPage costs 0.46–26 ms; hits are free. Every page is set to
    /// PdfPageContentRegenerationStrategy::Manual on load (annotation ops must not regenerate the content stream).
    pub fn get(&mut self, doc: &PdfDocument<'static>, i: PageIndex) -> Result<&mut PdfPage<'static>, EngineError>;
    pub fn flush(&mut self);              // before FPDF_MovePages / ImportPagesByIndex / close
}
```

Open strategy (`DocSource`): files ≤ 32 MB are read on the `io` thread and handed to
`load_pdf_from_byte_vec(bytes, password)` (document owns the Vec, lazy parse 0.15 ms for
1 MB). Larger files use `load_pdf_from_reader(std::fs::File, password)` —
`PdfiumReader: Read + Seek + Send` is satisfied by `File`, pdfium then reads blocks on demand
through `FPDF_LoadCustomDocument`, so a 400 MB scan costs no heap. The price is an open file
handle (Windows sharing) which the save flow (§5.7) handles by close → rename → reopen.

Password: `PdfiumError::PdfiumLibraryInternalError(PdfiumInternalError::PasswordError)` for
both "missing" and "wrong" → `EngineError::PasswordRequired` → the UI prompts and retries.

### 2.5 Memory model and limits

| Pool | Bound | Mechanism |
|---|---|---|
| libpdfium + `FPDF_InitLibrary` | ~9 MiB | fixed |
| Document bytes | file size (Vec path) or 0 (reader path) | 32 MB threshold |
| `PageLru` | 24 pages ≈ 25–60 MiB incl. pdfium parse caches | LRU; `limit_render_image_cache_size(true)` on every tile config bounds pdfium's per-page image cache |
| Largest single allocation | 1 MiB tile, or ≤ 10 MiB whole page (2.5 Mpx cap) | geometry rule §3.1; export pages stream one at a time |
| Encoded tile cache | 64 MiB default (`Budgets.tile_cache_bytes`) | LRU on the protocol thread |
| Text layers | 32 MiB LRU + plain text (~5 kB/page) kept | `TextCache` |
| Encode pool in-flight | ≤ 8 raw bitmaps ≈ 8–80 MiB transient | bounded `crossbeam_channel::bounded(8)`; engine blocks < 1 ms if full |
| Graveyard pages | unbounded by pages, bounded by undo depth 100 | dropped with the undo stack |
| Thumbnails | 0 engine-side | browser cache + 1.5 ms re-render |

RSS on macOS is a high-water mark, so the *peak* is what matters: engine peak for a 500-page
document ≈ 9 + 10 (file) + 60 + 64 + 32 + 80 ≈ 255 MiB worst case, ~150 MiB typical. The webview
process is bounded by the number of mounted tile `<img>`s (§4.5, ≤ ~120 tiles ≈ 120 MiB
decoded) and its own image cache, which WebKit shrinks under pressure. `engine_stats()` reports
`rss_bytes` (via `/proc`-free `mach_task_basic_info` on mac, `GetProcessMemoryInfo` on win —
both behind `cfg`, ~30 lines, no crate) so the dev perf overlay shows it live.

### 2.6 Phase-0 patch to pdfium-render (`src-tauri/vendor/pdfium-render/`)

Vendored copy of the registry crate (`cp -r ~/.cargo/registry/src/*/pdfium-render-0.9.4`) plus
`[patch.crates-io] pdfium-render = { path = "vendor/pdfium-render" }`. The diff (≈ 40 lines):

```rust
impl<'a> PdfDocument<'a>          { pub fn raw_handle(&self) -> FPDF_DOCUMENT { self.handle } }
impl<'a> PdfPage<'a>              { pub fn raw_handle(&self) -> FPDF_PAGE { self.page_handle } }
impl<'a> PdfPageAnnotation<'a>    { pub fn raw_handle(&self) -> FPDF_ANNOTATION }
impl<'a> PdfForm<'a>              { pub fn raw_handle(&self) -> FPDF_FORMHANDLE }
impl<'a> PdfFonts<'a> {
    /// FPDFText_LoadCidType2Font: glyphless CID font with our own ToUnicode (OCR layer, §6).
    pub fn load_cid_type2_font(&mut self, font_data: &[u8], to_unicode_cmap: &str, cid_to_gid_map: &[u8]) -> Result<PdfFontToken, PdfiumError>
}
```

The raw bindings object is `Pdfium::bind_to_library(&lib)` called a **second time before**
`Pdfium::new` (verified: the second call only fails after the OnceCell is populated), leaked to
`&'static dyn PdfiumLibraryBindings`. Both handles `dlopen` the same library; all raw calls
happen on the engine thread, so the fact that the two `thread_safe` wrappers have separate
mutexes is irrelevant. Fallback if the patch is refused by the owner: keep a *raw*
`FPDF_DOCUMENT` per document loaded from the same bytes for the annotation/form/page-op pipeline
and treat pdfium-render's document as a read-only render/text view that is reloaded from
`FPDF_SaveAsCopy` bytes after each edit batch (5 ms/MB — acceptable for ≤ 20 MB documents,
unacceptable for scans; hence the patch is the primary plan).

### 2.7 Startup path

```
t=0      main(): tauri::Builder … .setup():
           spawn pdf-engine (binds pdfium in 2.1 ms on its own thread; nothing waits for it)
           spawn io, encode pool; manage(EngineHandle); register seepdf:// (async variant)
           window "main" is created with visible:false
t≈300ms  webview loads dist/index.html (Vite build): initial chunk = React 19 + zustand + i18n ko.json + shell ≈ 110 kB gz;
           the viewer chunk (layout, TileManager, tools) is `import()`-ed and prefetched with <link rel=modulepreload> but not on the critical path
t≈330ms  React mounts <App>; first paint of Welcome or empty Viewer; effect calls getCurrentWindow().show() (no white flash)
           and invoke('take_pending_opens') → argv / RunEvent::Opened paths
t≈340ms  open_document(path): io thread reads bytes (≤ 10 ms for 10 MB) → engine Open (0.2 ms) → page_sizes (3.5 ms / 500 pages)
           → DocInfo { pages: [[w,h,rot], …] } back to JS → layout computed → page shells mounted
t≈360ms  TileManager requests placeholder (page 0) + visible tiles at Lane::Interactive; engine renders (≈ 1 + 3–10 ms),
           pool encodes (≈ 0.2 ms/tile), WebKit decodes; sharp first page ≈ t+40 ms
```

Cold-launch specifics: no web font on the critical path (system stack: Apple SD Gothic Neo /
Malgun Gothic / Pretendard only if the user opts in, loaded lazily as a ~300 kB Korean-subset
woff2); lucide icons tree-shaken (≈ 0.5 kB each); `tesseract.js` and the OCR UI are a separate
chunk loaded on first OCR use. Window state restore (`tauri_plugin_window_state`) runs before
`show()`. Measured baseline to beat: the spike's `tauri dev` window with the same plugins is
interactive well under the 700 ms target on this Mac; CI records `perf/launch_ms` (§9).

---

## 3. Render pipeline and transport

### 3.1 Scale ladder, LOD and tile geometry (`src-tauri/src/render/geometry.rs`, `src/viewer/geometry.ts` — same math, cross-tested)

- Device scale `s = zoom × devicePixelRatio`, quantised: `scaleKey = round(zoomPercent × dpr)`,
  `s = scaleKey / 100`. Whole-percent zooms at dpr 1 and 2 are exact; pinch settles to the
  nearest whole percent. Range: `scaleKey ∈ [10, 800]` (8× device scale); beyond that the
  frontend CSS-upscales the 800 tiles (no pdfium render above 8×, ever).
- Page device size: `W = round(fround(w_pt) × s)`, `H = round(fround(h_pt) × s)` — exactly what
  pdfium-render does (`(source_width.value * width_scale).round() as c_int`, `render_config.rs:800`,
  f32 arithmetic). The frontend uses `Math.fround` to match; in the pathological x.5 case the
  engine's `X-Image-Width/Height` headers win and the last tile column is 1 px off, invisibly.
- **Whole page** (`kind = page`) when `W × H ≤ 2.5 Mpx` (Letter/A4 up to s = 2.0; A3 up to
  s ≈ 1.5); render 0.95–2.3 ms, PNG 0.35–1.4 MB. **Tiles** (`kind = tile`, 512×512 device px,
  origin `(tx·512, ty·512)`, edge tiles clipped to `W, H`) above that. `cols = ceil(W/512)`,
  `rows = ceil(H/512)`.
- **Placeholder** (`kind = ph`): whole page at `s_ph = min(1.0, 640 / w_pt)` (Letter: 612×792,
  0.95 ms, ~350 kB PNG), annotations + forms on. Mounted for every page within ±1.5 screens,
  CSS-scaled under the tiles; stays mounted until every sharp tile of that page has loaded.
- **Thumbnail** (`kind = thumb`): `set_target_width(w).set_maximum_height(h)` with
  `w = sidebarThumbCssWidth × dpr` (typically 200–400 px), 1.5–2.8 ms/page, annotations on.
  `PdfRenderConfig::thumbnail()` is never used (squashes to n×n, disables annotations).
- **Rotation**: intrinsic `/Rotate` is already applied by pdfium (`page_sizes()` returns the
  rotated size). View rotation (⌘L/⌘R) is `.rotate(PdfPageRenderRotation::DegreesN, true)` and is
  part of the tile key (`rot`). Page-op rotation (`set_rotation`) changes `DocMeta` + gen.

### 3.2 Render configuration (`src-tauri/src/render/tiles.rs`)

```rust
fn tile_config(s: f32, rot: PdfPageRenderRotation, origin: (Pixels, Pixels), night: bool) -> PdfRenderConfig {
    PdfRenderConfig::new()
        .scale_page_by_factor(s)                       // page size = round(points * s)
        .rotate(rot, true)
        .render_form_data(true)                        // FPDF_FFLDraw with the same origin — only on this path
        .render_annotations(true)                      // FPDF_ANNOT; also makes pdfium generate + persist /AP streams
        .use_lcd_text_rendering(false)                 // subpixel AA is wrong once composited in a browser
        .limit_render_image_cache_size(true)           // FPDF_RENDER_LIMITEDIMAGECACHE: bounds pdfium's decoded-image cache
        .set_reverse_byte_order(true)                  // pdfium writes RGBA: as_raw_bytes() is already RGBA, one copy, no swizzle
        .set_format(PdfBitmapFormat::BGRA)             // never BGR (as_rgba_bytes 83.6 ms)
        .set_clear_color(if night { PdfColor::new(0,0,0,0) } else { PdfColor::WHITE })
        .set_origin(-origin.0, -origin.1)              // tile: pdfium clips to the tile-sized bitmap
}

pub fn render_tile(page: &PdfPage<'_>, req: &TileRequest) -> Result<RawRgba, EngineError> {
    let (w, h) = req.bitmap_size();                    // 512×512 or the clipped edge size, or W×H for kind=page/ph
    let mut bmp = PdfBitmap::empty(w, h, PdfBitmapFormat::BGRA)?;   // reused vs fresh: same speed (2.23 vs 2.24 ms)
    page.render_into_bitmap_with_config(&mut bmp, &tile_config(req.scale(), req.rot, req.origin(), req.night))?;
    Ok(RawRgba { w, h, bytes: bmp.as_raw_bytes() })    // 0.16 ms for a 2× page; tightly packed, stride == w*4
}
```

Never: `translate/scale/clip/apply_matrix` on the config (switches to
`FPDF_RenderPageBitmapWithMatrix`, silently sets `render_form_data = false`, ignores `set_origin`).
Verified tile correctness: `set_origin` tiles match a crop of the full render within 427/1,048,576 px
at max channel delta 1, and tile-to-tile 0 px.

**Night mode**: the transparent clear colour keeps real alpha (484,566/484,704 px on a text page),
so the frontend composites tiles over its own dark page background and applies
`filter: invert(1) hue-rotate(180deg)` to the *tile layer only*; the SVG annotation layer (our own
annotations while editing) is not inverted. Baked third-party annotations invert with the page —
accepted for P1.

**Heavy pages (P1, progressive render)**: if a page's last full render took > 50 ms (tracked in
`DocMeta.render_ms[page]`), its tiles are rendered through raw
`FPDF_RenderPageBitmap_Start(bitmap, page, sx, sy, W, H, rot, flags, &mut pause)` /
`FPDF_RenderPage_Continue` with an `IFSDK_PAUSE.NeedToPauseNow` returning true after 8 ms, and
the engine loop pumps higher-priority jobs between continues (the `_Start`/`_Continue` wrappers
are lock-wrapped in `thread_safe.rs:470`). Form fields need a following `FPDF_FFLDraw`. Requires
the raw page handle (Phase-0 patch). Not needed for the p50/p95 targets on text pages
(4.6 ms/page) — it exists so a 200 ms CAD page cannot freeze scrolling.

### 3.3 Encode pool (`src-tauri/src/render/encode.rs`)

```rust
pub struct EncodeJob { key: TileKey, raw: RawRgba, format: TileFormat, respond: Respond }
pub enum Respond { Protocol(UriSchemeResponder, http::HeaderMap), Ipc(Reply<Result<Vec<u8>, EngineError>>), File(PathBuf, Channel<JobEvent>) }
pub enum TileFormat { Png, Jpeg(u8), Rgba }

fn encode_png(raw: &RawRgba) -> Vec<u8> {            // measured: 2.29 ms / 1.40 MB for 1224×1584; ≈ 0.2 ms per 512² tile
    let mut out = Vec::with_capacity(raw.bytes.len() / 4);
    let mut enc = png::Encoder::new(&mut out, raw.w as u32, raw.h as u32);
    enc.set_color(png::ColorType::Rgba); enc.set_depth(png::BitDepth::Eight);
    enc.set_compression(png::Compression::Fast); enc.set_filter(png::Filter::Sub);
    enc.write_header().unwrap().write_image_data(&raw.bytes).unwrap();
    out
}
fn encode_jpeg(raw: &RawRgba, q: u8) -> Vec<u8> {    // strip alpha (1.0 ms) then JpegEncoder::new_with_quality(q).encode(rgb, w, h, ExtendedColorType::Rgb8): q85 12.7 ms / 411 kB per 2× page
```

Format policy (decided per *page*, cached in `DocMeta.page_kind[page]` after the first
`PageLru::get`): `Jpeg(82)` when the page has ≥ 1 image object whose bounds cover ≥ 60 % of the
page and ≤ 20 text objects (a scan); `Png` otherwise; `Rgba` only for OCR/export-preview IPC
consumers. The classifier costs one `objects().iter()` pass (0.1–2 ms) and is done on the engine
thread once per page per gen.

The pool thread does, in order: encode → `tile_cache.insert(key, Arc<[u8]>)` → respond. The engine
thread's occupancy per tile is therefore the render (0.01–0.8 ms) plus a 0.16 ms copy.

### 3.4 `seepdf://` protocol contract (`src-tauri/src/protocol/mod.rs`)

Registered with `register_asynchronous_uri_scheme_protocol("seepdf", handler)`; the handler runs on
the main thread on macOS (spike-verified) and must return in µs: parse, cache lookup, submit.

| Route | Query | Response |
|---|---|---|
| `/tile` | `doc, gen, page, sk (scaleKey), rot, tx, ty, night?` | `image/png` or `image/jpeg`; `Cache-Control: private, max-age=604800, immutable`; `ETag "doc:gen:page:sk:rot:tx:ty:n"`; `X-Render-Ms`, `X-Encode-Ms`, `X-Cache: hit|miss`, `X-Image-Width/Height` |
| `/page` | `doc, gen, page, sk, rot, night?` | whole page (kind=page) |
| `/ph` | `doc, gen, page, rot` | placeholder (kind=ph, engine chooses `s_ph`) |
| `/thumb` | `doc, gen, page, w, h, rot` | thumbnail |
| `/ocr` | `doc, gen, page, dpi` | `image/png` gray8 at `dpi` (300 default), for tesseract.js workers |
| `/raw` | `doc` | original bytes, `Range` → 206 (unchanged from the spike) |

Rules: `gen` in the URL makes every response immutable → the browser never revalidates (the spike
verified `must-revalidate` + 304; `immutable` is the same mechanism minus the round trip — first
implementation task of the render module is to confirm WebKit and WebView2 honour it on a custom
scheme; fallback is the verified `max-age=0, must-revalidate` with 304 from the encoded cache).
Unknown doc → 404 from the `docs` mirror without touching the engine; stale `gen` (older than the
doc's current gen) → 410 so a lagging `<img>` never paints outdated pixels; render error → 500;
engine busy → 503 with `Retry-After: 0` (TileManager retries with backoff). The handler never
drops a `UriSchemeResponder` without responding (the `<img>` would hang until WebKit's timeout).

Windows: the same route under `http://seepdf.localhost/...`; the frontend never sniffs the OS —
`convertFileSrc('', 'seepdf')` gives the origin (`src/ipc/tileUrl.ts`, from the spike's
`seepdfUrl.ts`). CSP keeps both `seepdf:` and `http://seepdf.localhost https://seepdf.localhost`
in `img-src`/`connect-src` (spike-verified in the bundle).

### 3.5 Caches

**Engine-side `TileCache`** (`src-tauri/src/render/cache.rs`): `parking_lot::Mutex<LruMap<TileKey, Arc<[u8]>>>`
with a byte budget (64 MiB; ≈ 350 PNG tiles at 2× or ≈ 45 whole text pages), looked up on the
protocol thread *before* submitting to the engine. `TileKey { doc, gen, page, kind, sk, rot, tx, ty, night }`.
Invalidation is implicit (gen in key); a sweep runs on every `DocGen` bump and on `Close` to drop
dead entries eagerly (O(n), n ≤ 400). Hand-rolled `HashMap + VecDeque` (no `lru` crate needed).

**Browser side**: WebKit/Chromium image cache holds decoded tiles by URL. The frontend keeps at
most `MAX_MOUNTED_TILES = 120` tile `<img>` elements alive (≈ 120 MiB decoded at 512²×4); shells
beyond ±1.5 screens are unmounted, which lets the browser drop their decoded bitmaps. Nothing is
cached in JS memory except the placeholder URLs and the text layers of the ±3 visible pages.

**Text cache** (§2.4): `Arc<TextLayer>` per page, 32 MiB LRU; the plain lower-cased `Vec<char>`
per page (≈ 20 kB/page) is kept for the document lifetime so re-queries are ≤ 50 ms on 500 pages.

### 3.6 Prefetch and priority (frontend `TileManager` + engine `Lane`)

Order in which requests are issued (and the lane/priority they carry):

1. Visible tiles of the current scale, `Lane::Interactive`, `priority = chebyshev_distance(tile, viewportCentreTile)` — centre-out.
2. Placeholder (`/ph`) for every page entering ±1.5 screens, `Interactive`, `priority = 64 + |page − centrePage|`.
3. Tiles for the next 1 screen in the scroll direction (velocity > 0.2 px/ms) else ±0.5 screen both ways, `Lane::Prefetch`.
4. Thumbnails for the visible sidebar rows ± 1 screen, `Lane::Thumb`.
5. Nothing else is prefetched; search/OCR/export are user-initiated `Background` jobs.

In-flight bound: ≤ 24 outstanding `<img>` loads (`TileManager.inflight`), ≤ 8 during a fling
(velocity > 1.5 px/ms), because `<img>`s issued during a fling are mostly stale by the time they
render. Each scroll settle (rAF after `scroll` with velocity < 0.05 px/ms) and each zoom settle
calls `set_viewport()` once (fire-and-forget IPC, bumps `viewport_gen`, records centre page/scale
for the engine's staleness check and the perf overlay).

Budget check for the 60 fps scroll target at s = 2 on a 2560×1440 viewport: a new row of 3 tiles
per ~200 px of scroll; 3 × (0.3 ms render + 0.2 ms encode + ~1.5 ms transport + ~1 ms decode) ≈ 9 ms
of *background* work per row, 0 ms on the compositor thread. Placeholder for a page entering: 1 ms
render + 0.5 ms encode + ~3 ms transport → sharp-ish within one frame, sharp tiles within 3–4 frames.

### 3.7 Thumbnails and the organiser grid

Both use `/thumb` with the cell width. 500 pages at 200 px ≈ 2.8 ms each on `Lane::Thumb` = 1.4 s
if all were requested — they are not: the sidebar/grid is virtualised (§4.4) and requests only
visible rows ± 1 screen (≈ 30 thumbs ≈ 90 ms). Grid layout comes from `DocInfo.pages` (page sizes)
before any pixel exists, so cells never reflow. After a page-op batch the gen bumps and only the
visible thumbs re-request (the browser cache keeps the rest keyed by the old gen, which is useless,
so `page_ops` responses include `changedPages` and the frontend evicts only those URLs from its
mounted set).

### 3.8 Forms, annotations and the "baked vs live" rule

pdfium bakes annotations (FPDF_ANNOT) and form widgets (FPDF_FFLDraw) into every tile. The SVG
overlay (§4.7) draws only: the annotation being created, the selection handles, and a *ghost* of an
annotation while it is dragged/resized. On commit the engine mutates, bumps gen, and the affected
page's tiles re-request (2–10 ms + transport); the ghost is removed when the first new tile lands
(`img.onload`), so there is no flash. Form text fields are edited in an HTML `<input>`/`<textarea>`
overlay (IME-safe for Korean) and committed through `FORM_On*` on blur (§5.2).

### 3.9 Telemetry

`engine_stats()` (every 500 ms while the dev overlay ⌥⌘D is open): queue depth per lane, jobs
dropped as stale, tile p50/p95 render ms, encode ms, cache hit rate, `PageLru` size, RSS. The
frontend adds `PerformanceObserver` frame timing and a scroll-jank counter. These numbers are
written to `docs/perf/baseline.json` by the harness (§9.5); a PR that regresses p95 tile latency by
> 20 % fails CI.

---

## 4. Frontend architecture

### 4.1 Stack

React 19 + TypeScript + Vite 8 (existing). Additions (all tiny, tree-shakeable): **zustand** (state,
~1 kB), **lucide-react** (icons), **@tauri-apps/plugin-dialog / -fs / -store / -window-state**
(typings only; the raw `invoke('plugin:dialog|open', …)` calls work today). No i18next (a 60-line
`t()` over two flat JSON files), no react-window (the layout is bespoke), no canvas library, no CSS
framework (CSS modules + design tokens from `ux-research.md` §6).

### 4.2 Directory layout and component tree

```
src/
  main.tsx                     mount, theme bootstrap, show window after first paint
  app/                         shell: App.tsx, TitleBar.tsx, ModeSwitcher.tsx, ToolStrip.tsx, StatusBar.tsx, Sidebar/*, PropertiesPanel/*, Welcome.tsx, Settings.tsx
  store/                       zustand slices (§4.3)
  ipc/                         typed client: commands.ts (invoke wrappers), events.ts, tileUrl.ts, types/*.ts (§7)
  viewer/                      Viewer.tsx (scroll container), PageShell.tsx, geometry.ts, layout.ts, TileManager.ts, zoom.ts, scrollAnchor.ts
  viewer/text/                 TextLayer.ts (typed-array model), Selection.ts, SelectionOverlay.tsx, SearchHighlights.tsx
  viewer/annot/                AnnotOverlay.tsx (SVG), shapes/*.tsx, handles.tsx, InkCanvas.tsx, NotePopover.tsx, PropertiesPopover.tsx
  viewer/forms/                FormOverlay.tsx (HTML inputs over widgets)
  tools/                       ToolController.ts (mode/tool state machine), tools/*.ts (one file per tool: highlight, ink, rect, …)
  edit/                        UndoStack.ts, ops.ts (invertible op descriptors)
  organize/                    OrganizeGrid.tsx (⌘4 mode), dragReorder.ts
  search/                      SearchController.ts (streamed results), SearchPanel.tsx
  ocr/                         OcrDialog.tsx, OcrController.ts, tesseractPool.ts, ocrTypes.ts
  export/                      ExportDialog.tsx, SaveController.ts
  i18n/                        ko.json, en.json, t.ts, useT.ts
  ui/                          primitives: Button, Segmented, Slider, Popover, Menu, Toggle, Tooltip, tokens.css, theme.css
  keyboard/                    shortcuts.ts (table from ux-research §4.7), useShortcuts.ts
public/ocr/                    worker.min.js, tesseract-core-simd-lstm.wasm.js, tesseract-core-lstm.wasm.js, eng.traineddata.gz, kor.traineddata.gz
public/fonts/                  NotoSansKR-subset.woff2 (UI, lazy), NotoSansKR-subset.ttf (embedding, fetched by Rust from resources instead — see §5.6)
```

Component tree (viewer mode):

```
<App>
  <TitleBar/> <ToolStrip/>                      (chrome, re-renders on mode/tool change only)
  <Sidebar> <Thumbnails/> | <Outline/> | <AnnotList/> | <SearchPanel/> </Sidebar>
  <Viewer>                                       (one scroll container, overflow:auto, native scroll)
    <div class="spacer" style="height: totalH">  (absolute page shells for visible ± 1.5 screens)
      <PageShell page=i>                         (position:absolute; left/top/width/height from layout)
        <div class="tiles" ref=TileManager>      (imperative: <img> placeholder + <img> tiles)
        <SelectionOverlay/> <SearchHighlights/>   (absolute divs, only when non-empty)
        <AnnotOverlay/>                          (SVG, viewBox in PDF user space via the page matrix)
        <FormOverlay/>                           (양식 mode only)
      </PageShell> …
  <PropertiesPanel/> <StatusBar/> <PerfOverlay/>
```

### 4.3 State (`src/store/*.ts`, zustand)

```ts
interface DocState {            // store/docs.ts
  docs: Record<DocId, DocInfo>; activeDocId: DocId | null;
  open(path: string, password?: string): Promise<DocId>; close(id: DocId): Promise<void>;
  applyDocChanged(ev: DocChangedEvent): void;   // gen + page metas replaced; TileManager subscribes
}
interface ViewState {           // store/view.ts — hot path; only *settled* values live here
  mode: 'read' | 'annotate' | 'edit' | 'pages' | 'forms';
  layout: 'single' | 'continuous' | 'two-page';
  zoomPercent: number; zoomMode: 'custom' | 'fit-page' | 'fit-width' | 'actual';
  dpr: number; viewRotation: 0 | 90 | 180 | 270; night: 'off' | 'dark' | 'sepia';
  currentPage: number; visibleRange: [number, number];       // updated on settle, not per frame
}
interface SelectionState { docId; anchor: CharRef | null; focus: CharRef | null; selectedAnnots: AnnotId[]; selectedPages: number[] }
interface AnnotState { byPage: Record<number, Annot[]>; pending: Record<AnnotId, Annot> }  // optimistic ghosts
interface UiState { sidebar: { open, tab, width }; panel: { open }; theme; locale; toolDefaults: Record<ToolId, ToolStyle> }
interface JobState { jobs: Record<JobId, JobProgress> }
```

What is *not* in zustand: scroll position, in-progress zoom scale, tile inventory, ink strokes
being drawn, pointer state — all in refs/classes so React never re-renders per frame.

### 4.4 Viewer: layout, virtualisation, scroll, zoom (`src/viewer/`)

`layout.ts` computes, for `(pages: PageMeta[], zoomPercent, layout, viewRotation, containerWidth)`,
an array `PageLayout { x, y, w, h }` in CSS px plus prefix sums of row bottoms. Continuous mode is a
single column (16 px gap); two-page mode pairs pages; single mode shows one page, but the same
container is used so ⌘2/⌘3 switch without remounting. Visible range =
`binarySearch(rowBottoms, scrollTop − 1.5·viewportH) … binarySearch(rowBottoms, scrollTop + 2.5·viewportH)`;
it is recomputed in a rAF handler on `scroll` and committed to `ViewState.visibleRange` **only when it changes**.
PageShells for that range are keyed by page index so React reuses them across scrolls.

Zoom: `zoom.ts` keeps the *gesture* scale in a ref. During a wheel-zoom/pinch/⌘± sequence the
`.tiles` container of each mounted shell gets `transform: scale(k)` with `transform-origin` at
the gesture point (GPU, ≤ 1 ms), and `scrollAnchor.ts` records the document-space point under the
cursor. On settle (120 ms of no gesture) `zoomPercent` is committed, the layout recomputed, scroll
position restored so the anchor point stays put, and TileManager requests tiles at the new
`scaleKey` while leaving the CSS-scaled old tiles in place until each new tile's `onload`.
Fit-width/fit-page recompute on container resize with a `ResizeObserver`, debounced 100 ms.

### 4.5 `TileManager` (`src/viewer/TileManager.ts`, imperative)

```ts
class TileManager {
  constructor(docId: DocId, container: HTMLElement, getGen: () => DocGen);
  setViewport(v: { scaleKey; rot; night; visibleRange; centrePage; centrePx; velocity }): void;  // called on settle
  mountPage(page: number, shell: HTMLElement): void;   // creates .ph <img> and the tile grid lazily
  unmountPage(page: number): void;                     // removes <img>s → browser may drop decoded bitmaps
  onDocChanged(gen: DocGen, changedPages: number[]): void; // re-requests only changed visible pages
  private request(tile: TileRef, lane: Lane, priority: number): void;   // creates <img decoding="async">, src = tileUrl(...)
  private inflight = new Set<string>(); readonly maxInflight = 24;
}
```

`request()` orders by the rules in §3.6, sets `img.src` only while `inflight.size < maxInflight`,
and keeps a small FIFO of deferred requests that is re-filtered against the current viewport
before being issued (so scrolled-away tiles are never loaded). Each `<img>` is absolutely
positioned at `tx·512/dpr, ty·512/dpr` CSS px inside `.tiles`, with `width = tileW/dpr`. The
placeholder `<img class="ph">` sits beneath with `width: 100%`. Old-scale tiles get
`data-stale="1"` and are removed when the new tile at the same grid position loads.

### 4.6 Text layer, selection, search (`src/viewer/text/`, `src-tauri/src/text/`)

Rust builds one `TextLayer` per page (5.6 ms/page incl. word/line grouping; the spike's
`build_text_layer` is the reference implementation), cached per gen, and serialises it as a single
little-endian buffer returned through `tauri::ipc::Response` (binary, ~28 B/char):

```
u32 magic 'STXL' | u32 version=1 | u32 page | u32 nChars | u32 nWords | u32 nLines | u32 flags | u32 reserved
f32[6]  matrix     page(user space, unrotated) → CSS px at zoom 100 % (crop box + /Rotate; spike-verified vs FPDF_PageToDevice)
chars   nChars × { u32 codepoint; f32 l, b, r, t (loose box, user space); u16 flags (generated|hyphen|space); u16 fontSizeX10; u32 objectRef }
words   nWords × { u32 firstChar; u32 nChars; u32 line }
lines   nLines × { u32 firstWord; u32 nWords; f32 baseline; f32 l, b, r, t }
```

`TextLayer.ts` wraps the typed arrays; `hitTest(x, y)` (CSS px → user space via the inverse
matrix → binary search on lines by y → linear scan of ≤ ~100 chars) runs in µs;
`rangeRects(a, b)` unions loose boxes per line (40 µs measured for 111 chars). Selection is a char
index range in `SelectionState`; `SelectionOverlay` renders one absolutely positioned div per line
rect; copy builds the string from code points (generated `\r\n` kept as line breaks). No DOM node
per character, ever. The text layer is fetched for the page under the cursor at `Lane::Interactive`
when a text tool is armed, and for visible pages ±1 in the background.

Search (`search_start`): the engine walks pages outward from `fromPage` (+1, −1, +2, −2 …); the
first 4 pages are submitted on `Lane::Interactive`, the rest on `Background`, all sharing one
`JobToken`. Per page it ensures the plain text exists (`page.text().all()` 0.66–1.4 ms, or the
cached layer), runs a `char`-based case-folded scan (65 µs/page) and sends
`SearchEvent::Page { page, hits: [{ charStart, len, rects, context }] }` through the
`tauri::ipc::Channel` (42k–100k msg/s). Hit rects come from the text layer (built on demand for
pages with hits only). A new query cancels the old token. Whole-word and case options are
implemented in Rust; pdfium's `FPDFText_FindStart` is not used (rects only, no char index, 25 ms).

### 4.7 Annotation overlay and tools (`src/viewer/annot/`, `src/tools/`)

`AnnotOverlay` is an `<svg>` per page with `viewBox="0 0 w_pt h_pt"` and a `<g transform>` that
flips y (`matrix(1 0 0 -1 0 h_pt)`), so every shape is authored in PDF user space and needs no
per-zoom recomputation — zoom changes the SVG's CSS size only. It draws (a) ghosts of
`AnnotState.pending` (optimistic creates, ≤ 16 ms target), (b) the selected annotation's outline
+ 8 handles, (c) the drag ghost. Ink in progress is drawn on a transparent `<canvas>` overlay
(`InkCanvas.tsx`) with `pointerrawupdate`/coalesced events; on `pointerup` the stroke is simplified
(Douglas–Peucker, ε = 0.35 pt) and committed. Hit-testing of *baked* annotations uses the JSON
annotation list (`list_annotations`, rects/quads/ink paths from the engine, cached per gen), not
pixels.

`ToolController` is a small state machine: `{ mode, tool, sticky }` with the momentary-vs-latched
key behaviour from `ux-research.md` §4.6 (held key = momentary, tap = latch, Esc = select). Each
tool is a class `Tool { onPointerDown/Move/Up, cursor, render(ghost) }` with no React inside; it
produces an `EditOp` (§4.9) on commit.

### 4.8 Forms overlay (`src/viewer/forms/FormOverlay.tsx`)

In 양식 mode, `list_form_fields` supplies `{ id, type, rect, value, options, flags }` per visible
page. Text/combo/list fields get positioned HTML controls (font-size from the field's `/DA` size
× zoom); their value is committed with `set_form_field_value` on blur/Enter (§5.2), after which
the page's tiles refresh (the widget AP now contains the value). Checkbox/radio are single clicks
→ engine click. Field highlight (`highlight_all_form_fields`) is a render flag in the tile key
(`hl=1`), toggled from the tool strip.

### 4.9 Undo/redo (`src/edit/UndoStack.ts`)

Frontend command pattern; every mutation is an `EditOp { do(): Promise<Result>, undo(): Promise<void>, coalesceKey? }`.
Inverses are built from the engine's response, not guessed:

| Op | Inverse | Cost |
|---|---|---|
| create annotation (returns id) | `delete_annotation(id)` | 0.05 ms |
| update annotation (response carries `previous`) | `update_annotation(previous)` | ≤ 0.1 ms (+ SetAP(NULL) recipe) |
| delete annotation (response carries the full `Annot`) | `create_annotation(annot)` with the same `/NM` | 0.05 ms |
| form value (response carries previous value) | set previous | ≤ 2 ms |
| page delete (response carries `graveyardId`) | `page_ops([{ kind: 'restore', graveyardId, at }])` | 0.4 ms |
| page move / rotate / insert blank / duplicate | move back / rotate −θ / delete / delete | 0.02–2 ms |
| OCR apply, redaction apply, flatten | not undoable — confirmed dialogs, `UndoStack.clear()` | — |

Depth 100; coalescing for slider drags (opacity/thickness) by `coalesceKey` within 500 ms.
Undo latency target ≤ 50 ms is met because every inverse is one engine op plus a tile refresh.

### 4.10 i18n, theme, keyboard

- `src/i18n/t.ts`: `t(key, params?)` with `{{name}}` interpolation and `_one/_other` plural
  suffixes; `ko.json`/`en.json` are the 373-key catalogue from `ux-research.md` §5;
  `scripts/check-i18n.mjs` fails CI on key drift. Locale from `navigator.language`, override in settings.
- Theme: tokens in `ui/tokens.css`, `data-theme` on `<html>`, `prefers-color-scheme` default.
  Night mode is a separate `ViewState.night` applied to the tile layer only (§3.2).
- Keyboard: one `shortcuts.ts` table (macOS/Windows columns from §4.7 of the UX spike) consumed by
  `useShortcuts` on the viewer root; single-letter tool keys only when the canvas has focus and no
  input is active; `Space` = page-down on tap (< 200 ms, no movement), pan on hold+drag.

---

## 5. Document editing model (engine side: `src-tauri/src/edit/`)

All editing runs on the engine thread on `Lane::Edit`, one `DocGen` bump per command (a
`page_ops` batch is one bump). Pages are held with `PageLru` and are in
`PdfPageContentRegenerationStrategy::Manual`; `regenerate_content()` is called explicitly only
after page-object edits (redaction, text/image objects, OCR layer) — never for annotation or form
work (it would rewrite the content stream on every create).

### 5.1 Annotations (`edit/annots.rs`, `edit/annot_ffi.rs`)

The high-level pdfium-render annotation API is used only for enumeration and for creating the
subtypes it supports; every property write goes through a thin raw module (`annot_ffi.rs`) because
of the verified crash (`set_fill_color`/`set_stroke_color` on an annotation with an `/AP` →
SIGSEGV) and the missing setters (opacity, border, ink list, DA, flags, AP).

Create pipeline (spike-verified order): `FPDFPage_CreateAnnot` → `FPDFAnnot_SetRect` →
`FPDFAnnot_SetColor(C)` and `(IC)` **with identical alpha** (every SetColor rewrites `/CA`) →
`FPDFAnnot_SetBorder(0, 0, width)` → quads in **TL, TR, BL, BR** order (`quad_tl()`; never
`PdfQuadPoints::from_rect()`) / `FPDFAnnot_AddInkStroke` / `AppendObject` → `FPDFAnnot_SetFlags(PRINT)`
→ `SetStringValue` for `T`, `Contents`, `NM` (uuid v4), `Subj`, `CreationDate` → **one thumbnail-size
render of the page before any save** so pdfium generates and persists the `/AP` (saving without a
render leaves markup annotations invisible in Acrobat/Preview). The tile refresh after the gen bump
performs that render for free.

Edit pipeline: `FPDFAnnot_SetAP(annot, NORMAL, NULL)` first → set rect/quads/colour/opacity →
next render regenerates the AP (persisted, verified). Moves of pdfium-generated markup APs are
re-mapped automatically; the SetAP(NULL) step makes third-party APs follow too.

Subtype mapping for the P0 tool set:

| Tool | PDF subtype | Notes |
|---|---|---|
| 형광펜/밑줄/취소선 | Highlight/Underline/StrikeOut | quads from the text layer's line rects; `/Rect` = union |
| 메모 | Text | pdfium note-icon AP; popup drawn by the UI (`NotePopover`), never a Popup annot (unlinkable, never rendered) |
| 펜 | Ink (`/InkList` via `AddInkStroke`) | width via SetBorder; eraser = delete whole strokes it touches (v1) |
| 사각형 | Square | high-level create + raw colour/border |
| 타원 | Circle | raw `FPDFPage_CreateAnnot(CIRCLE)` (no high-level constructor) |
| 선/화살표 | **Ink with 2 points** + private `/SeePDFLine "x1 y1 x2 y2 heads"` string | PDFium cannot create Line annots (CreateAnnot → NULL); arrowheads are extra strokes. Re-opened by SeePDF as a line; other viewers see ink. Documented limitation. |
| 텍스트 상자 | FreeText with `/DA "r g b rg /Helv size Tf"` for Latin-1; **Stamp + text object** with the bundled Korean subset font otherwise | FreeText without DA is invisible after save; Stamp+text renders 한글 (needs the font, §5.6) |
| 도장 / 서명 | Stamp + image object (`PdfPageImageObject::new_with_size`, transform **before** `add_image_object`, `set_bounds` first) | signature images stored in the app store |
| 링크 | Link with `FPDFAnnot_SetURI` | go-to-page links: P2 (needs lopdf) |

Annotation JSON (`Annot`, §7) is produced by `list_annotations` from the high-level enumeration plus
raw reads for `/CA`, border, ink lists, line endpoints, `/SeePDFLine`. Widgets are listed separately.

### 5.2 Forms (`edit/forms.rs`)

pdfium-render initialises the form-fill environment at load; the raw `FPDF_FORMHANDLE` comes from
`PdfForm::raw_handle()` (patch). Values are **never** written with `PdfFormTextField::set_value`
(writes `/V` only, no appearance → invisible everywhere). `set_form_field_value`:

```
text:      FORM_OnAfterLoadPage (once per page load) → FORM_OnLButtonDown/Up at the widget centre → FORM_SelectAllText → 
           for each UTF-16 unit of the new value: FORM_OnChar(form, page, ch, 0) → FORM_ForceToKillFocus   (Korean verified)
checkbox/radio: FORM_OnLButtonDown/Up at the widget centre; state read back with FPDFAnnot_IsChecked
combo/list:     FORM_SetIndexSelected(form, page, index, selected) → FORM_ForceToKillFocus
```

The regenerated `/AP` (896 bytes measured) renders via `FPDF_FFLDraw` in the tiles and persists.
Push-button JS/URI actions stay inert (no FFI callbacks). XFA is unsupported (this pdfium build).

### 5.3 Page operations (`edit/pages.rs`)

One `page_ops(docId, ops[])` command applies a batch in order with `PageLru::flush()` first:

| Op | Implementation | Measured |
|---|---|---|
| move | raw `FPDF_MovePages(doc, indices, len, dest)` after Rust-side validation (no dups, all < n, dest+len ≤ n); `false` → reload document from a `save_to_bytes` taken before the batch | 0.02 ms |
| delete | `copy_page_from_document` into the graveyard doc (undo), then `PdfPage::delete()` descending by index | 2.2 ms/page |
| rotate | `set_rotation` (Manual strategy, one `regenerate_content` per page) | ≈ 0.3 ms |
| insert blank | `create_page_at_index(PdfPagePaperSize::a4()/from_points(...))` | 0 ms |
| duplicate | raw `FPDF_ImportPagesByIndex(doc, doc, [i], 1, i+1)` (verified src == dest works) | ≈ 0.4 ms |
| insert from file | `copy_pages_from_document(&src, "1-3,5", at)` — **1-based** range string; internal ranges use `copy_page_range_from_document(0-based RangeInclusive)`; never mix | 0.65 ms/4 pages |
| extract / split | **never** "new doc + import" (drops `/AcroForm`): `save_to_bytes` → `load_pdf_from_byte_vec` → delete the other pages → raw save | 5 ms/MB + 2 ms/page |
| merge (new document) | `create_new_pdf` + `copy_pages_from_document` per source; warn in the UI when a source has form fields/outline (not preserved until lopdf) | 0.65 ms/4 pages |

### 5.4 Text and image objects (P1 add / P2 in-place)

Add text (P1): `create_text_object(x, y, text, font, size)` with Helvetica for Latin-1, the bundled
Korean subset font (`load_true_type_from_bytes(bytes, true)`, once per document) otherwise; one
object per line (no reflow). Add image: `create_image_object(x, y, &DynamicImage, Some(w), None)`.
In-place edit (P2): the spike's model — `set_text` only for base-14/fully-embedded fonts that pass
the trial coverage check (`page.text()` after `set_text`: every char `loose_bounds().width() > 0`),
else replace-object with matrix/colour copied; XObject-form text and fonts without ToUnicode are
read-only (badge). Always `regenerate_content()` before save.

### 5.5 Redaction (P1, `edit/redact.rs`)

Approach A (verified): for each mark rect, remove every text object whose bounds intersect
(`remove_object_at_index`, descending), re-create the surviving characters of partially covered
objects as new text objects (font via `text_object.font()`, size via `unscaled_font_size()`,
positions from char `origin()`), blank intersecting image pixels (`set_image` on a modified copy),
add a black `create_path_object_rect`, `regenerate_content()`, then **verify by re-extracting** the
page text and failing the command if any marked string survives. `PdfPageAnnotation::Redact` cannot
be created; pending marks live in the frontend until 적용.

### 5.6 OCR text layer (`ocr/layer.rs`)

Per line: `font_size_pt = line_height_px × 72/dpi`; per word: `PdfPageTextObject::new(&doc, word, font, size)`,
`set_render_mode(Invisible)`, natural width from `bounds()` (works before adding), `scale(box_w/natural_w, 1)` clamped
to [0.5, 2.0], `translate(x, baseline)`, `add_text_object`; `Manual` strategy + one `regenerate_content()`
(2 ms for 730 words vs 81 ms automatic). Fonts: Helvetica for Latin-1-only words; for Hangul the
**glyphless CID font via `load_cid_type2_font`** (Phase-0 patch; tiny, language-agnostic, the
standard pdf.ttf technique) — fallback until the patch lands: the bundled Noto Sans KR subset
(needs `subsetter` + `ttf-parser`; whole-file embedding of a system font is +6–31 MB and AppleGothic
maps spaces to TAB, so both are forbidden). Rotated pages: OCR boxes are in display pixel space;
map with `page.pixels_to_points(x, y, &same_render_config)` and rotate the object counter-clockwise
by `/Rotate` (unit test on `fixtures/rotation.pdf` before shipping).

### 5.7 Save, Save As, dirty tracking, backups (`edit/save.rs`, `io` thread)

- Dirty = `doc.gen != doc.saved_gen`; the status bar shows 저장됨 / 저장되지 않은 변경 사항; ⌘W and
  quit are guarded (`beforeunload`-equivalent through Tauri's `CloseRequested`).
- **Strategy** (`SaveRequest.strategy = auto`): `FPDF_INCREMENTAL` when the target is the file we
  opened, the file is ≥ 8 MB and `EditHistory` since `saved_gen` contains only annotation/form
  edits (0.2 ms + append-sized write → the ≤ 300 ms target on 500 pages); otherwise a full rewrite
  (`flags = 0` / `FPDF_NO_INCREMENTAL`, 5 ms/MB, drops unreferenced objects). Save As always full.
  Raw `FPDF_SaveAsCopy(doc, &mut FileWriter, flags)` with the spike's `#[repr(C)] FileWriter`
  (`FPDF_FILEWRITE` prefix + `Vec<u8>`); flag values in build 8057: INCREMENTAL 1, NO_INCREMENTAL 2,
  REMOVE_SECURITY 4 (3 is deprecated).
- **Atomic pipeline**: engine serialises into memory (blocks tiles for 5 ms/MB only) → `io` thread
  writes `<dir>/.<name>.seepdf-tmp`, `fsync` → engine **verifies** by `load_pdf_from_byte_vec` of the
  written bytes (page count == expected, `FPDF_GetLastError == 0`) → for reader-backed documents:
  drop pages, drop `PdfDocument` (closes the `File`) → `rename(tmp, target)` (atomic on APFS/NTFS) →
  reopen the document (0.2–1.5 ms), `PageLru` empty, `saved_gen = gen`, `doc:changed`. On any
  failure the original is untouched and the temp is deleted.
- **Backups**: before the first successful save of a session per file, copy the original to
  `$APPDATA/SeePDF/backups/<sha1(path)>-<mtime>.pdf` (keep 3, 200 MB cap) — cheap insurance against
  the "full rewrite lost something" class of bug; visible in 설정 › 백업.
- **Autosave/recovery** (P1): every 60 s while dirty, `FPDF_INCREMENTAL` to
  `$APPDATA/SeePDF/recovery/<id>.pdf` (0.2 ms + write), offered on next launch.
- Encrypted input: `save` keeps encryption (flags 0); 비밀번호 제거 = `FPDF_REMOVE_SECURITY` (P1);
  set/change password is out of v1 (no pdfium API).

### 5.8 Export and print (`edit/export.rs`)

Images: per page a `Background` job renders at `dpi/72` (150 DPI Letter = 1275×1650, 4.6 ms) and
hands the raw bitmap to the encode pool, which writes `<name>-<page>.png|jpg` and reports on the
`Channel<JobEvent>`; the dialog shows an estimated size (PNG ≈ 1.17 MB/page at 150 DPI for text,
JPEG q85 ≈ 0.4 MB). Text: `page.text().all()` per page (1.4 ms). Flattened PDF: `save_to_bytes`
copy → per page raw `FPDFPage_Flatten(page, FLAT_NORMALDISPLAY)` (never `PdfPage::flatten()`,
which uses FLAT_PRINT and deletes non-printable annotations; also gated behind a crate feature) →
`FPDFPage_GenerateContent` → save. Print (v1): flatten to a temp file and hand it to the OS default
PDF handler's print path via `tauri-plugin-opener`; a native `NSPrintOperation`/`StartDoc` dialog is P1.

---

## 6. OCR pipeline (`src-tauri/src/ocr/`, `src/ocr/`)

Decisions follow `docs/spikes/ocr.md`: **macOS → Vision primary** (CER 0.21 % Korean, 448 ms/page,
0 bytes), **tesseract.js 7 WASM fallback and Windows baseline** (bundled offline, 8.4 MB), one
engine-agnostic payload:

```ts
interface OcrPage { page: number; dpi: number; widthPx: number; heightPx: number; rotation: 0|90|180|270;
  lines: { text: string; bbox: Box; baseline?: [number, number, number, number];
           words: { text: string; bbox: Box; confidence: number }[] }[] }   // Box = [x0, y0, x1, y1] image px, origin top-left
```

Flow (`OcrController.ts`):
1. Dialog (범위, 언어 kor+eng, 레이아웃 자동/단일 단/단일 블록 for Tesseract only, "이미 텍스트가 있는 페이지 건너뛰기").
2. For each page (bounded pipeline, N = worker count ≤ 4 in flight so memory stays at N bitmaps):
   - macOS: `ocr_recognize_native({docId, page, dpi})` — engine renders gray/BGRA at 300 DPI
     (12–21 ms) → `ocr-native` thread builds a `CGImage` from the bytes (`CGDataProvider::with_data`,
     no PNG) → `VNRecognizeTextRequest` (`.accurate`, `["ko-KR","en-US"]`, `usesLanguageCorrection`)
     → `OcrPage` (word boxes via `boundingBoxForRange`, UTF-16 ranges, normalised coords flipped).
     Requires `objc2-vision` + `objc2-core-graphics` (not yet in Cargo.toml).
   - Windows / fallback: the worker pool (`tesseractPool.ts`, `createScheduler`, workers created
     with local `workerPath/corePath/langPath`, `gzip: true`, `cacheMethod: 'none'`) fetches
     `seepdf://…/ocr?doc&gen&page&dpi=300` (gray8 PNG, 0.2–0.9 MB) and calls
     `recognize(img, {}, { text: true, blocks: true })` with `tessedit_pageseg_mode` from the dialog,
     `user_defined_dpi: '300'`; spaces rebuilt from `line.text` (Korean word over-segmentation).
3. `ocr_apply_layer({docId, page: OcrPage})` per page on `Lane::Edit` (single-digit ms) → gen bump.
4. Progress on a `Channel<JobEvent>`; cancel = `job_cancel` (Rust `AtomicBool` between pages,
   `VNRequest.cancel()`) + `scheduler.terminate()` for Tesseract (no per-job cancel; recreate
   workers, 70–170 ms warm).

CSP additions required for the workers (verify in a `tauri build --debug` bundle first):
`worker-src 'self' blob:` and `script-src 'self' 'wasm-unsafe-eval'` (or `workerBlobURL: false`).
Bundle: `public/ocr/{worker.min.js, tesseract-core-simd-lstm.wasm.js, tesseract-core-lstm.wasm.js, eng.traineddata.gz, kor.traineddata.gz}` (4.0.0_best_int; `tessdata_fast` has 6× worse Hangul CER, the 4.0.0 legacy files produce garbage with the LSTM core).

---

## 7. IPC contract (`src/ipc/types/*.ts`, mirrored by `src-tauri/src/commands/*.rs`)

Conventions: commands are `snake_case` Rust fns invoked with camelCase args; every command that
touches pdfium is `async` and returns `Result<T, EngineError>` (`EngineError` serialises as
`{ code: string; message: string; page?: number }`); pixels go over `seepdf://` (§3.4); binary data
JS must process (`get_text_layer`, `ocr_render_page`) returns `tauri::ipc::Response` →
`ArrayBuffer`; progress uses `tauri::ipc::Channel<T>`; app-wide broadcasts use `emit`. Enum payloads
carry `#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]`.

```ts
// ---------- shared ----------
export type DocId = number; export type DocGen = number; export type JobId = number; export type AnnotId = string;
export type Rect = [l: number, b: number, r: number, t: number];      // PDF user space, points, y up
export type Quad = [x1,y1,x2,y2,x3,y3,x4,y4: number];                 // TL,TR,BL,BR (PDFium order)
export type Rgba = [r: number, g: number, b: number, a: number];       // 0..255
export type Rotation = 0 | 90 | 180 | 270;
export interface EngineError { code: 'passwordRequired'|'notFound'|'corrupt'|'stale'|'cancelled'|'busy'|'pdfium'|'io'|'unsupported'|'verifyFailed'; message: string; page?: number }
export interface PageMeta { w: number; h: number; rotation: Rotation; label: string | null }   // w/h = display (rotated) size in points
export interface DocInfo { id: DocId; gen: DocGen; path: string | null; title: string | null; pageCount: number; pages: PageMeta[];
                           hasForm: boolean; encrypted: boolean; permissions: Permissions; sizeBytes: number; source: 'bytes'|'reader'|'memory' }
export interface Permissions { print: boolean; modify: boolean; copy: boolean; annotate: boolean; fillForms: boolean; assemble: boolean }
export interface DocChangedEvent { docId: DocId; gen: DocGen; pages: PageMeta[] | null /* null = page list unchanged */; changedPages: number[] | 'all'; reason: string }
export type JobEvent = { type:'started'; jobId: JobId; total: number } | { type:'progress'; jobId: JobId; done: number; total: number; page?: number }
                     | { type:'done'; jobId: JobId; elapsedMs: number; result?: unknown } | { type:'cancelled'; jobId: JobId; done: number } | { type:'error'; jobId: JobId; error: EngineError };

// ---------- documents ----------
open_document(args: { path: string; password?: string })                      -> DocInfo            // throws {code:'passwordRequired'}
close_document(args: { docId: DocId })                                        -> void
list_documents()                                                              -> DocInfo[]
get_outline(args: { docId: DocId })                                           -> OutlineNode[]      // { title, page: number|null, children }
get_metadata(args: { docId: DocId })                                          -> Record<'title'|'author'|'subject'|'keywords'|'creator'|'producer'|'creationDate'|'modDate', string|null>
set_viewport(args: { docId: DocId; scaleKey: number; rot: Rotation; centrePage: number; first: number; last: number; velocity: number }) -> void   // fire-and-forget, bumps viewportGen
engine_stats()                                                                -> EngineStats        // queue depths, drops, tile p50/p95, cache hit rate, rss
take_pending_opens()                                                          -> string[]
add_recent(args: { path: string; page: number; zoomPercent: number })         -> void               // store plugin behind it
get_recent()                                                                  -> { path: string; title: string; page: number; zoomPercent: number; openedAt: string }[]

// ---------- text ----------
get_text_layer(args: { docId: DocId; page: number })                          -> ArrayBuffer        // §4.6 binary layout
get_page_text(args: { docId: DocId; page: number })                           -> string
search_start(args: { docId: DocId; query: string; matchCase: boolean; wholeWord: boolean; fromPage: number }, onEvent: Channel<SearchEvent>) -> JobId
//   SearchEvent = { type:'page'; page; hits: { charStart: number; len: number; rects: Rect[]; context: string; contextMatch: [number, number] }[] }
//               | { type:'done'; total: number; pagesScanned: number } | { type:'cancelled' }
job_cancel(args: { jobId: JobId })                                            -> boolean

// ---------- annotations ----------
export type Annot = AnnotBase & (
  | { type:'highlight'|'underline'|'strikeout'|'squiggly'; quads: Quad[] }
  | { type:'ink'; paths: number[][]; width: number; line?: { p1:[number,number]; p2:[number,number]; heads:[boolean,boolean] } }
  | { type:'square'|'circle'; border: number; fill: Rgba | null }
  | { type:'freeText'; text: string; fontSize: number; textColor: Rgba; align: 'left'|'center'|'right' }
  | { type:'note'; icon: 'comment'|'note'|'key' } | { type:'stamp'; kind: string; imageId?: string }
  | { type:'link'; uri: string; quads: Quad[] } | { type:'widget'; fieldId: string } | { type:'unsupported'; subtype: string });
export interface AnnotBase { id: AnnotId; page: number; rect: Rect; color: Rgba; opacity: number; author: string | null; contents: string;
                             created: string | null; modified: string | null; printable: boolean; hidden: boolean; readOnly: boolean }
export type NewAnnot = Omit<Annot, 'id'|'created'|'modified'|'page'>;
export type AnnotPatch = Partial<Pick<AnnotBase,'rect'|'color'|'opacity'|'contents'|'author'>> & { quads?: Quad[]; paths?: number[][]; width?: number; text?: string; fontSize?: number; fill?: Rgba|null; border?: number };
list_annotations(args: { docId: DocId; page: number })                        -> Annot[]
create_annotation(args: { docId: DocId; page: number; annot: NewAnnot; id?: AnnotId /* undo re-create */ }) -> { annot: Annot; gen: DocGen }
update_annotation(args: { docId: DocId; page: number; id: AnnotId; patch: AnnotPatch }) -> { annot: Annot; previous: Annot; gen: DocGen }
delete_annotation(args: { docId: DocId; page: number; id: AnnotId })          -> { previous: Annot; gen: DocGen }

// ---------- forms ----------
export interface FormField { id: string; name: string; type: 'text'|'checkbox'|'radio'|'combo'|'list'|'button'|'signature'|'unknown';
                             rect: Rect; value: string | boolean | number[] | null; options?: { label: string; value: string }[]; readOnly: boolean; required: boolean; maxLen?: number; multiline?: boolean; fontSize?: number }
list_form_fields(args: { docId: DocId; page: number })                        -> FormField[]
set_form_field_value(args: { docId: DocId; page: number; fieldId: string; value: string | boolean | number[] }) -> { field: FormField; previous: FormField['value']; gen: DocGen }
reset_form(args: { docId: DocId })                                            -> { gen: DocGen }   // P1

// ---------- pages ----------
export type PageOp = { kind:'move'; pages: number[]; to: number } | { kind:'delete'; pages: number[] } | { kind:'rotate'; pages: number[]; by: 90|180|270 }
                   | { kind:'insertBlank'; at: number; size: 'a4'|'letter'|'sameAs'; sameAs?: number } | { kind:'duplicate'; page: number }
                   | { kind:'insertFrom'; at: number; path: string; ranges: string /* 1-based "1-3,5" */; password?: string }
                   | { kind:'restore'; graveyardId: number; at: number };
page_ops(args: { docId: DocId; ops: PageOp[] })                               -> { info: DocInfo; changedPages: number[] | 'all'; graveyardIds: number[] /* one per deleted page, in order */ }
extract_pages(args: { docId: DocId; pages: number[]; outPath: string })       -> { bytes: number }
merge_documents(args: { inputs: { path: string; ranges: string; password?: string }[]; outPath: string }) -> { pages: number; warnings: ('formsDropped'|'outlineDropped')[] }

// ---------- save / export / security ----------
save_document(args: { docId: DocId; mode: 'save'|'saveAs'; path?: string; strategy?: 'auto'|'incremental'|'full' }, onProgress: Channel<JobEvent>) -> { path: string; bytes: number; strategy: 'incremental'|'full'; gen: DocGen; elapsedMs: number }
export_images(args: { docId: DocId; pages: number[]; format: 'png'|'jpeg'; dpi: number; quality?: number; outDir: string }, onProgress: Channel<JobEvent>) -> JobId
export_text(args: { docId: DocId; outPath: string })                          -> { chars: number }
export_flattened(args: { docId: DocId; outPath: string }, onProgress: Channel<JobEvent>) -> JobId
estimate_export(args: { docId: DocId; pages: number[]; format: 'png'|'jpeg'; dpi: number }) -> { bytes: number }   // renders 2 sample pages
remove_password(args: { docId: DocId; outPath: string })                      -> { bytes: number }   // P1, FPDF_REMOVE_SECURITY = 4
apply_redactions(args: { docId: DocId; page: number; rects: Rect[]; fill: Rgba }) -> { removedObjects: number; verified: boolean; gen: DocGen }   // P1
print_prepare(args: { docId: DocId })                                         -> { tempPath: string }

// ---------- OCR ----------
ocr_capabilities()                                                            -> { native: 'vision'|'windows'|null; languages: string[] }
ocr_render_page(args: { docId: DocId; page: number; dpi: number })            -> ArrayBuffer     // gray8, header { w, h } in X- headers; used by tesseract path via seepdf://…/ocr instead when possible
ocr_recognize_native(args: { docId: DocId; page: number; dpi: number; langs: string[] }) -> OcrPage   // macOS Vision
ocr_apply_layer(args: { docId: DocId; page: OcrPage; skipIfText: boolean })   -> { words: number; skipped: boolean; gen: DocGen }
page_has_text(args: { docId: DocId; page: number })                           -> boolean

// ---------- events (emit / listen) ----------
'open-file'    { path: string; source: 'argv'|'macos-opened'|'drop' }
'doc:changed'  DocChangedEvent                                                 // after every engine mutation, incl. save (gen unchanged, savedGen updated)
'doc:saved'    { docId: DocId; gen: DocGen; path: string }
'engine:pressure' { level: 'normal'|'high' }                                   // budgets halved; frontend lowers MAX_MOUNTED_TILES
```

`src/ipc/commands.ts` wraps each command in a typed function (`api.openDocument(path)`), converts
`EngineError` into a typed `SeePdfError`, and is the only file that imports `invoke`.
---

## 8. v1 feature list with acceptance criteria

Priorities follow `ux-research.md` §8. Each line has a measurable acceptance test that becomes a
CI check or a manual QA script step.

### P0 (v1 does not ship without these)

| # | Feature | Acceptance criteria |
|---|---|---|
| 1 | Open (path, drag-drop, Finder/argv, password), render, continuous/single/two-page, zoom modes, pinch, view rotation | tracemonkey first paint ≤ 250 ms; synthetic 500-page doc first paint ≤ 400 ms; wrong password → prompt, no crash; pinch keeps the anchor point within 2 px |
| 2 | Thumbnail sidebar (lazy) + outline sidebar | 500-page thumbs: placeholders ≤ 100 ms, visible thumbs ≤ 500 ms; outline click scrolls to page |
| 3 | Text selection/copy; streamed search with result list | selection rects match pdfium `FPDF_PageToDevice` within 1 px on `rotation.pdf`; "monkey" on tracemonkey: 62 hits, first event ≤ 150 ms |
| 4 | Annotations: highlight/underline/strikeout/note/ink+eraser/rect/ellipse/line/arrow/free text; select/move/resize/delete; colour/opacity/thickness; annotation list; undo/redo | each type persists after save+reopen with an `/AP` (checked by `FPDFAnnot_GetAP`), renders in pdfium and pdf.js; ghost visible ≤ 16 ms; recolouring a reopened annotation never crashes (SetAP(NULL) recipe test) |
| 5 | Save (incremental when eligible), Save As, unsaved guard, recent files with position | save of annotation-only edits on the 500-page fixture ≤ 300 ms; killing the process mid-save leaves the original intact (temp+rename test); reopened page count verified |
| 6 | Organize pages: reorder, delete, rotate, duplicate, insert blank/from file, extract, merge | 160F-2019.pdf extract keeps `form()` = Some (AcroForm preserved); reorder commit ≤ 16 ms; undo of delete restores identical text |
| 7 | Form filling (text/checkbox/radio/combo/list) + save + field highlight | value visible in pdfium and after reopen (AP regenerated); Korean input through the HTML overlay round-trips |
| 8 | OCR ko+en → searchable PDF, page range, progress, cancel, offline | Korean synthetic page: Vision CER ≤ 1 %, Tesseract PSM 4 CER ≤ 3 %; output searchable in pdfium (`search()` hit) and pdf.js; cancel stops within 1 page |
| 9 | Export PNG/JPEG, text, flattened PDF; print | 14 pages @150 DPI ≤ 400 ms; flattened file has 0 annotations and identical rendering (pixel diff ≤ 0.5 %) |
| 10 | ko/en i18n (373 keys), light/dark, welcome/recent | `check-i18n` passes; no literal strings in `src/` (lint rule) |
| 11 | §7 perf targets for open/scroll/zoom/search | perf harness in CI (§9.5) |

### P1 (v1 if schedule holds, else v1.1)

Signature (draw/type/image) · Redaction with verified removal (§5.5) · Remove password / permission
display · Add text & image objects · Night mode · Korean stamp set · Compress presets with estimate
(app-level image downsample; JPEG re-encode requires lopdf) · Split by range/every-N · Per-tool
default styles · Reading mode/full screen · Progressive render for heavy pages (§3.2) · Autosave/recovery ·
Native print dialog · Windows.Media.Ocr opportunistic path.

### P2 / explicitly out of v1

In-place editing of existing text (Korean glyph coverage problem), document tabs, link creation
with go-to destinations, outline editing, metadata writing, page labels, set/change password,
headers/footers/watermarks, batch OCR/export, compare, form authoring, Word/HWP export, annotation
threads, XFA, JS actions, Line/Polygon annotations as real subtypes (PDFium cannot create them).

---

## 9. Testing and CI

### 9.1 Rust (`cd src-tauri && cargo test`; fixtures from `../fixtures/`, pdfium from `resources/pdfium/`)

- `engine/scheduler` unit tests with a fake `run()`: lane ordering, FIFO within Edit, stale drop
  by generation, cancel between chunks, starvation guard, queue bound.
- `render/geometry`: tile grid == pdfium-render's rounding for 200 random (size, scaleKey) pairs;
  edge tiles; placeholder scale.
- `render` integration (`tests/render.rs`): tile vs full-render crop (≤ 0.1 % px differ, max delta 1)
  on tracemonkey p0 at 4× and alphatrans p0; `rotation.pdf` sizes; forms present in tiles for
  160F-2019 (non-white pixels inside a widget rect); night mode alpha.
- `text` (`tests/text.rs`): layer serialisation round-trip; matrix vs `FPDF_PageToDevice` on
  rotation 0/90 (and 180/270 fixtures generated by the test with `set_rotation`); search hits and
  indices vs pdfium `FPDFText_FindStart` counts (62/1/4 on "monkey").
- `edit/annots` (`tests/annots.rs`): create every P0 type → render → save → reopen → `/AP` present,
  quads TL/TR/BL/BR, recolour-with-AP does not crash (run in a child process like the spike), flatten
  NORMALDISPLAY keeps non-printable annots.
- `edit/forms`: FORM_* fill visible in FFLDraw render and after reopen; Korean value.
- `edit/pages`: move/delete/duplicate/insert; extract keeps AcroForm; 1-based vs 0-based range
  helpers; `FPDF_MovePages` validation rejects the three documented failure cases.
- `edit/save`: temp+rename atomicity (kill via `std::process::abort` in a child after the temp is
  written → original intact); incremental save reopens with the edit; verify step rejects a truncated file.
- `ocr/layer`: invisible words searchable after reopen; 0 dark pixels; `rotation.pdf` round trip.
- Memory: `tests/memory.rs` renders 200 tiles at 8× and asserts `TileCache.bytes ≤ budget` and
  RSS growth < 150 MB (macOS `mach_task_basic_info`).

### 9.2 TypeScript (`vitest`)

`layout.ts` (visible range, two-page pairing, fit modes), `geometry.ts` (tile grid parity with a
Rust-generated JSON table), `TileManager` (request ordering, in-flight cap, stale removal, with a
DOM stub), `TextLayer` (hit test, range rects on a serialised fixture layer), `UndoStack`
(inverse construction, coalescing), `SearchController` (event merging/ordering), `t()` + key parity,
`shortcuts` table conflicts, `ToolController` state machine.

### 9.3 Headless smoke and end-to-end

- `seepdf --smoke <file.pdf>` (a hidden CLI flag handled in `lib.rs` before the window is created):
  bind pdfium from the *bundled* resource dir, open the file, render page 0 at 2×, build the text
  layer, exit 0/1 — run against the built bundle on every CI target; proves resource resolution and
  library loading on x64/arm64/Windows.
- `tauri-driver` + WebDriver e2e on Windows and Linux runners (WebKitWebDriver is unavailable on
  macOS): open fixture, scroll to page 10, ⌘F "monkey", create a highlight, save, reopen, assert
  the annotation list. macOS gets the same script through a `--e2e-script` JSON command file
  executed by the app itself in a debug build (screenshots via `screencapture` as in the spike).
- Perf harness (`src-tauri/examples/perf_baseline.rs` + `scripts/perf-webview.mjs`): opens
  tracemonkey, TAMReview and a generated 500-page doc (`scripts/make-500.mjs` concatenating
  fixtures), records launch, open, first-paint, tile p50/p95, search first-hit, save times into
  `docs/perf/baseline.json`; CI compares with ±20 % tolerance on the mac-arm64 runner only (others
  informational).

### 9.4 GitHub Actions (`.github/workflows/ci.yml`)

```yaml
strategy: { matrix: { include: [ {os: macos-14, target: aarch64-apple-darwin}, {os: macos-13, target: x86_64-apple-darwin}, {os: windows-2022, target: x86_64-pc-windows-msvc} ] } }
steps:
  - actions/checkout, actions/setup-node@v4 (node 24, cache npm), dtolnay/rust-toolchain@stable (+ target), Swatinem/rust-cache@v2 (workspaces: src-tauri)
  - run: npm ci                      # postinstall fetches libpdfium for the host via scripts/fetch-pdfium.mjs (PDFIUM_TARGET=${{ matrix.target }})
  - run: npm run typecheck && npx vitest run && node scripts/check-i18n.mjs
  - run: cd src-tauri && cargo fmt --check && cargo clippy -- -D warnings && cargo test --release   # release: encoder timings are meaningless in debug
  - run: npm run tauri build -- --target ${{ matrix.target }} --bundles app|nsis  (tauri.macos.conf.json / tauri.windows.conf.json overlays ship only the host libpdfium)
  - run: node scripts/bundle-size.mjs --max-mb 60          # fails if the installed size exceeds 60 MB
  - run: <bundle>/seepdf --smoke fixtures/tracemonkey.pdf
  - run (macos-14 only): perf harness + compare to docs/perf/baseline.json
  - upload artifacts: bundles, perf json, e2e screenshots
```

A nightly job additionally runs the full OCR benchmark (`scripts/spike-ocr.mjs`) and the Vision
probe so accuracy regressions after a tesseract.js or macOS update are caught.

### 9.5 Bundle size budget (installed)

Tauri runtime + our binary ≈ 8–12 MB, libpdfium 7 MB (one per platform), OCR 8.4 MB, Korean subset
fonts ≈ 1.5 MB, UI ≈ 1 MB → ≈ 27–30 MB, well under the 60 MB target. `scripts/bundle-size.mjs`
enforces it and prints the top-10 contributors.

---

## 10. Work breakdown for 6–8 agents in git worktrees

Rules: each module owns its directories exclusively; nobody edits another module's files;
`src-tauri/src/lib.rs`, `Cargo.toml`, `tauri.conf.json`, `capabilities/*.json`, `package.json` and
`src/ipc/types/*.ts` are owned by the **integrator (M0)**, who applies requested changes from
PR descriptions within hours; every module ships a `register()`/command list the integrator wires
into `generate_handler!`. Shared cargo cache: `CARGO_TARGET_DIR=/Users/veri/Dev/SeePDF/src-tauri/target`.
Dev runs use `npm run tauri dev -- --no-watch`.

| Module | Owner | Exclusive files | Depends on | Definition of done (tests that must pass) |
|---|---|---|---|---|
| **M0 Integrator / Phase 0** | A | `src-tauri/vendor/pdfium-render/` (patch §2.6), `Cargo.toml` (+ crates in §12, `[profile.dev.package."*"] opt-level=3`), `lib.rs`, `commands/mod.rs`, `tauri.conf.json`, `tauri.{macos,windows}.conf.json`, `capabilities/`, `package.json`, `src/ipc/types/*.ts`, `src/ipc/commands.ts`, `.github/`, `scripts/{check-i18n,bundle-size,make-500}.mjs`, `docs/perf/` | — | Phase-0 PR: vendored crate compiles with `raw_handle()` accessors + a test that `FPDF_MovePages` works through it; CI green on the 3 targets with the `--smoke` flag |
| **M1 engine-core** | B | `src-tauri/src/engine/{mod,types,thread,scheduler,registry,page_lru,budgets,stats,error}.rs`, `commands/documents.rs` | M0 | §9.1 scheduler + registry tests; `open_document` on all fixtures incl. encrypted; drop-order test (close with 20 open pages, no UAF under `valgrind`-free sanity: run under `RUST_BACKTRACE` child process); `engine_stats` |
| **M2 render + protocol** | C | `src-tauri/src/render/{geometry,tiles,encode,cache,thumbs,classify}.rs`, `src-tauri/src/protocol/`, `src/viewer/{geometry,TileManager}.ts`, `src/ipc/tileUrl.ts` | M1 (job types) | §9.1 render tests; protocol e2e in a debug bundle: `immutable` caching verified on WKWebView + WebView2 (or fallback documented); 500-page fling shows no white gaps in the e2e video; tile p95 ≤ 10 ms on tracemonkey |
| **M3 viewer-core (TS)** | D | `src/viewer/{Viewer,PageShell,layout,zoom,scrollAnchor}.tsx/ts`, `src/store/{docs,view}.ts`, `src/organize/` | M2 interface (`TileManager` API), M0 types | vitest for layout/zoom; e2e: open, scroll, zoom anchor, layout modes, organiser drag reorder (model-only commit ≤ 16 ms); `set_viewport` cadence ≤ 1 call per settle |
| **M4 text + search** | E | `src-tauri/src/text/{layer,serialize,search,select}.rs`, `commands/text.rs`, `src/viewer/text/`, `src/search/`, `src/store/selection.ts` | M1 | §9.1 text tests; vitest `TextLayer`; e2e: select 3 lines and copy on tracemonkey, search streams 62 hits, first event ≤ 150 ms on the 500-page doc |
| **M5 annotations + forms** | F | `src-tauri/src/edit/{annot_ffi,annots,forms,flatten,redact}.rs`, `commands/{annots,forms}.rs`, `src/viewer/annot/`, `src/viewer/forms/`, `src/tools/`, `src/edit/`, `src/store/annots.ts` | M1, M4 (quads from text layer), M0 patch | §9.1 annots/forms tests incl. the child-process crash test; every P0 tool persists + renders in pdf.js; Korean free text via Stamp+text; undo/redo round trips; forms Korean input |
| **M6 pages + save + export** | G | `src-tauri/src/edit/{pages,merge,save,export,security}.rs`, `commands/{pages,save,export}.rs`, `src/export/`, `src/app/dialogs/{Export,Merge,Split}.tsx` | M1 | §9.1 pages/save tests incl. atomicity + incremental; extract keeps AcroForm; export 14 pages ≤ 400 ms; bundle-size script unaffected |
| **M7 OCR** | H | `src-tauri/src/ocr/{mod,vision_mac,layer,fonts}.rs`, `commands/ocr.rs`, `src/ocr/`, `public/ocr/`, `scripts/spike-ocr.mjs` | M1, M0 (CSP + macOS deps + subsetter crates) | Vision + Tesseract produce `OcrPage`; searchable output tests; CSP verified in bundle; cancel within 1 page; memory ≤ 4 workers |
| **M8 shell + i18n + theme + keyboard** | (D or a 8th agent) | `src/app/`, `src/ui/`, `src/i18n/`, `src/keyboard/`, `src/store/{ui,jobs}.ts`, `src/main.tsx`, `index.html`, welcome/recent/settings | M0 types | check-i18n; lint "no literal strings"; keyboard table vitest; cold launch ≤ 700 ms measured by the harness; theme switch without reflow |

Dependency order / phases:

1. **Phase 0 (M0, ~1 day)**: vendored patch, Cargo/npm deps, skeleton dirs with `todo!()` commands
   and the full `src/ipc/types`, mock engine for the frontend (`src/ipc/mock.ts` returning the
   fixture-derived `DocInfo` and blank tiles), CI skeleton. Everyone branches from this.
2. **Phase 1 (parallel)**: M1, M2, M4, M6, M7 on the Rust side; M3 and M8 on the frontend against
   the mock; M5 starts with `annot_ffi.rs` + tests (no UI dependency).
3. **Phase 2 (integration, pairwise)**: M2+M3 (real tiles in the viewer, perf harness first run),
   M4+M3 (selection/search UI), M5+M3 (tools over the overlay), M6+M8 (dialogs), M7+M8 (OCR dialog).
4. **Phase 3**: perf pass against `docs/perf/baseline.json`, P1 items by whoever finishes first, e2e.

Interface freeze points: `EngineHandle::submit` + `JobKind` (end of Phase 0), `TileManager` public
API and the tile URL grammar (Phase 1 week 1), `src/ipc/types` (any change is an M0 PR that all
owners review within the day).

---

## 11. Risks and mitigations

| Risk | Likelihood / impact | Mitigation |
|---|---|---|
| The pdfium-render patch is refused or drifts from upstream | medium / high (blocks page reorder, save flags, safe recolour, FORM_* fill, flatten mode, CID OCR font) | Patch is 40 lines and additive; kept as a `git diff` in `vendor/PATCH.md`; fallback = raw document mirror with reload-after-edit (§2.6), acceptable ≤ 20 MB |
| `Cache-Control: immutable` not honoured on a custom scheme (WKWebView or WebView2) | medium / low | First M2 task verifies in a debug bundle; fallback is the spike-verified `must-revalidate` + 304 served from the encoded cache (one extra ~1 ms round trip per revisit) |
| RSS high-water on macOS after a large export or a 4× whole page | high / medium | Tiles bound the largest allocation to 1 MiB (export streams one page at a time, whole-page mode capped at 2.5 Mpx); `engine:pressure` halves budgets; `engine_stats` visible in the perf overlay |
| WebKit keeps decoded tiles longer than we want (webview RSS) | medium / medium | `MAX_MOUNTED_TILES = 120`; unmount shells beyond ±1.5 screens; measured in the perf harness (`webview_rss`) |
| A single vector-heavy page (> 100 ms render) stalls scrolling | low / medium | Placeholder first; P1 progressive render with `IFSDK_PAUSE` every 8 ms; per-page `render_ms` history triggers it automatically |
| Korean free text / add text needs an embedded font: pdfium embeds whole files, AppleGothic breaks extraction | high / medium | Bundle Noto Sans KR (OFL), subset with `subsetter`, verify `cmap` with `ttf-parser`; until crates are approved, FreeText+DA for Latin only and Stamp+text with Helvetica fallback flagged in the UI |
| tesseract.js workers blocked by CSP in WebView2 | medium / high for Windows OCR | CSP additions in Phase 0; verified in a debug bundle by M7 before any OCR UI work |
| Incremental save produces a file some viewers dislike (prior xref + appended revision) | low / high | Verify step reopens with pdfium; nightly check with pdf.js and `qpdf --check` on CI; user-facing "최적화하여 저장" forces a full rewrite |
| Windows: `File`-backed documents lock the file; rename over it fails | medium / medium | Save closes the document before rename and reopens (§5.7); if reopen fails, fall back to the in-memory bytes |
| Line/arrow shipped as Ink | certain / low | Documented; private key keeps them editable in SeePDF; real Line subtype needs lopdf post-processing (P2) |
| Thread-safety assumptions if someone calls pdfium from a command thread | low / catastrophic | `PdfDocument`/`PdfPage` never leave `engine/`; `#![deny(clippy::disallowed_types)]` list with `pdfium_render::prelude::*` outside `engine/`, `render/`, `text/`, `edit/`, `ocr/layer` |
| Perf regressions creep in | high / medium | Harness numbers in CI with ±20 % tolerance; p95 tile latency and first-paint are hard gates |

---

## 12. Appendix

### 12.1 Crates and packages to add (Phase 0, M0)

Rust (all on the USTC index unless noted): `lopdf` (P1/P2: metadata, page labels, outline,
AcroForm re-attach, image stream replacement), `ttf-parser` + `subsetter` (Korean font subsetting
for free text / add text / OCR fallback), macOS-only `objc2 0.6`, `objc2-foundation 0.3`,
`objc2-core-foundation 0.3`, `objc2-core-graphics 0.3`, `objc2-vision 0.3` (features per
`docs/spikes/ocr.md` §3b), optional `tokio = { features = ["sync"] }` (real oneshot; not required),
Windows-only `windows 0.62` with `Media_Ocr` (P1). Not needed: `lru`, `rayon`, `memmap2`, `image`
beyond what is installed. The vendored `pdfium-render` path patch (§2.6).

npm: `zustand`, `lucide-react`, `@tauri-apps/plugin-{dialog,fs,store,window-state}`, `vitest`,
`@testing-library/react` (light use), `tesseract.js` (already present).

### 12.2 Measured numbers this design relies on (source: `docs/spikes/*.md`)

bind 2.1 ms · open 1 MB 0.15 ms · page_sizes 0.1 ms/14 pages · FPDF_LoadPage 0.46–26 ms · render
Letter 1×/2×/4×/8× = 0.95/2.24/6.54/38.9 ms · 512² tile at 8× 0.01–0.8 ms · tiling overhead ≤ 25 % ·
PNG Fast+Sub 2.29 ms/1.40 MB per 2× page · JPEG q85 12.7 ms/411 kB · raw copy 0.16 ms · IPC raw
141 MB/s, base64 17 MB/s, JSON 10 MB/s · Channel 42k–100k msg/s · crossbeam round trip 0.3 µs ·
text layer 5.6 ms/page · search 65 µs/page cached · annotation create 0.05 ms · save 5 ms/MB full,
0.2 ms incremental · FPDF_MovePages 0.02 ms · delete page 2.2 ms · thumb 200 px 2.8 ms · export
150 DPI 4.6 ms render + 4.4 ms encode per page · Vision Korean 448 ms/page CER 0.21 % · tesseract.js
kor+eng PSM4 1.5 s/page CER 2.7 % · RSS: lib 9 MiB, 14 pages + doc ≈ 25 MiB, 2× page RGBA 7.4 MiB.

### 12.3 Open questions for the owner

1. Approve the vendored pdfium-render patch (blocks M5/M6/M7 raw paths).
2. Approve the crate additions in §12.1 (Korean font pipeline and Vision).
3. Line/arrow as Ink for v1 — acceptable?
4. Print: OS-handler hand-off in v1, native dialog in P1 — acceptable?
