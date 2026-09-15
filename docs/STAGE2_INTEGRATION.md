# Stage 2 — integration log

What the integrator changed, what was verified in the **real** app (real engine, real PDFium, no
mock) and what is still open. Written against `WORKPLAN.md` §"Stage 2" and the integration-request
sections of every module's notes.

Date: 2026-09-16 · host: macOS 26 arm64, Rust 1.96, Node 24.15, PDFium 155.0.8057.

---

## 0. Gates

| Gate | Command | Result |
|---|---|---|
| Rust compile, all targets | `cd src-tauri && cargo check --all-targets` | **pass**, 0 warnings (the `examples/spike_*.rs` still compile) |
| Rust tests | `cargo build --release --tests && cargo test --release` | **pass** — 126 tests in 16 binaries, 0 failed |
| Rust tests, determinism | `touch src/lib.rs && cargo test --release` ×3 | **pass 3/3**, no `panic strategy 'abort'` errors, **0** `output filename collision` warnings |
| Typecheck | `npm run typecheck` | **pass** |
| Frontend tests | `npx vitest run` | **pass** — 33 files, 269 tests |
| i18n parity | `node scripts/check-i18n.mjs` | **pass** — ko 442 / en 442 keys |
| Production build | `npm run build` | **pass** |
| Bundle budget | `node scripts/check-bundle-size.mjs` | **pass** — critical path **105.1 kB gz** of 120; `dist` 588.7 kB of 2048 |
| Desktop bundle | `npm run tauri build -- --debug` | **pass** — `SeePDF.app` + `SeePDF_0.1.0_aarch64.dmg` |

### 0.1 The flaky `cargo test --release` (STAGE1B §5.3) — fixed at the root

`src-tauri/Cargo.toml` had the Tauri template's `crate-type = ["staticlib", "cdylib", "rlib"]`.
Two extra lib units wrote the same artifact paths (four `output filename collision` warnings per
build) and, with `[profile.release] panic = "abort"`, a test target could link the unit built for
the wrong panic strategy — roughly one `cargo test --release` in three died with 173 errors of
`the crate X requires panic strategy 'abort'`.

SeePDF is desktop-only (macOS + Windows) and `src/main.rs` links the `rlib`; the other two crate
types exist only so the same crate can be linked into an iOS/Android host. **`crate-type` is now
`["rlib"]`.** The collision warnings are gone, three consecutive `cargo test --release` runs after
touching a library source were green, and `npm run tauri build -- --debug` still produces the
`.app` and the `.dmg`.

`.github/workflows/ci.yml` now runs `cargo build --release --tests` as its own step before
`cargo test --release` — not because it is needed any more, but because it separates a compile
failure from a test failure in the log.

---

## 1. Integration requests, by owner

### 1.1 Backend

