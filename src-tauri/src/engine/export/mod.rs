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
pub mod nup;
pub mod pagejob;
pub mod summary;
pub mod textflow;

pub use crate::engine::render::cache::PrintAnnots;

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
///
/// v0.3 (X7): flattened with `FLAT_PRINT`, so what reaches paper is what the annotations' `/F`
/// flags say prints (a NoView + Print watermark is baked in, a screen-only comment is not),
/// filtered by `annots` (인쇄 ▸ 주석). X8: the file lives in [`print_temp_dir`], which
/// [`cleanup_print_temp`] empties at startup and exit; the command also deletes each file
/// [`PRINT_TEMP_MAX_AGE`] after it was written.
pub fn print_prepare(
    st: &mut EngineState<'_>,
    doc_id: &str,
    pages: Option<&[PageIndex]>,
) -> Result<PathBuf, EngineError> {
    print_prepare_with(st, doc_id, pages, PrintAnnots::All)
}

/// [`print_prepare`] with the 주석 option.
pub fn print_prepare_with(
    st: &mut EngineState<'_>,
    doc_id: &str,
    pages: Option<&[PageIndex]>,
    annots: PrintAnnots,
) -> Result<PathBuf, EngineError> {
    let bytes = print_bytes(st, doc_id, pages, annots)?;
    write_print_temp(st, doc_id, "", &bytes)
}

/// Writes `bytes` as `<stem>-<docId><suffix>-<pid>-<n>.pdf` in [`print_temp_dir`], after
/// sweeping files older than [`PRINT_TEMP_MAX_AGE`].
pub fn write_print_temp(
    st: &EngineState<'_>,
    doc_id: &str,
    suffix: &str,
    bytes: &[u8],
) -> Result<PathBuf, EngineError> {
    static SEQ: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let stem = st
        .doc(doc_id)?
        .path
        .as_ref()
        .and_then(|p| p.file_stem())
        .map(|s| sanitize(&s.to_string_lossy()))
        .unwrap_or_else(|| "document".to_string());
    let dir = print_temp_dir();
    cleanup_print_dir(&dir, PRINT_TEMP_MAX_AGE);
    std::fs::create_dir_all(&dir).map_err(EngineError::from)?;
    // The doc id and a sequence number are in the name so two prints never overwrite a file
    // the OS handler may still have open.
    let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let path = dir.join(format!(
        "{stem}-{doc_id}{suffix}-{}-{n}.pdf",
        std::process::id()
    ));
    crate::engine::pages::write_atomic(&path, bytes)?;
    Ok(path)
}

/// The bytes the handler path prints: the selected pages (all when `None` or empty) of the
/// current document, flattened with `FLAT_PRINT` after dropping the markup `annots` excludes.
pub fn print_bytes(
    st: &mut EngineState<'_>,
    doc_id: &str,
    pages: Option<&[PageIndex]>,
    annots: PrintAnnots,
) -> Result<Vec<u8>, EngineError> {
    // v0.3 pkg3 (S5): the document's print permission. Every paper path comes through here:
    // `print_prepare` and `make_nup` (whose sheets are built from these bytes).
    crate::engine::security::ensure_doc_permitted(
        st,
        doc_id,
        crate::engine::security::Perm::Print,
    )?;
    let bytes = match pages {
        Some(list) if !list.is_empty() => crate::engine::pages::subset_bytes(st, doc_id, list)?,
        _ => save::serialize(st, doc_id)?,
    };
    flatten_for_print(st, bytes, st.doc(doc_id)?.password.clone(), annots)
}

/// Flattens bytes that are not an open document for paper: `FLAT_PRINT` (only annotations
/// with the Print flag are baked, the others are dropped) after removing the annotations the
/// 주석 option leaves out. Form widgets are always kept: a field value is document content.
fn flatten_for_print(
    st: &EngineState<'_>,
    bytes: Vec<u8>,
    password: Option<String>,
    annots: PrintAnnots,
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
        if annots != PrintAnnots::All {
            for i in (0..raw::annot::count(bindings, &page)).rev() {
                let keep = match raw::annot::slot(bindings, &page, i) {
                    None => continue,
                    Some(a) => {
                        let subtype = a.subtype();
                        subtype == raw::consts::FPDF_ANNOT_WIDGET
                            || (annots == PrintAnnots::Stamps
                                && crate::engine::render::tiles::is_stamp_or_signature(
                                    subtype,
                                    a.string("Subj").as_deref(),
                                ))
                    }
                };
                if !keep {
                    raw::annot::remove(bindings, &page, i)?;
                }
            }
        }
        // Print, not NormalDisplay: what reaches paper is what the `/F` flags say prints.
        raw::page::flatten(bindings, &page, raw::page::FlattenMode::Print)?;
        drop(page);
    }
    raw::save::save_as_copy(bindings, &scratch, raw::save::SaveFlags::NoIncremental)
}

