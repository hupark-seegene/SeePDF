# SeePDF — v1 feature list, acceptance criteria and proofs

Priorities: **P0** = v1 does not ship without it · **P1** = ships in v1 if the schedule holds, else v1.1 ·
**P2** = after v1. Every P0 row names the test that proves it; `cargo test` names are Rust integration
tests in `src-tauri/tests/`, `vitest` names are under `src/**/*.test.ts`, `QA` items are rows of
`docs/qa/checklist.md` (they need a human and a second PDF viewer). Feature ids are referenced by
`IPC_CONTRACT.md` §12 and `WORKPLAN.md`.

Fixtures: `tracemonkey.pdf` (14 p text), `TAMReview.pdf` (23 p, all text inside Form XObjects, 47 images),
`160F-2019.pdf` (76 widgets, 67 named fields), `rotation.pdf` (/Rotate 90), `alphatrans.pdf`
(transparency), `annotation-{highlight,line,freetext}.pdf`, generated: `outline-labels.pdf`,
`encrypted-rc4-40.pdf`, `korean-300dpi` scan page, `gen/500p.pdf` (36 × tracemonkey).

---

## P0 — viewing

| id | Feature | Acceptance criteria | Proof |
|---|---|---|---|
| F-01 | **Open**: file dialog, drag & drop onto the window, Finder/Explorer double-click + argv, recent files with restored reading position, password prompt | Every fixture opens and paints page 0 in ≤ 250 ms (release, warm cache); `gen/500p.pdf` in ≤ 400 ms. `encrypted-rc4-40.pdf` with no password shows 암호 입력, a wrong password shows `security.password.wrong` and re-prompts, the right one opens. A path arriving before the webview mounts is not lost. Recents reopen at the stored page/zoom/layout. | `cargo test open_all_fixtures`, `open_password_flow`; `vitest recents`; QA-1 (Finder double-click on a bundled `.app`), QA-2 (drag & drop) |
| F-02 | **Continuous scroll viewer** with single / continuous / two-page layouts, zoom 25–400 %, 페이지 맞춤 / 너비 맞춤 / 실제 크기, ⌘±, ctrl+wheel, pinch | Scrolling `gen/500p.pdf` at 100 % shows no white gaps and holds 60 fps; a zoom step repaints (CSS-scaled) within one frame and sharp tiles land ≤ 250 ms; the point under the cursor stays within 2 px across a pinch. Only pages within ±1.5 screens are mounted. | `vitest layout`, `vitest zoom.anchor`, `vitest TileManager.order`; perf harness `scroll_fps`, `zoom_sharp_ms`; QA-3 (pinch on a trackpad) |
| F-03 | **Rotate view** (non-destructive, ⌘L/⌘R) | All four rotations render the correct pixel size (`rotation.pdf` p1 = 792×612 pt); text selection, search highlights and annotation hit-testing stay aligned in all four. | `cargo test render_rotation_sizes`; `vitest geometry.rotations` (round trip against a Rust-generated table) |
| F-04 | **Thumbnails sidebar**, lazy, current page ringed, drag to reorder, multi-select | On `gen/500p.pdf` placeholders appear ≤ 100 ms and the visible thumbnails ≤ 500 ms; aspect ratio is correct for portrait and landscape (never `thumbnail(n)`); rotating a page refreshes only that tile. | `cargo test render_thumb_aspect`; perf harness `thumbs_ms` |
| F-05 | **Outline sidebar** | `outline-labels.pdf` renders its 7 nodes in 3 levels; clicking a node scrolls to the right page; a document without an outline shows `sidebar.outline.empty`. | `cargo test outline_tree`; `vitest OutlinePanel` |
| F-06 | **Text selection + copy** | Selecting "Trace-based" on tracemonkey p1 copies exactly that string; selection rectangles match `FPDF_PageToDevice` within 1 px at 2×; a 3-line selection produces 3 rectangles; ⌘A selects the visible page's text. | `cargo test text_layer_tracemonkey`, `text_matrix_vs_pdfium`; `vitest selection.hit`, `selection.rects` |
| F-07 | **Search** with options (대소문자 구분 / 단어 단위), streamed results list with ±40 characters of context, ⌘G / ⇧⌘G navigation, in-page highlight of the current hit | "monkey" over tracemonkey = 62 hits (match-case 1, whole-word 4), identical to pdfium's own count; on `gen/500p.pdf` the first result event arrives ≤ 150 ms and a re-query of cached text finishes ≤ 50 ms; a new query cancels the previous pass. | `cargo test search_counts_match_pdfium`, `search_cancel`; `vitest SearchController.merge`; perf harness `search_first_hit_ms` |

