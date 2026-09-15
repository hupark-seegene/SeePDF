//! `create_annotation` — one `AnnotSpec` → one annotation on the page.
//!
//! Which PDF object each tool becomes (`ARCHITECTURE.md` §6.1, annotations spike §2):
//!
//! | spec | PDF | why |
//! |---|---|---|
//! | highlight / underline / strikeout / squiggly | the matching markup subtype + `/QuadPoints` | |
//! | note | `Text` | PDFium generates the note-icon appearance |
//! | ink / signature | `Ink` with a real `/InkList` | `AddInkStroke`; the high-level ink annotation only gets path objects in its appearance stream |
//! | square / circle | `Square` / `Circle` | Circle has no high-level constructor in 0.9.4 |
//! | line / arrow | **`Ink`** + `/Subj "SeePDF:Line"`/`"SeePDF:Arrow"` | `FPDFPage_CreateAnnot(FPDF_ANNOT_LINE)` returns NULL |
//! | textbox | **`Stamp`** + text objects + `/Subj "SeePDF:TextBox"`, `/DA` for size and colour | FreeText persists only with `/DA`, and `/Helv` cannot render 한글 |
//! | stamp | `Stamp` + an image object, or a built-in label | |
//! | link | `Link` + `FPDFAnnot_SetURI` | not reachable from `AnnotSpec` (P2 in the contract); the engine entry point exists and is tested |
//!
//! Every annotation is created with the `/F 4` Print flag, because
//! `FPDFPage_Flatten(FLAT_PRINT)` silently *deletes* annotations without it and
//! pdfium-render never sets it.

use crate::engine::annot::{
    self, font, read, ScratchPage, SUBJ_ARROW, SUBJ_LINE, SUBJ_SIGNATURE, SUBJ_TEXTBOX,
};
use crate::engine::raw::{self, annot::AnnotRef, annot::ColorKind, consts};
use crate::engine::registry::OpenDoc;
use crate::ipc::error::PdfiumResultExt;
use crate::ipc::types::{
    AnnotId, AnnotSpec, InkSpec, LineSpec, MarkupSpec, NoteSpec, PageIndex, Rect, Rgb, ShapeSpec,
    StampImage, StampSpec, TextAlign, TextBoxSpec,
};
use crate::ipc::{EngineError, ErrorCode};
use pdfium_render::prelude::{
    PdfColor, PdfPage, PdfPageAnnotationCommon, PdfPageImageObject, PdfPageObjectCommon,
    PdfPageObjectsCommon, PdfPagePathObject, PdfPageTextObject, PdfPoints, PdfRect,
    PdfiumLibraryBindings,
};
use std::os::raw::c_int;

/// Side of the sticky-note icon PDFium draws, in points.
const NOTE_SIZE: f32 = 20.0;
/// Inset between a text box's `/Rect` and its first glyph.
const TEXTBOX_PADDING: f32 = 3.0;
/// Baseline-to-baseline distance, as a multiple of the font size.
const LINE_HEIGHT: f32 = 1.25;

