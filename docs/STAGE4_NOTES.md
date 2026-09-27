# Stage 4 — watermark / header-footer, compress, r5/r6 (P1-4, P1-5)

Status: landed in the working tree, verified, not committed yet (2026-09-28).

## 1. What landed

* **P1-4 stamps.** 도구 → 워터마크 / 머리글·바닥글 (⌥⌘W / Ctrl+Alt+W, the native 도구 menu and the ⋯
  overflow menu) opens one dialog with three roles. 워터마크, 머리글 and 바닥글 differ only in their
  defaults and in the undo label. The dialog has:
  * a text source (multi-line, token chips `{{page}}` / `{{total}}` / `{{date}}` / `{{filename}}`,
    size, colour swatches) or an image source (PNG/JPEG, width in pt, height from the aspect ratio);
  * opacity, rotation (watermark only), a 3×3 anchor grid + margin, and a page range;
  * a CSS preview of page 1.
  `add_stamp` is one undo step: `undo.watermark` or `undo.headerFooter`.
* **P1-5 compress.** 도구 → 압축 has 300 / 150 / 96 DPI presets and a page range.
  * 예상 runs a cancellable job on a **scratch copy**, with progress per page.
  * The final `done` event carries a `CompressReport`: before/after bytes, a signed Δ%, and images
    downsampled / total. An amber warning shows when the result is not smaller.
  * 적용 replaces the document as one undo step (`undo.compress`).
  * Closing the dialog (취소, Esc, backdrop) cancels the job and discards the pending result.
* **r5 / r6.** `SecurityRevision` gains `R5` / `R6`. 문서 정보 shows the row 암호화 as `AES-256 (R6)`
  and so on, 없음 for unprotected documents, and 알 수 없음 otherwise.
* **Undo labels (contract §D).**
  * Every `undo.*` key the engine emits is now in `ko.json` and `en.json`. The check script walks
    every `"undo.*"` literal in `src-tauri/src`, and none is missing.
  * The title bar ↶ / ↷ tooltips read "실행 취소: 워터마크" (`historyLabel.ts`).
  * The mock now uses the real keys.
* **Shared `Swatches`.** The swatches component moved out of the Inspector into `src/app/Swatches.tsx`
  so that the stamp dialog can use it too.

## 2. Design decisions

**Stamps are page content, not annotations.**
* Text becomes one text object per line. Latin-1 lines use Helvetica and other lines use the bundled
  Hangul subset, the same as `objects::add_text`.
* An image is drawn once on a one-page scratch document and imported with `FPDF_NewXObjectFromPage`
  as one Form XObject, which every stamped page references. A logo on 300 pages is stored once.
* Every object is tagged with the content mark `SeePDF:Stamp` (`FPDFPageObj_AddMark`) so that a
  future "remove stamps" command can find them.

**Visual-space geometry.**
* The box is laid out on the page *as the user sees it*: the CropBox, with width and height swapped
  for `/Rotate 90/270`.
* The box is placed by anchor + margin, then rotated about its own centre by `rotateDeg`.
* `visual_to_user` then maps the box into unrotated user space, so a top-left header stays top-left
  and upright on a rotated page.
* **Sign:** +`rotateDeg` is counter-clockwise as seen, so the default 45° is the usual rising
  diagonal. CSS `rotate()` is clockwise, so the preview applies `rotate(-Xdeg)`. The verifier
  checked that both sides agree.
* Lines align to the anchored edge, or are centred for centre anchors. The preview now does the same
  (fixed during verification).

**Opacity.** Text opacity is fill + stroke alpha, which PDFium writes as an `/ExtGState` with
`/ca` / `/CA`. Image opacity is baked into the image's alpha channel (an `/SMask`), because PDFium
emits no graphics state for form objects.

**Compress never touches the open document.**
* `begin` serialises the document (the bytes a save would write) and loads them into a scratch
  `PdfDocument` held in `OpenDoc::compress_work`.
* The job is one `Lane::Background` command per page for a scan pass, one per page for a process
  pass, and one finish command. Tiles still interleave, and Cancel is honoured between pages.
* `finish` serialises the scratch copy, checks it with `save::verify_bytes`, and parks it as
  `compress_pending` (one per document) behind a `u64` token.
* `apply` is a `mutate_bytes` replace. It refuses with `stale` if the document's generation moved
  on after the estimate.
* A pending result is dropped by `replace`, by a newer estimate, by `discard`, and on close (it lives
  in `OpenDoc`).

**What compress skips.** These images count in `imagesTotal` but not in `imagesDownsampled`:
* images with transparency (SMask / Mask);
* 1-bit images and stencil masks;
* images inside Form XObjects;
* images whose encoded stream appears more than once. `FPDFImageObj_SetBitmap` would give every user
  its own copy, so the file would grow.

**Encoding.**
* A DCT or JPX original is re-encoded as JPEG q80 and swapped in with
  `FPDFImageObj_LoadJpegFileInline`, but only if the result is smaller.
