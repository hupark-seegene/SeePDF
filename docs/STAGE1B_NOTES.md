# Stage 1 (b) — pages, objects, export, save, metadata, OCR layer (backend)

Owner notes for `src-tauri/src/engine/{pages,objects,save,export,fonts,ocr}/**`,
`src-tauri/src/commands/{pages,objects,save,export,ocr}.rs`,
`src-tauri/tests/{pages,objects,save,export,ocr_layer}.rs`, `src-tauri/resources/fonts/**` and
`src-tauri/examples/build_hangul_font.rs`.

Companions: `WORKPLAN.md` row (b), `IPC_CONTRACT.md` §7.3/§7.4/§7.6/§7.7/§7.9, `ARCHITECTURE.md`
§6.2/§6.3/§6.6/§8, `docs/spikes/{pages,text,ocr}.md` (ground truth), `STAGE0_NOTES.md` §1 (the
engine API) and `STAGE1A_NOTES.md` §1 (the raw wrappers this row consumes).

**§1 is the bundled font** — the asset every other module was waiting for. §2 is the support
matrix, §3 what is deliberately not supported, §4 the frontend-facing payload notes for (e) and
(f), §5 the integration requests, §6 the test inventory.

---

## 1. `resources/fonts/SeePDF-Hangul.ttf` — 498,636 bytes (487 KB)

| | |
|---|---|
| source | **Noto Sans KR** variable, instanced at `wght 400`, SIL OFL 1.1 |
| built by | `cargo run --release --example build_hangul_font -- <NotoSansKR-Variable.ttf>` |
| glyphs | 3,126 + `.notdef` |
| coverage | ASCII, Latin-1 (`U+00A1..FF` minus the soft hyphen), Hangul Jamo `U+1100..11FF`, Compatibility Jamo `U+3130..318F`, the **2,350 KS X 1001 syllables**, CJK symbols and punctuation `U+3001..303F`, general punctuation / arrows / maths, fullwidth forms `U+FF01..FF5E`, ₩ and € |
| names | family `SeePDF Hangul`, PostScript `SeePDF-Hangul` |
| licence | `resources/fonts/OFL.txt` — the source's OFL text with a provenance header. It carries **Adobe's** copyright ("with Reserved Font Name 'Source'") because Noto Sans KR is built from Source Han Sans; that is what Google ships and what the font's own `name` ID 0 says. |
| not covered | five of the 3,131 requested code points: U+2252, U+3130 and U+318F have no glyph in the source, and U+3164 (Hangul filler) and U+FF5E share a glyph with U+0020 and U+301C so they are dropped rather than allowed to poison the reverse `/ToUnicode` map (§1.2) |
| effect on a document | **+257 KB** once per document (Flate-compressed inside the PDF); `fixtures/gen/korean-300dpi.pdf` is 263,240 bytes in total |

The `WORKPLAN.md` §5 estimate was 1–1.5 MB; 487 KB is what the KS X 1001 repertoire actually
costs once the outlines are subset. NanumGothic-Regular also builds (1.22 MB) and is still
supported by the example, but it has **no conjoining Jamo at all** and is missing 57 accented
Latin-1 characters, so a mixed `한글 café` string would have failed the coverage probe. Noto is
what ships.

### 1.1 Why the font is rebuilt, not just subset

`subsetter` targets PDF writers that supply their own CMap, so it **deletes the `cmap` table**.
PDFium does the opposite: `FPDFText_LoadFont` maps the string's code points to glyph ids through
the *font's own* `cmap`, and reverse-maps the same table to generate `/ToUnicode`. A font without
a `cmap` renders nothing at all.

So `build_hangul_font.rs` subsets the outlines with `subsetter` (which also pulls in composite
glyph components and rewrites `glyf`, `loca`, `hmtx`, `hhea`, `head`, `maxp`, `post`) and then
writes a **fresh sfnt container**: the subsetter's tables, plus

* a `cmap` (format 4, referenced from both `(3,1)` and `(0,3)`) built from our own code point →
  new glyph id map;
* a `name` table naming the face `SeePDF Hangul` while keeping the source's copyright (ID 0),
  licence (13) and licence URL (14) records, as OFL §1 requires;
* the source's `OS/2`, which `subsetter` drops and FreeType likes to have;

with every table checksum and `head.checkSumAdjustment` recomputed.

### 1.2 The AppleGothic space→TAB bug — `WORKPLAN.md` §6 probe: **resolved**

> The bundled Hangul font's generated `/ToUnicode` not mapping space → TAB

The cause is a **many-to-one `cmap`**: when several code points map to one glyph, PDFium's
reverse map can pick the wrong one, and AppleGothic maps U+0009 and U+0020 to the same blank
glyph. The builder therefore keeps **one code point per glyph id** — the lowest, with ASCII
first in the ordered wanted-set, so U+0020 owns the space glyph and U+00A0, U+2007, U+3000 and
the Hangul filler U+3164 are dropped from the `cmap` rather than sharing it. `verify()` refuses
to write a font where any two code points share a glyph, and prints which ones would have.

