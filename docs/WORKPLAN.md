# SeePDF — implementation plan (parallel agents, git worktrees)

Companions: `ARCHITECTURE.md` (how), `IPC_CONTRACT.md` (the seam), `FEATURES.md` (what + acceptance),
`UI_SPEC.md` (shell + i18n). This document says **who owns which files, what "done" means, and in which
order branches merge**.

---

## 0. Ground rules

1. **Exclusive file ownership.** A module edits only the directories and files listed in its row. No
   exceptions, no "quick fix" in someone else's file — open an issue for the owner or a PR to the
   integrator.
2. **Frozen files** (integrator only, changed by PR that every owner reviews the same day):
   `src-tauri/src/lib.rs`, `src-tauri/src/main.rs`, `src-tauri/Cargo.toml`, `src-tauri/tauri*.conf.json`,
   `src-tauri/capabilities/*`, `package.json`, `vite.config.ts`, `tsconfig.json`, `src/ipc/types.ts`,
   `src-tauri/src/ipc/types.rs`, `.github/`.
3. Stage 0 registers **every** command of `IPC_CONTRACT.md` in `generate_handler!` with its real
   signature and an `Err(EngineError::unsupported(...))` body, so `lib.rs` never has to change again.
   A Stage-1 agent fills in bodies inside its own `commands/<area>.rs`.
4. **Worktree setup** (each agent):
   ```sh
   git worktree add ../SeePDF-<module> -b feat/<module>
   export PATH="$HOME/.cargo/bin:$PATH"
   export CARGO_TARGET_DIR=/Users/veri/Dev/SeePDF/src-tauri/target   # shared cache; file-lock waits are normal
   cd ../SeePDF-<module> && npm install                              # postinstall fetches libpdfium + OCR assets
   npm run tauri dev -- --no-watch                                   # the tauri watcher restarts on any src-tauri change
   ```
5. Every claim about a pdfium API must match `docs/spikes/*.md`. If a spike marked something
   **unverified**, the owning agent probes it on day 1 and reports before building on it (§6).
6. Release numbers only. `[profile.dev.package."*"] opt-level = 3` is in Stage 0; without it PNG encoding
   is 141–260 ms instead of 2.3 ms and every dev measurement is meaningless.
7. Commit messages end with the attribution lines the session requires; no binaries, no `fixtures/out/`.

---

## Stage 0 — foundation (one agent, everyone waits)

Goal: the contract compiles, the engine opens and renders a document, the shell runs, and the frontend
can be developed against a mock without any backend.

