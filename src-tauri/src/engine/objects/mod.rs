//! Page objects — text and image editing. `IPC_CONTRACT.md` §7.4, `ARCHITECTURE.md` §6.2.
//!
//! `ObjectId` is the page-object **index**, and it is only valid inside the generation it was
//! listed in — that is why every mutating command carries `expectGeneration` and fails with
//! `stale` when it no longer matches. Every command here regenerates the page content exactly
//! once, at the end: `set_text`, `set_render_mode`, `set_fill_color` and
//! `set_unscaled_font_size` do **not** mark the page dirty, so without the explicit
//! `regenerate_content()` the edit is simply lost (text spike §7).
//!
//! ## The editability boundary
//!
//! PDFium checks nothing before it draws: a glyph the object's font does not have is silently
//! dropped, which is how `set_text("SeePDF edited title")` on a LaTeX subset font renders as
//! `SeePDFeditedtitle` — the space has width 0 (text spike §5). `PdfFontGlyphs::len()` is 0 for
//! exactly those fonts, so the glyph API cannot answer the question either. The only reliable
//! detector is the **trial edit**: apply `set_text`, open a fresh `page.text()` (it is built
//! from the in-memory objects, so it sees the change before regeneration), require
//! `loose_bounds().width() > 0` for every non-generated character *including spaces* **and**
//! that the characters read back are the ones we asked for (a non-CID font maps everything
//! above U+00FF to 0xFF, so `한글` becomes `ÿÿ` with perfectly good widths), then revert.
//! [`probe`] is that check and [`edit_text`] runs it again before committing.
//!
//! Text inside a Form XObject is refused outright: `set_text` on a child object returns `Ok`,
//! the in-memory text changes, and after save/reopen **nothing happened** — `FPDFPage_Generate
//! Content` only rewrites the page stream and re-emits `/Form Do` (text spike §8.7). Reporting
//! success there would be a silent data loss.

use crate::engine::annot::ScratchPage;
use crate::engine::fonts;
use crate::engine::registry::{self, MutateOpts, OpenDoc};
use crate::engine::types::EngineState;
use crate::ipc::error::PdfiumResultExt;
use crate::ipc::types::{
    ChangeReason, DocGeneration, Editability, Mat6, NotEditableReason, ObjectId, PageIndex,
    PageObject, PageObjectList, PageObjectType, Point, Rect, Rgb, TextAlign, TextEditProbe,
    TextEditStrategy, TextObjectPatch,
};
use crate::ipc::{EngineError, ErrorCode};
use pdfium_render::prelude::*;

/// Line spacing of a multi-line `add_text_object`, as a multiple of the font size.
const LINE_HEIGHT: f32 = 1.2;

// ---------------------------------------------------------------------------------------
// Listing
// ---------------------------------------------------------------------------------------

/// `list_page_objects` — every **top-level** object of the page, in index order.
///
/// Children of a Form XObject are deliberately not given ids of their own: the contract's
/// `ObjectId` is the page-object index and `PdfPageXObjectFormObject` has no add/remove, so an
/// id for a child would be an id nothing can act on. The form object itself is listed with its
/// inner text in `text` so the UI can show what is there, marked `readOnly` /`insideXObject`.
pub fn list(doc: &mut OpenDoc<'_>, page_index: PageIndex) -> Result<PageObjectList, EngineError> {
    let generation = doc.generation;
    let bundled_family = fonts::BUNDLED_FAMILY;
    let bundled_ps = fonts::BUNDLED_POSTSCRIPT;
    let page = doc.page(page_index)?;
    let text_page = page.text().ctx("load text page")?;
    let mut objects = Vec::with_capacity(page.objects().len());
    for (index, object) in page.objects().iter().enumerate() {
        objects.push(describe(
            index as u32,
            &object,
            &text_page,
            bundled_family,
            bundled_ps,
        ));
    }
    Ok(PageObjectList {
        doc_generation: generation,
        objects,
    })
}