Measured on the finished font: `fixtures/gen/korean-300dpi.pdf` extracts as
`"검색 가능한 한글 문서\r\nSeePDF OCR 정확도 측정용 고정 페이지입니다. …"` — ASCII spaces, no tabs,
and `search("검색 가능한")` hits. `ocr_layer_apply_searchable` and `objects_add_text_korean`
assert exactly that after save + reopen.

### 1.3 Loading it at runtime — `engine::fonts`

```rust
fonts::bundled_path()  -> Option<PathBuf>       // resource dir, .app Resources, CARGO_MANIFEST_DIR
fonts::bundled_bytes() -> Result<&'static [u8]> // read once per process
fonts::load(&mut PdfDocument)     -> Result<PdfFontToken>   // embeds one copy
fonts::embedded_token(&PdfPage)   -> Option<PdfFontToken>   // reuse a copy already on the page
fonts::covers(text)               -> Result<Coverage>       // ttf-parser cmap probe
fonts::is_latin1(text) / fonts::pick(text) -> Pick::{Helvetica, Bundled}
fonts::token_for(&mut PdfDocument, text, &mut Option<PdfFontToken>) -> Result<(PdfFontToken, Pick)>
fonts::ksx1001::{syllables(), contains(cp), KSX1001_RANGES}
```

`load` **must not be called twice for one document**: `FPDFText_LoadFont` appends a fresh copy of
the whole file every time and PDFium does not deduplicate. Every caller here takes one token and
reuses it (`ocr_apply` for a whole batch, `add_text_object` for all lines of a block), and
`embedded_token` finds a copy that is already on the page — `PdfPage::fonts()` returns
**non-owning** `PdfFont` wrappers, so taking a `PdfFontToken` from one is safe. See §5.2 for the
case this still does not cover.

`covers` is the honest answer to "can we draw this?": PDFium checks nothing and silently drops
glyphs it cannot find, which is how `한글 테스트` becomes `  Hangul ABC` with the wrong font.
`token_for` returns `fontCoverage` with the offending characters in `message` rather than
embedding a font that will produce blanks.

---

## 2. What works

### 2.1 Pages (`engine::pages`, `commands/pages.rs`)

