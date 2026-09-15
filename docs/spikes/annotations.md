# SeePDF — Annotations, AcroForm, flatten & redaction spike (pdfium-render 0.9.4)

**Status:** spike complete, compiles and runs. **Date:** 2026-09-15. **Engine:** pdfium-render 0.9.4 (feature `thread_safe`,
bindgen `pdfium_7881`) driving PDFium chromium/8057 (`src-tauri/resources/pdfium/libpdfium-aarch64-apple-darwin.dylib`).
**Code:** `src-tauri/examples/spike_annot.rs` — run with `cd src-tauri && cargo run --example spike_annot`
(~2 s wall clock, writes `fixtures/out/spike_annot_*.{pdf,png}`; every claim below is printed as a `RESULT | …` line).
**Audience:** the agents implementing the annotation tool layer, the form-fill layer, flatten/export and redaction.

Read §1 (what works and how) and §5 (gotchas) before writing any annotation code. Everything marked **raw** needs the raw
FFI trait, not the high-level pdfium-render API — §2 explains how to get at it.

---

## 0. TL;DR

| Topic | Verdict |
|---|---|
| Enumerate annotations (type, rect, contents, author, dates, colour, flags, quads, objects) | High-level API is complete for common properties. Opacity (`/CA`), border width, ink paths, line endpoints, popup/parent links, appearance stream are **raw-only** reads. |
| Delete annotation, save, reopen | Works (`delete_annotation` → `FPDFPage_RemoveAnnot`), persisted. |
| Create highlight / underline / strikeout / squiggly with explicit quads | Works and renders **only if the QuadPoints are in PDFium order (TL, TR, BL, BR)**. `PdfQuadPoints::from_rect()` uses BL,BR,TR,TL and yields an invisible 1-px sliver. Use the `quad_tl()` helper from the spike. |
| Ink | Two ways. High-level: path objects appended to the ink annotation (`objects_mut().add_path_object`) → AP only, no `/InkList`. Raw: `FPDFAnnot_AddInkStroke` + `FPDFAnnot_SetBorder` → real `/InkList` + generated AP. Both render and persist. |
| Square / Circle | Square: high-level. Circle: **no `create_circle_annotation` in 0.9.4** → raw `FPDFPage_CreateAnnot(page, FPDF_ANNOT_CIRCLE)`. Both render + persist (interior colour, border width, opacity all honoured). |
| Line, Polygon, Polyline, Redact, Caret | **PDFium cannot create them** (`FPDFPage_CreateAnnot` returns NULL). Existing Line annotations are read/rendered fine (`FPDFAnnot_GetLine`). |
| FreeText | Creatable. PDFium generates a persisted AP **only if `/DA` is set** (raw `FPDFAnnot_SetStringValue_str(annot,"DA","0 0 1 rg /Helv 12 Tf")`). Without DA the high-level FreeText is invisible after save. Portable alternative: Stamp + text object (works, renders, persists, handles 한글). |
| Stamp with image | High-level works (`PdfPageImageObject::new_with_size` + `translate` **before** `add_image_object`). Persisted, renders. |
| Link (URI) | Works (`create_link_annotation(uri)`, quads OK). **Go-to-page (`/Dest`) links cannot be created** with PDFium's public API. |
| Text (sticky note) | Creatable; PDFium generates a "note" icon AP (rounded box with lines). Persisted. |
| Popup | Creatable but **cannot be linked to a parent** (`/Popup`/`/Parent` are indirect refs, no API) and PDFium never draws standalone Popup annots → invisible. Use the UI layer for popups. |
| Opacity | `FPDFAnnot_SetColor` alpha → `/CA`. High-level: `set_stroke_color(PdfColor::new(r,g,b,alpha))`. **Every SetColor call rewrites `/CA`**, so pass the same alpha for stroke and fill. |
| Appearance streams | PDFium generates APs for Circle, Highlight, Ink, Popup, Square, Squiggly, StrikeOut, Text, Underline (and FreeText-with-DA) **lazily when the page is rendered** (`CPDF_AnnotList` construction) and writes them into the annotation dict, so **render once before saving** if other viewers must see the annotation. Saving without a render leaves no `/AP` (Acrobat/Preview would have to synthesise it themselves). |
| Modify existing annotations | Rect move/resize and contents: fine. **Colour/opacity change on an annotation that already has an AP: `FPDFAnnot_SetColor` returns false and pdfium-render's fallback SEGFAULTS (signal 11)** — see §5.1. Correct recipe (raw): `FPDFAnnot_SetAP(annot, NORMAL, NULL)` → set colour → render → AP regenerated (verified, persisted). |
| Flatten | `PdfPage::flatten()` = `FPDFPage_Flatten(FLAT_PRINT)` + reload. **Annotations without the Print flag are silently deleted, not flattened.** pdfium-render never sets the Print flag on new annotations. Either `set_is_printed(true)` on creation or use raw `FPDFPage_Flatten(page, FLAT_NORMALDISPLAY)` (verified: keeps everything visible). |
| AcroForm read | `document.form()` + `annotation.as_form_field()`; name/type/value/options/rect all fine (160F-2019.pdf: 64 text, 10 push buttons, 2 radios; **no checkboxes** in that fixture). |
| AcroForm write | High-level `set_value`/`set_checked` only writes `/V` (+`/AS`); **no appearance regeneration → value invisible in PDFium and after save**. Raw `FORM_OnLButtonDown/Up` + `FORM_OnChar` + `FORM_ForceToKillFocus` regenerates the AP (896-byte stream), renders with `FPDF_FFLDraw`, persists (Korean text OK). |
| Redaction (approach A) | Remove text objects overlapping the rect + black rect + `regenerate_content()`; text gone from extraction after save (verified). PDFium cannot split a text object, so the **whole object** goes (collateral: "Andreas Gal" removed to redact "Gal"). Redact annotations (approach B) cannot be created. |