fn describe(
    object_id: ObjectId,
    object: &PdfPageObject<'_>,
    text_page: &PdfPageText<'_>,
    bundled_family: &str,
    bundled_ps: &str,
) -> PageObject {
    let rect = object
        .bounds()
        .map(|q| {
            let r = q.to_rect();
            Rect::new(
                r.left().value,
                r.bottom().value,
                r.right().value,
                r.top().value,
            )
        })
        .unwrap_or(Rect::ZERO);
    let matrix: Mat6 = object
        .matrix()
        .map(|m| [m.a(), m.b(), m.c(), m.d(), m.e(), m.f()])
        .unwrap_or([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
    let object_type = match object.object_type() {
        PdfPageObjectType::Text => PageObjectType::Text,
        PdfPageObjectType::Image => PageObjectType::Image,
        PdfPageObjectType::Path => PageObjectType::Path,
        PdfPageObjectType::Shading => PageObjectType::Shading,
        PdfPageObjectType::XObjectForm => PageObjectType::Form,
        _ => PageObjectType::Other,
    };

    let mut out = PageObject {
        object_id,
        object_type,
        rect,
        matrix,
        ours: false,
        text: None,
        font_name: None,
        font_size_pt: None,
        color: None,
        editable: Editability::Full,
        reason: None,
    };

    match object_type {
        PageObjectType::Text => {
            let t = object.as_text_object().expect("text object");
            // `PdfPageTextObject::text()` loads a whole FPDF_TEXTPAGE per call (0.45 ms);
            // `for_object` reuses the one we already have (text spike §8.1).
            let text = text_page.for_object(t);
            let font_name = t.font().name();
            out.ours = font_name == bundled_ps || font_name == bundled_family;
            out.font_size_pt = Some(t.unscaled_font_size().value);
            out.color = object.fill_color().ok().map(rgb_of);
            let (editable, reason) = text_editability(t, &text);
            out.editable = editable;
            out.reason = reason;
            out.text = Some(text);
            out.font_name = Some(font_name);
        }
        PageObjectType::Form => {
            out.editable = Editability::ReadOnly;
            out.reason = Some(NotEditableReason::InsideXObject);
            if let Some(form) = object.as_x_object_form_object() {
                let mut inner = String::new();
                for child in form.iter() {
                    if let Some(t) = child.as_text_object() {
                        inner.push_str(&text_page.for_object(t));
                    }
                }
                if !inner.is_empty() {
                    out.text = Some(inner);
                }
            }
        }
        PageObjectType::Image => {
            out.editable = Editability::Full;
        }
        _ => {
            // Paths and shadings can be moved and deleted but have no content to edit.
            out.editable = Editability::MoveOnly;
        }
    }
    out
}

fn rgb_of(c: PdfColor) -> Rgb {
    [c.red(), c.green(), c.blue()]
}

/// Static properties that rule a text object out before any trial edit is attempted.
fn text_editability(
    t: &PdfPageTextObject<'_>,
    text: &str,
) -> (Editability, Option<NotEditableReason>) {
    if matches!(
        t.render_mode(),
        PdfPageTextRenderMode::Invisible | PdfPageTextRenderMode::InvisibleClipping
    ) {
        // The OCR layer. Deleting it is how "re-OCR" works; editing the characters of a layer
        // nobody can see is meaningless.
        return (Editability::MoveOnly, Some(NotEditableReason::Invisible));
    }
    if !text.is_empty() && !has_usable_unicode(text) {
        // A font with no `/ToUnicode` extracts as raw char codes (`\u{2}\u{3}…`, TAMReview),
        // and `set_text` on it produces "ÿ" (text spike §8.8).
        return (Editability::MoveOnly, Some(NotEditableReason::NoUnicode));
    }
    (Editability::Full, None)
}

/// A run is usable when its characters are real text, not raw glyph indices leaking through a
/// missing `/ToUnicode`.
fn has_usable_unicode(text: &str) -> bool {
    let mut total = 0usize;
    let mut junk = 0usize;
    for ch in text.chars() {
        total += 1;
        let cp = ch as u32;
        if cp < 0x20 && !matches!(ch, '\n' | '\r' | '\t') {
            junk += 1;
        } else if cp == 0xFFFD {
            junk += 1;
        }
    }
    total == 0 || junk * 4 < total
}

// ---------------------------------------------------------------------------------------
// Generation guard
// ---------------------------------------------------------------------------------------

/// Object indices are only meaningful inside one generation (contract §3).
fn check_generation(doc: &OpenDoc<'_>, expect: DocGeneration) -> Result<(), EngineError> {
    if doc.generation != expect {
        return Err(EngineError::stale(format!(
            "expectGeneration {expect} but the document is at {}; re-list the page objects",
            doc.generation
        )));
    }
    Ok(())
}

fn object_at<'a>(
    page: &'a PdfPage<'_>,
    object_id: ObjectId,
) -> Result<PdfPageObject<'a>, EngineError> {
    let len = page.objects().len();
    if object_id as usize >= len {
        return Err(EngineError::not_found(format!(
            "object {object_id} of {len}"
        )));
    }
    page.objects()
        .get(object_id as usize)
        .ctx(&format!("load object {object_id}"))
}

