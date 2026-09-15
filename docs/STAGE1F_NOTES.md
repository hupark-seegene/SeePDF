# Stage 1 (f) — OCR worker pipeline, notes for the integrator and for (e)

What landed, what was probed, what the next module has to do. Companions: `spikes/ocr.md` (the
measurements this is built on), `IPC_CONTRACT.md` §7.9 + §9, `UI_SPEC.md` §10 + §15.11,
`FEATURES.md` F-21, `WORKPLAN.md` row (f).

```
src/ocr/
  tesseractPool.ts   N tesseract.js workers, offline assets, PSM/lang options, cancel
  normalize.ts       tesseract | Vision → the contract's OcrPage (Hangul spacing, syllable merge)
  ocrJob.ts          page range → /ocr bitmap → recognise → normalise → ocr_apply, progress + cancel
  OcrDialog.tsx/.css the modal sheet of UI_SPEC §10
  dialogState.ts     openOcrDialog() / closeOcrDialog() — the seam with (e)
  probe.ts           the WORKPLAN §6 CSP probe (dead code in a normal build)
  index.ts           the public surface
src/store/jobStore.ts   + update(), localJobId(), registerCanceller(), etaMs()
scripts/prepare-ocr.mjs assembles public/ocr/ (5 files, 8.1 MB), idempotent, offline-capable
scripts/ocr-accuracy.mjs the F-21 CER gate, in Node, against the shipped assets + normaliser
public/ocr/**           gitignored build output
```

---

## 1. First-day probe — tesseract.js under the production CSP: **PASS**

The question (WORKPLAN §6, ARCHITECTURE §11): the CSP is not applied in `tauri dev`, so does the
worker + WASM core actually run inside a bundle with
`script-src 'self' 'wasm-unsafe-eval'` / `worker-src 'self' blob:` / `default-src 'self' ipc: …`?

```sh
node scripts/prepare-ocr.mjs --probe
VITE_SEEPDF_MOCK=1 VITE_SEEPDF_OCR_PROBE=1 npx vite build
export PATH="$HOME/.cargo/bin:$PATH"; export CARGO_TARGET_DIR=/Users/veri/Dev/SeePDF/src-tauri/target
npx tauri build --debug --bundles app --config '{"build":{"beforeBuildCommand":""}}'
./src-tauri/target/debug/bundle/macos/SeePDF.app/Contents/MacOS/seepdf
```

Result, read off the panel the probe renders (screenshot archived at
`fixtures/out/ocr/stage1f-csp-probe.png`; the same object is on `window.__OCR_PROBE__`):

```json
{ "ok": true, "origin": "tauri://localhost", "simd": true,
  "core": "tesseract-core-simd-lstm.wasm.js",
  "workerUrl": "tauri://localhost/ocr/worker.min.js",
  "langUrl":  "tauri://localhost/ocr/tessdata",
  "cspViolations": [],
  "fetchMs": 6, "imageBytes": 176415, "initAndRecognizeMs": 1139,
  "rawWords": 170, "words": 90, "lines": 10, "meanConfidence": 92,
  "cer": 0.03125, "hangulCer": 0.0116 }
```

* **No CSP change is needed.** `tauri.conf.json` as it stands is sufficient; do not touch it for OCR.
* The probe listens for `securitypolicyviolation` on the document for the whole run and reported
  **zero** violations — worker spawn, `importScripts` of the 3.7 MB core, `WebAssembly.instantiate`,
  and the worker's own `fetch` of `tessdata/*.traineddata.gz` all pass.
* The fallback the workplan asked us to prove is in fact the **shipped configuration**:
  `workerBlobURL: false` + a same-origin `new Worker('/ocr/worker.min.js')`. Nothing we do needs
  `blob:` in `worker-src` — that allowance could be dropped from the CSP later if the integrator
  wants a tighter policy, but it is harmless.
* `corePath` is passed as one **file**, not a directory. Left to itself the worker runs
  `wasm-feature-detect` and asks for `tesseract-core-relaxedsimd-lstm.wasm.js` on Chromium
  (WebView2), which we deliberately do not ship — that would have been a Windows-only 404 nobody
  would have found until the Windows CI job. The SIMD check now happens on the main thread
  (`hasWasmSimd()`, one `WebAssembly.validate`, no import).