| Request | Status | Where |
|---|---|---|
| **STAGE1B §5.1** — `set_metadata` / `remove_metadata` / `remove_password` bodies belong in `commands/security.rs` | **done**. `remove_password` is implemented (`generate_appearances` → `FPDF_SaveAsCopy(FPDF_REMOVE_SECURITY)` → `write_atomic`). The two metadata writers now go through `engine::save::write_metadata`, which owns the single "PDFium 8057 has no `FPDF_SetMetaText`" message, so the P1 `lopdf` patch has one call site. An unknown `docId` is `notFound`, not `unsupported`. | `src-tauri/src/commands/security.rs` |
| **STAGE1B §5.2** — one Hangul font token per `OpenDoc`, not per page | **done**. `OpenDoc.hangul_font` + `hangul_token_for(text)` / `hangul_token()` / `adopt_hangul_token(page)`; cleared by `registry::replace` (the `FPDF_FONT` dies with the document). `objects::edit_text_object`, `objects::add_text` and `ocr::apply` use it, so Korean on five pages of one document embeds the 487 kB subset **once**, and (f)'s per-page `ocr_apply` chunking became affordable. | `engine/registry.rs`, `engine/objects/mod.rs`, `engine/ocr/mod.rs` |
| **STAGE1B §5.4** — `replace_image` has no command | **done** — `commands::objects::replace_image`, registered in `lib.rs`, documented in `IPC_CONTRACT.md` §7.4. | `src-tauri/src/commands/objects.rs` |
| **STAGE1B §5.4** — `extract_images` has no command | **not done**, deliberately. `objects::extract_images` returns pixels, so the contract shape is a `seepdf://` route, not a command, and nothing in the v1 UI consumes it. Left as engine-only. | — |
| **STAGE1B §5.4** — `SplitMode` has no `byOutline` | **not done** — there is no backend for it in v1 (`pages::split_plan` would need outline ranges). Noted in §5. | — |
| **STAGE1B §5.5** — `ocr_apply` cancellation granularity | **confirmed, no change**. (f) calls `ocr_apply` **per page** (`ocrJob.ts` applies each page as it is recognised), so each page is its own `registry::mutate` = its own undo step, and cancelling at page 3 of 14 leaves pages 1–2 applied and the rest untouched. The document-level font token above is what makes per-page calls cheap. | — |
| **page_ops move semantics** | **verified on both sides**. `moveOpFor` sends the block in document order with `to` counted against the array with the moved pages removed; the engine calls `FPDF_MovePages` with exactly that. Live: `move([2,3] → 1)` on the 14-page fixture turned `A B C D` into `A C D B`. | `src/organize/moveOp.ts`, `engine/raw/page.rs` |
| **rotate swaps `widthPt`/`heightPt`** | **verified**: after `rotate([0], 90)` page 0 reports `792 × 612, rotation 90`, and it survives save + reopen. The **mock** did not swap them (STAGE1E §5.5) — fixed. | `src/ipc/mock.ts` |
| **`split_document`'s `done` carries `outputs`** | **verified** — three parts, `outputs` present on `done`. | — |
| **`save_document` rejects read-only / path-less** | **already correct** (`engine::save::save` → `ErrorCode::ReadOnly`, `cargo test save_untitled_needs_a_path`). | — |
| **`DocChangedEvent` gains `canUndo`/`canRedo`** | **done** end to end: `DocChangedPayload` carries them, both emitters set them, `src/ipc/types.ts` mirrors them, `docStore.applyDocChanged` folds them into `DocInfo`, and **(d)'s per-edit `get_document` is gone** (`annot/sync.ts`). | `ipc/types.rs`, `engine/registry.rs`, `src/store/docStore.ts`, `src/annot/sync.ts` |
| **STAGE1C §7.1** — `OutlineNode` has no destination rect | **done**. `OutlineDest { x?, y?, zoom? }` read from `FPDFDest_GetView` + `FPDFDest_GetLocationInPage` (document-level, no `FPDF_LoadPage`, so it is affordable for every node at open). `None` for a plain page reference or a fit-to-window view. `viewStore.goToPage(page, yPt)` and the scroller land on the heading. Live: `TAMReview.pdf` → `dest {x: 0, y: 806}`; `gen/outline-labels.pdf` uses `/Fit`, so `dest` is correctly `None`. | `ipc/types.rs`, `engine/registry.rs`, `src/sidebar/Outline.tsx`, `src/viewer/Scroller.tsx` |
| **`Settings.recentsCount`** | **done** — a first-class contract field with `#[serde(default)]`, so a settings file written before the change still loads. `SettingsDialog` and `Welcome` read it and fall back to the old `toolDefaults.recentsCount`. | `ipc/types.rs`, `src/dialogs/SettingsDialog.tsx`, `src/welcome/Welcome.tsx` |

### 1.2 Frontend