// ---------------------------------------------------------------------------------------
// probe_text_edit
// ---------------------------------------------------------------------------------------

/// `probe_text_edit` — can this object's characters be changed to `text`, and how?
///
/// Never mutates: the trial `set_text` is reverted before returning and the page content is
/// not regenerated, so the document is byte-identical afterwards. That is why it runs outside
/// `registry::mutate`.
pub fn probe(
    doc: &mut OpenDoc<'_>,
    page_index: PageIndex,
    object_id: ObjectId,
    new_text: &str,
) -> Result<TextEditProbe, EngineError> {
    let page = doc.page(page_index)?;
    let object = object_at(page, object_id)?;
    if object.object_type() == PdfPageObjectType::XObjectForm {
        return Ok(refused(NotEditableReason::InsideXObject));
    }
    let Some(t) = object.as_text_object() else {
        return Err(EngineError::invalid(format!(
            "object {object_id} is not a text object"
        )));
    };
    let current = page.text().ctx("load text page")?.for_object(t);
    if let (_, Some(reason)) = text_editability(t, &current) {
        return Ok(refused(reason));
    }
    drop(object);

    let in_place = trial_covers(page, object_id, new_text, &current)?;
    if in_place {
        return Ok(TextEditProbe {
            strategy: TextEditStrategy::InPlace,
            substitute_font: None,
            reason: None,
        });
    }
    Ok(TextEditProbe {
        strategy: TextEditStrategy::ReplaceFont,
        substitute_font: Some(fonts::pick(new_text).name().to_string()),
        reason: Some(NotEditableReason::GlyphsMissing),
    })
}

fn refused(reason: NotEditableReason) -> TextEditProbe {
    TextEditProbe {
        strategy: TextEditStrategy::Refused,
        substitute_font: None,
        reason: Some(reason),
    }
}

/// The trial coverage check of `ARCHITECTURE.md` §6.2: set the text, read it back through a
/// fresh `page.text()`, restore the original.
///
/// Two things have to hold, and each catches a different silent failure:
///
/// * **every non-generated character has a positive advance width**, spaces included — an
///   embedded subset without a space glyph gives it width 0 and renders
///   `SeePDFeditedtitle` (text spike §5);
/// * **the characters read back are the ones we asked for** — a non-CID font maps every code
///   point above 255 to 0xFF, so `한글` becomes `ÿÿ`, which has a perfectly good advance width
///   and is completely wrong (text spike §9 gotcha 9).
fn trial_covers(
    page: &PdfPage<'_>,
    object_id: ObjectId,
    new_text: &str,
    original: &str,
) -> Result<bool, EngineError> {
    if new_text.is_empty() {
        // `PdfPageTextObject::new("")` substitutes " " (pdfium crashes on an empty text
        // object), so an empty string is a delete, not an edit.
        return Ok(false);
    }
    let mut object = object_at(page, object_id)?;
    let Some(t) = object.as_text_object_mut() else {
        return Ok(false);
    };
    t.set_text(new_text).ctx("trial set_text")?;
    drop(object);

    // Everything between here and the restore below runs without `?`, because an early return
    // would leave the trial text in the in-memory object.
    let ok = measure(page, object_id, new_text);

    // Restore. The page content has not been regenerated, so the document is unchanged.
    let mut object = object_at(page, object_id)?;
    if let Some(t) = object.as_text_object_mut() {
        let restore = if original.is_empty() { " " } else { original };
        t.set_text(restore).ctx("restore after trial")?;
    }
    Ok(ok)
}

