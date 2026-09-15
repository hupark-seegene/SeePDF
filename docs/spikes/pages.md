# Spike: page operations, images, metadata and export (pdfium-render 0.9.4)

**Status:** compiles and runs clean, all 8 sections pass.
**Spike source:** `/Users/veri/Dev/SeePDF/src-tauri/examples/spike_pages.rs`
**Run:** `cd /Users/veri/Dev/SeePDF/src-tauri && cargo run --release --example spike_pages`
**Outputs:** `/Users/veri/Dev/SeePDF/fixtures/out/` and `/Users/veri/Dev/SeePDF/fixtures/out/export/`

| | |
|---|---|
| crate | `pdfium-render 0.9.4`, features `thread_safe` + defaults (`pdfium_7881` bindgen, `image_025`) |
| native lib | `src-tauri/resources/pdfium/libpdfium-aarch64-apple-darwin.dylib`, PDFium chromium **8057** |
| host | macOS 25.6 (Darwin), Apple Silicon |
| timings | release profile (`lto = true`, `opt-level = 3`) unless stated |

---

## 0. Two things that shape every design decision

### 0.1 `PdfDocument::handle()` and `bindings()` are `pub(crate)`

There is **no public way to get the `FPDF_DOCUMENT` out of a `PdfDocument`**, and
`PdfiumLibraryBindingsAccessor` is `pub(crate)` too. Several things we need
(`FPDF_MovePages`, save flags) have no safe wrapper, so the spike keeps a *second*
independent bindings handle:

```rust
// Both binds MUST happen before Pdfium::new(): bind_to_library() returns
// Err(PdfiumLibraryBindingsAlreadyInitialized) once the global OnceCell is populated.
let owned = Pdfium::bind_to_library(&lib)?;
let raw   = Pdfium::bind_to_library(&lib)?;
let pdfium = Pdfium::new(owned);            // calls FPDF_InitLibrary()
let raw: &dyn PdfiumLibraryBindings = raw.as_ref();
```

`raw` can then do `FPDF_LoadDocument` / `FPDF_MovePages` / `FPDF_SaveAsCopy` on its
own raw handles. **Caveat:** with the `thread_safe` feature each bindings instance owns
its *own* mutex, so the two handles do not mutually exclude. Either keep all raw work
on one thread, or (better, for the app) funnel every pdfium call through one actor
thread — see recommendations.

### 0.2 Mutating a page needs only `&PdfDocument`

`PdfPage` owns its own `FPDF_PAGE` handle and is returned by value from
`PdfPages::get(&self, …)`. So this compiles with a **non-mut** document:

```rust
let doc = pdfium.load_pdf_from_file(path, None)?;   // not `mut`
let mut page = doc.pages().get(0)?;
page.set_rotation(PdfPageRenderRotation::Degrees90);
page.objects_mut().create_image_object(...)?;
doc.save_to_file(out)?;                              // changes are there
```

`&mut PdfDocument` is only needed for `pages_mut()` (create/copy pages) and
`attachments_mut()`. This means Rust's borrow checker gives you **no protection**
against concurrent edits — enforce single-writer at the app layer.

---

## 1. Page operations

### Verified signatures

```rust
// pdf/document/pages.rs
pub fn len(&self) -> PdfPageIndex;                       // PdfPageIndex = c_int (i32)
pub fn is_empty(&self) -> bool;
pub fn as_range(&self) -> Range<PdfPageIndex>;
pub fn as_range_inclusive(&self) -> RangeInclusive<PdfPageIndex>;
pub fn get(&self, index: PdfPageIndex) -> Result<PdfPage<'a>, PdfiumError>;
pub fn page_size(&self, index: PdfPageIndex) -> Result<PdfRect, PdfiumError>;
pub fn page_sizes(&self) -> Result<Vec<PdfRect>, PdfiumError>;
pub fn first(&self) -> Result<PdfPage<'a>, PdfiumError>;
pub fn last(&self) -> Result<PdfPage<'a>, PdfiumError>;
pub fn create_page_at_start(&mut self, size: PdfPagePaperSize) -> Result<PdfPage<'a>, PdfiumError>;
pub fn create_page_at_end(&mut self, size: PdfPagePaperSize)   -> Result<PdfPage<'a>, PdfiumError>;
pub fn create_page_at_index(&mut self, size: PdfPagePaperSize, index: PdfPageIndex)
    -> Result<PdfPage<'a>, PdfiumError>;
pub fn iter(&self) -> PdfPagesIterator<'_>;              // Item = PdfPage<'a>
pub fn page_mode(&self) -> PdfPageMode;
pub fn watermark<F>(&self, watermarker: F) -> Result<(), PdfiumError>;

// pdf/document/page.rs
pub fn width(&self) -> PdfPoints;                        // PdfPoints { pub value: f32 }
pub fn height(&self) -> PdfPoints;
pub fn page_size(&self) -> PdfRect;
pub fn orientation(&self) -> PdfPageOrientation;
pub fn rotation(&self) -> Result<PdfPageRenderRotation, PdfiumError>;
pub fn set_rotation(&mut self, rotation: PdfPageRenderRotation);   // note: NOT a Result
pub fn label(&self) -> Option<&str>;
pub fn delete(self) -> Result<(), PdfiumError>;          // consumes the page
pub fn regenerate_content(&mut self) -> Result<(), PdfiumError>;
pub fn content_regeneration_strategy(&self) -> PdfPageContentRegenerationStrategy;
pub fn set_content_regeneration_strategy(&mut self, s: PdfPageContentRegenerationStrategy);
pub fn boundaries(&self) -> &PdfPageBoundaries<'_>;      // .media()/.crop()/.trim()/.art()/.bleed()/.bounding()
pub fn boundaries_mut(&mut self) -> &mut PdfPageBoundaries<'a>;   // .set_media(PdfRect) etc.
pub fn has_transparency(&self) -> bool;
pub fn fonts(&self) -> Vec<PdfFont<'_>>;
pub fn flatten(&mut self) -> Result<(), PdfiumError>;    // needs crate feature "flatten" (NOT enabled)

// enums
pub enum PdfPageRenderRotation { None, Degrees90, Degrees180, Degrees270 }  // .as_degrees() / .as_radians()
pub enum PdfPageContentRegenerationStrategy { AutomaticOnEveryChange, AutomaticOnDrop, Manual }

// pdf/document/page/size.rs
PdfPagePaperSize::a4() / a4r() / a3() / new_portrait(std) / new_landscape(std)
PdfPagePaperSize::from_points(w: PdfPoints, h: PdfPoints) / from_inches(f32,f32) / from_mm / from_cm
    .width() -> PdfPoints, .height() -> PdfPoints, .as_rect() -> PdfRect, .landscape(), .portrait()
```

