# OCR feasibility spike (2026-09-15)

Goal: pick the OCR engine(s) for SeePDF (Tauri v2: macOS WKWebView + Windows WebView2) that can OCR scanned
Korean + English PDFs at commercial-like quality **and** return word boxes so we can add a searchable
(invisible) text layer with pdfium.

Everything below was measured on this machine: Apple M5 (10 cores), macOS 26, Node 24.15, Rust 1.96,
pdfium-render 0.9.4 + PDFium chromium/8057, tesseract.js 7.0.0 / tesseract.js-core 7.0.0.

Artifacts:

| File | What |
|---|---|
| `src-tauri/examples/spike_ocr_render.rs` | renders test images, builds a synthetic Korean page, proves the invisible text layer end to end (`cargo run --example spike_ocr_render`) |
| `scripts/spike-ocr.mjs` | tesseract.js benchmark: `node scripts/spike-ocr.mjs [--variant best_int\|fast\|full] [--psm AUTO\|SINGLE_COLUMN\|...] [--langs eng,kor+eng] [--images tracemonkey,korean]` |
| `fixtures/out/ocr/` | 300-DPI PNGs, ground-truth `.txt`, OCR outputs, `spike-ocr-results-*.json`, generated PDFs, tesseract traineddata cache (108 MB total; **not gitignored yet – add `fixtures/out/` to `.gitignore`**) |
| `package.json` | `tesseract.js ^7.0.0` added to `dependencies` (the only change) |

---

## 1. Recommendation (TL;DR)

1. **Cross-platform baseline: tesseract.js 7 (WASM, LSTM-only core) in a Web Worker, fully bundled for offline use.**
   Installer impact **≈ 8.4 MB** (worker.min.js 0.11 MB + `tesseract-core-simd-lstm.wasm.js` 3.72 MB +
   `eng` 2.95 MB + `kor` 1.57 MB `.traineddata.gz`, the `4.0.0_best_int` files tesseract.js defaults to).
   Language string is always `kor+eng` for Korean documents (`kor` alone garbles English, see §4).
2. **macOS: use the native Vision framework (`VNRecognizeTextRequest`, `ko-KR`) as the primary engine, Tesseract as fallback.**
   It is dramatically better on Korean (CER 0.21 % vs 2.1–12 % for Tesseract on the same page), 3–8x faster
   (448 ms vs 1.5–2.0 s for the Korean page; 1.08 s vs 3.5–4.5 s for a dense English page), costs 0 bytes,
   returns per-word boxes, and needs no language downloads. Bind through `objc2-vision` 0.3.2 (+ `objc2`,
   `objc2-foundation`, `objc2-core-graphics`), all present on the USTC index; API shape verified in §3a.
3. **Windows: ship Tesseract only for v1.** `Windows.Media.Ocr` (via the `windows` crate, feature `Media_Ocr`)
   works only if the user has installed the Korean OCR language pack, caps images at 2600 px on the long side
   (a 300-DPI Letter page is 3300 px), and gives no per-word confidence. Worth adding later as an optional
   "use Windows OCR when the Korean pack is installed" path, not as the guaranteed engine.
4. Rejected: `ocrs` (Latin-only, verified in source), `leptess` / `tesseract-rs` (native libs; needs vcpkg on
   Windows and shipping tesseract + leptonica + image codec DLLs for the *same* engine and models we already get
   from WASM).
5. Text layer: pdfium invisible text objects (render mode 3) work and are searchable in any viewer
   (`/ToUnicode` CMap is generated). **Bundle a small Hangul font for the layer** – pdfium embeds the *whole*
   font file with no subsetting (a 15 MB system TTF produced a 6.2 MB PDF; the 55 MB `.ttc` a 30 MB PDF).
   Use the built-in Helvetica (no embedding) for pure Latin-1 words.

---

## 2. Test material (step 1)

`cargo run --example spike_ocr_render` (compiles clean, runs in ~2 s) produces:

| Output | Facts |
|---|---|
| `tracemonkey-p1-300dpi.png` | 2550x3300 px gray8, 932 KB. Render time **12–21 ms** (`PdfRenderConfig::new().scale_page_by_factor(300.0/72.0)`), so rendering is never the bottleneck |
| `tracemonkey-p1.txt` | pdfium `page.text()?.all()`: 5067 chars / 708 words, used as ground truth |
| `korean-300dpi.png` | synthetic A4 page (2480x3508 px, 172 KB), 10 lines of Korean/English/mixed text at 9–18 pt, rendered in 4 ms |
| `korean.txt` | the exact 485 chars / 91 words drawn |
| `korean-synthetic.pdf` | 30 MB, because `FPDFText_LoadFont` embedded all of `AppleSDGothicNeo.ttc` |
| `invisible-text-proof.pdf` | 2 invisible words (Hangul + Latin) with AppleGothic; see §5 |
| `tracemonkey-p1-searchable-{auto,manual}.pdf` | 300-DPI image + 730 invisible OCR words; see §5 |

