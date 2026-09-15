# Stage 1 (a) — annotations, forms, redaction (backend)

Owner notes for `src-tauri/src/engine/{raw,annot,form,redact}/**`,
`src-tauri/src/commands/{annots,forms,security}.rs` and `src-tauri/tests/{annot,form,redact}.rs`.
Companions: `WORKPLAN.md` row (a), `IPC_CONTRACT.md` §7.1/§7.2/§7.5, `ARCHITECTURE.md` §6.1/§6.4/§6.5,
`docs/spikes/annotations.md` (the ground truth) and `STAGE0_NOTES.md` §1 (the engine API).

**§1 is what Stage 1 (b) needs on day 1** — the raw wrapper signatures. It landed first and is stable.
§2 is the support matrix, §3 the `FORM_*` probe result, §4 what is deliberately not supported, §5 the
things another owner has to change.

---

## 1. Raw wrappers for Stage 1 (b) — `engine::raw::{page, save}`

All of them are implemented and covered by `tests/annot.rs::annot_raw_wrappers_smoke`. Import as
`use crate::engine::raw;` and get the bindings from `doc.bindings()` (or `raw::bindings(st.pdfium)`).

```rust
// src-tauri/src/engine/raw/page.rs
pub enum FlattenMode    { NormalDisplay, Print }        // NormalDisplay unless you mean FLAT_PRINT
pub enum FlattenOutcome { Flattened, NothingToDo }

pub fn move_pages(
    bindings: &dyn PdfiumLibraryBindings,
    doc: &PdfDocument<'_>,
    pages: &[u16],            // the pages to lift, in the order they should land
    dest: u16,                // where the block lands (FPDF_MovePages ordered-list semantics)
) -> Result<(), EngineError>;

pub fn import_pages_by_index(
    bindings: &dyn PdfiumLibraryBindings,
    dest: &PdfDocument<'_>,
    src: &PdfDocument<'_>,    // may be the SAME document -> duplicate a page
    indices: &[u16],          // 0-based source pages
    at: u16,                  // 0-based insertion point in `dest`
) -> Result<(), EngineError>;

pub fn flatten(
    bindings: &dyn PdfiumLibraryBindings,
    page: &PdfPage<'_>,
    mode: FlattenMode,
) -> Result<FlattenOutcome, EngineError>;   // Flatten + FPDFPage_GenerateContent

pub fn page_count(bindings: &dyn PdfiumLibraryBindings, doc: &PdfDocument<'_>) -> u16;

// src-tauri/src/engine/raw/save.rs
pub enum SaveFlags { Default, NoIncremental, RemoveSecurity }   // 0 / 2 / 4

pub fn save_as_copy(
    bindings: &dyn PdfiumLibraryBindings,
    doc: &PdfDocument<'_>,
    flags: SaveFlags,
) -> Result<Vec<u8>, EngineError>;

pub fn save_with_version(
    bindings: &dyn PdfiumLibraryBindings,
    doc: &PdfDocument<'_>,
    flags: SaveFlags,
    version: i32,             // 14 = PDF 1.4, 17 = PDF 1.7
) -> Result<Vec<u8>, EngineError>;
```

Caller contract:

| wrapper | what the caller must do |
|---|---|
| `move_pages`, `import_pages_by_index` | run inside `registry::mutate(..).structural()` — it flushes the page LRU *before* the closure, which both calls need because they bypass pdfium-render's `PdfPageIndexCache`. Arguments are validated here (duplicates, range, destination), so an `Err` means "nothing happened". If PDFium itself returns false the document may be inconsistent; returning that `Err` from the `mutate` closure is the right response. |
| `flatten` | the `FPDF_PAGE` is **invalid afterwards** — `OpenDoc::invalidate_page(i)`, then `OpenDoc::page(i)`, before rendering or reading. Use `FlattenMode::NormalDisplay`: `FLAT_PRINT` *deletes* annotations without `/F 4`. (SeePDF sets `/F 4` on everything it creates, so `Print` is also safe for our own annotations — but not for a file that came from elsewhere.) |
| `save_as_copy` | call `render::tiles::generate_appearances(st, doc_id)` first, or markup annotations are saved without an `/AP` and are invisible in Preview and Acrobat. `SaveFlags::NoIncremental` is the v1 save; `RemoveSecurity` is value **4** (3 is the deprecated alias and does nothing on build 8057). |