/// Creates the annotation described by `spec` and returns its id.
///
/// `id` is `Some` when redo re-creates an annotation that must keep its `/NM`.
/// The caller is `registry::mutate`; this function adds `page_index` to `doc.touched` so the
/// pre-save appearance pass renders it.
pub fn create(
    doc: &mut OpenDoc<'_>,
    page_index: PageIndex,
    spec: &AnnotSpec,
    id: Option<AnnotId>,
) -> Result<AnnotId, EngineError> {
    let id = id.unwrap_or_else(annot::new_id);
    let bindings = doc.bindings();

    // Fonts first: `fonts_mut()` needs `&mut PdfDocument` (annotations spike §5.14).
    let font = match spec {
        AnnotSpec::Textbox(t) => Some(font::resolve(doc.pdf_mut(), &t.text)?),
        AnnotSpec::Stamp(StampSpec {
            image: StampImage::Builtin { builtin },
            ..
        }) => Some(font::resolve(doc.pdf_mut(), builtin)?),
        _ => None,
    };

    let mut scratch = ScratchPage::open(doc, page_index)?;
    match spec {
        AnnotSpec::Highlight(m) => {
            markup(bindings, &scratch.page, consts::FPDF_ANNOT_HIGHLIGHT, m, &id)?
        }
        AnnotSpec::Underline(m) => {
            markup(bindings, &scratch.page, consts::FPDF_ANNOT_UNDERLINE, m, &id)?
        }
        AnnotSpec::Strikeout(m) => {
            markup(bindings, &scratch.page, consts::FPDF_ANNOT_STRIKEOUT, m, &id)?
        }
        AnnotSpec::Squiggly(m) => {
            markup(bindings, &scratch.page, consts::FPDF_ANNOT_SQUIGGLY, m, &id)?
        }
        AnnotSpec::Note(n) => note(bindings, &scratch.page, n, &id)?,
        AnnotSpec::Ink(i) => ink(bindings, &scratch.page, i, &id, None)?,
        AnnotSpec::Signature(i) => ink(bindings, &scratch.page, i, &id, Some(SUBJ_SIGNATURE))?,
        AnnotSpec::Square(s) => shape(bindings, &scratch.page, consts::FPDF_ANNOT_SQUARE, s, &id)?,
        AnnotSpec::Circle(s) => shape(bindings, &scratch.page, consts::FPDF_ANNOT_CIRCLE, s, &id)?,
        AnnotSpec::Line(l) => line(bindings, &scratch.page, l, &id, false)?,
        AnnotSpec::Arrow(l) => line(bindings, &scratch.page, l, &id, true)?,
        AnnotSpec::Textbox(t) => {
            let font = font.expect("resolved above for Textbox");
            textbox(doc, &mut scratch.page, t, &id, &font)?
        }
        AnnotSpec::Stamp(s) => {
            stamp(doc, &mut scratch.page, s, &id, font.as_ref())?;
        }
    }
    drop(scratch);

    doc.touched.insert(page_index);
    doc.annots.remove(&page_index);
    Ok(id)
}

/// `Link` + `FPDFAnnot_SetURI`. Not reachable from `AnnotSpec` (link creation is P2 in the
/// contract) but complete and tested, so the tool only has to call it.
pub fn create_link(
    doc: &mut OpenDoc<'_>,
    page_index: PageIndex,
    rect: Rect,
    quads: &[Rect],
    uri: &str,
) -> Result<AnnotId, EngineError> {
    let id = annot::new_id();
    let bindings = doc.bindings();
    let scratch = ScratchPage::open(doc, page_index)?;
    {
        let mut a = raw::annot::create(bindings, &scratch.page, consts::FPDF_ANNOT_LINK)?;
        a.set_rect(rect);
        for q in quads {
            a.append_quad(*q);
        }
        if !a.set_uri(uri) {
            return Err(EngineError::new(
                ErrorCode::Pdfium,
                "FPDFAnnot_SetURI failed",
            ));
        }
        write_identity(&mut a, &id, None);
    }
    drop(scratch);
    doc.touched.insert(page_index);
    doc.annots.remove(&page_index);
    Ok(id)
}

// ---------------------------------------------------------------------------------------
// one builder per family
// ---------------------------------------------------------------------------------------

fn alpha_of(opacity: f32) -> u8 {
    (opacity.clamp(0.0, 1.0) * 255.0).round() as u8
}

fn union_of(rects: &[Rect]) -> Rect {
    rects
        .iter()
        .copied()
        .reduce(|a, b| a.union(&b))
        .unwrap_or(Rect::ZERO)
}