### Results

```
tracemonkey.pdf: 14 pages, page 0 = 612.0 x 792.0 pt, rotation=None, label=None
page_sizes() for 14 pages                  0.23 ms   <- use this for a fast organiser layout pass
rotation.pdf page 1: rotation=Degrees90, reported size 792 x 612 pt (already swapped)

set_rotation(page 0, 90)                   2.76 ms
delete(page 1)                             2.16 ms   14 -> 13 pages
create_page_at_index(A4, 2)                0.00 ms   -> 595.3 x 841.9 pt
create_page_at_end(200x400 pt)                       -> 200.0 x 400.0 pt
save_to_file (947 KB)                      5.20 ms
```

Reopened `fixtures/out/pages_ops.pdf`: 15 pages, page 0 rotation **persisted** as
`Degrees90` and now reports `792 x 612 pt`; inserted page 2 is a genuine empty A4
(0 page objects); the old page 2 has moved to index 1, so the delete stuck.

`set_rotation()` is not a `Result` and takes ~2.8 ms because dropping the `PdfPage`
triggers content regeneration under the default
`PdfPageContentRegenerationStrategy::AutomaticOnEveryChange`.

### Move / reorder — no safe API, use `FPDF_MovePages`

`PdfPages` has **no move/reorder function**. PDFium build 8057 does have
`FPDF_MovePages` (available in the bindings from `pdfium_6043` upward):

```rust
unsafe fn FPDF_MovePages(
    &self,
    document: FPDF_DOCUMENT,
    page_indices: *const c_int,
    page_indices_len: c_ulong,
    dest_page_index: c_int,
) -> FPDF_BOOL;
```

Measured on tracemonkey.pdf (14 pages):

```
FPDF_MovePages([3,2] -> 1)                 true    0.02 ms   order becomes [0,3,2,1,4,5,...]
FPDF_MovePages([2,2] -> 0)  duplicates     false
FPDF_MovePages([0,999,3] -> 1) out of range false
FPDF_MovePages([0,3,1] -> 13) dest too high false   (dest + count must fit)
```

It is effectively free (0.02 ms), the ordered list semantics match the header docs
(`[3,2] -> 1` gives `A D C B`), and the failure cases return `false` cleanly without
corrupting the document. This is **the** primitive for drag-reorder.

**Gotcha:** calling it raw bypasses `PdfPageIndexCache`, pdfium-render's internal
map from `FPDF_PAGE` handles to indices. Do the move on a document where you hold
*no* open `PdfPage`s from the safe layer, or save + reopen afterwards.

### Duplicate a page

There is no same-document copy in the safe layer, because
`copy_page_from_document(&mut self, source: &PdfDocument, …)` cannot take `&doc` and
`&mut doc` simultaneously. Two options, both verified:

1. **Second handle to the same bytes** (safe): open the file twice, copy from B into A.
   `copy_page_from_document(&src, 0, 1)` = **0.37 ms**, text of page 0 == page 1.
   For a document with *unsaved* edits, `save_to_bytes()` first then
   `load_pdf_from_byte_vec()`.
2. **`FPDF_ImportPagesByIndex` with `src == dest`** (raw): **works** — page count went
   14 -> 15. Undocumented but valid in build 8057.

---

## 2. Merge and split

### Verified signatures

```rust
pub fn copy_page_from_document(&mut self, source: &PdfDocument,
    source_page_index: PdfPageIndex, destination_page_index: PdfPageIndex) -> Result<(), PdfiumError>;

/// `pages` is a 1-BASED comma/range string, e.g. "1,3,5-7".
pub fn copy_pages_from_document(&mut self, source: &PdfDocument,
    pages: &str, destination_page_index: PdfPageIndex) -> Result<(), PdfiumError>;

/// `source_page_range` is 0-BASED and inclusive.
pub fn copy_page_range_from_document(&mut self, source: &PdfDocument,
    source_page_range: RangeInclusive<PdfPageIndex>, destination_page_index: PdfPageIndex)
    -> Result<(), PdfiumError>;

pub fn append(&mut self, document: &PdfDocument) -> Result<(), PdfiumError>;
pub fn tile_into_new_document(&self, rows: u8, cols: u8, size: PdfPagePaperSize)
    -> Result<PdfDocument<'a>, PdfiumError>;
```