---

## 1. Verified API (exact signatures, pdfium-render 0.9.4)

### 1.1 Binding and the raw escape hatch

```rust
Pdfium::bind_to_library(path: impl AsRef<Path>) -> Result<Box<dyn PdfiumLibraryBindings>, PdfiumError>
Pdfium::new(bindings: Box<dyn PdfiumLibraryBindings>) -> Pdfium       // seals a global OnceCell
pdfium.load_pdf_from_file(path: &(impl AsRef<Path> + ?Sized), password: Option<&str>) -> Result<PdfDocument<'_>, PdfiumError>
document.save_to_file(path: &(impl AsRef<Path> + ?Sized)) -> Result<(), PdfiumError>
```

pdfium-render 0.9.4 exposes **no public raw handle**: `PdfDocument::handle()`, `PdfPage::page_handle()`, `PdfForm::handle()`,
every annotation `handle()` and `PdfPageObjectOwnership` are `pub(crate)`. The only way to call the ~30 FFI functions the
high-level API does not wrap is a **second, separate binding**:

```rust
let raw: &'static dyn PdfiumLibraryBindings = Box::leak(Pdfium::bind_to_library(&lib)?); // BEFORE Pdfium::new
let pdfium = Pdfium::new(Pdfium::bind_to_library(&lib)?);
```

Both are `dlopen`s of the same library (shared global state); `bind_to_library` only refuses once `Pdfium::new` has run.
The prelude re-exports the FFI types you need (`FPDF_DOCUMENT`, `FPDF_PAGE`, `FPDF_ANNOTATION`, `FPDF_FORMHANDLE`,
`FS_POINTF { x, y }`, `FS_RECTF { left, top, right, bottom }`, `FS_QUADPOINTSF`, `FS_MATRIX`, `FPDF_FILEWRITE`,
`FPDF_FORMFILLINFO`, `FPDF_WCHAR`) but **not the constants** (subtype ids, `FPDF_ANNOT`, `FLAT_*`, colour types) — copy
them from `bindgen/pdfium_7881.rs` (the spike has them). A raw document and a pdfium-render document are different
`FPDF_DOCUMENT`s; the practical pattern is "raw pipeline for the edit, pdfium-render for reading/rendering the result",
or (recommended, §6) one thin raw wrapper for everything annotation/form related.

Raw save needs a `FPDF_FILEWRITE { version: 1, WriteBlock: Some(cb) }` placed first in a `#[repr(C)]` struct with the
`Vec<u8>` buffer (`write_block` in the spike); `FPDF_SaveAsCopy(doc, &mut fw, 0)`.

### 1.2 Annotation collection (`page.annotations()` / `page.annotations_mut()`)

```rust
PdfPageAnnotations::len(&self) -> usize
PdfPageAnnotations::get(&self, index: usize) -> Result<PdfPageAnnotation<'a>, PdfiumError>
PdfPageAnnotations::iter(&self) -> PdfPageAnnotationsIterator<'_>
PdfPageAnnotations::delete_annotation(&mut self, annotation: PdfPageAnnotation<'a>) -> Result<(), PdfiumError>
// constructors (each calls FPDFPage_CreateAnnot and stamps /CreationDate):
create_free_text_annotation(&mut self, text: &str) -> Result<PdfPageFreeTextAnnotation<'a>, PdfiumError>
create_highlight_annotation(&mut self)             -> Result<PdfPageHighlightAnnotation<'a>, PdfiumError>
create_underline_annotation / create_strikeout_annotation / create_squiggly_annotation(&mut self) -> Result<…, PdfiumError>
create_ink_annotation(&mut self)                   -> Result<PdfPageInkAnnotation<'a>, PdfiumError>
create_square_annotation(&mut self)                -> Result<PdfPageSquareAnnotation<'a>, PdfiumError>
create_stamp_annotation(&mut self)                 -> Result<PdfPageStampAnnotation<'a>, PdfiumError>
create_link_annotation(&mut self, uri: &str)       -> Result<PdfPageLinkAnnotation<'a>, PdfiumError>   // FPDFAnnot_SetURI
create_text_annotation(&mut self, text: &str)      -> Result<PdfPageTextAnnotation<'a>, PdfiumError>
create_popup_annotation(&mut self)                 -> Result<PdfPagePopupAnnotation<'a>, PdfiumError>
// convenience: create_{highlight_over,underline_under,strikeout_through,squiggly_under}_object(&mut self, object: &PdfPageObject, color: PdfColor, contents: Option<&str>)
```

Missing constructors: **Circle** (type exists, `PdfPageAnnotation::Circle`, but no `create_circle_annotation`), Line,
Polygon, Polyline, Redact, Caret, FileAttachment. Delete must go through `annotations_mut().get(i)` (the value returned by
`annotations().get(i)` borrows the page immutably and cannot be passed to `delete_annotation`).

### 1.3 `PdfPageAnnotationCommon` (blanket impl on every annotation type; import via prelude)