// ---------------------------------------------------------------------------------------
// v0.3 pkg8 (X8): print temp files
// ---------------------------------------------------------------------------------------

/// How long a print temp file may live: long enough for any OS handler to have read it.
pub const PRINT_TEMP_MAX_AGE: std::time::Duration = std::time::Duration::from_secs(10 * 60);

/// `%TEMP%/seepdf-print` (`$TMPDIR/seepdf-print` on macOS): flattened — and, for an encrypted
/// source printed n-up, decrypted — copies. Nothing else is ever written there.
pub fn print_temp_dir() -> PathBuf {
    std::env::temp_dir().join("seepdf-print")
}

/// Deletes every print temp file. Called at startup and at exit (lib.rs); a file the OS
/// handler still holds open on Windows is skipped and caught by the next sweep. Returns the
/// number of files removed.
pub fn cleanup_print_temp() -> usize {
    cleanup_print_dir(&print_temp_dir(), std::time::Duration::ZERO)
}

/// Deletes the regular files in `dir` last modified at least `older_than` ago.
pub fn cleanup_print_dir(dir: &Path, older_than: std::time::Duration) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    let now = std::time::SystemTime::now();
    let mut removed = 0;
    for entry in entries.flatten() {
        let Ok(meta) = entry.metadata() else { continue };
        if !meta.is_file() {
            continue;
        }
        let age = meta
            .modified()
            .ok()
            .and_then(|m| now.duration_since(m).ok())
            .unwrap_or_default();
        if age >= older_than && std::fs::remove_file(entry.path()).is_ok() {
            removed += 1;
        }
    }
    removed
}

/// Deletes `path` after `delay` on a detached thread (no pdfium involved). The OS handler
/// opens the file asynchronously after `openPath` returns, so it cannot be deleted on return.
pub fn schedule_print_temp_removal(path: PathBuf, delay: std::time::Duration) {
    let _ = std::thread::Builder::new()
        .name("seepdf-print-temp".into())
        .spawn(move || {
            std::thread::sleep(delay);
            let _ = std::fs::remove_file(&path);
        });
}

// ---------------------------------------------------------------------------------------
// v0.3 pkg8 (X3): embedded images, one stitched image, layout text, multi-page TIFF
// ---------------------------------------------------------------------------------------

/// A stitched image never exceeds this many pixels (≈ 190 MB of RGB while it is built).
pub const STITCH_MAX_PX: u64 = 64_000_000;
/// …nor this many on a side (JPEG's own limit is 65 535).
pub const STITCH_MAX_SIDE: u32 = 65_000;

/// `export_stitched_image`'s geometry: the DPI actually used (lowered from the requested one
/// when the result would exceed [`STITCH_MAX_PX`] / [`STITCH_MAX_SIDE`]) and the canvas size.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StitchPlan {
    pub dpi: u32,
    pub width: u32,
    pub height: u32,
    /// The DPI had to be lowered to stay under the cap.
    pub lowered: bool,
}