| Op | How | Notes |
|---|---|---|
| `move` | raw `FPDF_MovePages` | ordered-list semantics: `[3,2] → 1` gives `A D C B`. 0.02 ms |
| `delete` | `PdfPage::delete()` descending | refuses to empty the document |
| `rotate` | `set_rotation` under `Manual` | relative delta; `/Rotate` is a page attribute, so no content regeneration (that was the 2.8 ms the spike measured) |
| `insertBlank` | `create_page_at_index` | `a4` / `letter` / explicit points / `sameAs` (reads the neighbour's size **live**) |
| `duplicate` | raw `FPDF_ImportPagesByIndex(doc, doc, [i], i+1)` | descending, so each copy lands after its source |
| `insertFrom` | `copy_pages_from_document(&src, "1-3,5", at)` | the range string is **1-based**, as typed |
| `reverse` | one `FPDF_MovePages` with the full order | |
| `extract_pages` | copy the original, delete the rest | keeps `/AcroForm`, the outline, `/PageLabels`, metadata |
| `split_document` | the same, once per part | `everyN` and `ranges`; one `Lane::Background` command per file |
| `merge_documents` | `create_new_pdf` + `copy_pages_from_document` | the **only** importer; reports `formsDropped` / `outlineDropped` / `metadataDropped` |

One `page_ops` call is one `registry::mutate(..).structural()` = one generation = one undo step,
whatever the batch contains.

**Validation inside a batch uses the live PDFium page count**, not `OpenDoc::page_count()`:
`pages_meta` is only refreshed after the `mutate` closure returns, so a batch like
`[insertBlank, delete(14)]` would otherwise be validated against the page list as it was before
the insert. `pages::live_count` is that read.

Extract/split never import. `FPDF_ImportPages*` keeps the widget annotations but drops the
catalog's `/AcroForm`, so an imported form renders and cannot be filled (pages spike §2) —
`pages_extract_keeps_acroform` pins the copy-and-delete strategy on `160F-2019.pdf`'s 76 widgets.

### 2.2 Page objects (`engine::objects`, `commands/objects.rs`)

`list_page_objects` reports every **top-level** object in index order, which is what `ObjectId`
means in the contract. Text objects carry their text (through `PdfPageText::for_object`, never
`PdfPageTextObject::text()`, which loads a whole `FPDF_TEXTPAGE` per call), font name, unscaled
size, fill colour and an `editable` / `reason` pair:

| Object | `editable` | `reason` |
|---|---|---|
| text, base-14 or covering embedded font | `full` | — |
| text, render mode Invisible (an OCR layer) | `moveOnly` | `invisible` |
| text whose run is raw char codes (no `/ToUnicode`) | `moveOnly` | `noUnicode` |
| Form XObject | `readOnly` | `insideXObject` |
| image | `full` | — |
| path / shading | `moveOnly` | — |

`ours` is true when the object's font is the bundled subset — the only marker a page object can
carry (there is no dictionary to write a private key into).

`probe_text_edit` runs the **trial edit** of `ARCHITECTURE.md` §6.2 and reverts it: `set_text` →
fresh `page.text()` → **two** conditions, because each catches a different silent failure:

1. every non-generated character, spaces included, reports `loose_bounds().width() > 0` — an
   embedded subset with no space glyph gives it width 0 and renders `SeePDFeditedtitle`
   (text spike §5);
2. the characters read back through `for_object` **are the ones we asked for** — a non-CID font
   maps every code point above 255 to 0xFF, so `한글` becomes `ÿÿ`, which has a perfectly good
   advance width and passes condition 1 (text spike §9 gotcha 9). This is not in the
   architecture's description of the check; without it, changing a Helvetica run to Korean
   reported `inPlace` and produced `ÿÿÿ`.

That is the only reliable detector (`PdfFontGlyphs::len()` is 0 for subset fonts *and* for the
built-ins). It never regenerates the page content, so the document is byte-identical afterwards —
`objects_text_probe_subset_font` asserts the generation does not move.

`edit_text_object` re-runs the probe and then either `set_text`s in place or **replaces the
object**: a new `PdfPageTextObject` with the same matrix, fill colour and render mode, drawn with
Helvetica (Latin-1) or the bundled subset, appended and then the old one removed (append-first
keeps the old index valid). With `allowFontSubstitution: false` the replace path is
`fontCoverage` instead, which is the confirmation hook for 글꼴이 대체됩니다.

`add_text_object` lays out multi-line text itself — PDFium has no reflow — as **one object per
line**, measured with `bounds()` before the object is added and aligned left / centre / right
inside the rect, stepping down by `1.2 × fontSize`.

`add_image_object` places a PNG or JPEG at a rect (`keepAspect` letterboxes it inside);
`transform_object` anchors scale and rotation at the object's own bottom-left corner
(`translate(-x,-y)` → scale/rotate → `translate(x,y)`, because a bare `scale()` post-multiplies in
page space and moves the object); `delete_objects` removes descending.

Everything here regenerates the page content **exactly once**, at the end: `set_text`,
`set_render_mode`, `set_fill_color` and `set_unscaled_font_size` do not mark the page dirty, so
without it the edit is simply lost (text spike §7).

Two engine-level extras with no contract command yet: `objects::replace_image` (`set_image`
preserves the object matrix) and `objects::extract_images` (`get_processed_image` → PNG). See
§5.4.

### 2.3 Save (`engine::save`, `commands/save.rs`)

`ARCHITECTURE.md` §8, step for step:

1. `render::tiles::generate_appearances` — one 32 px render per touched page, so PDFium writes
   the `/AP` streams;
2. `FORM_ForceToKillFocus` (commits the focused field's `/V` **and** regenerates its `/AP`) and
   drop the page LRU;
3. `raw::save::save_as_copy(FPDF_NO_INCREMENTAL)`;
4. temp file `<dir>/.<name>.seepdf-<pid>.tmp`, `write_all`, `sync_all`;
5. **verify**: reopen the bytes with PDFium, page count must match and `page_sizes()` must read;
6. backup to `$TEMP/seepdf-backups/<stem>/<ts>-<name>` (keeps 3; a failure is logged, never fatal);
7. `fs::rename`, with the 5 × 100 ms retry for Windows sharing violations;
8. `registry::replace` from exactly those bytes, `saved_generation = generation`, `doc-saved`.

Failure at any point removes the temp and leaves the original byte-identical
(`save_atomic_abort` tests both a pre-temp abort and a post-temp one). A read-only file or
directory is `readOnly` **before** the 5 ms/MB serialisation, so the UI can fall through to Save
As cheaply. An untitled document (a merge result) with no `path` is also `readOnly` with
"use Save As" in the message.

`save::serialize` is the shared step-1..3 helper; `export_flattened`, `print_prepare` and the
page-subset writer all go through it so there is exactly one place that knows an `/AP` render has
to happen before serialisation.

**Encryption survives.** `save_keeps_encryption` opens `gen/encrypted-rc4-40.pdf` with the user
password, saves, and asserts the result still answers `passwordRequired` without one and opens
with it. `FPDF_NO_INCREMENTAL` keeps the encryption dictionary; dropping it is the separate
`FPDF_REMOVE_SECURITY = 4` path (P1 `remove_password`).

### 2.4 Export and print (`engine::export`, `commands/export.rs`)

* `export_images` — PNG or JPEG at 36–1200 DPI, `round(pt × dpi/72)` pixels, annotations and form
  values rendered, `use_print_quality(true)`, no LCD subpixel AA. `transparentBackground` applies
  to PNG only (JPEG has no alpha and would come out black). One `Lane::Background` command per
  page sharing one `JobToken`, progress and `outputs` over the `Channel`, cancellable.
* `estimate_export` — renders and **encodes** up to 3 sample pages and scales; a formula from the
  pixel count cannot meet F-24's ±25 % (PNG of a text page compresses ~30× better than a photo).
  Measured on tracemonkey at 150 DPI the estimate is within a few per cent.
* `export_text` — `page.text().all()` per page, separated by U+000C.
* `export_flattened` / `print_prepare` — raw `FPDFPage_Flatten(FLAT_NORMALDISPLAY)` +
  `FPDFPage_GenerateContent` on a **scratch copy**, then serialise. The open document is never
  touched and stays clean.
* `print_prepare` writes `$TEMP/seepdf-print/<stem>-<docId>-<pid>.pdf` and applies a page
  selection before flattening.

`export_flatten_normaldisplay` is the test that pins the decision: it **clears** the `/F 4` flag
on the fixture's highlight, flattens both ways, and compares the pixels inside the annotation's
own rectangle — `NORMALDISPLAY` keeps it (0.0 % differ), `FLAT_PRINT` deletes it (100 % differ).
Comparing whole pages does not work: a highlight is 0.2 % of a page either way.

### 2.5 Metadata

**Read** works and is already in `DocInfo.meta` (Stage 0's `registry::read_metadata`);
`save::read_metadata` is the accessor for `commands/security.rs`.

**Write does not exist in PDFium.** There is no `FPDF_SetMetaText` in build 8057, `PdfMetadata`
has `get`/`iter`/`len` and no setter, and the only document-level string PDFium can write is
`PdfCatalog::set_language` (pages spike §5). `save::write_metadata` returns the `unsupported`
error with that reason in `message`, ready for `set_metadata` / `remove_metadata` to call — see
§5.1, because `commands/security.rs` belongs to (a).

### 2.6 OCR layer (`engine::ocr`, `commands/ocr.rs`)

* `ocr_capabilities` — `["tesseract"]` plus `["kor", "eng"]`. `vision` (P1) and `windows` (P2) are
  deliberately **not** advertised until `ocr_recognize_native` has a body, so (f) never routes to
  an engine that does not exist.
* `ocr_page_status` — counts non-whitespace, non-control characters and calls a page image-only
  below 16 of them. PDFium inserts generated `\r\n` between runs, so a raw `text().len()` makes an
  empty scan look like it has text.
* `ocr_apply` — **one** `registry::mutate` for the whole batch = one undo step. Per word:
  `PdfPageTextObject::new` → `set_render_mode(Invisible)` → measure `bounds()` (works before the
  object is added) → `scale(clamp(box_w_pt / natural_w, 0.5, 2.0), 1.0)` → rotate by the page's
  `/Rotate` → `translate` → `add_text_object`; one `regenerate_content()` per page (2 ms for 730
  words versus 81 ms with the automatic strategy). Words below confidence 30 and empty tokens are
  skipped. Font size per **line** from `rowHeightPx × 72/dpi`, falling back to the line box height
  with a ×1.15 correction for Latin-only lines (a mixed-case Latin box is ~0.87 em; a Hangul box
  is the em).
* `replaceExisting` removes every invisible text object on the page first, so a re-run does not
  double the layer. The same page twice in one batch is `invalidArgument` for the same reason.
* `ocr::points_to_image_px` is the inverse mapping, for the review overlay and for the test.

**Rotation — `WORKPLAN.md` §6-adjacent probe, now measured.** The ocr spike reasoned gotcha 16
from the API and never ran it. `ocr_layer_rotation_roundtrip` does: on `fixtures/rotation.pdf`'s
`/Rotate 90` page it places a word from an image box, saves, reopens, finds the characters,
unions their loose boxes and maps them back with `FPDF_PageToDevice`. The box comes back within
24 px of where it went in, the text extracts, search finds it, and the page renders
pixel-identically.

---

## 3. Unsupported / deferred

| Item | Status | Why |
|---|---|---|
| `set_metadata` / `remove_metadata` | `unsupported` | no `FPDF_SetMetaText` in PDFium 8057; needs the P1 `lopdf` dependency (pages spike §5) |
| write `/PageLabels`, create/edit bookmarks | ✘ | same: PDFium has no writer at all |
| `split_document` "by outline" | ✘ | `SplitMode` (frozen) has only `everyN` and `ranges`. The outline is readable (`OpenDoc::outline`), so this is a one-variant contract change plus ~15 lines |
| encrypt / set password | ✘ | `FPDF_SaveAsCopy` takes no password argument; P1, and not `lopdf` either |
| `remove_password` | ready, not wired | `raw::save::SaveFlags::RemoveSecurity` + `write_atomic` is the whole body; `commands/security.rs` is (a)'s file (§5.1) |
| incremental save | ✘ by decision | `FPDF_INCREMENTAL` is 25× faster and appends a whole second revision; useful for an autosave journal, wrong for a save |
| editing text inside a Form XObject | refused | the edit returns `Ok` and `FPDFPage_GenerateContent` throws it away (text spike §8.7). P2 is flattening the XObject first |
| text with no `/ToUnicode` | `moveOnly` | the characters are raw glyph codes; `set_text` produces `ÿ` |
| re-encoding an image in place | engine-only | `objects::replace_image` exists; `set_image` always writes Flate, never JPEG, so a "compress" feature needs `lopdf` (pages spike §8) |
| `PageObject` block grouping | not reported | `PageObjectList` (frozen) has no block field. Objects come back in index order and the paragraph grouping the spike describes is a frontend concern; `engine::text::layer` already exposes words and lines |
| per-document font deduplication across pages | partial | see §5.2 |
| `ocr_recognize_native` | `unsupported` | P1, macOS Vision |

---

## 4. What the frontend needs to know

### 4.1 (e) — organizer, dialogs, welcome

* **`page_ops` is one undo step per call.** Send the whole drag as one `ops` array; do not split
  it. A refused batch changes nothing and does not bump `docGeneration`.
* **Every user-typed range is 1-based** (`"1-3,5"`, open-ended `"5-"` allowed, `""`/`"all"` means
  the whole document). Every `PageIndex` in the contract is 0-based. `page_ops`'s `insertFrom`
  takes the 1-based string; `extract_pages` takes 0-based indices.
* `merge_documents` returns an **untitled** document (`info.path === null`): the first ⌘S must go
  through Save As. Its `warnings` array is the 양식/목차/메타데이터 warning the UI owes the user.
* `split_document` returns a `JobId` immediately and streams `JobEvent`s; `done.outputs` is the
  list of files written, in order. Output names are `<stem>-001-005.pdf` (everyN) or
  `<stem>-01.pdf` (ranges).
* `extract_pages` writes the file first and only then deletes; with `removeAfter: true` that
  delete is one `page_ops` generation and the returned `docGeneration` is the one after it, so
  an undo puts the pages back without touching the file that was written.
* `save_document` on a document with no path fails with **`readOnly`**, not `notFound`. So does a
  read-only file or directory. Both mean "offer Save As".
* A successful save emits **`doc-saved` only** — not `doc-changed`. The content did not change,
  so the generation does not move and every tile URL stays valid; clear the dirty flag (and
  update the window title and `DocInfo.path`) from `doc-saved` / the resolved `SaveResult`.
* `export_images` writes `<baseName>-<3-digit 1-based page>.png|jpg` into `outDir`. `dpi` outside
  36–1200 is `invalidArgument`; so is a page/DPI combination over the render ceiling, with the
  size in the message.
* `estimate_export` renders **and encodes** three sample pages, so it costs roughly 30 ms at
  150 DPI and more at 300 — fine on a change of the DPI or format selector, not on every
  keystroke.
* `print_prepare` returns a path in `$TEMP/seepdf-print/`, named `<stem>-<docId>-<pid>.pdf` so
  two open documents never collide. Nothing cleans the directory up but the OS.

### 4.2 (d) — tools, properties, inspector

* `list_page_objects` → `probe_text_edit` → `edit_text_object` is the mandated order.
  `probe_text_edit` is free of side effects and cheap enough for a debounced preview; its
  `substituteFont` is the string to put in the confirmation dialog.
* `edit_text_object` with `allowFontSubstitution: false` returns **`fontCoverage`**; retry with
  `true` after the user confirms.
* A replace-object edit **moves the object to the end of the list**. Every mutating command
  returns the whole re-listed page, so use the returned `objects` array and the new
  `docGeneration`; never reuse an `objectId` across generations (the engine answers `stale`).
* **"font substituted" is reported by the data, not by a flag** — `PageObject` (frozen) has no
  `fontSubstituted` field. Before the edit, `probe_text_edit` says `strategy: "replaceFont"` with
  the name in `substituteFont`; after it, the object in the returned list has
  `fontName: "SeePDF-Hangul"` (or `"Helvetica"`) and `ours: true`. Comparing the two is how the
  UI shows 글꼴이 대체되었습니다 without a contract change.
* `add_text_object` makes **one object per line** of `text`. `\n` and `\r\n` both split.
* Objects with `editable: "moveOnly"` accept `transform_object` and `delete_objects` but not
  `edit_text_object`; `"readOnly"` accepts neither.

### 4.3 (f) — OCR worker pipeline

* `OcrPage.widthPx` / `heightPx` **must be the dimensions of the image the worker actually
  recognised**, because they are the device box `FPDF_DeviceToPage` inverts. Pass what the `/ocr`
  route returned in `X-Image-Width` / `X-Image-Height`, not a recomputed number.
* `OcrPage.rotation` is informational — the `/ocr` route takes no rotation parameter, so the image
  is always the page at its own `/Rotate`, which the engine reads itself. A mismatch is logged,
  not an error.
* Boxes are `[x0, y0, x1, y1]` in **image pixels, origin top-left**; the engine sorts the corners,
  so a flipped box is fine.
* Give each line a `rowHeightPx` when the engine reports one (Tesseract's
  `rowAttributes.rowHeight`). Without it the font size comes from the line box height, which is
  ~13 % too small for mixed-case Latin.
* **Spaces inside a word token are fine and are the right thing** for Hangul: the bundled font's
  `/ToUnicode` maps U+0020 to U+0020, so `"검색 가능한"` as one token extracts and searches
  correctly. That is what `normalize.ts`'s rebuilt Hangul spacing should produce.
* Words with `confidence < 30` or empty/whitespace text are dropped by the engine; you do not have
  to filter them.
* Do not send the **same page twice in one call** — it is `invalidArgument`, because the words
  would be added twice and `replaceExisting` would not save you (the second pass would delete the
  layer the first one just added).
* `ocr_apply` is **one** command for the whole batch and one undo step, and it resolves with the
  new `DocInfo` rather than a `JobId`. Progress arrives on the channel per page, but **the batch
  cannot be cancelled once it is running** — `registry::mutate` does not roll back a closure that
  fails half-way, so stopping in the middle would leave some pages OCR'd and the generation not
  bumped. Chunk the call (per page, or per 5 pages) if the user needs to stop early; each chunk
  is then its own undo step. See §5.5.
* A non-Latin word when `resources/fonts/SeePDF-Hangul.ttf` is missing is `fontCoverage`, not a
  silently empty layer.

---

## 5. Integration requests

Each one is a workaround that is already in place, with the patch its owner would apply.

### 5.1 `commands/security.rs` — three bodies that belong to (b) (owner: (a) / integrator)

`IPC_CONTRACT.md` §7.5 says "Owner (a) for redaction, **(b) for the metadata and security file
rewrites**", but the file is in (a)'s row, so these were not touched. The engine side is ready:

```rust
// remove_password — pages spike §5, verified: FPDF_REMOVE_SECURITY produces a file that opens
// with no password.
#[tauri::command]
pub async fn remove_password(engine: State<'_, EngineHandle>, doc_id: String, out_path: String)
    -> Result<BytesWritten, EngineError> {
    engine.call(Lane::Edit, "remove_password", move |st| {
        crate::engine::render::tiles::generate_appearances(st, &doc_id)?;
        let doc = st.doc(&doc_id)?;
        let bytes = crate::engine::raw::save::save_as_copy(
            doc.bindings(), doc.pdf(), crate::engine::raw::save::SaveFlags::RemoveSecurity)?;
        let written = crate::engine::pages::write_atomic(std::path::Path::new(&out_path), &bytes)?;
        Ok(BytesWritten { bytes: written })
    }).await
}

// set_metadata / remove_metadata — until `lopdf` lands (P1):
crate::engine::save::write_metadata(&meta)   // returns the `unsupported` error with the reason
```

`save::write_metadata` exists so the reason ("PDFium build 8057 has no `FPDF_SetMetaText`") is
written once and the P1 patch has one call site.

### 5.2 `registry.rs` — a per-document font token on `OpenDoc` (owner: Stage 0 / integrator)

`FPDFText_LoadFont` embeds a fresh copy of the ~487 KB font on every call and PDFium does not
deduplicate. Within one command that is handled (one token, reused); across commands,
`fonts::embedded_token` finds a copy that is already **on the same page**. Adding Korean text to
five *different* pages of one document therefore embeds five copies (~2.4 MB).

The fix is one field:

```rust
 pub struct OpenDoc<'p> {
     …
+    /// Token for the bundled Hangul font once it has been embedded in this document.
+    /// Cleared by `replace()` (a reload invalidates every `FPDF_FONT`).
+    pub hangul_font: Option<PdfFontToken>,
```

set in `open`/`replace` to `None`, and `engine::fonts::token_for` would take `&mut OpenDoc`
instead of `&mut Option<PdfFontToken>`. It cannot live in a module-level cache: the handle dies
with the `FPDF_DOCUMENT`, and `registry::replace` swaps that on every undo, redo and save.

### 5.3 `Cargo.toml` — the `crate-type` collision makes `cargo test` flaky (owner: integrator)

Every build prints four

```
warning: output filename collision at …/deps/libseepdf_lib.{a,dylib,dylib.dSYM,rlib}
  = note: the lib target `seepdf_lib` … has the same output filename as the lib target `seepdf_lib` …
```

and roughly one `cargo test` in three then fails with **173 errors** of the form
`the crate X requires panic strategy 'abort' which is incompatible with this crate's strategy of
'unwind'`, or `there are multiple different versions of crate pdfium_render in the dependency
graph`, in test targets that had just compiled cleanly. Re-running the same command succeeds with
no source change. It is `crate-type = ["staticlib", "cdylib", "rlib"]` plus
`[profile.release] panic = "abort"`: two lib units write the same artifact paths and a test target
can pick up the wrong one.

This is **pre-existing** (it is not caused by anything in this row: when it fires, every test
target fails, `tests/{annot,form,history,protocol,registry,text}.rs` included) and it will make
CI flaky. The reliable workaround, and what CI should run, is to build the test binaries in their
own invocation first:

```sh
cargo build --release --tests && cargo test --release
```

That sequence has not failed once; `cargo test --release` alone after a change to a library
source file fails roughly one time in three and succeeds on an immediate re-run with no source
change. The real fix is to stop building `staticlib`/`cdylib` for test builds — e.g. put the
extra crate types behind a feature the bundle enables. The same cause is why integration tests
cannot use
`pdfium_render` types at all — `tests/{history,protocol,registry}.rs` already work around it, and
`tests/{objects,ocr_layer}.rs` go through `engine::text::search` rather than
`PdfSearchOptions` for the same reason.

### 5.4 `ipc/types.rs` — three small gaps (owner: integrator)

* **`replace_image` has no command.** `objects::replace_image(docId, page, objectId,
  expectGeneration, path)` is implemented and preserves the object matrix, which is what "이미지
  바꾸기" needs. Adding it is one `#[tauri::command]` plus a `PageObjectList` return — the same
  shape as `add_image_object`.
* **`extract_images` has no command.** `objects::extract_images(doc, page)` returns
  `(objectId, width, height, PNG bytes)` per image. The natural contract shape is a
  `seepdf://` route rather than a command, since it is pixels.
* `SplitMode` has no `byOutline` variant (§3).

### 5.5 `ocr_apply` cancellation granularity (owner: (f) / integrator)

The contract says `ocr_apply` is **one** `registry::mutate` for the whole batch, and it is. The
consequence is that the `AtomicBool` cannot stop it half-way: `registry::mutate` does not restore
a closure that failed part-way through (`STAGE1A_NOTES.md` §5.2), so an abort at page 3 would
leave three pages carrying a text layer in a document whose generation never moved. The command
therefore creates a job id for symmetry with the other channel commands but **does not attach the
cancel token to the engine command**, and `cancel_job` on it is a no-op.

`FEATURES.md` F-21 wants "cancelling at page 3 of 14 leaves the document unchanged", and that is
exactly what happens today — the recognition loop in (f) is where the pages stop arriving, and
nothing is applied until the batch commits. If the UI instead wants "pages 1–2 are kept", (f)
should call `ocr_apply` per page or per chunk; each call is then its own undo step. Flagging it so
the two documents do not drift apart.

### 5.6 Files outside (b)'s row that were touched

One file, module declarations only — there is no other way to register a module:

* `src-tauri/src/engine/mod.rs`: `+pub mod export; +pub mod fonts; +pub mod objects; +pub mod ocr;
  +pub mod pages; +pub mod save;`

Nothing else. `git status` shows `commands/{pages,objects,save,export,ocr}.rs`, the new
`engine/{pages,objects,save,export,fonts,ocr}/**`, `tests/{pages,objects,save,export,ocr_layer}.rs`,
`examples/build_hangul_font.rs`, `resources/fonts/{SeePDF-Hangul.ttf,OFL.txt,README.md}`,
`docs/STAGE1B_NOTES.md` — plus the regenerated `fixtures/gen/korean-300dpi.pdf`, which
`examples/gen_fixtures.rs` (Stage 0's, unmodified) has been waiting for since `STAGE0_NOTES.md`
§4.2.

---

## 6. Test inventory

```sh
cd src-tauri
# The DoD filters. Note the `--`: `cargo test` itself takes only one TESTNAME, so the form
# printed in WORKPLAN.md row (b) is rejected with "unexpected argument 'objects_' found".
cargo test --release -- pages_ objects_ save_ export_ ocr_layer_
cargo test --release            # everything, Stage 0 and (a) included: 40 unit + 86 integration
```

| DoD name | file | what it proves |
|---|---|---|
| `pages_move_order` | `tests/pages.rs` | `[3,2] → 1` gives `A D C B` **by page text**, not by page count; `reverse` too |
| `pages_extract_keeps_acroform` | | the extracted `160F-2019.pdf` still has `/AcroForm` and all 76 widgets |
| `pages_duplicate` | | 14 → 15 pages and the copy sits directly after its source |
| `pages_insert_from_range_1based` | | `"1-3,5"` brings pages 1, 2, 3, 5 of `gen/500p.pdf` — not 2, 3, 4, 6 |
| `objects_text_inplace` | `tests/objects.rs` | a base-14 run edits in place, survives save + reopen, and a stale `expectGeneration` is refused |
| `objects_text_probe_subset_font` | | tracemonkey's Type1 subset title probes as `replaceFont`, the probe reverts itself, `allowFontSubstitution: false` is `fontCoverage`, and the substituted run keeps its spaces |
| `objects_text_xobject_refused` | | a Form XObject is `readOnly`/`insideXObject`, probe and edit both refuse, generation unchanged |
| `objects_text_replace_with_korean` | | changing a Helvetica run to 한글 probes as `replaceFont` (**not** `inPlace` — the round-trip half of the trial check), substitutes `SeePDF Hangul`, keeps the new colour and survives save + reopen |
| `objects_add_text_korean` | | 한글 renders, extracts **with ASCII spaces**, is searchable, survives save + reopen, and a second block on the same page reuses the embedded font |
| `objects_add_image` | | a PNG lands on its rect to 0.5 pt after save + reopen; move and delete persist |
| `save_atomic_abort` | `tests/save.rs` | a pre-temp abort and a post-temp (rename) abort both leave the original byte-identical with no `.tmp` behind |
| `save_verify_rejects_truncated` | | truncated bytes, a page-count mismatch and non-PDF garbage are all `verifyFailed` |
| `save_keeps_encryption` | | the saved file still answers `passwordRequired` and opens with the original password |
| `export_images_sizes` | `tests/export.rs` | PNG/JPEG headers report `round(pt × dpi/72)`; the estimate is within ±25 % of three real encodes; bad DPI is refused |
| `export_text` | | per-page text equals `page.text().all()` with U+000C between pages |
| `export_flatten_normaldisplay` | | an annotation with `/F 0` survives `FLAT_NORMALDISPLAY` and is **deleted** by `FLAT_PRINT`, compared inside its own rect; the open document is untouched |
| `ocr_layer_apply_searchable` | `tests/ocr_layer.rs` | Hangul extracts with spaces and no TAB, search hits, the render is pixel-identical, the run fills its box, one generation, `replaceExisting` does not double it |
| `ocr_layer_rotation_roundtrip` | | on `/Rotate 90`: text extracts, search hits, the box round-trips to within 24 px, nothing is painted |

Plus `pages_batch_is_one_undo_step`, `pages_extract_removes_after`, `pages_split_writes_every_part`,
`pages_merge_reports_what_it_lost`, `pages_rejects_impossible_requests`, `pages_insert_blank_sizes`,
`objects_transform_is_anchored`, `objects_extract_images`, `save_document_roundtrip`,
`save_document_as_repoints_the_document`, `save_untitled_needs_a_path`, `export_print_prepare`,
`export_reports_crop_box`, `ocr_layer_page_status`, `ocr_layer_rejects_bad_input`, and unit tests
for the range parser, the split planner, the KS X 1001 table, the bundled font's coverage and
one-code-point-per-glyph rule, the OCR font-size rule, DPI bounds and file-name sanitising.

### Samples for a human to open

`fixtures/out/stage1b/` (gitignored), written by the tests:

| file | what to look for |
|---|---|
| `merged.pdf` | 4 pages from three sources; the form fields of `160F-2019.pdf` are dead (that is the `formsDropped` warning) |
| `ocr-korean.pdf` | a blank page that is **empty on screen** and from which `검색 가능한 한글 문서` can be selected, copied and found with ⌘F |
| `ocr-rotated.pdf` | the same on a `/Rotate 90` page — the selection rectangle should sit where the text would be |
| `korean-text-box.pdf` | two visible Korean lines on tracemonkey page 1 |
| `text-replaced.pdf` | tracemonkey's title replaced through the substitute-font path |
| `image-placed.pdf` | a 64×32 PNG at (100,100)–(260,180) pt |
| `flattened.pdf` | the highlight is in the page content and `/Annots` is empty |
| `extract-form.pdf` | one page of `160F-2019.pdf` whose fields are still fillable |
| `split/`, `extract-two.pdf` | the split parts (5 + 5 + 4 pages) and a two-page extract |
| `encrypted-resaved.pdf` | still asks for the password `user` |
| `export-images/`, `tracemonkey.txt` | the image and text exports |

The human half — opening `ocr-korean.pdf` in macOS Preview and Acrobat and searching for
`검색 가능한` — has **not** been done and is still owed (the same debt `STAGE1A_NOTES.md` records
for QA-4).