```rust
fn annotation_type(&self) -> PdfPageAnnotationType       // on the enum
fn is_supported(&self) -> bool                            // false for Line/Polygon/… (still exposes common props)
fn name(&self) -> Option<String>                          // /NM
fn bounds(&self) -> Result<PdfRect, PdfiumError>          // FPDFAnnot_GetRect, page space, y up
fn set_bounds(&mut self, bounds: PdfRect) -> Result<(), PdfiumError>   // FPDFAnnot_SetRect (+ /M)
fn set_position / set_width / set_height
fn contents(&self) -> Option<String>;  fn set_contents(&mut self, &str)          // /Contents
fn creator(&self) -> Option<String>;   fn set_creator(&mut self, &str)           // /T (author)
fn creation_date(&self) -> Option<String>; fn set_creation_date(&mut self, DateTime<Utc>)   // /CreationDate, raw "D:…" string
fn modification_date(&self) -> Option<String>; fn set_modification_date(&mut self, DateTime<Utc>)  // /M (auto-updated by every setter)
fn stroke_color(&self) -> Result<PdfColor, PdfiumError>; fn set_stroke_color(&mut self, PdfColor)   // /C  (+ /CA from alpha)
fn fill_color(&self) -> Result<PdfColor, PdfiumError>;   fn set_fill_color(&mut self, PdfColor)     // /IC (+ /CA from alpha)
fn is_hidden / set_is_hidden, is_printed / set_is_printed, is_zoomable, is_rotatable, is_read_only, is_locked, is_editable …  // /F flags
fn objects(&self) -> &PdfPageAnnotationObjects<'_>       // FPDFAnnot_GetObjectCount/GetObject (AP contents; read-only except Ink/Stamp)
fn attachment_points(&self) -> &PdfPageAnnotationAttachmentPoints<'_>   // QuadPoints
```

`PdfRect::new_from_values(bottom, left, top, right)` — note the argument order. `PdfColor::new(r, g, b, a)`.
There is **no** opacity, border, ink-list, line, vertices, popup-link, appearance-stream or generic key getter/setter.

### 1.4 Attachment points (QuadPoints) — text markup + Link

```rust
PdfPageAnnotationAttachmentPoints::len / get(index) -> Result<PdfQuadPoints, PdfiumError> / iter()
PdfPageAnnotationAttachmentPoints::create_attachment_point_at_end(&mut self, PdfQuadPoints) -> Result<(), PdfiumError>  // FPDFAnnot_AppendAttachmentPoints
PdfPageAnnotationAttachmentPoints::set_attachment_point_at_index(&mut self, index, PdfQuadPoints) -> Result<(), PdfiumError>
PdfQuadPoints::new_from_values(x1, y1, x2, y2, x3, y3, x4, y4)      // use PDFium order: TL, TR, BL, BR
PdfQuadPoints::from_rect(&PdfRect)   // WRONG ORDER for PDFium (BL,BR,TR,TL) — do not use for markup
PdfQuadPoints::to_rect() / left() / right() / bottom() / top() / width() / height()   // order-agnostic, OK
```

`attachment_points_mut()` exists on Highlight/Underline/Strikeout/Squiggly/Link. Appending quads does not touch `/Rect`;
set `set_bounds` to the union of the quads yourself (Acrobat uses `/Rect` for hit-testing).

### 1.5 Ink / Stamp page objects

```rust
PdfPageInkAnnotation::objects_mut(&mut self) -> &mut PdfPageAnnotationObjects<'a>
PdfPageStampAnnotation::objects_mut(&mut self) -> &mut PdfPageAnnotationObjects<'a>
PdfPageObjectsCommon::add_path_object(&mut self, PdfPagePathObject<'a>) -> Result<PdfPageObject<'a>, PdfiumError>   // FPDFAnnot_AppendObject
PdfPageObjectsCommon::add_image_object(&mut self, PdfPageImageObject<'a>) / add_text_object(&mut self, PdfPageTextObject<'a>)
PdfPagePathObject::new(document: &PdfDocument, x: PdfPoints, y: PdfPoints, stroke: Option<PdfColor>, stroke_width: Option<PdfPoints>, fill: Option<PdfColor>) -> Result<Self, PdfiumError>
PdfPagePathObject::line_to(&mut self, x, y) / move_to / close_path; ::new_rect(document, rect, stroke, width, fill); ::new_line(...)
PdfPageImageObject::new_with_size(document: &PdfDocument, image: &image::DynamicImage, width: PdfPoints, height: PdfPoints) -> Result<Self, PdfiumError>
PdfPageTextObject::new(document: &PdfDocument, text: impl ToString, font: impl ToPdfFontToken, font_size: PdfPoints) -> Result<Self, PdfiumError>
object.translate(dx: PdfPoints, dy: PdfPoints) / scale(sx, sy) / set_fill_color(...)   // BEFORE add_*_object
document.fonts_mut().helvetica() -> PdfFontToken   // Copy; obtain before borrowing pages (fonts_mut needs &mut document)
```

`FPDFAnnot_AppendObject` creates the AP stream if missing with `BBox = /Rect` — **call `set_bounds` before appending or
the objects are clipped away**. pdfium-render never calls `FPDFAnnot_UpdateObject`, so transforms after appending are not
written back; transform first. Text objects get proper font resources in the AP (Helvetica; "한글" rendered via PDFium
fallback in the spike, embed a CJK font for portability — see the text spike).

### 1.6 Page-level