/// Plans a vertical stitch of `pages` at `dpi`: the canvas is as wide as the widest page and as
/// tall as the sum of the page heights (each page's pixel size is [`pixel_size`]'s).
pub fn stitch_plan(
    st: &EngineState<'_>,
    doc_id: &str,
    pages: &[PageIndex],
    dpi: u32,
) -> Result<StitchPlan, EngineError> {
    let dpi = check_dpi(dpi)?;
    let size = |dpi: u32| -> Result<(u32, u32), EngineError> {
        let (mut w, mut h) = (0u32, 0u32);
        for &p in pages {
            let (pw, ph) = pixel_size(st, doc_id, p, dpi)?;
            w = w.max(pw);
            h = h.saturating_add(ph);
        }
        Ok((w, h))
    };
    let (w, h) = size(dpi)?;
    let fits = |w: u32, h: u32| {
        (w as u64) * (h as u64) <= STITCH_MAX_PX && w <= STITCH_MAX_SIDE && h <= STITCH_MAX_SIDE
    };
    if fits(w, h) {
        return Ok(StitchPlan {
            dpi,
            width: w,
            height: h,
            lowered: false,
        });
    }
    let area = (STITCH_MAX_PX as f64 / (w as f64 * h as f64)).sqrt();
    let side = (STITCH_MAX_SIDE as f64 / w.max(h) as f64).min(1.0);
    let mut lowered = ((dpi as f64) * area.min(side)).floor() as u32;
    // Rounding can leave it one step over; walk down until it fits.
    while lowered >= MIN_DPI {
        let (w, h) = size(lowered)?;
        if fits(w, h) {
            return Ok(StitchPlan {
                dpi: lowered,
                width: w,
                height: h,
                lowered: true,
            });
        }
        lowered -= 1;
    }
    Err(EngineError::invalid(format!(
        "{} pages are too long for one image even at {MIN_DPI} DPI; choose fewer pages",
        pages.len()
    )))
}

/// One page as RGB on white at `dpi` (the stitch and TIFF source).
pub fn render_rgb(
    st: &mut EngineState<'_>,
    doc_id: &str,
    page: PageIndex,
    dpi: u32,
) -> Result<image::RgbImage, EngineError> {
    Ok(render_page(st, doc_id, page, dpi, false)?.to_rgb8())
}

/// Pastes `page` into the stitch canvas at `y`, centred horizontally; returns the next `y`.
pub fn stitch_page(canvas: &mut image::RgbImage, page: &image::RgbImage, y: u32) -> u32 {
    let x = (canvas.width().saturating_sub(page.width())) / 2;
    image::imageops::replace(canvas, page, x as i64, y as i64);
    y + page.height()
}

/// Encodes a finished stitch as PNG or JPEG (quality 85) and writes it atomically.
pub fn write_image_file(
    image: image::RgbImage,
    format: ImageFormat,
    out_path: &Path,
) -> Result<u64, EngineError> {
    let bytes = encode(&image::DynamicImage::ImageRgb8(image), format, None)?;
    crate::engine::pages::write_atomic(out_path, &bytes)
}

/// `export_embedded_images`, one page: every image object as the page shows it (soft mask,
/// colour conversion applied), written as `<base>-p<page>-<n>.png` (1-based page and n).
pub fn export_embedded_page(
    st: &mut EngineState<'_>,
    doc_id: &str,
    page: PageIndex,
    out_dir: &Path,
    base_name: &str,
) -> Result<Vec<PathBuf>, EngineError> {
    // v0.3 integration (pkg8 X3 × pkg3 S5): pulling the embedded images out is "extract text
    // and graphics" — the permission `export_text` and 스냅샷 need.
    crate::engine::security::ensure_doc_permitted(
        st,
        doc_id,
        crate::engine::security::Perm::ExtractText,
    )?;
    let images = crate::engine::objects::extract_images(st.doc_mut(doc_id)?, page)?;
    std::fs::create_dir_all(out_dir).map_err(EngineError::from)?;
    let mut written = Vec::with_capacity(images.len());
    for (n, image) in images.iter().enumerate() {
        let path = out_dir.join(format!(
            "{}-p{}-{}.png",
            sanitize(base_name),
            page as u32 + 1,
            n + 1
        ));
        crate::engine::pages::write_atomic(&path, &image.png)?;
        written.push(path);
    }
    Ok(written)
}

/// `export_text_flow`'s start: the copy / extract permission (v0.3 integration, pkg8 X6 ×
/// pkg3 S5 — Word / 한글 / HTML / Markdown carry the document's text and images out) and the
/// title the written document gets.
pub fn text_flow_start(st: &EngineState<'_>, doc_id: &str) -> Result<String, EngineError> {
    crate::engine::security::ensure_doc_permitted(
        st,
        doc_id,
        crate::engine::security::Perm::ExtractText,
    )?;
    Ok(st.doc(doc_id)?.name_stem())
}