/// The two conditions of the trial check, with every failure answered `false` rather than `?`.
fn measure(page: &PdfPage<'_>, object_id: ObjectId, new_text: &str) -> bool {
    let Ok(text_page) = page.text() else {
        return false;
    };
    let Ok(object) = object_at(page, object_id) else {
        return false;
    };
    let Some(t) = object.as_text_object() else {
        return false;
    };
    if text_page.for_object(t).trim_end() != new_text.trim_end() {
        return false;
    }
    let Ok(chars) = t.chars(&text_page) else {
        return false;
    };
    let mut seen = 0usize;
    for ch in chars.iter() {
        if ch.is_generated().unwrap_or(false) {
            continue;
        }
        seen += 1;
        if ch.loose_bounds().map(|b| b.width().value).unwrap_or(0.0) <= 0.0 {
            return false;
        }
    }
    seen > 0
}

// ---------------------------------------------------------------------------------------
// edit_text_object
// ---------------------------------------------------------------------------------------

/// `edit_text_object` — change a run's characters, size and/or colour.
///
/// The characters go in place when the object's own font covers them; otherwise the object is
/// **replaced** by a new one carrying the same matrix, fill colour and render mode but drawn
/// with Helvetica (Latin-1) or the bundled Hangul subset. `allow_font_substitution: false`
/// makes that case a `fontCoverage` error instead, which is what the UI uses to put the
/// "글꼴이 대체됩니다" confirmation in front of the user.
pub fn edit_text(
    st: &mut EngineState<'_>,
    doc_id: &str,
    page_index: PageIndex,
    object_id: ObjectId,
    expect_generation: DocGeneration,
    patch: TextObjectPatch,
    allow_font_substitution: bool,
) -> Result<PageObjectList, EngineError> {
    check_generation(st.doc(doc_id)?, expect_generation)?;
    registry::mutate(
        st,
        doc_id,
        MutateOpts::new("undo.objectEdit", ChangeReason::Edit).page(page_index),
        |doc| {
            // `ScratchPage` holds a `PdfPage<'p>` that does not borrow `doc`, so the document
            // stays available for `fonts_mut()` and the object constructors afterwards.
            let mut scratch = ScratchPage::open(doc, page_index)?;
            let current = {
                let object = object_at(&scratch.page, object_id)?;
                if object.object_type() == PdfPageObjectType::XObjectForm {
                    return Err(not_editable(NotEditableReason::InsideXObject)
                        .with_page(page_index)
                        .with_detail("insideXObject"));
                }
                let Some(t) = object.as_text_object() else {
                    return Err(EngineError::invalid(format!(
                        "object {object_id} is not a text object"
                    )));
                };
                let text = scratch.page.text().ctx("load text page")?.for_object(t);
                if let (_, Some(reason)) = text_editability(t, &text) {
                    return Err(not_editable(reason).with_page(page_index));
                }
                text
            };

            let mut replaced = false;
            if let Some(new_text) = patch.text.as_deref() {
                if new_text.is_empty() {
                    return Err(EngineError::invalid(
                        "an empty string is a delete, not an edit",
                    ));
                }
                if trial_covers(&scratch.page, object_id, new_text, &current)? {
                    let mut object = object_at(&scratch.page, object_id)?;
                    let t = object.as_text_object_mut().expect("text object");
                    t.set_text(new_text).ctx("set_text")?;
                } else {
                    let pick = fonts::pick(new_text);
                    if !allow_font_substitution {
                        return Err(EngineError::new(
                            ErrorCode::FontCoverage,
                            format!(
                                "the object's own font cannot draw this text; it would have to \
                                 be replaced with {}",
                                pick.name()
                            ),
                        )
                        .with_page(page_index));
                    }
                    let mut hangul = fonts::embedded_token(&scratch.page);
                    let (token, _) = fonts::token_for(doc.pdf_mut(), new_text, &mut hangul)?;
                    replace_object(doc.pdf(), &mut scratch.page, object_id, new_text, token, &patch)?;
                    replaced = true;
                }
            }
            if !replaced {
                let mut object = object_at(&scratch.page, object_id)?;
                if let Some(size) = patch.font_size_pt {
                    let size = check_font_size(size)?;
                    let t = object.as_text_object_mut().expect("text object");
                    t.set_unscaled_font_size(PdfPoints::new(size))
                        .ctx("set_unscaled_font_size")?;
                }
                if let Some(color) = patch.color {
                    object
                        .set_fill_color(PdfColor::new(color[0], color[1], color[2], 255))
                        .ctx("set_fill_color")?;
                }
            }
            scratch
                .page
                .regenerate_content()
                .ctx("regenerate page content")?;
            Ok(())
        },
    )?;
    relist(st, doc_id, page_index)
}

