# Spike: rendering + engine lifetime with pdfium-render 0.9.4

Example: `src-tauri/examples/spike_render.rs` — `cd src-tauri && cargo run --example spike_render`
(debug build runs in ~4 s; all numbers below are from `--release`, see "How the numbers were taken").

Environment: macOS 26 (Darwin 25.6.0), Apple Silicon, Rust 1.96.0, pdfium-render **0.9.4**
(features: default = `pdfium_latest` (=7881 bindings) + `image_latest` + `thread_safe`),
libpdfium **chromium/8057** from `src-tauri/resources/pdfium/libpdfium-aarch64-apple-darwin.dylib`.
Fixtures: `fixtures/tracemonkey.pdf` (14 text pages, 612x792 pt), `fixtures/alphatrans.pdf`
(1 page with transparency, 595x842 pt), plus `fixtures/rotation.pdf` for page rotation/labels.

## TL;DR / decision

* **Ownership model: (a) one dedicated engine thread.** Every pdfium call (open, render, text,
  save) happens on a single `std::thread` named `pdf-engine` that owns `Pdfium`, all
  `PdfDocument`s and an LRU of open `PdfPage`s. Tauri commands talk to it over a
  `crossbeam_channel` (round trip 0.3 us). Only plain `Vec<u8>` / POD results cross the channel.
  Reasons (all verified below): the crate's `thread_safe` feature only locks **289 of 481** FFI
  entry points (`FPDF_RenderPageBitmapWithMatrix`, `FPDFBitmap_CreateEx`, `FPDFText_GetText`,
  `FPDF_GetPageLabel`, `FORM_On*` … are unlocked), so a `Mutex`/`Sync`-based design is not
  actually sound; pdfium is serialised anyway (2 threads = 0 % speedup); and page/document drop
  order is not enforced by the type system, which is trivial to get right on one thread and easy
  to get wrong with `'static` handles spread across Tauri state.
* Rendering is fast: a letter page renders in **0.95 ms @1x, 2.2 ms @2x, 6.5 ms @4x**; a
  512 px tile at 8x costs **0.01–0.8 ms** depending on content; tiling a whole page costs ≤ 25 %
  more than one full render. Page load (`FPDF_LoadPage`) is 0.5–26 ms and should be cached.
* Tiles: use `PdfRenderConfig::set_origin(-tx, -ty)` with a tile-sized `PdfBitmap` (keeps forms
  + annotations). The matrix/`clip()` path gives byte-identical output but silently disables
  form-field drawing.
* Transport: ship **raw RGBA** (`set_reverse_byte_order(true)` + `as_raw_bytes()`, zero
  encode) through Tauri's binary IPC, or PNG `Compression::Fast` + `Filter::Sub`
  (2.3 ms / 1.4 MB for a full 2x page, ~0.2 ms per 512 px tile) through a custom URI scheme.
  JPEG q85 is 12.7 ms / 411 KB — only worth it for scanned/image pages.
* Memory: libpdfium + 14 open pages + 1 MB document ≈ 40 MiB RSS; RSS is dominated by whatever
  RGBA you keep (2x page = 7.4 MiB, 8x page = 118 MiB). macOS never returns freed bitmap memory,
  so peak == steady state: avoid full-page renders above ~4x, tile instead.

## Verified signatures (pdfium-render 0.9.4, read from `~/.cargo/registry/src/*/pdfium-render-0.9.4`)