/// A multi-page TIFF being written (`export_tiff`): Deflate, RGB, the DPI recorded as the
/// resolution. It is written to `<out>.part` and renamed by [`TiffPages::finish`], so a
/// cancelled export leaves nothing that looks complete.
pub struct TiffPages {
    encoder: tiff::encoder::TiffEncoder<std::io::BufWriter<std::fs::File>>,
    part: PathBuf,
    out: PathBuf,
    pub pages: u32,
}

impl TiffPages {
    pub fn create(out: &Path) -> Result<Self, EngineError> {
        if let Some(dir) = out.parent() {
            if !dir.as_os_str().is_empty() {
                std::fs::create_dir_all(dir).map_err(EngineError::from)?;
            }
        }
        let mut name = out.as_os_str().to_owned();
        name.push(".part");
        let part = PathBuf::from(name);
        let file = std::fs::File::create(&part).map_err(EngineError::from)?;
        let encoder = tiff::encoder::TiffEncoder::new(std::io::BufWriter::new(file))
            .map_err(tiff_error)?
            .with_compression(tiff::encoder::Compression::Deflate(
                tiff::encoder::DeflateLevel::Balanced,
            ));
        Ok(Self {
            encoder,
            part,
            out: out.to_path_buf(),
            pages: 0,
        })
    }

    pub fn add(&mut self, image: &image::RgbImage, dpi: u32) -> Result<(), EngineError> {
        let mut frame = self
            .encoder
            .new_image::<tiff::encoder::colortype::RGB8>(image.width(), image.height())
            .map_err(tiff_error)?;
        frame.resolution(
            tiff::tags::ResolutionUnit::Inch,
            tiff::encoder::Rational { n: dpi, d: 1 },
        );
        frame.write_data(image.as_raw()).map_err(tiff_error)?;
        self.pages += 1;
        Ok(())
    }

    /// Flushes and renames the `.part` file into place.
    pub fn finish(self) -> Result<PathBuf, EngineError> {
        use std::io::Write;
        let Self {
            encoder, part, out, ..
        } = self;
        // The encoder owns the writer; dropping it after an explicit flush of the file is
        // the only way to get it back, so flush through a fresh handle to the same file.
        drop(encoder);
        std::fs::OpenOptions::new()
            .append(true)
            .open(&part)
            .and_then(|mut f| f.flush().and_then(|_| f.sync_all()))
            .map_err(EngineError::from)?;
        std::fs::rename(&part, &out).map_err(EngineError::from)?;
        Ok(out)
    }

    /// Deletes the partial file (cancel / error).
    pub fn abandon(self) {
        let part = self.part.clone();
        drop(self);
        let _ = std::fs::remove_file(part);
    }
}

fn tiff_error(e: tiff::TiffError) -> EngineError {
    EngineError::new(ErrorCode::Io, format!("TIFF: {e}"))
}

/// `export_text` with 레이아웃 유지 (X3).
pub fn export_text_with(
    st: &mut EngineState<'_>,
    doc_id: &str,
    pages: &[PageIndex],
    out_path: &str,
    preserve_layout: bool,
) -> Result<ExportTextResult, EngineError> {
    if !preserve_layout {
        return export_text(st, doc_id, pages, out_path);
    }
    // v0.3 integration (pkg8 X3 × pkg3 S5): 레이아웃 유지 is still text extraction, so it needs
    // the copy / extract permission `export_text` checks.
    crate::engine::security::ensure_doc_permitted(
        st,
        doc_id,
        crate::engine::security::Perm::ExtractText,
    )?;
    let pages = check_pages(st, doc_id, pages)?;
    let mut out = String::new();
    for (i, &page) in pages.iter().enumerate() {
        if i > 0 {
            out.push('\u{0C}');
        }
        let layer = layer::layer(st.doc_mut(doc_id)?, page)?;
        out.push_str(&layout_text(&layer));
    }
    let chars = out.chars().count() as u64;
    crate::engine::pages::write_atomic(Path::new(out_path), out.as_bytes())?;
    Ok(ExportTextResult { chars })
}

