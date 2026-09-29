//! Export and print — `IPC_CONTRACT.md` §7.7, `ARCHITECTURE.md` §3.2.
//!
//! Image export renders through the same `PdfRenderConfig` the viewer uses, so what you export
//! is what you saw: annotations and form field values included, `/Rotate` honoured (PDFium's
//! own renderer applies it), no LCD subpixel AA. The output pixel size is
//! `round(pt × dpi / 72)`, which is what `export_images_sizes` pins.
//!
//! Flattening is raw `FPDFPage_Flatten(page, FLAT_NORMALDISPLAY)` + `FPDFPage_GenerateContent`
//! on a **scratch copy** of the document — never `PdfPage::flatten()`, which is `FLAT_PRINT`
//! and deletes annotations that lack the `/F 4` Print flag (annotations spike §3.3, and why
//! the crate's `flatten` feature is off in `Cargo.toml`). The page handle is invalid after the
//! call, so the scratch document is serialised rather than read back page by page.

pub mod job;
pub mod summary;

use crate::engine::raw;
use crate::engine::render::geometry;
use crate::engine::save;
use crate::engine::text::layer;
use crate::engine::types::EngineState;
use crate::ipc::error::PdfiumResultExt;
use crate::ipc::types::{ExportEstimate, ExportTextResult, ImageFormat, PageIndex, Rect};
use crate::ipc::{EngineError, ErrorCode};
use pdfium_render::prelude::*;
use std::path::{Path, PathBuf};

/// The `estimate_export` sample size: enough to average out a title page against a dense one
/// without making the dialog wait.
const SAMPLE_PAGES: usize = 3;

/// Lowest and highest DPI the UI may ask for.
const MIN_DPI: u32 = 36;
const MAX_DPI: u32 = 1200;

// ---------------------------------------------------------------------------------------
// Image export
// ---------------------------------------------------------------------------------------

/// One exported image.
pub struct ExportedImage {
    pub path: PathBuf,
    pub width: u32,
    pub height: u32,
    pub bytes: u64,
}

/// Renders one page at `dpi` and writes it as PNG or JPEG.
///
/// `transparent` only affects PNG: JPEG has no alpha, so a transparent request falls back to
/// white rather than producing a black page.
#[allow(clippy::too_many_arguments)]
pub fn export_page_image(
    st: &mut EngineState<'_>,
    doc_id: &str,
    page: PageIndex,
    format: ImageFormat,
    dpi: u32,
    quality: Option<u8>,
    out_dir: &Path,
    base_name: &str,
    transparent: bool,
) -> Result<ExportedImage, EngineError> {
    let image = render_page(
        st,
        doc_id,
        page,
        dpi,
        transparent && format == ImageFormat::Png,
    )?;
    std::fs::create_dir_all(out_dir).map_err(EngineError::from)?;
    let name = format!(
        "{}-{:03}.{}",
        sanitize(base_name),
        page as u32 + 1,
        match format {
            ImageFormat::Png => "png",
            ImageFormat::Jpeg => "jpg",
        }
    );
    let path = out_dir.join(name);
    let encoded = encode(&image, format, quality)?;
    let bytes = crate::engine::pages::write_atomic(&path, &encoded)?;
    Ok(ExportedImage {
        path,
        width: image.width(),
        height: image.height(),
        bytes,
    })
}

