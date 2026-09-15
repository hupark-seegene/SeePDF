# Stage 2b — hardening log

What the hardening pass changed after `STAGE2_INTEGRATION.md` §5 ("What is still open, worst
first"), in that order, and the evidence for each. Everything here was verified against the
**real** app or the real engine — no mock.

Date: 2026-09-16 · host: macOS 26 arm64 (Apple M5, 10 cores, 16 GB), Rust 1.96, Node 24.15,
PDFium 155.0.8057. Screenshots are in `fixtures/out/stage2b/` (gitignored).

| # | Stage 2 item | Status |
|---|---|---|
| 1 | **F-30** — the performance baseline is not recorded | **done** — §1 |
| 2 | Form widgets are drawn twice (**F-20**) | **done** — §2 |
| 3 | The native menu is English only (STAGE0 §4.6) | **done** — §3 |
| 4 | **F-26** — print has no print-only DOM | **done** — §4 |
| 5 | **QA-1** — Finder / `open -a` on the bundled `.app` unverified | **done** — §5 |
| 6 | `registry::mutate` does not restore a half-failed closure (STAGE1A §5.2) | **done** — §6.1 |
| 6 | `invalidate_page` drops the text layer (STAGE1A §5.1) | **done** — §6.2 |
| 6 | 서명 만들기 does not exist (STAGE1D §7.5) | **done** — §6.3 |

Still open at the end of this pass: §8.

---

## 1. F-30 — the performance baseline is real now

`scripts/perf-baseline.mjs` was the Stage 0 skeleton: a table of `null`s. It now **measures**,
from four sources, each where the thing being measured actually lives.

### 1.1 The harness

| source | what it drives | why there |
|---|---|---|
| `engine` | `cd src-tauri && cargo run --release --example perf_bench` — **new file**. Real engine thread, real bundled PDFium, no window. | These rows do not need the UI, and a headless `--release` measurement is both faster and far less noisy than driving a debug app. It calls the same `tiles::render` the `seepdf://` handler calls and bypasses the encoded-PNG cache, so every sample is a real render. |
| `app` | `scripts/dev-bridge.mjs` + `src/dev/testHook.ts` against a running `npm run tauri dev`. Two **new ops**: `openTimed` (clears `window.__seepdfFirstPaint`, starts the clock, opens the file, waits for the first `<img>` to land) and `scrollThrough` (walks the document, then `engine_stats` reports RSS). | "Open → first painted pixel" and "RSS after scrolling 500 pages" only exist in the UI. Skipped, and marked skipped, when the bridge does not answer. |
| `ocr` | `scripts/ocr-accuracy.mjs --json` — tesseract.js in Node on `fixtures/out/ocr/korean-300dpi.png`, the traineddata from `public/ocr/tessdata`, PSM 4. | Exactly the machinery the brief asked to reuse; it already reports `recognizeMs`, `initMs` and the Hangul CER. |
| `bundle` | `scripts/check-bundle-size.mjs --json`. | Unchanged from Stage 0. |

Output is `docs/perf/baseline.md` (the table, with units, date and machine) **and**
`docs/perf/baseline.json` (the same numbers, machine-readable, which is what `--check`
compares against).

### 1.2 The numbers, against the FEATURES / ARCHITECTURE §13 targets

Every measured row is inside its budget except one. Highlights, full table in
`docs/perf/baseline.md`:

| row | target | measured |
|---|---|---|
| open 14 p → first page painted (app) | ≤ 250 ms | **62 ms** |
| open 500 p → first page painted (app) | ≤ 400 ms | **26 ms** |
| whole-page render A4 @2× | ≤ 30 / 120 ms p50/p95 | **3.76 / 9.14 ms** |
| 512 px tile @2× | ≤ 30 / 120 ms | **0.48 / 2.31 ms** |
| 512 px tile @4× | ≤ 30 / 120 ms | **0.34 / 1.63 ms** |
| thumbnail, 240 px | ≤ 36 ms | **0.98 ms** p50 |
| text layer, one page | ≤ 16 ms | **0.91 ms** p50 |
| search "monkey" over 14 pages, cold layers | ≤ 2.5 s | **16.4 ms**, **62 hits** (the number F-07 pins) |
| save round trip (serialise + reopen, ≈1 MB) | ≤ 600 ms | **17.7 ms** |
| OCR one A4 page @300 DPI kor+eng | ≤ 2 s | **1 030 ms**, Hangul CER **1.16 %** |
| engine RSS, 500 p open + 40 pages rendered | ≤ 400 MB | **63.3 MB** |
| **app RSS, 500 p open + scrolled** | ≤ 400 MB | **406.8 MB** — ⚠️ **miss** |

**The one miss.** `app.rss.500p.mb` is **406.8 MB** against a 400 MB budget — 1.7 % over, and
it varies between runs (450.7 MB on an earlier recording, 395.8 MB on another). It is measured
against `npm run tauri dev`, i.e. a **debug** binary with an unoptimised engine and a
devtools-enabled WKWebView; the *engine*'s own RSS on the same document is 63.3 MB, so the rest
is the debug webview. The row is kept in the table as a miss rather than excused, because the
honest statement is "not measured on a release build yet". Re-measuring it needs a bridge in a
release build, which by design does not exist (`src/dev/**` is `import.meta.env.DEV` only).
Tracked in §8.

### 1.3 The CI gate — which rows, and why not the others

`node scripts/perf-baseline.mjs --check` re-measures and compares against
`docs/perf/baseline.json` with a **±20 %** tolerance. It fails only on the rows marked **gate**:
`search.tracemonkey.hits`, `save.bytes`, `doc.*.pages`, `bundle.criticalGz`, `bundle.dist`.
Every wall-clock row is **informational** and printed with its delta.

That split is not timidity, it is a measurement. One `--check` run on the recording machine
itself, minutes after recording:

```
[perf] ok    bundle.criticalGz          105.8 →   105.8 kB  +0.0 %
[perf] ok    bundle.dist                597.3 →   597.3 kB  +0.0 %
[perf] info  open.tracemonkey.ms         36.3 →    40.9 ms  +12.7 %
[perf] info  open.500p.ms                44.8 →    10.3 ms  -77.0 %   <—
[perf] info  render.500p.firstPage.ms    0.78 →    0.55 ms  -29.5 %   <—
[perf] info  render.tile.4x.p50.ms       0.34 →    0.34 ms   +0.0 %
[perf] ok    search.tracemonkey.hits       62 →        62    +0.0 %
[perf] ok    save.bytes                996699 →    996699    +0.0 %
[perf] ok — every gated row is within tolerance
```

`open.500p.ms` moved **−77 %** and `render.500p.firstPage.ms` **−30 %** on an idle machine,
minutes apart, purely from OS page-cache state (the same row swung **+318 %** on an earlier
pair of runs). A ±20 % band on a shared GitHub runner around a 0.34 ms tile would be a coin
flip; gating it would train everyone to ignore a red build. The gated rows are the ones where
±20 % genuinely means "nothing moved", and a 3× timing regression is still obvious in the log.

`.github/workflows/ci.yml` runs the step on **macos-arm64 only** (the baseline was recorded
there; comparing a timing or an RSS across architectures says nothing) with `--skip-app`
(needs a window) and `--skip-ocr` (needs the generated fixtures).

### 1.4 A CI bug the harness uncovered: the bundle gate was measuring a debug build

Wiring the perf step in made `check-bundle-size.mjs` run twice and the second run failed with
`critical path 172.3 kB gz > 120` — on a tree that ships **105.8**. Cause:
`npm run tauri build -- --debug` sets `TAURI_ENV_DEBUG`, and `vite.config.ts` keys `minify` and
`sourcemap` off it, so it leaves an unminified, source-mapped `dist/` behind (3 616 kB, 29
`.map` files). CI's `build` job ran the size gate **immediately after** that step, so the gate
had been measuring the debug artifact all along and would have failed on the next run.

Three fixes, all small:

* `check-bundle-size.mjs` detects a source-mapped `dist/` and exits **2** with
  "`dist` is a **debug** build (29 sourcemaps, not minified) … run `npm run build`", instead of
  reporting a size nobody can act on;
* `ci.yml` rebuilds the production frontend between `tauri build --debug` and the gate;
* `perf-baseline.mjs` treats a failing size check as "bundle rows not measured" rather than
  crashing with a stack trace.

---

## 2. F-20 — a form field is rendered exactly once

### 2.1 What was wrong

In 양식 mode PDFium drew the widget (value text, field wash, pushbutton caption) into the page
bitmap **and** the HTML overlay drew an `<input>` with the same value on top, offset by a pixel
or two. Visible on `s2-07.png`'s two filled fields and on the *Réinitialiser* button.

### 2.2 The fix: a `forms=0` render

`seepdf://tile` and `seepdf://page` take a new `forms` parameter (default `1`), part of the
`TileKey`, the cache key and the `ETag`. `forms=0` makes the engine skip `FPDF_FFLDraw`.
PDFium's own contract for the remaining flag set is decisive — *"with the `FPDF_ANNOT` flag it
renders all annotations that do not require user-interaction, which are **all annotations
except widget and popup annotations**"* — so nothing of the AcroForm is left in the bitmap and
the overlay is the single renderer.

`annot/bridge.tsx` asks for `forms=0` exactly while the overlay is mounted
(`mode === "form" && info.hasForm`) and the pages are re-requested with the widgets back on
when it unmounts, so a reader always sees the document as the file describes it.

**Why not `render_form_data(false)`.** pdfium-render's own switch for this routes the call to
`FPDF_RenderPageBitmapWithMatrix`, which has no `start_x`/`start_y` and therefore cannot render
a 512 px tile of a larger page — tiles are the thing SeePDF renders most. The new
`engine/raw/render.rs` makes the FFI call directly with exactly the arguments pdfium-render
would have passed, minus `FPDF_FFLDraw`: `FPDFBitmap_CreateEx` over a Rust-owned buffer,
`FPDFBitmap_FillRect` with the same clear colour, `FPDF_RenderPageBitmap`. No change to the
vendored fork was needed.

The overlay's pushbutton/signature boxes stopped printing `field.name`: with the widget
suppressed, "B.Reset" and "link" were being drawn on top of the page's own text. The name is
now the tooltip and the 양식 inspector's job; the box still marks where the widget is.

### 2.3 Evidence

* `cargo test form_forms_flag_suppresses_only_the_widgets` — fills `A.NOM` with
  `박현우 테스트`, renders page 0 of `160F-2019.pdf` at 2× both ways and walks all 2 002 770
  pixels: **0** pixels differ outside any widget rectangle, the field's interior has **>100**
  ink pixels with `forms=1` and **exactly 0** with `forms=0`.
* `cargo test form_forms_flag_tiles_match_the_full_render` — tile (1,1) at 4× with `forms=0` is
  the matching crop of the whole-page render, **byte for byte**, which is what proves the
  hand-rolled origin handling.
* `vitest src/ipc/protocol.test.ts` — `forms` is absent by default and `0` only when asked.
* `cargo test` — the whole `form_*` suite (8 tests) still green.
* Screenshots, live app, `160F-2019.pdf` with both Korean fields filled:
  `fixtures/out/stage2b/form-100.png` (100 %) and `form-200.png` (200 %). One rendering of each
  field, values sitting inside their boxes, aligned with the page's ruled lines.
* Save → close → reopen through the app: `listFormFields` reads back
  `A.NOM = '박현우 테스트'`, `A.PRENOM = 'Raw FORM 입력'` from
  `fixtures/out/stage2b/form-filled.pdf`.

### 2.4 Known trade-off

While the overlay is mounted, a widget's **own** appearance is not drawn — including a
pushbutton's caption. 필드 강조 표시 (on by default) gives every field a tinted box in its
place, and leaving 양식 mode restores the document's own look. The alternative (an overlay that
is transparent until focus) keeps the caption but needs an opaque cover over the focused field,
which is wrong on a coloured widget; suppression was chosen because it makes "exactly one
rendering" true for every field type rather than for most of them.