fn not_editable(reason: NotEditableReason) -> EngineError {
    let message = match reason {
        NotEditableReason::InsideXObject => {
            "this text lives inside a Form XObject and cannot be edited in place"
        }
        NotEditableReason::NoUnicode => {
            "this font has no /ToUnicode map, so the characters cannot be re-encoded"
        }
        NotEditableReason::Type3 => "Type3 fonts cannot be re-encoded",
        NotEditableReason::Invisible => {
            "this is an invisible OCR text object; delete it and re-run OCR instead"
        }
        NotEditableReason::Permissions => "the document's permission bits forbid modification",
        NotEditableReason::GlyphsMissing => "the font does not have glyphs for this text",
    };
    EngineError::new(ErrorCode::Unsupported, message)
}

fn check_font_size(size: f32) -> Result<f32, EngineError> {
    if !size.is_finite() || size <= 0.0 || size > 1638.0 {
        return Err(EngineError::invalid(format!(
            "font size {size} pt is out of range"
        )));
    }
    Ok(size)
}

/// The replace-object model: a new text object at the old matrix, then the old one removed.
///
/// Appending first and removing afterwards keeps the old index valid for the removal, and the
/// new object ends up **last** — the caller re-lists the page, so the id it reports is right.
fn replace_object<'p>(
    document: &PdfDocument<'p>,
    page: &mut PdfPage<'p>,
    object_id: ObjectId,
    new_text: &str,
    font: PdfFontToken,
    patch: &TextObjectPatch,
) -> Result<(), EngineError> {
    let (matrix, size, fill, render_mode) = {
        let object = object_at(page, object_id)?;
        let t = object.as_text_object().expect("checked by the caller");
        (
            object.matrix().ctx("read matrix")?,
            t.unscaled_font_size(),
            object.fill_color().ok(),
            t.render_mode(),
        )
    };
    let size = match patch.font_size_pt {
        Some(s) => PdfPoints::new(check_font_size(s)?),
        None => size,
    };
    let mut fresh =
        PdfPageTextObject::new(document, new_text, font, size).ctx("create replacement object")?;
    fresh.apply_matrix(matrix).ctx("apply matrix")?;
    if let Some(color) = patch.color {
        fresh
            .set_fill_color(PdfColor::new(color[0], color[1], color[2], 255))
            .ctx("set_fill_color")?;
    } else if let Some(fill) = fill {
        fresh.set_fill_color(fill).ctx("set_fill_color")?;
    }
    if !matches!(render_mode, PdfPageTextRenderMode::Unknown) {
        fresh.set_render_mode(render_mode).ctx("set_render_mode")?;
    }
    // Append first, remove afterwards: the old index stays valid for the removal, and the new
    // object lands last. The caller re-lists the page, so the id it reports is the new one.
    page.objects_mut()
        .add_text_object(fresh)
        .ctx("add replacement object")?;
    page.objects_mut()
        .remove_object_at_index(object_id as usize)
        .ctx("remove replaced object")?;
    Ok(())
}

// ---------------------------------------------------------------------------------------
// add_text_object
// ---------------------------------------------------------------------------------------