/// The shared render step: `dpi / 72` scale, annotations and form data on.
fn render_page(
    st: &mut EngineState<'_>,
    doc_id: &str,
    page: PageIndex,
    dpi: u32,
    transparent: bool,
) -> Result<image::DynamicImage, EngineError> {
    let dpi = check_dpi(dpi)?;
    let scale = dpi as f32 / 72.0;
    let geom = st.doc(doc_id)?.geom(page)?.clone();
    let (w, h) = geometry::page_pixels(&geom, 0, scale);
    if w.saturating_mul(h) > geometry::HARD_MAX_PX {
        return Err(EngineError::invalid(format!(
            "{w}x{h} px at {dpi} DPI exceeds the render ceiling; lower the DPI"
        ))
        .with_page(page));
    }
    let config = PdfRenderConfig::new()
        .scale_page_by_factor(scale)
        .render_form_data(true)
        .render_annotations(true)
        .use_lcd_text_rendering(false)
        .use_print_quality(true)
        .set_format(PdfBitmapFormat::BGRA)
        .set_reverse_byte_order(true)
        .set_clear_color(if transparent {
            PdfColor::new(255, 255, 255, 0)
        } else {
            PdfColor::WHITE
        });
    let doc = st.doc_mut(doc_id)?;
    let pdf_page = doc.page(page)?;
    let mut bitmap =
        PdfBitmap::empty(w as Pixels, h as Pixels, PdfBitmapFormat::BGRA).ctx("allocate bitmap")?;
    pdf_page
        .render_into_bitmap_with_config(&mut bitmap, &config)
        .ctx("render page")?;
    let pixels = bitmap.as_raw_bytes();
    let buffer = image::RgbaImage::from_raw(w, h, pixels).ok_or_else(|| {
        EngineError::new(ErrorCode::Pdfium, "the rendered bitmap has the wrong size")
    })?;
    Ok(image::DynamicImage::ImageRgba8(buffer))
}

fn encode(
    image: &image::DynamicImage,
    format: ImageFormat,
    quality: Option<u8>,
) -> Result<Vec<u8>, EngineError> {
    let mut out = Vec::new();
    let mut cursor = std::io::Cursor::new(&mut out);
    match format {
        ImageFormat::Png => image
            .write_to(&mut cursor, image::ImageFormat::Png)
            .map_err(|e| EngineError::new(ErrorCode::Io, format!("PNG encode: {e}")))?,
        ImageFormat::Jpeg => {
            // JPEG has no alpha channel; compositing on white first avoids a black page.
            let rgb = image::DynamicImage::ImageRgb8(image.to_rgb8());
            let quality = quality.unwrap_or(85).clamp(1, 100);
            let mut encoder =
                image::codecs::jpeg::JpegEncoder::new_with_quality(&mut cursor, quality);
            encoder
                .encode_image(&rgb)
                .map_err(|e| EngineError::new(ErrorCode::Io, format!("JPEG encode: {e}")))?;
        }
    }
    Ok(out)
}

fn check_dpi(dpi: u32) -> Result<u32, EngineError> {
    if !(MIN_DPI..=MAX_DPI).contains(&dpi) {
        return Err(EngineError::invalid(format!(
            "{dpi} DPI is outside {MIN_DPI}..={MAX_DPI}"
        )));
    }
    Ok(dpi)
}

fn sanitize(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, '-' | '_' | '.' | ' ') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let trimmed = cleaned.trim();
    if trimmed.is_empty() {
        "page".to_string()
    } else {
        trimmed.to_string()
    }
}

/// `estimate_export` — renders and encodes up to three sample pages and scales the result.
///
/// F-24 wants the estimate within ±25 % of the real output, which a sample of real encodes
/// reaches easily; a formula from the pixel count does not (PNG of a text page compresses ~30×
/// better than a photo).
pub fn estimate(
    st: &mut EngineState<'_>,
    doc_id: &str,
    pages: &[PageIndex],
    format: ImageFormat,
    dpi: u32,
) -> Result<ExportEstimate, EngineError> {
    let pages = check_pages(st, doc_id, pages)?;
    let step = (pages.len() / SAMPLE_PAGES).max(1);
    let sample: Vec<PageIndex> = pages
        .iter()
        .copied()
        .step_by(step)
        .take(SAMPLE_PAGES)
        .collect();
    let mut total = 0u64;
    for &page in &sample {
        let image = render_page(st, doc_id, page, dpi, false)?;
        total += encode(&image, format, None)?.len() as u64;
    }
    let sampled = sample.len().max(1) as u64;
    Ok(ExportEstimate {
        bytes: total / sampled * pages.len() as u64,
        sampled_pages: sampled as u32,
    })
}