```rust
use pdfium_render::prelude::*;   // exports everything used below

// binding (once per process)
Pdfium::bind_to_library(path: impl AsRef<Path>) -> Result<Box<dyn PdfiumLibraryBindings>, PdfiumError>
Pdfium::new(bindings: Box<dyn PdfiumLibraryBindings>) -> Pdfium        // stores bindings in a global OnceCell, calls FPDF_InitLibrary
Pdfium::default() -> Pdfium                                             // AFTER a bind: returns a 2nd handle (short-circuits on AlreadyInitialized)
impl Drop for Pdfium                                                    // does NOT call FPDF_DestroyLibrary -> leaking is free

// documents
Pdfium::load_pdf_from_byte_vec(&self, bytes: Vec<u8>, password: Option<&str>) -> Result<PdfDocument<'_>, PdfiumError>  // doc owns the Vec
Pdfium::load_pdf_from_byte_slice<'a>(&'a self, bytes: &'a [u8], password: Option<&str>) -> Result<PdfDocument<'a>, PdfiumError>
Pdfium::load_pdf_from_file<'a>(&'a self, path: &(impl AsRef<Path> + ?Sized), password: Option<&str>) -> Result<PdfDocument<'a>, PdfiumError>
PdfDocument::pages(&self) -> &PdfPages<'a>            // note 'a = the Pdfium lifetime, not &self
PdfDocument::bookmarks(&self) -> &PdfBookmarks<'_>
PdfDocument::form(&self) -> Option<&PdfForm<'_>>       // Some => FPDFDOC_InitFormFillEnvironment already done at load
PdfDocument::metadata(&self) -> &PdfMetadata<'_>;  PdfMetadata::get(&self, PdfDocumentMetadataTagType) -> Option<PdfDocumentMetadataTag>; .value() -> &str
PdfDocument::version(&self) -> Option<PdfDocumentVersion>
impl Drop for PdfDocument      // drops form, fonts, then FPDF_CloseDocument

// pages
pub type PdfPageIndex = c_int;   // i32, NOT u16
PdfPages::len(&self) -> PdfPageIndex
PdfPages::page_sizes(&self) -> Result<Vec<PdfRect>, PdfiumError>          // FPDF_GetPageSizeByIndexF, no page load: 0.1 ms for 14 pages
PdfPages::page_size(&self, index: PdfPageIndex) -> Result<PdfRect, PdfiumError>
PdfPages::get(&self, index: PdfPageIndex) -> Result<PdfPage<'a>, PdfiumError>   // FPDF_LoadPage EVERY call; no cache; 'a = Pdfium lifetime
PdfPage::width(&self) -> PdfPoints; height(); page_size() -> PdfRect        // PdfPoints { pub value: f32 }
PdfPage::rotation(&self) -> Result<PdfPageRenderRotation, PdfiumError>      // None|Degrees90|Degrees180|Degrees270, .as_degrees() -> f32
PdfPage::label(&self) -> Option<&str>                                       // read via FPDF_GetPageLabel at get() time
PdfPage::boundaries(&self) -> &PdfPageBoundaries; .media() -> Result<PdfPageBoundaryBox>  (.bounds: PdfRect)
PdfPage::text(&self) -> Result<PdfPageText<'_>, PdfiumError>;  PdfPageText::all(&self) -> String
impl Drop for PdfPage          // (regenerate content if dirty) then FPDF_ClosePage

// bookmarks (depth-first prefix order)
PdfBookmarks::iter(&self) -> PdfBookmarksIterator<'_>;  PdfBookmark::title(&self) -> Option<String>
PdfBookmark::destination(&self) -> Option<PdfDestination<'a>>;  PdfDestination::page_index(&self) -> Result<PdfPageIndex, PdfiumError>

// rendering
PdfRenderConfig::new() -> Self         // defaults: BGRA, clear white, render_form_data(true), render_annotations(true), matrix identity
  .scale_page_by_factor(f32) / .set_target_width(Pixels) / .set_target_height / .set_target_size(w,h) / .thumbnail(Pixels)
  .set_maximum_width / .set_maximum_height / .set_fixed_size(w,h)
  .rotate(PdfPageRenderRotation, do_rotate_constraints: bool)
  .render_form_data(bool) .render_annotations(bool) .use_lcd_text_rendering(bool) .disable_native_text_rendering(bool)
  .use_grayscale_rendering(bool) .use_print_quality(bool) .set_text_smoothing/.set_image_smoothing/.set_path_smoothing(bool)
  .set_reverse_byte_order(bool) .set_format(PdfBitmapFormat) .set_clear_color(PdfColor) .clear_before_rendering(bool)
  .set_origin(left: Pixels, top: Pixels)                       // start_x/start_y for FPDF_RenderPageBitmap (form-data path only)
  .translate(PdfPoints, PdfPoints) -> Result<Self>  .scale(f32,f32) -> Result<Self>  .apply_matrix(PdfMatrix) -> Result<Self>   // switch to matrix path
  .clip(left, top, right, bottom: Pixels)                      // FS_RECTF for FPDF_RenderPageBitmapWithMatrix; disables form data
pub type Pixels = i32;
PdfPage::render_with_config(&self, &PdfRenderConfig) -> Result<PdfBitmap<'_>, PdfiumError>
PdfPage::render_into_bitmap_with_config(&self, &mut PdfBitmap, &PdfRenderConfig) -> Result<(), PdfiumError>   // no size check; pdfium clips
PdfBitmap::empty(width: Pixels, height: Pixels, PdfBitmapFormat) -> Result<PdfBitmap<'a>, PdfiumError>      // FPDFBitmap_CreateEx
PdfBitmap::width()/height() -> Pixels;  format() -> Result<PdfBitmapFormat>   // Gray|BGR|BGRx|BGRA (default BGRA)
PdfBitmap::as_raw_bytes(&self) -> Vec<u8>     // copy of stride*height (BGRA: stride == width*4, tightly packed)
PdfBitmap::as_rgba_bytes(&self) -> Vec<u8>    // copy + BGRA->RGBA swizzle (no swizzle if set_reverse_byte_order(true) was used)
PdfColor::new(r,g,b,a: u8); PdfColor::WHITE
// Send/Sync (only with feature thread_safe): unsafe impl Send+Sync for Pdfium, PdfDocument, PdfPages, PdfPage, PdfBitmap, PdfPageText, PdfBookmarks
```