| # | Deliverable | Files (all owned by Stage 0) |
|---|---|---|
| 0.1 | Dependency and config baseline: crates and npm packages of §5, `[patch.crates-io]` for pdfium-render (§4), CSP + capability changes, platform bundle overlays, `[profile.dev.package."*"]` | `src-tauri/Cargo.toml`, `tauri.conf.json`, `tauri.macos.conf.json`, `tauri.windows.conf.json`, `capabilities/default.json`, `package.json`, `vite.config.ts` |
| 0.2 | **Contract types**, both sides, byte-identical in meaning | `src/ipc/types.ts`, `src-tauri/src/ipc/types.rs`, `src-tauri/src/ipc/error.rs` |
| 0.3 | Command registry: every command of the contract, stubbed | `src-tauri/src/lib.rs`, `src-tauri/src/commands/{mod,documents,text,annots,forms,pages,objects,save,export,ocr,security,app}.rs` |
| 0.4 | **Engine thread**: `EngineHandle::{spawn,call,send}`, lanes, priority heap, viewport-generation stale drop, `JobToken` cancellation, `Reply::oneshot` (ported from `src-tauri/src/spike/engine.rs`) | `src-tauri/src/engine/{mod,thread,types,jobs,error,stats}.rs` |
| 0.5 | **Registry**: `DocId`, generations, `PageLru` (FORM_OnAfterLoadPage + `Manual` strategy), drop order, `registry::mutate` + `registry::replace`, snapshot history with disk spill, open/close/outline/permissions/metadata read | `src-tauri/src/engine/{registry,page_lru,history}.rs` |
| 0.6 | **Render + protocol**: geometry (scale ladder, tile grid, `round(pt*s)`), tile/page/thumb/ocr render, encode pool, 64 MiB encoded LRU, `seepdf://` routes with headers and status codes (ported from `spike/protocol.rs`) | `src-tauri/src/engine/render/*`, `src-tauri/src/protocol/*` |
| 0.7 | **Text layer + search**: char/word/line builder, page→device matrix, binary serialisation (§10.1 of the contract), Rust search with streaming | `src-tauri/src/engine/text/*` |
| 0.8 | App plumbing: pdfium path resolution, pending opens (argv + `RunEvent::Opened`), drag-drop, windows, native macOS menu, recents + thumbnails, settings store (ported from `spike/{files,pdfium_path,jobs}.rs`) | `src-tauri/src/app/*` |
| 0.9 | **App shell**: layout (titlebar, mode switcher, tool strip, sidebar chrome, status bar, inspector frame), zustand stores, i18n loader + the full `ko/en` catalogues, theme tokens, keymap table, tool-mode routing | `src/main.tsx`, `src/App.tsx`, `src/app/**`, `src/store/*.ts`, `src/i18n/**`, `src/styles/**`, `src/keys/**` |
| 0.10 | **Mock adapter**: `src/ipc/mock.ts` + fixture JSON + 1×1 PNG tiles, switched by `VITE_SEEPDF_MOCK=1`, so every frontend module runs in plain `vite dev` | `src/ipc/{api,events,protocol,mock}.ts`, `src/test/**` |
| 0.11 | Test scaffolding: Rust test harness (`OnceLock` engine + fixtures), vitest + jsdom config, `scripts/{check-i18n,check-bundle-size,perf-baseline,prepare-ocr}.mjs`, CI workflow with the 3-target matrix and the `--smoke` flag | `src-tauri/tests/common/mod.rs`, `vite.config.ts` test block, `scripts/*`, `.github/workflows/ci.yml` |

**Definition of done (all must pass):**

```sh
cd src-tauri && cargo fmt --check && cargo clippy -- -D warnings && cargo test --release
#   passes: engine_lanes, engine_stale_drop, registry_drop_order, registry_open_all_fixtures,
#           render_tile_matches_crop, render_rotation_sizes, text_layer_tracemonkey, text_matrix_vs_pdfium,
#           search_counts_match_pdfium, history_roundtrip, protocol_routes
npm run typecheck && npx vitest run && node scripts/check-i18n.mjs
npm run tauri dev -- --no-watch -- -- fixtures/tracemonkey.pdf   # opens, scrolls, zooms, selects text, searches
VITE_SEEPDF_MOCK=1 npm run dev                                   # the shell runs with no Rust at all
npm run tauri build -- --debug --bundles app && ./…/SeePDF.app/Contents/MacOS/seepdf --smoke fixtures/tracemonkey.pdf
```

---

## Stage 1 — six modules in parallel

Each row: **owned files** (exclusive) · **consumes** (contract sections it calls) · **provides** ·
**DoD** (the exact commands that must pass).

### (a) Backend — annotations, forms, redaction

* **Owns:** `src-tauri/src/engine/raw/**` (the only `unsafe` in the tree), `src-tauri/src/engine/annot/**`,
  `engine/form/**`, `engine/redact/**`, `src-tauri/src/commands/{annots,forms,security}.rs`,
  `src-tauri/tests/{annot,form,redact}.rs`.
* **Consumes:** `registry::mutate/page`, the text layer (line rects → quads), `IPC_CONTRACT` §7.1, §7.2, §7.5.
* **Provides:** `list_annotations`, `scan_annotations`, `create_annotation`, `update_annotation`,
  `delete_annotations`, `set_annotations_hidden` (P1), `list_form_fields`, `set_form_field_value`,
  `reset_form` (P1), `redact_preview`, `apply_redactions`.
* **First-day probe:** `FORM_SetFocusedAnnot` + `FORM_SelectAllText` + `FORM_ReplaceSelection`
  (unverified) against `160F-2019.pdf`; fall back to the verified click + `FORM_OnChar` loop and say so.