// ---------------------------------------------------------------------------------------
// Text export
// ---------------------------------------------------------------------------------------

/// `export_text` — `page.text().all()` per page, separated by a form feed (F-25).
pub fn export_text(
    st: &mut EngineState<'_>,
    doc_id: &str,
    pages: &[PageIndex],
    out_path: &str,
) -> Result<ExportTextResult, EngineError> {
    // v0.3 pkg3 (S5): the document's copy / extract permission.
    crate::engine::security::ensure_doc_permitted(
        st,
        doc_id,
        crate::engine::security::Perm::ExtractText,
    )?;
    let pages = check_pages(st, doc_id, pages)?;
    let mut out = String::new();
    for (i, &page) in pages.iter().enumerate() {
        if i > 0 {
            // U+000C, the conventional page separator in extracted PDF text.
            out.push('\u{0C}');
        }
        let doc = st.doc_mut(doc_id)?;
        out.push_str(&layer::page_text(doc, page)?.text);
    }
    let chars = out.chars().count() as u64;
    crate::engine::pages::write_atomic(Path::new(out_path), out.as_bytes())?;
    Ok(ExportTextResult { chars })
}

// ---------------------------------------------------------------------------------------
// Flatten
// ---------------------------------------------------------------------------------------

/// Flattens a **copy** of the document and returns its bytes.
///
/// `annotations` (every non-widget annotation) and `forms` (AcroForm widgets) select what is
/// merged into the content stream; with both false there is nothing to do and the document is
/// returned as-is. `pages` restricts the flattening to a subset (the others are copied
/// untouched) — the print path uses that for "print the current page".
///
/// Only one of the two is the hard case: `FPDFPage_Flatten` bakes *every* visible annotation
/// and then deletes the page's whole `/Annots` array, so it cannot be told to leave the fields
/// (or the comments) alone. On a page with both kinds the ones to keep are parked outside
/// `/Annots` for the flatten and put back afterwards — see [`KeepPlan`].
pub fn flatten_bytes(
    st: &mut EngineState<'_>,
    doc_id: &str,
    annotations: bool,
    forms: bool,
    pages: Option<&[PageIndex]>,
) -> Result<Vec<u8>, EngineError> {
    let source = save::serialize(st, doc_id)?;
    if !annotations && !forms {
        return Ok(source);
    }
    let password = st.doc(doc_id)?.password.clone();
    let bindings = st.doc(doc_id)?.bindings();
    let total = st.doc(doc_id)?.page_count();
    let wanted: Vec<u16> = match pages {
        Some(list) => list.iter().copied().filter(|&p| p < total).collect(),
        None => (0..total).collect(),
    };
    // Partial: `forms` alone bakes the widgets, `annotations` alone everything else.
    let (plan, source) = if annotations != forms {
        KeepPlan::park(source, password.as_deref(), &wanted, forms)?
    } else {
        (KeepPlan::default(), source)
    };
    let scratch = st
        .pdfium
        .load_pdf_from_byte_vec(source, password.as_deref())
        .map_err(|e| EngineError::pdfium("reopen for flatten", e))?;
    for index in wanted {
        if plan.untouched.contains(&index) {
            // Nothing of the selected kind on this page: it stays as it is.
            continue;
        }
        let page = scratch
            .pages()
            .get(index as i32)
            .ctx(&format!("load page {index}"))?;
        // NormalDisplay, never Print: FLAT_PRINT deletes annotations without `/F 4`.
        raw::page::flatten(bindings, &page, raw::page::FlattenMode::NormalDisplay)?;
        // The FPDF_PAGE is invalid after a flatten; dropping it here is the reload.
        drop(page);
    }
    let flattened =
        raw::save::save_as_copy(bindings, &scratch, raw::save::SaveFlags::NoIncremental)?;
    drop(scratch);
    plan.restore(flattened, password.as_deref())
}