/// Monospace columns a character takes: 2 for East Asian wide / full-width characters.
fn columns_of(c: char) -> usize {
    let u = c as u32;
    let wide = matches!(u,
        0x1100..=0x115F | 0x2E80..=0x303E | 0x3041..=0x33FF | 0x3400..=0x4DBF | 0x4E00..=0x9FFF
        | 0xA000..=0xA4CF | 0xAC00..=0xD7A3 | 0xF900..=0xFAFF | 0xFE30..=0xFE4F | 0xFF00..=0xFF60
        | 0xFFE0..=0xFFE6 | 0x20000..=0x3FFFD);
    if wide {
        2
    } else {
        1
    }
}

/// 레이아웃 유지: the page's characters bucketed into rows by baseline and placed at the
/// monospace column their x position maps to (one column = the median advance of the page's
/// narrow characters), so a table's columns stay aligned in a fixed-width font. A vertical gap
/// of more than one and a half rows becomes blank lines (at most two).
pub fn layout_text(layer: &layer::TextLayer) -> String {
    struct Glyph {
        x: f32,
        r: f32,
        y: f32,
        size: f32,
        c: char,
    }
    let glyphs: Vec<Glyph> = layer
        .chars
        .iter()
        .filter(|c| !c.is_generated())
        .filter_map(|c| {
            let ch = char::from_u32(c.codepoint)?;
            if ch.is_control() {
                return None;
            }
            Some(Glyph {
                x: c.loose.l,
                r: c.loose.r,
                y: c.baseline_y,
                size: c.font_size.max(1.0),
                c: ch,
            })
        })
        .collect();
    if glyphs.is_empty() {
        return String::new();
    }
    let median = |mut v: Vec<f32>| -> f32 {
        if v.is_empty() {
            return 0.0;
        }
        v.sort_by(|a, b| a.total_cmp(b));
        v[v.len() / 2]
    };
    let narrow: Vec<f32> = layer
        .chars
        .iter()
        .filter(|c| !c.is_generated())
        .filter(|c| {
            char::from_u32(c.codepoint)
                .map(|ch| !ch.is_whitespace() && columns_of(ch) == 1)
                .unwrap_or(false)
        })
        .map(|c| c.loose.r - c.loose.l)
        .filter(|w| *w > 0.1)
        .collect();
    let size = median(glyphs.iter().map(|g| g.size).collect());
    let unit = {
        let m = median(narrow);
        if m > 0.1 {
            m
        } else {
            size * 0.5
        }
    };
    let left = glyphs.iter().map(|g| g.x).fold(f32::MAX, f32::min);

    // Rows: glyphs sorted top-down (y-up space), a new row when the baseline moves by more
    // than a third of the font size.
    let mut order: Vec<usize> = (0..glyphs.len()).collect();
    order.sort_by(|&a, &b| glyphs[b].y.total_cmp(&glyphs[a].y));
    let mut rows: Vec<(f32, Vec<usize>)> = Vec::new();
    for i in order {
        let g = &glyphs[i];
        match rows.last_mut() {
            Some((y, members)) if (*y - g.y).abs() <= g.size / 3.0 => members.push(i),
            _ => rows.push((g.y, vec![i])),
        }
    }

    let mut out = String::new();
    let mut prev_y: Option<f32> = None;
    for (y, mut members) in rows {
        if let Some(py) = prev_y {
            let gap = (py - y) / (size * 1.2);
            let blank = (gap - 1.5).ceil().clamp(0.0, 2.0) as usize;
            for _ in 0..blank {
                out.push('\n');
            }
        }
        prev_y = Some(y);
        members.sort_by(|&a, &b| glyphs[a].x.total_cmp(&glyphs[b].x));
        let mut line = String::new();
        let mut col = 0usize;
        let mut prev_right: Option<f32> = None;
        for i in members {
            let g = &glyphs[i];
            if g.c.is_whitespace() {
                continue;
            }
            // Inside a word the glyphs follow each other; only a word's first glyph is placed
            // by its x position (at least one space after the previous word).
            let word_start = prev_right.is_none_or(|r| g.x - r > unit * 0.35);
            if word_start {
                let want = ((g.x - left) / unit).round().max(0.0) as usize;
                let target = if col == 0 { want } else { want.max(col + 1) };
                line.extend(std::iter::repeat_n(' ', target - col));
                col = target;
            }
            line.push(g.c);
            col += columns_of(g.c);
            prev_right = Some(g.r);
        }
        out.push_str(line.trim_end());
        out.push('\n');
    }
    out
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
