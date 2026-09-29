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
//! **Word spaces.** Every word but the last of its line is written with a trailing U+0020
//! (`"quick "`). PDFium's extractor otherwise only *guesses* a word break from the gap between
//! two runs — wider than about a quarter of a glyph — which tesseract's tight ink boxes (~24 px
//! apart at 300 DPI) clear and Vision's padded ones (6–13 px) do not: its layer read
//! `Thequickbrownfox`. PDFium never generates a second space next to a real one, so both engines
//! extract single spaces. The space rides on the word's own run because a run of nothing but a
//! space has a zero-width (ink) rect, and `CPDF_TextPage` skips those; the word is still fitted
//! to its box on its own, so word selection rects are unchanged and only the space's box (the
//! font's space advance at the word's scale) is approximate. Lines need no separator: the
//! baseline jump makes PDFium emit `\r\n`.
//!
//! **Rotation.** PDFium's renderer applies `/Rotate`, so the OCR boxes are in *display* pixel
//! space while page objects live in unrotated user space. `page.pixels_to_points` (`FPDF_Device
//! ToPage`) is the conversion that knows about `/Rotate`, and the text object is rotated by the
//! page rotation so it reads upright (ocr spike gotcha 16). `ocr_layer_rotation_roundtrip`
//! pins it on `fixtures/rotation.pdf`, which the spike never measured.
//!
//! **Fonts.** Latin-1-only words go through `helvetica()` and embed nothing; everything else
//! uses the glyphless CID font of [`crate::engine::fonts::glyphless`] (v0.3 O5): ~0.6 KB of font
//! plus a `/ToUnicode` of the batch's characters, loaded once per `ocr_apply`, instead of the
//! 487 KB Hangul subset — and any script (日本語, 中文) instead of KS X 1001 only.
//!
//! **Rotation fix-up** (v0.3 O2). A page sent with `setRotation` gets that `/Rotate` first, in
//! the same `mutate`; its OCR boxes are in the display pixels of the *new* rotation.

pub mod orientation;
pub mod vision;
pub mod winocr;

use crate::engine::annot::ScratchPage;
use crate::engine::fonts;
use crate::engine::fonts::glyphless::{CodeMap, GlyphlessFont};
use crate::engine::registry::{self, MutateOpts, OpenDoc};
use crate::engine::render::cache::{Night, RenderKind, TileKey};
use crate::engine::render::tiles::{self, RenderRequest};
use crate::engine::text::layer;
use crate::engine::types::EngineState;
use crate::ipc::error::PdfiumResultExt;
use crate::ipc::types::{
    ChangeReason, DocInfo, OcrCapabilities, OcrEngine, OcrEngineLanguages, OcrLine, OcrPage,
    OcrPageStatus, PageIndex, Rotation,
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

/// The OCR languages SeePDF offers, as tesseract spells them (the app-wide code): 한국어,
/// English, 日本語, 中文(简体). Chips, settings and `engineLanguages` all use these.
pub const APP_LANGUAGES: [&str; 4] = ["kor", "eng", "jpn", "chi_sim"];

/// `code` as the `'static` entry of [`APP_LANGUAGES`], if it is one.
pub fn app_language(code: &str) -> Option<&'static str> {
    APP_LANGUAGES.iter().copied().find(|c| *c == code)
}

/// `ocr_capabilities` — which engines this build can drive, and in which languages.
///
/// The engine list is what the **backend** knows about. tesseract.js is always available (it is
/// bundled in `public/ocr/`, 8.4 MB, and runs entirely offline). `vision` (P1-11) is added on a
/// Mac whose Vision reads Korean — macOS 13 or later, checked against Vision's own language
/// list, once per process. `windows` (v0.3 O3) is added where `Windows.Media.Ocr` has a
/// recogniser for one of [`APP_LANGUAGES`] (`AvailableRecognizerLanguages`).
pub fn capabilities() -> OcrCapabilities {
    let mut engines = vec![OcrEngine::Tesseract];
    let vision = vision_languages();
    if vision_available() {
        engines.push(OcrEngine::Vision);
    }
    let windows = windows_languages();
    if !windows.is_empty() {
        engines.push(OcrEngine::Windows);
    }
    // The traineddata `scripts/prepare-ocr.mjs` always fetches. Always `kor+eng` together:
    // `kor` alone reads English as digits (ARCHITECTURE §9).
    let tesseract = vec!["kor".to_string(), "eng".to_string()];
    OcrCapabilities {
        engines,
        languages: tesseract.clone(),
        engine_languages: OcrEngineLanguages {
            tesseract,
            vision,
            windows,
        },
    }
}