/// The page key a partial flatten parks the annotations it keeps under.
const PARKED_ANNOTS: &[u8] = b"SeePDFKeptAnnots";

/// The annotations a partial flatten keeps.
///
/// `FPDFPage_Flatten` drops the page's `/Annots` wholesale, PDFium has no call that puts an
/// existing annotation dictionary back on a page, and `FPDF_SaveAsCopy` only writes objects
/// that are still referenced. So, with `lopdf`, each page's entries to keep (references, or
/// the inline dictionary itself) move from `/Annots` to a private page key before PDFium
/// loads the bytes — `/Annots` then holds only what is baked, and the kept dictionaries stay
/// referenced, keep their object numbers and are written — and move back into `/Annots`
/// afterwards. Both moves are incremental updates, so nothing is copied twice and an
/// encrypted file stays encrypted (lopdf encrypts the rewritten page dictionaries with the
/// document's own key). The kept annotations stay live: fields fillable, comments editable.
#[derive(Default)]
struct KeepPlan {
    /// Pages whose kept annotations are parked.
    parked: Vec<u16>,
    /// Pages with nothing to bake (only kept annotations, or none at all).
    untouched: std::collections::HashSet<u16>,
}

impl KeepPlan {
    /// Parks the kept annotations of every page in `wanted` that has both kinds. `widgets`:
    /// the widgets are baked (and everything else kept) — or the reverse.
    fn park(
        bytes: Vec<u8>,
        password: Option<&str>,
        wanted: &[u16],
        widgets: bool,
    ) -> Result<(Self, Vec<u8>), EngineError> {
        use lopdf::Object;
        let doc = load_lopdf(&bytes, password)?;
        let page_ids = doc.get_pages();
        let mut plan = KeepPlan::default();
        let mut moves: Vec<(lopdf::ObjectId, Vec<Object>, Vec<Object>)> = Vec::new();
        for &index in wanted {
            let Some(&page_id) = page_ids.get(&(u32::from(index) + 1)) else {
                continue;
            };
            let page = doc
                .get_dictionary(page_id)
                .map_err(|e| save::lopdf_error("page", e))?;
            let entries: Vec<Object> = match page.get(b"Annots") {
                Ok(Object::Reference(id)) => doc
                    .get_object(*id)
                    .and_then(Object::as_array)
                    .cloned()
                    .unwrap_or_default(),
                Ok(Object::Array(array)) => array.clone(),
                _ => Vec::new(),
            };
            let (mut bake, mut keep) = (Vec::new(), Vec::new());
            for entry in entries {
                let dict = match &entry {
                    Object::Reference(id) => doc.get_dictionary(*id).ok(),
                    Object::Dictionary(d) => Some(d),
                    _ => None,
                };
                // A slot that holds no annotation (null, dangling) is neither: the flatten
                // ignores it and it does not come back.
                let Some(dict) = dict else { continue };
                let is_widget = dict
                    .get(b"Subtype")
                    .and_then(Object::as_name)
                    .map(|n| n == b"Widget")
                    .unwrap_or(false);
                if is_widget == widgets {
                    bake.push(entry);
                } else {
                    keep.push(entry);
                }
            }
            if bake.is_empty() {
                plan.untouched.insert(index);
            } else if !keep.is_empty() {
                plan.parked.push(index);
                moves.push((page_id, bake, keep));
            }
        }
        if moves.is_empty() {
            return Ok((plan, bytes));
        }
        let mut update = lopdf::IncrementalDocument::create_from(bytes, doc);
        for (page_id, bake, keep) in moves {
            update
                .opt_clone_object_to_new_document(page_id)
                .map_err(|e| save::lopdf_error("page", e))?;
            let page = update
                .new_document
                .get_dictionary_mut(page_id)
                .map_err(|e| save::lopdf_error("page", e))?;
            page.set("Annots", Object::Array(bake));
            page.set(PARKED_ANNOTS, Object::Array(keep));
        }
        Ok((plan, save_update(update)?))
    }