## P0 — annotation

| id | Feature | Acceptance criteria | Proof |
|---|---|---|---|
| F-08 | **Text markup**: 형광펜 / 밑줄 / 취소선 / 물결선 over a text selection | Created on tracemonkey p0, saved, reopened: present, bounds equal, `/AP` non-empty, quads in TL,TR,BL,BR order, ≥ N changed pixels at the right place, `/CA` equals the chosen opacity. | `cargo test annot_markup_roundtrip`, `annot_quad_order`; QA-4 (macOS Preview + Acrobat show them) |
| F-09 | **Ink** (펜) + eraser (whole stroke) | A 2-stroke ink annotation persists with a real `/InkList` and the chosen width (`SetBorder`); the eraser removes the strokes it touches and the removal survives save/reopen. | `cargo test annot_ink_roundtrip`; `vitest tools.ink` |
| F-10 | **Shapes**: 사각형 (Square), 타원 (Circle), 선 / 화살표 | Square and Circle persist with stroke colour, interior colour, border width and opacity; line/arrow are written as Ink with `/Subj "SeePDF:Line"`/`"SeePDF:Arrow"` and are re-read by SeePDF as lines (other viewers show ink — documented). | `cargo test annot_shapes_roundtrip`, `annot_line_subj_roundtrip`; `vitest tools.shape` |
| F-11 | **Free text box** including Korean | A text box containing "한글 테스트" renders ≥ 100 dark pixels in its rect after save + reopen and its text is in `/Contents`; the embedded font is the bundled subset, not a system font (file growth ≤ 1.6 MB). | `cargo test annot_textbox_korean`; QA-4 |
| F-12 | **Sticky note** (메모) | A note annotation persists with `/Contents`, `/T` and `/CreationDate` and renders pdfium's note icon; the popup is drawn by the UI and edits save on blur. | `cargo test annot_note_roundtrip`; `vitest NotePopover` |
| F-13 | **Stamp (image)** and **signature (image)** | A PNG placed as a stamp persists with the right bounds and renders; a saved signature image can be re-placed on another page. | `cargo test annot_stamp_image_roundtrip`; `vitest tools.stamp` |
| F-14 | **Annotation properties** (colour, opacity, width, author, note) + delete + annotation list sidebar | Changing the colour of an annotation **loaded from disk** does not crash (the `SetAP(NULL)` recipe) and the new colour survives save/reopen; delete removes it from the file; the sidebar lists every annotation grouped by page and scrolls to one on click. | `cargo test annot_recolor_after_reopen` (run in a child process: the naive path SIGSEGVs), `annot_delete_persists`; `vitest AnnotationList` |
| F-15 | **Undo / redo** for every mutation | 20 random edits followed by 20 undos reproduce the original bytes; redo restores; one OCR run, one `page_ops` batch and one redaction are each a single undo step; undo of a ≤ 5 MB document completes ≤ 50 ms. | `cargo test history_roundtrip`, `history_batch_is_one_step`, `history_spill_to_disk` |

## P0 — pages, content, forms