| Request | Status | Where |
|---|---|---|
| **STAGE1D §7.1** — `PageShell` must expose the tile `onload` to a layer | **done**. `PageShellProps.onPageRendered(page, docGeneration)` fires from the placeholder, the whole-page bitmap and every tile; `Viewer`/`Scroller` forward it; `annot/bridge.tsx` settles the optimistic ghost with it. **`src/annot/RenderProbe.tsx` deleted** along with its `.annot-probe` CSS, and `annot/overlay.test.tsx` now drives a real `PageShell`. | `src/viewer/PageShell.tsx`, `src/annot/bridge.tsx` |
| **STAGE1D §7.2** — the viewer never passes `hl` to tile/page URLs | **done**. `Viewer`/`Scroller` take `fieldHighlight`; `bridge.tsx` sets it from `formStore.highlight && mode === "form" && info.hasForm`, so only 양식 mode pays a re-render. `hl` is part of the tile cache key. | `src/viewer/Scroller.tsx`, `src/annot/bridge.tsx` |
| **STAGE1C §8 / STAGE1D §7.3** — `--page-paper` / `--page-ink` / `--page-field` are not tokens | **done**: defined once in `styles/tokens.css` §14.3b and deliberately **not** redefined per theme (they are the colours of paper, not of chrome). The three `background: #fff` literals left in `styles/shell.css` now use `var(--page-paper)`, so F-27's "no colour literal outside tokens.css" holds. | `src/styles/tokens.css`, `annot.css`, `viewer.css`, `shell.css` |
| **STAGE1E §5.1** — `.thumb` has no `data-page` | **done**, with the `.thumb-num` fallback kept in `pageMenus.ts`. | `src/sidebar/Thumbnails.tsx` |
| **STAGE1E §5.2** — `ToolSurface` swallows `contextmenu` | **done**: the `stopPropagation` is gone, replaced by a comment pointing at the `data-context-menu` opt-out both `App.tsx` and `pageMenus.ts` already honour. `App.tsx` still `preventDefault`s over a page, so the webview's own menu never appears. | `src/annot/ToolSurface.tsx` |
| **STAGE1E §5.4** — `docStore.adopt(info)` | **done** and used by `mergePaths` instead of `useDocStore.setState`. It also re-fetches the outline when the adopted document has one. | `src/store/docStore.ts`, `src/dialogs/flows.ts` |
| **STAGE1C §7.3** — `events.ts` rejects on unlisten under StrictMode | **done**. `once()` nulls the slot before calling, wraps in `try/catch` and swallows a rejected unlisten promise, for both `subscribe` and `onFileDrop`. | `src/ipc/events.ts` |
| **STAGE1C §7.2 / STAGE1D §7.8** — duplicate ⌘G / 선택 해제, and `edit.copy/cut/paste` never reach `useCommands` | **done, with one deviation — see §2.** `edit.findNext` / `edit.findPrevious` / `edit.deselect` / `edit.selectAll` are dispatched in `useCommands` only; the duplicate listener inside `viewerCommands.ts` is deleted and the three primitives are exported instead. | `src/app/useCommands.ts`, `src/viewer/viewerCommands.ts`, `src-tauri/src/app/menu.rs` |
| **STAGE1E §5.5** — the mock always answers the same path | **done**: `openFileDialog` answers three fixture paths for `multiple`, a folder for `directory`, one path otherwise; `page_ops { rotate }` swaps `widthPt`/`heightPt`; `doc-changed` carries `canUndo`/`canRedo`. | `src/ipc/api.ts`, `src/ipc/mock.ts` |
| **STAGE1D §7.7** — the mock's page bitmaps do not paint annotations | **not done** (nice-to-have, skipped). | — |
| **`VITE_SEEPDF_MOCK` defaults off** | **already correct** — `useMock()` is true only when the flag is explicitly set or there is no Tauri. The real app talks to the engine; `dist/` has no reference to the mock outside its own lazy chunk. | `src/ipc/env.ts` |
| **Print capability** | **done** — `capabilities/default.json` gains `opener:allow-open-path` **scoped to `$TEMP/seepdf-print/**`**, which is where `print_prepare` writes. Without it the print fallback was denied. `core:webview:allow-print` was already there. See §2 for which path is primary. | `src-tauri/capabilities/default.json` |
| **Leftover spike bits** | `src-tauri/examples/spike_*.rs` are kept as documentation and still compile (`cargo check --all-targets` is clean). There is no `src/spike/**` or `src-tauri/src/spike/**`. | — |