```rust
PdfPage::flatten(&mut self) -> Result<(), PdfiumError>            // FPDFPage_Flatten(page, FLAT_PRINT) + regenerate + FPDF_LoadPage
PdfPage::set_content_regeneration_strategy(&mut self, PdfPageContentRegenerationStrategy::{AutomaticOnEveryChange (default), AutomaticOnDrop, Manual})
PdfPage::regenerate_content(&mut self) -> Result<(), PdfiumError> // FPDFPage_GenerateContent
PdfPage::render_with_config(&self, &PdfRenderConfig) -> Result<PdfBitmap<'_>, PdfiumError>
PdfRenderConfig::new().scale_page_by_factor(f32).render_annotations(bool /*default true*/).render_form_data(bool /*default true*/)
PdfBitmap::as_image(&self) -> Result<image::DynamicImage, PdfiumError>   // image 0.25.10 == project `image` dep
PdfPage::text()?.search(text, &PdfSearchOptions::new()) -> PdfPageTextSearch; .find_next() -> Option<PdfPageTextSegments>; segment.bounds() -> PdfRect
PdfPage::objects().get(i)?.bounds() -> Result<PdfQuadPoints, PdfiumError>; .as_text_object()?.text(); objects_mut().remove_object_at_index(i); objects_mut().create_path_object_rect(rect, stroke, width, fill)
```

### 1.7 Forms

```rust
PdfDocument::form(&self) -> Option<&PdfForm<'_>>         // FPDFDOC_InitFormFillEnvironment happens at document load
PdfForm::form_type(&self) -> PdfFormType                  // Acrobat for 160F-2019.pdf
PdfForm::field_values(&self, pages: &PdfPages) -> HashMap<String, Option<String>>
PdfPageAnnotation::as_form_field(&self) -> Option<&PdfFormField<'_>>; as_form_field_mut(&mut self)
PdfFormField::field_type(&self) -> PdfFormFieldType {Unknown, PushButton, Checkbox, RadioButton, ComboBox, ListBox, Text, Signature}
PdfFormFieldCommon::name(&self) -> Option<String>; is_read_only(); appearance_stream(); appearance_mode_value(PdfAppearanceMode)
PdfFormTextField::value(&self) -> Option<String>; set_value(&mut self, &str) -> Result<(), PdfiumError>   // writes /V + /M only
PdfFormCheckboxField::is_checked() -> Result<bool>; set_checked(&mut self, bool)   // writes /AS + /V only
PdfFormRadioButtonField::is_checked(); set_checked(&mut self); group_value(); index_in_group()
PdfFormComboBoxField / PdfFormListBoxField::options().iter() -> PdfFormFieldOption { label(), is_set(), index() }; value()
```

Raw form fill (what actually makes a value visible), all verified:

```rust
FPDFDOC_InitFormFillEnvironment(doc, *mut FPDF_FORMFILLINFO /*version 2, all callbacks None*/) -> FPDF_FORMHANDLE
FORM_OnAfterLoadPage(page, form); FPDF_SetFormFieldHighlightAlpha(form, 0)
FORM_OnLButtonDown(form, page, 0, x: f64, y: f64) -> FPDF_BOOL; FORM_OnLButtonUp(...)     // page space, y up
FORM_OnChar(form, page, ch as c_int, 0); FORM_ForceToKillFocus(form)  -> /V set AND /AP regenerated
FPDFAnnot_GetFormFieldValue(form, annot, buf: *mut FPDF_WCHAR, len) -> c_ulong (bytes, UTF-16LE)
FPDFAnnot_IsChecked(form, annot); FPDFAnnot_GetFormFieldType(form, annot) -> c_int
FPDF_FFLDraw(form, bitmap, page, 0, 0, w, h, 0, FPDF_ANNOT); FORM_OnBeforeClosePage; FPDFDOC_ExitFormFillEnvironment
```

### 1.8 Raw annotation FFI used (all present in `PdfiumLibraryBindings`)

```
FPDFPage_CreateAnnot(page, subtype) -> FPDF_ANNOTATION (NULL if unsupported)      FPDFPage_GetAnnot / GetAnnotCount / RemoveAnnot / CloseAnnot
FPDFAnnot_IsSupportedSubtype(subtype) -> FPDF_BOOL          FPDFAnnot_IsObjectSupportedSubtype(subtype)   (Ink, Stamp)
FPDFAnnot_SetRect(annot, *const FS_RECTF) / GetRect          FPDFAnnot_SetColor(annot, 0=/C | 1=/IC, R,G,B,A) / GetColor  (A -> /CA)
FPDFAnnot_SetBorder(annot, h_radius, v_radius, width) / GetBorder            (/BS /W — used by AP generation)
FPDFAnnot_AddInkStroke(annot, *const FS_POINTF, count) -> stroke index; FPDFAnnot_RemoveInkList; GetInkListCount; GetInkListPath(annot, i, buf, len)
FPDFAnnot_GetLine(annot, *mut FS_POINTF, *mut FS_POINTF); FPDFAnnot_GetVertices(annot, buf, len)
FPDFAnnot_GetNumberValue(annot, "CA", *mut f32)  (no SetNumberValue exists — opacity only via SetColor alpha)
FPDFAnnot_SetStringValue_str(annot, key, &str) / GetStringValue(annot, key, buf, len) / HasKey / GetValueType
FPDFAnnot_GetLinkedAnnot(annot, "Popup" | "Parent" | "IRT") -> FPDF_ANNOTATION (read only; nothing can create links)
FPDFAnnot_SetAP_str(annot, mode 0, &str)  (NULL/empty removes the AP; stream gets BBox=/Rect, no /Resources)   FPDFAnnot_GetAP(annot, mode, buf, len)
FPDFAnnot_AppendObject / UpdateObject / RemoveObject; FPDFAnnot_SetFlags / GetFlags (Print = 4)
FPDFPage_Flatten(page, 0 = FLAT_NORMALDISPLAY | 1 = FLAT_PRINT) -> 0 fail / 1 success / 2 nothing to do  (then FPDFPage_GenerateContent + reload page)
```