| id | Feature | Acceptance criteria | Proof |
|---|---|---|---|
| F-16 | **Page organizer**: reorder by drag, delete, rotate, insert blank, duplicate, insert from file, extract, merge, split | `move([3,2] → 1)` yields `A D C B`; delete works descending; duplicate takes 14 → 15 pages; extract of `160F-2019.pdf` keeps `form() == Some` (import would drop `/AcroForm`); merge warns when a source has a form or an outline; the drag commits the model in ≤ 16 ms and everything is undoable. | `cargo test pages_move_order`, `pages_extract_keeps_acroform`, `pages_duplicate`, `pages_insert_from_range_1based`; `vitest organize.dnd` |
| F-17 | **Edit existing text within rules** | On a base-14 or fully covering embedded font, changing a run's text persists and re-renders; when the font's glyphs do not cover the new string the UI asks before substituting the bundled font and never emits tofu; text inside a Form XObject, without `/ToUnicode`, or Type3 is refused with a badge and a reason. | `cargo test objects_text_inplace`, `objects_text_probe_subset_font`, `objects_text_xobject_refused` (TAMReview) |
| F-18 | **Add text box** (page object) including Korean with the bundled font | A new Korean text object renders, extracts with real spaces (not TAB) and survives save/reopen; the document grows by ≤ 1.6 MB once, not per object. | `cargo test objects_add_text_korean` |
| F-19 | **Insert image** (move, resize, delete) | A PNG/JPEG placed at a rect lands at exactly those bounds after save/reopen; moving and deleting it persists. | `cargo test objects_add_image` |
| F-20 | **Form filling**: text, checkbox, radio, combo, list + field highlight + save | On `160F-2019.pdf`, "Raw FORM 입력" typed through the HTML overlay is readable with `FPDFAnnot_GetFormFieldValue`, changes ≥ 100 pixels under `FPDF_FFLDraw`, and is still there after save + reopen (i.e. the `/AP` was regenerated — the high-level `set_value` path fails this test); a radio toggles and reads back. | `cargo test forms_text_value_persists`, `forms_radio_toggle`; QA-5 (value visible in macOS Preview) |
| F-21 | **OCR**: current page / all pages / range, `kor+eng`, searchable output, progress + cancel, fully offline | The synthetic Korean 300-DPI page reaches Hangul CER ≤ 3 % at PSM 4; after apply, `search("검색가능한")` hits, the words render 0 dark pixels, and the file is searchable in a second viewer; cancelling at page 3 of 14 leaves the document unchanged; no network request is made at any point. | `cargo test ocr_layer_apply_searchable`, `ocr_layer_rotation_roundtrip`; `vitest ocr.normalize`, `ocr.cancel`; QA-6 (offline check + Preview search) |
| F-22 | **Redaction**: mark regions, preview what will be removed, apply with true content removal | After apply and **save**, the marked string cannot be extracted from the saved file; the preview names collateral removals (PDFium cannot split a text object: redacting "Gal" removes "Andreas Gal"); intersecting images are blanked; if verification fails the command rolls back and reports `verifyFailed`. | `cargo test redact_removes_text_from_saved_file`, `redact_preview_collateral`, `redact_verify_rollback` |

## P0 — output, shell, quality