Bitmap format constants: `FPDFBitmap_Gray=1, BGR=2, BGRx=3, BGRA=4`. Render flags composed by the config:
`FPDF_ANNOT`, `FPDF_LCD_TEXT`, `FPDF_NO_NATIVETEXT`, `FPDF_GRAYSCALE`, `FPDF_RENDER_LIMITEDIMAGECACHE`,
`FPDF_RENDER_FORCEHALFTONE`, `FPDF_PRINTING`, `FPDF_RENDER_NO_SMOOTH{TEXT,IMAGE,PATH}`, `FPDF_REVERSE_BYTE_ORDER`.

## 1. Bind, load, page info

| step | measured (release) |
|---|---|
| `Pdfium::bind_to_library` + `Pdfium::new` | 2.1 ms; RSS 6.2 -> 15.2 MiB |
| `load_pdf_from_byte_vec` tracemonkey (1,016,315 B) | 0.15 ms (lazy; pdfium parses on demand) |
| `pages().len()` | ~0 ms; `pages().page_sizes()` for 14 pages: 0.098 ms |
| `pages().get(i)` (FPDF_LoadPage + content parse) | p0 3.68 ms, p1 2.24 ms, p2 0.46 ms, avg 1.30 ms; alphatrans p0 **25.9 ms** (TCPDF fonts) |
| `page.text().all()` p0 (5,067 chars) | 0.66 ms |
| rotation.pdf | p0 612x792 rot 0 label "1"; p1 **792x612** (width/height already swapped) rot 90 label "2" |
| metadata | alphatrans: Title/Author/Producer present; tracemonkey: Producer only; `form()` = None for all three |
| bookmarks | none in these fixtures; `iter()` works (0 items, 0.002 ms) |

Notes: `page_sizes()` returns the *rotated* (display) size, same as `PdfPage::width()/height()`;
`boundaries().media()` returns the unrotated MediaBox (612x792 on the rotated page). The
first `get()` of a document is the most expensive (font loading); keep pages open.

## 2. Full-page render times (release; `screen_config` = scale_page_by_factor + forms + annots + white clear)

tracemonkey.pdf page 0 (612x792 pt):