**There is no `import_pages_from_document`.** The range-string call is
`copy_pages_from_document`; the index-range call is `copy_page_range_from_document`.

> **Indexing trap:** `copy_pages_from_document(src, "1-3,5", 0)` copies source pages
> **1,2,3,5** (one-based), while `copy_page_range_from_document(src, 2..=5, 0)` copies
> source pages **3,4,5,6** (zero-based). Two neighbouring APIs, two conventions.

### Results

```
copy_pages_from_document(tracemonkey, "1-3,5", 0)     0.65 ms  -> 4 pages
copy_pages_from_document(highlight,   "1",     4)              -> 5 pages
copy_page_from_document (highlight, 0, index 2)                -> 6 pages   (mid-document insert works)
merged.pdf                                                     357 713 bytes
copy_page_range_from_document(2..=5)                  0.46 ms  -> 4 pages, 144 118 bytes
burst 3 single-page documents (new doc + copy + save) 2.01 ms total
tile_into_new_document(2, 2, A4)                      3.5  ms  -> 4 pages (14-up-to-4, N-up printing)
PdfPages::append(14 pages into a 2-page doc)          0.84 ms  -> 16 pages
```

### Annotations survive; the AcroForm does **not**

```
annotation-highlight.pdf    1 page, 2 annotations (Highlight + Popup)
merged.pdf reopened         6 pages, 4 annotations; page 2 ["Highlight","Popup"], page 5 ["Highlight","Popup"]
```
Annotations are copied faithfully, including the linked Popup.

```
160F-2019.pdf                                  form_type=Some(Acrobat), 76 widgets, 67 named fields
load -> save_to_file -> reload (same document)  form_type=Some(Acrobat), 76 widgets, 67 named fields
copy all pages into a NEW document              form_type=None,          76 widgets
same, after save + reopen                       form_type=None,          76 widgets
```

**Finding:** `FPDF_ImportPages*` copies the widget *annotations* but not the document
catalog's `/AcroForm` dictionary, so the imported pages render but the form is dead —
`PdfDocument::form()` is `None` and `field_values()` is unavailable. A plain
load/save round trip of the original keeps everything.

Consequences for SeePDF:
* Split/extract of a form document **must** preserve `/AcroForm` (and `/DR`, `/NeedAppearances`)
  — pdfium cannot do it; this needs `lopdf` post-processing, or split by
  *deleting the other pages from a copy of the original document* instead of importing
  into a new one. **Deleting is the safer split strategy** and is what we should ship.
* Merging two form documents will need field-name de-duplication anyway; treat it as a
  separate, later feature and warn the user (Korean UI: 양식 필드가 유지되지 않을 수 있습니다).

---

## 3. Images

### Verified signatures

```rust
// listing
page.objects() -> &PdfPageObjects<'_>;   page.objects_mut() -> &mut PdfPageObjects<'a>
PdfPageObjectsCommon::{len, get(index: usize), iter, first, last, bounds}
PdfPageObject::object_type(&self) -> PdfPageObjectType    // Text|Path|Image|Shading|XObjectForm|Unsupported
PdfPageObject::as_image_object(&self) -> Option<&PdfPageImageObject<'_>>
PdfPageObject::as_image_object_mut(&mut self) -> Option<&mut PdfPageImageObject<'a>>
PdfPageObjectCommon::bounds(&self) -> Result<PdfQuadPoints, PdfiumError>   // .left()/.right()/.bottom()/.top()/.width()/.height()

// reading pixels
PdfPageImageObject::get_raw_bitmap(&self) -> Result<PdfBitmap<'_>, PdfiumError>
PdfPageImageObject::get_raw_image(&self)  -> Result<DynamicImage, PdfiumError>
PdfPageImageObject::get_processed_bitmap(&self, document: &PdfDocument) -> Result<PdfBitmap<'_>, PdfiumError>
PdfPageImageObject::get_processed_image(&self, document: &PdfDocument)  -> Result<DynamicImage, PdfiumError>
PdfPageImageObject::get_processed_image_with_width/height/size(...)
PdfPageImageObject::get_raw_image_data(&self) -> Result<Vec<u8>, PdfiumError>   // the encoded stream
PdfPageImageObject::width()/height() -> Result<Pixels /* i32 */, PdfiumError>
PdfPageImageObject::horizontal_dpi()/vertical_dpi() -> Result<f32, PdfiumError>
PdfPageImageObject::bits_per_pixel() -> Result<u8, PdfiumError>
PdfPageImageObject::color_space() -> Result<PdfColorSpace, PdfiumError>
PdfPageImageObject::filters(&self) -> PdfPageImageObjectFilters<'_>   // .iter() -> .name() -> &str

// writing
PdfPageImageObject::new(document: &PdfDocument<'a>, image: &DynamicImage) -> Result<Self, PdfiumError>
PdfPageImageObject::new_with_width/new_with_height/new_with_size(document, image, PdfPoints…)
PdfPageImageObject::new_from_jpeg_file(document, path) / new_from_jpeg_reader(document, R: Read + Seek)
PdfPageImageObject::set_image(&mut self, image: &DynamicImage) -> Result<(), PdfiumError>
PdfPageImageObject::set_bitmap(&mut self, bitmap: &PdfBitmap) -> Result<(), PdfiumError>

PdfPageObjectsCommon::add_image_object(&mut self, object: PdfPageImageObject<'a>)
    -> Result<PdfPageObject<'a>, PdfiumError>
PdfPageObjectsCommon::create_image_object(&mut self, x: PdfPoints, y: PdfPoints,
    image: &DynamicImage, width: Option<PdfPoints>, height: Option<PdfPoints>)
    -> Result<PdfPageObject<'a>, PdfiumError>
PdfPageObjectsCommon::remove_object(&mut self, object) / remove_object_at_index(&mut self, index: usize)
```