| id | Feature | Acceptance criteria | Proof |
|---|---|---|---|
| F-23 | **Save / Save As**, atomic, with dirty prompt | Save writes a temp file in the same directory, fsyncs, reopens and verifies the page count, then renames; killing the process after the temp is written leaves the original byte-identical; a read-only target falls through to Save As with an explanation; closing or quitting with unsaved changes shows 저장 / 저장 안 함 / 취소; an encrypted document stays encrypted. | `cargo test save_atomic_abort`, `save_verify_rejects_truncated`, `save_keeps_encryption`; QA-7 |
| F-24 | **Export pages to PNG / JPEG** (range, DPI, quality, transparent background) | 14 pages at 150 DPI complete in ≤ 400 ms (release) with per-page progress and working cancel; output pixel size equals `round(pt × dpi/72)`; the size estimate is within ±25 % of the result. | `cargo test export_images_sizes`; perf harness `export_14p_ms` |
| F-25 | **Export text and flattened PDF** | Text export reproduces `page.text().all()` per page with a form feed between pages; the flattened PDF has 0 annotations, renders identically (pixel diff ≤ 0.5 %) and — unlike `PdfPage::flatten()` — keeps annotations that lack the Print flag. | `cargo test export_text`, `export_flatten_normaldisplay` |
| F-26 | **Print** | ⌘P opens the system print dialog with the rendered pages at 150 DPI, honours a page range, and prints annotations and filled form values as shown. Fallback path (`print_prepare` + OS handler) works when webview printing is unavailable. | QA-8 (print to PDF on macOS and Windows) |
| F-27 | **Dark mode** (app theme: 시스템/밝게/어둡게) | Every colour comes from a token; switching theme does not reflow the layout or re-render pages; no colour literal exists outside `styles/tokens.css`. | `vitest theme.tokens` + a lint rule; QA-9 |
| F-28 | **ko / en i18n**, Korean default | `ko.json` and `en.json` have identical key sets and identical `{{placeholders}}`; no string literal in JSX outside `src/i18n/`; the UI is laid out against the Korean strings first. | `node scripts/check-i18n.mjs` (CI gate), `vitest i18n.parity`, lint rule `no-literal-jsx-text` |
| F-29 | **Keyboard shortcuts** (the full macOS/Windows table of `UI_SPEC.md` §11) | Every action resolves on both platforms with no duplicate chord; single-letter tool keys are inert while an input or contenteditable has focus; Space pages down on a tap (< 200 ms) and pans on hold; holding a tool key switches momentarily and reverts on release. | `vitest keymap.platform`, `keymap.conflicts`, `keymap.sticky` |
| F-30 | **Performance targets** of `ARCHITECTURE.md` §13 | The harness records launch, open, first paint, tile p50/p95, search first hit, save and export into `docs/perf/baseline.md`; a regression > 20 % on the macOS arm64 runner fails CI. | `scripts/perf-baseline.mjs` in CI |

---

## P0 — feature → commands → UI entry point

Every command of `IPC_CONTRACT.md` appears here exactly once, which is how the three documents stay
consistent: a command with no feature is out of scope, a feature with no command is not reachable.