`engine::raw::annot` (the `FPDFAnnot_*` pipeline, with an RAII `AnnotRef`) and the Stage-1 half of
`engine::raw::form` (`set_focused_annot`, `select_all_text`, `replace_selection`, `click`, `type_text`,
`backspace`, `set_index_selected`, `is_index_selected`) are (a)'s own surface. (b) should not need them;
the one exception worth knowing is `AnnotRef::set_flags` to set `/F 4` before a `FLAT_PRINT` flatten.

---

## 2. Support matrix

Legend: **create** = `create_annotation` produces it · **edit** = `update_annotation` changes its
properties · **render** = PDFium draws it (verified by counting changed pixels in the annotation's rect)
· **persist** = it survives `save_to_bytes` → reopen → enumerate with its properties intact.

| Kind | On disk | create | edit | render | persist | notes |
|---|---|---|---|---|---|---|
| highlight | `Highlight` + `/QuadPoints` | ✔ | ✔ | ✔ | ✔ | one quad per line rect, `/Rect` = their union |
| underline | `Underline` | ✔ | ✔ | ✔ | ✔ | |
| strikeout | `StrikeOut` | ✔ | ✔ | ✔ | ✔ | |
| squiggly | `Squiggly` | ✔ | ✔ | ✔ | ✔ | |
| ink | `Ink` + real `/InkList` | ✔ | ✔ | ✔ | ✔ | multi-stroke; width via `FPDFAnnot_SetBorder` |
| square | `Square` | ✔ | ✔ | ✔ | ✔ | stroke + interior colour + border width |
| circle | `Circle` | ✔ | ✔ | ✔ | ✔ | **raw only** — no `create_circle_annotation` in 0.9.4 |
| line | `Ink` + `/Subj "SeePDF:Line"` | ✔ | ✔ | ✔ | ✔ | one 2-point stroke; `linePoints` read back from it |
| arrow | `Ink` + `/Subj "SeePDF:Arrow"` | ✔ | ✔ | ✔ | ✔ | segment + two head strokes (3 strokes) |
| note | `Text` | ✔ | ✔ | ✔ | ✔ | PDFium's note-icon appearance; popup is a React concern |
| textbox | `Stamp` + text objects + `/Subj "SeePDF:TextBox"`, `/DA` | ✔ | ✔ (rebuild) | ✔ | ✔ | **한글 verified**; size and colour live in `/DA` |
| stamp (image) | `Stamp` + image object | ✔ | move only | ✔ | ✔ | PNG and JPEG (the `image` crate features we ship) |
| stamp (built-in) | `Stamp` + border + label | ✔ | move only | ✔ | ✔ | `approved` / `draft` / `confidential` / … , `/Subj` = the name |
| signature (drawn) | `Ink` + `/Subj "SeePDF:Signature"` | ✔ | ✔ | ✔ | ✔ | same pipeline as ink |
| signature (image) | `Stamp` + image object | ✔ | move only | ✔ | ✔ | created through `stamp`; there is no image variant in `AnnotSpec` |
| link | `Link` + `FPDFAnnot_SetURI` | ✔ engine only | — | invisible by design | ✔ | `AnnotSpec` has no `link` variant (P2); `annot::create::create_link` exists and is tested |
| widget | `Widget` | — | via forms | ✔ | ✔ | read-only as an annotation; see §3 |
| line (foreign) | `Line` | ✘ | move only | ✔ | ✔ | `FPDFPage_CreateAnnot(FPDF_ANNOT_LINE)` returns NULL; existing ones read via `FPDFAnnot_GetLine` |
| polygon / polyline / caret / redact | — | ✘ | — | — | — | `CreateAnnot` returns NULL |
| popup | `Popup` | ✘ | — | ✘ | — | cannot be linked to a parent and PDFium never draws a standalone one; filtered out of `list_annotations` |

Commands, all with real bodies: `list_annotations`, `scan_annotations`, `create_annotation`,
`update_annotation`, `delete_annotations`, `set_annotations_hidden`, `list_form_fields`,
`set_form_field_value`, `reset_form`, `redact_preview`, `apply_redactions`.

### 2.1 The colour mirror — the one design decision that is not in any spike

`FPDFAnnot_GetColor` **refuses to read `/C` or `/IC` once the annotation has an `/AP`** (PDFium: "do not
attempt to get the colour if the annotation contains an AP stream") — the same guard that makes
`FPDFAnnot_SetColor` fail, which the spike documented only for the setter. Appearance streams are
generated on the first render, so *every* annotation reported black after a save and reopen.

There is no other colour reader in PDFium's public API (`/C` is an array, and the only array accessors
are for ink lists, quads and vertices). So `engine::annot` mirrors the colour into two private keys of
the annotation dictionary, `/SeePDFC` and `/SeePDFIC` (`"r g b"`), written by every create and update and
read back first. Legal PDF, ignored by other viewers, and the only way to round-trip a colour.

Annotations from other producers have neither a readable `/C` nor the mirror: they report the default
black and are marked `editable: "moveOnly"`, which is the honest answer — regenerating their appearance
would throw away what the other application drew.

### 2.2 Other things worth knowing

* **`update_annotation` is `SetAP(NULL)` first** (except for Stamps, two bullets down), then the
  setters, then the next render regenerates. `annot_recolor_after_reopen` runs that path **in a child process** and asserts exit
  status 0, so the SIGSEGV of annotations spike §5.1 shows up as a failed test, not as a corrupted run.
* Two patches cannot be applied in place and are done as **delete + re-create with the same `/NM`**:
  a markup `rects` list of a different length (PDFium can append and replace a quad but never remove
  one) and a text box's `text` / `fontSize` / `color` / `fillColor` (the glyphs are baked page objects).
  `annot_update_markup_rects_rebuild` and `annot_textbox_edit_keeps_its_appearance` pin that the id,
  contents and author survive.
* **A Stamp's appearance stream *is* its content** (the image and the text objects live inside it,
  appended by `FPDFAnnot_AppendObject`) and PDFium never regenerates a Stamp appearance. So
  `update_annotation` skips the `SetAP(NULL)` step — and the colour and border setters that need it —
  for Stamp-backed kinds; clearing the AP there would erase the annotation.