---

## 3. The native macOS menu follows the app language

`src-tauri/src/app/menu.rs` builds every label from a `(key, 한국어, English)` table that is
`UI_SPEC.md` §15.2 verbatim — submenu titles, the custom items, and the predefined ones
(About / Services / Hide / Quit / Full Screen / Minimize / Zoom / Close, and Cut/Copy/Paste,
which stay predefined for the F-06 reasons of `STAGE2_INTEGRATION.md` §2.1 but now carry our
labels). Korean is the default, English when `Settings.locale` is `en`.

macOS caches the menu, so a language change cannot relabel the existing items:
`app::menu::rebuild` builds a whole new `Menu` and calls `set_menu` **on the main thread**
(`run_on_main_thread`, since `set_settings` is handled on a command thread).
`commands::app::set_settings` calls it only when the locale actually moved — the same command
writes the theme, the zoom default and every tool default, and rebuilding the menu bar on each
of those would flash it.

Evidence:

* `fixtures/out/stage2b/menu-ko.png` — 파일 · 편집 · 보기 · 이동 · 도구 · 윈도우 · 도움말.
* `fixtures/out/stage2b/menu-ko-file.png` — the 파일 menu open: 열기… ⌘O · 최근 항목 열기 ⇧⌘O ·
  메뉴 지우기 · 닫기 ⌘W · 저장 ⌘S · 다른 이름으로 저장… ⇧⌘S · 내보내기… ⌥⌘E · 인쇄… ⌘P ·
  문서 정보 ⌘I · Finder에서 보기.