**Transforms are macro-generated inherent methods, not a trait.** On
`PdfPageImageObject` they take `&mut self` and return `Result<(), PdfiumError>`:
`transform(a,b,c,d,e,f)`, `apply_matrix(PdfMatrix)`, `translate(PdfPoints, PdfPoints)`,
`scale(f32, f32)`, `rotate_clockwise_degrees(f32)`, `rotate_counter_clockwise_degrees`,
`skew_degrees(f32,f32)`, `flip_horizontally`, `flip_vertically`, `reset_matrix(PdfMatrix)`.
Getters: `matrix() -> Result<PdfMatrix, _>`, `get_translation()`, `get_scale()`,
`get_rotation_clockwise_degrees()`, `get_skew_degrees()`.
`set_matrix()` is **deprecated** — use `apply_matrix()`.

### Which fixtures contain images

```
tracemonkey.pdf           0      annotation-highlight.pdf  0
160F-2019.pdf             2      annotation-line.pdf       0
TAMReview.pdf            47      annotation-freetext.pdf   0
alphatrans.pdf            1      rotation.pdf              0
issue12120_reduced.pdf    0
```

### Extraction

```
160F-2019.pdf page 0, obj 235:
  bounds L471.1 B410.5 R495.5 T427.7 (24.4 x 17.2 pt) | 102x72 px, 301 dpi, 8 bpp, Indexed | filters ["FlateDecode"]
  get_raw_image()           102x72   0.27 ms  -> 519 byte PNG
  get_processed_image(&doc) 100x69   3.51 ms  -> 602 byte PNG
  get_raw_image_data()      77 bytes of encoded (FlateDecode) stream data
```

* `get_raw_image()` = the image XObject's own pixels, no object matrix, no masks. Fast.
* `get_processed_image(&doc)` = what the page actually shows (soft masks, clipping,
  colour conversion applied); ~10x slower and it may differ in size by a pixel or two.
  **Use `get_processed_image()` for "extract images" UX, `get_raw_image()` for editing.**
* `get_raw_image_data()` returns the *compressed* stream bytes; for a `DCTDecode`
  image those bytes are a valid JPEG file, so a lossless "export original image"
  feature is possible by sniffing `filters()` first.

### Insertion and replacement

```
create_image_object(x=72, y=600, 160x120 pt)    5.72 ms  -> bounds L72.0 B600.0 R232.0 T720.0
manual: PdfPageImageObject::new + scale(200,150) + rotate_clockwise_degrees(15) + translate(120,400)
        matrix = [193.185 -51.764 38.823 144.889 120.0 400.0]
set_image() replacing an existing image object   0.03 ms  (object matrix is preserved,
                                                           bounds stayed 24.4 x 17.2 pt)
```

Order of operations for manual placement: a fresh `PdfPageImageObject` is **1 x 1 pt at
the origin**, so `scale(w_pt, h_pt)` then `rotate…` then `translate(x, y)` — transforms
are applied in call order and the result differs if you reorder them.

**Regeneration:** with the default `AutomaticOnEveryChange` strategy each object
mutation regenerates the page content stream. For bulk edits set
`page.set_content_regeneration_strategy(PdfPageContentRegenerationStrategy::Manual)`
and call `page.regenerate_content()` once.

---

## 4. Export

### Verified signatures

```rust
PdfRenderConfig::new()
    .scale_page_by_factor(f32) | .scale_page_width_by_factor | .scale_page_height_by_factor
    .set_target_width(Pixels) | .set_target_height | .set_target_size(w,h)
    .set_fixed_width/height/size(...)            // ignores aspect ratio
    .set_maximum_width(Pixels) | .set_maximum_height(Pixels)
    .thumbnail(size: Pixels)
    .rotate(PdfPageRenderRotation, bool) | .rotate_if_portrait | .rotate_if_landscape
    .set_format(PdfBitmapFormat)                 // Gray | BGR | BGRx | BGRA
    .render_annotations(bool) | .render_form_data(bool)
    .use_grayscale_rendering(bool) | .use_print_quality(bool) | .use_lcd_text_rendering(bool)
    .set_text_smoothing/.set_image_smoothing/.set_path_smoothing(bool)
    .clear_before_rendering(bool) | .set_clear_color(PdfColor)
    .highlight_all_form_fields(PdfColor)

PdfPage::render_with_config(&self, &PdfRenderConfig) -> Result<PdfBitmap<'_>, PdfiumError>
PdfPage::render_into_bitmap_with_config(&self, &mut PdfBitmap, &PdfRenderConfig) -> Result<(), PdfiumError>
PdfBitmap::empty(width: Pixels, height: Pixels, format: PdfBitmapFormat) -> Result<PdfBitmap<'a>, PdfiumError>
PdfBitmap::as_image(&self) -> Result<DynamicImage, PdfiumError>   // NOTE: returns a Result
PdfBitmap::as_rgba_bytes(&self) -> Vec<u8>;  as_raw_bytes(&self) -> Vec<u8>
PdfPage::text(&self) -> Result<PdfPageText<'_>, PdfiumError>;  PdfPageText::all(&self) -> String
```

### Measured — tracemonkey.pdf, 14 pages, 150 DPI (scale = 150/72), release build