| id | IPC commands / routes | UI entry point (`UI_SPEC.md`) |
|---|---|---|
| F-01 | `open_document`, `close_document`, `get_document`, `take_pending_opens`, `open_in_new_window`, `window_bind_document`, `get_recent`, `update_recent`, `remove_recent`, `set_recent_pinned`, `clear_recent`, `write_recent_thumbnail`, `reveal_in_file_manager`, `/recent-thumb` | §11 welcome screen, ⌘O, drag & drop, Finder/Explorer, 암호 입력 dialog |
| F-02 | `set_viewport`, `/page`, `/tile` | §5 canvas, §8 status-bar zoom, ⌘±/⌘0/⌘8/⌘9, pinch |
| F-03 | `/page`, `/tile` (`rot` parameter) | §8 status bar, ⌘L/⌘R |
| F-04 | `/thumb` | §4 축소판 tab, §9 organizer cells |
| F-05 | `get_outline` | §4 목차 tab |
| F-06 | `get_text_layer`, `get_page_text` | §6 읽기 선택 tool, ⌘C, context menu |
| F-07 | `search_start`, `cancel_job` | §4 검색 tab, ⌘F, ⌘G/⇧⌘G |
| F-08…F-13 | `create_annotation`, `list_annotations` | §3 주석 tool strip, §7 inspector, §12 context menus |
| F-14 | `update_annotation`, `delete_annotations`, `scan_annotations`, `set_annotations_hidden` (P1) | §7 properties panel + inline popover, §4 주석 tab |
| F-15 | `undo`, `redo` | ⌘Z / ⇧⌘Z, toolbar ↶ ↷, Edit menu labels |
| F-16 | `page_ops`, `extract_pages`, `split_document`, `merge_documents` | §9 organizer grid, §4 축소판 drag, 파일 합치기 / 문서 분할 dialogs |
| F-17 | `list_page_objects`, `probe_text_edit`, `edit_text_object`, `transform_object`, `delete_objects` | §6 편집 mode 텍스트 수정 |
| F-18 | `add_text_object` | §6 편집 mode 텍스트 추가 |
| F-19 | `add_image_object` | §6 편집 mode 이미지 추가 |
| F-20 | `list_form_fields`, `set_form_field_value`, `reset_form` (P1), tile parameter `hl=1` | §3 양식 tool strip, HTML overlay inputs |
| F-21 | `ocr_capabilities`, `ocr_page_status`, `ocr_apply`, `/ocr`, `cancel_job` | §10 OCR dialog, ⋯ overflow menu |
| F-22 | `redact_preview`, `apply_redactions` | §6 편집 mode 영역 표시, §7 redaction panel |
| F-23 | `save_document`, `save_document_as` | ⌘S / ⇧⌘S, §8 save state, unsaved sheet |
| F-24 | `export_images`, `estimate_export` | §10 내보내기 sheet, ⌥⌘E |
| F-25 | `export_text`, `export_flattened` | §10 내보내기 sheet |
| F-26 | `print_prepare`, `render_page_raw`, `/page` | ⌘P, print-only DOM |
| F-27 | `get_settings`, `set_settings` | 설정 › 모양 |
| F-28 | `get_settings`, `set_settings` | 설정 › 일반 |
| F-29 | — (frontend only) | §13 shortcut table, native macOS menu |
| F-30 | `engine_stats` | dev perf overlay ⌥⌘D, CI harness |

The five remaining commands of the contract belong to P1 rows: `set_metadata` + `remove_metadata` → P1-1,
`remove_password` → P1-2, `set_password` → P1-3, `ocr_recognize_native` → P1-11. They are stubbed in
Stage 0 and return `unsupported` until their row is scheduled.

---

## Release gate for v1

A build ships only when all of the following are green on both platforms:

1. Every P0 row above passes its named test; `cargo test --release` and `npx vitest run` are green on
   `macos-14`, `macos-13` and `windows-2022`.
2. `node scripts/check-i18n.mjs` passes and no JSX literal string exists outside `src/i18n/`.
3. `docs/perf/baseline.md` is regenerated and every row of `ARCHITECTURE.md` §13 is met on the macOS
   arm64 runner; no row regressed by more than 20 %.
4. `node scripts/check-bundle-size.mjs` reports ≤ 60 MB installed on each target.
5. `--smoke` passes against the built bundle on each target (this is what proves resource resolution and
   the libpdfium load path in a packaged app).
6. `docs/qa/checklist.md` is fully ticked, including: annotations saved by SeePDF render correctly in
   macOS Preview **and** Acrobat; a filled form shows its values in Preview; an OCR'd scan is searchable
   in a third-party viewer; a redacted file yields no trace of the redacted string when its text is
   extracted by another tool; the app performs no network request at any point.
7. No `unsafe` block exists outside `src-tauri/src/engine/raw/`, and no `pdfium_render::` import exists
   outside `src-tauri/src/engine/`.

---

## P1 — ships in v1 if the schedule holds