* Accuracy inside the bundle is identical to Node (Hangul CER 1.16 %), so the WebView core and the
  Node core produce the same result on the same models.

Re-running the probe is one command away and it is worth repeating on Windows during Stage 2; the
Windows job is the only untested half of this (see §7).

---

## 2. Offline assets — `scripts/prepare-ocr.mjs`

`npm run prepare:ocr` (also chained into `postinstall` after `fetch-pdfium`). Idempotent: a second
run copies nothing. `--force` rebuilds, `--quiet` is for postinstall, `--with-fallback-core` adds the
non-SIMD core, `--probe` stages the self-test fixture.

| File | Size | Why |
|---|---|---|
| `public/ocr/worker.min.js` | 0.11 MB | tesseract.js worker entry, served same-origin |
| `public/ocr/tesseract-core-simd-lstm.wasm.js` | 3.72 MB | LSTM-only core, WASM base64-embedded — no second fetch, no `.wasm` MIME question |
| `public/ocr/tessdata/eng.traineddata.gz` | 2.80 MB | `4.0.0_best_int` |
| `public/ocr/tessdata/kor.traineddata.gz` | 1.49 MB | `4.0.0_best_int` |
| `public/ocr/LICENSE.txt` | 0.7 kB | Apache-2.0 notices for all of the above |
| **total** | **8.11 MB** | WORKPLAN §5 target ≈ 8.4 MB ✓ |

Sources, in order: the spike cache (`fixtures/out/ocr/tessdata-cache/best_int/*.traineddata`,
gzipped here), then jsdelivr `@tesseract.js-data/<lang>/4.0.0_best_int`, then GitHub `tessdata_best`
raw. **Both paths were exercised**: the download fallback produces 2.82 / 1.50 MB files that score
identically (Hangul CER 1.16 %). Never the plain `4.0.0` (legacy+LSTM) files — 90 % CER with the
LSTM-only core (spike §4.5). The script refuses to finish if a `.traineddata.gz` does not inflate to
something ≥ 1 MB, so a 404 page can never masquerade as a model.

`public/ocr/` is gitignored (one line added to `.gitignore`).

---

## 3. Accuracy — `node scripts/ocr-accuracy.mjs`

Scores `fixtures/out/ocr/korean-300dpi.png` against `korean.txt` using the traineddata in
`public/ocr/` **and** the shipped `src/ocr/normalize.ts` (loaded straight into Node — that file has
no runtime imports on purpose). Exit code 1 when the F-21 budget is missed, so CI can gate on it.

| PSM | CER (raw) | Hangul CER | word recall / precision | recognise |
|---|---|---|---|---|
| **4 `column` (default)** | 2.71 % | **1.16 %** | 95.6 / 94.6 % | 1014 ms |
| 6 `block` | 2.08 % | **0.00 %** | 95.6 / 95.6 % | 1095 ms |
| 3 `auto` | 12.29 % | 23.84 % | 83.5 / 96.2 % | 1150 ms |

**F-21 gate met: Hangul CER 1.16 % ≤ 3 % at PSM 4.** The numbers reproduce the spike's table row for
row, including PSM 3 dropping a whole paragraph (spike §4.1) — that is why the dialog's 레이아웃
default is 단일 단 (PSM 4) and not 자동, a deliberate deviation from UI_SPEC §10 in favour of the
measured result. 자동 and 단일 블록 are both still offered under 고급.

Normalisation on the same page: **170 raw words → 90 words** (the ground truth has 91), and the
normalised text scores the same 1.16 % Hangul CER. Its CER against the full ground truth is 3.33 %
rather than 2.71 % because the normaliser drops words below confidence 30 (spike §5) — that is a
better text *layer* (word precision rises to 96.7 %) even though it is a slightly worse transcript.

---

## 4. Throughput and memory (measured here, Node, Apple M5 / 10 cores, Korean A4 @ 300 DPI, PSM 4)