/// `add_text_object` — one object per line, laid out inside `rect` from the top down.
///
/// PDFium has no reflow, so the layout is ours: each line is created at the origin, measured
/// with `bounds()` (which works before the object is added to a page) and then translated to
/// its aligned position. Korean goes through the bundled subset, which is loaded **once** even
/// for a many-line block — `load_true_type_from_bytes` appends a fresh copy of the whole file
/// every call.
#[allow(clippy::too_many_arguments)]
pub fn add_text(
    st: &mut EngineState<'_>,
    doc_id: &str,
    page_index: PageIndex,
    rect: Rect,
    text: &str,
    font_size_pt: f32,
    color: Rgb,
    align: TextAlign,
) -> Result<PageObjectList, EngineError> {
    let size = check_font_size(font_size_pt)?;
    if text.trim().is_empty() {
        return Err(EngineError::invalid("the text is empty"));
    }
    let lines: Vec<String> = text
        .replace("\r\n", "\n")
        .split('\n')
        .map(|l| l.to_string())
        .collect();
    registry::mutate(
        st,
        doc_id,
        MutateOpts::new("undo.objectAdd", ChangeReason::Edit).page(page_index),
        |doc| {
            // Open the page first: `ScratchPage` holds a `PdfPage<'p>` that does not borrow
            // `doc`, so the document is still available for `fonts_mut()` afterwards — and the
            // page is what tells us whether the bundled font is already embedded here.
            let mut scratch = ScratchPage::open(doc, page_index)?;
            let mut hangul = fonts::embedded_token(&scratch.page);
            let mut tokens: Vec<PdfFontToken> = Vec::with_capacity(lines.len());
            for line in &lines {
                let probe = if line.trim().is_empty() { "A" } else { line };
                let (token, _) = fonts::token_for(doc.pdf_mut(), probe, &mut hangul)?;
                tokens.push(token);
            }
            let document = doc.pdf();
            let mut baseline = rect.t - size;
            for (line, token) in lines.iter().zip(tokens) {
                if !line.trim().is_empty() {
                    let mut object =
                        PdfPageTextObject::new(document, line, token, PdfPoints::new(size))
                            .ctx("create text object")?;
                    object
                        .set_fill_color(PdfColor::new(color[0], color[1], color[2], 255))
                        .ctx("set_fill_color")?;
                    let natural = object.bounds().ctx("measure text object")?.to_rect();
                    let width = natural.width().value;
                    let x = match align {
                        TextAlign::Left => rect.l,
                        TextAlign::Center => rect.l + (rect.width() - width) / 2.0,
                        TextAlign::Right => rect.r - width,
                    };
                    object
                        .translate(PdfPoints::new(x), PdfPoints::new(baseline))
                        .ctx("place text object")?;
                    scratch
                        .page
                        .objects_mut()
                        .add_text_object(object)
                        .ctx("add text object")?;
                }
                baseline -= size * LINE_HEIGHT;
            }
            scratch
                .page
                .regenerate_content()
                .ctx("regenerate page content")?;
            Ok(())
        },
    )?;
    relist(st, doc_id, page_index)
}

// ---------------------------------------------------------------------------------------
// add_image_object
// ---------------------------------------------------------------------------------------

/// `add_image_object` — a PNG or JPEG placed at `rect`.
///
/// A fresh `PdfPageImageObject` is **1 × 1 pt at the origin** and transforms are applied in
/// call order, so the order `scale(w, h)` → `translate(x, y)` is load-bearing (pages spike §3).
/// With `keep_aspect` the image is centred inside `rect`.
pub fn add_image(
    st: &mut EngineState<'_>,
    doc_id: &str,
    page_index: PageIndex,
    rect: Rect,
    path: &str,
    keep_aspect: bool,
) -> Result<PageObjectList, EngineError> {
    if rect.width() <= 0.0 || rect.height() <= 0.0 {
        return Err(EngineError::invalid("the image rect is empty"));
    }
    let source = std::path::Path::new(path).to_path_buf();
    if !source.is_file() {
        return Err(EngineError::not_found(format!("{}", source.display())));
    }
    let image = image::open(&source).map_err(|e| {
        EngineError::new(
            ErrorCode::InvalidArgument,
            format!("{}: {e}", source.display()),
        )
    })?;
    registry::mutate(
        st,
        doc_id,
        MutateOpts::new("undo.objectAdd", ChangeReason::Edit).page(page_index),
        |doc| {
            let mut scratch = ScratchPage::open(doc, page_index)?;
            let document = doc.pdf();
            let mut object =
                PdfPageImageObject::new(document, &image).ctx("create image object")?;
            let (w, h) = fit(&image, rect, keep_aspect);
            object.scale(w, h).ctx("scale image object")?;
            let x = rect.l + (rect.width() - w) / 2.0;
            let y = rect.b + (rect.height() - h) / 2.0;
            object
                .translate(PdfPoints::new(x), PdfPoints::new(y))
                .ctx("place image object")?;
            scratch
                .page
                .objects_mut()
                .add_image_object(object)
                .ctx("add image object")?;
            scratch
                .page
                .regenerate_content()
                .ctx("regenerate page content")?;
            Ok(())
        },
    )?;
    relist(st, doc_id, page_index)
}