/// `/NM`, `/CreationDate`, `/M`, `/Subj` and the `/F 4` Print flag — the last step of every
/// create. (Nothing to do with the Stamp subtype: this runs for all of them.)
fn write_identity(a: &mut AnnotRef<'_>, id: &str, subj: Option<&str>) {
    a.set_flags(a.flags() | consts::FPDF_ANNOT_FLAG_PRINT);
    a.set_string("NM", id);
    if let Some(subj) = subj {
        a.set_string("Subj", subj);
    }
    let now = annot::pdf_date_now();
    a.set_string("CreationDate", &now);
    a.set_string("M", &now);
}

fn markup(
    bindings: &'static dyn PdfiumLibraryBindings,
    page: &PdfPage<'_>,
    subtype: c_int,
    spec: &MarkupSpec,
    id: &str,
) -> Result<(), EngineError> {
    if spec.rects.is_empty() {
        return Err(EngineError::invalid("a markup annotation needs ≥ 1 rect"));
    }
    let mut a = raw::annot::create(bindings, page, subtype)?;
    // `/Rect` must be the union of the quads: Acrobat hit-tests against it.
    a.set_rect(union_of(&spec.rects));
    a.set_color(ColorKind::Stroke, spec.color, alpha_of(spec.opacity));
    annot::record_colors(&mut a, spec.color, None);
    for r in &spec.rects {
        // TL, TR, BL, BR — `PdfQuadPoints::from_rect()` is the wrong order and renders a
        // 1-px sliver (annotations spike §5.2).
        if !a.append_quad(*r) {
            return Err(EngineError::new(
                ErrorCode::Pdfium,
                "FPDFAnnot_AppendAttachmentPoints failed",
            ));
        }
    }
    if let Some(c) = &spec.contents {
        a.set_string("Contents", c);
    }
    write_identity(&mut a, id, None);
    Ok(())
}

fn note(
    bindings: &'static dyn PdfiumLibraryBindings,
    page: &PdfPage<'_>,
    spec: &NoteSpec,
    id: &str,
) -> Result<(), EngineError> {
    let mut a = raw::annot::create(bindings, page, consts::FPDF_ANNOT_TEXT)?;
    // The contract's anchor is the note's top-left corner.
    a.set_rect(Rect::new(
        spec.at[0],
        spec.at[1] - NOTE_SIZE,
        spec.at[0] + NOTE_SIZE,
        spec.at[1],
    ));
    a.set_color(ColorKind::Stroke, spec.color, 255);
    annot::record_colors(&mut a, spec.color, None);
    a.set_string("Contents", &spec.contents);
    write_identity(&mut a, id, None);
    Ok(())
}

fn ink(
    bindings: &'static dyn PdfiumLibraryBindings,
    page: &PdfPage<'_>,
    spec: &InkSpec,
    id: &str,
    subj: Option<&str>,
) -> Result<(), EngineError> {
    if spec.paths.iter().all(|p| p.len() < 4) {
        return Err(EngineError::invalid(
            "an ink annotation needs at least one stroke with two points",
        ));
    }
    let mut a = raw::annot::create(bindings, page, consts::FPDF_ANNOT_INK)?;
    a.set_rect(paths_bounds(&spec.paths, spec.width));
    a.set_color(ColorKind::Stroke, spec.color, alpha_of(spec.opacity));
    annot::record_colors(&mut a, spec.color, None);
    a.set_border_width(spec.width);
    for path in &spec.paths {
        if path.len() >= 4 {
            a.add_ink_stroke(path)?;
        }
    }
    write_identity(&mut a, id, subj);
    Ok(())
}

fn shape(
    bindings: &'static dyn PdfiumLibraryBindings,
    page: &PdfPage<'_>,
    subtype: c_int,
    spec: &ShapeSpec,
    id: &str,
) -> Result<(), EngineError> {
    let mut a = raw::annot::create(bindings, page, subtype)?;
    a.set_rect(spec.rect);
    let alpha = alpha_of(spec.opacity);
    a.set_color(ColorKind::Stroke, spec.color, alpha);
    if let Some(fill) = spec.fill_color {
        // The same alpha: every SetColor call rewrites `/CA` (annotations spike §5.7).
        a.set_color(ColorKind::Interior, fill, alpha);
    }
    annot::record_colors(&mut a, spec.color, spec.fill_color);
    a.set_border_width(spec.width);
    write_identity(&mut a, id, None);
    Ok(())
}