| config | pixels | render ms | `as_raw_bytes` ms | `as_rgba_bytes` ms |
|---|---|---|---|---|
| 1x | 612x792 | **0.95** | 0.12 | 0.02 |
| 2x | 1224x1584 | **2.24** | 0.16 | 0.18 |
| 4x | 2448x3168 | **6.54** | 0.67 | 0.73 |
| 8x (tile section reference) | 4896x6336 | 38.9 | – | – |
| 2x + LCD text | 1224x1584 | 2.32 | 0.19 | 0.48 |
| 2x no annots / no forms | 1224x1584 | 2.22 | 0.17 | 0.16 |
| 2x `set_reverse_byte_order(true)` | 1224x1584 | 2.20 | 0.16 | 0.14 (no swizzle) |
| 2x BGR 24bpp | 1224x1584 | 2.22 | 0.10 | **83.6** (aligned_bgr_to_rgba is slow) |
| 2x print quality | 1224x1584 | 2.19 | 0.16 | 0.24 |
| 1x rotate 90 | 792x612 | 2.34 | 0.06 | 0.03 |
| `thumbnail(200)` | 200x200 | 1.54 | 0 | 0 |
| `set_target_width(1000)` | 1000x1294 | 3.09 | 0.11 | 0.09 |
| 2x into reused `PdfBitmap::empty` | 1224x1584 | 2.23 | 0.16 | 0.12 |
| all 14 pages @2x (`get()` + render) | 27.1 Mpx | 72.6 ms total = **5.2 ms/page** | | |

alphatrans.pdf page 0 (595x842 pt, transparency groups): 1x 0.75 ms, 2x 2.26 ms, 4x 8.46 ms,
print-quality 3.14 ms. Transparent clear colour (`PdfColor::new(0,0,0,0)`) with BGRA keeps real
alpha (484,566 / 484,704 px have alpha < 255 on the text page) — usable for dark-mode
compositing without re-rendering.

Render cost is ~linear in pixel count (≈ 0.65–0.9 ns/px); allocation is not the bottleneck
(reused bitmap == fresh bitmap). Debug-profile numbers for pdfium itself are the same
(dylib is optimised); only Rust-side copies/encoders change.

## 3. Tiled rendering

Two ways exist in 0.9.4; both were verified pixel-exact against a crop of the full 8x render.

**A. `set_origin` (form-data path, recommended).** `FPDF_RenderPageBitmap(bitmap, page,
start_x=-tx, start_y=-ty, size_x=full_w, size_y=full_h, rotate, flags)` into a *tile-sized*
bitmap, followed by `FPDF_FFLDraw` with the same offsets. Pdfium clips to the bitmap. Forms and
annotations are drawn.

```rust
let cfg = PdfRenderConfig::new()
    .scale_page_by_factor(scale)          // full page would be round(points*scale) px
    .render_form_data(true).render_annotations(true)
    .set_clear_color(PdfColor::WHITE)
    .set_origin(-tx, -ty);                // tile origin in device px of the scaled page
let mut bmp = PdfBitmap::empty(tile_w, tile_h, PdfBitmapFormat::BGRA)?;
page.render_into_bitmap_with_config(&mut bmp, &cfg)?;
```

**B. matrix + clip (`FPDF_RenderPageBitmapWithMatrix`).** Any `translate/scale/apply_matrix`
or `clip()` on the config switches to this path and **silently sets `render_form_data=false`**
(form fields are not drawn; annotations still are via `FPDF_ANNOT`). Convention verified:
`translate()` is multiplied *before* the scale factor (row-vector, `M = T * S`), in a **y-down
device space in points**, so offsets are `-tx/scale, -ty/scale`; the y-flipped variant renders
blank (0 non-white px, 49,275 px wrong).

```rust
let cfg = screen_config(scale)
    .translate(PdfPoints::new(-(tx as f32) / scale), PdfPoints::new(-(ty as f32) / scale))?
    .clip(0, 0, tile_w, tile_h);
```

Verification (1024x1024 tile at (1024, 2048) of the 8x render, 4896x6336):
set_origin vs full-render crop: **427 / 1,048,576 px differ, max channel delta 1** (AA rounding);
matrix+clip: identical 427 px; set_origin vs matrix tile-to-tile: **0 px differ**. Spot checks
at (10,10), (500,300), (1000,1000), (700,20) all equal.

