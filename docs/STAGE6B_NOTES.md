# Stage 6b — signatures (P1-9), Korean stamps, tool defaults, reading mode, hide-while-dragging (P1-12)

Status: landed in the working tree, verified, not committed yet (2026-09-28). Built in parallel with
Stage 6a (P1-7 batch OCR, P1-10 night mode); the shared files were edited with small anchored edits.

## 1. What landed

* **P1-9 서명.** 서명 만들기 has three tabs, 그리기 · 입력 · 이미지, and a 저장된 서명 list.
  * 입력: type a name (≤ 40 chars) and pick 필기체 / 손글씨 / 정자체. Each style is a CSS font stack of
    fonts the OS already ships (Snell Roundhand / Segoe Script, Bradley Hand / Segoe Print / Ink Free,
    AppleMyungjo / Batang). Nothing is downloaded. 확인 renders the name tight to its ink box on a
    canvas at 96 px, as a transparent PNG.
  * `write_signature_image(bytes)` writes the PNG to `$APPDATA/SeePDF/signatures/sig-<fnv64>.png`
    and returns the path. The 서명 tool places it through the **existing** `stamp { image: { path } }`
    spec, at the PNG's aspect (`setStampImage(id, image, aspect)`; 160 pt wide).
  * 이 서명 저장 (off by default) appends to `Settings.signatures`, at most 10. A drawn signature keeps
    its unit-space strokes, rounded to 4 decimals. A typed one keeps text + style and is re-rendered
    each time it is used. The list never evicts: when it is full, the box is disabled and says why.
    Saving an identical signature again does nothing. One click on a saved one places it; × deletes it.
  * 이미지 hands over to the file picker, as before.
* **P1-12 한국어 도장.** Arming 도장 now opens **도장 선택**: 결재 · 승인 · 기밀, then APPROVED · FINAL ·
  DRAFT · CONFIDENTIAL, then 이미지 선택…. The properties panel's 도장 변경… reopens it (and 서명 변경…
  reopens 서명 만들기).
  * The ghost that follows the cursor shows the label in the stamp's colour. A click places the stamp
    at its catalogue size (a 64 pt square for 결재 / 승인, 96 × 44 for 기밀); a drag sets the size.
  * The engine draws the three Korean stamps in 인주 red (`KOREAN_SEAL_RED` = 206,32,41) with a
    2.5 pt border. The label uses the **document-level** Hangul token (`OpenDoc::hangul_token_for`),
    so the font is embedded once per document and the text is coverage-checked. The label is shrunk
    to fit 84 % of the width and centred on its glyph box. `/Subj` is the builtin id, which is what
    `Annot.stampKind` reads back.
* **P1-12 도구별 기본 스타일.** Each drawing tool has its own built-in style: yellow 형광펜 at 40 %,
  blue 밑줄, red 취소선 / 물결선 / 도형 / 선, near-black 펜 / 텍스트 상자.
  * Arming a tool loads its style.
  * A panel change with nothing selected becomes that tool's default. It is stored as a *partial*
    `Settings.toolDefaults[toolId]`, written 400 ms after the last change.
  * With a selection, 이 스타일을 기본값으로 sets the default of the tool that drew it (사각형 → 사각형).
  * 기본값으로 재설정 appears once the tool has an override.
* **P1-12 읽기 모드 + 전체 화면.**
  * ⌃⌘R / F8 / the native `view.readingMode` item hides the title bar, sidebar, 주석 tool strip,
    properties panel and status bar. A toast says how to leave. Esc leaves; closing the document
    leaves too.
  * ⌃⌘F / F11 / `view.fullScreen` toggles `Window.setFullscreen`. This needed the new capability
    `core:window:allow-set-fullscreen`. On macOS, the native View item is the predefined one.
  * The two combine. Esc in 읽기 모드 leaves both.
* **P1-12 hide-while-dragging.** Moving or resizing with 선택 hides the bitmap copy of the annotation;
  the overlay repaints it in full as it moves. On drop, the annotation is back, at its new place.

## 2. Design decisions

**Typed signature = PNG file + existing image-stamp path.** This was the lightest option that fits
`image: { path }`: one small file-only command and no new `AnnotSpec` variant. The engine already
reads PNGs, and it keeps the alpha channel. `annot_stamp_png_keeps_transparency` checks this over
body text, so an opaque white box would fail it.
* The file name is a hash of the bytes, so the same signature reuses one file.
* The directory keeps the 32 most recently used files. A PNG is only an intermediate, because the
  saved signature is the text + style.
* A temp file was rejected: the OS may clean it while a redo or a paste still needs the path.

**`Settings.signatures` is typed but lenient.** A bad entry is dropped on its own, and the list is
capped at 10. `get_settings` falls back to `Settings::default()` when the whole value fails to parse,
so strict typing would let one hand-edited entry reset every setting.

**`toolDefaults` stays `Map<String, Value>` / `Record<string, unknown>`.** It already carried the
legacy `recentsCount`. The frontend validates each field (`toolStyles.parseToolStyle`), and
`mergeToolDefaults` touches only the styled tools' keys.

**Hide-while-dragging order** (`src/annot/dragHide.ts`):
1. The first live patch from the 선택 tool adds full-paint ghosts, holds the patch queue, and calls
   `set_annotations_hidden(true)`. Its `viewNonce` goes into that page's URLs as `vn`, because images
   are `immutable` and the webview would keep the old bitmap otherwise. `vn` is in the tile key and in
   the `/page` + `/tile` URLs. The engine ignores it.