---

## 2. Support matrix (what the spike measured on tracemonkey.pdf p.0, 2× render, pixel-probed)

Legend: HL = high-level pdfium-render API, RAW = needs `PdfiumLibraryBindings`, ✔ verified, ✘ not possible, ~ partial.

| Type | Create | Edit rect | Edit colour/opacity | Rendered by PDFium | Persisted after save + reopen | Notes |
|---|---|---|---|---|---|---|
| Highlight (explicit quads, 2 quads) | ✔ HL | ✔ (rect + quads; generated APs follow QuadPoints) | ~ only before AP exists; else RAW SetAP(NULL) first | ✔ (multiply blend, /CA honoured) | ✔ | quads must be TL,TR,BL,BR |
| Underline / StrikeOut / Squiggly | ✔ HL | ✔ | ~ same | ✔ | ✔ | 1 pt line from /C |
| Ink (path objects) | ✔ HL | ~ (objects fixed once appended) | object colours | ✔ | ✔ | no /InkList (Acrobat still shows AP) |
| Ink (/InkList) | ✔ RAW | ✔ | ✔ before AP | ✔ | ✔ | width via SetBorder; 2 strokes verified |
| Square | ✔ HL | ✔ (AP re-mapped via Rect/BBox matrix) | ~ | ✔ | ✔ | border width RAW SetBorder |
| Circle | ✔ RAW only | ✔ | ~ | ✔ | ✔ | /CA 0.5 + interior colour verified |
| Line | ✘ (CreateAnnot NULL) | — | — | ✔ existing files (AP) | — | read via GetLine |
| Polygon / Polyline / Caret / Redact | ✘ | — | — | — | — | |
| FreeText (with /DA) | ✔ RAW for DA | ✔ | ✔ (/C = border, DA colour = text) | ✔ (AP generated: border + Helv text) | ✔ (132-byte AP on disk) | HL cannot set /DA |
| FreeText (no /DA, HL) | ✔ HL | ✔ | — | ~ unreliable, empty after save | ✘ no AP written | do not ship |
| FreeText (explicit SetAP) | ✔ RAW | ✔ | — | ✔ | ✔ | no /Resources → other viewers may lack the font |
| Stamp + text object (+ bg rect) | ✔ HL | ✔ | via object colours | ✔ | ✔ | portable "free text" substitute |
| Stamp + image | ✔ HL | ✔ | — | ✔ | ✔ | 64×64 RGBA → 40 pt |
| Link (URI + quads) | ✔ HL | ✔ | n/a | (invisible by design) | ✔ URI read back | /Dest go-to: ✘ |
| Text (sticky note) | ✔ HL | ✔ | ✔ | ✔ note icon AP | ✔ | PDFium icon only |
| Popup | ✔ HL | ✔ | — | ✘ (unlinked popups are skipped) | dict persisted | linking impossible |
| FileAttachment | RAW create returns handle | — | — | ✘ | dict only | pdfium-render: Unsupported |
| Widget (form field) | existing only | — | — | ✔ via FFLDraw | ✔ | see §3.4 |

---

## 3. Task items in detail

### 3.1 Enumerate existing annotations, delete, save

* `annotation-highlight.pdf`: `#0 Highlight bounds=[l=70.9 b=736.2 r=193.9 t=745.6] contents="Highlight content" author="Tim van der Meij" modified="D:20151230152248+01'00'" created=… name=<uuid> print=true objects=1 quads=(70.9,745.6)(193.9,745.6)(70.9,736.2)(193.9,736.2)` — note the quad order (TL,TR,BL,BR). Raw: `CA=1.0 border=(0,0,1) popupLink=true AP_len=117 Subj="Highlight"`. `#1 Popup … parentLink=true, AP none`.
* `annotation-line.pdf`: `#0 Line supported=false` (pdfium-render `Unsupported` variant) but bounds/contents/author/dates readable; raw `FPDFAnnot_GetLine = ((75.2,729.9),(245.7,688.3))`, `AP_len=70` → renders.
* `annotation-freetext.pdf`: two FreeText, `DA=" /Helvetica 9 Tf"`, `Subj="Typewriter"`, AP present.
* `160F-2019.pdf`: 76 Widget annotations on 1 page; `objects()` returns their AP objects (3–5 each).
* High-level colour getters return `n/a` (Err) for markup annotations whose colour lives only in the AP — that is the
  `FPDFAnnot_GetColor` false path; the fallback pdfium-render takes is the UB path of §5.1 but happens to return false here.
* Delete: `annotations_mut().get(0)` → `delete_annotation(a)` → 2→1 in memory → 1 after save+reopen. ✔

### 3.2 Create + render + persist (tracemonkey.pdf p.0)

13 annotations created in 0.7 ms (Manual strategy). Saved twice: before any render (`spike_annot_norender.pdf`) and after
one `render_with_config` (`spike_annot.pdf`). Raw `FPDFAnnot_GetAP` on the files:

| | norender file | rendered file |
|---|---|---|
| Highlight/Underline/StrikeOut/Squiggly/Square/Text | **no /AP** | /AP present (164/58/54/64/67/214 bytes) + `/PDFIUM_HasGeneratedAP true` |
| Ink (objects) / Stamp | /AP present (AppendObject creates it) | same |
| FreeText (no DA) / Link / Popup | no /AP | no /AP |

Both files render identically in PDFium (it regenerates missing APs on load), but only the rendered file carries APs for
Acrobat/Preview/pdf.js. `FPDF_SaveAsCopy` after a render also converts the annotation dicts to indirect objects; the
norender file keeps them as direct objects inside `/Annots`.