* `fixtures/out/stage2b/menu-en.png` — the same bar after
  `set_settings({locale: "en"})` through the dev bridge, **with no restart**: File · Edit ·
  View · Go · Tools · Window · Help. The log line is
  `INFO main seepdf_lib::app::menu: menu bar rebuilt locale=En`.
* `cargo test app::menu::tests::menu_labels_cover_every_item` — every id in `MENU_IDS` plus
  every submenu / predefined label `build` asks for has a non-empty Korean **and** English
  string, so a missing row cannot ship as a blank menu title.
* `cargo test app::menu::tests::korean_is_the_default_menu_language`.

---

## 4. F-26 — printing the document, not the app

### 4.1 The print-only DOM

`src/print/` is new: `printStore.ts` (the in-flight job), `PrintRoot.tsx` (one full-width
`<img>` per requested page, straight off the `seepdf://page` route at **150 DPI**, i.e.
`sk = round(150 / 72 × 100) = 208`) and `print.css`. `App.tsx` mounts it as a **sibling** of
`.app-shell` and only while a job is in flight. `@media print` hides `#root > .app-shell`,
gives `@page { size: auto; margin: 0 }` and breaks after every `.print-page`.

`flows.runPrint(pages, method)` takes a method; the 인쇄 dialog offers both:

* **시스템 인쇄 대화상자** (default) — fill the print store, wait for every page image, call
  `window.print()`. This is what F-26 asks for: ⌘P opens the system print dialog showing the
  pages.
* **PDF 앱에서 열기** — the Stage 2 path, kept as the alternative: `print_prepare` writes a
  flattened temp file (chosen pages, annotations and filled form values baked in) and the OS
  handler opens it.

### 4.2 Two things that had to be right

Both were found by looking at the actual macOS print sheet, and both would have shipped as "a
blank page" otherwise:

1. **The print root cannot be `display: none` on screen.** A hidden subtree is not painted, and
   WebKit's print snapshot produced one blank sheet even though every `<img>` had fired `load`.
   It is parked off-canvas (`position: fixed; left: -20000px; opacity: .01`) instead, and
   `@media print` puts it back in flow.
2. **`window.print()` returns before WKWebView has laid the panel out.** Clearing the job in a
   `finally` unmounted the page images while the sheet was still rendering them. The job is now
   cleared on `afterprint` (or a 3-minute timeout, for a webview that never fires it).

### 4.3 Evidence

* `fixtures/out/stage2b/print-dialog.png` — 인쇄 범위 + the new 인쇄 방법 selector, Korean.
* `fixtures/out/stage2b/print-preview.png` — the macOS print sheet after
  `runPrint([0,1,2], "document")` on `tracemonkey.pdf`: **"All 3 Pages"**, **"Page 1 of 3"**,
  and the preview shows the paper's title page and page 2 — no toolbar, no sidebar, no canvas
  element. (Compare `s2-16.png` from Stage 2: the app chrome on one sheet.)