```
total                                        153.0 ms
  render (FPDF_RenderPageBitmap)              64.8 ms   4.6 ms/page   1275 x 1650 px
  PdfBitmap -> DynamicImage                    3.1 ms   0.2 ms/page
  PNG encode + write                          61.7 ms   4.4 ms/page
PNG output                               16 822 109 bytes   1 173 KB/page
reusing ONE 1275x1650 PdfBitmap              67.5 ms   4.8 ms/page   (no measurable win)
text of 14 pages                             20.0 ms   1.4 ms/page   84 260 chars
```

Debug build for comparison: render is unchanged (4-5 ms/page, it is all C++) but PNG
encoding explodes to **480 ms/page**. Any timing you take of the `image` crate in a
debug build is meaningless.

Thumbnails at 180 px:

```
PdfRenderConfig::thumbnail(180)                    2.68 ms/page  -> 180 x 180 px  (ASPECT RATIO DESTROYED)
set_maximum_width(180).set_maximum_height(180)     2.53 ms/page  -> 180 x 180 px  (ALSO destroyed)
set_target_width(180).set_maximum_height(180)      2.77 ms/page  -> 139 x 180 px  (correct)
```

> **Gotcha:** `thumbnail(n)` internally does `set_target_size(n, n)`, which sets *both*
> scales, so pages are squashed into a square. `set_maximum_*` alone is no better —
> with neither target nor scale set, `do_maintain_aspect_ratio` is `false` and each
> axis is clamped independently. For an organiser grid use
> `set_target_width(n).set_maximum_height(n)` (or `set_target_height` for landscape),
> or just `scale_page_by_factor(n as f32 / longest_edge_pt)`.
> `thumbnail()` also silently turns **off** annotation and form rendering.

Page rotation is honoured by the renderer with no extra work:

```
rotation.pdf page 0 (None,      612x792 pt) -> 1275 x 1650 px
rotation.pdf page 1 (Degrees90, 792x612 pt) -> 1650 x 1275 px
```
(`FPDF_GetPageWidthF` already returns the rotated dimensions, confirmed against the
actual PNG IHDR bytes.)

---

## 5. Metadata, page labels, permissions, encryption

### Reading metadata — works

```rust
pub enum PdfDocumentMetadataTagType {
    Title, Author, Subject, Keywords, Creator, Producer, CreationDate, ModificationDate }
PdfMetadata::get(&self, tag: PdfDocumentMetadataTagType) -> Option<PdfDocumentMetadataTag>
PdfMetadata::iter(&self) -> Iter<'_, PdfDocumentMetadataTag>       // only tags that exist
PdfMetadata::len(&self) -> usize
PdfDocumentMetadataTag::{tag_type() -> PdfDocumentMetadataTagType, value() -> &str}
PdfDocument::version(&self) -> PdfDocumentVersion;  set_version(&mut self, PdfDocumentVersion)
PdfCatalog::{is_tagged() -> bool, get_language() -> Result<String,_>, set_language(impl ToString)}
```

```
tracemonkey.pdf  Creator "TeX", Producer "pdfeTeX-1.21a", CreationDate "D:20090401163925-07'00'", version Pdf1_4
TAMReview.pdf    Title "Overview of the Technology Acceptance Model…", Author "ARRAY(0x2c7cf020)", version Pdf1_3
new doc via copy Creator/Producer "PDFium", CreationDate "D:20260915175612+09'00'"
```

Dates come back as raw PDF date strings (`D:YYYYMMDDHHmmSSOHH'mm'`) — parse them ourselves.

### Writing metadata — **NOT SUPPORTED**

`PdfMetadata` has `get`/`iter`/`len` and **no setter**. The underlying library is the
limit, not the wrapper: PDFium exports `FPDF_GetMetaText` and there is **no
`FPDF_SetMetaText`** in build 8057. `PdfCatalog::set_language()` is the only document-level
string we can write.

> **Needs another crate.** Title/Author/Subject/Keywords/Creator/Producer/dates must be
> written by post-processing the saved bytes with `lopdf` (mutate the `/Info` dictionary
> and, for PDF 2.0 correctness, the XMP metadata stream). See "needed crates".

### Page labels — read only, and they work well

`PdfPage::label() -> Option<&str>` is `FPDF_GetPageLabel`, and PDFium resolves the whole
`/PageLabels` number tree for us. Verified against a hand-built fixture with three styles:

```
outline-labels.pdf  ->  [Some("i"), Some("ii"), Some("1"), Some("2"), Some("App-A"), Some("App-B")]
                        (/S /r,        then /S /D /St 1,    then /S /A /P (App-))
160F-2019.pdf       ->  [Some("1")]
tracemonkey.pdf     ->  [None, None, …]   (no /PageLabels at all)
```

There is **no way to write page labels** — same `lopdf` post-processing story.

### Permissions

```rust
PdfPermissions::security_handler_revision(&self) -> Result<PdfSecurityHandlerRevision, PdfiumError>
    // Unprotected | Revision2 | Revision3 | Revision4 | …
PdfPermissions::{can_print_high_quality, can_print_only_low_quality, can_assemble_document,
    can_modify_document_content, can_extract_text_and_graphics,
    can_fill_existing_interactive_form_fields, can_create_new_interactive_form_fields,
    can_add_or_modify_text_annotations} -> Result<bool, PdfiumError>
```

```
tracemonkey.pdf (unencrypted)     security_handler_revision = Ok(Unprotected), everything true
encrypted, USER password          Ok(Revision2): print=false, extract_text=false,
                                  modify=true, annotate=true, fill_forms=true, assemble=true
encrypted, OWNER password         Ok(Revision2): everything true
```