* **Opacity has no setter.** `/CA` is the alpha of the last `FPDFAnnot_SetColor` call, so stroke and
  interior are always written with the same alpha. `FPDFAnnot_GetNumberValue("CA")` *does* work with an
  `/AP` present (no guard there), so opacity round-trips without a mirror.
* Every created annotation gets `/F 4` (Print), `/NM` (uuid v4), `/CreationDate` and `/M`.
* `list_annotations` assigns a `/NM` to annotations that arrive without one, as `IPC_CONTRACT.md` §7.1
  requires. That writes to the in-memory document but deliberately does **not** bump the generation or
  dirty the document — it is an identity stamp, not a user edit.
* Reads (`list_annotations`, `list_form_fields`) go through the **page LRU**; writes open their own
  page (`annot::ScratchPage`), because building a Stamp needs `&PdfDocument` and `&mut PdfPage` at the
  same time and `OpenDoc::page()` borrows the whole `OpenDoc`. `ScratchPage::open` calls
  `invalidate_page` first so two handles never straddle an edit.
* **Korean text boxes need a font.** `font::resolve` picks `helvetica()` for Latin-1 text, then
  `resources/fonts/SeePDF-Hangul.ttf` (Stage 1 (b)'s deliverable), then a host font as a development
  fallback. **That file does not exist yet**, so `annot_textbox_korean` currently embeds
  `/System/Library/Fonts/Supplemental/AppleGothic.ttf` and the resulting
  `fixtures/out/stage1a/textbox-korean.pdf` is **7.2 MB** instead of ~1 MB. Re-run the test once (b)
  commits the subset. `FontChoice::source` says which font was used.

### 2.3 Redaction

`apply_redactions` removes content; it does not cover it. The order is what makes the rollback in
`IPC_CONTRACT.md` §7.5 real, because **`registry::mutate` does not undo a closure that failed half-way**
— its contract is "return `Err` only if nothing happened" and deleting a page object cannot be undone
from inside the closure. So:

1. a form field under a mark → `unsupported` (its value lives in the AcroForm dictionary, which PDFium
   cannot rewrite);
2. a marked run whose characters belong to **no removable page object** → `verifyFailed`, decided on an
   untouched document. This is the real case, not a hypothetical: `TAMReview.pdf` page 1 keeps 2738 of
   its 2779 characters inside Form XObjects, which `remove_object_at_index` cannot reach. A naive
   implementation deletes nothing, draws the box and reports success;
3. objects removed (descending), fill boxes drawn, `regenerate_content()` once;
4. the page text is re-extracted and each marked run **counted** again — counting, not "is it absent",
   so redacting one "the" on a page full of them is not reported as a failure;
5. `redact::apply_verified` takes a byte snapshot before all of this and restores it if either
   verification point fires, so the document is byte-for-byte what it was. `redact_verify_rollback`
   asserts the generation, the text and the rendered pixels are unchanged.

`redact_preview` reports the collateral before the destructive step, computed per character from the
text layer's `tight` boxes and `object_id`: PDFium cannot split a text object, so redacting "Gal" takes
"Andreas Gal" with it and the user is told exactly that.

---

## 3. The `FORM_*` method actually used — `WORKPLAN.md` §6 probe: **resolved, the fast path works**

> `FORM_SetFocusedAnnot` + `FORM_SelectAllText` + `FORM_ReplaceSelection` (unverified)

**Verified working** on `160F-2019.pdf`, and it is what ships. `engine::form::set_text` tries it first
and reports `TextEntryMethod::ReplaceSelection`; `form_text_value_persists` asserts a documented path
ran and prints which, and `form_probe_replace_selection` records the step-by-step result:

```
SetFocusedAnnot=true  SelectAllText=true  value_before_commit=None
value_after_ReplaceSelection=Some("PROBE")
|| click=(true,true)  SelectAllText=true  value_after_OnChar=Some("CHAR")
```

Two findings that cost time and are not in the spike:

1. **`FORM_ForceToKillFocus` is part of the probe, not an afterthought.**
   `FPDFAnnot_GetFormFieldValue` reads the committed `/V`, and the form layer only commits on
   kill-focus. Reading the value straight after `FORM_ReplaceSelection` always returns the *old* value,
   which made the fast path look broken and silently selected the fallback.
2. **Success cannot be an exact string comparison.** A `/MaxLen` (every comb field has one) truncates
   the input server-side, so `form::accepted()` asks "is the field now a non-empty prefix of what we
   asked for". `160F-2019.pdf`'s comb fields cap at 16 characters.

The verified fallback (click the widget centre → `FORM_SelectAllText` → backspace → one `FORM_OnChar`
per UTF-16 unit) is still implemented and still exercised, because it is what runs whenever the fast
path does not take. Both end in `FORM_ForceToKillFocus`, which is what regenerates `/AP` — that is the
whole point, and why `PdfFormTextField::set_value` is used nowhere in the tree.

Checkbox / radio: PDFium has no "set state" call, so the engine clicks the widget centre and reads
`FPDFAnnot_IsChecked` back, clicking once more if needed. Combo / list: `FORM_SetFocusedAnnot` then
`FORM_SetIndexSelected` per index.

`160F-2019.pdf` census (asserted by `form_lists_every_widget`): 76 widgets — 64 text, 10 push buttons,
2 radio buttons, **no checkbox**. The checkbox path is therefore implemented and type-checked but not
covered by a fixture; a document with checkboxes should be added to `fixtures/` (§5).

---

## 4. Unsupported / deferred

| Item | Status | Why |
|---|---|---|
| `link` creation from the UI | engine-only | `AnnotSpec` (frozen) has no `link` variant — link creation is P2 in `IPC_CONTRACT.md` §7.1. `annot::create::create_link(doc, page, rect, quads, uri)` is implemented and tested; a one-line spec variant would expose it. |
| go-to-page links | ✘ | `FPDFAnnot_SetURI` is the only link writer in PDFium's API; `/Dest` needs `lopdf` (P1). |
| Line / Polygon / Polyline / Caret / Redact annotation subtypes | ✘ | `FPDFPage_CreateAnnot` returns NULL. Lines and arrows ship as Ink + `/Subj`. |
| Popup annotations | ✘ | cannot be linked to a parent, never drawn standalone. Filtered out of `list_annotations`; the UI draws popups from `contents` / `author` / `modified`. |
| FreeText | not used | persists only with `/DA`, and `/Helv` cannot render 한글. Text boxes are Stamp + text objects. Existing FreeText annotations from other producers are read and reported as `textbox`. |
| `/MaxLen`, comb cell count | not reported | `FormField.maxLen` is always `null`: PDFium exposes no reader. The field still truncates correctly; the UI cannot pre-empt it. |
| radio "clear" | ✘ | a radio cannot be switched off by clicking it. `set_form_field_value({checked:false})` returns `unsupported` with a message the UI can explain; select another button of the group instead. |
| `reset_form` (P1) | partial | clears text, combo and list fields and unticks checkboxes. **Radio groups keep their selection**, and it resets to *empty*, not to `/DV` (PDFium exposes no default-value reader). |
| redaction of partially covered images | box only | images and paths are removed only when **entirely** inside a mark. A partially covered image keeps its pixels under the box. P2: `get_raw_image` → blank the region → `set_image`, matrix preserved (`ARCHITECTURE.md` §6.5). |
| word-level redaction | ✘ | PDFium cannot split a text object; the whole run goes. P2 is re-creating the surviving characters as new text objects. |
| redaction over text inside a Form XObject | refused | reported as `verifyFailed` with the surviving text in the message, rather than a fake redaction. P2: flatten the XObject first. |
| redaction over a form field | refused | the value lives in `/AcroForm`, which PDFium cannot rewrite. Flatten the form first (Stage 1 (b) owns flatten). |
| text box alignment across an edit | lost | `Annot` (frozen) has no `align` field, so a rebuild (`text` / `fontSize` / `color` / `fillColor` patch) cannot recover it and falls back to `left`. The UI can re-send `align` by re-creating the box, or `Annot` gains the field. |
| `image_id` on stamps | always `null` | there is no image store yet; the contract field is reserved for one. |
| stamp `rotate` | recorded, not applied | kept in a private `/SeePDFRotate` key. PDFium has no annotation rotation entry; the UI can rotate the source image instead. |

---

## 5. Integration requests

Nothing here blocks (a); each is a workaround that is already in place, with the patch the owner would
apply.

### 5.1 `registry.rs` — `OpenDoc::invalidate_page` also drops the text layer (Stage 0 / integrator)

`invalidate_page(i)` does `pages.invalidate(i)` **and** `text.invalidate_page(i)`. Annotation writes only
need the page handle dropped; throwing away the text layer costs a 5.6 ms rebuild the next time the
viewer selects text on that page. Requested patch:

```rust
    /// Drops one page handle (after a rotate, or when its content changed under it).
    pub fn invalidate_page(&mut self, index: u16) {
        self.pages.invalidate(index);
        self.text.invalidate_page(index);
    }

+   /// Drops the page handle only. For edits that change the annotation dictionary but not
+   /// the page content stream, where the extracted text is still valid.
+   pub fn invalidate_page_handle(&mut self, index: u16) {
+       self.pages.invalidate(index);
+   }
```

Worked around locally: the read paths (`annot::read::list_page`, `form::list`) use the LRU page instead
of a `ScratchPage`, so a plain listing no longer invalidates anything. Only writes pay the rebuild,
which is correct anyway because their page content may have changed.

### 5.2 `registry.rs` — `mutate` does not restore the document when the closure fails (Stage 0 / integrator)

`STAGE0_NOTES.md` §1.2 says "**If your closure returns `Err`, nothing happened**: the generation is not
bumped and the snapshot is rolled back". The implementation only discards the *snapshot*; the document
keeps whatever the closure did before it failed. That is fine as a contract ("fail only before you
change anything") but it is not what the sentence says, and `apply_redactions` cannot honour it —
`remove_object_at_index` is not reversible from inside the closure.

Either the documentation should say "the closure must not mutate before returning `Err`", or `mutate`
should restore from the snapshot it already holds:

```rust
     let out = match f(doc) {
         Ok(out) => out,
         Err(e) => {
-            // Undo the bookkeeping: the snapshot describes a state we never left.
-            let _ = doc.history.take_undo(doc.bytes.clone());
+            // Restore: the closure may have changed the document before failing.
+            if let Ok(Some((_, bytes))) = doc.history.take_undo(doc.bytes.clone()) {
+                replace(st, doc_id, bytes)?;
+            }
             return Err(e);
         }
     };
```

Worked around locally: `redact::apply_verified` takes its own byte snapshot and calls
`registry::replace` itself when the redaction reports `verifyFailed`. If `mutate` grows the behaviour
above, that wrapper collapses to a plain `mutate` call.

### 5.3 `ipc/types.rs` — two frozen-file notes (integrator)

* `AnnotSpec` has no `link` variant, so `create_annotation` can never create a link although
  `AnnotKind::Link` exists and `Annot.uri` is populated on read. `engine::annot::create::create_link` is
  ready; exposing it needs
  `Link { rect: Rect, rects: Vec<Rect>, uri: String }` added to the enum (and the TS mirror).
* `AnnotSpec::Stamp` carries no author, contents or `/Subj`, so an **image signature** is created as a
  plain stamp and reports `kind: "stamp"`, not `"signature"`. A drawn signature (`AnnotSpec::Signature`,
  an ink spec) does report `"signature"`. If the UI needs the distinction for image signatures, the
  cheapest fix is a `subj` / `kind` field on `StampSpec`.

### 5.4 Files outside (a)'s row that were touched

Two `mod.rs` files, module declarations only — there is no other way to register a module:

* `src-tauri/src/engine/mod.rs`: `+pub mod annot; +pub mod form; +pub mod redact;`
* `src-tauri/src/engine/raw/mod.rs`: `+pub mod annot; +pub mod page; +pub mod save;`
  (`raw/**` *is* (a)'s row; listed for completeness.)

Nothing else outside the row changed: `git status` shows `commands/{annots,forms,security}.rs`,
`engine/raw/{consts,form}.rs` and the new `engine/{annot,form,redact}/**`, `engine/raw/{annot,page,
save}.rs`, `tests/{annot,form,redact}.rs`, `docs/STAGE1A_NOTES.md`.

### 5.5 Fixtures (whoever owns `examples/gen_fixtures.rs`)

`160F-2019.pdf` has **no checkbox and no combo/list field**, so `set_form_field_value`'s checkbox and
choice paths are implemented and type-checked but not covered by a test. A small generated fixture with
one checkbox, one combo and one list box would close that gap.

---

## 6. Test inventory

```sh
cd src-tauri
cargo test --release annot_ form_ redact_    # the DoD filters
cargo test --release                         # everything, Stage 0 included
```

| DoD name | file | what it proves |
|---|---|---|
| `annot_markup_roundtrip` | `tests/annot.rs` | four markup types, two quads, opacity, contents, `/F 4`, visible after reopen |
| `annot_quad_order` | | the quad is the rect, not a 1-px sliver, and it paints its area |
| `annot_ink_roundtrip` | | a real `/InkList` with two strokes and the point coordinates intact; signature `/Subj` |
| `annot_shapes_roundtrip` | | square + circle (raw), interior colour, border width, `/CA 0.5` |
| `annot_line_subj_roundtrip` | | Ink + `/Subj` round-trips as `line` / `arrow`; the arrow has three strokes |
| `annot_textbox_korean` | | 한글 renders from an embedded CID font; `/DA` carries the size and colour |
| `annot_stamp_image_roundtrip` | | PNG stamp covers its rect; built-in label stamp |
| `annot_recolor_after_reopen` | | **child process**, exit status 0: the `SetAP(NULL)` path does not SIGSEGV |
| `annot_delete_persists` | | one of two annotations deleted, survives save + reopen; an unknown id is `notFound` and changes nothing |
| `form_text_value_persists` | `tests/form.rs` | the §6 probe; value visible in the render **and** after reopen |
| `form_radio_toggle` | | click + `IsChecked`, persists; clearing a radio is refused |
| `redact_removes_text_from_saved_file` | `tests/redact.rs` | the word is gone from the reopened file and the box is drawn |
| `redact_verify_rollback` | | a real `verifyFailed` (XObject text) restores text, generation and pixels |

Plus `annot_raw_wrappers_smoke` (§1, the surface (b) consumes), `annot_link_roundtrip`,
`annot_hidden_is_transient`, `annot_update_markup_rects_rebuild`, `annot_ids_assigned_to_foreign_annotations`,
`annot_reads_foreign_line`, `annot_textbox_edit_keeps_its_appearance`, `form_lists_every_widget`, `form_rejects_bad_writes`,
`form_reset_clears_text_fields`, `form_probe_replace_selection`, `redact_preview_reports_collateral`,
`redact_multiple_rects_and_fill_colour`, `redact_refuses_before_changing_anything`,
`redact_handles_a_repeated_word`, and unit tests for the quad corner order, the `/DA` parser, the arrow
head geometry, the survivor counting rule and the 7881 constants.

### QA-4 artifacts

`fixtures/out/stage1a/` (gitignored) is written by the tests, one saved PDF plus a full-page PNG per
family, for opening in macOS Preview and Acrobat:
`markup`, `shapes`, `ink`, `line-arrow`, `stamp`, `textbox-korean`, `textbox-edited`, `delete`,
`recolour`, `form-filled.pdf`, `form-radio.pdf`, `redacted.pdf`.

Every saved file carries a real `/AP` for every annotation (checked with `grep -a '/AP'`: 4 for
`markup.pdf`, 2 for `shapes.pdf`, …), which is what makes them visible in viewers that do not synthesise
appearances. **The human half of QA-4 — opening these in Preview and in Acrobat — has not been done and
is still owed.**