* **DoD:**
  ```sh
  cd src-tauri && cargo test --release annot_ && cargo test --release form_ && cargo test --release redact_
  # must include: annot_markup_roundtrip, annot_quad_order, annot_ink_roundtrip, annot_shapes_roundtrip,
  #   annot_line_subj_roundtrip, annot_textbox_korean, annot_stamp_image_roundtrip,
  #   annot_recolor_after_reopen (child process; the naive path SIGSEGVs), annot_delete_persists,
  #   form_text_value_persists, form_radio_toggle, redact_removes_text_from_saved_file, redact_verify_rollback
  ```
  plus QA-4 recorded: the saved file's annotations are visible in macOS Preview **and** Acrobat.

### (b) Backend — pages, objects, export, save, metadata, OCR layer

* **Owns:** `src-tauri/src/engine/{pages,objects,save,export,fonts}/**`, `engine/ocr/**` (layer only),
  `src-tauri/src/commands/{pages,objects,save,export,ocr}.rs`,
  `src-tauri/tests/{pages,objects,save,export,ocr_layer}.rs`,
  `src-tauri/resources/fonts/**`, `src-tauri/examples/build_hangul_font.rs`.
* **Consumes:** `registry::mutate` (structural flag), `engine::raw` (`move_pages`, `import_pages_by_index`,
  `flatten`, `save_as_copy` flags) — the raw wrappers are provided by (a); agree the signatures on day 1.
* **Provides:** `page_ops`, `extract_pages`, `split_document`, `merge_documents`, the whole object group,
  `save_document(_as)`, `export_*`, `estimate_export`, `print_prepare`, `ocr_page_status`, `ocr_apply`,
  `ocr_capabilities`.
* **DoD:**
  ```sh
  cd src-tauri && cargo test --release pages_ objects_ save_ export_ ocr_layer_
  # must include: pages_move_order, pages_extract_keeps_acroform, pages_duplicate,
  #   pages_insert_from_range_1based, objects_text_inplace, objects_text_probe_subset_font,
  #   objects_text_xobject_refused, objects_add_text_korean, objects_add_image,
  #   save_atomic_abort, save_verify_rejects_truncated, save_keeps_encryption,
  #   export_images_sizes, export_text, export_flatten_normaldisplay,
  #   ocr_layer_apply_searchable (Hangul extracts WITH spaces), ocr_layer_rotation_roundtrip
  ```

### (c) Frontend — viewer

* **Owns:** `src/viewer/**` (Scroller, PageShell, TileManager, layout, geometry, zoom, text/, search/),
  `src/sidebar/{Thumbnails,Outline,SearchPanel}.tsx`, `src/store/{docStore,viewStore}.ts`.
* **Consumes:** `open_document`/`get_document`/`get_outline`, `get_text_layer`, `search_start`,
  `set_viewport`, the `seepdf://` routes, `doc-changed`.
* **Provides:** `PageShell` layer slots and the `pageToDevice` helper that (d) and (e) build on — **frozen
  by day 2 of Stage 1**.
* **DoD:**
  ```sh
  npm run typecheck && npx vitest run src/viewer
  # geometry.rotations (cross-checked against the Rust table), layout.visibleRange, layout.twoPage,
  # zoom.anchor, TileManager.order, TileManager.inflightCap, TextLayer.hitTest, SearchController.merge
  ```
  plus recorded: 60 fps scroll on `gen/500p.pdf`, first paint ≤ 250 ms on tracemonkey, no white gaps.

### (d) Frontend — annotation tools, properties, forms overlay, undo/redo

* **Owns:** `src/tools/**` (ToolController + one file per tool), `src/annot/**` (SVG overlay, handles,
  popovers), `src/forms/**` (HTML input overlay), `src/app/Inspector/**`,
  `src/sidebar/AnnotationList.tsx`, `src/store/annotStore.ts`.
* **Consumes:** (c)'s `PageShell` slots and `pageToDevice`; contract §7.1, §7.2, §7.8.
* **Provides:** the tool-mode contract used by the tool strip (already stubbed in Stage 0).
* **DoD:**
  ```sh
  npm run typecheck && npx vitest run src/tools src/annot
  # one test per tool state machine (down/move/up -> AnnotSpec), inspector patch mapping,
  # undo/redo store reconciliation with a new docGeneration, optimistic ghost removal on tile onload
  ```
  plus manual: create every P0 annotation type, edit its colour after reopening the file, fill
  `160F-2019.pdf` with Korean text through the overlay.