PDFium correctly applies the *owner-password* escalation, so a document opened with the
owner password reports full rights. Our UI must show permissions from the handle we
actually opened with.

### Encryption

* **Reading works.** No fixture shipped encrypted, so the spike generates
  `fixtures/out/encrypted-rc4-40.pdf` (standard security handler, V=1 R=2 RC4-40,
  user `user`, owner `owner`, `/P -21`) with
  `fixtures/out/_gen/make_encrypted.py`.
* `load_pdf_from_file(path, None)` and a wrong password both return
  `PdfiumError::PdfiumLibraryInternalError(PdfiumInternalError::PasswordError)` — a clean,
  distinguishable signal for a "enter password" dialog.
* Strings/streams are decrypted transparently: `Title` read back as
  `"Encrypted Spike Fixture"`.
* **Writing encryption is NOT SUPPORTED.** `FPDF_SaveAsCopy` takes no password argument
  and the only security-related flag *removes* security. Confirmed behaviour:
  * `PdfDocument::save_to_file()` (flags = 0) on a document opened with a password
    **keeps the encryption** — reopening without a password returns `PasswordError`.
  * `FPDF_SaveAsCopy(…, FPDF_REMOVE_SECURITY)` produces a plain file that opens with no
    password (1082 bytes vs 1108).

> **Needs another crate** for "set/change password" and "remove permissions": `lopdf`
> has no encryption writer either; realistic options are `qpdf` bindings, `pdf-writer`
> plus our own standard-security-handler implementation, or shelling out. Recommend
> deferring "encrypt" to a later milestone and shipping only "decrypt / remove password"
> (which pdfium does natively) in v1.

---

## 6. Bookmarks / outline

### Verified signatures

```rust
PdfDocument::bookmarks(&self) -> &PdfBookmarks<'_>
PdfBookmarks::root(&self) -> Option<PdfBookmark<'_>>
PdfBookmarks::iter(&self) -> PdfBookmarksIterator<'_>        // depth-first prefix order, whole tree
PdfBookmarks::find_first_by_title(&self, &str) -> Result<PdfBookmark<'_>, PdfiumError>
PdfBookmarks::find_all_by_title(&self, &str) -> Vec<PdfBookmark<'_>>

PdfBookmark::title(&self) -> Option<String>
PdfBookmark::action(&self) -> Option<PdfAction<'a>>          // .action_type()
PdfBookmark::destination(&self) -> Option<PdfDestination<'a>>
PdfDestination::page_index(&self) -> Result<PdfPageIndex, PdfiumError>
PdfDestination::view_settings(&self) -> Result<PdfDestinationViewSettings, PdfiumError>
PdfBookmark::parent(&self) -> Option<PdfBookmark<'a>>
PdfBookmark::children_len(&self) -> usize
PdfBookmark::first_child(&self) -> Option<PdfBookmark<'a>>
PdfBookmark::next_sibling(&self) -> Option<PdfBookmark<'a>>
PdfBookmark::iter_siblings(&self) / iter_direct_children(&self) / iter_all_descendants(&self)
```

Verified on the generated `fixtures/out/outline-labels.pdf` (7 nodes, 3 levels):

```
"Chapter A"        -> page 0   (children_len 2)
  "A.1 Introduction" -> page 1
  "A.2 Method"       -> page 2
"Chapter B"        -> page 3   (children_len 2)
  "B.1 Results"      -> page 4 (children_len 1)
    "B.1.a Detail"   -> page 5
"Chapter C"        -> page 5
depth-first walk visited 7 nodes; PdfBookmarks::iter() counted 7
```

Of the shipped fixtures only `TAMReview.pdf` has an outline, and it is a single flat
node (`"htmldoc797.html"` -> page 0).

> **Gotcha:** `children_len()` is documented as "the number of direct children" but is
> really `|FPDFBookmark_GetCount|`, i.e. the `/Count` entry — the number of *visible
> descendants*. My "Chapter B" has one direct child but `/Count 2`, and `children_len()`
> returned **2**. Never size a tree node from it; count `iter_direct_children()` instead.
> Its sign (dropped by `unsigned_abs()`) is the open/closed state, which is therefore
> unreadable through the safe API — read it with raw `FPDFBookmark_GetCount` if the
> UI wants to restore expand/collapse state.

> **`root()` is the FIRST TOP-LEVEL bookmark, not a synthetic root.** To walk the whole
> tree yourself you must also follow `root().iter_siblings()`. `PdfBookmarks::iter()`
> already does this.

**Writing the outline is NOT SUPPORTED** — no create/delete/rename/move/reorder anywhere
in `PdfBookmarks` or `PdfBookmark`, and PDFium has no public API for it either. Bookmark
editing needs `lopdf`.

---

## 7. Attachments (embedded files) — full read *and* write

```rust
PdfDocument::attachments(&self) -> &PdfAttachments<'_>;  attachments_mut(&mut self) -> &mut PdfAttachments<'a>
PdfAttachments::{len() -> PdfAttachmentIndex /* u16-ish c_int */, is_empty, as_range, as_range_inclusive,
                 get(index) -> Result<PdfAttachment<'a>, PdfiumError>, iter()}
PdfAttachments::create_attachment_from_bytes(&mut self, name: &str, bytes: &[u8])
    -> Result<PdfAttachment<'_>, PdfiumError>
PdfAttachments::create_attachment_from_file(&mut self, name: &str, path) -> Result<…>
PdfAttachments::create_attachment_from_reader<R: Read>(&mut self, name: &str, reader: R) -> Result<…>
PdfAttachments::delete_at_index(&mut self, index: PdfAttachmentIndex) -> Result<(), PdfiumError>
PdfAttachment::{name() -> String, len() -> usize, is_empty(),
                save_to_bytes() -> Result<Vec<u8>, _>, save_to_writer<W: Write>, save_to_file(path)}
```