/// The on-page size in points, letterboxed inside `rect` when the aspect ratio is kept.
fn fit(image: &image::DynamicImage, rect: Rect, keep_aspect: bool) -> (f32, f32) {
    let (w, h) = (rect.width(), rect.height());
    if !keep_aspect {
        return (w, h);
    }
    let (iw, ih) = (image.width(), image.height());
    if iw == 0 || ih == 0 {
        return (w, h);
    }
    let scale = (w / iw as f32).min(h / ih as f32);
    (iw as f32 * scale, ih as f32 * scale)
}

/// `replace_image` (engine-only; no contract command yet — see `STAGE1B_NOTES.md` §5).
///
/// `set_image` preserves the object matrix, so the replacement lands exactly where the old
/// image was, at the same size (pages spike §3).
pub fn replace_image(
    st: &mut EngineState<'_>,
    doc_id: &str,
    page_index: PageIndex,
    object_id: ObjectId,
    expect_generation: DocGeneration,
    path: &str,
) -> Result<PageObjectList, EngineError> {
    check_generation(st.doc(doc_id)?, expect_generation)?;
    let image = image::open(path).map_err(|e| {
        EngineError::new(ErrorCode::InvalidArgument, format!("{path}: {e}"))
    })?;
    registry::mutate(
        st,
        doc_id,
        MutateOpts::new("undo.objectEdit", ChangeReason::Edit).page(page_index),
        |doc| {
            let mut scratch = ScratchPage::open(doc, page_index)?;
            {
                let mut object = object_at(&scratch.page, object_id)?;
                let target = object.as_image_object_mut().ok_or_else(|| {
                    EngineError::invalid(format!("object {object_id} is not an image"))
                })?;
                target.set_image(&image).ctx("set_image")?;
            }
            scratch
                .page
                .regenerate_content()
                .ctx("regenerate page content")?;
            Ok(())
        },
    )?;
    relist(st, doc_id, page_index)
}

/// Every image object of a page, as `(objectId, width_px, height_px, encoded PNG)`.
///
/// `get_processed_image` is what the page actually shows (soft masks, clipping and colour
/// conversion applied) and is the right source for an "extract images" feature; `get_raw_image`
/// is ~10× faster and is what editing uses (pages spike §3).
pub fn extract_images(
    doc: &mut OpenDoc<'_>,
    page_index: PageIndex,
) -> Result<Vec<ExtractedImage>, EngineError> {
    // Opening the page through `ScratchPage` releases the `&mut OpenDoc` borrow, so the
    // document reference `get_processed_image` needs is available at the same time.
    let scratch = ScratchPage::open(doc, page_index)?;
    let document = doc.pdf();
    let mut out = Vec::new();
    for (index, object) in scratch.page.objects().iter().enumerate() {
        let Some(img) = object.as_image_object() else {
            continue;
        };
        let Ok(dynamic) = img.get_processed_image(document) else {
            continue;
        };
        let mut png = Vec::new();
        if dynamic
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .is_ok()
        {
            out.push(ExtractedImage {
                object_id: index as u32,
                width_px: dynamic.width(),
                height_px: dynamic.height(),
                png,
            });
        }
    }
    Ok(out)
}

/// One image lifted off a page by [`extract_images`].
#[derive(Debug, Clone)]
pub struct ExtractedImage {
    pub object_id: ObjectId,
    pub width_px: u32,
    pub height_px: u32,
    /// Re-encoded as PNG, which is lossless for every colour space PDFium hands back.
    pub png: Vec<u8>,
}

// ---------------------------------------------------------------------------------------
// transform / delete
// ---------------------------------------------------------------------------------------