### (e) Frontend — page organizer, dialogs, welcome

* **Owns:** `src/organize/**`, `src/dialogs/**` (Open/Save/Export/Merge/Split/OCR host/Print/Settings/
  Password/Unsaved/DocInfo), `src/welcome/**`, `src/store/{pagesStore,appStore}.ts`,
  `src/app/ContextMenu.tsx`, `src/app/Toasts.tsx`.
* **Consumes:** `page_ops`, `extract_pages`, `split_document`, `merge_documents`, `save_document(_as)`,
  `export_*`, `get_recent`/`update_recent`, `get_settings`/`set_settings`, `take_pending_opens`,
  `open-file`, dialog plugin commands.
* **DoD:**
  ```sh
  npm run typecheck && npx vitest run src/organize src/dialogs src/welcome
  # organize.dnd (drag order -> a single move op), organize.selection, pageRange.parse,
  # dialogs.export.estimate, dialogs.unsaved.flow, welcome.recents
  ```
  plus manual: reorder/delete/rotate/extract on `gen/500p.pdf` at 60 fps, unsaved-changes flow, welcome
  drop of three files → 하나로 합치기.

### (f) OCR worker pipeline

* **Owns:** `src/ocr/**` (`ocrJob.ts`, `tesseractPool.ts`, `normalize.ts`, `OcrDialog.tsx`),
  `src/store/jobStore.ts`, `public/ocr/**`, `scripts/prepare-ocr.mjs`.
* **Consumes:** the `/ocr` protocol route, `ocr_page_status`, `ocr_apply`, `cancel_job`, `JobEvent`.
* **First-day probe (blocking, report before building):** a tesseract.js worker inside a
  `tauri build --debug` bundle with `workerBlobURL: false`, `worker-src 'self' blob:` and
  `script-src 'self' 'wasm-unsafe-eval'` — CSP is **not** applied in `tauri dev`, so this must be tested
  in a bundle. If it fails, the CSP change is a PR to the integrator.
* **DoD:**
  ```sh
  npm run typecheck && npx vitest run src/ocr
  # normalize.tesseract (Hangul spacing rebuilt from line.text, over-segmented syllables merged),
  # normalize.vision (same OcrPage shape), ocr.range.parse, ocr.cancel, ocr.skipText
  node scripts/prepare-ocr.mjs && ls -la public/ocr   # 8.4 MB, five files, no network at runtime
  ```
  plus: the synthetic Korean page reaches Hangul CER ≤ 3 % at PSM 4 and is searchable after `ocr_apply`;
  cancelling at page 3 of 14 leaves the document unchanged; four workers stay under ~500 MB RSS.

---

## Stage 2 — integration, then review

**Integration agent** (owns nothing exclusively; merges and fixes seams):

1. Merge in the order of §3, running the full DoD of every merged module after each merge.
2. Replace the mock adapter with the real one everywhere (`VITE_SEEPDF_MOCK` defaults off), delete
   `src-tauri/src/spike/**` and `src/spike/**`.
3. Wire the seams nobody owns end to end: optimistic annotation ghost ↔ tile `onload`; `doc-changed`
   fan-out to every store; job progress in the status bar; `take_pending_opens` on mount; the native
   menu ids ↔ keymap ids; dirty state ↔ window title ↔ close guard.
4. Smoke script on all three CI targets: open every fixture, scroll, zoom, select, search, annotate,
   fill a form, OCR one page, save, reopen, verify.
5. Fill `docs/perf/baseline.md` from `scripts/perf-baseline.mjs` and turn on the ±20 % CI gate.

**Review / QA agent:** runs `docs/qa/checklist.md` on macOS and Windows (Preview/Acrobat rendering of
saved annotations, Finder double-click, pinch, printing, Korean text with Windows fonts, offline OCR),
reviews every module against `FEATURES.md` acceptance rows, files the P1 backlog in priority order.

---

## 3. Merge order and dependencies

