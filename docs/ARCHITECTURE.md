# SeePDF — Architecture (v1)

Synthesis of `docs/spikes/{render,text,annotations,pages,tauri,ocr,ux-research}.md` and the three
proposals in `docs/design/`. Every API claim here is one a spike compiled; every number carries its
unit and comes from a spike measurement (release profile unless stated).

Companion documents: `IPC_CONTRACT.md` (the backend/frontend seam), `FEATURES.md` (scope + acceptance),
`UI_SPEC.md` (shell, tokens, i18n), `WORKPLAN.md` (who builds what).

---

## 0. The decisions, with the evidence

| # | Decision | Evidence / reason |
|---|---|---|
| D1 | One dedicated `seepdf-engine` thread owns `Pdfium`, every `PdfDocument`, every `PdfPage`. No pdfium call anywhere else. | `thread_safe` locks only 289/481 FFI entry points (render §5); 2 threads give 0 % speed-up (58.9 vs 59.2 ms); `PdfPage<'a>` outliving its document compiles and is a UAF. |
| D2 | Drop the `thread_safe` feature. pdfium types become `!Send`, so the compiler enforces D1. | render §5: the feature is not a correctness guarantee; nothing in this design moves pdfium handles between threads. |
| D3 | Engine commands are boxed closures over `EngineState` on one `crossbeam-channel`, popped from a priority heap with lanes + viewport-generation stale-drop. | Channel round trip 0.3 µs (render §5); a FIFO made tiles wait ~30 ms behind a full-page render (tauri §2). Closures mean no shared `match` for parallel agents. |
| D4 | Pixels travel over the **`seepdf://` custom protocol** as PNG (`Compression::Fast` + `Filter::Sub`), with `gen` in the URL and immutable caching. Binary IPC (`tauri::ipc::Response`) is used only for data JS must process (text layer, raw page buffer). JSON/base64 never carries pixels. | tauri §6 recommendation 1: transport cost is equal, the protocol gives free webview caching, decode off the JS thread and no Blob/object-URL bookkeeping. Raw IPC 141 MB/s vs base64 17 MB/s vs JSON 10 MB/s. PNG encode is 0.2 ms per 512² tile, 2.29 ms per 2× page. |
| D5 | Whole-page bitmap while device scale `s ≤ 2.0` **and** page ≤ 2.5 Mpx; 512×512 device-px tiles via `set_origin(-tx,-ty)` above that. Never the matrix/`clip()` path. | render §3: tiling overhead ≤ 25 %, empty tiles cost 0.00 ms, 8× full page = 38.9 ms / 118 MiB; any `translate/scale/clip` silently sets `render_form_data = false`. |
| D6 | Undo/redo = **whole-document snapshots** (`save_to_bytes()`, 5 ms/MB) pushed by the one `Registry::mutate()` path, byte-budgeted with disk spill. | text §7 / pages §8: no incremental API in the safe layer. One mechanism covers annotations, forms, page ops, objects, OCR and redaction with zero inverse-op bookkeeping — the cheapest thing to build with six parallel agents. |
| D7 | Save is always a **full rewrite** (`FPDF_SaveAsCopy`, flags 0) written to a temp file in the same directory, fsynced, reopened and verified, then `rename`d. No incremental save in v1. | pages §8: flags 0 already drops unreferenced objects; 5 ms/MB meets the ≤ 300 ms target up to ~60 MB. `FPDF_INCREMENTAL` grows the file and is a second code path. |
| D8 | Annotations are created and edited through a **thin raw `FPDFAnnot_*` pipeline** on the live document; pdfium-render's high-level API is used for enumeration and for the constructors it has. | annotations §5.1: `set_fill_color`/`set_stroke_color` on an annotation that already has an `/AP` **SIGSEGVs**; opacity, border, ink list, `/DA`, flags, AP removal and Circle have no high-level API. |
| D9 | pdfium-render 0.9.4 is consumed through a **4-method visibility patch** (`[patch.crates-io]`). This is the only fork in the project. | The handles are `pub(crate)` (verified independently by the pages, annotations and text spikes) and P0 form filling (`FORM_*`) and page reorder (`FPDF_MovePages`) are impossible without them. §1.3 states the exact patch, the fallback, and what degrades without it. |
| D10 | OCR v1 = **tesseract.js 7 in a Web Worker pool**, `kor+eng`, fully bundled (8.4 MB). macOS Vision is P1 behind the same `OcrPage` contract. | ocr §1/§3a: bundled, offline, CER 2.1–2.7 % on Korean at PSM 4/6. Vision is better (0.21 %) but needs 5 unverified `objc2-*` crates; the contract makes it a drop-in later. |
| D11 | The OCR text layer and all "add text" paths use a **pre-subsetted Hangul TTF shipped in `resources/fonts/`**, loaded once per document. Never a system font. | text §5 / ocr §4.10: pdfium embeds the whole font file (AppleGothic +6.2 MB, AppleSDGothicNeo.ttc +31 MB) and AppleGothic maps space → TAB, breaking search. |
| D12 | Frontend: React 19 + zustand + lucide-react + a ~60-line `t()`; bespoke virtual scroller; text layer as typed arrays (no DOM node per char); annotations drawn by pdfium, SVG only for gestures/handles. | ux-research §4/§6/§7; a 5,087-char page would otherwise be 5,087 DOM nodes. |

---

## 1. Process and thread model

### 1.1 Threads

```
main (tauri/wry event loop)   window + native menu + seepdf:// handler entry (parse URL, forward, return in µs)
seepdf-engine (std::thread)   Pdfium, all documents/pages, registry, caches, scheduler. ONLY pdfium caller.
seepdf-encode-0..1            PNG/JPEG encode, tile-cache insert, UriSchemeResponder::respond, export file writes
seepdf-io                     file reads for open, atomic save writes, backups, recents thumbnails
tokio (tauri::async_runtime)  #[tauri::command] async fns: submit to the engine, await the oneshot
webview                       React shell + TileManager + tesseract.js Web Workers (≤ min(4, cores-2))
seepdf-ocr-native (P1, macOS) Vision requests (GPU/ANE); receives BGRA bytes, returns OcrPage
```

Encode is a separate pool because it is the only parallelisable part (export: 4.6 ms render + 4.4 ms
encode per page at 150 DPI, pages §4) and the engine thread must never run an encoder.