/// The app languages Vision reads on this Mac (`ko-KR` → `kor`, `ja-JP` → `jpn`, …); empty
/// where Vision is not offered.
pub fn vision_languages() -> Vec<String> {
    #[cfg(target_os = "macos")]
    {
        let supported = vision::mac::supported();
        APP_LANGUAGES
            .iter()
            .filter(|code| {
                let tag = &vision::vision_languages(&[code.to_string()])[0];
                supported.iter().any(|s| s == tag)
            })
            .map(|c| c.to_string())
            .collect()
    }
    #[cfg(not(target_os = "macos"))]
    {
        Vec::new()
    }
}

/// The app languages `Windows.Media.Ocr` has a recogniser for here; empty off Windows.
pub fn windows_languages() -> Vec<String> {
    #[cfg(windows)]
    {
        winocr::app_languages(winocr::win::available_languages())
    }
    #[cfg(not(windows))]
    {
        Vec::new()
    }
}

/// `true` when `ocr_recognize_native` can run here: Vision (macOS 13+ with Korean) or
/// `Windows.Media.Ocr` with at least one of the app's languages.
pub fn native_available() -> bool {
    vision_available() || !windows_languages().is_empty()
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
        return Err(EngineError::invalid(format!(
            "dpi {dpi} is outside 72..1200"
        )));
    }
    let doc = st.doc(doc_id)?;
    let count = doc.page_count();
    if page >= count {
        return Err(
            EngineError::invalid(format!("page {page} is outside 0..{count}")).with_page(page),
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
    let pixels: Vec<u8> = raw
        .pixels
        .chunks_exact(4)
        .take(expected)
        .map(|px| px[0])
        .collect();
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

/// **Any thread but the engine's** (it blocks for 0.1–3 s). The platform's native recogniser
/// over a rendered page → the contract's `OcrPage`: Vision on macOS, `Windows.Media.Ocr` on
/// Windows (v0.3 O3). `languages` may be tesseract codes or BCP 47 tags
/// ([`vision::vision_languages`], [`winocr::pick_language`]); other OSes get `unsupported`.
pub fn recognize_gray(image: &GrayPage, languages: &[String]) -> Result<OcrPage, EngineError> {
    #[cfg(windows)]
    {
        let Some(tag) = winocr::pick_language(languages, winocr::win::available_languages()) else {
            return Err(EngineError::unsupported(&format!(
                "ocr_recognize_native: no Windows recogniser for {languages:?}"
            ))
            .with_page(image.page));
        };
        // Pages larger than the engine's limit are read at an integer fraction and scaled back.
        let max = winocr::win::max_dimension();
        let factor = image.width.max(image.height).div_ceil(max).max(1);
        let (pixels, width, height) =
            winocr::downscale(&image.pixels, image.width, image.height, factor);
        let lines = winocr::win::recognize(&pixels, width, height, &tag)
            .map_err(|e| e.with_page(image.page))?;
        let observations = winocr::observations(&lines, image.width, image.height, factor as f64);
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
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        let _ = (image, languages);
        Err(EngineError::unsupported("ocr_recognize_native"))
    }
}

/// **Any thread but the engine's.** 페이지 회전 자동 감지 (v0.3 O2) with Apple Vision: `image`
/// (rendered at [`orientation::DETECT_DPI`]) is read once and every line votes with its
/// reading direction ([`vision::TextDirection`]); the turn that wins clearly
/// ([`orientation::pick`]) is the answer. Vision reads sideways text as confidently as upright
/// text, so four turned reads compared by confidence cannot tell which way is up — measured on
/// the Korean fixture: 100 % at all four turns. Elsewhere `unsupported`: the frontend detects
/// with tesseract (four turned reads, whose confidence *does* collapse on a wrong turn), which
/// is also what the Windows engine uses — it reports no confidence at all.
pub fn detect_orientation(
    image: &GrayPage,
    languages: &[String],
) -> Result<orientation::OrientationResult, EngineError> {
    #[cfg(target_os = "macos")]
    {
        let languages = vision::vision_languages(languages);
        let directions =
            vision::mac::text_directions(&image.pixels, image.width, image.height, &languages)
                .map_err(|e| e.with_page(image.page))?;
        let scores = orientation::scores_from_directions(&directions);
        Ok(orientation::OrientationResult {
            rotation: orientation::pick(&scores),
            scores,
        })
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (image, languages);
        Err(EngineError::unsupported("ocr_detect_orientation"))
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
                EngineError::invalid(format!("page {page} is outside 0..{count}")).with_page(page),
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
    apply_cancellable(st, doc_id, pages, replace_existing, progress, &|| false)
}

/// [`apply`] that polls `cancelled` before every page: the `ocr_apply` command passes its job
/// token, so `cancel_job` stops a long batch. A cancelled batch is `cancelled` and — being one
/// `mutate` — rolled back whole: no page of it stays applied, no undo step, no new generation.
pub fn apply_cancellable(
    st: &mut EngineState<'_>,
    doc_id: &str,
    pages: &[OcrPage],
    replace_existing: bool,
    progress: &mut dyn FnMut(usize, PageIndex),
    cancelled: &dyn Fn() -> bool,
) -> Result<DocInfo, EngineError> {
    let rotations = vec![None; pages.len()];
    apply_rotated_cancellable(
        st,
        doc_id,
        pages,
        &rotations,
        replace_existing,
        progress,
        cancelled,
    )
}

/// [`apply_cancellable`] with the per-page `/Rotate` fix-up of 페이지 회전 자동 감지 (v0.3 O2):
/// `set_rotation[i] = Some(r)` sets page `pages[i].page`'s `/Rotate` to `r` before its layer is
/// written, inside the same `mutate` (one undo step). `pages[i].rotation` must be `r` — the image
/// was recognised at the new rotation. `set_rotation` is as long as `pages`.
pub fn apply_rotated_cancellable(
    st: &mut EngineState<'_>,
    doc_id: &str,
    pages: &[OcrPage],
    set_rotation: &[Option<Rotation>],
    replace_existing: bool,
    progress: &mut dyn FnMut(usize, PageIndex),
    cancelled: &dyn Fn() -> bool,
) -> Result<DocInfo, EngineError> {
    if pages.is_empty() {
        return Err(EngineError::invalid("ocr_apply was given no page"));
    }
    if set_rotation.len() != pages.len() {
        return Err(EngineError::invalid(
            "ocr_apply: one rotation entry per page",
        ));
    }
    let count = st.doc(doc_id)?.page_count();
    for (p, rotate) in pages.iter().zip(set_rotation) {
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
        if let Some(r) = rotate {
            if !matches!(r, 0 | 90 | 180 | 270) || p.rotation % 360 != *r {
                return Err(EngineError::invalid(format!(
                    "page {}: setRotation {r} needs an OCR result recognised at {r}° (it says {}°)",
                    p.page, p.rotation
                ))
                .with_page(p.page));
            }
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
    // A new `/Rotate` swaps the page's width and height: the page list's geometry must be
    // re-read whole, which is what a structural mutate does.
    let turns = {
        let doc = st.doc(doc_id)?;
        pages.iter().zip(set_rotation).any(|(p, r)| {
            r.is_some_and(|r| doc.geom(p.page).map(|g| g.rotation != r).unwrap_or(true))
        })
    };
    let mut opts = MutateOpts::new("undo.ocrApply", ChangeReason::Ocr).pages(touched);
    if turns {
        opts = opts.structural();
    }
    registry::mutate(st, doc_id, opts, |doc| {
        // One glyphless font for the whole **call** (O5): its `/ToUnicode` must name every
        // character before the first word is written, so the batch's non-Latin words are
        // collected first. Latin-1 words stay in base-14 Helvetica and embed nothing.
        let non_latin: Vec<&str> = pages
            .iter()
            .flat_map(|p| p.lines.iter())
            .flat_map(|l| l.words.iter())
            .map(|w| w.text.trim())
            .filter(|t| !fonts::is_latin1(t))
            .collect();
        let glyphless = if non_latin.is_empty() {
            None
        } else {
            let map = CodeMap::new(non_latin.iter().copied())?;
            Some(GlyphlessFont::load(doc.bindings(), doc.pdf(), map)?)
        };
        let helvetica = doc.pdf_mut().fonts_mut().helvetica();
        for (done, (page, rotate)) in pages.iter().zip(set_rotation).enumerate() {
            if cancelled() {
                return Err(EngineError::cancelled(format!(
                    "ocr_apply cancelled after {done} of {} pages",
                    pages.len()
                )));
            }
            apply_page(
                doc,
                page,
                *rotate,
                replace_existing,
                helvetica,
                glyphless.as_ref(),
            )?;
            progress(done + 1, page.page);
        }
        Ok(())
    })?;
    Ok(st.doc(doc_id)?.info())
}

fn apply_page(
    doc: &mut OpenDoc<'_>,
    ocr: &OcrPage,
    set_rotation: Option<Rotation>,
    replace_existing: bool,
    helvetica: PdfFontToken,
    glyphless: Option<&GlyphlessFont>,
) -> Result<(), EngineError> {
    let mut scratch = ScratchPage::open(doc, ocr.page)?;
    let document = doc.pdf();

    // O2: the page turns first, so `pixels_to_points` below inverts the new display transform.
    if let Some(rotation) = set_rotation {
        scratch.page.set_rotation(render_rotation(rotation));
    }
    let page_rotation: Rotation = scratch
        .page
        .rotation()
        .map(|r| r.as_degrees() as Rotation)
        .unwrap_or(0)
        % 360;
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

    if replace_existing {
        remove_layer(&mut scratch.page)?;
    }

    // The same render configuration the `/ocr` protocol route used, so `pixels_to_points`
    // inverts exactly the transform the worker's pixels came from.
    let config = image_config(ocr.width_px, ocr.height_px);

    let mut added = 0usize;
    for line in &ocr.lines {
        let size_pt = line_font_size(line, ocr.dpi);
        // Pass 1: the line's usable words, each with its origin and width in user space.
        let mut placed: Vec<PlacedWord<'_>> = Vec::with_capacity(line.words.len());
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
            let bl = to_points(&scratch.page, &config, left, bottom)?;
            let br = to_points(&scratch.page, &config, right, bottom)?;
            let box_w = ((br.0 - bl.0).powi(2) + (br.1 - bl.1).powi(2)).sqrt();
            if box_w <= 0.0 {
                continue;
            }

            let text = word.text.trim();
            let font =
                if fonts::is_latin1(text) {
                    WordFont::Helvetica(helvetica)
                } else {
                    match glyphless {
                        Some(font) => WordFont::Glyphless(font),
                        // Cannot happen (the font is loaded whenever a word needs it), but a
                        // silently skipped word would produce a layer that looks complete and is not.
                        None => return Err(EngineError::new(
                            ErrorCode::FontCoverage,
                            "the OCR result contains non-Latin text but no text layer font was \
                             loaded",
                        )
                        .with_page(ocr.page)),
                    }
                };
            placed.push(PlacedWord {
                // The box is the ink: stray whitespace around a token would shift the fit.
                text,
                font,
                bl,
                box_w,
            });
        }

        // Pass 2: the runs; every word but the line's last carries the word space (module
        // docs, "Word spaces").
        let last = placed.len().saturating_sub(1);
        for (i, word) in placed.iter().enumerate() {
            add_invisible_word(
                &mut scratch.page,
                document,
                word,
                i < last,
                size_pt,
                page_rotation,
            )
            .map_err(|e| e.with_page(ocr.page))?;
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

/// `/Rotate` in degrees → pdfium-render's rotation (0 for anything but a quarter turn).
fn render_rotation(degrees: Rotation) -> PdfPageRenderRotation {
    match degrees % 360 {
        90 => PdfPageRenderRotation::Degrees90,
        180 => PdfPageRenderRotation::Degrees180,
        270 => PdfPageRenderRotation::Degrees270,
        _ => PdfPageRenderRotation::None,
    }
}

/// Which font a word is written in.
#[derive(Clone, Copy)]
enum WordFont<'f> {
    /// Latin-1: base-14 Helvetica, nothing embedded.
    Helvetica(PdfFontToken),
    /// Everything else: the batch's glyphless CID font (O5).
    Glyphless(&'f GlyphlessFont),
}

/// One word of a line, ready to place.
struct PlacedWord<'a> {
    text: &'a str,
    font: WordFont<'a>,
    /// The box's bottom-left corner in user space: the run's origin.
    bl: (f32, f32),
    /// The box width in points, along the (possibly rotated) baseline.
    box_w: f32,
}

/// Adds one invisible (`3 Tr`) text object for `word`, fitted to its box, with a trailing
/// U+0020 when `space_after`.
///
/// The fit is measured on the bare word, before the space is appended, so the glyphs fill the
/// box exactly as they did without it and the space simply follows at the same scale.
fn add_invisible_word<'a>(
    page: &mut PdfPage<'a>,
    document: &PdfDocument<'a>,
    word: &PlacedWord<'_>,
    space_after: bool,
    size_pt: f32,
    page_rotation: Rotation,
) -> Result<(), EngineError> {
    let fit = |natural_w: f32| {
        if natural_w > 0.0 {
            (word.box_w / natural_w).clamp(MIN_SX, MAX_SX)
        } else {
            1.0
        }
    };
    match word.font {
        WordFont::Helvetica(token) => {
            let mut object =
                PdfPageTextObject::new(document, word.text, token, PdfPoints::new(size_pt))
                    .ctx("create OCR text object")?;
            object
                .set_render_mode(PdfPageTextRenderMode::Invisible)
                .ctx("set render mode 3")?;
            let natural_w = object
                .bounds()
                .ctx("measure OCR word")?
                .to_rect()
                .width()
                .value;
            let sx = fit(natural_w);
            if space_after {
                object
                    .set_text(format!("{} ", word.text))
                    .ctx("append the OCR word space")?;
            }
            // Order matters: scale post-multiplies in page space, so it must happen before the
            // translate that positions the run (ocr spike gotcha 11).
            object.scale(sx, 1.0).ctx("fit OCR word to its box")?;
            if !page_rotation.is_multiple_of(360) {
                object
                    .rotate_counter_clockwise_degrees(page_rotation as f32)
                    .ctx("rotate OCR word")?;
            }
            object
                .translate(PdfPoints::new(word.bl.0), PdfPoints::new(word.bl.1))
                .ctx("place OCR word")?;
            page.objects_mut()
                .add_text_object(object)
                .ctx("add OCR word")?;
        }
        WordFont::Glyphless(font) => {
            // The same sequence through the raw bindings (the font has no pdfium-render token).
            let object = font.text_object(document, word.text, size_pt)?;
            let sx = fit(object.width()?);
            if space_after {
                object.set_text(&format!("{} ", word.text))?;
            }
            object.transform([sx as f64, 0.0, 0.0, 1.0, 0.0, 0.0]);
            if !page_rotation.is_multiple_of(360) {
                let theta = (page_rotation as f64).to_radians();
                let (sin, cos) = (theta.sin(), theta.cos());
                object.transform([cos, sin, -sin, cos, 0.0, 0.0]);
            }
            object.transform([1.0, 0.0, 0.0, 1.0, word.bl.0 as f64, word.bl.1 as f64]);
            object.insert(page)?;
        }
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
        assert_eq!(
            caps.engines.contains(&OcrEngine::Vision),
            vision_available()
        );
        assert_eq!(
            caps.engines.contains(&OcrEngine::Windows),
            !windows_languages().is_empty()
        );
        #[cfg(not(windows))]
        assert!(!caps.engines.contains(&OcrEngine::Windows));
        assert_eq!(caps.engine_languages.tesseract, ["kor", "eng"]);
        assert_eq!(caps.engine_languages.vision, vision_languages());
        #[cfg(not(target_os = "macos"))]
        assert!(!vision_available());
        assert!(caps.languages.iter().any(|l| l == "kor"));
    }
}