Timings (release, 8x, page 0 already loaded):

| tile | set_origin ms | matrix+clip ms | non-white px |
|---|---|---|---|
| 256 @(1024,2048) | 0.01 | 0.00 | 0 % |
| 512 @(1024,2048) | 0.01 | 0.01 | 0 % |
| 1024 @(1024,2048) | 0.92 | 0.92 | 4.7 % |
| 2048 @(1024,2048) | 7.62 | 7.67 | 10.5 % |
| 256 @ dense text (720,2640) | 0.24 | – | 8.3 % |
| 512 @ dense text | 0.79 | – | 12.4 % |
| 1024 @ dense text | 2.61 | – | 15.6 % |
| 256 in blank margin | 0.00 | – | 0 % |

Pdfium culls objects outside the clip, so empty tiles are free and tile cost tracks content.
Covering a whole page with tiles vs one full render (same open page):
2x: full 2.3 ms vs 12x512 px 2.6 ms vs 4x1024 px 2.9 ms; 4x: full 6.6 ms vs 35x512 px 7.5 ms vs
12x1024 px 7.0 ms — **tiling overhead ≤ 25 %**, so there is no reason to ever render a huge
full page. Full 8x render is 38.9 ms and 118 MiB — never do that in the viewer.

## 4. Encoding benchmark (2x text page, 1224x1584, 7,755,264 B RGBA; release)

| encoding | bytes | encode ms | ratio |
|---|---|---|---|
| raw RGBA | 7,755,264 | 0 | 1.0 |
| PNG RGBA `Compression::Fast` + `Filter::NoFilter` | 3,634,153 | 4.30 | 2.1 |
| PNG RGBA `Fastest` + `NoFilter` | 3,634,153 | 4.40 | 2.1 |
| **PNG RGBA `Fast` + `Filter::Sub`** | **1,398,932** | **2.29** | 5.5 |
| PNG RGBA `Fast` + `Adaptive` | 1,088,186 | 4.28 | 7.1 |
| PNG RGBA `Balanced` + `Adaptive` | 431,654 | 30.4 | 18.0 |
| PNG RGB `Fast` + `NoFilter` (+1.0 ms strip) | 2,906,094 | 4.45 | 2.7 |
| JPEG q85 RGB (image crate) | 410,733 | 12.7 | 18.9 |
| JPEG q75 | 337,033 | 11.0 | 23.0 |
| JPEG q92 | 508,923 | 12.1 | 15.2 |

Debug build: PNG 141 ms, JPEG 249 ms, BGR->RGBA 188 ms — 30–60x slower; never judge encoders
from `cargo run` without `--release`. png 0.18 API: `png::Encoder::new(w, width, height)`,
`set_color(png::ColorType::Rgba)`, `set_depth(png::BitDepth::Eight)`,
`set_compression(png::Compression::{NoCompression,Fastest,Fast,Balanced,High})`,
`set_filter(png::Filter::{NoFilter,Sub,Up,Avg,Paeth,Adaptive,MinEntropy})`. image 0.25:
`image::codecs::jpeg::JpegEncoder::new_with_quality(&mut Vec<u8>, q).encode(&rgb, w, h,
image::ExtendedColorType::Rgb8)` — JPEG needs RGB input (strip alpha first, 1 ms).

## 5. Engine ownership model (the important part)