### 1.2 Engine command protocol

```rust
// src-tauri/src/engine/types.rs
pub enum Lane { Interactive = 0, Edit = 1, Prefetch = 2, Thumb = 3, Background = 4 }

pub struct Cmd {
    pub lane: Lane,
    pub priority: u32,          // within a lane: tile distance from the viewport centre, or |page - centre|
    pub seq: u64,               // submission order; FIFO tie-break, and the only order inside Lane::Edit
    pub viewport_gen: u64,      // snapshot of EngineShared::viewport_gen at submit time (renders only)
    pub label: &'static str,    // tracing
    pub run: Box<dyn FnOnce(&mut EngineState<'_>) + Send + 'static>,
}

pub struct EngineHandle { tx: crossbeam_channel::Sender<Cmd>, pub shared: Arc<EngineShared> }
pub struct EngineShared {
    seq: AtomicU64, viewport_gen: AtomicU64,
    pub viewport: parking_lot::Mutex<Viewport>,             // centre page, visible range, scaleKey
    pub docs: parking_lot::RwLock<HashMap<DocId, DocSummary>>,  // mirror for protocol 404/410 checks, no pdfium
    pub tiles: TileCache,                                   // encoded PNG LRU, read on the protocol thread
}
```

`EngineHandle::call(lane, label, f).await` (used by every `#[tauri::command]`) wraps `f` with a
`Reply::oneshot()` built on `tauri::async_runtime::channel(1)` — tauri does not re-export
`tokio::sync::oneshot` (tauri §3). `EngineHandle::send(...)` is fire-and-forget; the protocol handler
moves its `UriSchemeResponder` (which is `Send`) straight into the closure so the encode pool answers
the webview directly (tauri §1).

Loop (`engine/thread.rs`):

```
recv() one Cmd → drain try_recv() into a BinaryHeap → pop by (lane, priority, seq)
  drop renders whose viewport_gen + 1 < shared.viewport_gen and whose page is outside visible±1 → reply Stale
  drop cancelled jobs (JobToken.cancel: Arc<AtomicBool>)
  run one command (≤ ~10 ms of pdfium work; chunked jobs re-enqueue their next page)
starvation guard: after 8 consecutive Interactive/Prefetch pops, take one Edit/Background if queued
```

Long jobs (search, export, OCR apply, flatten) are submitted as **one command per page** on
`Lane::Background` with a shared `JobToken`, so visible tiles interleave and cancellation is observed
within one page (≤ 30 ms).

### 1.3 The pdfium-render patch (D9) — scope, fallback, and what it gates

```toml
# src-tauri/Cargo.toml
pdfium-render = { version = "0.9", default-features = false, features = ["pdfium_7881", "image_025"] }
[patch.crates-io]
pdfium-render = { git = "https://github.com/hupark-seegene/pdfium-render", rev = "<sha>" }  # branch seepdf/0.9.4-raw
```

The branch is 0.9.4 plus one file of additive accessors (no behaviour change):

```rust
impl PdfDocument<'_>       { pub fn raw_handle(&self) -> FPDF_DOCUMENT }
impl PdfPage<'_>           { pub fn raw_handle(&self) -> FPDF_PAGE }
impl PdfForm<'_>           { pub fn raw_handle(&self) -> FPDF_FORMHANDLE }
impl PdfPageAnnotation<'_> { pub fn raw_handle(&self) -> FPDF_ANNOTATION }
impl Pdfium               { pub fn raw_bindings(&self) -> &dyn PdfiumLibraryBindings }   // the SAME bindings instance
```

Raw FFI is confined to `src-tauri/src/engine/raw/` (the only module with `unsafe`), which exposes safe
wrappers: `set_ap_none`, `set_color`, `set_border`, `add_ink_stroke`, `set_flags`, `set_string`,
`create_annot(subtype)`, `move_pages`, `import_pages_by_index`, `flatten(page, FLAT_NORMALDISPLAY)`,
`save_as_copy(doc, writer, flags)`, and the `FORM_*` family. Constants come from `bindgen/pdfium_7881.rs`
and are re-checked by a unit test.

**If the patch is rejected**, the spikes' alternative is a second `Pdfium::bind_to_library()` (legal
before `Pdfium::new()`) driving a *separate* raw `FPDF_DOCUMENT` of the same bytes, with the
pdfium-render document reloaded from `save_to_bytes()` after every edit batch — 5 ms/MB per edit,
acceptable ≤ 20 MB, unusable for scans. What degrades without the patch *and* without the fallback:
form filling becomes read-only (P0-20), page reorder must be copy+delete (breaks `/AcroForm`),
ellipse falls back to rectangle, annotation border width and safe recolour are lost, and
"remove password" is impossible. This is why the patch is the primary plan.

The patch is **not** used for: `FPDF_INCREMENTAL` (not in v1), `FPDFText_LoadCidType2Font` (glyphless
OCR font is P2; v1 uses the bundled subset font), XFA (not built into this pdfium).

---

## 2. Document registry, ids, generations

```rust
pub type DocId = String;        // "d1", "d2", … process-unique, never reused
pub struct OpenDoc<'p> {
    // field order == drop order: pages MUST drop before the document (render §5, SPIKE_UAF)
    pages: PageLru<'p>,                 // ≤ 24 open PdfPage handles; FPDF_LoadPage costs 0.46–26 ms
    doc: PdfDocument<'p>,               // load_pdf_from_byte_vec: the document owns the Vec
    form: Option<FormEnv>,              // raw FPDF_FORMHANDLE + which pages had FORM_OnAfterLoadPage
    pub bytes: Arc<[u8]>,               // the bytes `doc` was loaded from (undo base, /raw route)
    pub path: Option<PathBuf>,          // None for untitled/merged documents
    pub generation: u32,                // +1 on every mutation; part of every cache key and tile URL
    pub saved_generation: u32,          // dirty  <=>  generation != saved_generation
    pub pages_meta: Vec<PageGeom>,      // size, rotation, crop, label — from page_sizes() (0.1 ms/14 pages)
    pub touched: BTreeSet<u16>,         // pages with annotation edits: need one render before save (/AP)
    pub history: History,               // snapshot stack (§7)
    pub text: TextCache,                // (page) -> Arc<TextLayer>, 32 MiB LRU
    pub annots: HashMap<u16, Vec<AnnotSummary>>,   // rebuilt lazily per page per generation
}
```