Font loading: `/System/Library/Fonts/AppleSDGothicNeo.ttc` **does** load via
`doc.fonts_mut().load_true_type_from_file(path, /*is_cid_font*/ true)` (FreeType opens face 0 of the TTC).
Single-face fallbacks with Hangul on a stock Mac: `/System/Library/Fonts/Supplemental/AppleGothic.ttf` (15.3 MB),
`AppleMyungjo.ttf` (18.8 MB), `Arial Unicode.ttf` (23.3 MB). `NotoSansGothic-Regular.ttf` is the *Gothic
alphabet*, not Hangul. No NanumGothic on this machine.

---

## 3. Engine assessment

### 3a. tesseract.js 7.0.0 (WASM) – measured (step 2)

Setup: `createWorker(lang, OEM.LSTM_ONLY, { cachePath, langPath, gzip })`, then
`worker.setParameters({ tessedit_pageseg_mode, user_defined_dpi: '300', preserve_interword_spaces: '1' })`,
`worker.recognize(png, {}, { text: true, blocks: true })`. CER = Levenshtein distance / ground-truth length on
whitespace-normalised NFC text; "Hangul CER" is the same on Hangul code points only. Word recall/precision are
bag-of-words. Init = `createWorker` wall time (core instantiate + traineddata load); "cold" includes the first
download, "warm" reads the cache. All numbers single-run, `fixtures/out/ocr/spike-ocr-results-*.json` has the raw rows.

| traineddata | langs | PSM | image | init cold / warm | recognize | CER | Hangul CER | word recall / precision |
|---|---|---|---|---|---|---|---|---|
| best_int | eng | 3 AUTO | tracemonkey | 333 / 69 ms | **3473 ms** | **1.40 %** | – | 93.8 / 90.5 % |
| best_int | eng | 3 AUTO | korean | | 495 ms | 48.5 % | 100 % | 35 / 38 % |
| best_int | kor+eng | 3 AUTO | tracemonkey | 222 / 82 ms | 4487 ms | 1.38 % | – | 94.1 / 90.6 % |
| best_int | kor+eng | 3 AUTO | korean | | 902 ms | 12.29 % | 23.84 % | 83.5 / 96.2 % (a whole paragraph dropped, see §4) |
| best_int | kor+eng | 4 SINGLE_COLUMN | korean | 102 / 139 ms | 1544 ms | **2.71 %** | **1.16 %** | 95.6 / 94.6 % |
| best_int | kor+eng | 6 SINGLE_BLOCK | korean | | 2001 ms | **2.08 %** | **0.00 %** | 95.6 / 95.6 % |
| best_int | kor+eng | 11 SPARSE_TEXT | korean | | 2054 ms | 18.5 % | 19.8 % | 76 / 61 % |
| best_int | eng+kor | 4 SINGLE_COLUMN | korean | | 1240 ms | 2.71 % | 1.16 % | identical to kor+eng (order is irrelevant) |
| best_int | kor | 3 AUTO | korean | 66 / 75 ms | 1541 ms | 43.1 % | 25.6 % | English lines become digits/garbage |
| fast | eng | 3 AUTO | tracemonkey | 614 / 162 ms | **2521 ms** | 1.46 % | – | 93.5 / 89.6 % |
| fast | kor+eng | 3 AUTO | tracemonkey | 444 / 81 ms | 3236 ms | 1.44 % | – | 93.8 / 89.7 % |
| fast | kor+eng | 3 AUTO | korean | | 660 ms | 12.5 % | 24.4 % | same paragraph drop |
| fast | kor+eng | 4 SINGLE_COLUMN | korean | | 834 ms | 3.54 % | **6.98 %** | 92.3 / 92.3 % |
| full (tessdata 4.0.0) | kor+eng | 3 AUTO | tracemonkey | 158 / 52 ms | 4962 ms | **90.3 %** | – | garbage – do not use with the LSTM-only core |
| full (tessdata 4.0.0) | kor+eng | 3 AUTO | korean | | 651 ms | 32.1 % | 4.07 % | English garbled |

Residual tracemonkey errors (both engines) are almost entirely author-affiliation symbols (`∗ † $ # +` read as
`* ® ™ ”`) and a few inserted spaces; body text is correct.

Word boxes / confidences: every word carries `bbox {x0,y0,x1,y1}` in **image pixels, origin top-left** and a 0–100
`confidence`; every line has `baseline {x0,y0,x1,y1}`, `rowAttributes {ascenders, descenders, rowHeight}` and
`bbox`. Sample: `{"text":"광학","confidence":96,"bbox":{"x0":497,"y0":273,"x1":623,"y1":339}}`. Symbols
(`word.symbols[]`) also carry boxes. Data comes back only when `{ blocks: true }` is passed as the third
`recognize` argument (v6+ dropped the old `data.words`).