2. The drop, pointercancel, a tool switch, the surface unmounting and the host stopping all end the
   drag the same way: await the hide → `hidden: false` → release the hold → one `update_annotation` →
   settle the ghosts at the new generation. The ghosts vanish when the new bitmap paints.
3. **Unhide comes before the update, because the undo snapshot is taken pre-edit.** An update made
   while `/F` has the HIDDEN bit would make 실행 취소 bring the annotation back *hidden*.
   `annot_drag_hide_unhides_before_the_undo_snapshot` pins both halves.
4. Nothing moved or the update failed, but the page did switch to the hidden render: the restore
   nonce forces a re-render.
5. Only kinds the overlay paints faithfully are hidden: markup, 메모, 도형, 선 / 화살표, ink with
   paths, 텍스트 상자, and catalogue stamps (`AnnotShape` now draws the built-in label). Image stamps
   and foreign stamps keep the old behaviour.

**Reading mode lives in `appStore`, not `viewStore`.** It is chrome, and `viewStore` belonged to the
night-mode work. `app/readingMode.ts` asks the window for its full-screen state instead of caching it,
because the macOS green button and the native menu change it behind our back.

## 3. Gates (verification run, 2026-09-28)

Run on the shared tree with Stage 6a's changes in it.

| gate | result |
|---|---|
| `cargo check --all-targets` | clean, 0 warnings |
| `cargo build --release --tests && cargo test --release` | 188 passed, 0 failed |
| `npm run typecheck` | pass |
| `npx vitest run` | 58 files, 420 tests passed (+8 files / +40 tests from this stage) |
| `node scripts/check-i18n.mjs` | ok, ko 649 / en 649 keys (+17 from this stage) |
| `npm run build && node scripts/check-bundle-size.mjs` | ok; dist 705.3 kB of 2048; critical path 111.5 kB gz of 120. SignatureDialog and StampPickerDialog are lazy chunks |

## 4. Files

Backend:
* New: `app/signatures.rs` (+ 3 lib tests).
* `ipc/types.rs`: `SavedSignature`, `Settings.signatures`, lenient deserializer, serde test.
* `app/store.rs`: `signatures_dir`. `commands/app.rs`: `write_signature_image`. `app/mod.rs`, `lib.rs`.
* `engine/annot/create.rs`: document-level label font, Korean colours, fit + centre.
* `tests/annot.rs`: `annot_stamp_korean_builtin`, `annot_stamp_png_keeps_transparency`,
  `annot_drag_hide_unhides_before_the_undo_snapshot`.
* `capabilities/default.json`: `core:window:allow-set-fullscreen`.

Frontend:
* New modules: `dialogs/{typedSignature,signatureLibrary}.ts`, `dialogs/StampPickerDialog.tsx`,
  `tools/stampCatalog.ts`, `store/toolStyles.ts`, `annot/{dragHide,toolDefaults}.ts`,
  `app/readingMode.ts`.
* New tests: `signatureLibrary.test.ts`, `signature.flow.test.tsx`, `stampPicker.flow.test.tsx`,
  `toolStyles.test.ts`, `Inspector/defaults.test.tsx`, `dragHide.test.ts`, `readingMode.test.tsx`,
  `ipc/viewNonce.test.ts`.
* Changed: `SignatureDialog.tsx` (rewritten), `tools/stamp.ts`, `tools/ToolController.ts`
  (`ToolPreview.label`), `annot/{AnnotationHost,ToolSurface,shapes}.tsx`, `annot/actions.ts`
  (`holdPatchFlush`), `store/{annotStore,appStore}.ts`, `app/Inspector/InspectorBody.tsx`,
  `App.tsx`, `app/useCommands.ts`, `ipc/{types,api,mock,protocol}.ts`,
  `viewer/Scroller.tsx` (`vn` only), `dialogs/{DialogHost.tsx,dialogState.ts,dialogs.css}`,
  `styles/shell.css`, `i18n/{ko,en}.json` (+17 keys), `test/ipc-samples/settings.json`.
* Docs: `IPC_CONTRACT.md` (§7.1, §9 `vn`, §11), `UI_SPEC.md` (§6, §7, §13, §15.10), `FEATURES.md`.

## 5. Still open

* **Hangul typed names** fall back per glyph to the system Korean font. Only 정자체 (명조) looks
  designed for it, and there is no Korean handwriting font on a stock macOS.
* **A typed or picked-image signature is a `Stamp`.** It is listed as 도장, not 서명, because
  `StampSpec` has no way to set `/Subj "SeePDF:Signature"`.
* **Hidden during a drag, another writer could snapshot the HIDDEN bit.** The frontend holds its own
  patches, but an autosave copy written mid-drag (only if the generation had moved since the last
  copy) or ⌘Z / ⌘S pressed mid-drag would capture it. The engine does not strip transient flags.
* Image stamps and foreign stamps are not hidden while dragged, because the overlay cannot repaint
  them.
* With 선택 armed and nothing selected, the panel edits the style of the last *styled* tool.
* No real-app smoke yet. Check these by hand:
  * the typed styles on Windows;
  * the 결재 seal at 200 %;
  * a drag of a text box on a tiled (zoomed-in) page;
  * F11 on Windows;
  * reading mode on macOS, where the traffic lights sit over the page.
