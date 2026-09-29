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
| F-22 | **Redaction**: mark regions, preview what will be removed, apply with true content removal | After apply and **save**, the marked string cannot be extracted from the saved file; a partly marked text run loses only the marked characters (v0.3: redacting "Gal" keeps "Andreas" in place, justified / letter-spaced text included) and only a run that cannot be split (Type3, no `/ToUnicode`) is removed whole — the preview is a dry run of the apply, so it names that collateral first and refuses when the apply would; nothing outside the marks and the named collateral moves or disappears (page-wide check, also on pages whose content streams break mid-object); an image entirely inside a mark is removed and one partly under a mark has those pixels blacked out in its own data (v0.3, stencil-mask scans too; before v0.3 the scan pixels stayed under the box); if verification fails the command rolls back and reports `verifyFailed`. | `cargo test redact_removes_text_from_saved_file`, `redact_preview_reports_split_not_collateral`, `redact_word_keeps_the_rest_of_its_run`, `redact_blanks_partially_covered_image_pixels`, `redact_blanks_a_stencil_mask_scan_in_place`, `redact_verify_rollback`, `redact_refuses_when_the_regeneration_would_drop_unmarked_text`; `tests/redact_split_regressions.rs` |

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

## P0 status — Stage 2 integration

✅ verified end to end in the **real** app (not the mock) · ⚠️ partial, with the gap named ·
❌ missing. Evidence, screenshots and the commands behind each line are in
`docs/STAGE2_INTEGRATION.md` §3 and `docs/STAGE2B_HARDENING.md`; `sN` names are files in
`fixtures/out/stage2/shots/`, plain names in `fixtures/out/stage2b/`.