| id | Feature | Notes / dependency |
|---|---|---|
| P1-1 | **Metadata edit** (제목/작성자/주제/키워드) and **메타데이터 제거** | pdfium has no `FPDF_SetMetaText`; needs **`lopdf`** to rewrite `/Info` (and the XMP stream) after save |
| P1-2 | **Remove password** | `FPDF_SaveAsCopy(FPDF_REMOVE_SECURITY = 4)` through the raw save path — note 4, not the deprecated 3 |
| P1-3 | **Password protect** (열기 암호 / 권한 암호 + permission flags, AES-256) | pdfium cannot write encryption; needs **`lopdf` ≥ 0.45** (`Document::encrypt`, `src/encryption/algorithms.rs`). This is the named extra crate. |
| P1-4 | **Watermark / header-footer / page numbers** | text + image page objects per page, reusing F-18's layout code; template tokens (`{{page}}`, `{{total}}`, date) |
| P1-5 | **Compress** (image downsample presets 300/150/96 DPI + estimate) | app-level: `get_raw_bitmap` → resize → `set_image` (−14.5 % measured on TAMReview); pdfium always re-encodes as Flate, so photo-heavy files can grow — the dialog must show the measured before/after and let the user cancel. Real JPEG passthrough needs `lopdf` |
| P1-6 | **Compare two documents** | text-diff per page with side-by-side scroll sync; no pixel diff in v1.1 |
| P1-7 | **Batch OCR** over several files | reuses the worker pool; one window, a queue, per-file progress |
| P1-8 | **Autosave / crash recovery** | a recovery copy every 60 s while dirty into `$APPDATA/SeePDF/recovery/`, offered on next launch; never touches the user's file |
| P1-9 | **Signature: draw / type** (image signature is P0) + saved signature library | draw → Ink with `/Subj "SeePDF:Signature"`; type → text object rendered to an image stamp |
| P1-10 | **Night mode for the document** (끄기 / 어둡게 / 세피아) | CSS filter on the tile layer only, with a transparent clear colour |
| P1-11 | **macOS Vision OCR** | `ocr_recognize_native` behind the same `OcrPage` contract; needs `objc2`, `objc2-foundation`, `objc2-core-foundation`, `objc2-core-graphics`, `objc2-vision` (unverified compile) |
| P1-12 | Korean stamp set (결재 / 승인 / 기밀), per-tool default styles, reading mode + full screen, hide-annotation-while-dragging (`set_annotations_hidden`) | small UI work on top of P0 machinery |

## P2 — after v1

Paragraph reflow when editing text · document tabs in one window · link creation and go-to-page
destinations (needs `lopdf`) · outline/bookmark editing (needs `lopdf`) · page labels · crop and resize
pages · form field authoring · word-level redaction (re-creating surviving glyphs) and the raster
fallback for XObject text · glyphless CID OCR font (`FPDFText_LoadCidType2Font`) instead of the bundled
subset · incremental save · Windows.Media.Ocr opportunistic path · export to Word/hwp (third-party
engine, licence review first) · annotation replies/threads and summary export · multi-file search ·
split view · TTS · auto-update.

---

## Explicit non-goals for v1

| Not in v1 | Why |
|---|---|
| Office / hwp import and export | needs a third-party conversion engine; licensing must be settled first |
| Digital signature certificates (공인전자서명), PKI validation | a separate compliance workstream (see open question 3 of the UX research) |
| XFA forms | not built into this pdfium; documents are opened read-only with a banner |
| PDF/A, preflight, Bates numbering | Acrobat-tier features our users did not ask for |
| Form field authoring, JavaScript actions, field calculation | pdfium exposes no authoring API and no JS engine is shipped |
| Document tabs | one window per document has the same capability and no tab-state code |
| Cloud sync, accounts, telemetry, AI features | the product promise is a local, offline, quiet tool |
| Bookmark/outline editing, metadata writing, page labels in P0 | pdfium cannot write any of them; deferred until `lopdf` lands (P1) |
| Incremental save | a second save path for a ≤ 5 ms/MB win; the full rewrite already meets the target |
| Polygon / polyline / cloud / callout / measurement annotations | `FPDFPage_CreateAnnot` returns NULL for Polygon and Polyline |
| Real `/Line` annotations | same reason; v1 writes Ink with a `/Subj` tag and says so in the docs |
| Partial-stroke ink erasing | v1 erases whole strokes |