/// A line or arrow: one two-point ink stroke, plus one stroke per arrow head.
fn line(
    bindings: &'static dyn PdfiumLibraryBindings,
    page: &PdfPage<'_>,
    spec: &LineSpec,
    id: &str,
    arrow: bool,
) -> Result<(), EngineError> {
    let heads = spec.heads.unwrap_or(if arrow {
        [false, true]
    } else {
        [false, false]
    });
    let mut paths: Vec<Vec<f32>> = vec![vec![spec.p1[0], spec.p1[1], spec.p2[0], spec.p2[1]]];
    if heads[1] {
        paths.extend(arrow_head(spec.p2, spec.p1, spec.width));
    }
    if heads[0] {
        paths.extend(arrow_head(spec.p1, spec.p2, spec.width));
    }
    let ink_spec = InkSpec {
        paths,
        color: spec.color,
        width: spec.width,
        opacity: spec.opacity,
    };
    ink(
        bindings,
        page,
        &ink_spec,
        id,
        Some(if arrow { SUBJ_ARROW } else { SUBJ_LINE }),
    )
}

/// Two short strokes forming a `>` at `tip`, opening towards `from`.
pub(crate) fn arrow_head(tip: [f32; 2], from: [f32; 2], width: f32) -> Vec<Vec<f32>> {
    let (dx, dy) = (from[0] - tip[0], from[1] - tip[1]);
    let len = (dx * dx + dy * dy).sqrt();
    if len < f32::EPSILON {
        return Vec::new();
    }
    let (ux, uy) = (dx / len, dy / len);
    let size = (width * 4.0).clamp(6.0, len * 0.5);
    // ±25° around the direction back along the line.
    let (c, s) = (0.9063_f32, 0.4226_f32);
    let a = [
        tip[0] + size * (ux * c - uy * s),
        tip[1] + size * (ux * s + uy * c),
    ];
    let b = [
        tip[0] + size * (ux * c + uy * s),
        tip[1] + size * (-ux * s + uy * c),
    ];
    vec![
        vec![tip[0], tip[1], a[0], a[1]],
        vec![tip[0], tip[1], b[0], b[1]],
    ]
}

fn paths_bounds(paths: &[Vec<f32>], width: f32) -> Rect {
    let pad = (width / 2.0).max(1.0);
    let mut r: Option<Rect> = None;
    for path in paths {
        for p in path.chunks_exact(2) {
            let point = Rect::new(p[0], p[1], p[0], p[1]);
            r = Some(match r {
                Some(acc) => acc.union(&point),
                None => point,
            });
        }
    }
    let r = r.unwrap_or(Rect::ZERO);
    Rect::new(r.l - pad, r.b - pad, r.r + pad, r.t + pad)
}

// ---------------------------------------------------------------------------------------
// Stamp-backed kinds: these need the high-level API, because appending a page object to an
// annotation goes through `PdfPageAnnotationObjects` (the object handles pdfium-render
// builds are `pub(crate)`, so `FPDFAnnot_AppendObject` is out of reach from `raw`).
// ---------------------------------------------------------------------------------------