* `vitest src/print/print.test.tsx` — 5 tests: one image per requested page, `sk = 208`,
  `window.print()` only after every image settles, an image that 410s still settles, and the
  images stay mounted until `afterprint`.

---

## 5. QA-1 — Finder / `open -a` on the bundled `.app`

`npm run tauri build -- --debug` → `src-tauri/target/debug/bundle/macos/SeePDF.app` +
`SeePDF_0.1.0_aarch64.dmg`.

* `open -a "$PWD/…/SeePDF.app" "$PWD/fixtures/tracemonkey.pdf"` opens the document through the
  `RunEvent::Opened` path — `fixtures/out/stage2b/qa1-finder-open.png`: page 1 painted, 14
  pages in the rail, Korean menu bar.
* A **second** `open -a … fixtures/rotation.pdf` while it is running is handled by the **same
  process** (pid 14618 before and after; `ps` shows exactly one
  `SeePDF.app/Contents/MacOS/seepdf`) and the title bar becomes `rotation.pdf` —
  `fixtures/out/stage2b/qa1-second-open.png`. macOS routes a document open to the running
  instance of a bundled app, so **`tauri-plugin-single-instance` is not needed on macOS**. It
  *is* available on the USTC mirror (2.4.4 stable, 3.0.0-alpha.0 latest) if Windows turns out
  to need it — Windows passes the path in `argv`, which spawns a second process, and that is
  still unverified from this host (§8).
* The packaged binary also passes the resource-resolution gate:
  `SeePDF.app/Contents/MacOS/seepdf --smoke fixtures/tracemonkey.pdf` →
  `smoke ok: 14 pages, page 0 rendered 1224x1584 px at 2x, 5087 chars`, exit 0.

---

## 6. The leftovers from Stage 1

### 6.1 `registry::mutate` restores a half-failed closure (STAGE1A §5.2)

`mutate` used to only drop its undo bookkeeping when the closure returned `Err` — and it did
that with `take_undo`, which also pushes a **redo** entry, so a failed edit offered to "redo"
something that never happened. Worse, a closure that mutated and *then* failed (a
three-annotation batch whose third `FPDFPage_CreateAnnot` returns null; `apply_redactions`
losing its verification pass after it has already deleted objects) left the half-edit in the
in-memory document under the **old** generation: every cached tile, the text layer and
`list_annotations` kept describing a document the bytes no longer matched.

Now: `History::push` reports whether it actually stored (it can coalesce),
`History::discard_last_undo` pops without touching the redo stack, and the error path reloads
the document from the pre-edit snapshot with `registry::replace`. The generation is still not
bumped and no `doc-changed` is emitted, so nothing outside the engine ever saw the half-edit.
The reload (≈15 ms for a 1 MB document) only happens on the error path.
`redact::apply_verified` keeps its own byte snapshot; it is belt-and-braces now rather than the
only safety net.