/// `transform_object` — move, scale about the object's own anchor, rotate.
///
/// `scale()` and `rotate…()` post-multiply in **page** space, so applying them alone also moves
/// the object; the anchored form is `translate(-x, -y)` → scale/rotate → `translate(x, y)`
/// (text spike §8.5).
#[allow(clippy::too_many_arguments)]
pub fn transform(
    st: &mut EngineState<'_>,
    doc_id: &str,
    page_index: PageIndex,
    object_id: ObjectId,
    expect_generation: DocGeneration,
    translate: Option<Point>,
    scale: Option<[f32; 2]>,
    rotate_deg: Option<f32>,
) -> Result<PageObjectList, EngineError> {
    check_generation(st.doc(doc_id)?, expect_generation)?;
    if translate.is_none() && scale.is_none() && rotate_deg.is_none() {
        return relist(st, doc_id, page_index);
    }
    if let Some([sx, sy]) = scale {
        if !sx.is_finite() || !sy.is_finite() || sx == 0.0 || sy == 0.0 {
            return Err(EngineError::invalid("scale factors must be non-zero"));
        }
    }
    registry::mutate(
        st,
        doc_id,
        MutateOpts::new("undo.objectTransform", ChangeReason::Edit)
            .page(page_index)
            .coalesced(),
        |doc| {
            let mut scratch = ScratchPage::open(doc, page_index)?;
            {
                let mut object = object_at(&scratch.page, object_id)?;
                let anchor = object.bounds().ctx("read bounds")?.to_rect();
                let (ax, ay) = (anchor.left().value, anchor.bottom().value);
                if scale.is_some() || rotate_deg.is_some() {
                    object
                        .translate(PdfPoints::new(-ax), PdfPoints::new(-ay))
                        .ctx("anchor object")?;
                    if let Some([sx, sy]) = scale {
                        object.scale(sx, sy).ctx("scale object")?;
                    }
                    if let Some(deg) = rotate_deg {
                        object
                            .rotate_clockwise_degrees(deg)
                            .ctx("rotate object")?;
                    }
                    object
                        .translate(PdfPoints::new(ax), PdfPoints::new(ay))
                        .ctx("restore anchor")?;
                }
                if let Some([dx, dy]) = translate {
                    object
                        .translate(PdfPoints::new(dx), PdfPoints::new(dy))
                        .ctx("translate object")?;
                }
            }
            scratch
                .page
                .regenerate_content()
                .ctx("regenerate page content")?;
            Ok(())
        },
    )?;
    relist(st, doc_id, page_index)
}

/// `delete_objects` — removes objects **descending**, because every removal shifts the indices
/// after it by one.
pub fn delete(
    st: &mut EngineState<'_>,
    doc_id: &str,
    page_index: PageIndex,
    object_ids: &[ObjectId],
    expect_generation: DocGeneration,
) -> Result<PageObjectList, EngineError> {
    check_generation(st.doc(doc_id)?, expect_generation)?;
    if object_ids.is_empty() {
        return relist(st, doc_id, page_index);
    }
    let mut ids: Vec<ObjectId> = object_ids.to_vec();
    ids.sort_unstable();
    ids.dedup();
    registry::mutate(
        st,
        doc_id,
        MutateOpts::new("undo.objectDelete", ChangeReason::Edit).page(page_index),
        |doc| {
            let mut scratch = ScratchPage::open(doc, page_index)?;
            let len = scratch.page.objects().len();
            if let Some(&max) = ids.last() {
                if max as usize >= len {
                    return Err(EngineError::not_found(format!("object {max} of {len}")));
                }
            }
            for &id in ids.iter().rev() {
                // The returned object is detached and destroyed on drop.
                scratch
                    .page
                    .objects_mut()
                    .remove_object_at_index(id as usize)
                    .ctx(&format!("remove object {id}"))?;
            }
            scratch
                .page
                .regenerate_content()
                .ctx("regenerate page content")?;
            Ok(())
        },
    )?;
    relist(st, doc_id, page_index)
}

fn relist(
    st: &mut EngineState<'_>,
    doc_id: &str,
    page_index: PageIndex,
) -> Result<PageObjectList, EngineError> {
    let doc = st.doc_mut(doc_id)?;
    list(doc, page_index)
}