* Everything else goes through `SetBitmap` as BGR24 or Gray8. PDFium always writes this as Flate,
  which can grow, and the report shows it.

**JobEvent.** `done` gains an optional `report` field instead of a new event type. `JobReporter`
now writes to a `JobSink` closure, so that `cargo test` can drive the real job without a Tauri
channel.

## 3. Gates (verification run, 2026-09-28)

| gate | result |
|---|---|
| `cargo check` | clean, 0 warnings |
| `cargo build --release --tests && cargo test --release` | 158 passed, 0 failed (incl. 6 in `tests/stamp.rs`, 5 in `tests/compress.rs`, lib unit tests incl. the stage-4 serde shape test) |
| `npm run typecheck` | pass |
| `npx vitest run` | 43 files, 334 tests passed |
| `node scripts/check-i18n.mjs` | ok, ko 556 / en 556 keys |
| `npm run build && node scripts/check-bundle-size.mjs` | ok; dist 636.8 kB of 2048; critical path 106.6 kB gz of 120; `StampDialog` 3.4 kB gz chunk; `CompressDialog` 6.0 kB raw chunk |
| `node scripts/perf-baseline.mjs --check --skip-app` | ok, every gated row within tolerance (engine + OCR measured; app rows need `tauri dev`) |

No real-app smoke test was run: `npm run tauri dev` needs a window and several minutes. The
evidence is the vitest flows against the mock and the cargo tests against real PDFium.

## 4. Files changed

Backend:
* New: `engine/stamp.rs`, `engine/compress.rs`, `engine/raw/object.rs` (marks, Form XObject, JPEG
  swap), `commands/stamp.rs`, `tests/stamp.rs`, `tests/compress.rs`.
* `ipc/types.rs`: stamp and compress types, `JobEvent::Done.report`, `SecurityRevision::R5/R6`.
* `engine/export/job.rs`: `JobSink`, `start_with`, `finish_with_report`.
* `engine/registry.rs`: `compress_work` / `compress_pending`, dropped on `replace`; `geom_from_page`
  is now public; r5/r6 mapping.
* `engine/mod.rs`, `engine/raw/mod.rs`, `commands/mod.rs`, `lib.rs`, `commands/{ocr,save}.rs`
  (`report: None`), `tests/security.rs`.
* `app/menu.rs`: 도구 → 워터마크 / 머리글·바닥글… item (added during verification).
* `docs/IPC_CONTRACT.md` §7.4a, §7.6a.

Frontend:
* New: `dialogs/StampDialog.tsx`, `stamp.ts`, `CompressDialog.tsx`, `compress.ts`,
  `app/Swatches.tsx`, `app/historyLabel.ts`, and tests (`stamp.test.ts`, `stamp.flow.test.tsx`,
  `compress.test.ts`, `compress.flow.test.tsx`, `historyLabel.test.tsx`).
* `ipc/{types,api,mock}.ts`, `DialogHost.tsx`, `dialogState.ts`, `dialogs.css`, `useCommands.ts`,
  `keys/keymap.ts` (`tools.stamp`, `MENU_ONLY_IDS`), `TitleBar.tsx`, `store/docStore.ts`,
  `Inspector/InspectorBody.tsx`, `docInfo.ts`, `DocInfoDialog.tsx`, `i18n/{ko,en}.json`,
  `docs/UI_SPEC.md`.

## 5. Fixed during verification

* `CompressDialog.tsx`: if 취소 was pressed or the dialog closed before the `JobId` came back, the
  job kept running to completion because nothing held its id. It is now cancelled as soon as the id
  arrives.
* `StampDialog.tsx`: the preview centred every line, while the engine aligns lines to the anchored
  edge. The preview now aligns them the same way.
* `app/menu.rs`: the native 도구 menu had no 워터마크 item. It is now there, with ⌥⌘W / Ctrl+Alt+W.

## 6. Still open

* **Rotated stamps near edges.** The anchor places the *unrotated* box, so a rotated watermark at a
  corner anchor can poke past the page edge and be clipped.
* **Compress skips** images inside Form XObjects, images with an SMask or Mask, and images shared
  between pages (§2). Scanned PDFs are usually unaffected. Some exported office files are.
* **Flate growth.** Non-JPEG images are re-encoded as Flate and can come out larger. The dialog warns
  in amber, but it does not stop 적용.
* **Compress on encrypted documents is untested.** The scratch copy reuses the open password;
  whether the serialise → reload round trip keeps the encryption has not been checked by a test.
* There is **no "remove stamps" command** yet. The `SeePDF:Stamp` mark is in place for it.
* The **native 편집 menu** shows plain 실행 취소 / 다시 실행, without the step name. Only the title
  bar tooltips name the step.
* The **preview** is a CSS approximation of page 1. It ignores the page's `/Rotate`, its CropBox and
  the image's real aspect ratio, and it shows no image thumbnail.
* `doc-changed` drops the undo labels as stale. The title bar re-fetches them on hover, one
  `get_document` call.
* No manual QA in the real app yet: check a watermark and a compress in the running app, and open
  the output in Acrobat and Preview.