---

## 2. Deviations, with reasons

### 2.1 Cut / Copy / Paste stay **predefined** native menu items

The brief asked for custom items emitting `menu:edit/copy` etc. That would regress **F-06**.

On macOS a menu key equivalent is consumed by the menu, so a custom item replaces the native
`copy:` action with a Tauri event. `document.execCommand("copy")` from a Tauri event callback has
**no user activation**, and WKWebView rejects `navigator.clipboard.writeText` on a custom scheme —
measured live: the dev bridge calling `copyToClipboard()` outside a gesture returned `false` and
the clipboard was untouched, while the native **Edit ▸ Copy** item copied all 5 012 characters of
the page selection through `viewerCommands.ts`'s clipboard mirror.

So the three predefined items stay, and the thing they were blocking — the **annotation**
clipboard — is reached from the DOM `copy` / `cut` / `paste` events those items raise, which is the
seam `STAGE1D_NOTES` §7.8 was missing. `AnnotationHost.start()` installs capture-phase listeners
that run only in 주석 mode, only outside an input, and only `preventDefault()` when they handled it,
so a text selection still falls through to the viewer's own handler.

**Select All is now custom** (`edit.selectAll`, ⌘A): 페이지 mode selects cells, the canvas selects
the page's text, an input selects its own value — none of which WebKit can do for us, and all of
which work with no user activation. Verified live: ⌘A over the canvas selected all of page 1.

### 2.2 Printing: the OS handler is the **primary** path, the webview is the fallback

`core:webview:allow-print` is in the capability and the `plugin:webview|print` core command does
open the macOS print panel — but what it prints is the **webview's DOM**: the SeePDF toolbar,
sidebar and canvas element, on one sheet ("Page 1 of 1" in the panel preview, screenshot
`s2-16.png`). F-26 wants the rendered pages.

`runPrint` therefore now does `print_prepare` → `opener.openPath(tempFile)` first, and only falls
back to the webview panel if no OS handler exists. The flattened temp file carries the chosen
pages with annotations and filled form values baked in, which is exactly what F-26 asks for.
The webview path becomes correct once there is a print-only DOM (`STAGE1E_NOTES` §7.4, P1).

### 2.3 Two IPC payload bugs found by the smoke — both fixed

Tauri maps the keys of `invoke(cmd, args)` onto the command's **parameter names**. Two commands
take a single named struct and the frontend spread its fields instead:

* `set_viewport(hint: ViewportHint)` — every viewport hint had been failing with
  `missing required key hint`, silently (the caller is `void api.setViewport(...)`). Prefetch and
  the render-priority hints had therefore never reached the engine since Stage 0.
* `export_images(args: ExportImagesArgs, on_progress)` — 내보내기 › PNG failed with
  `missing required key args`. **F-24 had never worked from the UI.**

Both are one-line fixes in `src/ipc/api.ts` (`{ hint: a }`, `{ args: a, onProgress }`) plus a new
regression test, `src/ipc/invokeShape.test.ts`, which captures the payload of a representative
command of each shape.

---

## 3. End-to-end smoke in the real app

Driven through a **dev-only** command bridge so every step is reproducible and its result is a
JSON read-back rather than a squint at a screenshot:

* `scripts/dev-bridge.mjs` — a Vite **dev-server** plugin (`apply: "serve"`); it does not exist in
  `npm run build` and never reaches a bundle. `POST /__dev/cmd` blocks until the page answers.