    /// Moves the parked entries back into each page's `/Annots`.
    fn restore(self, flattened: Vec<u8>, password: Option<&str>) -> Result<Vec<u8>, EngineError> {
        if self.parked.is_empty() {
            return Ok(flattened);
        }
        let prev = load_lopdf(&flattened, password)?;
        let page_ids = prev.get_pages();
        let mut update = lopdf::IncrementalDocument::create_from(flattened, prev);
        for index in self.parked {
            let page_id = *page_ids.get(&(u32::from(index) + 1)).ok_or_else(|| {
                EngineError::new(ErrorCode::Pdfium, format!("page {index} vanished"))
            })?;
            update
                .opt_clone_object_to_new_document(page_id)
                .map_err(|e| save::lopdf_error("page", e))?;
            let page = update
                .new_document
                .get_dictionary_mut(page_id)
                .map_err(|e| save::lopdf_error("page", e))?;
            let kept = page.remove(PARKED_ANNOTS).ok_or_else(|| {
                EngineError::new(
                    ErrorCode::Pdfium,
                    format!("page {index} lost its parked annotations"),
                )
            })?;
            page.set("Annots", kept);
        }
        save_update(update)
    }
}

/// `lopdf` over bytes PDFium wrote, decrypted with the document's password when it has one
/// (an owner-restricted file opens with the empty user password).
fn load_lopdf(bytes: &[u8], password: Option<&str>) -> Result<lopdf::Document, EngineError> {
    let options = lopdf::LoadOptions::with_password(password.unwrap_or(""));
    lopdf::Document::load_mem_with_options(bytes, options)
        .map_err(|e| save::lopdf_error("parse", e))
}

fn save_update(mut update: lopdf::IncrementalDocument) -> Result<Vec<u8>, EngineError> {
    let mut out = Vec::new();
    update
        .save_to(&mut out)
        .map_err(|e| save::lopdf_error("write", e))?;
    Ok(out)
}

/// `export_flattened` — [`flatten_bytes`] written atomically to `out_path`.
pub fn export_flattened(
    st: &mut EngineState<'_>,
    doc_id: &str,
    out_path: &str,
    annotations: bool,
    forms: bool,
    pages: Option<&[PageIndex]>,
) -> Result<u64, EngineError> {
    let bytes = flatten_bytes(st, doc_id, annotations, forms, pages)?;
    crate::engine::pages::write_atomic(Path::new(out_path), &bytes)
}

/// `print_prepare` — a flattened temp copy for the OS print handler.
///
/// The primary print path is the frontend's print-only DOM plus `getCurrentWebview().print()`;
/// this is the fallback (`WORKPLAN.md` §6). Annotations and form values are flattened in so the
/// printed page matches the screen even in a handler that ignores them.
pub fn print_prepare(
    st: &mut EngineState<'_>,
    doc_id: &str,
    pages: Option<&[PageIndex]>,
) -> Result<PathBuf, EngineError> {
    // v0.3 pkg3 (S5): the document's print permission.
    crate::engine::security::ensure_doc_permitted(
        st,
        doc_id,
        crate::engine::security::Perm::Print,
    )?;
    let subset = match pages {
        Some(list) if !list.is_empty() => {
            Some(crate::engine::pages::subset_bytes(st, doc_id, list)?)
        }
        _ => None,
    };
    let bytes = match subset {
        // A page subset has to be flattened in its own scratch document, because the temp file
        // is what gets printed and the selection has already been applied.
        Some(subset) => flatten_detached(st, subset, st.doc(doc_id)?.password.clone())?,
        None => flatten_bytes(st, doc_id, true, true, None)?,
    };
    let stem = st
        .doc(doc_id)?
        .path
        .as_ref()
        .and_then(|p| p.file_stem())
        .map(|s| sanitize(&s.to_string_lossy()))
        .unwrap_or_else(|| "document".to_string());
    let dir = std::env::temp_dir().join("seepdf-print");
    std::fs::create_dir_all(&dir).map_err(EngineError::from)?;
    // The doc id is in the name so printing two documents with the same file stem does not make
    // the second one overwrite the first while the OS handler still has it open.
    let path = dir.join(format!("{stem}-{doc_id}-{}.pdf", std::process::id()));
    crate::engine::pages::write_atomic(&path, &bytes)?;
    Ok(path)
}

