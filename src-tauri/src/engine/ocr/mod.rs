//! The invisible OCR text layer — `IPC_CONTRACT.md` §7.9, `ARCHITECTURE.md` §6.6, ocr spike §5.
//!
//! The recognition itself happens in the frontend's tesseract.js workers (Stage 1 (f)); this
//! module is the other half: turning `OcrPage` word boxes — **image pixels, origin top-left** —
//! into invisible text objects in PDF user space, so the scan becomes selectable and
//! searchable without a single visible pixel changing.
//!
//! Per word:
//!
//! ```text
//! PdfPageTextObject::new(&doc, word, font, size)   // size from the line's row height
//!   .set_render_mode(Invisible)                    // "3 Tr"
//!   natural_w = bounds()                           // works before the object is added
//!   .scale(clamp(box_w_pt / natural_w, 0.5, 2.0), 1.0)    // BEFORE translate
//!   .translate(x_pt, baseline_pt)
//! add_text_object
//! ```
//!
//! and one `regenerate_content()` per page: 730 words take 2 ms that way and 81 ms under the
//! default `AutomaticOnEveryChange` strategy (ocr spike gotcha 12).
//!
//! **Rotation.** PDFium's renderer applies `/Rotate`, so the OCR boxes are in *display* pixel
//! space while page objects live in unrotated user space. `page.pixels_to_points` (`FPDF_Device
//! ToPage`) is the conversion that knows about `/Rotate`, and the text object is rotated by the
//! page rotation so it reads upright (ocr spike gotcha 16). `ocr_layer_rotation_roundtrip`
//! pins it on `fixtures/rotation.pdf`, which the spike never measured.
//!
//! **Fonts.** Latin-1-only words go through `helvetica()` and embed nothing; everything else
//! uses the bundled Hangul subset, loaded **once per document** — `load_true_type_from_bytes`
//! appends another copy of the whole file on every call.

pub mod vision;

use crate::engine::annot::ScratchPage;
use crate::engine::fonts;
use crate::engine::registry::{self, MutateOpts, OpenDoc};
use crate::engine::text::layer;
use crate::engine::types::EngineState;
use crate::ipc::error::PdfiumResultExt;
use crate::engine::render::cache::{Night, RenderKind, TileKey};
use crate::engine::render::tiles::{self, RenderRequest};
use crate::ipc::types::{
    ChangeReason, DocInfo, OcrCapabilities, OcrEngine, OcrLine, OcrPage, OcrPageStatus, PageIndex,
    Rotation,
};
use crate::ipc::{EngineError, ErrorCode};
use pdfium_render::prelude::*;

/// A page with fewer extractable characters than this is treated as image-only.
///
/// Scanned pages are not always at zero: a header stamp, a page number or a stray Form XObject
/// leaves a handful of characters behind. 16 is low enough that a real text page never falls
/// under it and high enough that a stamped scan does.
pub const TEXT_THRESHOLD: u32 = 16;

/// Words below this confidence are noise; the spike dropped 4 of 734 on a clean page.
const MIN_CONFIDENCE: f32 = 30.0;

/// Horizontal-scale clamp, so one mis-segmented word cannot produce an absurd run.
const MIN_SX: f32 = 0.5;
const MAX_SX: f32 = 2.0;

/// For Hangul the word box height *is* the em; Latin word boxes are ~0.87 em, so a size taken
/// from a Latin box alone needs this correction (ocr spike §5).
const LATIN_BOX_TO_EM: f32 = 1.15;

// ---------------------------------------------------------------------------------------
// Capabilities and status
// ---------------------------------------------------------------------------------------

/// `ocr_capabilities` — which engines this build can drive, and in which languages.
///
/// The engine list is what the **backend** knows about. tesseract.js is always available (it is
/// bundled in `public/ocr/`, 8.4 MB, and runs entirely offline). `vision` (P1-11) is added on a
/// Mac whose Vision reads Korean — macOS 13 or later, checked against Vision's own language
/// list, once per process. `windows` (P2) has no `ocr_recognize_native` body and is never listed.
pub fn capabilities() -> OcrCapabilities {
    let mut engines = vec![OcrEngine::Tesseract];
    if vision_available() {
        engines.push(OcrEngine::Vision);
    }
    OcrCapabilities {
        engines,
        // The traineddata `scripts/prepare-ocr.mjs` fetches. Always `kor+eng` together:
        // `kor` alone reads English as digits (ARCHITECTURE §9).
        languages: vec!["kor".to_string(), "eng".to_string()],
    }
}