* `src/dev/testHook.ts` — the browser half, imported by `App.tsx` only under `import.meta.env.DEV`,
  so the bundler drops the branch and the chunk (verified: `dist/` contains no `__dev/cmd`).
  No `eval` — a fixed op table (`run` a command id, `api`/`job` an IPC call, `annotate`, `pageOps`,
  `save`, `ocr`, `search`, `text`, `state`).

Also new: `src-tauri/examples/inspect_pdf.rs`, which reopens a **saved file** with a fresh engine
and prints its pages, annotations (subtype, rect, colour, opacity, border, contents), AcroForm
values and extracted text. That is the independent check that what the UI showed reached the bytes.

Screenshots are in `fixtures/out/stage2/shots/` (gitignored), saved files in
`fixtures/out/stage2/`.

| # | Flow | Evidence |
|---|---|---|
| 1 | **Open by argv** — `npm run tauri dev -- --no-watch -- -- fixtures/tracemonkey.pdf` | `s2-01.png`: page 1 painted, thumbnail rail, Korean UI, native menu. Log: `open request path=… source=Argv` |
| 2 | **Scroll / zoom / layout / rotate** | `s2-18.png`: `gen/500p.pdf`, 두 쪽 layout, view rotation 90°, jumped to page 250. `engine_stats` at that point: tile p50 **0.18 ms**, p95 **1.26 ms**, RSS 367 MB (debug build) |
| 3 | **Select + copy** | `s2-02.png`: ⌘A (native Select All → `menu:edit/selectAll` → `useCommands`) selects all of page 1 with per-line rectangles. Edit ▸ Copy put **5 012 characters** on the system pasteboard (`pbpaste`), starting `Trace-based Just-in-Time Type Specialization for\nLanguages\n…` |
| 4 | **Search** | `search("monkey")` over tracemonkey → **62 hits**, the number F-07 pins, with ±40-character context |
| 5 | **Annotate** — highlight, square, ink, note, Korean text box | `s2-03.png`: all five painted by PDFium plus the selection handles and the 속성 inspector |
| 6 | **Save → reopen → verify** | `fixtures/out/stage2/annotated.pdf`; `inspect_pdf` lists all five with the right subtypes, rects, colours, `opacity 0.40` on the highlight, `border 3.0` on the ink, `contents "한글 메모 테스트 — Stage 2"` and `text "한글 테스트 텍스트 상자"` |
| 7 | **QA-4 — macOS Preview** | `s2-06-preview.png`: Preview renders the highlight, the rectangle, the ink stroke, the note icon and the Korean text box from SeePDF's saved file |
| 8 | **Recolour an annotation loaded from disk** | the square from `annotated.pdf` → `[123, 214, 74]`, border 5; saved as `recoloured.pdf`; `inspect_pdf` confirms. No crash — the `SetAP(NULL)` recipe holds after a reopen |
| 8b | **Save in place (⌘S)** | `file.save` on `inplace.pdf`: file rewritten (1 016 315 → 1 009 828 B, mtime moved), `dirty` cleared, `status.saved` toast, and `inspect_pdf` finds the new highlight in the file |
| 9 | **Unsaved-changes prompt** | `s2-04.png`: `"tracemonkey.pdf"의 변경 사항을 저장하시겠습니까?` with 취소 / 저장 안 함 / 저장; 저장 안 함 closes without writing |
| 10 | **Welcome + recents** | `s2-05.png`: recents card with thumbnail, path, `오늘 · 14쪽 · 992.5 KB` |
| 11 | **Forms, Korean** | `s2-07.png`: 양식 mode, 76 fields tinted; `A.NOM = "박현우 테스트"`, `A.PRENOM = "Raw FORM 입력"`. `inspect_pdf` on `form-filled.pdf` reads both values back |
| 12 | **페이지 mode** | `s2-08.png`: the 14-cell grid + rail. `move([2,3] → 1)` → `A C D B` (page text heads), `rotate([0], 90)` → `792 × 612 rot 90`, `delete([13])` → 13 pages. After save + reopen: 13 pages, page 0 still `792 × 612 rot 90`, order unchanged |
| 13 | **Export PNG** | 3 files at 150 DPI; sizes exactly `round(pt × dpi/72)` — rotated page 0 is **1650 × 1275**, page 1 **1275 × 1650**. `estimate_export` said 3 916 110 B, the files total 3 916 112 B (**0.00 %** off; F-24 allows ±25 %) |
| 14 | **Export text / flattened** | `text.txt` 11 538 chars with a form feed between pages; `flat.pdf` has **0 annotations** on page 0 |
| 15 | **OCR, full loop** | built a scan through the app itself (blank A4 + `add_image_object` of `fixtures/out/ocr/korean-300dpi.png` → `scan.pdf`, **0 extractable characters**), OCR page 0 at 300 DPI `kor+eng` → 91 words in **1.29 s** → 484 characters; `search("대한민국")` and `search("광학")` hit; saved as `scan-ocr.pdf`; reopened → still 1 hit. `s2-13.png` shows the selection rectangles sitting exactly on the scanned lines, so the text layer is invisible **and** aligned |
| 16 | **Redaction** | `redact_preview` named the two title text objects and the collateral it would take; `apply_redactions` → `removedObjects 2, verified true`; the saved file's extracted text lost the title (5 087 → 5 018 chars) |
| 17 | **Page objects** | `add_text_object("한글 페이지 객체 테스트")` ✓; `probe_text_edit` on the title answered `replaceFont / glyphsMissing / Helvetica` and `edit_text_object` **refused** without `allowFontSubstitution`, then succeeded with it. Both strings are searchable in the saved `objects.pdf` |
| 18 | **Outline** | `gen/outline-labels.pdf` → 7 nodes, 3 levels; `TAMReview.pdf` → `dest {x: 0, y: 806}` |
| 19 | **Undo / redo** | create → `canUndo true`; undo → 0 annotations, `canUndo false / canRedo true`; redo → 1 annotation, `canUndo true / canRedo false` — all from the `doc-changed` payload, no `get_document` |
| 20 | **Merge / split** | merge of two files → 7 pages with `outlineDropped`/`metadataDropped` warnings, adopted by `docStore.adopt`; `split everyN 2` → 3 files, `done` carrying `outputs` |
| 21 | **Settings live** | `s2-09.png`: English + dark applied without a reload; the `Recent items` field is the new `Settings.recentsCount` |
| 22 | **Password** | `s2-10.png` 암호 입력 → `s2-11.png` wrong password shows `암호가 올바르지 않습니다` and re-prompts → `user` opens the 1-page encrypted document |
| 23 | **Print** | `s2-15.png` / `s2-17.png`: `print_prepare` writes `$TMPDIR/seepdf-print/annotated-d9-5715.pdf` (14 pages, annotations baked in) and the opener hands it to Preview. `s2-16.png` is the webview panel showing why it is not the primary path (§2.2) |