Rules enforced inside the registry, not by convention:

1. `registry::page(doc, i)` opens through the LRU, runs `FORM_OnAfterLoadPage` when the document has a
   form (pdfium-render never calls it) and sets `PdfPageContentRegenerationStrategy::Manual`
   (annotations §5.10: the default regenerates the whole content stream on every annotation create).
   Eviction runs `FORM_OnBeforeClosePage`.
2. **`registry::mutate(doc, label, f)` is the only `&mut` path into a document.** It: pushes an undo
   snapshot → flushes the page LRU when `f` is structural (raw `FPDF_MovePages` /
   `FPDF_ImportPagesByIndex` bypass pdfium-render's `PdfPageIndexCache`, pages §1) → runs `f` →
   refreshes `pages_meta` → `generation += 1` → invalidates the affected caches → emits `doc-changed`.
   One user action = one `mutate` = one generation = one undo step.
3. `registry::replace(doc, bytes)` (undo/redo/after save) drops pages, then the document, then loads
   the new bytes (0.15 ms, lazy).
4. `Pdfium` is bound exactly once per process, on the engine thread (`bind_to_library` →
   `PdfiumLibraryBindingsAlreadyInitialized` on a second call, tauri §5); it lives on the engine
   thread's stack, so documents borrow it with a local lifetime — no `Box::leak`, no `'static`.
5. Ids: `docId` string; `pageIndex` 0-based `u16` in IPC (`PdfPageIndex` is `c_int`, converted at the
   boundary); `annotId` is the annotation's `/NM` (uuid v4, assigned by us when missing) and is stable
   across generations; `objectId` is the page-object index and is valid **only** for the generation it
   was listed in (commands carry `expectGeneration` and fail with `stale`).

Open: `load_pdf_from_byte_vec(bytes, password)` for every file (the `io` thread reads it; 1 MB in
0.3–1.5 ms). Password errors are `PdfiumError::PdfiumLibraryInternalError(PasswordError)` for both
"missing" and "wrong" → `passwordRequired` / `passwordWrong` depending on whether one was supplied.
Only `page_sizes()` runs at open (0.1 ms per 14 pages ⇒ ~3.5 ms for 500 pages); outline, text,
annotations, form fields and thumbnails are all lazy.

---

## 3. Rendering pipeline

### 3.1 Levels of detail

| Kind | Geometry | Cost | When |
|---|---|---|---|
| `thumb` | `set_target_width(w).set_maximum_height(h)` — never `thumbnail(n)` (squashes to n×n and disables annotations, pages §4) | 2.5–2.8 ms at 200 px | sidebar rows + organizer cells in view ± 1 screen |
| `page` (also the placeholder) | whole page at `s`, used while `s ≤ 2.0` and `W×H ≤ 2.5 Mpx`; placeholder is the same route at `s_ph = min(1.0, 640 / w_pt)` | 0.95 ms @1×, 2.24 ms @2× | every page within ±1.5 screens |
| `tile` | 512×512 device px, origin `(tx·512, ty·512)`, edges clipped to `W,H` | 0.01–0.8 ms | `s > 2.0` |

Device scale `s = zoomPercent/100 × devicePixelRatio`, quantised to `scaleKey = round(zoomPercent × dpr)`
so cache keys are finite. Page pixel size is `round(fround(points) × s)` — exactly pdfium-render's own
arithmetic; the frontend uses `Math.fround` and the engine's `X-Image-Width/Height` headers win on a tie.
Hard ceiling `scaleKey ≤ 800`; above that the frontend CSS-upscales.

### 3.2 Render configuration (the only one; `engine/render/tiles.rs`)

```rust
PdfRenderConfig::new()
    .scale_page_by_factor(s)
    .rotate(view_rotation, true)          // intrinsic /Rotate is already applied by pdfium
    .render_form_data(true)               // FPDF_FFLDraw with the same origin — only on the set_origin path
    .render_annotations(true)             // FPDF_ANNOT; also makes pdfium generate and persist /AP
    .use_lcd_text_rendering(false)        // subpixel AA is wrong once composited by the webview
    .set_format(PdfBitmapFormat::BGRA)    // never BGR: as_rgba_bytes() costs 83.6 ms
    .set_reverse_byte_order(true)         // pdfium writes RGBA: as_raw_bytes() needs no swizzle (0.16 ms)
    .set_clear_color(PdfColor::WHITE)
    .set_origin(-tx, -ty)                 // tiles only; pdfium clips to the tile-sized bitmap
```

into `PdfBitmap::empty(w, h, BGRA)` + `render_into_bitmap_with_config`. Verified: `set_origin` tiles match
a crop of the full render within 427/1,048,576 px at max channel delta 1, tile-to-tile 0 px (render §3).
`highlight_all_form_fields` is added when the viewer's 필드 강조 toggle is on (part of the tile key).

### 3.3 Cache and priorities

* **Engine tile cache**: `parking_lot::Mutex<LruMap<TileKey, Arc<[u8]>>>`, **64 MiB** of encoded PNG,
  keyed `{doc, gen, page, kind, scaleKey, rot, tx, ty, night, hl}`; looked up **on the protocol thread**
  before anything is submitted to the engine. Hand-rolled `HashMap + VecDeque`, no `lru` crate.
  A generation bump sweeps the doc's dead entries (n ≤ ~500, O(n)).
* **Browser cache**: tile URLs contain `gen` and are served `Cache-Control: private, max-age=31536000,
  immutable`, so the webview never revalidates; the TileManager keeps at most 120 mounted `<img>`s so
  decoded bitmaps can be reclaimed.
* **Text cache**: `Arc<TextLayer>` per page, 32 MiB LRU; the plain page text is kept for the document's
  lifetime (≈ 5 kB/page) so search re-queries are pure memory.
* **Request order** (frontend issues, engine honours): visible tiles centre-out (`Lane::Interactive`,
  `priority = Chebyshev distance`) → placeholder for pages entering ±1.5 screens → prefetch one screen
  in the scroll direction (`Lane::Prefetch`) → thumbnails (`Lane::Thumb`). ≤ 24 in-flight requests,
  ≤ 8 during a fling (> 1.5 px/ms). `set_viewport` is called once per scroll/zoom settle and bumps
  `viewport_gen`.

### 3.4 Night mode

The tile layer gets `filter: invert(1) hue-rotate(180deg)` (dark) or a sepia matrix; the SVG overlay
(in-progress gestures, handles) is outside the filtered element. Annotations baked into the bitmap
invert with the page — hue-rotate keeps a yellow highlight yellowish. A transparent clear colour
(`PdfColor::new(0,0,0,0)`, verified to keep real alpha on 484,566/484,704 px of a text page) is used when
`night != off` so the page is composited over the UI's dark background instead of inverting white.

---

## 4. Transport

| Payload | Channel | Why |
|---|---|---|
| tiles, whole pages, placeholders, thumbnails, OCR page images, recents thumbnails, original bytes | `seepdf://` custom protocol, PNG (`Fast`+`Sub`) | free webview memory cache, decode off the JS thread, `<img>` semantics, no Blob bookkeeping (tauri §6.1) |
| text layer, raw page buffer (snapshot tool) | binary IPC — `tauri::ipc::Response::new(Vec<u8>)` → `ArrayBuffer` | 141 MB/s, zero JS decode step; JS must index into it |
| everything else | JSON commands | small |
| progress | `tauri::ipc::Channel<JobEvent>` per invocation | ordered, scoped to the caller, 42k–100k msg/s |
| app-wide news (`doc-changed`, `open-file`, `recents-changed`) | `app.emit` + `listen` | broadcast to every window |

The handler is registered with `register_asynchronous_uri_scheme_protocol("seepdf", …)`; it runs on the
main thread on macOS, so it only parses the URL, checks the tile cache and the `docs` mirror, and submits.
It **always** responds (a dropped `UriSchemeResponder` hangs the `<img>` until the webview times out):
404 unknown doc, 410 stale `gen`, 409 stale viewport, 400 bad params, 500 render error, 503 busy.
Windows serves the same routes from `http://seepdf.localhost/`; the frontend never sniffs the OS and
builds URLs with `convertFileSrc('', 'seepdf')`. Route grammar and headers: `IPC_CONTRACT.md` §9.

---

## 5. Text layer, selection, search

`engine/text/layer.rs` builds one `TextLayer` per page (5.6 ms/page including word/line grouping,
text §2) from `page.text()?.chars()`: per char the **loose** box (selection), the tight box (redaction
hit-tests), baseline origin, scaled font size, `is_generated`, and the owning text-object index (linear
`char.text_object()` pass, text §5 method B). Words break on whitespace/generated chars, a baseline jump
> 0.5 em, a horizontal gap > 0.25 em, or x going backwards; lines group words with the same baseline ±0.5 em.

The layer is serialised into one binary buffer (32 B/char ⇒ ~163 kB for a 5,087-char page; format in
`IPC_CONTRACT.md` §10.1) and delivered over binary IPC. The frontend wraps the typed arrays: `hitTest`
is a binary search on lines then a scan of ≤ ~100 chars (µs), `rangeRects` unions loose boxes per line
(40 µs for 111 chars). **No DOM node per character.** A hidden `<div role="document">` carries the page
text for screen readers.

Selection state is `{page, startChar, endChar}` per page (multi-page = one range per page); copy joins
code points, keeping pdfium's generated `\r\n` as line breaks.

Search runs in Rust over the cached per-page text (case-folded on `char`s, 65 µs/page; whole-word and
match-case implemented here), **not** through `FPDFText_FindStart` — pdfium's search returns rects with
no char index (text §3). Pages are visited outward from the current page; the first four go on
`Lane::Interactive`, the rest on `Background`, all sharing one `JobToken`; hits stream over a
`Channel<SearchEvent>` so the panel fills as they arrive. Cross-check: a `cargo test` asserts our hit
count equals pdfium's (62 / 1 / 4 for "monkey" on tracemonkey with default / match-case / whole-word).

---

## 6. Editing model

Everything below runs on `Lane::Edit` inside `registry::mutate` (snapshot → edit → generation bump).
Pages are always in `Manual` regeneration; `regenerate_content()` is called exactly once at the end of a
command **that touched page objects** (redaction, text/image objects, OCR layer) and never for annotation
or form work.

### 6.1 Annotations (raw `FPDFAnnot_*` pipeline — annotations §6)

Create, in this order (order matters): `FPDFPage_CreateAnnot(subtype)` → `FPDFAnnot_SetRect` (the AP BBox
is the Rect at creation; objects appended before it are clipped away) → `FPDFAnnot_SetColor(C, rgba)` and
`(IC, rgba)` **with identical alpha** (every SetColor rewrites `/CA`) → `FPDFAnnot_SetBorder(0,0,width)` →
geometry (quads in **TL, TR, BL, BR** order — `PdfQuadPoints::from_rect()` is BL,BR,TR,TL and renders a
1-px sliver; or `FPDFAnnot_AddInkStroke`; or `AppendObject` for Stamp) → `SetFlags(PRINT)` (pdfium-render
never sets `/F 4`, and flatten deletes non-printable annotations) → `SetStringValue` for `NM` (uuid v4),
`T` (author), `Contents`, `Subj`, `CreationDate`.

Edit: **`FPDFAnnot_SetAP(annot, NORMAL, NULL)` first**, then rect/quads/colour/border, then let the next
render regenerate the AP. Never `set_fill_color`/`set_stroke_color` from pdfium-render on an annotation
that has an AP (SIGSEGV, annotations §5.1) — every annotation read back from disk has one.

Before every save, each page in `touched` is rendered once at `set_target_width(32)` (≈ 0.3 ms) so pdfium
writes the `/AP` streams; without it Highlight/Underline/StrikeOut/Squiggly/Square/Circle/Text are
invisible in Preview, Acrobat and pdf.js (annotations §3.2). In practice the viewer's own tile render has
already done this.

| Tool (UI) | Written as | Notes |
|---|---|---|
| 형광펜 / 밑줄 / 취소선 / 물결선 | Highlight / Underline / StrikeOut / Squiggly | one quad per selected line run; `/Rect` = union |
| 메모 | Text | pdfium note-icon AP; the popup is React, never a Popup annot (unlinkable, never drawn) |
| 펜, 서명(그리기) | Ink with a real `/InkList` (`AddInkStroke` per stroke) + `SetBorder` width | eraser removes whole strokes in v1 |
| 사각형 | Square | |
| 타원 | Circle — raw `FPDFPage_CreateAnnot(FPDF_ANNOT_CIRCLE)` | no high-level constructor in 0.9.4 |
| 선 / 화살표 | **Ink**, one 2-point stroke (+ 2 head strokes), `/Subj "SeePDF:Line"` / `"SeePDF:Arrow"` | PDFium cannot create Line annots (`CreateAnnot` returns NULL). Other viewers see ink; SeePDF re-reads the tag. lopdf rewrite to `/Line` is P2. |
| 텍스트 상자 | **Stamp + text objects** with the bundled Hangul font, `/Contents` = the text, `/Subj "SeePDF:TextBox"` | FreeText needs `/DA` to persist and `/Helv` cannot render 한글; Stamp+text is verified for Korean |
| 도장 / 서명(이미지) | Stamp + image object (`new_with_size`, transform **before** `add_image_object`) | |
| 링크 (P2) | Link + `FPDFAnnot_SetURI` | go-to-page destinations cannot be created |

### 6.2 Text objects (편집 mode) — explicit editability boundary

`editability(object)` is computed and shown as a badge; the UI never silently substitutes:

| Object | move/scale/recolour/delete | change characters |
|---|---|---|
| base-14 font, or embedded font with ToUnicode that passes the trial check | yes | in place (`set_text`) |
| embedded subset that fails the trial check | yes | replace-object with the bundled font, after an explicit "글꼴이 대체됩니다" confirmation |
| no ToUnicode (raw char codes) / Type3 | yes | refused (`error.edit.noUnicode`) |
| inside a Form XObject | **refused** (edits return `Ok` but are not persisted) | refused |
| render mode Invisible (OCR layer) | delete only | refused |

The **trial coverage check** is the only reliable detector (`PdfFontGlyphs::len()` is 0 for subset fonts
and built-ins, `PdfFont::name()` strips the subset tag): apply `set_text(new)`, open a fresh `page.text()`
(it reflects in-memory edits), require `loose_bounds().width() > 0` for every non-generated char
*including spaces*, then keep or revert. Safe in-place edits: `translate`; anchored scale
(`translate(-x,-y); scale(k,k); translate(x,y)` — `scale()` alone scales about the page origin);
`set_unscaled_font_size`; `set_fill_color`; `remove_object_at_index` (descending — indices shift).
Replace-object copies matrix, fill colour and render mode onto a new `PdfPageTextObject`.

### 6.3 Page operations (`page_ops` batch, pages §1–2)

move = raw `FPDF_MovePages` (0.02 ms) after Rust-side validation (no duplicates, all `< len`,
`dest + n ≤ len`; a `false` return means "reload from the snapshot"); delete = `page.delete()` descending
(2.2 ms each); rotate = `set_rotation` under `Manual`; insert blank = `create_page_at_index`;
duplicate = raw `FPDF_ImportPagesByIndex(doc, doc, [i], 1, i+1)` (verified src == dest); insert from file =
`copy_pages_from_document(&src, "1-3,5", at)` (**1-based string**, used only for user-typed ranges;
internal ranges use the **0-based** `copy_page_range_from_document` — never mix).
**Extract and split copy the original and delete the other pages**, never "new document + import":
`FPDF_ImportPages*` keeps the widgets but drops `/AcroForm`, the outline and metadata (pages §2).
Merge of unrelated files is the one place import is used, and the UI warns when a source has a form or
an outline.

### 6.4 Forms (annotations §3.4)

`PdfFormTextField::set_value` writes `/V` only and the value is invisible in every viewer, so filling goes
through the form-fill environment on the raw handles:

```
text:           FORM_SetFocusedAnnot → FORM_SelectAllText → FORM_ReplaceSelection(utf16)  [unverified]
                fallback (verified):  FORM_OnLButtonDown/Up at the widget centre → FORM_OnChar per UTF-16 unit
always:         FORM_ForceToKillFocus      // commits /V AND regenerates /AP (896 B stream measured)
checkbox/radio: FORM_OnLButtonDown/Up at the widget centre, verify with FPDFAnnot_IsChecked, click again if needed
combo/list:     FORM_SetFocusedAnnot → FORM_SetIndexSelected(i, on) per index → ForceToKillFocus
```

Korean input is typed into an **HTML `<input>` overlay** positioned by the page matrix (real IME
composition; never keystroke forwarding), committed on blur/Enter. Widgets are drawn by `FPDF_FFLDraw`
inside the tile, so the committed value is what the user sees. Push-button JS/URI actions stay inert
(pdfium-render leaves the FFI callbacks `None`); signature fields and XFA documents are read-only with a
banner.

### 6.5 Redaction (annotations §3.5)

`redact_preview` reports exactly what will be removed before the destructive step. Apply, per region:
text objects whose **tight** char boxes intersect are removed (`remove_object_at_index`, descending) —
PDFium cannot split a text object, so the whole run goes and the preview says so ("Gal" removes
"Andreas Gal"); intersecting images are re-written with the region blanked (`get_raw_image` → fill →
`set_image`, matrix preserved); intersecting annotations are deleted; a black
`create_path_object_rect` is added; `regenerate_content()`; then the page text is **re-extracted and
verified** — if any marked string survives, the whole command fails and the snapshot is restored.
Word-level re-creation of surviving glyphs and the raster fallback for XObject text are P2.

### 6.6 OCR text layer (ocr §5)

Per line `font_size_pt = rowHeightPx × 72/dpi`; per word: `PdfPageTextObject::new(&doc, word, font, size)`
→ `set_render_mode(Invisible)` → `natural_w = bounds()` (works before adding) →
`scale(clamp(box_w_pt / natural_w, 0.5, 2.0), 1.0)` **before** `translate(x_pt, baseline_pt)` →
`add_text_object`; pixels map to points with `page.pixels_to_points(x, y, &same_render_config)` so
`/Rotate` is honoured, and the object is rotated back by the page rotation. Spaces are rebuilt from
`line.text` (Tesseract over-segments Hangul into syllables); words with confidence < 30 are skipped;
`Manual` + one `regenerate_content()` per page (2 ms for 730 words vs 81 ms automatic). Latin-1-only
words use `helvetica()` (no embedding); everything else uses the bundled Hangul font
(`load_true_type_from_bytes(bytes, true)`, once per document).

---

## 7. Undo / redo (snapshots)

`History { undo: Vec<Snapshot>, redo: Vec<Snapshot>, ram_bytes, spill_dir }`,
`Snapshot = Ram(Arc<[u8]>) | Disk(PathBuf)`. `registry::mutate` pushes the **pre-edit** bytes
(`save_to_bytes()`, 5 ms/MB; the first push after open/save/undo reuses `OpenDoc::bytes` and is free).
Undo = `registry::replace(doc, pop())` + generation bump; the current state moves to the redo stack.
Budget: 256 MiB RAM then spill to `$TEMP/seepdf-history/<docId>/`, depth 50; for documents > 48 MB the
depth drops to 3 and snapshots go straight to disk. Slider/drag edits coalesce within 500 ms (one
snapshot per gesture). OCR of a page range, a `page_ops` batch and a redaction apply are each **one**
undo step. Not undoable and therefore confirmed in a dialog: nothing in v1 — even redaction and OCR are
undoable until save, which is what the UI promises (`pages.deleteWarning`).

Known limitation: a 100 MB scan costs ~0.5 s of engine time per edit for the snapshot. Documented; the
follow-up (delta snapshots via `FPDF_INCREMENTAL`, 0.2 ms) sits behind the same `History::push` seam.

---

## 8. Save, Save As, dirty tracking

```
save(doc, path):
  1. render every page in `touched` at set_target_width(32)          // /AP generation, ~0.3 ms/page
  2. FORM_ForceToKillFocus; drop the page LRU
  3. bytes = FPDF_SaveAsCopy(doc, FileWriter, flags = 0)             // full rewrite, 5 ms/MB
  4. io thread: write `<dir>/.<name>.seepdf-<pid>.tmp`, fsync
  5. verify: reopen the temp with pdfium; page count + every page size must match; else abort
  6. backup (default on): copy the original to $APPDATA/SeePDF/backups/<hash>/<ts>-<name>.pdf, keep 3
  7. fs::rename(tmp, path)                                            // atomic on APFS/NTFS, same directory
  8. registry::replace(doc, bytes); saved_generation = generation; emit doc-saved
```

Failure at any step leaves the original untouched and deletes the temp. A read-only file or volume
returns `readOnly` and the UI falls through to Save As with an explanation. Windows retries the rename
5× over 500 ms on sharing violations. Documents opened with a password stay encrypted (flags 0 keeps
encryption, verified); "remove password" (P1) is `FPDF_REMOVE_SECURITY = 4` (**not** 3, which is the
deprecated value). Dirty = `generation != saved_generation`; the title shows `• name` and
`⌘W`/quit show the 저장/저장 안 함/취소 sheet.

---

## 9. OCR pipeline

```
OcrDialog → ocrJob.ts (JS orchestrator; ≤ min(4, cores-2) workers, one page in flight per worker)
  per page: skip if ocr_page_status says hasText (unless 강제) 
            GET seepdf://…/ocr?doc&gen&page&dpi=300           (gray8 PNG, 0.2–0.9 MB, 12–21 ms render)
            createImageBitmap → scheduler.addJob('recognize', img, {}, { text: true, blocks: true })
            normalize.ts → OcrPage (engine-agnostic)
  then:     ocr_apply(docId, pages[]) → one undo step, per-page commands on Lane::Background
```

tesseract.js 7 is configured entirely offline: `createWorker(['kor','eng'], OEM.LSTM_ONLY, {
workerPath: '/ocr/worker.min.js', corePath: '/ocr/', langPath: '/ocr/', gzip: true, cacheMethod: 'none',
workerBlobURL: false })`, `setParameters({ tessedit_pageseg_mode, user_defined_dpi: '300',
preserve_interword_spaces: '1' })`. Always `kor+eng` (`kor` alone reads English as digits). Layout option
자동/단일 단/단일 블록 = PSM 3/4/6, default 자동 — PSM 4/6 reduced Hangul CER from 23.8 % to 1.2 %/0.0 % on
the sparse Korean page, but PSM 6 interleaves columns on two-column documents. Bundled assets
(`public/ocr/`, 8.4 MB): `worker.min.js`, `tesseract-core-simd-lstm.wasm.js`, `tesseract-core-lstm.wasm.js`,
`eng.traineddata.gz`, `kor.traineddata.gz` — the `4.0.0_best_int` set (the legacy `4.0.0` files produce
90 % CER with the LSTM-only core). Cancel = `job_cancel` (the Rust `AtomicBool` is checked between pages)
plus `scheduler.terminate()` (tesseract.js has no per-job cancel; re-creating workers costs 70–170 ms).
Memory ≈ 95–120 MB RSS per worker.

macOS Vision (P1) implements `ocr_recognize_native` and returns the same `OcrPage`; nothing else changes.

---

## 10. Frontend architecture

```
src/
  main.tsx App.tsx            mount, theme + locale bootstrap, show window after first paint
  ipc/       types.ts api.ts events.ts protocol.ts mock.ts     the only files that import `invoke`
  store/     appStore viewStore docStore annotStore pagesStore jobStore   (zustand, one file per owner)
  i18n/      index.ts ko.json en.json
  styles/    tokens.css base.css
  keys/      keymap.ts useKeymap.ts
  app/       TitleBar ModeSwitcher ToolStrip StatusBar SidebarFrame Inspector/ ContextMenu Toasts
  sidebar/   Thumbnails Outline SearchPanel AnnotationList
  viewer/    Viewer Scroller PageShell TileManager.ts layout.ts geometry.ts zoom.ts text/ search/
  tools/     ToolController.ts tools/*.ts        annot/  SVG overlay, handles, popovers
  forms/     HTML input overlay over widgets
  organize/  PagesGrid PageTile dnd.ts
  dialogs/   Export Merge Split Password Unsaved Settings Print OcrDialog(host)
  welcome/   Welcome RecentCard
  ocr/       ocrJob.ts tesseractPool.ts normalize.ts
public/ocr/  public/fonts/
```

* **State**: zustand slices. What is *not* in the store: scroll position, in-gesture zoom scale, tile
  inventory, ink points, pointer state — all refs/classes, so React never re-renders per frame.
* **Virtual scroller**: layout comes from `DocInfo.pages[]` alone (no pixels needed), one absolutely
  positioned `PageShell` per page in `[scrollTop − 1.5vh, scrollTop + 2.5vh]`. Zoom during a gesture is a
  CSS `transform: scale()` on the tile container (≤ 1 ms); on settle (120 ms) the layout is recomputed,
  the anchor point under the cursor is preserved, and new tiles replace the scaled ones on `img.onload`.
* **Layers per page**, bottom to top: `<img class="ph">` placeholder → tile `<img>`s → selection/search
  rect divs → SVG annotation overlay (`viewBox` in PDF user space with a y-flip `matrix(1 0 0 -1 0 h)`,
  so shapes never recompute on zoom) → HTML form inputs (양식 mode) → tool pointer-capture surface.
* **Tool modes**: `ToolController` is a state machine `{mode, tool, sticky}`; each tool is a pure
  `onDown/onMove/onUp(state, pt) → {state, preview?, commit?}` module, unit-testable without React.
  Momentary (hold) vs latched (tap) keys per `UI_SPEC.md` §6.
* **Optimism**: a created annotation is drawn in SVG immediately (≤ 16 ms), the command runs, the new
  generation invalidates the page's tiles, and the ghost is removed on the new tile's `onload` — no flash.
* **i18n / theme**: `t(key, params)` over two flat JSON catalogues, `data-theme` and `data-os` on `<html>`,
  every colour a token. `scripts/check-i18n.mjs` fails CI on key drift.

---

## 11. Security, CSP, capabilities

```json
"csp": {
  "default-src": "'self' ipc: http://ipc.localhost",
  "script-src":  "'self' 'wasm-unsafe-eval'",
  "worker-src":  "'self' blob:",
  "connect-src": "'self' ipc: http://ipc.localhost seepdf: http://seepdf.localhost https://seepdf.localhost",
  "img-src":     "'self' data: blob: seepdf: http://seepdf.localhost https://seepdf.localhost",
  "style-src":   "'self' 'unsafe-inline'",
  "font-src":    "'self' data:"
}
```

`script-src`/`worker-src` are required by the tesseract.js worker (it `importScripts` the WASM core) and
are **unverified inside a Tauri window** — the OCR module's first task is a `tauri build --debug` probe
(CSP is not applied in `tauri dev`, where the HTML comes from Vite). `img-src`/`connect-src` need both
`seepdf:` (macOS) and `http://seepdf.localhost` (Windows); verified in a bundle (tauri §6).

Capabilities (`capabilities/default.json`, windows `["main", "doc-*"]`): `core:default`,
`core:window:allow-start-dragging`, `core:window:allow-set-title`, `core:window:allow-create`,
`core:webview:allow-print`, `dialog:default`, `store:default`, `window-state:default`, `opener:default`.
The `fs:*` permissions from the spike are **removed**: the engine reads and writes every file, and the
dialog plugin extends its own scope for picked paths. No network permission is granted anywhere; OCR,
fonts and pdfium are all local (offline is a hard product requirement).

---

## 12. Packaging

* `bundle.resources` per platform overlay so a build ships one pdfium, not four (~30 MB saved):
  `tauri.macos.conf.json` → `["resources/pdfium/*apple-darwin.dylib", "resources/pdfium/LICENSE.pdfium",
  "resources/pdfium/VERSION", "resources/fonts/*"]`; `tauri.windows.conf.json` → the `.dll`.
  The glob must always match something or `build.rs` fails (`GlobPathNotFound`) — `LICENSE.pdfium` and
  `VERSION` stay committed.
* Runtime resolution (`app/pdfium_path.rs`, kept from the spike): `<resource_dir>/resources/pdfium` →
  `<resource_dir>/pdfium` → (debug only) `CARGO_MANIFEST_DIR/resources/pdfium`, file chosen by
  `(OS, ARCH)`. Works in `tauri dev` (tauri-build copies resources next to the dev binary) and in the
  bundle (`SeePDF.app/Contents/Resources`, or the exe directory on Windows).
* `bundle.fileAssociations` for `.pdf` generates `CFBundleDocumentTypes`, which is what makes
  `RunEvent::Opened { urls }` fire for Finder double-click, `open -a`, and Dock drops. Paths arriving
  before the webview exists are queued (`PendingOpens`) and drained by `take_pending_opens`.
* macOS ships two DMGs (arm64, x86_64); a universal build would double libpdfium.
* Installed size budget ≤ 60 MB: tauri shell ~8–12 + libpdfium 7 + OCR 8.4 + Hangul font ~1.5 +
  Pretendard subset 0.35 + frontend ~2 ≈ **28 MB**. `scripts/check-bundle-size.mjs` gates it.

---

## 13. Performance budgets

| Scenario | Target | Budget from measurements |
|---|---|---|
| Cold launch → welcome visible | ≤ 700 ms (win ≤ 1.0 s) | tauri shell ~350 ms + initial JS ≤ 120 kB gz; the engine binds pdfium (2.1 ms) off the critical path |
| Open 14-page PDF → first page painted | ≤ 250 ms | read ≤ 2 ms + load 0.15 ms + `page_sizes` 0.1 ms + placeholder 0.95 ms + PNG 2.3 ms + transport |
| Open 500-page PDF → first page painted | ≤ 400 ms | `page_sizes` ≈ 3.5 ms; nothing else is eager |
| Thumbnail rail, 500 pages | placeholders ≤ 100 ms, visible thumbs ≤ 500 ms | 2.5–2.8 ms/thumb, only visible rows ± 1 screen (~30 thumbs ≈ 90 ms) |
| Scroll at 100 %, continuous | 60 fps, sharp ≤ 300 ms after stop | a new tile row ≈ 3 × (0.3 ms render + 0.2 ms encode + ~1.5 ms transport); 0 JS per frame |
| Zoom step | ≤ 16 ms visual, sharp ≤ 250 ms | CSS transform now; 12–20 tiles × ~1 ms ≈ 20 ms |
| Page render A4 @150 DPI | p50 ≤ 30 ms, p95 ≤ 120 ms | 4.6 ms/page measured for text pages |
| Search, 500 pages | first hit ≤ 150 ms, full pass ≤ 2.5 s, streamed | 1.4 ms/page text + 65 µs/page match |
| Annotation create → visible | ≤ 16 ms | optimistic SVG; engine create 0.05 ms, tiles follow in ~10 ms |
| Undo | ≤ 50 ms (≤ 5 MB docs) | replace = 0.15 ms load + 2 visible page renders |
| Save, 10 MB document | ≤ 300 ms | 5 ms/MB + fsync + verify reopen |
| Export 14 pages @150 DPI PNG | ≤ 400 ms | 65 ms render + 62 ms encode measured |
| OCR, 1 page A4 @300 DPI ko+en | ≤ 2 s/page, cancellable | tesseract PSM 4 1.5 s + render 12–21 ms; 4 workers ⇒ ~1.5 s/page wall for a batch |
| Idle RSS, 500-page document | ≤ 400 MB | engine ≈ 15 (lib) + file + ≤ 60 (24 pages) + 64 (tiles) + 32 (text) + transient ≈ 200 MB; webview bounded by ≤ 120 mounted tiles |
| Installed bundle | ≤ 60 MB | ≈ 28 MB (§12) |

macOS never returns freed bitmap memory, so RSS is a high-water mark: the largest single allocation is
bounded to 1 MiB (tile) or 10 MiB (whole page at the 2.5 Mpx cap), and export streams one page at a time.

---

## 14. Testing strategy

**Rust** (`cd src-tauri && cargo test --release`; a `OnceLock` test harness binds the host libpdfium from
`resources/pdfium/` once and owns one engine thread, because `bind_to_library` works once per process):

| Area | Asserts |
|---|---|
| `engine_` | lane ordering, Edit FIFO, stale drop by viewport generation, cancel between chunks, starvation guard; page-LRU drop order (close with 20 open pages); password error mapping |
| `render_` | 512² tile at 8× equals the crop of the full render (≤ 427/1,048,576 px, max channel delta 1); `rotation.pdf` p1 = 792×612 px @1×; thumbnails keep aspect ratio; form widgets visible in a 160F tile |
| `text_` | tracemonkey p1 = 5,087 chars / 89 lines / 754 words; the page→device matrix matches `FPDF_PageToDevice` within 1 px on rotation 0 and 90; search "monkey" = 62 hits with char indices; binary layer round-trips |
| `annot_` | every P0 type: create → render → save → reopen → present with a non-empty `/AP`, right bounds, ≥ N changed pixels; quads are TL,TR,BL,BR; recolour after reopen does **not** crash (child process, exit status checked); delete persists; Korean text box renders ≥ 100 dark px |
| `form_` | 160F-2019: 64 text / 10 button / 2 radio listed; "Raw FORM 입력" reads back, ≥ 100 px change under FFLDraw, persists after save+reopen; radio toggles |
| `pages_` | move `[3,2]→1` yields `A D C B`; delete descending; duplicate 14→15; extract-by-delete keeps `form() == Some` on 160F; `"1-3,5"` copies pages 1,2,3,5; MovePages validation rejects the three documented failures |
| `save_` | temp+rename atomicity (abort a child process after the temp is written → original intact); verify rejects a truncated file; encrypted input stays encrypted |
| `history_` | 20 random edits → 20 undos → bytes equal snapshot 0; redo restores; spill to disk at a 4 MB test budget |
| `ocr_` | a synthetic `OcrPage` ("검색가능한 Searchable") applies, reopens, `text().all()` contains both words **with spaces** (guards the AppleGothic TAB bug for the bundled font), `search` hits, 0 dark px in the boxes; `rotation.pdf` `points_to_pixels → pixels_to_points` round trip |
| `redact_` | the marked word is gone from the re-extracted text of the **saved** file; the command fails (and restores) when verification fails |

**TypeScript** (`npx vitest run`): `geometry` (4 rotations + inverse round trip against a Rust-generated
JSON table), `layout` (visible range, two-page pairing, fit modes), `TileManager` (order, in-flight cap,
stale removal), `TextLayer` (hit test, range rects on a recorded fixture), `tools/*` (each state machine's
emitted spec), `keymap` (per-platform chords, `when` guards, sticky timing), `i18n` (key parity, plurals,
placeholders), `ocr/normalize` (Tesseract and Vision samples → identical `OcrPage`), `api` against
`mockInvoke`.

**Smoke / e2e**: `seepdf --smoke <file.pdf>` (handled in `lib.rs` before any window is created) binds the
*bundled* pdfium, opens the file, renders page 0 at 2×, builds the text layer, exits 0/1 — run against the
built bundle on every CI target. `scripts/perf-baseline.mjs` drives the release binary over
`fixtures/` plus a generated 500-page document and writes `docs/perf/baseline.md`. `docs/qa/checklist.md`
holds what needs a human: saved annotations rendered by macOS Preview and Acrobat, Finder double-click,
pinch zoom, Korean strings with Windows fonts.

**CI matrix** (`.github/workflows/ci.yml`): `macos-14/aarch64-apple-darwin`,
`macos-13/x86_64-apple-darwin`, `windows-2022/x86_64-pc-windows-msvc`.
Steps: `npm ci` (postinstall fetches the host libpdfium and prepares `public/ocr/`) →
`node scripts/check-i18n.mjs` → `npm run typecheck` → `npx vitest run` →
`cargo fmt --check && cargo clippy -- -D warnings && cargo test --release` →
`npm run tauri build --target <t>` → `node scripts/check-bundle-size.mjs` → `--smoke` on the bundle →
(macos-14 only) perf harness compared to the baseline with a ±20 % tolerance.
`[profile.dev.package."*"] opt-level = 3` is mandatory so `tauri dev` numbers are meaningful
(PNG encode is 141–260 ms unoptimised versus 2.3 ms).

---

## 15. Open questions

1. **Approve the pdfium-render visibility patch** (§1.3). It gates P0 form filling and page reorder; the
   documented fallback is materially worse.
2. Windows window chrome: native caption (assumed for v1) versus custom-drawn buttons.
3. Bundled Hangul font choice for embedding into user PDFs (Noto Sans KR vs Pretendard, both OFL) — the
   subset must carry its licence file.