/// `true` when `ocr_recognize_native` can run here (macOS 13+ with Korean in Vision).
pub fn vision_available() -> bool {
    #[cfg(target_os = "macos")]
    {
        vision::mac::available()
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

// ---------------------------------------------------------------------------------------
// Native recognition (P1-11, macOS Vision)
// ---------------------------------------------------------------------------------------

/// One page rendered for a native recogniser: 8-bit gray, one byte a pixel, row-major.
pub struct GrayPage {
    pub page: PageIndex,
    pub dpi: u32,
    pub pixels: Vec<u8>,
    pub width: u32,
    pub height: u32,
    /// The page's `/Rotate`, which the render applied (the boxes are display pixels).
    pub rotation: Rotation,
    pub render_ms: f64,
}

/// **Engine thread.** The `/ocr` route's image — same key, same grayscale render, so a Vision
/// page and a tesseract page of the same document are pixel-identical inputs and `ocr_apply`'s
/// `pixels_to_points` inverts the same transform for both.
pub fn render_page_gray(
    st: &mut EngineState<'_>,
    doc_id: &str,
    page: PageIndex,
    dpi: u32,
) -> Result<GrayPage, EngineError> {
    if !(72..=1200).contains(&dpi) {
        return Err(EngineError::invalid(format!("dpi {dpi} is outside 72..1200")));
    }
    let doc = st.doc(doc_id)?;
    let count = doc.page_count();
    if page >= count {
        return Err(
            EngineError::invalid(format!("page {page} is outside 0..{count}")).with_page(page)
        );
    }
    let generation = doc.generation;
    let rotation = doc.geom(page)?.rotation;
    let key = TileKey {
        doc: doc_id.to_string(),
        generation,
        page,
        kind: RenderKind::Ocr,
        scale_key: dpi,
        rotation: 0,
        tx: 0,
        ty: 0,
        night: Night::Off,
        hl: false,
        forms: true,
    };
    let raw = tiles::render(st, &RenderRequest::new(key))?;
    let expected = raw.width as usize * raw.height as usize;
    // pdfium rendered with `use_grayscale_rendering`, so R == G == B: one byte a pixel.
    let pixels: Vec<u8> = raw.pixels.chunks_exact(4).take(expected).map(|px| px[0]).collect();
    if pixels.len() != expected {
        return Err(EngineError::new(
            ErrorCode::Pdfium,
            format!("OCR render returned {} of {expected} pixels", pixels.len()),
        ));
    }
    Ok(GrayPage {
        page,
        dpi,
        pixels,
        width: raw.width,
        height: raw.height,
        rotation,
        render_ms: raw.render_ms,
    })
}

/// **Any thread but the engine's** (it blocks for 0.1–3 s). Vision over a rendered page →
/// the contract's `OcrPage`. `languages` may be tesseract codes or Vision's own
/// ([`vision::vision_languages`]); other OSes get `unsupported`.
pub fn recognize_gray(image: &GrayPage, languages: &[String]) -> Result<OcrPage, EngineError> {
    #[cfg(target_os = "macos")]
    {
        let languages = vision::vision_languages(languages);
        let observations =
            vision::mac::recognize(&image.pixels, image.width, image.height, &languages)
                .map_err(|e| e.with_page(image.page))?;
        Ok(vision::normalize(
            &observations,
            &vision::VisionContext {
                page: image.page,
                dpi: image.dpi,
                width_px: image.width,
                height_px: image.height,
                rotation: image.rotation,
                min_confidence: vision::MIN_CONFIDENCE,
            },
        ))
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (image, languages);
        Err(EngineError::unsupported("ocr_recognize_native"))
    }
}

/// `ocr_page_status` — does this page already have extractable text?
///
/// Counts real characters only: PDFium inserts generated whitespace (`\r\n` between runs,
/// ~15 % of a LaTeX page) that would make an empty scan look like it has text.
pub fn page_status(
    doc: &mut OpenDoc<'_>,
    pages: &[PageIndex],
) -> Result<Vec<OcrPageStatus>, EngineError> {
    let count = doc.page_count();
    let wanted: Vec<PageIndex> = if pages.is_empty() {
        (0..count).collect()
    } else {
        pages.to_vec()
    };
    let mut out = Vec::with_capacity(wanted.len());
    for page in wanted {
        if page >= count {
            return Err(
                EngineError::invalid(format!("page {page} is outside 0..{count}")).with_page(page)
            );
        }
        let text = layer::page_text(doc, page)?;
        let char_count = text
            .text
            .chars()
            .filter(|c| !c.is_whitespace() && !c.is_control())
            .count() as u32;
        out.push(OcrPageStatus {
            page,
            has_text: char_count >= TEXT_THRESHOLD,
            char_count,
        });
    }
    Ok(out)
}

// ---------------------------------------------------------------------------------------
// Apply
// ---------------------------------------------------------------------------------------

/// `ocr_apply` — **one** `registry::mutate` for the whole batch, so a 14-page OCR run is one
/// undo step (contract §7.9).
///
/// `replace_existing` deletes the invisible text objects a previous run left behind before
/// adding the new ones; without it a re-run would double every word and break search ranking.
pub fn apply(
    st: &mut EngineState<'_>,
    doc_id: &str,
    pages: &[OcrPage],
    replace_existing: bool,
    progress: &mut dyn FnMut(usize, PageIndex),
) -> Result<DocInfo, EngineError> {
    if pages.is_empty() {
        return Err(EngineError::invalid("ocr_apply was given no page"));
    }
    let count = st.doc(doc_id)?.page_count();
    for p in pages {
        if p.page >= count {
            return Err(
                EngineError::invalid(format!("page {} is outside 0..{count}", p.page))
                    .with_page(p.page),
            );
        }
        if p.dpi == 0 || p.width_px == 0 || p.height_px == 0 {
            return Err(EngineError::invalid(format!(
                "page {} has no usable image geometry ({} DPI, {}x{} px)",
                p.page, p.dpi, p.width_px, p.height_px
            ))
            .with_page(p.page));
        }
    }
    let touched: Vec<PageIndex> = pages.iter().map(|p| p.page).collect();
    {
        // The same page twice in one batch would add its words twice, and `replaceExisting`
        // would not save it (the second pass removes the layer the first one just added).
        let mut seen = touched.clone();
        seen.sort_unstable();
        let unique = seen.len();
        seen.dedup();
        if seen.len() != unique {
            return Err(EngineError::invalid(
                "ocr_apply was given the same page twice",
            ));
        }
    }
    registry::mutate(
        st,
        doc_id,
        MutateOpts::new("undo.ocrApply", ChangeReason::Ocr).pages(touched),
        |doc| {
            // One font object for the whole **document**: `load_true_type_from_bytes` embeds
            // another copy of the ~0.5 MB file every time it is called, so the token is cached
            // on `OpenDoc` and survives across `ocr_apply` calls — which is what makes (f)'s
            // per-page chunking (one `ocr_apply` per page) affordable.
            let mut hangul: Option<PdfFontToken> = None;
            let needs_hangul = pages
                .iter()
                .flat_map(|p| p.lines.iter())
                .flat_map(|l| l.words.iter())
                .any(|w| !fonts::is_latin1(&w.text));
            if needs_hangul {
                hangul = Some(doc.hangul_token()?);
            }
            let helvetica = doc.pdf_mut().fonts_mut().helvetica();
            for (done, page) in pages.iter().enumerate() {
                apply_page(doc, page, replace_existing, helvetica, hangul)?;
                progress(done + 1, page.page);
            }
            Ok(())
        },
    )?;
    Ok(st.doc(doc_id)?.info())
}

fn apply_page(
    doc: &mut OpenDoc<'_>,
    ocr: &OcrPage,
    replace_existing: bool,
    helvetica: PdfFontToken,
    hangul: Option<PdfFontToken>,
) -> Result<(), EngineError> {
    let page_rotation = doc.geom(ocr.page)?.rotation;
    if ocr.rotation % 360 != page_rotation {
        // `OcrPage.rotation` is informational: the `/ocr` route takes no rotation parameter, so
        // the worker always sees the page at its own `/Rotate`, which is what
        // `FPDF_DeviceToPage` undoes below. A mismatch means the caller rendered the image some
        // other way and the boxes may land wrong.
        tracing::warn!(
            page = ocr.page,
            reported = ocr.rotation,
            actual = page_rotation,
            "OcrPage.rotation disagrees with the page's /Rotate; using the page's"
        );
    }
    let mut scratch = ScratchPage::open(doc, ocr.page)?;
    let document = doc.pdf();

    if replace_existing {
        remove_layer(&mut scratch.page)?;
    }

    // The same render configuration the `/ocr` protocol route used, so `pixels_to_points`
    // inverts exactly the transform the worker's pixels came from.
    let config = image_config(ocr.width_px, ocr.height_px);

    let mut added = 0usize;
    for line in &ocr.lines {
        let size_pt = line_font_size(line, ocr.dpi);
        for word in &line.words {
            if word.confidence < MIN_CONFIDENCE || word.text.trim().is_empty() {
                continue;
            }
            let [x0, y0, x1, y1] = word.bbox;
            if !(x0.is_finite() && y0.is_finite() && x1.is_finite() && y1.is_finite()) {
                continue;
            }
            let (left, right) = (x0.min(x1), x0.max(x1));
            let (top, bottom) = (y0.min(y1), y0.max(y1));
            if right - left < 0.5 || bottom - top < 0.5 {
                continue;
            }
            // Both bottom corners of the box, in user space. `pixels_to_points` wraps
            // `FPDF_DeviceToPage`, which applies the page's `/Rotate` for us.
            let (bl_x, bl_y) = to_points(&scratch.page, &config, left, bottom)?;
            let (br_x, br_y) = to_points(&scratch.page, &config, right, bottom)?;
            let box_w = ((br_x - bl_x).powi(2) + (br_y - bl_y).powi(2)).sqrt();
            if box_w <= 0.0 {
                continue;
            }

            let latin = fonts::is_latin1(&word.text);
            let font = if latin {
                helvetica
            } else {
                match hangul {
                    Some(token) => token,
                    // A Hangul word with no bundled font: skipping it silently would produce a
                    // layer that looks complete and is not.
                    None => {
                        return Err(EngineError::new(
                            ErrorCode::FontCoverage,
                            "the OCR result contains non-Latin text but the bundled Hangul font \
                             is not available",
                        )
                        .with_page(ocr.page))
                    }
                }
            };
            let mut object =
                PdfPageTextObject::new(document, &word.text, font, PdfPoints::new(size_pt))
                    .ctx("create OCR text object")?;
            object
                .set_render_mode(PdfPageTextRenderMode::Invisible)
                .ctx("set render mode 3")?;
            let natural = object.bounds().ctx("measure OCR word")?.to_rect();
            let natural_w = natural.width().value;
            let sx = if natural_w > 0.0 {
                (box_w / natural_w).clamp(MIN_SX, MAX_SX)
            } else {
                1.0
            };
            // Order matters: scale post-multiplies in page space, so it must happen before the
            // translate that positions the run (ocr spike gotcha 11).
            object.scale(sx, 1.0).ctx("fit OCR word to its box")?;
            if page_rotation % 360 != 0 {
                object
                    .rotate_counter_clockwise_degrees(page_rotation as f32)
                    .ctx("rotate OCR word")?;
            }
            object
                .translate(PdfPoints::new(bl_x), PdfPoints::new(bl_y))
                .ctx("place OCR word")?;
            scratch
                .page
                .objects_mut()
                .add_text_object(object)
                .ctx("add OCR word")?;
            added += 1;
        }
    }
    if added > 0 || replace_existing {
        scratch
            .page
            .regenerate_content()
            .ctx("regenerate page content")?;
    }
    Ok(())
}

/// The inverse of [`to_points`]: a user-space rectangle back into the image pixel box it came
/// from, `[x0, y0, x1, y1]` with the origin top-left.
///
/// `FPDF_PageToDevice` applies `/Rotate` exactly as `FPDF_DeviceToPage` undoes it, so this is
/// the round trip that proves the rotated-page mapping (ocr spike gotcha 16). The OCR review
/// overlay uses it to draw a saved layer back onto the page image.
pub fn points_to_image_px(
    doc: &mut OpenDoc<'_>,
    page_index: PageIndex,
    width_px: u32,
    height_px: u32,
    rect: crate::ipc::types::Rect,
) -> Result<[f32; 4], EngineError> {
    let config = image_config(width_px, height_px);
    let page = doc.page(page_index)?;
    let mut xs = [0f32; 2];
    let mut ys = [0f32; 2];
    for (i, (x, y)) in [(rect.l, rect.b), (rect.r, rect.t)].into_iter().enumerate() {
        let (px, py) = page
            .points_to_pixels(PdfPoints::new(x), PdfPoints::new(y), &config)
            .ctx("map points to OCR pixels")?;
        xs[i] = px as f32;
        ys[i] = py as f32;
    }
    Ok([
        xs[0].min(xs[1]),
        ys[0].min(ys[1]),
        xs[0].max(xs[1]),
        ys[0].max(ys[1]),
    ])
}

/// The render configuration the `/ocr` route produced this image with: no view rotation, the
/// image's own pixel size as the device box.
fn image_config(width_px: u32, height_px: u32) -> PdfRenderConfig {
    PdfRenderConfig::new().set_target_size(
        width_px.min(i32::MAX as u32) as Pixels,
        height_px.min(i32::MAX as u32) as Pixels,
    )
}

/// `FPDF_DeviceToPage` on the same configuration the page image was rendered with.
fn to_points(
    page: &PdfPage<'_>,
    config: &PdfRenderConfig,
    x: f32,
    y: f32,
) -> Result<(f32, f32), EngineError> {
    let (px, py) = page
        .pixels_to_points(x.round() as Pixels, y.round() as Pixels, config)
        .ctx("map OCR pixels to points")?;
    Ok((px.value, py.value))
}

/// The font size for a line, in points.
///
/// `rowHeightPx` (Tesseract's `rowAttributes.rowHeight`, ascenders plus descenders) is the
/// right measure when the engine gives it. Falling back to the line box height needs the
/// Latin correction, because a mixed-case Latin box is about 0.87 em while a Hangul box is the
/// full em.
fn line_font_size(line: &OcrLine, dpi: u32) -> f32 {
    let px_to_pt = 72.0 / dpi as f32;
    if let Some(row) = line.row_height_px.filter(|h| *h > 0.0) {
        return (row * px_to_pt).clamp(1.0, 400.0);
    }
    let [_, y0, _, y1] = line.bbox;
    let height = (y1 - y0).abs().max(1.0);
    let hangul = !fonts::is_latin1(&line.text);
    let factor = if hangul { 1.0 } else { LATIN_BOX_TO_EM };
    (height * px_to_pt * factor).clamp(1.0, 400.0)
}

/// Removes every invisible text object from the page — the previous OCR layer.
///
/// Descending, because `remove_object_at_index` shifts every index after it.
fn remove_layer(page: &mut PdfPage<'_>) -> Result<usize, EngineError> {
    let mut victims: Vec<usize> = Vec::new();
    for (index, object) in page.objects().iter().enumerate() {
        if let Some(t) = object.as_text_object() {
            if matches!(
                t.render_mode(),
                PdfPageTextRenderMode::Invisible | PdfPageTextRenderMode::InvisibleClipping
            ) {
                victims.push(index);
            }
        }
    }
    for &index in victims.iter().rev() {
        page.objects_mut()
            .remove_object_at_index(index)
            .ctx(&format!("remove OCR object {index}"))?;
    }
    Ok(victims.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::types::OcrWord;

    fn line(text: &str, bbox: [f32; 4], row: Option<f32>) -> OcrLine {
        OcrLine {
            text: text.to_string(),
            bbox,
            baseline: None,
            row_height_px: row,
            words: vec![OcrWord {
                text: text.to_string(),
                bbox,
                confidence: 90.0,
            }],
        }
    }

    #[test]
    fn font_size_prefers_row_height() {
        // 60 px of row height at 300 DPI = 14.4 pt.
        let l = line("검색", [0.0, 0.0, 100.0, 60.0], Some(60.0));
        assert!((line_font_size(&l, 300) - 14.4).abs() < 0.01);
    }

    #[test]
    fn font_size_falls_back_to_the_box_with_a_latin_correction() {
        let hangul = line("한글", [0.0, 0.0, 100.0, 60.0], None);
        let latin = line("Hangul", [0.0, 0.0, 100.0, 60.0], None);
        assert!((line_font_size(&hangul, 300) - 14.4).abs() < 0.01);
        assert!((line_font_size(&latin, 300) - 14.4 * LATIN_BOX_TO_EM).abs() < 0.01);
    }

    #[test]
    fn capabilities_are_offline_tesseract_plus_vision_where_it_reads_korean() {
        let caps = capabilities();
        assert_eq!(caps.engines[0], OcrEngine::Tesseract);
        assert_eq!(caps.engines.contains(&OcrEngine::Vision), vision_available());
        assert!(!caps.engines.contains(&OcrEngine::Windows));
        #[cfg(not(target_os = "macos"))]
        assert!(!vision_available());
        assert!(caps.languages.iter().any(|l| l == "kor"));
    }
}