Pixel results (both files): highlight quad0 1513 px changed / quad1 330, underline 188, strikeout 148, squiggly 282,
ink 4907, square 4929, stamp-text 27600, stamp-image 5808, sticky 1327, popup 0, freetext(no DA) 0 in the rendered file
(but a solid blue box with black text in the same session on the norender file — an in-memory-only AP; unreliable).

Raw section: `FPDFAnnot_IsSupportedSubtype` = Text, Link, FreeText, Square, Circle, Highlight, Underline, Squiggly,
StrikeOut, Stamp, Ink, Popup, FileAttachment. Circle (CA 0.5 read back as 0.50196, border 3) 2501 px, InkList 10191 px,
FreeText-with-DA 3647 px changed, generated AP text:
`/GS gs q 0 0 1 rg 60 700 230 30 re 61 701 228 28 re f* Q /Tx BMC q BT 0 0 1 rg 61 710.8 Td /Helv 12 Tf (raw FreeText via DA) Tj ET Q EMC`.
Sticky-note AP is a 20×20 "note" glyph (`… m … l … B*`).

### 3.3 Modify + flatten

* Square moved 565→480 / resized: `set_bounds` OK, 13289 fill px at the new rect, 0 at the old. Existing AP is re-mapped
  through the form matrix (`/Rect` vs `/BBox`), so a move/resize needs no regeneration.
* Highlight quads shifted −200 pt with `set_attachment_point_at_index` + `set_bounds`: visible at the new place
  (2150 px), nothing at the old — PDFium draws pdfium-generated markup APs at the QuadPoints bounds
  (`CPDF_Annot::RectForDrawing` when `/PDFIUM_HasGeneratedAP` is set). For APs written by other producers this does not
  hold; the safe recipe is the raw one below.
* Colour change on an annotation with AP: raw `FPDFAnnot_SetColor` → **false**; `FPDFAnnot_SetAP(annot, 0, NULL)` →
  true; `SetColor` → true; render → AP regenerated (13289 magenta px); persisted after save+reopen. Same for the
  highlight after the quad move (2150 px at the new quad).
* `set_fill_color` / `set_stroke_color` on an annotation with AP through pdfium-render: **child process exit
  `unix_wait_status(11)` (SIGSEGV)**, see §5.1.
* Flatten: `PdfPage::flatten()` in 0.8 ms; annots 12→0, page objects 151→152 (one form XObject), every visible annotation
  still visible, text extraction still 5112 chars. With the Print flag cleared on all annotations: annots 12→0, objects
  151→151, **everything gone**. Raw `FPDFPage_Flatten(page, FLAT_NORMALDISPLAY)` on that same file: rc=1, all annotations
  flattened and visible (`spike_annot_flattened_normaldisplay.pdf`). Flatten must be followed by `FPDF_ClosePage` +
  `FPDF_LoadPage` before rendering (pdfium-render does this internally; do it yourself in raw code).

### 3.4 AcroForm (160F-2019.pdf, A4-ish, 76 widgets)

* `document.form()` → `Some`, `form_type() = Acrobat`. Field counts: Text 64, PushButton 10, RadioButton 2 — the fixture
  has **no checkbox** fields (the task description was wrong); the spike toggles a radio instead. Names like `A.NOM`,
  `A.PRENOM`, `F.1`, `B.Reset`; values `None`; `field_values()` gives 67 entries. Enumeration 4 ms.
* High-level `set_value("SeePDF 스파이크 12345")`: Ok, `value()` reads back, persisted after save (`/V` on the widget),
  but the render shows **0 changed pixels** in the field, both in-session and after reopen (empty comb cells in
  `spike_annot_crop_form_highlevel_reopened_text.png`). No AP regeneration, and 160F has no `NeedAppearances`, so
  PDFium (and any viewer that trusts `/AP`) shows an empty field. Radio `set_checked()` → `is_checked() = true`, but the
  radio was already on, `group_value()` reports "Off" (export value bookkeeping is unreliable).
* Raw form fill: `FORM_OnLButtonDown` returned 0 for this top-edge field but `FORM_OnLButtonUp` 1 and typing worked:
  `FPDFAnnot_GetFormFieldValue = "Raw FORM 입력"`, `/AP` 896 bytes regenerated by the form layer; `FPDF_FFLDraw` render
  196 px changed (`spike_annot_crop_form_raw_text.png` shows "R a w F O R M 입 력" in the comb field); saved (381 KB);
  reopened: value persisted, 231 px changed with FFLDraw and 1094 px without form data (AP drawn directly). Korean
  glyphs render (PDFium substitutes a system font into the AP).
* Limitations: radio/checkbox toggling through FORM_* is a click at the widget centre (state read back with
  `FPDFAnnot_IsChecked`); combo/list selection via `FORM_SetIndexSelected(form, page, index, selected)`; signature fields
  are read-only (`as_signature_field`); XFA forms are not enabled in this pdfium build (`form_type()` would say
  `XfaForeground/XfaFull`, fields unreachable). Push buttons (`B.Reset`, `link`) carry JS/URI actions that need the
  FFI callbacks (`FFI_DoURIAction`, `m_pJsPlatform`) — all `None` in pdfium-render; ignore.

### 3.5 Redaction