| id | status | evidence |
|---|---|---|
| F-01 | ✅ | argv open, welcome+recents, wrong→right password all live (`s2-01/05/10/11`); **QA-1 done in Stage 2b**: `open -a SeePDF.app tracemonkey.pdf` on the bundled app opens it (`qa1-finder-open`), a second `open -a … rotation.pdf` is handled by the **same process** (`qa1-second-open`), and `--smoke` passes against the packaged binary. QA-2 drag & drop still unverified |
| F-02 | ✅ | `gen/500p.pdf` at 두 쪽 + rotate 90, jump to p250 (`s2-18`); Stage 2b baseline: open 500 p → first paint **26 ms**, tile @2× p50/p95 **0.48 / 2.31 ms**, @4× **0.34 / 1.63 ms**; fps and pinch still not instrumented |
| F-03 | ✅ | view rotation 90° live; `cargo test render_rotation_sizes` |
| F-04 | ✅ | rail renders and rings the current page (`s2-01/03`); `cargo test render_thumbnails_keep_aspect_ratio` |
| F-05 | ✅ | `outline-labels.pdf` → 7 nodes / 3 levels; `TAMReview.pdf` node carries `dest {x:0, y:806}` and the scroller lands on it |
| F-06 | ✅ | ⌘A selects page 1, Edit ▸ Copy puts 5 012 chars on the pasteboard (`pbpaste`), rectangles match per line (`s2-02`) |
| F-07 | ✅ | `search("monkey")` → **62 hits** with ±40-char context, the number the row pins |
| F-08 | ✅ | highlight saved + reopened + rendered by macOS Preview (`s2-03/06`); `opacity 0.40` read back by `inspect_pdf` |
| F-09 | ✅ | 2-stroke ink, `border 3.0`, round-trips and renders in Preview; eraser is `vitest tools.ink` |
| F-10 | ✅ | square round-trips with colour/border/opacity; line/arrow by `cargo test annot_line_subj_roundtrip` (not driven live) |
| F-11 | ✅ | `한글 테스트 텍스트 상자` saved, reopened, and rendered by Preview; bundled subset, file +0.36 MB |
| F-12 | ✅ | note with `contents "한글 메모 테스트 — Stage 2"` persists and renders PDFium's icon |
| F-13 | ⚠️ | `cargo test annot_stamp_image_roundtrip` passes. **서명 만들기 now exists** (Stage 2b §6.3): a canvas sheet whose strokes commit as `AnnotSpec { kind: "signature" }` → a real `/Ink` with `/Subj "SeePDF:Signature"`, covered by `vitest src/dialogs/signature.test.ts` (7 tests). The *image* 도장 path still is not driven live — arming it opens a native file picker |
| F-14 | ✅ | a square **loaded from disk** recoloured to `[123,214,74]` border 5, saved, verified — no crash; inspector + 주석 list render |
| F-15 | ✅ | create → undo → redo round trip; `canUndo`/`canRedo` now ride on `doc-changed`, no `get_document` per edit |
| F-16 | ⚠️ | `move([2,3]→1)` → `A C D B`, rotate swaps 792×612, delete 14→13, insertBlank, merge (7 p + warnings), split everyN → 3 files with `outputs`; **the drag gesture itself and 추출 were not driven with a mouse** |
| F-17 | ✅ | probe answered `replaceFont/glyphsMissing/Helvetica`, the edit **refused** without `allowFontSubstitution` and succeeded with it; the new string is searchable in the saved file |
| F-18 | ✅ | `한글 페이지 객체 테스트` added, saved, and found by search with real spaces |
| F-19 | ✅ | a 2480×3508 PNG placed at 0,0–595×842 pt; the saved file is the OCR fixture of F-21 |
| F-20 | ✅ | **Fixed in Stage 2b**: `forms=0` on the tile/page routes makes the engine skip `FPDF_FFLDraw` while the 양식 overlay is mounted, so each field is rendered exactly once. `cargo test form_forms_flag_suppresses_only_the_widgets` walks all 2 002 770 px: **0** differ outside a widget rect, the filled field has >100 ink px with `forms=1` and **0** with `forms=0`; `form_forms_flag_tiles_match_the_full_render` proves the tile origin byte for byte; `protocol_forms_parameter_is_its_own_cache_entry` pins the route. Live at 100 % and 200 % (`form-100`, `form-200`), values read back after save → close → reopen. Trade-off: a pushbutton caption is not drawn while the overlay is up |
| F-21 | ✅ | scan with 0 extractable chars → OCR 300 DPI kor+eng, 91 words in 1.29 s → 484 chars; `search("대한민국")` hits before and after save+reopen; selection rects sit on the scanned lines (`s2-13`) |
| F-22 | ✅ | preview named the collateral, apply reported `removedObjects 2, verified true`, and the saved file's text lost the title (5 087 → 5 018 chars) |
| F-23 | ✅ | ⌘S in place: file rewritten (mtime + size changed), dirty cleared, `status.saved` toast, the new highlight present on reopen; Save As wrote every artefact of this table; close with unsaved changes shows 저장 / 저장 안 함 / 취소 (`s2-04`); `cargo test save_atomic_abort`, `save_keeps_encryption` |
| F-24 | ✅ | 3 pages at 150 DPI, sizes exactly `round(pt×dpi/72)` (rotated p0 = 1650×1275), estimate 3 916 110 B vs 3 916 112 B actual. **Fixed in Stage 2**: the payload shape made this fail from the UI |
| F-25 | ✅ | `text.txt` 11 538 chars with form feeds; `flat.pdf` has 0 annotations |
| F-26 | ✅ | **Fixed in Stage 2b**: `src/print/` is a print-only DOM — one full-width page image per printed page off `seepdf://page` at 150 DPI (`sk=208`), `print.css` hides the app shell, then `window.print()`. The macOS sheet shows **"All 3 Pages" / "Page 1 of 3"** with the document's own pages (`print-preview`), not the app chrome. The `print_prepare` + OS-handler path is kept as the alternative in the 인쇄 dialog (`print-dialog`). `vitest src/print/print.test.tsx`, 5 tests |
| F-27 | ✅ | dark ⇄ light switched live with no reflow (`s2-09`); `--page-paper/-ink/-field` moved into `tokens.css` and the last `#fff` literals in `shell.css` are gone |
| F-28 | ✅ | `node scripts/check-i18n.mjs` — ko 447 / en 447; English switched live (`s2-09`), and since Stage 2b the **native menu** follows the setting too (`menu-en`) |
| F-29 | ✅ | ⌘A / ⌘C / ⌘S / ⌘P / ⌘Z / ⇧⌘Z / ⌘G and the mode keys verified live; the full two-platform table is `vitest keymap.*`. **The native menu is Korean now** (Stage 2b §3): labels come from a ko/en table that is UI_SPEC §15.2 verbatim, and `set_settings` rebuilds the menu on the main thread when the locale changes — `menu-ko`, `menu-ko-file`, `menu-en` (live switch, no restart); `cargo test menu_labels_cover_every_item` |
| F-30 | ✅ | `scripts/perf-baseline.mjs` measures for real from four sources — a new `cargo run --release --example perf_bench`, the dev bridge against the running app (`openTimed` / `scrollThrough`), `ocr-accuracy.mjs` and the bundle report — and writes `docs/perf/baseline.{md,json}` with units, date and machine. Every row is inside its ARCHITECTURE §13 budget except `app.rss.500p.mb` (406.8 MB vs 400 MB, on a **debug** binary — flagged in the table; the engine's own RSS on the same document is 63.3 MB). CI runs `--check` on macos-arm64: ±20 % **gated** on the counts and sizes, **informational** on every wall-clock row, because a re-run on the recording machine moved `open.500p.ms` by −77 % (and +318 % on another pair) from page-cache state alone (Stage 2b §1.3). The wiring also uncovered that CI's bundle gate was measuring the **debug** `dist/` that `tauri build --debug` leaves behind — fixed (Stage 2b §1.4) |

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
| F-20 | `list_form_fields`, `set_form_field_value`, `reset_form` (P1), tile parameters `hl=1` and `forms=0` | §3 양식 tool strip, HTML overlay inputs |
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
Stage 0 and return `unsupported` until their row is scheduled. The six P2 commands (`set_outline`,
`create_link`, `update_link`, `delete_link`, `set_page_labels`, `get_page_labels`) belong to the rows of
"P2 status" below.

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
| P1-11 | **macOS Vision OCR** | `ocr_recognize_native` behind the same `OcrPage` contract (`objc2` 0.6 + `objc2-{foundation,core-foundation,core-graphics,vision}` 0.3.2, links against the Command Line Tools SDK); 인식 엔진 자동 / Apple Vision / Tesseract in the OCR sheet and 여러 파일 OCR. **Done (Stage 8).** |
| P1-12 | Korean stamp set (결재 / 승인 / 기밀), per-tool default styles, reading mode + full screen, hide-annotation-while-dragging (`set_annotations_hidden`) | small UI work on top of P0 machinery |

### P1 status — Stage 3

✅ verified by tests against real PDFium · ⚠️ partial, with the gap named. Details in
`docs/STAGE3_SECURITY_NOTES.md`.

| id | status | evidence |
|---|---|---|
| P1-1 | ✅ | 문서 정보 edits 제목/작성자/주제/키워드 (blank = remove), 보안 → 메타데이터 제거; lopdf `/Info` rewrite + XMP drop, Hangul as UTF-16BE, one undo step; `cargo test --test security metadata_*`, `docInfo.flow.test.tsx`. ⚠️ refused on encrypted documents |
| P1-2 | ✅ | 보안 → 암호 제거 writes an `-unlocked` copy (`FPDF_REMOVE_SECURITY = 4`); enabled only when the document is encrypted |
| P1-3 | ✅ | 보안 → 암호 설정: AES-256 V5/R6 copy, 4 permission checkboxes, verified reopen before write; `cargo test --test security password_*`. AES-256 files now report `r5` / `r6` (Stage 4) and 문서 정보 shows `AES-256 (R6)` |

### P1 status — Stage 4

Details in `docs/STAGE4_NOTES.md`.

| id | status | evidence |
|---|---|---|
| P1-4 | ✅ | 도구 → 워터마크 / 머리글·바닥글 (⌥⌘W): text with `{{page}}`/`{{total}}`/`{{date}}`/`{{filename}}`, multi-line, Hangul font, or a PNG/JPEG shared as one Form XObject; 9 anchors + margin, rotation, opacity, page range; upright on `/Rotate` pages; objects tagged `SeePDF:Stamp`; one undo step; `cargo test --test stamp` (6), `stamp.test.ts`, `stamp.flow.test.tsx`. ⚠️ no "remove stamps" command yet; a rotated stamp near an edge can be clipped |
| P1-5 | ✅ ⚠️ | 도구 → 압축: 300/150/96 DPI presets, cancellable estimate on a scratch copy, measured before/after + amber "no gain" warning, apply = one undo step (`stale` if the document changed); JPEG sources re-encoded as JPEG only when smaller; `cargo test --test compress` (5), `compress.flow.test.tsx`. ⚠️ images in Form XObjects / with SMask / shared between pages are skipped; non-JPEG images go through Flate and can grow; untested on encrypted documents |

### P1 status — Stage 5

Details in `docs/STAGE5_NOTES.md`.

| id | status | evidence |
|---|---|---|
| P1-6 | ✅ | 도구 → 문서 비교…: pick file B, 대소문자 무시, cancellable job (one progress per page pair); word-level Myers diff per page pair (pages paired by index), adjacent delete + insert = replace, one rect per line; full-window side-by-side view with red (A) / green (B) marks, 이전/다음 변경, 변경만 보기; B is closed on 닫기, Esc, window close and when A is replaced; `cargo test --test compare` (6) + lib diff tests (LCS vs brute force), `compare.flow.test.tsx`, `model.test.ts`. ⚠️ text only (no pixel diff, scanned pages have no words); pages paired by index, no page-insertion alignment; >2000 edits on one page collapse to one replace |
| P1-8 | ✅ | 설정 → 자동 저장 간격 (끄기 / 30초 / 1분 / 5분, default 1분): while dirty, a copy of the current state (appearance streams, encryption kept) goes to `$APPDATA/SeePDF/recovery/<uuid>.pdf` + `.json`, atomically, only when the generation moved; cleared on 저장 / 다른 이름으로 저장 / clean close; next launch offers 복구 (열기 / 삭제 / 모두 삭제 / 나중에), a recovered document saves via 다른 이름으로 저장 and is kept out of 최근 항목; copies of open documents are not offered; `cargo test --test recovery` (7), `autosave.test.ts` (fake timers), `recovery.flow.test.tsx`. ⚠️ recovered document's title shows the uuid file name until saved |

### P1 status — Stage 6a

| id | status | evidence |
|---|---|---|
| P1-7 | ✅ | 도구 → 여러 파일 OCR… (native 도구 menu, ⋯; no shortcut): a queue of PDFs on disk with per-file status (대기 / 진행 중 n/m 페이지 / 완료 / 건너뜀 / 실패 + 이유 / 취소됨), shared 언어 / 해상도 / 텍스트가 있는 페이지 건너뛰기, output `<name>-ocr.pdf` beside the source or in a chosen folder, never over the source or an existing file (` (2)`, ` (3)`… via the new `path_exists`); one file at a time, pages in parallel on **one** tesseract pool for the whole batch; open → OCR → save as → close on success, failure and cancel; encrypted files ask 암호 입력 (dismiss = 건너뜀); 취소 stops and closes the current file, 시작 resumes; 닫기 leaves it running in the status bar (one `batchOcr` job, × cancels, toast at the end); window close cancels it. `queue.test.ts`, `batchOcr.flow.test.tsx` (3 files: done / encrypted skipped / damaged failed, (2) naming, cancel mid-queue closes the doc), `cargo test --test ocr_layer ocr_layer_batch_file_roundtrip`, `path_exists` lib test. ⚠️ one `ocr_apply` (= one history snapshot) per page, as in the OCR sheet; no real-app smoke yet |
| P1-10 | ✅ | 끄기 → 어둡게 → 세피아 from ⌃⌘N / Ctrl+Shift+N, 보기 ▸ 야간 모드 (new native item) and a `moon` button in the status bar; the filter is on `.page-bitmaps` only; night bitmaps have a transparent clear colour and no engine colour transform (`cargo test --test render render_night_is_transparent_not_inverted`), the page shell paints `--night-paper` (= the filtered white) / `--sepia-paper` (sepia multiplies onto it), `--page-*` re-scoped so form text, handles and the text editor stay readable; marks screen on dark only; thumbnails stay day-rendered. `night.test.tsx` (cycle, key + button, filtered layer only, `night=1` requests, CSS contract). ⚠️ not persisted across launches (UI_SPEC does not ask for it); baked annotation colours shift under the invert (by design, ARCHITECTURE §3.4) |

### P1 status — Stage 6b

Details in `docs/STAGE6B_NOTES.md`.

| id | status | evidence |
|---|---|---|
| P1-9 | ✅ | 서명 만들기 has 그리기 · 입력 · 이미지 tabs. 입력: a name in 필기체 / 손글씨 / 정자체 (system-font stacks, nothing downloaded) → transparent PNG → `write_signature_image` (`$APPDATA/SeePDF/signatures/sig-<hash>.png`) → placed through the existing `stamp { image: { path } }` at its own aspect. 저장된 서명: 이 서명 저장 keeps drawn strokes / typed text + style in `Settings.signatures` (≤ 10, never evicts, duplicates ignored, lenient serde), one click places, × deletes. `signature.flow.test.tsx`, `signatureLibrary.test.ts`, `stampPicker.flow.test.tsx`, `app::signatures` lib tests (3), `ipc::types` serde test, `cargo test --test annot annot_stamp_png_keeps_transparency`. ⚠️ Hangul names fall back to the system Korean font (only 정자체 looks designed for it); a typed/image signature is a Stamp, not tagged `SeePDF:Signature` |
| P1-12 | ✅ | **도장 선택** (arming 도장, or 도장 변경… in the panel): 결재 / 승인 / 기밀 (인주 red, heavier border, Hangul label from the bundled subset via the document-level token — one embed per document, coverage-checked) + APPROVED / FINAL / DRAFT / CONFIDENTIAL; the ghost shows the label; `/Subj` = the id (`cargo test --test annot annot_stamp_korean_builtin`). **도구별 기본 스타일**: each tool has a built-in style, a panel edit with nothing selected becomes that tool's default in `Settings.toolDefaults` (partial, debounced), 이 스타일을 기본값으로 on a selection sets the drawing tool's, 기본값으로 재설정 (`toolStyles.test.ts`, `Inspector/defaults.test.tsx`). **읽기 모드** (⌃⌘R / F8 / `view.readingMode`) hides title bar, sidebar, tool strip, inspector, status bar; Esc leaves it; **전체 화면** (⌃⌘F / F11 / `view.fullScreen`) = `Window.setFullscreen`, combines, Esc leaves both (`readingMode.test.tsx`). **Hide while dragging**: a 선택-tool move/resize hides the annotation (`set_annotations_hidden` + per-page `vn` in the bitmap URLs), paints it in the overlay, holds the patches, and on drop / cancel / tool switch / unmount unhides **before** the single `update_annotation` (`dragHide.test.ts`, 8 cases incl. failures). ⚠️ image stamps and foreign stamps are not hidden (the overlay cannot repaint them); no real-app smoke |

### Stage 7 — 편집 mode UI and paragraph editing

Until Stage 7 the page-object commands behind F-17 / F-18 / F-19 were verified through IPC only: the 편집
tool strip was not wired, so none of them was reachable from the app. Stage 7 wires them and pulls
paragraph reflow forward from P2. Contract: `IPC_CONTRACT.md` §7.4b; UI: `UI_SPEC.md` §6–7.

| item | status | evidence |
|---|---|---|
| 편집 → 선택 | ✅ | hover outline, click / shift-click, drag = one `transform_object` on drop, corner scale, ⌫ delete, arrow nudge; read-only objects show the reason badge and refuse; `edit.flow.test.tsx` (mock) |
| 편집 → 텍스트 수정 = paragraph edit | ✅ | `probe_paragraph` groups text objects into lines and lines into a paragraph (size ±8 %, leading ±20 %, short-line end, de-hyphenation, justify ≥ 3 lines); `edit_paragraph` re-flows into the box (break at spaces, char-break on overflow, first-line indent, left / center / right / justify), keeps the paragraph font when it covers every glyph, else the bundled Hangul font after consent; one undo step `undo.paragraphEdit`; stale check; `cargo test --test paragraph` (6) + 3 line-break unit tests, tracemonkey's justified body (10 lines) detected. Stage 9: a longer paragraph pushes the content below it in its column (shorter pulls it up); obstacles (full-width figures, widgets, running footer, header/footer stamps) stop the push and the UI offers fit / overlap / keep editing; annotations and pending redaction marks move with the text; one undo step; `cargo test --test paragraph_flow` (18), `flow.flow.test.tsx`. ⚠️ edited paragraph goes to the end of the page text order (PDFium content generator); mixed styles collapse to the dominant one (UI warns); kerning / horizontal scale / synthetic italic not kept; rotated text refused |
| 편집 → 텍스트 추가 / 이미지 추가 | ✅ | same editing box → `add_text_object`; drag or click → PNG/JPEG picker → `add_image_object` (a click places a 240 pt box) |
| 편집 → 영역 표시 (F-22 redaction) | ✅ | drag marks an area, a click on text marks its run, × / ⌫ removes; marks are pending per page (hatched), survive tool switches, are dropped after a confirm when leaving 편집 and on undo / redo / page ops / OCR; the panel previews each marked page (`redact_preview`, debounced) with collateral text in amber + why, disables 적용 with the reason on form fields, confirms (destructive, "undo only until you save"), then `apply_redactions` per page in order, stopping on the first error; `verifyFailed` → "document restored" toast; text-selection menu 영역 표시로 표시. `redact.flow.test.tsx` (7, mock). ⚠️ a drag does not snap to the runs it crosses (the preview names them instead); no real-app smoke |

Stage 7 gates: cargo test 198/0, vitest 434/434. Stage 9 (paragraph flow): see `docs/STAGE9_NOTES.md`.
Real-app check 2026-09-28 (dev bridge, real engine): paragraph edit / Korean substitution / redaction /
stamps / compress / compare / recovery / Vision OCR / 편집 + night + reading mode all pass. 영역 표시 UI: vitest 441/441, i18n ko/en 695,
critical path 112.9 kB gz; the redaction flows are their own lazy chunk (3.4 kB gz) next to 편집 (4.7 kB gz).

## P2 — after v1

document tabs in one window · ~~link creation and go-to-page
destinations~~ · ~~outline/bookmark editing~~ · ~~page labels~~ (all three landed — see "P2 status") · crop and resize
pages · form field authoring · word-level redaction (re-creating surviving glyphs) and the raster
fallback for XObject text · glyphless CID OCR font (`FPDFText_LoadCidType2Font`) instead of the bundled
subset · incremental save · Windows.Media.Ocr opportunistic path · export to Word/hwp (third-party
engine, licence review first) · annotation replies/threads and summary export · multi-file search ·
split view · TTS · auto-update.

### P2 status — crop / resize, Bates, annotation summary, read aloud

✅ verified by tests against real PDFium (and the real `say` on macOS) · ⚠️ partial, with the gap named.
Contract: `IPC_CONTRACT.md` §7.3a, §7.4a, §7.7a, §11a; UI: `UI_SPEC.md` §8–10, §12, §15.20b.

| item | status | evidence |
|---|---|---|
| **Crop pages** | ✅ | 페이지 mode 자르기… (rail, cell / thumbnail menu): drag a box with 8 handles / move / redraw, 여백 (pt) fields, 남는 크기 in mm, 여백 자동 감지 (render ≤ 900 px → non-paper bounds + 6 pt), 선택한 페이지 / 모든 페이지 with the same margins relative to each page's crop box, 원래대로 (`crop: null` → crop = media); `set_page_boxes` = one undo step `undo.pageCrop`, `DocInfo` geometry exact for every changed page; rotated pages take margins as seen. `cargo test --test page_boxes` crop cases (save → reopen crop box equals, render is 468 × 600 with ink inside, rotated margins, reset, media clips crop, refusals roll back), `boxes` lib tests, `crop.test.ts`, `crop.flow.test.tsx`. ⚠️ the preview shows the current crop, so enlarging past it needs 원래대로 first; a page inheriting both boxes from the page tree resets to PDFium's page box |
| **Resize pages** | ✅ ⚠️ | 페이지 크기 변경…: A4 / 레터 / A3 (page orientation kept) or mm, 확대/축소 (uniform, centred) or 가운데 배치; `resize_pages` = one undo step `undo.pageResize`; content via `FPDFPage_TransFormWithClip` (objects stay editable — Form XObject wrapping rejected), annotations follow (`FPDFPage_TransformAnnots` + quads / ink / popups by hand). `cargo test --test page_boxes` resize cases (A4 → Letter: rect scaled by 0.9407 and centred, aspect kept, save → reopen, undo; centre mode offsets only; highlight quad + ink points mapped; `/Rotate 90` keeps landscape; resize then stamp survives save → reopen). Found and worked around a pdfium-render 0.9 use-after-free: `reload_in_place()` leaves the page's boundaries / objects / annotations collections on the closed handle, so the page is reopened. ⚠️ a shading pattern shared by several resized pages is transformed once per page; `/L`, `/Vertices`, `/CL` of third-party annotations keep old values (their appearance moves) |
| **Bates numbering** | ✅ | `{{bates}}` token + `batesStart` / `batesDigits` / `batesPrefix` / `batesSuffix` on `StampSpec` (serde defaults, not serialised at default), one-pass token expansion, numbers count the stamped pages; 워터마크 / 머리글·바닥글 dialog: Bates 번호 매기기 preset (footer / 오른쪽 아래, or header / 오른쪽 위), token chip, 시작 번호 / 자릿수 / 접두어 / 접미어, sample over the range, preview; `remove_stamps` by role unchanged. `cargo test --test stamp stamp_bates_numbers_every_page_and_is_extractable` (14 pages ABC000101…ABC000114, one per page, extractable after save → reopen, range 3–5 → 000001-K…000003-K, footer removal, bad digits / prefix refused), `stamp` lib tests, `bates.test.tsx` |
| **Annotation summary export** | ✅ | `export_annotation_summary` TXT / CSV / Markdown: page, label, kind (i18n), author, created / modified (local time), colour, contents, quoted text of markups from the cached text layer; CSV RFC 4180 + UTF-8 BOM + formula guard; 주석 sidebar 내보내기… and 내보내기 ▸ 주석 목록 (range, format, `<name>-주석 목록.<ext>`). `cargo test --test annot_summary` (5 annotations created out of order → 5 rows in page order, `Trace-based` quoted, Korean note with comma / quotes / line break round-trips, BOM, page filter, third-party highlight), `summary` lib tests, `annotSummary.flow.test.tsx` |
| **Read aloud (TTS)** | ✅ ⚠️ | `tts_speak` / `tts_stop` / `tts_status`: macOS `say` (stdin, Yuna for Korean, `-r`), Windows PowerShell `System.Speech` (UTF-8 stdin, culture voice, no window), `unsupported` elsewhere; one utterance app-wide, killed on stop, on a new speak, on window close and on `RunEvent::Exit`. UI: 읽어 주기 in the text-selection menu, 이 페이지 읽어 주기 (canvas menu, 보기 menu, ⋯), floating bar 속도 (restarts) / 정지, status-bar indicator, polling ends it. `cargo test --test tts` (fake speaker: speak / replace / stop / finish / drop / unsupported; macOS: `say -v ?` lists a Korean voice, a long text speaks and stops, a short one renders audio to a file — silent), `tts` lib tests, `tts.flow.test.tsx`. ⚠️ the Windows path is compiled and unit-tested (script, base64, rate) but not run on Windows yet; no word highlighting; 속도 restarts from the beginning |
### P2 status — outline editing, links, page labels

✅ verified by tests against real PDFium (write → save → reopen → read back) and the mock-backed UI flows ·
⚠️ partial, with the gap named. Contract `IPC_CONTRACT.md` §7.10, UI `UI_SPEC.md` §4.1 / §6 / §7 / §8 / §10,
design `ARCHITECTURE.md` §6.7. All three are lopdf rewrites checked by PDFium's own readers (one undo step each).

| item | status | evidence |
|---|---|---|
| 목차 편집 (`set_outline`, `undo.outlineEdit`) | ✅ | 목차 tab → 편집: 현재 페이지 추가 (page + y of the current view), rename inline (Enter / double-click), delete (Delete / ⌫, subtree included), 들여쓰기 / 내어쓰기 (Tab / ⇧Tab), ⌥↑ / ⌥↓ and drag (before / after / inside), 목적지를 현재 보기로; 완료 = one call with the whole tree, 취소 = nothing written, toast + 실행 취소. Engine: old items deleted, `/Count` (+open / −closed) and `/First` `/Last` `/Prev` `/Next` `/Parent` consistent (inspected with lopdf), Hangul titles UTF-16BE, `/XYZ` or `/Fit` dests, `/A /URI` web nodes; `get_outline` (now a raw `FPDFBookmark_*` walk) reads back exactly what was written, `open` and `url` included; TAMReview's heading dest still `{x:0,y:806}`; undo restores `outline-labels.pdf`'s 7 nodes; `[]` removes `/Outlines`. `cargo test --test structure structure_outline_*` (3), `outlineEdit.test.ts` (6), `outline.flow.test.tsx` (5). ⚠️ named destinations become explicit, non-GoTo/URI actions become title-only, item colour/style dropped; no multi-select in the editor |
| 링크 (`create_link` / `update_link` / `delete_link`, `kind: 'link'` with `uri` / `dest`) | ✅ | 편집 → 링크 (`link-2`): drag ≥ 4 pt → popover 페이지로 이동 (label or number, 현재 위치 사용) \| 웹 주소 → one `create_link`; click selects, the panel re-targets (`update_link`) / 링크 열기 / 삭제, ⌫ deletes; a link selected in 주석 mode gets the same panel; 읽기 mode: hover outline + tooltip, click → page jump or confirm → opener plugin (http(s) / mailto only). Engine: web links through PDFium (`FPDFAnnot_SetURI`, `/Border [0 0 0]`, `/F 4`, `/NM`), page links and anything that adds / drops a `/Dest` through lopdf, checked with `FPDFLink_GetDest`; reads `Annot.dest` via `FPDFLink_GetDest`; save → reopen → read back; undo per step. `cargo test --test structure structure_link_*` (3), `links.flow.test.tsx` (6). ⚠️ links are drawn with no border (other viewers show nothing until hovered); no multi-rect (`/QuadPoints`) links from the UI; 읽기-mode clicks rely on the annotation list of the page being loaded |
| 페이지 레이블 (`set_page_labels` / `get_page_labels`, `DocInfo.pageLabels`) | ✅ | 페이지 mode rail and 문서 정보 → 페이지 레이블…: rows 시작 페이지 · 스타일 (decimal / roman / ROMAN / alpha / ALPHA / 번호 없음) · 접두사 · 시작 번호, inline errors, live preview of every page, 모두 제거; 적용 = one call, toast + 실행 취소. Shown in 축소판, 페이지 mode chips, 목차's page column, the link form and the status-bar box (label or number accepted; `(n / total)` beside it). Engine: `/PageLabels /Nums` (leading decimal range added when needed, `/St` omitted when 1), every page checked against `FPDF_GetPageLabel`; `get_page_labels` reads `/Nums` and `/Kids`; save → reopen → labels; undo restores the fixture's `i, ii, 1, 2, App-A, App-B`; `[]` removes. `cargo test --test structure structure_labels_*` (2) + 8 formatter / tree unit tests, `pageLabels.test.ts` (6), `pageLabels.flow.test.tsx` (2) |
| encrypted documents | ✅ | `set_outline`, `set_page_labels`, `get_page_labels` and every `/Dest` link change answer `unsupported` with no undo entry (like metadata); web links, rect moves and deletes still work; a failed PDFium check rolls back (`structure_refused_on_encrypted`, `structure_failed_check_rolls_back`). UI: 편집 disabled, the dialog read-only, 페이지로 이동 disabled, each with `structure.encrypted` |

P2 gates (2026-09-28): `cargo check --all-targets` 0 warnings · `cargo test --release` 303 / 0 (`tests/structure.rs`
10) · vitest 583 / 583 (75 files) · i18n ko 820 / en 820 · critical path 115.7 kB gz of 120 (was 113.2); the editor
(3.9 kB gz), the dialog (2.0 kB gz) and the link form are lazy chunks. ⚠️ Not yet driven in the real app (dev
bridge): only against real PDFium in cargo tests and against the mock in vitest.

### P2 status — threads, multi-file search, split view

✅ verified by tests (Rust against real PDFium, vitest against the mock) · ⚠️ partial, with the gap named.
Contract: `IPC_CONTRACT.md` §5 (split viewport hint), §7.1 (threads), §11 (`list_pdf_files`); UI: `UI_SPEC.md` §4, §5,
§7, §8, §10, §12, §13, §15.20b.

| item | status | evidence |
|---|---|---|
| Annotation replies / threads | ✅ ⚠️ | `reply_annotation` writes a `Text` reply with `/IRT` = an indirect reference to the parent + `/RT /R` through lopdf (`registry::mutate_bytes`, one undo step `undo.annotReply`), an empty `/AP` so no viewer draws it over the parent (kept after an edit), the parent's colour, `/T` = 설정 ▸ 작성자; read back as `Annot.inReplyTo` (two-pass list, so order does not matter); `delete_annotations` takes a deleted annotation's replies with it, transitively; a rebuilt parent (text box / markup) keeps its thread. UI: the 주석 tab groups replies under their top-level annotation (count badge, fold), the 메모 popover lists the thread with edit / delete per reply and a reply box, 답글 on any other annotation opens a thread popover, 답글 in the canvas and 주석-row context menus, deleting a parent with replies asks first. `cargo test --test thread` (7: reopen with PDFium **and** lopdf → `/IRT` is the parent's reference and `/RT /R`; nested replies; invisible on the page even after an edit (pixel compare); delete a branch / the whole thread + undo; undo removes a reply; a rebuilt parent keeps its replies; a reply to a foreign highlight; errors), `threads.test.ts`, `thread.flow.test.tsx` (reply flow, edit, delete, thread popover, deletion confirm, encrypted refusal), `invokeShape.test.ts`. ⚠️ refused on encrypted documents (lopdf would need the owner password, like `set_metadata`); a reply always answers the thread root in the UI (nested replies from other apps are shown, flattened); no summary export yet |
| Multi-file search | ✅ | 편집 ▸ 여러 파일에서 검색… / ⋯ / the 검색 panel: 파일 추가 / 폴더 추가 (`list_pdf_files`, recursive, hidden skipped, ≤ 2 000) / 목록 비우기, query + 대소문자 구분 + 단어 단위로; one `multiSearch` job in the status bar (× cancels): each file opened beside the window's document (encrypted → 암호 입력, dismiss = 건너뜀), `search_start` from page 0 to `done`, closed on success, failure and cancel; the window's own document is searched in memory; results grouped by file with counts and ±40-character context; a click opens the file through `openPath` (unsaved guard, the search's password tried first) and hands the hits to the 검색 panel (`SearchController.show`: highlights, current hit scrolled to, ⌘G). `multiSearch.flow.test.tsx` (3 files incl. an encrypted one skipped, every opened file closed, click → file open with the panel's hits; cancel mid-way closes the file being read and cancels its search job; 폴더 추가 dedup), `list_pdf_files_recurses_sorts_and_skips_hidden` lib test |
| Split view | ✅ ⚠️ | 보기 ▸ 분할 보기 (native item), ⌥⌘S / Ctrl+Alt+S, status-bar `columns-2`: two panes of one document, 좌우 ⇄ 위아래, each with its own zoom / zoom mode / rotation / current page / scroll (`viewStore` `PaneView` ×2 — the top level is the focused pane's, so every existing reader acts on the pane in use; layout and night stay document-level), one shared `TileManager` (per-pane clients, one in-flight budget) and one engine viewport hint covering both panes; annotation / form / 편집 layers only in the focused pane (a click focuses the other); 동기화 스크롤 by page delta; Esc (nothing else to cancel), the menu or another document closes it without remounting the main pane. `viewStore.split.test.ts` (7), `split.test.ts` (TileManager clients, page-unit position, sync + viewport union), `split.flow.test.tsx` (open, second pane to page 5 while the first stays, tools follow focus, per-pane zoom, Esc closes). ⚠️ the divider is fixed at 50 %; a drawing tool needs a second click after focusing the other pane; no real-app smoke yet |

P2 gates (2026-09-28): `cargo check --all-targets` 0 warnings · `cargo test --release` 294/0 (lib 100, `thread` 7) ·
vitest 590/590 (76 files) · i18n ko/en 794 · critical path 116.4 kB gz of 120 (여러 파일에서 검색 is its own lazy chunk;
the thread UI rides in the annotation chunks).

Integration of the three P2 branches (2026-09-28): `cargo test --release` 345 / 0 · vitest 654 / 654 (87 files) ·
i18n ko / en 966 · critical path 108.1 kB gz of 120 (120.9 right after the merge): the viewer (scroller, tile
manager, panes, layer slots) and the sidebar's 축소판 / 목차 / 검색 panels are lazy chunks, fetched when the window
is idle after start-up; their CSS stays in the entry stylesheet, in its old place in the cascade.

### v0.3 status

✅ verified by tests (Rust against real PDFium, vitest against the mock) · ⚠️ partial, with the gap named.
Contract: `IPC_CONTRACT.md` §7.4b (reading order, rotated text), §7.4c (`ungroup_object`), §7.5 (redaction); UI:
`UI_SPEC.md` §4, §6 (텍스트 수정), §7 (영역 표시), §12, §15.13.

| item | status | evidence |
|---|---|---|
| R1 Redaction blanks the pixels of partly covered images (scans) — pkg1 | ✅ ⚠️ | Every image pixel whose footprint overlaps a mark (unit-square pixel through the image matrix, separating-axis test: rotated / flipped / scaled images exact) is set to black in the object's own bitmap and written back into the same object (matrix, clip, graphics state kept): `/DCTDecode` re-encoded as JPEG q 92 through `FPDFImageObj_LoadJpegFileInline` like `compress.rs`, anything else `SetBitmap` (Flate). Verified on the data read back (Flate exactly 0; JPEG ≤ 96 per channel, mean ≤ 16) — `verifyFailed` otherwise — plus a per-pixel rasterisation check that nothing else changed (palette / inverted `/Decode` → removed whole instead). Preview `imageObjects[].blank`; the panel says 이미지에서 표시한 부분을 검게 지웁니다 / 이미지 전체가 제거됩니다. `tests/redact.rs`: `redact_blanks_partially_covered_image_pixels` (full-page scan-like image: every pixel under the mark 0 and every other pixel identical **in the saved file**, the region renders black, the rest of the page is pixel-identical, the old image stream is not in the file), `redact_blanks_a_jpeg_image` (stays `/DCTDecode`), `redact_blanks_a_rotated_image` (30°, anisotropic scale), `redact_removes_a_transparent_image_whole` (`/SMask`), `redact_blank_of_a_shared_image_touches_only_its_page`, `image::tests` (3). ⚠️ Removed whole instead of blanked: images with an `/SMask` / `/Mask`, stencil masks, non-gray 1-bit, `/Indexed` / `/Separation` / `/DeviceN`. A large straight-segment path partly under a mark (table grid, frame) stays under the box — PDFium has no per-object clip, so "clip the covered area" is not implementable through its API; small or curved paths (outlined glyphs, logos) are removed. One image XObject drawn twice on the **same** page, only one copy under a mark, comes out blanked in both (PDFium shares the decoded image — over-blanking, never under). After a blank the document is reloaded from the bytes a save writes, so another *page* drawing the same image shows it unblanked, exactly as the file holds it (`redact_blank_of_a_shared_image_touches_only_its_page`) |
| R2 Word-level redaction — pkg1 | ✅ ⚠️ | A partly marked text object is split: the object is rewritten to its first unmarked run (`set_text` through the font's `/ToUnicode`, origin moved, every glyph pinned with `FPDFText_SetPositions`, which PDFium writes as a `TJ` — kerning / `Tc` / `Tw` survive) and keeps its place in the stream; further runs are copies moved out of a second parse (clip, colours, marks kept). Each run must read back at the same tight boxes (± 0.5 pt) from the in-memory text page, and again after regeneration (else the batch rolls back and retries once with that object whole, reported in `RedactBatchResult.collateral` → toast). Fallback (whole run, collateral in the preview): Type3 (no font program — PDFium's writer drops Type3 text), no usable `/ToUnicode`, off-baseline runs. Drags are no longer snapped to whole runs (`redact.flow.test.tsx`). `tests/redact.rs`: `redact_word_keeps_the_rest_of_its_run` ("Gal" goes, "Andreas" extractable at the same boxes after save), `redact_middle_of_a_run_leaves_both_sides` (two runs), `redact_splits_a_hangul_run` (CID / Identity-H 한글 subset: 900101-1234567 goes, 홍길동 / 서울 stay), `redact_type3_run_falls_back_to_whole_removal`, `redact_preview_reports_split_not_collateral`; `split::tests` (3). ⚠️ Copies for the second and later runs of one object are new objects in PDFium's stream order (drawn after the rest of their stream); a font whose `/ToUnicode` maps two codes to one character re-encodes to the first code (caught by the read-back when the glyph differs) |
| R3 Search-and-redact by keyword and Korean PII pattern — pkg1 | ✅ | 영역 표시 panel ▸ 검색해서 표시: 검색어 + 주민등록번호 · 전화번호 · 이메일 · 계좌번호, 모든 쪽 / 현재 쪽 / 쪽 지정, over each page's text layer (`get_text_layer`, fetched once per page, cancellable); every match's character boxes become labelled marks; 표시 목록 lists them (a match spanning two lines is one entry), each removable (×) before 적용, which is the existing one-step batch. 검색 panel ▸ 모든 결과를 영역 표시 marks every hit. Matchers in `src/edit/redactPatterns.ts` (pure): RRN with / without hyphen and a real YYMMDD, `2024-01-01` never matches; mobile / landline / 070 / +82; ASCII and 한글 e-mail addresses without swallowing a trailing 조사; grouped (10–16 digits) and bare (11–14) account numbers that are not phone numbers; overlapping matches merged. `redactPatterns.test.ts` (11), `redactSearch.flow.test.tsx`: search 홍길동 → 모든 결과를 영역 표시 → 적용 → the text layer of both pages no longer has it; the panel finds RRN + e-mail, one entry removed from 표시 목록 survives the apply; scope 현재 쪽 / 쪽 지정 / a bad range |
| R4 Edit and redact text inside Form XObjects (그룹 해제) — pkg1 | ✅ ⚠️ | `ungroup_object` (`engine/objects/ungroup.rs`): children detached (`FPDFFormObj_RemoveObject`), transformed by the form matrix (clip included), colours re-set as RGB (PDFium writes only DeviceRGB / Gray colours — an ICC / CalRGB box came out black), inserted at the form's index; one undo step 그룹 해제; `unsupported/groupTransparency` for a group with transparency. 텍스트 수정 on text in a group asks 그룹 해제 후 편집할까요? and opens the editor. Redaction lists `groups` in the preview; 적용 confirms once (그룹을 해제하고 적용할까요?) and sends `ungroup: true` — the groups are ungrouped (nested, up to 8 levels) in the same undo step, so verification passes; without it the pre-v0.3 refusal stands. `tests/redact.rs`: `ungroup_object_keeps_the_page_identical` (form with text, a `/CalRGB` box and a line: render diff mean < 1/255, in memory and after save; stale / not-a-group refusals; undo restores the group), `redact_inside_a_group_with_ungroup` (refused without, one step with, "Gal" gone, neighbours in place), `redact_tamreview_with_ungroup`; `edit.flow.test.tsx` (ask → 취소 / 그룹 해제 → editor), `redactSearch.flow.test.tsx` (`ungroup: true` after the one confirm). ⚠️ PDFium does not write the `Tc` / `Tw` / `Tz` text state of text it did not create, so after ungrouping, lines justified by character spacing tighten (`TAMReview.pdf` p.2: three lines; its fonts have no `/ToUnicode`, so its text is not editable anyway); a clip set on the page before `/Form Do` is not re-applied to the children |
| R5 Paragraph edit keeps the reading order; rotated text can be paragraph-edited — pkg1 | ✅ ⚠️ | Reading order: the write leaves a marked placeholder (an emptied copy of the first run, moved out of a second parse so it keeps its content stream) and marks the new objects; after the mutation a `lopdf` pass (`flow::restore_reading_order`) moves the marked blocks to the placeholder (only streams holding a marker are decoded — inline images elsewhere are never touched), PDFium re-verifies, and the document is replaced without a history entry of its own (one undo step, pixel-identical render). Rotated text: probed again in a frame rotated like the text, laid out there, drawn rotated back; nothing moves for it. `tests/paragraph_order.rs`: `paragraph_edit_keeps_reading_order` (paragraph 2 of 3 edited and grown → text order 1, 2, 3 in memory and after save; no marker in the file; render identical; one undo), `rotated_paragraph_edits_in_place` (90°: probe `inPlace`, edit, glyphs drawn rotated in the old band); `flow::order_tests` (2); the Stage 9 flow suite (29) passes with the new object order. ⚠️ Skipped for encrypted documents (lopdf cannot write them back — the paragraph stays last, as before) and when the pass fails (logged); each committed edit costs one serialise + lopdf round trip + reload; a rotated paragraph is edited in an upright box over its page-aligned bounds, and never pushes content |
| R6 Sidebar context menus — pkg1 | ✅ | 목차 item: 이동 · 하위 항목 모두 펼치기 · 하위 항목 모두 접기 (the node and every node below it). 검색 result: 이동 · 복사 (the match) · 이 결과 형광펜 (the 형광펜 tool's markup over the hit's line rects, its remembered style) · 영역 표시. `redactSearch.flow.test.tsx`: 형광펜 creates a highlight whose quads are the hit's rects (mock), 복사 writes the match, 이동 makes it current, 영역 표시 marks it; the outline menu folds / unfolds the branch and navigates |
| I1 Korean IME: Enter while composing — pkg1 | ✅ | `SearchPanel.tsx` and `SignatureDialog.tsx` return on `isComposing` / `keyCode 229` before handling Enter (as `OutlineEditor`, `FormLayer`, `TextEditor` already did); the new 검색해서 표시 field too. `redactSearch.flow.test.tsx`: a composing Enter neither runs the search nor steps to the next hit, a plain one does; a composing Enter does not confirm a typed signature, a plain one does |

v0.3 pkg1 gates (2026-09-29): `cargo fmt --check` ✓ · `cargo clippy -- -D warnings` ✓ · `cargo clippy --target
x86_64-pc-windows-msvc -- -D warnings` ✓ (on macOS with an `llvm-rc` stand-in) · `cargo build --release --tests` ✓ ·
`cargo test --release` 380 / 0 · vitest 744 / 744 (99 files) · i18n ko / en 1012 · critical path 110.3 kB gz of 120
(the new UI code is in the 편집 / Inspector / sidebar chunks). ⚠️ Not yet driven in the real app: only against real
PDFium in cargo tests and against the mock in vitest.

---

## Explicit non-goals for v1

| Not in v1 | Why |
|---|---|
| Office / hwp import and export | needs a third-party conversion engine; licensing must be settled first |
| Digital signature certificates (공인전자서명), PKI validation | a separate compliance workstream (see open question 3 of the UX research) |
| XFA forms | not built into this pdfium; documents are opened read-only with a banner |
| PDF/A, preflight | Acrobat-tier features our users did not ask for (Bates numbering landed in P2) |
| Form field authoring, JavaScript actions, field calculation | pdfium exposes no authoring API and no JS engine is shipped |
| Document tabs | one window per document has the same capability and no tab-state code |
| Cloud sync, accounts, telemetry, AI features | the product promise is a local, offline, quiet tool |
| Bookmark/outline editing, metadata writing, page labels in P0 | pdfium cannot write any of them; deferred until `lopdf` lands (P1). Metadata landed in Stage 3 (P1-1); outline editing, link destinations and page labels landed as P2 (see "P2 status") |
| Incremental save | a second save path for a ≤ 5 ms/MB win; the full rewrite already meets the target |
| Polygon / polyline / cloud / callout / measurement annotations | `FPDFPage_CreateAnnot` returns NULL for Polygon and Polyline |
| Real `/Line` annotations | same reason; v1 writes Ink with a `/Subj` tag and says so in the docs |
| Partial-stroke ink erasing | v1 erases whole strokes |