No shipped fixture has embedded files, so the spike creates them:

```
create_attachment_from_bytes                  0.01 ms
save + reopen fixtures/out/with_attachments.pdf -> 2 attachments
  [0] notes.txt  -> 27 bytes, "hello from the SeePDF spike"
  [1] second.bin ->  6 bytes, binary round-tripped intact
delete_at_index(0) -> 1 attachment
```

Names must be unique (the API errors otherwise). This is the one metadata-ish area where
pdfium gives us a complete read/write story.

---

## 8. Save flags and "optimise"

### `PdfDocument::save_*` hard-codes `flags = 0`

```rust
// pdf/document.rs, save_to_writer()
let flags = 0;   // TODO comment in the crate: FPDF_INCREMENTAL / NO_INCREMENTAL / REMOVE_SECURITY unsupported
```

To use flags you must call the raw bindings with your own `FPDF_FILEWRITE`. The struct
is exported (`pdfium_render::prelude::FPDF_FILEWRITE`) so a `#[repr(C)]` extension works:

```rust
#[repr(C)]
struct FileWriter {
    version: c_int,                                   // must be 1
    write_block: Option<unsafe extern "C" fn(*mut FileWriter, *const c_void, c_ulong) -> c_int>,
    buf: Vec<u8>,                                     // our extra field, after the FPDF_FILEWRITE prefix
}
// pass `self as *mut FileWriter as *mut FPDF_FILEWRITE`

unsafe fn FPDF_SaveAsCopy(&self, document: FPDF_DOCUMENT, pFileWrite: *mut FPDF_FILEWRITE,
                          flags: FPDF_DWORD /* = c_ulong */) -> FPDF_BOOL;
unsafe fn FPDF_SaveWithVersion(&self, document: FPDF_DOCUMENT, pFileWrite: *mut FPDF_FILEWRITE,
                               flags: FPDF_DWORD, fileVersion: c_int) -> FPDF_BOOL;
```

Flag values in build 8057: `FPDF_INCREMENTAL = 1`, `FPDF_NO_INCREMENTAL = 2`,
`FPDF_REMOVE_SECURITY_DEPRECATED = 3`, **`FPDF_REMOVE_SECURITY = 4`**.
(Note the 3-vs-4 change — passing 3 is the deprecated value.)

### Measured on tracemonkey.pdf (source 1 016 315 bytes, unmodified)

```
flags = 0 (default)        ok    996 699 bytes   5.5 ms
FPDF_INCREMENTAL           ok  2 027 459 bytes   1.0 ms     <- original + a full appended revision
FPDF_NO_INCREMENTAL        ok    996 699 bytes   2.4 ms
FPDF_REMOVE_SECURITY       ok    996 699 bytes   2.4 ms
SaveWithVersion(14)        ok    996 699 bytes   2.4 ms
SaveWithVersion(17)        ok    996 699 bytes   2.4 ms
```

After an actual edit (`FPDFPage_Delete(0)`):

```
FPDF_INCREMENTAL      1 018 893 bytes   0.2 ms
FPDF_NO_INCREMENTAL     965 848 bytes   5.3 ms
```

Read: **flags = 0 behaves as a full (non-incremental) rewrite**, which already drops
unreferenced objects (~2 % here). `FPDF_INCREMENTAL` is ~25x faster but grows the file —
useful for an autosave/undo journal, useless as a "save" or "optimise". `SaveWithVersion`
changes only the header version, not the byte layout. `FPDF_REMOVE_SECURITY` on an
encrypted document produced a file that opens without a password (verified).

**PDFium offers no image downsampling, font subsetting, or stream re-compression.** Any
real "optimise/compress" must be app-level. Probe on TAMReview.pdf (47 images, widest 161 px):

```
halve every image >= 64 px wide, re-set_image(), regenerate, save
23 images rewritten in 29 ms;  674 629 -> 576 729 bytes  (-14.5 %)
```

> **Gotcha:** `set_image()` hands pdfium a raw bitmap, which it stores **Flate-compressed,
> never JPEG**. Downsampling a photo-heavy PDF this way can *increase* the file size.
> A real optimiser should read `filters()`, and for `DCTDecode` sources re-encode to JPEG
> ourselves and write the object back — which `PdfPageImageObject` cannot do
> (`new_from_jpeg_reader` creates a *new* object, there is no `set_jpeg`). Realistically
> "압축/최적화" needs `lopdf` to replace the image XObject stream in place.

---

## Unsupported / broken summary