/// The text box: a Stamp carrying a background rectangle and one text object per line.
fn textbox(
    doc: &OpenDoc<'_>,
    page: &mut PdfPage<'_>,
    spec: &TextBoxSpec,
    id: &str,
    font: &font::FontChoice,
) -> Result<(), EngineError> {
    let bindings = doc.bindings();
    let size = spec.font_size.max(1.0);
    let pdf_rect = to_pdf_rect(spec.rect);
    {
        let mut a = page
            .annotations_mut()
            .create_stamp_annotation()
            .ctx("create stamp annotation")?;
        // Before any object: the appearance BBox is the `/Rect` at append time.
        a.set_bounds(pdf_rect).ctx("set stamp bounds")?;
        if let Some(fill) = spec.fill_color {
            let bg = PdfPagePathObject::new_rect(
                doc.pdf(),
                pdf_rect,
                None,
                None,
                Some(rgb(fill, 255)),
            )
            .ctx("text box background")?;
            a.objects_mut()
                .add_path_object(bg)
                .ctx("append text box background")?;
        }
        let mut baseline = spec.rect.t - TEXTBOX_PADDING - size;
        for raw_line in spec.text.split('\n') {
            if baseline < spec.rect.b {
                break; // clipped by the box; the UI grows the rect instead
            }
            if !raw_line.is_empty() {
                let mut t = PdfPageTextObject::new(doc.pdf(), raw_line, font.token, points(size))
                    .ctx("text box line")?;
                t.set_fill_color(rgb(spec.color, 255)).ctx("text colour")?;
                let width = t
                    .bounds()
                    .map(|b| b.right().value - b.left().value)
                    .unwrap_or(0.0);
                let x = match spec.align {
                    TextAlign::Left => spec.rect.l + TEXTBOX_PADDING,
                    TextAlign::Center => {
                        spec.rect.l + (spec.rect.width() - width) / 2.0
                    }
                    TextAlign::Right => spec.rect.r - TEXTBOX_PADDING - width,
                };
                // Transform before appending: pdfium-render never calls
                // `FPDFAnnot_UpdateObject` (annotations spike §5.6).
                t.translate(points(x), points(baseline))
                    .ctx("place text box line")?;
                a.objects_mut()
                    .add_text_object(t)
                    .ctx("append text box line")?;
            }
            baseline -= size * LINE_HEIGHT;
        }
    }
    let index = raw::annot::count(bindings, page).saturating_sub(1);
    let mut a = raw::annot::get(bindings, page, index)?;
    a.set_string("Contents", &spec.text);
    // `/DA` is where the size and colour survive a round trip (there is no number setter).
    a.set_string("DA", &read::build_da(spec.color, size));
    annot::record_colors(&mut a, spec.color, spec.fill_color);
    write_identity(&mut a, id, Some(SUBJ_TEXTBOX));
    Ok(())
}