### What compiles
* With `thread_safe`, all of `Pdfium`, `PdfDocument<'static>`, `PdfPages`, `PdfPage<'static>`,
  `PdfBitmap`, `PdfPageText`, `PdfRenderConfig` are `Send + Sync`; `Mutex<Engine>` (and even a
  bare `Engine` holding `PdfDocument<'static>` + `PdfPage<'static>`) satisfies
  `tauri::Manager::manage`'s `Send + Sync + 'static`. Without the feature every `unsafe impl` is
  cfg'd out and nothing can leave the thread that created it.
* `Box::leak(Box::new(Pdfium::new(bindings)))` -> `&'static Pdfium` -> `PdfDocument<'static>`
  works and is cheap (`Pdfium::drop` never destroys the library anyway). `OnceLock<Pdfium>`
  works identically. A second `Pdfium` handle after the bind: `Pdfium::default()`.
  A second `bind_to_library` returns `PdfiumError::PdfiumLibraryBindingsAlreadyInitialized`.
* **`PdfPage<'a>` borrows the `Pdfium` lifetime, not the document.** `doc.pages().get(i)`
  returns `PdfPage<'a>`, so with `'a = 'static` the compiler happily lets you `drop(doc)` while a
  page is alive (guarded probe in the example, `SPIKE_UAF=1`, compiles) — that is a
  use-after-free inside pdfium (`FPDF_ClosePage` after `FPDF_CloseDocument`). Engine code MUST
  drop pages before their document (the spike's `Engine::close` and `impl Drop` do this).
* `PdfPageText<'a>` holds `&'a PdfPage<'a>`: dropping the page while text is alive is a
  compile error — verified: `error[E0505]: cannot move out of 'page' because it is borrowed`.
* Two documents open at once: fine. Interleaved renders, close one, keep rendering the other.

### Why `thread_safe` + global Mutex is NOT enough
`src/bindings/thread_safe.rs` wraps 481 FFI functions but takes the global
`PdfiumThreadMarshall` lock in only **289** of them. Unlocked (verified by scanning the file)
include: `FPDF_RenderPageBitmapWithMatrix` (the matrix/clip tile path!), `FPDF_RenderPage`,
`FPDFBitmap_CreateEx`, `FPDFBitmap_FillRect`, `FPDF_GetPageSizeByIndexF`, `FPDF_GetPageLabel`,
`FPDF_GetMetaText`, `FPDFBookmark_GetTitle/GetNextSibling`, `FPDFText_GetText`,
`FPDFText_GetCharBox`, `FPDFText_GetCharIndexAtPos`, `FPDFText_CountRects/GetRect`,
`FPDFText_GetBoundedText`, `FPDFText_FindStart`, `FPDFDOC_InitFormFillEnvironment`,
`FORM_OnMouseMove/LButtonDown/LButtonUp/Focus/…`, `FPDFAnnot_GetColor/SetColor/GetAP/
GetFormFieldValue/…`, `FPDFTextObj_*`, `FPDFImageObj_SetBitmap/GetRenderedBitmap`.
So two threads that both hold `&PdfDocument` (which is `Sync`) can call pdfium concurrently
through these paths with no lock at all. The feature is fine for *moving* handles between
threads; it is not a correctness guarantee for concurrent use.

Measured: 14 renders @2x through `Arc<Mutex<Engine>>` — 1 thread 58.9 ms, 2 threads 59.2 ms;
2 threads sharing `&PdfDocument` without a mutex 66.9 ms. Zero parallelism, as expected from a
global lock (and pdfium's own single-threaded design).

### Decision: (a) dedicated engine thread over crossbeam
Prototype in the example (`spawn_engine_thread`): the thread owns `Pdfium` on its stack,
`HashMap<DocId, PdfDocument<'_>>` and `HashMap<(DocId, PdfPageIndex), PdfPage<'_>>` borrow it
locally (no `'static`, no leak needed), commands `Open/Render/Close/Shutdown` arrive on an
unbounded `crossbeam_channel`, replies go back on per-request `bounded(1)` channels. Measured:
ping round-trip **0.3 us**, first 2x render of a page 7.0 ms (incl. `FPDF_LoadPage`), cached
page 2.6 ms — the channel is free compared with the work. Shutdown drops pages then docs on the
engine thread.

Justification vs (b) global `Mutex<Engine>` in Tauri state:
1. Soundness: (a) makes the partial locking above irrelevant; (b) relies on it.
2. Drop order and lifetimes are local to one function; no `&'static` juggling.
3. Cancellation / priority queues / progress are natural on a command loop (drop stale
   requests before rendering); with (b) every Tauri command thread would block on the mutex.
4. pdfium's per-document font/image caches stay thread-affine (cache hits: page 0 3.7 ms ->
   0.5 ms for later pages).
5. Cost: one thread, one channel, ~0.3 us per call. No downside measured.

The Tauri managed state should therefore be `EngineHandle { tx: Sender<Cmd> }` (Send+Sync,
Clone) — never a `PdfDocument`.

## 6. Memory (RSS from `ps`, release; isolated `SPIKE_ONLY=memory` run, `/usr/bin/time -l`)

| point | RSS |
|---|---|
| process start | 6.2 MiB |
| after bind (libpdfium mapped, FPDF_InitLibrary) | 15.2 MiB |
| after `load_pdf_from_byte_vec` tracemonkey (1 MB) | 16.7 MiB |
| after rendering all 14 pages @2x, keeping 14 `PdfPage`s open + 103.5 MiB RGBA cache | 145.2 MiB (=> pdfium doc + 14 pages + caches ≈ **25 MiB**) |
| + one transient 4x render (29.6 MiB bitmap, dropped) | 175.5 MiB and stays there after dropping cache/pages/doc |
| `/usr/bin/time -l` max RSS / peak footprint | 184 MB / 168 MB |

Full spike run (includes the 8x 118 MiB reference render and its copies): max RSS 661 MB,
peak footprint 645 MB, 1.8 s wall. macOS malloc keeps freed large blocks, so RSS is a
high-water mark: bound the largest single allocation (tiles!) and the cache budget.

## Recommended render pipeline for the viewer

1. **Engine thread** (`src-tauri/src/engine/`): `Pdfium` + docs + `PdfPage` LRU (32 pages,
   ~1–2 MiB each; saves 0.5–26 ms per page and pdfium's parse/font caches). Commands carry a
   `viewport_generation`; the loop drains the queue and skips requests whose generation is
   stale before touching pdfium. Two channels (`hi` = visible tiles, `lo` = prefetch/thumbnails)
   with a biased `select!`, or one channel + in-memory priority heap re-sorted per loop.
   Long jobs (OCR, save) go through the same thread but in chunks (one page per command) so
   tiles interleave.
2. **Zoom levels & tiles.** Device scale `s = zoom * devicePixelRatio`. For `s <= 2` (page
   ≤ ~2 Mpx) render the whole page as one bitmap (≤ 2.3 ms). For `s > 2` render **512x512
   device-pixel tiles** with `set_origin(-tx, -ty)` (0.01–0.8 ms each; 20 visible tiles on a
   2560x1440 viewport ≈ 5–15 ms => one frame). Page pixel size must be computed exactly as
   pdfium-render does: `round(points * s)`. Never use the matrix/clip path in the viewer
   (drops form fields); never render full pages above 4x (30–120 MiB bitmaps).
3. **Placeholder + progressive.** Keep a 1x (or fit-width) whole-page bitmap per visible page
   (≈ 1 ms, 1.9 MiB) and upscale it on the canvas while tiles for the new zoom arrive
   (pdf.js/Preview behaviour). Thumbnails: `PdfRenderConfig::new().thumbnail(200)` (1.5 ms),
   low priority, cached forever per (doc generation, page).
4. **Cache.** LRU keyed `(doc_id, doc_generation, page, s_key, tx, ty)`; budget ~256 MiB raw
   RGBA (≈ 250 tiles of 512 px @ 1 MiB) or ~64 MiB PNG. Bump `doc_generation` on every edit
   (annotation, form value, page op) so all tiles of that page invalidate. The webview keeps its
   own small `ImageBitmap` cache for the visible set only.
5. **Transport.** Default: raw pixels, `set_reverse_byte_order(true)` + `as_raw_bytes()`
   (already RGBA, 0.16 ms copy, no swizzle) returned as `tauri::ipc::Response::new(Vec<u8>)`
   (binary IPC, verified to exist in tauri 2.11.5 `src/ipc/mod.rs`); JS wraps it in
   `ImageData` -> `createImageBitmap` -> `drawImage`. Alternative for `<img>`/`fetch` caching:
   `register_uri_scheme_protocol("seepdf-tile", …)` (tauri 2.11.5 `src/app.rs`) serving PNG
   `Compression::Fast` + `Filter::Sub` (≈ 0.2 ms per tile, 5.5x smaller than raw). JPEG q85
   only for scanned/image-only pages (12.7 ms/page, 19x). Do not base64 into JSON.
6. **Format/flags.** Always BGRA (BGR costs 84 ms to convert); LCD text off (subpixel AA is
   wrong once composited in a browser canvas); annotations on; forms on; white clear colour,
   or transparent clear colour when a dark page background is drawn by the UI.
7. **Rotation.** User rotation via `.rotate(PdfPageRenderRotation::DegreesN, true)`; page
   intrinsic `/Rotate` is already applied by pdfium (`page.width()` swaps). Cost is negligible.

## Gotchas (exact)

* `pdfium-render` 0.9.4 `thread_safe` locks only 289/481 FFI functions (list above) — treat
  the crate as single-threaded; use the engine thread.
* Any of `translate/scale/rotate_*/apply_matrix/transform/clip()` on `PdfRenderConfig` sets
  `do_render_form_data = false` silently; `set_origin()` is then ignored (matrix path only
  honours the matrix + clip). Forms need the `set_origin` tile path.
* `PdfPages::get()` calls `FPDF_LoadPage` every time (0.5–26 ms) — cache `PdfPage`s.
* `PdfPage<'a>` is not tied to its `PdfDocument`: drop pages before documents, always.
* `PdfPageText` borrows the page (`E0505` if you try to drop the page first) — return owned
  `String`/rects from the engine instead of keeping `PdfPageText` around.
* `as_rgba_bytes()` = two copies (GetBuffer -> Vec, then swizzle). Use
  `set_reverse_byte_order(true)` + `as_raw_bytes()` for one copy. BGRA rows are tightly packed
  (stride == width*4); BGR rows are 4-byte padded and the converter is 400x slower.
* `Pdfium::bind_to_library` must be called exactly once per process; `Pdfium::default()`
  afterwards yields another handle; `Pdfium::drop` never calls `FPDF_DestroyLibrary`.
* `PdfPageIndex` is `i32`, `Pixels` is `i32`. `PdfRect::new(bottom, left, top, right)` order.
* pdfium-render 0.9.4 binds against the pdfium **7881** header set while the shipped lib is
  8057 — all symbols resolved (`bind_to_library` loads every symbol eagerly and errors
  otherwise); no issue seen.
* `[profile.release]` in Cargo.toml has fat LTO + `codegen-units = 1`; for benchmark builds use
  `CARGO_PROFILE_RELEASE_LTO=false CARGO_PROFILE_RELEASE_CODEGEN_UNITS=16 cargo build
  --release --example spike_render` (61 s cold instead of many minutes; does not touch
  Cargo.toml).
* RSS never drops after big renders (macOS malloc); budget by peak allocation.

## How the numbers were taken

```sh
export PATH="$HOME/.cargo/bin:$PATH"; cd src-tauri
cargo run --example spike_render                     # debug: exit 0, ~4 s (encoder numbers meaningless)
CARGO_PROFILE_RELEASE_LTO=false CARGO_PROFILE_RELEASE_CODEGEN_UNITS=16 cargo build --release --example spike_render
/usr/bin/time -l ./target/release/examples/spike_render                      # 1.80 s real, max RSS 661 MB
SPIKE_ONLY=memory /usr/bin/time -l ./target/release/examples/spike_render    # 0.12 s real, max RSS 184 MB
```
Optional env: `SPIKE_FIXTURES=<dir>`, `SPIKE_PDFIUM=<path>`, `SPIKE_QUICK=1` (skip 8x reference).

No extra crates were needed: pdfium-render, png, image, crossbeam-channel from Cargo.toml.
Nice-to-have later: `lru` (tile cache) — or a hand-rolled `HashMap + VecDeque`.