/// Flattens bytes that are not an open document (the page-subset print path).
fn flatten_detached(
    st: &EngineState<'_>,
    bytes: Vec<u8>,
    password: Option<String>,
) -> Result<Vec<u8>, EngineError> {
    let bindings = raw::bindings(st.pdfium);
    let scratch = st
        .pdfium
        .load_pdf_from_byte_vec(bytes, password.as_deref())
        .map_err(|e| EngineError::pdfium("reopen for print", e))?;
    let _form = scratch.form();
    let total = raw::page::page_count(bindings, &scratch);
    for index in 0..total {
        let page = scratch
            .pages()
            .get(index as i32)
            .ctx(&format!("load page {index}"))?;
        raw::page::flatten(bindings, &page, raw::page::FlattenMode::NormalDisplay)?;
        drop(page);
    }
    raw::save::save_as_copy(bindings, &scratch, raw::save::SaveFlags::NoIncremental)
}

// ---------------------------------------------------------------------------------------
// Shared
// ---------------------------------------------------------------------------------------

/// An empty list means "every page"; anything else is validated and deduplicated.
pub fn check_pages(
    st: &EngineState<'_>,
    doc_id: &str,
    pages: &[PageIndex],
) -> Result<Vec<PageIndex>, EngineError> {
    let count = st.doc(doc_id)?.page_count();
    if pages.is_empty() {
        return Ok((0..count).collect());
    }
    let mut seen: std::collections::BTreeSet<PageIndex> = std::collections::BTreeSet::new();
    for &p in pages {
        if p >= count {
            return Err(
                EngineError::invalid(format!("page {p} is outside 0..{count}")).with_page(p),
            );
        }
        seen.insert(p);
    }
    Ok(seen.into_iter().collect())
}

/// The pixel size an export at `dpi` will produce, without rendering anything.
pub fn pixel_size(
    st: &EngineState<'_>,
    doc_id: &str,
    page: PageIndex,
    dpi: u32,
) -> Result<(u32, u32), EngineError> {
    let geom = st.doc(doc_id)?.geom(page)?.clone();
    Ok(geometry::page_pixels(
        &geom,
        0,
        check_dpi(dpi)? as f32 / 72.0,
    ))
}

/// The unrotated crop box, for callers that need the source rectangle in points.
pub fn crop(st: &EngineState<'_>, doc_id: &str, page: PageIndex) -> Result<Rect, EngineError> {
    Ok(st.doc(doc_id)?.geom(page)?.crop)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dpi_bounds() {
        assert!(check_dpi(72).is_ok());
        assert!(check_dpi(35).is_err());
        assert!(check_dpi(2400).is_err());
    }

    #[test]
    fn file_names_are_safe() {
        // Hangul is alphanumeric, so a Korean base name survives intact — only path
        // separators and shell metacharacters are replaced.
        assert_eq!(sanitize("보고서/2026"), "보고서_2026");
        assert_eq!(sanitize("a:b*c?"), "a_b_c_");
        assert_eq!(sanitize("  "), "page");
        assert_eq!(sanitize("scan 01"), "scan 01");
    }
}