Not driven: **drag & drop onto the window** (QA-2) and **Finder double-click on the bundled `.app`**
(QA-1) — neither is scriptable from here; `onFileDrop` and `take_pending_opens` are wired and the
argv path (the same `app::files::open_request` entry point) is verified. Windows is untouched.

---

## 4. Files changed

**Backend** — `Cargo.toml` (crate-type), `capabilities/default.json`, `src/app/menu.rs`,
`src/commands/{security,objects}.rs`, `src/engine/{registry,objects/mod,ocr/mod}.rs`,
`src/ipc/types.rs`, `src/lib.rs`, new `examples/inspect_pdf.rs`.

**Frontend** — `src/ipc/{api,events,types,mock}.ts`, `src/store/{docStore,viewStore}.ts`,
`src/viewer/{PageShell,Scroller,Viewer,viewerCommands,index}.tsx?`,
`src/annot/{bridge,AnnotationHost,ToolSurface,sync}.tsx?` (+ `RenderProbe.tsx` **deleted**),
`src/app/{useCommands,pageMenus}.ts`, `src/dialogs/{flows,SettingsDialog}.tsx?`,
`src/sidebar/{Outline,Thumbnails}.tsx`, `src/welcome/Welcome.tsx`, `src/App.tsx`,
`src/styles/{tokens,shell}.css`, `src/annot/annot.css`, `src/viewer/viewer.css`,
new `src/dev/testHook.ts`, new `src/ipc/invokeShape.test.ts`.