Throughput with a `createScheduler()` of N workers, `kor+eng`, PSM AUTO, the tracemonkey page x8:

| workers | 8 pages wall | per page | process RSS |
|---|---|---|---|
| 1 | 34.1 s | 4257 ms | 251 MB |
| 2 | 19.3 s | 2412 ms | 369 MB |
| 4 | 11.8 s | 1470 ms | 582 MB |
| 8 | 8.0 s | 997 ms | 1002 MB |

So ~95–120 MB RSS per worker, warm init 70–170 ms, 4.3x speed-up at 8 workers on 10 cores.

Bundle facts (from `node_modules`, tesseract.js 7.0.0):

| File | Size | Notes |
|---|---|---|
| `tesseract.js/dist/worker.min.js` | 0.11 MB | must be served same-origin (default is jsdelivr) |
| `tesseract.js/dist/tesseract.esm.min.js` | 0.06 MB | main-thread API (Vite bundles `tesseract.js` import) |
| `tesseract.js-core/tesseract-core-simd-lstm.wasm.js` | 3.72 MB | base64-embedded WASM; what the browser worker `importScripts` |
| `tesseract.js-core/tesseract-core-simd-lstm.js` + `.wasm` | 0.09 + 2.73 MB | split alternative (pass the `.js` as `corePath`; the glue fetches the `.wasm` next to it) |
| `tesseract-core-lstm.wasm.js` | 3.90 MB | non-SIMD fallback, only needed for very old WebViews |
| `eng.traineddata.gz` (4.0.0_best_int) | 2.95 MB | 4.96 MB inflated |
| `kor.traineddata.gz` (4.0.0_best_int) | 1.57 MB | 2.11 MB inflated |
| `tessdata_fast` kor / eng (uncompressed) | 1.60 / 3.92 MB | 20–40 % faster, Hangul CER 7 % vs 1.2 % – not worth it |
| `tessdata` 4.0.0 kor / eng (.gz) | 6.63 / 10.42 MB | legacy+LSTM, produced garbage here |

Total to ship (recommended): **8.4 MB** (`worker.min.js` + `simd-lstm.wasm.js` + eng.gz + kor.gz), or 7.4 MB with
the split core. tesseract.js inflates `.gz` itself (zlibjs) when `gzip: true`.

Download sources (all reachable here): `https://cdn.jsdelivr.net/npm/@tesseract.js-data/<lang>/4.0.0_best_int/<lang>.traineddata.gz`
(tesseract.js default), `https://tessdata.projectnaptha.com/4.0.0{,_best,_fast}/<lang>.traineddata.gz`,
`https://raw.githubusercontent.com/tesseract-ocr/tessdata_fast/main/<lang>.traineddata`.

### 3b. macOS Vision (`VNRecognizeTextRequest`) – measured with a Swift script, crate API verified

Runtime probe (Swift, CLT toolchain 6.3.2, same PNGs):

| image | languages | first `perform()` (model load) | warm | CER | Hangul CER | word recall / precision |
|---|---|---|---|---|---|---|
| korean | `["ko-KR","en-US"]`, `.accurate` | 757 ms | **448 ms** | **0.21 %** | **0.00 %** | 97.8 / 98.9 % |
| tracemonkey | `["en-US"]`, `.accurate` | 1090 ms | **1075 ms** | 1.54 % | – | 93.9 / 89.7 % |

Vision got every case Tesseract missed: `₩ 48,500`, `1,250,000`, `Tuesday`, the dropped paragraph. Request
revision 3 lists `ko-KR` among the 30 supported languages on this Mac (Korean support arrived with revision 3 /
macOS 13). 90 word boxes came back for the 90 words on the page via `boundingBox(for: range)`; boxes are
normalised (0–1, origin bottom-left, i.e. `y_px = (1 - maxY) * height`). Per-observation confidence 0.95 mean.
Vision returns one observation per *line*, no paragraph/column structure, so two-column pages need a manual
column sort (same reading-order effect as Tesseract on tracemonkey).

Rust binding (`objc2-vision` 0.3.2 on USTC, verified from the crate source, not compiled – not added to Cargo.toml):

```toml
[target.'cfg(target_os = "macos")'.dependencies]
objc2 = "0.6"
objc2-foundation = { version = "0.3", features = ["NSArray", "NSString", "NSDictionary", "NSError", "NSRange"] }
objc2-core-foundation = "0.3"
objc2-core-graphics = { version = "0.3", features = ["CGImage", "CGDataProvider", "CGColorSpace"] }
objc2-vision = { version = "0.3", default-features = false, features = ["std", "VNRequest", "VNRecognizeTextRequest", "VNRequestHandler", "VNObservation", "VNTypes", "objc2-core-graphics"] }
```

Verified signatures (all `unsafe` unless noted):