`cargo test registry_mutate_rolls_back_a_half_failed_closure`: one successful annotation, then
a closure that creates two more and fails. After it — 1 annotation (not 3), the same
generation, the same undo depth, redo depth still 0, the document still dirty from the
*successful* edit, and the surviving annotation is the one that was made.

### 6.2 `invalidate_page` keeps the text layer (STAGE1A §5.1)

A PDFium page handle is a parse of the page, not the page itself: dropping it says nothing
about whether the characters changed. `OpenDoc::invalidate_page_handle` drops the handle only,
and the two callers that need exactly that — `ScratchPage::open` (so two handles never straddle
an edit) and `refresh_pages_meta` (so `page()` re-reads `/Rotate` and the crop box) — now use
it. `mutate` stays the one place that decides text validity, via the new
`MutateOpts::keeps_text()`, which the three annotation commands set: an annotation lives in
`/Annots` and draws from its own appearance stream, and `FPDFText_*` only ever reads the page
content stream — even a Korean text box bakes its glyphs into the annotation's `/AP`.

`cargo test registry_annotation_edit_keeps_the_text_layer`: the cached layer (5 087 characters
on tracemonkey page 1) survives an annotation write with the same character count, and an edit
*without* `keeps_text()` still invalidates it. Saved per annotation write: the 5.6 ms
`FPDFText_LoadPage` rebuild the viewer used to pay the next time the user selected text.

### 6.3 서명 만들기 (STAGE1D §7.5)

Arming 서명 opened a file picker, so a user with no signature image could not sign at all — and
F-13 could not be driven live for the same reason. `src/dialogs/SignatureDialog.tsx` is the
missing sheet: a canvas, 지우기, 이미지 선택… (hands over to the existing picker) and 확인.

The drawn strokes are normalised to **unit space** (0…1 of their own bounding box, y-down) plus
the aspect they were drawn at, so the same signature can be placed at any size; `tools/stamp.ts`
maps them into the placement rectangle in PDF user space (y-**up**) and commits
`AnnotSpec { kind: "signature" }` — which the engine already wrote as a real `/Ink` annotation
with `/Subj "SeePDF:Signature"` (`engine::annot::create::ink`). No backend change: this is the
ink → signature path STAGE1D §7.5 pointed at, finally reachable.

Drawing and picking are mutually exclusive (the last thing the user made wins), and the
placement rectangle is 160 pt wide with the drawn aspect preserved rather than a fixed
160 × 56.

`vitest src/dialogs/signature.test.ts` — 7 tests: the bounding-box normalisation (offset does
not survive; a dot and a flat line do not divide by zero), the y-flip into PDF user space, the
aspect-preserving rectangle, that the commit is a `signature` spec and not a `stamp`, and the
drawn ⇄ picked exclusivity.

---

## 7. Gates

| Gate | Command | Result |
|---|---|---|
| Rust tests | `cd src-tauri && cargo build --release --tests && cargo test --release` | **pass** — **133 tests in 17 binaries, 0 failed** (Stage 2 had 126 in 16) |
| Typecheck | `npm run typecheck` | **pass** |
| Frontend tests | `npx vitest run` | **pass** — 35 files, 283 tests |
| i18n parity | `node scripts/check-i18n.mjs` | **pass** — ko 447 / en 447 |
| Production build + budget | `npm run build && node scripts/check-bundle-size.mjs` | **pass** — critical path 105.8 kB gz of 120, `dist` 597.3 kB of 2048 |
| Desktop bundle | `npm run tauri build -- --debug` | **pass** — `SeePDF.app` + `SeePDF_0.1.0_aarch64.dmg` |
| Perf gate | `node scripts/perf-baseline.mjs --check --skip-app --skip-ocr` | **pass** — every gated row within ±20 % |