| workers | ms / page | peak process RSS |
|---|---|---|
| 1 | 1076 | 245 MB |
| 2 | 675 | 333 MB |
| 3 | 374 | 436 MB |
| 4 | 384 | **539 MB** |

Warm init is 116–157 ms for the whole pool.

**Gap against the DoD**: "four workers stay under ~500 MB RSS" — four workers peak at **539 MB**
(better than the spike's 582 MB, still over the line). Three workers are just as fast here and peak
at 436 MB. The default is `min(4, floor(cores / 2))`, so a 4-core laptop already gets 2 workers
(333 MB) and only an 8-core-or-better machine reaches 4. If the ~500 MB budget is hard, change
`defaultWorkerCount` to cap at 3 — it costs nothing measurable. The pool is torn down at the end of
every job (warm init is 100–150 ms), so this is peak-during-OCR, not steady state.

---

## 5. What (e) has to do — the whole integration

```tsx
// dialog host
const OcrDialog = lazy(() => import("../ocr").then((m) => ({ default: m.OcrDialog })));
<Suspense fallback={null}><OcrDialog /></Suspense>

// useCommands.ts, for `tools.ocr` and the ⋯ overflow menu
void import("../ocr").then((m) => m.openOcrDialog());
```

`OcrDialog` renders `null` until `openOcrDialog()` runs, so mounting it costs nothing; lazy-loading
it keeps tesseract.js (≈ 27 kB gz) out of the entry chunk. Optional context:

```ts
openOcrDialog({
  docId, docGeneration, pageCount, pages,   // default: read from docStore
  currentPage,                              // default: viewStore.currentPage
  selectedPages,                            // preselects 페이지 범위 from the organizer's selection
});
```

Also exported from `src/ocr`: `closeOcrDialog()`, `useOcrDialogOpen()`, and — for anything that wants
OCR without the sheet (batch OCR is P1-7) — `runOcrJob()`, `parsePageRange()`, `formatPageRange()`,
`estimateSeconds()`, `TesseractPool`, `normalizeTesseract`, `normalizeVision`.

The dialog puts its job in `jobStore`, so the status-bar progress slot animates with no extra wiring.

### `jobStore` additions (shared file, (f) owns it)

* `update(id, patch)` — for a job the frontend drives itself.
* `localJobId()` — a **negative** id, so a frontend job can never collide with an engine `JobId`.
* `registerCanceller(id, fn)` — `cancel(id)` runs `fn` first and only calls `cancel_job` for
  non-negative ids. The status bar's × keeps working for both kinds.
* `etaMs(job)` — remaining time from the mean time per unit so far, `null` until one unit is done.

Nothing existing changed shape; `apply()`, `start()`, `cancel()` and `clearFinished()` behave as before.

---

## 6. Design decisions worth knowing

* **One `ocr_apply` per page, not one per document.** F-21 wants "cancelling at page 3 of 14 leaves
  the document unchanged [after page 3]"; a single batch call would be all-or-nothing. Each call is
  its own `registry::mutate`, therefore its own undo step — the dialog's 실행 취소 pops one per
  applied page.
* **Batches of `workers` pages.** Every `ocr_apply` bumps `docGeneration`, and `/ocr` answers `410
  Gone` for an older `gen` (§9). So a batch is fetched at the current generation, recognised in
  parallel, then applied in order, and the generation is re-read from each `DocInfo`. Prefetching
  across an apply boundary would 410. There is a test for this.
* **Pixels, not points, cross the seam.** `OcrPage` carries image pixels + `dpi` + `rotation` per
  §7.9 and the engine calls `pixels_to_points`. `normalize.ts` also exports
  `devicePixelToPdfPoint` / `imageBoxToPdfRect` — the exact inverse of the contract's `pageToDevice`
  for all four rotations, unit-tested — for the future review overlay and for cross-checking (b).
  `ocrJob` additionally warns when the `/ocr` bitmap is not within 2 % of `widthPt × dpi / 72`.
* **`kor` is never sent alone.** Turning off English in the dialog still sends `kor+eng`: `kor` alone
  reads English as digits (spike §4.2, `SeePDF` → `5660마`). There is a test.
* **Hangul spacing** is rebuilt by walking `line.text` with the word list and merging every run the
  line text does not separate, with a horizontal-gap fallback when the two disagree. Merged words get
  a length-weighted confidence and the union box.
* **No new npm dependencies.** `tesseract.js` was already in `package.json`; the only `package.json`
  edit is the `prepare:ocr` script plus its postinstall chain. `@testing-library/user-event` is *not*
  installed, so the dialog test uses `fireEvent` like `src/app/shell.test.tsx`.
* **The probe is dead code in a normal build.** The single entry point is
  `if (import.meta.env.VITE_SEEPDF_OCR_PROBE === "1")` at the bottom of `jobStore.ts`, which folds to
  `false` and makes rolldown drop the module *and* its chunk. Verified: a plain `npx vite build`
  emits no `probe-*.js`. Keep it a direct comparison — anything cleverer defeats the folding and
  ships a dead 27 kB chunk.

---

## 7. Integration requests (none of these are files I own)

1. **`scripts/check-bundle-size.mjs` will fail CI once `public/ocr/` exists.** Its own comment says
   "fonts and OCR data are counted elsewhere", but `walk(DIST)` counts them: with the OCR bundle the
   gate reports `dist 8746 kB > 2048 kB`. The `frontend` job survives only because it installs with
   `npm ci --ignore-scripts`; the **`build` matrix job runs `npm ci` and then the gate, so it will go
   red**. One-line fix for the owner:

   ```js
   const files = walk(DIST).filter((p) => !/^(ocr|fonts)[\\/]/.test(relative(DIST, p)));
   ```

   Excluding `dist/ocr/**` (and `dist/fonts/**`, for the Pretendard subset) brings the total from
   8747 kB to **410 kB**. Reporting them on a separate line would be nicer than hiding them entirely.
   The critical-path budget is untouched: OCR adds **0 kB** to the entry chunk — the dialog, the
   pool and tesseract.js are all behind a dynamic `import("../ocr")` that (e) owns.
2. **macOS 12 has no WASM SIMD.** `tauri.conf.json` sets `bundle.macOS.minimumSystemVersion: 12.0`, but
   fixed-width WASM SIMD arrived in Safari 16.4 / macOS 13. `hasWasmSimd()` falls back to
   `tesseract-core-lstm.wasm.js`, which `prepare-ocr.mjs` only ships with `--with-fallback-core`
   (+3.90 MB). Pick one: bump `minimumSystemVersion` to 13.0, or add `--with-fallback-core` to the
   postinstall and accept a 12.0 MB OCR bundle. Until then, OCR on macOS 12 fails with a 404 on the
   core (the dialog shows `ocr.failed`). Not decided by me — it is a product/packaging call.
3. **CI needs network for the traineddata** when `npm ci` runs postinstall and `fixtures/out/` is
   absent (it is gitignored). jsdelivr is reachable from GitHub runners; if you would rather not
   depend on it, cache `public/ocr/tessdata` with `actions/cache` keyed on the tesseract.js version.
4. **`ocr_capabilities` is not consulted yet.** The dialog assumes `tesseract`. When (b) returns a
   real engine list (and P1 adds `vision`), the 인식 엔진 row of UI_SPEC §10 belongs in `OcrDialog`
   and the engine choice in `runOcrJob`. `normalizeVision` is already written against the
   `VisionResult` shape the native side should emit.
5. **`src/vite-env.d.ts`** (Stage 0's file) could declare `VITE_SEEPDF_OCR_PROBE?: string` next to
   `VITE_SEEPDF_MOCK`. Not required — it typechecks through the `vite/client` index signature.

---

## 8. Tests

```sh
npx vitest run src/ocr          # 56 tests, 4 files
node scripts/ocr-accuracy.mjs   # Hangul CER gate (exit 1 on regression)
node scripts/prepare-ocr.mjs && ls -la public/ocr
```

* `normalize.test.ts` (17) — Hangul spacing from `line.text`, syllable merge, the geometric
  fallback, confidence weighting and filtering, absolute vs box-relative baselines, reading order,
  the Vision mapping (normalised bottom-left → top-left pixels, word splitting, ordering, shape
  parity with the tesseract path), and `devicePixelToPdfPoint` for all four rotations.
* `ocrJob.test.ts` (19) — `ocr.range.parse` (1-based → 0-based, reversed ranges, dedupe, every
  rejection case, round trip through `formatPageRange`), `ocr.skipText`, `ocr.cancel` (cancel at page
  3 of 14 applies exactly 3 and fetches exactly 3 bitmaps; a cancel during `ocr_apply` reaches
  `cancel_job`; an already-aborted signal does nothing; an aborted bitmap fetch), one `ocr_apply` per
  page with the contract's `OcrPage`, the generation walk, error → i18n key, progress into `jobStore`.
* `tesseractPool.test.ts` (11) — offline asset paths, `workerBlobURL: false`, `OEM.LSTM_ONLY`,
  `{ blocks: true }`, PSM mapping, the worker cap, `Uint8Array` → PNG blob, cancel kills exactly the
  busy worker, language change re-initialises instead of respawning.
* `OcrDialog.test.tsx` (9) — renders nothing until opened, defaults, `ocr.range.invalid` blocking
  시작, the options that reach `runOcrJob`, `kor+eng` enforcement, the progress view with a live
  취소, the result bar and 실행 취소, `ocr.noImagePages`, the 고급 layout select.

Two bugs the tests found and that are fixed: the pool spawned N workers for a cap of 2 when N
recognitions started concurrently (`slots.length` was checked before any `createWorker` resolved),
and a whitespace-only tesseract token was merged into its neighbour and left a trailing space inside
a word.

`npx vitest run` (the whole suite) currently shows 2 failures in `src/viewer/layout.test.ts` and
`src/app/shell.test.tsx` — files owned by (c), unrelated to this module; `src/ocr` is green and
`npx tsc --noEmit` is clean.

---

## 9. Gaps / not done

1. **`ocr.option.autoRotate` (페이지 회전 자동 감지) is not implemented and not shown.** Tesseract's
   orientation detection needs the legacy core, which we do not ship (spike §6). The page's own
   `/Rotate` is honoured — `/ocr` renders the rotated page and `OcrPage.rotation` carries it — but
   skew and upside-down scans are not detected. Add the checkbox when there is something behind it.
2. **Sub-page progress is collected but not shown per page.** `TesseractPool` forwards tesseract's
   `recognizing text` 0→1 logger, but the pool cannot say *which* page a logger event belongs to
   (the callback is per worker, not per job), so `onPageProgress` reports page `-1` and the dialog's
   bar is per page, not per fraction-of-page. Fixable by giving each `recognize` its own logger
   binding; not worth it for a bar that already moves once a second.
3. **No OCR review overlay** (spike §6 "review UI": highlight low-confidence words on the same
   300-DPI render before applying). `OcrPage` boxes map 1:1 onto that render and
   `imageBoxToPdfRect` is there, so this is a UI-only addition when someone wants it.
4. **`ocr_recognize_native` (Vision) is P1.** `normalizeVision` is written and tested against the
   shape from spike §3b, but nothing calls it and no `VisionResult` has ever come off a real
   `VNRecognizedTextObservation` — the field names are my proposal, not a verified wire format.
   Agree them with whoever writes the Rust side.
5. **`ocr.assetsMissing` is in both catalogues but never rendered.** A missing `public/ocr/` surfaces
   today as `ocr.failed` with the engine detail. Wiring the specific key needs a pre-flight HEAD on
   the core file, which costs a round trip on every dialog open; left out deliberately.
6. **Windows is untested.** The probe ran on macOS/WKWebView only. WebView2 is the case the explicit
   `corePath` was written for, and the `/ocr` route origin differs there
   (`http://seepdf.localhost`) — `protocol.ts` handles that, but nobody has watched it happen.
7. **The end-to-end "searchable after `ocr_apply`" acceptance** (F-21: `search("검색가능한")` hits,
   the words render 0 dark pixels) needs (b)'s `ocr_apply` to do something other than reject with
   `unsupported`. Everything on this side is ready and the mock adapter exercises the call path.