```rust
VNRecognizeTextRequest::init(this: Allocated<Self>) -> Retained<Self>              // VNRecognizeTextRequest::alloc().init()
VNRecognizeTextRequest::setRecognitionLanguages(&self, &NSArray<NSString>)         // safe fn; ["ko-KR", "en-US"]
VNRecognizeTextRequest::setRecognitionLevel(&self, VNRequestTextRecognitionLevel)  // ::Accurate = 0, ::Fast = 1
VNRecognizeTextRequest::setUsesLanguageCorrection(&self, bool)
VNRecognizeTextRequest::setAutomaticallyDetectsLanguage(&self, bool)               // macOS 13+
VNRecognizeTextRequest::supportedRecognitionLanguagesAndReturnError(&self) -> Result<Retained<NSArray<NSString>>, Retained<NSError>>
VNImageRequestHandler::initWithCGImage_options(this, &CGImage, &NSDictionary<VNImageOption, AnyObject>) -> Retained<Self>
VNImageRequestHandler::performRequests_error(&self, &NSArray<VNRequest>) -> Result<(), Retained<NSError>>   // safe fn
VNRequest::results(&self) -> Option<Retained<NSArray<VNObservation>>>              // downcast to VNRecognizedTextObservation
VNRecognizedTextObservation::topCandidates(&self, NSUInteger) -> Retained<NSArray<VNRecognizedText>>  // safe fn
VNRecognizedText::string(&self) -> Retained<NSString>; ::confidence(&self) -> VNConfidence (f32)
VNRecognizedText::boundingBoxForRange_error(&self, NSRange) -> Result<Retained<VNRectangleObservation>, Retained<NSError>>  // hand-written in src/observation.rs
VNDetectedObjectObservation::boundingBox(&self) -> CGRect                           // normalised
CGImage::new(width, height, bits_per_component, bits_per_pixel, bytes_per_row, Option<&CGColorSpace>, CGBitmapInfo, Option<&CGDataProvider>, decode: *const CGFloat, should_interpolate, CGColorRenderingIntent) -> Option<CFRetained<CGImage>>
CGDataProvider::with_data(info: *mut c_void, data: *const c_void, size: usize, release: CGDataProviderReleaseDataCallback) -> Option<CFRetained<CGDataProvider>>
```

Data flow on macOS is Rust-only: pdfium bitmap (`bitmap.as_raw_bytes()`, BGRA, 4 bytes/px) →
`CGDataProvider::with_data` → `CGImage::new(w, h, 8, 32, w*4, Some(&CGColorSpace::new_device_rgb()),
kCGBitmapByteOrder32Little | kCGImageAlphaNoneSkipFirst, …)` → handler → request → observations. No image ever
crosses the IPC boundary. `NSRange` for `boundingBoxForRange` is in UTF-16 units of the candidate string; split
the candidate on whitespace and take one box per word (what the Swift probe did), or per character for CJK.
Vision uses the GPU/ANE internally; run it on a dedicated OCR thread, not the pdfium engine thread.

### 3c. Windows.Media.Ocr (`windows` crate 0.62.2 on USTC, 674 features) – assessed, not compiled

Features: `Media_Ocr` (implies `Media`), `Globalization`, `Graphics_Imaging`, `Storage_Streams`,
`Foundation_Collections`. API shape (WinRT):

```rust
use windows::{Globalization::Language, Graphics::Imaging::{SoftwareBitmap, BitmapPixelFormat, BitmapAlphaMode}, Media::Ocr::OcrEngine, Storage::Streams::DataWriter, core::h};
let lang = Language::CreateLanguage(h!("ko"))?;
if !OcrEngine::IsLanguageSupported(&lang)? { /* Korean OCR pack not installed – fall back to Tesseract */ }
let engine = OcrEngine::TryCreateFromLanguage(&lang)?;          // or TryCreateFromUserProfileLanguages()
let max = OcrEngine::MaxImageDimension()?;                       // 2600 px – downscale 300-DPI Letter (3300 px) to fit
let bmp = SoftwareBitmap::CreateCopyFromBuffer(&buffer, BitmapPixelFormat::Bgra8, w, h)?;   // buffer from DataWriter over pdfium BGRA bytes
let result = engine.RecognizeAsync(&bmp)?.get()?;                 // OcrResult
for line in result.Lines()? { for word in line.Words()? { let r = word.BoundingRect()?; let t = word.Text()?; } }
let angle = result.TextAngle()?;                                 // IReference<f32>
```

Korean support: `OcrEngine` only knows languages whose *OCR* pack is installed (Settings > Time & Language >
Language > Korean > Options > "Optical character recognition", or `Add-WindowsCapability -Online -Name
Language.OCR~~~ko-KR~0.0.1.0`). It cannot be bundled with the app. It is one engine per language (no `kor+eng`
mixing; it recognises Latin inside Korean reasonably but not vice versa), returns no confidence, and quality is
roughly Tesseract-level (not measured here – no Windows machine). Fine as an opportunistic accelerator later,
not as the baseline.