```
Stage 0 ──┬─► (a) annotations/forms/redaction ─┐
          ├─► (b) pages/objects/export/save/ocr┤
          ├─► (c) viewer ──────────────────────┼─► Stage 2 integration ─► QA ─► v1
          ├─► (d) tools/properties/undo ───────┤      (then P1 items)
          ├─► (e) organizer/dialogs/welcome ───┤
          └─► (f) OCR worker pipeline ─────────┘
```

* (d) depends on (c)'s `PageShell` slots + `pageToDevice` (frozen day 2) and on (a)'s commands at
  integration time only — it develops against the mock.
* (e) depends on (b)'s commands at integration time only.
* (f) depends on the `/ocr` route (Stage 0) and (b)'s `ocr_apply`; the worker probe is independent.
* (b) depends on (a)'s `engine::raw` wrappers — signatures agreed on day 1, implemented by (a) first
  (`move_pages`, `import_pages_by_index`, `flatten`, `save_as_copy`).
* **Merge order:** (a) → (b) → (c) → (d) → (e) → (f). Rust first so the frontend merges against a real
  backend; (c) before (d)/(e) because they build on its slots.

---

## 4. The pdfium-render patch (decision + fallback)

**Decision: yes, one patch, and only one.** `[patch.crates-io] pdfium-render = { git =
"https://github.com/hupark-seegene/pdfium-render", rev = "<sha>" }`, branch `seepdf/0.9.4-raw`, adding
five `raw_handle()`/`raw_bindings()` accessors and nothing else (`ARCHITECTURE.md` §1.3). GitHub is
reachable from this network and from CI; the diff is also kept in `docs/pdfium-patch.diff` so the same
patch can be applied as a vendored `path = "vendor/pdfium-render"` if the network ever is not.

**Why the spikes' alternative is not enough:** the spikes offer a second `bind_to_library()` handle, but
`PdfDocument::handle()` is `pub(crate)`, so the raw bindings can only drive a *separate* document. Sharing
edits then means reloading the pdfium-render document from `save_to_bytes()` after every edit
(5 ms/MB — 0.5 s per annotation on a 100 MB scan). Without either, P0 form filling (`FORM_*`) and page
reorder (`FPDF_MovePages`) are impossible, ellipses degrade to rectangles, annotation border width and
safe recolouring are lost, and "remove password" cannot be built.

Also: **drop the `thread_safe` feature** (pdfium types become `!Send`, so the compiler enforces the
engine-thread rule) and **do not enable the `flatten` feature** (it is `FLAT_PRINT`, which deletes
annotations without the Print flag; we call `FPDFPage_Flatten(FLAT_NORMALDISPLAY)` through `engine::raw`).

---

## 5. Dependencies to add

### Rust (`src-tauri/Cargo.toml`)

| Crate | Version | Purpose | Stage |
|---|---|---|---|
| `pdfium-render` | 0.9.4 via `[patch]` fork, `default-features = false`, features `pdfium_7881`, `image_025` | PDF engine; the patch exposes raw handles; `thread_safe` removed on purpose | 0 |
| `uuid` | 1, feature `v4` | annotation `/NM` ids | 0 |
| `ttf-parser` | 0.25 | `cmap` coverage check before embedding a font (pdfium checks nothing and drops glyphs silently) and for advance widths in our own text layout | 1 (b) |
| `lru` | — | **not added**: a `HashMap + VecDeque` is enough for the tile and page caches | — |
| `rayon` | — | **not added**: pdfium is single-threaded; encoding parallelism is `std::thread` | — |
| `lopdf` | 0.45 | **P1 only**: `/Info` metadata, `/PageLabels`, outline editing, `/AcroForm` re-attach after import, and `Document::encrypt` (AES-256) — the named crate for "password protect" | P1 |
| `subsetter` | 0.2 | **dev-dependency only**: `examples/build_hangul_font.rs` produces the committed Hangul subset; nothing subsets at runtime | 1 (b) |
| `objc2`, `objc2-foundation`, `objc2-core-foundation`, `objc2-core-graphics`, `objc2-vision` | 0.6 / 0.3 | **P1, macOS only**: Vision OCR (`ocr_recognize_native`); compile unverified | P1 |
| `windows` | 0.62, feature `Media_Ocr` | **P2**: opportunistic Windows OCR when the Korean pack is installed | P2 |