Approach A on tracemonkey p.0, target word "Gal" (`[l=158.9 b=643.7 r=176.2 t=653.4]` from `text().search`):
2 text objects overlap → removed (`"Andreas Gal"` and the `"∗"` superscript), black rect added, `regenerate_content()`;
12.9 ms. Text extraction: 1 → 0 occurrences in memory and after save+reopen; 5095 → 5083 chars; 612 black px; objects
151 → 150. `spike_annot_crop_redacted.png` shows the black box. Collateral removal is the whole text object, i.e. a
whole word run/line — PDFium cannot split a text object. For true word-level redaction re-create the surviving
characters as new text objects (font via `text_object.font()`, size via `unscaled_font_size()`, position via
`get_translation()` / char `origin()`), then remove the original. Images intersecting the rect must be handled too
(remove, or `set_image` on a pixel-blanked copy). Approach B: `FPDFPage_CreateAnnot(page, FPDF_ANNOT_REDACT)` → NULL.

---

## 4. Benchmarks (M-series Mac, debug build, PDFium release dylib)

| What | Result |
|---|---|
| Enumerate annotations (1-page fixtures, 2–76 annots) | 1.6–5 ms (annotation-freetext.pdf 28 ms — first font load) |
| Render tracemonkey p.0 at 2× (1224×1584, annotations + form data) | 5.5–6.1 ms |
| Create 20 square annotations, `AutomaticOnEveryChange` vs `Manual` | 260 µs vs 207 µs (20 × `FPDFPage_GenerateContent` on a 151-object page is cheap here, but it rewrites the content stream every call) |
| Create 13 mixed annotations incl. image stamp (Manual) | 0.7–1.2 ms |
| `save_to_file` (1 MB doc) | 3–6 ms |
| `PdfPage::flatten()` (12 annotations) | 0.5–0.8 ms |
| Redaction: remove 2 objects + rect + `regenerate_content` | 12.6–13 ms |
| Whole spike (5 sections, ~25 renders, 15 saves) | ≈ 1.2 s |

---

## 5. Gotchas (all reproduced by the spike)

1. **Changing colour/opacity of an annotation that already has an `/AP` crashes the process.** `FPDFAnnot_SetColor`
   returns false whenever `/AP` exists ("path stream colours take priority"); pdfium-render's `set_fill_color_impl`/
   `set_stroke_color_impl` then call `FPDFPageObj_SetFillColor(self.handle() as FPDF_PAGEOBJECT, …)`, i.e. they treat a
   `CPDF_AnnotContext*` as a `CPDF_PageObject*`. Result: SIGSEGV (child process exit status 11). Every annotation you
   re-open from disk (or rendered once) has an AP. **Never call `set_fill_color`/`set_stroke_color` on such an
   annotation.** Recipe: raw `FPDFAnnot_SetAP(annot, FPDF_ANNOT_APPEARANCEMODE_NORMAL, NULL)` → `FPDFAnnot_SetColor`
   → render (or let the next render regenerate). Note the same `_impl` fallback exists for the getters (`fill_color()`),
   which returned Err harmlessly in the spike but is equally undefined.
2. **QuadPoints order.** PDFium reads a quad as (x1,y1)=TL,(x2,y2)=TR,(x3,y3)=BL,(x4,y4)=BR and takes
   left=x3, bottom=y3, right=x2, top=y2. `PdfQuadPoints::from_rect()` produces BL,BR,TR,TL → PDFium normalises to a
   zero-width rect and the AP is a 1-px sliver. Build quads with `new_from_values(l,t, r,t, l,b, r,b)`.
3. **Appearance streams are generated lazily and only when the page is rendered with annotations** (or flattened).
   Save without rendering → no `/AP` for Highlight/Underline/StrikeOut/Squiggly/Square/Circle/Text/FreeText → those
   annotations are invisible in viewers that do not synthesise appearances. Render once (any size, e.g. 10 px wide)
   before `save_to_file`. Ink/Stamp with objects get an AP at `FPDFAnnot_AppendObject` time.
4. **`/PDFIUM_HasGeneratedAP true` leaks into saved files** (6 occurrences in `spike_annot.pdf`). Harmless, but it is
   what makes PDFium re-map markup APs onto QuadPoints; other producers' APs will not move when you change quads —
   remove the AP (gotcha 1) after editing quads.
5. **`/Rect` must be set before appending objects to Ink/Stamp annotations** (AP BBox = Rect at creation); objects
   outside are clipped. pdfium-render's `create_*_annotation` leaves Rect at 0,0,0,0.
6. **Transform objects before `add_*_object`.** pdfium-render never calls `FPDFAnnot_UpdateObject`, so later
   `translate()`/`set_matrix()` on an annotation object are not written back to the AP.
7. **Opacity = alpha of the last `FPDFAnnot_SetColor` call.** `set_stroke_color(PdfColor::new(255,255,0,153))` writes
   `/CA 0.6`; a following `set_fill_color(PdfColor::new(…,255))` resets `/CA` to 1. There is no `FPDFAnnot_SetNumberValue`;
   `FPDFAnnot_SetStringValue("CA", …)` would write a string, which PDFium ignores.
8. **Flatten deletes non-printable annotations.** `PdfPage::flatten()` uses `FLAT_PRINT`; pdfium-render never sets
   `/F 4`. Call `set_is_printed(true)` on every annotation you create (Acrobat does), or flatten raw with
   `FLAT_NORMALDISPLAY`. Flatten also skips Popups and hidden annotations, and requires a page reload afterwards.
9. **High-level form setters write `/V` only.** No AP regeneration → PDFium renders the old (empty) appearance and so
   does every viewer that honours `/AP` (macOS Preview does). Use the FORM_* event path (raw) or a NeedAppearances
   strategy (no public API to set the AcroForm dict → would need lopdf post-processing).