### 3d. Pure-Rust `ocrs` 0.13.1 – rejected

`src/lib.rs`: `const DEFAULT_ALPHABET: &str = " 0123456789!\"#$%&'()*+,-./:;<=>?@[\\]^_\`{|}~EABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";`
and the upstream README: "ocrs currently recognizes the Latin alphabet only (eg. English). Support for more
languages is planned". No Hangul, so unusable for the primary user.

### 3e. Tesseract via Rust (`leptess` 0.14 / `tesseract` 0.15 / `tesseract-rs` 0.4) – rejected for v1

`tesseract-sys` 0.6.3 `build.rs`: on Windows it calls `vcpkg::Config::new().find_package("tesseract")` unless
`TESSERACT_INCLUDE_PATHS/LINK_PATHS/LINK_LIBS` are set; macOS/Linux use `pkg-config` (no Homebrew/tesseract on
this Mac, so it does not even build here). Shipping would mean tesseract55.dll + leptonica-1.8x.dll + libpng,
libjpeg, libtiff, libwebp, zlib, openjp2, gif (vcpkg dynamic triplet) ≈ 10–15 MB of DLLs per arch plus the same
traineddata, i.e. more bytes than the WASM build for the same engine and models. `tesseract-rs` 0.4 instead
compiles tesseract from source with cmake at build time (adds `reqwest`, `zip`, `cmake` build deps) – slow CI,
same runtime story. Native tesseract would be ~2–3x faster than WASM, but Vision already covers the platform
where speed matters most for our user base, and the Windows gap is better closed by the Windows OCR pack later.

---

## 4. Gotchas (all observed or verified in source)

1. **PSM 3 (AUTO) dropped a whole paragraph** (line 3 and half of line 2) on the sparse Korean page with
   `kor+eng`; PSM 4 (SINGLE_COLUMN) and 6 (SINGLE_BLOCK) recovered everything (Hangul CER 1.2 % / 0 %). AUTO is
   still right for multi-column documents (PSM 6 would interleave columns on tracemonkey). Expose a layout option
   (자동 / 단일 단 / 단일 블록), default 자동, and rely on Vision on macOS, which has no such mode.