Already present and kept: `tauri`, `tauri-plugin-{opener,dialog,fs,store,window-state}`, `serde`,
`serde_json`, `png`, `image`, `thiserror`, `parking_lot`, `crossbeam-channel`, `tracing*`.
The `fs` plugin stays as a dependency but its permissions are removed from the capability (§11 of
`ARCHITECTURE.md`).

### npm (`package.json`)

| Package | Purpose |
|---|---|
| `zustand` | state slices (~1 kB); no Redux, no context churn |
| `lucide-react` | icons, per-icon imports, stroke 1.75 |
| `@tauri-apps/plugin-dialog`, `-store`, `-window-state`, `-opener` | typings and helpers (the raw `invoke('plugin:…')` calls already work) |
| `vitest`, `jsdom`, `@testing-library/react` | tests |
| `tesseract.js` | already present (7.0.0) |
| **not added** | `i18next` (a 60-line `t()` covers 405 flat keys), `react-window` (bespoke scroller), any CSS or component framework |

### Assets

| Asset | Where | Note |
|---|---|---|
| Pretendard Variable Korean subset woff2 (~350 kB, OFL) | `public/fonts/` | UI font; loaded off the critical path with a system fallback |
| `SeePDF-Hangul.ttf` (Noto Sans KR or Pretendard subset, ~1–1.5 MB, OFL + licence file) | `src-tauri/resources/fonts/` | embedded into user PDFs for text boxes, added text and the OCR layer; built once by `examples/build_hangul_font.rs` and committed |
| tesseract.js runtime + `eng`/`kor` `4.0.0_best_int` traineddata (8.4 MB) | `public/ocr/` (gitignored, produced by `scripts/prepare-ocr.mjs` at postinstall) | offline OCR; never the `4.0.0` legacy files (90 % CER with the LSTM core) |
| generated fixtures (`outline-labels.pdf`, `encrypted-rc4-40.pdf`, `korean-300dpi`, `gen/500p.pdf`) | `fixtures/gen/` | produced deterministically by scripts; `fixtures/out/` is gitignored |

---

## 6. Unverified items each owner must probe first

| Item | Owner | Fallback if it fails |
|---|---|---|
| `FORM_SetFocusedAnnot` + `FORM_SelectAllText` + `FORM_ReplaceSelection` | (a) | the spike-verified click + per-character `FORM_OnChar` loop |
| tesseract.js worker under the production CSP inside a bundle | (f) | `workerBlobURL: false` + a same-origin worker file; last resort: relax `worker-src` |
| `Cache-Control: immutable` honoured on a custom scheme by WKWebView and WebView2 | Stage 0 | the spike-verified `max-age=0, must-revalidate` + 304 served from the encoded cache |
| `core:webview:allow-print` and `getCurrentWebview().print()` | (e) | `print_prepare` → flattened temp file → OS handler via `tauri-plugin-opener` |
| `objc2-vision` stack compiling with Command Line Tools only | P1 owner | tesseract.js on macOS too (CER 2.1–2.7 %, still shippable) |
| The bundled Hangul font's generated `/ToUnicode` not mapping space → TAB (the AppleGothic bug) | (b) | try the next OFL candidate; `ocr_layer_apply_searchable` is the gate |
| Incremental save / xref-stream behaviour | — | not in v1; full rewrite only |

---

## 7. Milestones

| Milestone | Contents | Gate |
|---|---|---|
| **M0** Foundation | Stage 0 | its DoD block, CI green on 3 targets |
| **M1** Viewer | (c) + Stage 0 backend | open/scroll/zoom/select/search/thumbnails/outline meet the perf rows of `FEATURES.md` F-01…F-07 |
| **M2** Editor | (a) + (d) + (b:pages/save) + (e) | F-08…F-20, F-23 acceptance rows pass, QA-4 recorded |
| **M3** OCR + output | (f) + (b:export/ocr) + F-22 | F-21, F-22, F-24…F-26 pass |
| **M4** Hardening | Stage 2 | every P0 row green, perf baseline recorded, bundle ≤ 60 MB, QA checklist done on macOS and Windows |

P1 items start only after M4, in the order of `FEATURES.md` (P1-1 … P1-12), by whichever agent is free.