10. **Default `PdfPageContentRegenerationStrategy::AutomaticOnEveryChange` rewrites the page content stream on every
    annotation create/delete/object append** (`FPDFPage_GenerateContent`). Annotation edits do not need it; set
    `Manual` for annotation work and call `regenerate_content()` only after page-object edits (redaction). Regeneration
    changed the extracted text length of tracemonkey p.0 (5087 → 5112 chars after flatten + regenerate).
11. **Popups cannot be linked**, standalone Popup annots are never drawn by PDFium, and `GetLinkedAnnot` is read-only.
    Draw note popups in the React layer from `contents()`/`creator()`/`modification_date()`.
12. **Go-to-page links cannot be created** (`FPDFAnnot_SetURI` only). Same for named destinations.
13. **Coordinates:** everything (Rect, QuadPoints, ink points, FORM_* mouse events, text search bounds) is PDF user
    space, origin bottom-left, y up, in points. The 2× bitmap pixel of a point is `(x*2, (page_h - y)*2)`. FS_RECTF is
    `{left, top, right, bottom}` while `PdfRect::new_from_values` is `(bottom, left, top, right)`.
14. **Borrowing:** `annotations().get(i)` borrows the page immutably for the annotation's lifetime; to delete use
    `annotations_mut().get(i)`. `document.fonts_mut()` conflicts with a live `PdfPage`; take `PdfFontToken`s first.
    `PdfPages::get` takes `PdfPageIndex = c_int`, annotation/object indices are `usize`.
15. **Line annotations from other producers** come back as `PdfPageAnnotation::Unsupported` — common props still work.
16. `FreeText` created by pdfium-render (no `/DA`) may show a solid blue box with black text in the same session on a
    freshly loaded file but nothing after save — treat as unsupported; always set `/DA` (raw) or use Stamp+text.
17. `FPDFBitmap_FillRect`/`FPDFPage_InsertObject` return `FPDF_BOOL` in the 7881 bindgen (`let _ =`).

---

## 6. Recommendations for the implementation

1. **Build one thin raw annotation module** (`engine/annot_ffi.rs`) around `&'static dyn PdfiumLibraryBindings` and keep
   the *document* raw too for everything in the annotation/form tool (`FPDF_LoadDocument`, `FPDF_LoadPage`, save via
   `FPDF_FILEWRITE`). pdfium-render's high-level objects cannot share a document with raw calls, and the
   annotation-related gaps (circle, ink list, border, opacity-safe recolour, AP removal, DA, FORM_* fill, flatten
   mode, popup/parent reads) are all core features. Keep pdfium-render for rendering/text/page ops where it is safe.
   If the team prefers staying high-level, a fork of pdfium-render adding `pub fn handle()` accessors is a 5-line patch.
2. Annotation create pipeline: `CreateAnnot` → `SetRect` → `SetColor(C, rgba)` / `SetColor(IC, rgba)` (same alpha) →
   `SetBorder(0,0,width)` → quads (TL,TR,BL,BR) / `AddInkStroke` / `AppendObject` → `SetFlags(PRINT)` → `SetStringValue`
   for `T`, `Contents`, `NM` (uuid), `Subj`, `CreationDate` → **render page once (thumbnail size) before save**.
3. Edit pipeline for existing annotations: `SetAP(NULL)` first, then set properties, then re-render. For moves of
   pdfium-generated markup this is optional, for third-party APs mandatory.
4. FreeText: set `/DA "<r> <g> <b> rg /Helv <size> Tf"` and `/Contents`; PDFium generates and persists a
   border+text AP. For CJK text embed a font and use the Stamp+text-object route, or write the AP yourself with
   `FPDFAnnot_SetAP` (accept that it has no `/Resources`).
5. Forms: implement fill through FORM_* events (focus by `FORM_SetFocusedAnnot(form, annot)` or click, `FORM_OnChar`,
   `FORM_ForceToKillFocus`), render with `FPDF_FFLDraw`, never through `PdfFormTextField::set_value`.
6. Flatten: raw `FPDFPage_Flatten(page, FLAT_NORMALDISPLAY)` + `FPDFPage_GenerateContent` + reload; or ensure Print flags.
7. Redaction: keep approach A; add text-object splitting for word-level precision; also scan image objects; always
   `regenerate_content()` and re-extract text to verify; optionally strip `/PDFIUM_HasGeneratedAP` and metadata.
8. Popups, note icons, link go-to destinations: model in the UI/JSON layer; if go-to links or popup linking are a
   must-have, post-process the saved file with `lopdf` (not installed; only needed for those structural edits).
9. Set `PdfPageContentRegenerationStrategy::Manual` on pages used for annotation work.

---

## 7. Files written by the spike (`fixtures/out/`)

`spike_annot_highlight_deleted.pdf`, `spike_annot_norender.pdf`, `spike_annot.pdf` (13 annotations, APs),
`spike_annot_raw.pdf` (circle, ink-list, freetext+AP, sticky, square), `spike_annot_freetext_persist{,2}.pdf`,
`spike_annot_modified.pdf`, `spike_annot_recolored.pdf`, `spike_annot_noprint.pdf`,
`spike_annot_flattened_{print,noprint,normaldisplay}.pdf`, `spike_annot_form_highlevel.pdf`, `spike_annot_form_raw.pdf`,
`spike_annot_redacted.pdf`; PNGs `spike_annot_{baseline,before_save,reopened_norender,reopened_rendered,raw_before_save,
raw_reopened,modified,recolored,flattened_*,form_*,redacted}.png` and ~55 `spike_annot_crop_*.png` close-ups (one per probe).