| Feature | Status |
|---|---|
| Move / reorder pages | no safe API — raw `FPDF_MovePages` works, 0.02 ms |
| Duplicate page in the same document | no safe API — second document handle, or raw `FPDF_ImportPagesByIndex(dest == src)` |
| Write document metadata (`/Info`) | **impossible with pdfium** (no `FPDF_SetMetaText`) → `lopdf` |
| Write page labels | **impossible with pdfium** → `lopdf` |
| Create / edit bookmarks | **impossible with pdfium** → `lopdf` |
| Encrypt / set password | **impossible with pdfium** → external crate |
| Remove password | works, `FPDF_SaveAsCopy(FPDF_REMOVE_SECURITY = 4)` via raw bindings |
| Save flags in the safe API | hard-coded to 0 → raw bindings + own `FPDF_FILEWRITE` |
| `/AcroForm` survives page import | **no** — widgets survive, the form dictionary does not |
| Image re-compression / JPEG passthrough on write | no — `set_image()` always writes Flate |
| `PdfPage::flatten()` | needs the crate's `flatten` feature, **not enabled** in our Cargo.toml |
| `PdfDocument::handle()` / `bindings()` | `pub(crate)`; must keep a second `bind_to_library()` handle |
| `children_len()` = direct children | **no**, it is `/Count` (visible descendants) |
| `thumbnail(n)` preserves aspect ratio | **no**, produces `n x n` |

## Needed crates (not installed — do not add without approval)

* **`lopdf`** — the single highest-value addition. Needed for: `/Info` metadata writing,
  `/PageLabels`, bookmark/outline editing, preserving `/AcroForm` across split/extract,
  in-place image stream replacement for real compression. Pure Rust, no C deps.
* *(optional, later)* an encryption writer for "set password" — `qpdf` FFI or a
  hand-rolled standard-security-handler on top of `pdf-writer`. Recommend deferring.
* `rayon` is **not** needed: pdfium is single-threaded and the `thread_safe` feature
  serialises every call on one mutex; parallelism must come from an actor/queue design,
  and PNG encoding (the only CPU-parallel part) can use `std::thread`.

---

## Recommendations for the page-organiser UX

**Architecture**

1. Run every pdfium call on **one dedicated worker thread** (`crossbeam-channel`
   command queue, already a dependency). `thread_safe` makes calls safe but not
   concurrent, and the borrow checker does not stop two `&PdfDocument` edits.
2. Keep exactly **one** `PdfDocument` open per editor tab, plus a second
   `Box<dyn PdfiumLibraryBindings>` obtained at startup (both `bind_to_library()`
   calls before `Pdfium::new()`) for `FPDF_MovePages` and flagged saves.
3. Do not hold `PdfPage` values across commands — `PdfPageIndexCache` maps handles to
   indices and a raw reorder invalidates it. Open, mutate, drop.

**Thumbnail grid**

4. Render thumbnails with
   `PdfRenderConfig::new().set_target_width(n).set_maximum_height(n)` — **never**
   `thumbnail(n)` (squashes to `n x n` and disables annotation rendering).
   At 180 px that is **2.8 ms/page**, so a 500-page document renders in ~1.4 s: render
   lazily for the visible rows, cache by `(page_handle_generation, size)`, and refresh a
   single tile after rotate/replace rather than the whole grid.
5. Use `pages().page_sizes()` (0.23 ms for 14 pages) to lay the grid out *before* any
   pixels exist, so tiles never reflow.
6. Rotation is free to preview: the renderer honours `/Rotate`, so after `set_rotation()`
   just re-render that one tile.

**Multi-select delete / rotate**

7. Delete **descending by index** (`page.delete()` shifts everything after it), or
   collect the keep-set and rebuild. Each delete costs ~2 ms.
8. For a multi-page rotate, set `PdfPageContentRegenerationStrategy::Manual` on each page,
   apply, then `regenerate_content()` once — the 2.8 ms of `set_rotation` is almost all
   regeneration.
9. Implement undo as a **command journal** (delete page 3, rotate page 5 by 90…) replayed
   against the on-disk original, not as document snapshots. `save_to_bytes()` of a 1 MB
   document is ~5 ms, so a snapshot-per-action is affordable for small files, but the
   journal keeps memory flat for 100 MB scans.

**Drag reorder**

10. Model the grid as a `Vec<PageId>`; on drop, diff against the previous order and emit
    a single `FPDF_MovePages(indices, dest)` call (0.02 ms). Dragging a contiguous
    multi-selection maps directly onto its ordered-list semantics.
11. Guard against the documented failure modes before calling: no duplicate indices, every
    index `< page_count`, and `dest + indices.len() <= page_count`. The call returns
    `false` and, per the header, may leave the document "in an indeterminate state" — so
    validate in Rust first and treat a `false` return as "reload the document".

**Merge / split dialogs**

12. Merge: `create_new_pdf()` then `copy_pages_from_document(&src, "1-3,5", at)` per source.
    ~0.65 ms per 4 pages. Expose the 1-based range string directly to the user (it is what
    people type) but convert to `copy_page_range_from_document`'s 0-based
    `RangeInclusive` internally where we generate ranges — **never mix the two**.
13. Split: prefer **load the original, delete the unwanted pages, save** over
    "new document + import". Import loses `/AcroForm` (verified: 76 widgets survive, the
    form dictionary does not), the outline, and document metadata; deleting keeps all of it.
    Use import only for true merges of unrelated files.
14. Warn the user (Korean-first UI) when a split/merge source has form fields or an
    outline that we cannot preserve yet, until `lopdf` post-processing lands.

**Export**

15. 150 DPI PNG export of a 14-page document is **153 ms** total, split evenly between
    pdfium rendering (4.6 ms/page) and PNG encoding (4.4 ms/page). Move PNG encoding off
    the pdfium thread — it is the only part that parallelises — and report progress per
    page. Reusing one `PdfBitmap` is not worth the complexity (no measurable gain).
16. 1.17 MB/page at 150 DPI for text pages: offer JPEG/WebP and a DPI selector, and never
    write PNGs into a temp dir without a size estimate up front.
17. Text export is 1.4 ms/page and needs no special handling.