/// An image stamp (PNG or JPEG), or a built-in label stamp.
fn stamp(
    doc: &OpenDoc<'_>,
    page: &mut PdfPage<'_>,
    spec: &StampSpec,
    id: &str,
    font: Option<&font::FontChoice>,
) -> Result<(), EngineError> {
    let bindings = doc.bindings();
    let pdf_rect = to_pdf_rect(spec.rect);
    let mut subj: Option<String> = None;
    {
        let mut a = page
            .annotations_mut()
            .create_stamp_annotation()
            .ctx("create stamp annotation")?;
        a.set_bounds(pdf_rect).ctx("set stamp bounds")?;
        match &spec.image {
            StampImage::Path { path } => {
                let bytes = std::fs::read(path).map_err(|e| {
                    EngineError::new(ErrorCode::Io, format!("read stamp image {path}: {e}"))
                })?;
                let image = image::load_from_memory(&bytes).map_err(|e| {
                    EngineError::new(
                        ErrorCode::InvalidArgument,
                        format!("{path} is not a PNG or JPEG image: {e}"),
                    )
                })?;
                let mut obj = PdfPageImageObject::new_with_size(
                    doc.pdf(),
                    &image,
                    points(spec.rect.width()),
                    points(spec.rect.height()),
                )
                .ctx("stamp image object")?;
                obj.translate(points(spec.rect.l), points(spec.rect.b))
                    .ctx("place stamp image")?;
                a.objects_mut()
                    .add_image_object(obj)
                    .ctx("append stamp image")?;
            }
            StampImage::Builtin { builtin } => {
                let font = font.ok_or_else(|| {
                    EngineError::invalid("a built-in stamp needs a label font")
                })?;
                let colour = builtin_color(builtin);
                let border = PdfPagePathObject::new_rect(
                    doc.pdf(),
                    pdf_rect,
                    Some(rgb(colour, 255)),
                    Some(points(2.0)),
                    None,
                )
                .ctx("stamp border")?;
                a.objects_mut()
                    .add_path_object(border)
                    .ctx("append stamp border")?;
                let label = builtin.to_uppercase();
                let size = (spec.rect.height() * 0.5).clamp(6.0, 48.0);
                let mut t = PdfPageTextObject::new(doc.pdf(), &label, font.token, points(size))
                    .ctx("stamp label")?;
                t.set_fill_color(rgb(colour, 255)).ctx("stamp colour")?;
                let width = t
                    .bounds()
                    .map(|b| b.right().value - b.left().value)
                    .unwrap_or(0.0);
                t.translate(
                    points(spec.rect.l + (spec.rect.width() - width) / 2.0),
                    points(spec.rect.b + (spec.rect.height() - size) / 2.0),
                )
                .ctx("place stamp label")?;
                a.objects_mut()
                    .add_text_object(t)
                    .ctx("append stamp label")?;
                subj = Some(builtin.clone());
            }
        }
    }
    let index = raw::annot::count(bindings, page).saturating_sub(1);
    let mut a = raw::annot::get(bindings, page, index)?;
    if let Some(rotate) = spec.rotate {
        // Recorded for the UI; PDFium has no annotation rotation entry of its own.
        a.set_string("SeePDFRotate", &format!("{rotate}"));
    }
    write_identity(&mut a, id, subj.as_deref());
    Ok(())
}

fn builtin_color(name: &str) -> Rgb {
    match name.to_ascii_lowercase().as_str() {
        "approved" | "final" | "completed" => [0, 140, 60],
        "draft" | "forcomment" | "notapproved" => [200, 120, 0],
        "confidential" | "urgent" | "void" => [200, 0, 0],
        _ => [40, 70, 160],
    }
}

fn points(v: f32) -> PdfPoints {
    PdfPoints::new(v)
}

fn rgb(c: Rgb, alpha: u8) -> PdfColor {
    PdfColor::new(c[0], c[1], c[2], alpha)
}

/// `Rect { l, b, r, t }` → `PdfRect::new_from_values(bottom, left, top, right)` — note the
/// argument order (`STAGE0_NOTES.md` §5.8).
fn to_pdf_rect(r: Rect) -> PdfRect {
    PdfRect::new_from_values(
        r.b.min(r.t),
        r.l.min(r.r),
        r.b.max(r.t),
        r.l.max(r.r),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arrow_head_points_back_along_the_line() {
        let heads = arrow_head([100.0, 100.0], [0.0, 100.0], 2.0);
        assert_eq!(heads.len(), 2);
        for stroke in &heads {
            assert_eq!(stroke.len(), 4);
            assert_eq!((stroke[0], stroke[1]), (100.0, 100.0));
            // The barbs point back towards `from`, i.e. to the left of the tip.
            assert!(stroke[2] < 100.0, "barb x {} should be left of the tip", stroke[2]);
        }
        // Symmetric about the line's axis.
        assert!((heads[0][3] - 100.0).abs() > 0.5);
        assert!(((heads[0][3] - 100.0) + (heads[1][3] - 100.0)).abs() < 0.001);
    }

    #[test]
    fn degenerate_arrow_has_no_head() {
        assert!(arrow_head([10.0, 10.0], [10.0, 10.0], 2.0).is_empty());
    }

    #[test]
    fn bounds_include_the_stroke_width() {
        let r = paths_bounds(&[vec![10.0, 10.0, 20.0, 30.0]], 4.0);
        assert_eq!((r.l, r.b, r.r, r.t), (8.0, 8.0, 22.0, 32.0));
    }
}