**Tooling / docs** — `vite.config.ts`, new `scripts/dev-bridge.mjs`, `.github/workflows/ci.yml`,
`docs/IPC_CONTRACT.md`, `docs/FEATURES.md` (status column), `README.md`, this file.

---

## 5. What is still open, worst first

1. **F-30 — the performance baseline is not recorded.** `scripts/perf-baseline.mjs` is still the
   Stage 0 skeleton: every engine row is `null` and nothing drives the release binary, so the
   ±20 % CI gate cannot be turned on. Individual numbers *were* measured by hand during the smoke
   (tile p50 0.18 ms / p95 1.26 ms, OCR 1.29 s/page at 300 DPI, export 3 pages at 150 DPI in
   ~0.3 s, RSS 367 MB on a 500-page document in a **debug** build) but they are not in
   `docs/perf/baseline.md` and the harness cannot reproduce them.
2. **F-26 — print has no print-only DOM.** The OS-handler path works and prints the right pages,
   but "⌘P opens the system print dialog" is one hop away: the user gets Preview (or the Windows
   handler) first. The webview path is wired and ready for the day the print-only DOM exists.
3. **Form widgets are drawn twice.** In 양식 mode the value appears both in PDFium's widget bitmap
   and in the HTML overlay input, offset by a pixel or two (visible in `s2-07.png` on the two
   filled fields and on 'Réinitialiser'). Either the overlay should be transparent until focus, or
   the tile should be rendered with the widget suppressed — `set_annotations_hidden` (P1) is the
   same machinery STAGE1D §7.6 wants for dragging.
4. **QA-1 / QA-2 / Windows.** Finder double-click on the bundled `.app`, drag & drop onto the
   window, and everything on Windows are unverified from this host.
5. **`extract_images` and `SplitMode::byOutline` have no command / no backend** (STAGE1B §5.4).
   Neither has a UI entry point in v1.
6. **Link annotations cannot be created** (STAGE1A §5.3): `engine::annot::create::create_link` is
   complete and tested but `AnnotSpec` has no `Link` variant. The contract calls link creation P2.
7. **Image signatures report `kind: "stamp"`** (STAGE1A §5.3) — `StampSpec` carries no `/Subj`.
8. **`registry::mutate` still does not restore a closure that failed half-way** (STAGE1A §5.2);
   `redact::apply_verified` keeps its own byte snapshot as the workaround. Left alone because every
   current caller either fails before mutating or carries its own snapshot.
9. **`OpenDoc::invalidate_page` still drops the text layer** (STAGE1A §5.1). The read paths avoid
   it, so only writes pay the ~5.6 ms rebuild.
10. **The native menu is English only** and is not rebuilt on `set_settings` (STAGE0 §4.6, P1).
    Visible in every screenshot: the menu bar says *File / Edit / View* under a Korean UI.
11. **macOS 12 has no WASM SIMD**, so OCR 404s on the core there unless `prepare-ocr.mjs` ships the
    fallback core (+3.9 MB) or `minimumSystemVersion` moves to 13.0 (STAGE1F §7.2). Undecided;
    `README.md` documents 13+ for OCR.
12. **Do not edit `src/**` while a dev-bridge smoke run is in flight.** Vite HMR re-evaluates the
    changed module *and its importers*, which gives zustand a second store instance: the UI keeps
    one, a closure captured earlier keeps the other, and the bridge starts answering from a store
    with no document. `import.meta.hot.dispose` now stops the superseded poller, but the only safe
    procedure is to finish editing, restart `tauri dev`, then run the script.