New tests in this pass: `form_forms_flag_suppresses_only_the_widgets`,
`form_forms_flag_tiles_match_the_full_render`, `protocol_forms_parameter_is_its_own_cache_entry`,
`registry_mutate_rolls_back_a_half_failed_closure`,
`registry_annotation_edit_keeps_the_text_layer`, `app::menu::tests::menu_labels_cover_every_item`,
`app::menu::tests::korean_is_the_default_menu_language` (Rust); `src/print/print.test.tsx` (5),
`src/dialogs/signature.test.ts` (7) and the `forms=0` cases in `src/ipc/protocol.test.ts`
(vitest 269 → 283).

---

## 8. Still open

1. **`app.rss.500p.mb` misses its budget (406.8 MB vs 400 MB, and run-to-run it moves between
   ~396 and ~451 MB)** — measured against a debug `tauri dev` binary, which is the only build
   the dev bridge exists in; the engine's own RSS on the same document is 63.3 MB. A
   release-build number needs either a release-safe measurement hook or an external profiler;
   until then the row is an upper bound, not a verdict.
2. **QA-2 (drag & drop onto the window) and everything on Windows** are still unverified from
   this host. `onFileDrop` and `take_pending_opens` are wired; Windows passes the path in
   `argv`, which spawns a second process, so `tauri-plugin-single-instance` (2.4.4, available
   on the USTC mirror) is the likely answer there.
3. **While the 양식 overlay is mounted, a pushbutton's caption is not drawn** — see §2.4.
4. `extract_images` and `SplitMode::byOutline` have no command / no backend (STAGE1B §5.4);
   link annotations cannot be created (STAGE1A §5.3); image signatures report `kind: "stamp"`.
   All unchanged, all P2 in the contract.
5. **macOS 12 has no WASM SIMD**, so OCR 404s on the core there unless `prepare-ocr.mjs` ships
   the fallback core (+3.9 MB) or `minimumSystemVersion` moves to 13.0 (STAGE1F §7.2).
   Unchanged; `README.md` documents 13+ for OCR.
6. **Do not edit `src/**` while a dev-bridge run is in flight** — unchanged from
   `STAGE2_INTEGRATION.md` §5.12. The Stage 2b runs followed it: edit, restart `tauri dev`,
   then drive.

---

## 9. Files changed

**Backend** — new `src/engine/raw/render.rs`, new `examples/perf_bench.rs`;
`src/app/menu.rs` (rewritten around the ko/en table + `rebuild`), `src/commands/app.rs`
(`set_settings` rebuilds the menu), `src/engine/history.rs` (`push` → `bool`,
`discard_last_undo`), `src/engine/registry.rs` (`invalidate_page_handle`,
`MutateOpts::keeps_text`, the rollback), `src/engine/raw/mod.rs`,
`src/engine/render/{cache,tiles}.rs` (`TileKey::forms`), `src/protocol/mod.rs` (`forms=0`),
`src/commands/annots.rs` (`keeps_text()`), `src/lib.rs`;
tests in `tests/{form,registry}.rs`.

**Frontend** — new `src/print/{printStore.ts,PrintRoot.tsx,print.css,print.test.tsx}`, new
`src/dialogs/{SignatureDialog.tsx,signature.test.ts}`; `src/ipc/protocol.ts` (+ its test),
`src/viewer/{Viewer,Scroller}.tsx`, `src/annot/{bridge,AnnotationHost}.tsx`,
`src/forms/FormLayer.tsx`, `src/dialogs/{flows.ts,PrintDialog.tsx,DialogHost.tsx,dialogState.ts,dialogs.css}`,
`src/tools/stamp.ts`, `src/App.tsx`, `src/dev/testHook.ts`, `src/i18n/{ko,en}.json` (+5 keys).

**Tooling / docs** — `scripts/perf-baseline.mjs` (rewritten), `scripts/check-bundle-size.mjs`
(debug-`dist` guard), `.github/workflows/ci.yml`,
`docs/perf/baseline.{md,json}`, `docs/IPC_CONTRACT.md` §9, `docs/FEATURES.md` (P0 status),
this file.