2. `kor` alone reads English as digits (`SeePDF` → `5660마`); always request `kor+eng`. Language order does not
   matter. With `kor+eng` in PSM 4/6 a Latin word was read as digits (`Tuesday` → `146503\`) that PSM 3 got right.
3. `₩` is outside the Tesseract kor/eng charsets (comes back as `W` or `\`), thousands separators are sometimes
   dropped (`1,250,000` → `1250,000`). Vision reads both correctly.
4. `blocks[].paragraphs[].lines[].words[]` over-segments Korean (`갑은 을에게` → `갑`,`은`,`을`,`에`,`게`, 140–170
   "words" for 91 real ones) while `line.text` / `data.text` are spaced correctly. Rebuild spacing from
   `line.text` when emitting the layer (walk the line's words, check whether the next char in `line.text` is a
   space); never join words with a space blindly.
5. The `4.0.0` (legacy+LSTM) traineddata files gave garbage with the LSTM-only core (CER 90 % on English). Ship
   the `4.0.0_best_int` files (tesseract.js 7 default). `tessdata_fast` is smaller uncompressed but worse on Hangul.
6. tesseract.js defaults point **everything** at jsdelivr: `workerPath`
   (`https://cdn.jsdelivr.net/npm/tesseract.js@v7.0.0/dist/worker.min.js`), `corePath`
   (`https://cdn.jsdelivr.net/npm/tesseract.js-core@v7.0.0`), `langPath` (`@tesseract.js-data/<lang>/4.0.0_best_int`).
   For offline use set all three to app-local paths, `gzip: true`, `cacheMethod: 'none'` (IndexedDB caching is
   pointless for local files). In Node the same code reads local directories; in the browser `langPath` must be a URL.
7. The browser worker is spawned as a `blob:` URL that `importScripts(workerPath)`, and the worker `importScripts`
   the `.wasm.js` core. Our current CSP (`default-src 'self' ipc: http://ipc.localhost`, no `script-src` /
   `worker-src`) will block that in WebView2. Add `worker-src 'self' blob:` (or pass `workerBlobURL: false`) and
   `script-src 'self' 'wasm-unsafe-eval'`. Not yet verified inside the Tauri window – first implementation task.
8. WKWebView supports WASM SIMD (Safari 16.4+), not relaxed-SIMD; `wasm-feature-detect` inside the worker picks
   `tesseract-core-simd-lstm` automatically, so ship the `simd-lstm` and plain `lstm` cores only.
9. Memory: ~95–120 MB RSS per worker; init 70–170 ms warm, 220–600 ms cold. Cap workers at
   `min(4, cores - 2)` and keep the scheduler alive between jobs.
10. `FPDFText_LoadFont` (pdfium-render `load_true_type_from_*`) embeds the entire font file as `FontFile2`, no
    subsetting: `.ttc` 55 MB → 30 MB PDF, AppleGothic 15 MB → 6.2 MB PDF. Never embed a system font; bundle a
    small Hangul font (e.g. a Noto Sans KR / NanumGothic subset covering KS X 1001's 2,350 syllables + Latin,
    ~1–1.5 MB; size not measured) and load it once per document with `load_true_type_from_bytes(bytes, true)`.
    Pure Latin-1 words go through `doc.fonts_mut().helvetica()` (standard-14, no embedding).
11. `PdfPageTextObject::bounds()` works on an object that has not been added to a page yet (used to measure the
    natural glyph-run width for fitting). Apply `scale(sx, 1.0)` **before** `translate(x, y)`; the matrix ends up
    as `sx 0 0 1 x y Tm`.
12. `PdfPage` defaults to `PdfPageContentRegenerationStrategy::AutomaticOnEveryChange`; 730 words took 81 ms
    that way vs 2 ms with `Manual` + one `regenerate_content()`. Use Manual for the layer builder.
13. `objects_mut().create_image_object(x, y, &DynamicImage, Some(w), Some(h))` with an 8.4 MP image costs
    ~840 ms and re-encodes the image (1.2 MB). For real scans add only the text objects to the existing page.
14. Vision `boundingBoxForRange_error` lives in `objc2-vision`'s hand-written `src/observation.rs`, not the
    generated bindings – it exists, but `grep` of `src/generated` will not find it. Ranges are UTF-16.
15. Windows.Media.Ocr: `MaxImageDimension` is 2600 px; Korean needs the OS OCR language pack; no confidences.
16. Rotated pages: pdfium's render config applies `/Rotate`, so OCR boxes are in *display* pixel space. Map
    them back with `page.pixels_to_points(x, y, &same_render_config)` (wraps `FPDF_DeviceToPage`, verified to
    take the rotation into account) and rotate the text object by `rotate_counter_clockwise_degrees(rotate)` so it
    reads upright on screen. Reasoned from the pdfium API, not yet measured on `fixtures/rotation.pdf` – add a
    round-trip unit test (`points_to_pixels` → `pixels_to_points`) there.
17. `fixtures/out/` is not gitignored; this spike left 108 MB there (67 MB of it traineddata cache).

---

## 5. Text-layer mechanics (verified with pdfium)

`invisible-text-proof.pdf` (part 3 of the example) and `tracemonkey-p1-searchable-manual.pdf` (part 4) prove the
whole chain with pdfium-render 0.9.4:

```rust
let font = doc.fonts_mut().load_true_type_from_bytes(HANGUL_FONT, true)?;   // once per document
page.set_content_regeneration_strategy(PdfPageContentRegenerationStrategy::Manual);
let mut obj = PdfPageTextObject::new(&doc, word, font, PdfPoints::new(font_size_pt))?;
obj.set_render_mode(PdfPageTextRenderMode::Invisible)?;                     // "3 Tr"
let b = obj.bounds()?; let natural_w = b.right().value - b.left().value;    // works before add
obj.scale(box_w_pt / natural_w, 1.0)?;                                      // fit the OCR box width
obj.translate(PdfPoints::new(x_pt), PdfPoints::new(baseline_pt))?;
page.objects_mut().add_text_object(obj)?;
// ... all words ...
page.regenerate_content()?; doc.save_to_file(path)?;
```

Result: content stream `BT 1.3197643 0 0 1 72 700 Tm /FXF1 14 Tf 3 Tr [<0A850EBA0A5B0BFB12FD>] TJ ET`, font
dictionary `Type0 / CIDFontType2 / Identity-H / FontFile2 / ToUnicode`, page renders all-white, and after
save + reload `page.text()?.all()` returns `"검색가능한 Searchable"` and `search("Searchable", …)` hits. On the
730-word tracemonkey page the layer is built in 2 ms, the file is 1.2 MB (image included), and
`search("Trace-based")` finds all 3 occurrences.

Picking the font size and position (image px → pt with `s = 72 / dpi`):

* Per **line**, not per word: `font_size_pt = line_height_px * s`, where `line_height_px` is
  `rowAttributes.rowHeight` (ascenders + descenders) for Tesseract or the observation box height for Vision.
  For Hangul lines the box height *is* the em (glyphs fill the em box); for mixed-case Latin the word box is
  ~0.87 em, which is why the example used `box_h * 1.15` when only word boxes were available.
* Baseline: Tesseract gives `line.baseline {x0,y0,x1,y1}` in px (y grows downward) → `baseline_pt = page_h_pt - y * s`
  (interpolate along the line for slanted scans, or just use `y0`). Vision has no baseline; use
  `box.bottom + 0.2 * height` for Latin, `box.bottom + 0.1 * height` for Hangul.
* Width: horizontal scale `sx = box_w_pt / natural_w_pt` per word (so search highlights land on the printed
  word); clamp `sx` to `[0.5, 2.0]` to avoid absurd runs on mis-segmented words.
* Space handling per gotcha 4; skip words with confidence < 30 and empty/whitespace tokens (4 of 734 skipped).
* Fonts: Helvetica for words entirely in Latin-1, the bundled Hangul font otherwise (one font object each per
  document).

---

## 6. Architecture

```
            ┌───────────── Rust (engine thread, pdfium) ─────────────┐
 open PDF → │ render page i at 300 DPI (12–21 ms) → BGRA/gray8 bytes │
            └───────┬───────────────────────────────┬────────────────┘
       macOS        │                               │  Windows (and macOS fallback)
   Vision (Rust     ▼                               ▼  shared-memory / seepdf:// image → WebView
   OCR thread)  CGImage → VNRecognizeTextRequest    Web Worker pool: tesseract.js scheduler (N ≤ 4)
   448 ms/page  ko-KR,en-US → lines/words          kor+eng, PSM per user option → blocks → words
                    │                               │
                    └──────────► OcrPage { dpi, w_px, h_px, rotate, lines[{ text, bbox, baseline?, words[{ text, bbox, conf }] }] } ◄──┘
                                                    │  invoke("ocr_apply_layer", pages)
                                                    ▼
                          Rust: pixels_to_points → invisible text objects (Manual regen) → save
```

* **Engine selection**: `cfg(target_os = "macos")` → Vision if `supportedRecognitionLanguagesAndReturnError`
  contains the requested language, else tesseract.js; Windows → tesseract.js (later: Windows OCR when
  `OcrEngine::IsLanguageSupported(ko)`). Same `OcrPage` shape from every engine so the layer builder and the UI
  (progress, review overlay) do not care which engine ran.
* **Rendering DPI**: 300 default. If the page is a single image object, use `clamp(native_dpi, 300, 400)`.
  Vision and Tesseract both want ~30 px x-height; 300 DPI gives that for 9 pt text (the 9 pt line read fine).
* **Image transport to the worker (Tesseract path)**: keep the bitmap out of JSON. Rust returns the page bitmap
  through the existing `seepdf://` custom protocol (already in the CSP) as PNG/gray8 or a raw buffer; the
  frontend turns it into an `ImageBitmap`/`ImageData` and passes it to `worker.recognize`, which accepts
  `ImageData`, `Blob`, canvas or URL. Gray8 PNG at 300 DPI is 0.2–0.9 MB per page.
* **Batch, progress, cancel**: the engine thread renders page `i + N` only when job `i` finishes (bounded
  channel, N = worker count) so memory stays at N bitmaps. Per page emit a Tauri event
  `ocr:progress { page, of, status }`; tesseract.js's `logger` reports `recognizing text` 0→1 for sub-page
  progress. Cancel = set an `AtomicBool` checked between pages on the Rust side and `scheduler.terminate()` on the
  JS side (tesseract.js has no per-job cancel; re-creating workers costs ~100 ms warm). Vision: keep a
  `VNRequest` handle per page and call `cancel()`; `performRequests` returns promptly.
* **Apply**: one `invoke` per document with all `OcrPage`s (or per page for very long scans); pdfium work per page
  is single-digit ms, so it stays on the engine thread. Skip pages that already have text (`page.text()?.len()`
  above a threshold, or existing render-mode-3 objects) unless the user forces re-OCR.
* **Rotated pages**: gotcha 16. Skew/orientation detection is out of scope for v1 (Vision handles slight skew;
  Tesseract's OSD needs the legacy core, which we do not ship).
* **Review UI**: word boxes (px) map 1:1 onto the same 300-DPI render, so the "OCR review" overlay can highlight
  low-confidence words (Tesseract `confidence`, Vision candidate `confidence`) before applying.

Bundle-size impact summary: **+8.4 MB** (tesseract.js runtime + eng + kor) for both platforms, **+~1–1.5 MB**
for the bundled Hangul layer font (to be measured once a font is chosen), **0 bytes** for Vision (system
framework, macOS 13+ for `ko-KR`) and for Windows OCR (OS component, user-installed pack).

---

## 7. Verified pdfium-render 0.9.4 signatures used here

```rust
Pdfium::bind_to_library(path: impl AsRef<Path>) -> Result<Box<dyn PdfiumLibraryBindings>, PdfiumError>   // errors if already bound
Pdfium::new(bindings) -> Pdfium
Pdfium::load_pdf_from_file(&self, path: &(impl AsRef<Path> + ?Sized), password: Option<&str>) -> Result<PdfDocument, PdfiumError>
Pdfium::load_pdf_from_byte_slice<'a>(&'a self, bytes: &'a [u8], password: Option<&str>) -> Result<PdfDocument<'a>, PdfiumError>
Pdfium::create_new_pdf(&self) -> Result<PdfDocument, PdfiumError>
PdfDocument::fonts_mut(&mut self) -> &mut PdfFonts;  PdfFonts::load_true_type_from_file(&mut self, path, is_cid_font: bool) -> Result<PdfFontToken, PdfiumError>
PdfFonts::load_true_type_from_bytes(&mut self, &[u8], is_cid_font: bool) -> Result<PdfFontToken, PdfiumError>;  PdfFonts::helvetica(&mut self) -> PdfFontToken  // PdfFontToken: Copy
PdfDocument::pages_mut(&mut self) -> &mut PdfPages;  PdfPages::create_page_at_end(&mut self, PdfPagePaperSize) -> Result<PdfPage<'a>, PdfiumError>
PdfPagePaperSize::a4();  PdfPagePaperSize::Custom(PdfPoints, PdfPoints);  PdfPoints::new(f32)  (.value: f32)
PdfPage::render_with_config(&self, &PdfRenderConfig) -> Result<PdfBitmap, PdfiumError>;  PdfRenderConfig::new().scale_page_by_factor(f32).render_form_data(bool).render_annotations(bool).set_text_smoothing(bool)
PdfBitmap::as_image(&self) -> Result<image::DynamicImage, PdfiumError>  (RGBA8; feature image_api on by default, image 0.25.10)
PdfBitmap::as_rgba_bytes(&self) -> Vec<u8>;  as_raw_bytes(&self) -> Vec<u8> (BGRA)
PdfPage::text(&self) -> Result<PdfPageText, PdfiumError>;  PdfPageText::all(&self) -> String
PdfPageText::search(&self, &str, &PdfSearchOptions) -> Result<PdfPageTextSearch, PdfiumError>;  .iter(PdfSearchDirection::SearchForward)
PdfPage::width()/height() -> PdfPoints;  rotation(&self) -> Result<PdfPageRenderRotation, PdfiumError>
PdfPage::pixels_to_points(&self, x: Pixels, y: Pixels, &PdfRenderConfig) -> Result<(PdfPoints, PdfPoints), PdfiumError>   // FPDF_DeviceToPage
PdfPage::set_content_regeneration_strategy(&mut self, PdfPageContentRegenerationStrategy);  regenerate_content(&mut self) -> Result<(), PdfiumError>
PdfPageTextObject::new(&PdfDocument, text: impl ToString, font: impl ToPdfFontToken, PdfPoints) -> Result<Self, PdfiumError>
PdfPageTextObject::set_render_mode(&mut self, PdfPageTextRenderMode) -> Result<(), PdfiumError>   // ::Invisible
PdfPageObjectCommon::bounds(&self) -> Result<PdfQuadPoints, PdfiumError>;  set_fill_color(&mut self, PdfColor) -> Result<(), PdfiumError>
transform setters (on text objects): translate(dx: PdfPoints, dy: PdfPoints), scale(sx: f32, sy: f32), rotate_counter_clockwise_degrees(f32), set_matrix(PdfMatrix) -> Result<(), PdfiumError>
PdfPageObjects (via PdfPageObjectsCommon): add_text_object(&mut self, PdfPageTextObject) -> Result<PdfPageObject, PdfiumError>
    create_image_object(&mut self, x, y, &DynamicImage, Option<PdfPoints>, Option<PdfPoints>) -> Result<PdfPageObject, PdfiumError>
PdfDocument::save_to_file(&self, path) / save_to_bytes(&self) -> Result<Vec<u8>, PdfiumError>
PdfiumError::ImageError is a unit variant (map image errors with `map_err(|_| PdfiumError::ImageError)`)
```

tesseract.js 7 (`src/index.d.ts`):

```ts
createWorker(langs?: string | string[] | Lang[], oem?: OEM /* default LSTM_ONLY=1 */, options?: Partial<WorkerOptions>, config?): Promise<Worker>
WorkerOptions { corePath, langPath, cachePath, dataPath, workerPath, cacheMethod, workerBlobURL, gzip, legacyLang, legacyCore, logger, errorHandler }
Worker.setParameters({ tessedit_pageseg_mode: PSM /* string enum, AUTO='3', SINGLE_COLUMN='4', SINGLE_BLOCK='6' */, user_defined_dpi, preserve_interword_spaces, ... })
Worker.recognize(image: ImageLike, options?: { rectangle, rotateAuto, rotateRadians }, output?: { text, blocks, hocr, tsv, ... }): Promise<{ data: Page }>
Page { text, confidence, blocks: Block[] | null };  Line { words, text, confidence, baseline {x0,y0,x1,y1}, rowAttributes {ascenders, descenders, rowHeight}, bbox }
Word { symbols, choices, text, confidence, bbox {x0,y0,x1,y1}, font_name }
createScheduler(); scheduler.addWorker(w); scheduler.addJob('recognize', image, options, output); scheduler.terminate()
```
