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
//! | stamp | `Stamp` + an image object, or a built-in label | `signature: true` adds `/Subj "SeePDF:Signature"` → read back as `signature` (Stage 8) |
//! | link | `Link` + `FPDFAnnot_SetURI` + `/Border [0 0 0]` | not in `AnnotSpec`: the P2 `create_link` command (`engine::structure::links`) calls [`create_link`] for a web address and writes a go-to-page link with lopdf |
//!
//! Every annotation is created with the `/F 4` Print flag, because
//! `FPDFPage_Flatten(FLAT_PRINT)` silently *deletes* annotations without it and
//! pdfium-render never sets it.

use crate::engine::annot::{
    self, font, lopdf_annots, read, ScratchPage, SUBJ_ARROW, SUBJ_LINE, SUBJ_SIGNATURE,
    SUBJ_TEXTBOX,
};
use crate::engine::fonts;
use crate::engine::raw::{self, annot::AnnotRef, annot::ColorKind, consts};
use crate::engine::registry::{self, MutateOpts, OpenDoc};
use crate::engine::types::EngineState;
use crate::ipc::error::PdfiumResultExt;
use crate::ipc::types::{
    AnnotId, AnnotSpec, ChangeReason, InkSpec, LineSpec, MarkupSpec, NoteSpec, PageIndex, Rect,
    Rgb, ShapeSpec, StampImage, StampShape, StampSpec, TextAlign, TextBoxSpec,
};
use crate::ipc::{EngineError, ErrorCode};
use pdfium_render::prelude::{
    PdfColor, PdfFontToken, PdfPage, PdfPageAnnotationCommon, PdfPageImageObject,
    PdfPageObjectCommon, PdfPageObjectsCommon, PdfPagePathObject, PdfPageTextObject, PdfPoints,
    PdfRect, PdfiumLibraryBindings,
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
    create_with(doc, page_index, spec, id, None)
}

/// [`create`] plus the author (v0.3 A9): `author` is 설정 ▸ 주석 작성자, read on the command
/// side (`app::store::get_settings`), written as `/T` when it is not blank. A text stamp's
/// `{{author}}` expands to it too.
pub fn create_with(
    doc: &mut OpenDoc<'_>,
    page_index: PageIndex,
    spec: &AnnotSpec,
    id: Option<AnnotId>,
    author: Option<&str>,
) -> Result<AnnotId, EngineError> {
    let author = author.map(str::trim).filter(|a| !a.is_empty());
    let id = id.unwrap_or_else(annot::new_id);
    let bindings = doc.bindings();

    // Fonts first: `fonts_mut()` needs `&mut PdfDocument` (annotations spike §5.14).
    let font = match spec {
        AnnotSpec::Textbox(t) => Some(font::resolve(doc.pdf_mut(), &t.text)?),
        AnnotSpec::Callout(c) => Some(font::resolve(doc.pdf_mut(), &c.text)?),
        _ => None,
    };
    // A label stamp (결재 / 승인 / 기밀 are Hangul, a text stamp can say anything): the
    // document-level bundled subset, loaded once per document and coverage-checked, so a
    // missing glyph is an honest `fontCoverage` error instead of a silently blank label — and
    // ten 결재 stamps embed the font once, not ten times (P1-12).
    let label = match spec {
        AnnotSpec::Stamp(StampSpec { image, .. }) => match stamp_label(image, author) {
            Some(label) => {
                let token = doc.hangul_token_for(&label.text)?.0;
                Some((label, token))
            }
            None => None,
        },
        _ => None,
    };

    let mut scratch = ScratchPage::open(doc, page_index)?;
    match spec {
        AnnotSpec::Highlight(m) => markup(
            bindings,
            &scratch.page,
            consts::FPDF_ANNOT_HIGHLIGHT,
            m,
            &id,
        )?,
        AnnotSpec::Underline(m) => markup(
            bindings,
            &scratch.page,
            consts::FPDF_ANNOT_UNDERLINE,
            m,
            &id,
        )?,
        AnnotSpec::Strikeout(m) => markup(
            bindings,
            &scratch.page,
            consts::FPDF_ANNOT_STRIKEOUT,
            m,
            &id,
        )?,
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
            stamp(doc, &mut scratch.page, s, &id, label)?;
        }
        // v0.3 A2: the PDFium half of a callout is the text box itself; `create_in` then
        // turns it into a `FreeText /FreeTextCallout` with lopdf (one undo step).
        AnnotSpec::Callout(c) => {
            let font = font.expect("resolved above for Callout");
            let t = TextBoxSpec {
                rect: c.rect,
                text: c.text.clone(),
                font_size: c.font_size,
                color: c.color,
                align: c.align,
                fill_color: c.fill_color,
            };
            textbox(doc, &mut scratch.page, &t, &id, &font)?
        }
        AnnotSpec::Polygon(_) | AnnotSpec::Polyline(_) => {
            return Err(EngineError::invalid(
                "a polygon / polyline is written with lopdf: use create_in",
            ));
        }
    }
    // v0.3 A9: the author, on every kind — the last write, like /M.
    if let Some(author) = author {
        let index = annot::index_of(bindings, &scratch.page, &id)?;
        raw::annot::get(bindings, &scratch.page, index)?.set_string("T", author);
    }
    drop(scratch);

    doc.touched.insert(page_index);
    doc.annots.remove(&page_index);
    Ok(id)
}

/// `create_annotation` as one undoable edit (v0.3): the PDFium kinds through
/// `registry::mutate` + [`create_with`], the lopdf kinds (real `/Line`, `/Polygon`,
/// `/PolyLine`) through `registry::mutate_bytes` + [`lopdf_annots::write_new`], and a callout
/// as a text box (PDFium) turned into a `FreeText` callout by lopdf in a **coalesced** second
/// step, so it is still one undo entry.
///
/// An encrypted document cannot take a lopdf rewrite (it would have to be re-encrypted): a
/// line / arrow falls back to SeePDF's Ink line there, a polygon / polyline / callout is
/// `unsupported`.
pub fn create_in(
    st: &mut EngineState<'_>,
    doc_id: &str,
    page: PageIndex,
    spec: &AnnotSpec,
    id: Option<AnnotId>,
    author: Option<&str>,
) -> Result<AnnotId, EngineError> {
    let opts = || {
        MutateOpts::new("undo.annotCreate", ChangeReason::Edit)
            .page(page)
            .keeps_text()
    };
    let author = author.map(str::trim).filter(|a| !a.is_empty());
    // `rewritable`: lopdf can write the file at all (not encrypted). `quiet`: it may also do so
    // where PDFium has a fallback — v0.3 integration (A2/A6 × S1): on a signed file that still
    // saves incrementally a line / arrow is drawn as the Ink line and a dashed square / circle
    // solid, so the signatures survive the save. A polygon or callout has no PDFium form and
    // is still written by lopdf there (the signed-document confirm and the save warning say
    // the signatures go).
    let (rewritable, quiet) = {
        let doc = st.doc(doc_id)?;
        if page >= doc.page_count() {
            return Err(EngineError::not_found(format!("page {page}")).with_page(page));
        }
        (
            !(doc.encrypted || doc.password.is_some()),
            doc.quiet_rewrite_ok(),
        )
    };
    let lopdf_shape = lopdf_drawn(spec);
    let has_fallback = matches!(spec, AnnotSpec::Line(_) | AnnotSpec::Arrow(_));
    match (spec, lopdf_shape) {
        (_, Some(drawn)) if rewritable && (quiet || !has_fallback) => {
            let id = id.unwrap_or_else(annot::new_id);
            let ident = lopdf_annots::Identity {
                id: id.clone(),
                author: author.map(str::to_string),
                contents: None,
                date: annot::pdf_date_now(),
            };
            registry::mutate_bytes(st, doc_id, opts(), |bytes, _| {
                lopdf_annots::write_new(bytes, page, &drawn, &ident)
            })?;
            Ok(id)
        }
        (AnnotSpec::Polygon(_) | AnnotSpec::Polyline(_), _) => Err(EngineError::new(
            ErrorCode::Unsupported,
            "a polygon cannot be written into an encrypted document",
        )),
        (AnnotSpec::Callout(c), _) => {
            if !rewritable {
                return Err(EngineError::new(
                    ErrorCode::Unsupported,
                    "a callout cannot be written into an encrypted document",
                ));
            }
            let id = registry::mutate(st, doc_id, opts(), |doc| {
                create_with(doc, page, spec, id, author)
            })?;
            let tip = c.callout.clone();
            let (color, made) = (c.color, id.clone());
            st.doc_mut(doc_id)?.history.refresh_last();
            let converted = registry::mutate_bytes(st, doc_id, opts().coalesced(), |bytes, _| {
                lopdf_annots::to_callout(bytes, page, &made, &tip, color, CALLOUT_WIDTH)
            });
            if let Err(e) = converted {
                // The text is not lost: it stays a plain text box (the first step).
                tracing::warn!(error = %e, "callout conversion failed; kept the text box");
            }
            Ok(id)
        }
        // v0.3 pkg4 (round 2): a dashed square / circle gets its `/BS` from lopdf in a
        // coalesced second step — one undo entry, like a callout.
        (AnnotSpec::Square(s) | AnnotSpec::Circle(s), _) if s.dashed && quiet => {
            let id = registry::mutate(st, doc_id, opts(), |doc| {
                create_with(doc, page, spec, id, author)
            })?;
            let (width, made) = (s.width, id.clone());
            st.doc_mut(doc_id)?.history.refresh_last();
            registry::mutate_bytes(st, doc_id, opts().coalesced(), |bytes, _| {
                lopdf_annots::set_border_style(bytes, page, &made, width, true)
            })?;
            st.doc_mut(doc_id)?.touched.insert(page);
            Ok(id)
        }
        // An encrypted document cannot take the lopdf `/BS`: the shape (and an Ink line) is
        // drawn solid there — and so on a signed file that still saves incrementally.
        _ => registry::mutate(st, doc_id, opts(), |doc| {
            create_with(doc, page, spec, id, author)
        }),
    }
}

/// Stroke width of a callout's leader line, points.
pub const CALLOUT_WIDTH: f32 = 1.0;

/// The lopdf drawing of a spec, `None` for the PDFium kinds.
pub fn lopdf_drawn(spec: &AnnotSpec) -> Option<lopdf_annots::Drawn> {
    match spec {
        AnnotSpec::Line(l) | AnnotSpec::Arrow(l) => {
            let arrow = matches!(spec, AnnotSpec::Arrow(_));
            Some(lopdf_annots::Drawn {
                shape: lopdf_annots::Shape::Line {
                    p1: l.p1,
                    p2: l.p2,
                    heads: l
                        .heads
                        .unwrap_or(if arrow { [false, true] } else { [false, false] }),
                },
                color: l.color,
                fill: None,
                width: l.width,
                opacity: l.opacity,
                dashed: l.dashed,
                measure: l.measure,
            })
        }
        AnnotSpec::Polygon(p) | AnnotSpec::Polyline(p) => Some(lopdf_annots::Drawn {
            shape: if matches!(spec, AnnotSpec::Polygon(_)) {
                lopdf_annots::Shape::Polygon {
                    vertices: p.vertices.clone(),
                    cloudy: p.cloudy,
                }
            } else {
                lopdf_annots::Shape::Polyline {
                    vertices: p.vertices.clone(),
                }
            },
            color: p.color,
            fill: p.fill_color,
            width: p.width,
            opacity: p.opacity,
            dashed: p.dashed,
            measure: p.measure,
        }),
        _ => None,
    }
}

/// `Link` + `FPDFAnnot_SetURI` + `/Border [0 0 0]` (no box drawn). Not reachable from
/// `AnnotSpec`: the P2 `create_link` command (`engine::structure::links`) calls it for a web
/// address; a go-to-page link needs a `/Dest`, which PDFium cannot write, and goes through lopdf.
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
        a.set_no_border();
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
    let heads = spec
        .heads
        .unwrap_or(if arrow { [false, true] } else { [false, false] });
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
    )?;
    // v0.3 A1: which ends carry a head, so the Inspector shows (and a move keeps) them.
    let index = annot::index_of(bindings, page, id)?;
    raw::annot::get(bindings, page, index)?.set_string(annot::KEY_HEADS, &heads_code(heads));
    Ok(())
}

/// `[start, end]` → `"0 1"` (the `SeePDFHeads` mirror).
pub fn heads_code(heads: [bool; 2]) -> String {
    format!("{} {}", heads[0] as u8, heads[1] as u8)
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
            let bg =
                PdfPagePathObject::new_rect(doc.pdf(), pdf_rect, None, None, Some(rgb(fill, 255)))
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
                    TextAlign::Center => spec.rect.l + (spec.rect.width() - width) / 2.0,
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
    // v0.3 A1: the alignment survives a round trip. `/Q` is an integer PDFium cannot write
    // (there is no FPDFAnnot_SetNumberValue), and on a Stamp it would mean nothing to other
    // viewers anyway, so it is mirrored like the colours: `SeePDFQ` = "0" | "1" | "2".
    a.set_string(annot::KEY_ALIGN, read::align_code(spec.align));
    annot::record_colors(&mut a, spec.color, spec.fill_color);
    write_identity(&mut a, id, Some(SUBJ_TEXTBOX));
    Ok(())
}

/// What a built-in stamp says: the Latin names in capitals (`approved` → `APPROVED`), the
/// Korean ones (결재 / 승인 / 기밀) as they are.
pub fn builtin_label(name: &str) -> String {
    name.to_uppercase()
}

/// Share of the stamp's width the label may take, so it never touches the border.
const STAMP_LABEL_MAX_WIDTH: f32 = 0.84;

/// What a label stamp draws: the text, its colour and the border around it.
pub struct StampLabel {
    pub text: String,
    pub color: Rgb,
    pub shape: StampShape,
}

/// v0.3 T2: the quick marks (✓ ✗ ●) are built-in ids drawn as vector paths — no font, so no
/// glyph coverage question.
pub const QUICK_MARKS: [&str; 3] = ["check", "cross", "dot"];

/// `/Subj` of a custom text stamp (내 도장 / 오늘 날짜), v0.3 T2.
pub const SUBJ_TEXT_STAMP: &str = "SeePDF:TextStamp";

/// Ink blue of the quick marks — `stampCatalog.ts` `QUICK_MARK_COLOR`.
pub const QUICK_MARK_COLOR: Rgb = [24, 64, 170];

/// `{{date}}` → today as `yyyy.MM.dd` (local time) and `{{author}}` → 설정 ▸ 작성자 (v0.3 T2).
pub fn expand_stamp_text(text: &str, author: Option<&str>) -> String {
    let date = chrono::Local::now().format("%Y.%m.%d").to_string();
    text.replace("{{date}}", &date)
        .replace("{{author}}", author.unwrap_or(""))
}

/// The label a stamp spec draws; `None` for an image stamp or a quick mark.
pub fn stamp_label(image: &StampImage, author: Option<&str>) -> Option<StampLabel> {
    match image {
        StampImage::Builtin { builtin } if !QUICK_MARKS.contains(&builtin.as_str()) => {
            Some(StampLabel {
                text: builtin_label(builtin),
                color: builtin_color(builtin),
                shape: StampShape::Rect,
            })
        }
        StampImage::Text { text, color, shape } => {
            let text = expand_stamp_text(text, author);
            let text = if text.trim().is_empty() {
                "-".to_string()
            } else {
                text.trim().to_string()
            };
            Some(StampLabel {
                text,
                color: *color,
                shape: *shape,
            })
        }
        _ => None,
    }
}

/// `rect` shrunk to `aspect` (width ÷ height), centred — the letterbox that keeps a picked
/// image undistorted when the placement rectangle has another shape (v0.3 T1).
pub fn letterbox(rect: Rect, aspect: f32) -> Rect {
    let (w, h) = (rect.width(), rect.height());
    if !(aspect.is_finite() && aspect > 0.0) || w <= 0.0 || h <= 0.0 {
        return rect;
    }
    let (fw, fh) = if w / h > aspect {
        (h * aspect, h)
    } else {
        (w, w / aspect)
    };
    let (cx, cy) = ((rect.l + rect.r) / 2.0, (rect.b + rect.t) / 2.0);
    Rect::new(cx - fw / 2.0, cy - fh / 2.0, cx + fw / 2.0, cy + fh / 2.0)
}

/// An image stamp (PNG or JPEG), a built-in or text label stamp, or a quick mark.
fn stamp(
    doc: &OpenDoc<'_>,
    page: &mut PdfPage<'_>,
    spec: &StampSpec,
    id: &str,
    label: Option<(StampLabel, PdfFontToken)>,
) -> Result<(), EngineError> {
    let bindings = doc.bindings();
    let pdf_rect = to_pdf_rect(spec.rect);
    let mut subj: Option<String> = None;
    let mut said: Option<String> = None;
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
                // Safety net (v0.3 T1): the UI sizes the rect to the image's aspect, but a
                // rect of another shape letterboxes the image instead of stretching it.
                let aspect = image.width() as f32 / image.height().max(1) as f32;
                let fit = letterbox(spec.rect, aspect);
                let mut obj = PdfPageImageObject::new_with_size(
                    doc.pdf(),
                    &image,
                    points(fit.width()),
                    points(fit.height()),
                )
                .ctx("stamp image object")?;
                obj.translate(points(fit.l), points(fit.b))
                    .ctx("place stamp image")?;
                a.objects_mut()
                    .add_image_object(obj)
                    .ctx("append stamp image")?;
            }
            StampImage::Builtin { builtin } if QUICK_MARKS.contains(&builtin.as_str()) => {
                let mark = quick_mark(doc, spec.rect, builtin)?;
                a.objects_mut()
                    .add_path_object(mark)
                    .ctx("append quick mark")?;
                subj = Some(builtin.clone());
            }
            StampImage::Builtin { .. } | StampImage::Text { .. } => {
                let (label, token) = label
                    .ok_or_else(|| EngineError::invalid("a label stamp needs a label font"))?;
                // A Korean seal (결재 / 승인 / 기밀) gets a heavier border, like 인주 on paper.
                let border_width = if fonts::is_latin1(&label.text) {
                    2.0
                } else {
                    2.5
                };
                if let Some(border) = stamp_border(doc, spec.rect, &label, border_width)? {
                    a.objects_mut()
                        .add_path_object(border)
                        .ctx("append stamp border")?;
                }
                // Half the height, then shrunk until it fits the width: a square 결재 seal
                // and a wide APPROVED banner both keep the label inside the border.
                let mut size = (spec.rect.height() * 0.5).clamp(6.0, 48.0);
                let mut t = PdfPageTextObject::new(doc.pdf(), &label.text, token, points(size))
                    .ctx("stamp label")?;
                let max_width = spec.rect.width() * STAMP_LABEL_MAX_WIDTH;
                let width = text_width(&t);
                if width > max_width && width > 0.0 {
                    size = (size * max_width / width).max(4.0);
                    t = PdfPageTextObject::new(doc.pdf(), &label.text, token, points(size))
                        .ctx("stamp label")?;
                }
                t.set_fill_color(rgb(label.color, 255))
                    .ctx("stamp colour")?;
                // Centre the glyphs' own box, not the em box: Hangul sits higher on the
                // baseline than Latin capitals, and `size` alone would put 결재 low.
                let (left, bottom, right, top) = t
                    .bounds()
                    .map(|b| {
                        (
                            b.left().value,
                            b.bottom().value,
                            b.right().value,
                            b.top().value,
                        )
                    })
                    .unwrap_or((0.0, 0.0, 0.0, size));
                t.translate(
                    points(spec.rect.l + (spec.rect.width() - (right - left)) / 2.0 - left),
                    points(spec.rect.b + (spec.rect.height() - (top - bottom)) / 2.0 - bottom),
                )
                .ctx("place stamp label")?;
                a.objects_mut()
                    .add_text_object(t)
                    .ctx("append stamp label")?;
                subj = Some(match &spec.image {
                    StampImage::Builtin { builtin } => builtin.clone(),
                    _ => {
                        said = Some(label.text.clone());
                        SUBJ_TEXT_STAMP.to_string()
                    }
                });
            }
        }
    }
    let index = raw::annot::count(bindings, page).saturating_sub(1);
    let mut a = raw::annot::get(bindings, page, index)?;
    if let Some(rotate) = spec.rotate {
        // Recorded for the UI; PDFium has no annotation rotation entry of its own.
        a.set_string("SeePDFRotate", &format!("{rotate}"));
    }
    if let Some(text) = &said {
        // A text stamp says what it says in `/Contents` too: other viewers' comment lists
        // and the 주석 목록 export read it there.
        a.set_string("Contents", text);
    }
    // Stage 8: a typed / image signature is a Stamp tagged `SeePDF:Signature`, which
    // `kind_of` reads back as `signature` (서명), like the drawn one (Ink + the same tag).
    if spec.signature {
        subj = Some(SUBJ_SIGNATURE.to_string());
    }
    write_identity(&mut a, id, subj.as_deref());
    Ok(())
}

/// The border of a label stamp: a rectangle, a rounded rectangle, or nothing.
fn stamp_border<'a>(
    doc: &'a OpenDoc<'_>,
    rect: Rect,
    label: &StampLabel,
    width: f32,
) -> Result<Option<PdfPagePathObject<'a>>, EngineError> {
    let stroke = Some(rgb(label.color, 255));
    match label.shape {
        StampShape::None => Ok(None),
        StampShape::Rect => Ok(Some(
            PdfPagePathObject::new_rect(
                doc.pdf(),
                to_pdf_rect(rect),
                stroke,
                Some(points(width)),
                None,
            )
            .ctx("stamp border")?,
        )),
        StampShape::Round => {
            // Inset by half the stroke so the rounded border stays inside the /Rect.
            let inset = width / 2.0;
            let (l, b, r, t) = (
                rect.l + inset,
                rect.b + inset,
                rect.r - inset,
                rect.t - inset,
            );
            let radius = ((t - b) / 2.0).min((r - l) / 2.0).max(0.0);
            // Bezier quarter circles.
            let k = radius * 0.552_284_8;
            let mut p = PdfPagePathObject::new(
                doc.pdf(),
                points(l + radius),
                points(b),
                stroke,
                Some(points(width)),
                None,
            )
            .ctx("stamp border")?;
            p.line_to(points(r - radius), points(b)).ctx("border")?;
            p.bezier_to(
                points(r),
                points(b + radius),
                points(r - radius + k),
                points(b),
                points(r),
                points(b + radius - k),
            )
            .ctx("border")?;
            p.line_to(points(r), points(t - radius)).ctx("border")?;
            p.bezier_to(
                points(r - radius),
                points(t),
                points(r),
                points(t - radius + k),
                points(r - radius + k),
                points(t),
            )
            .ctx("border")?;
            p.line_to(points(l + radius), points(t)).ctx("border")?;
            p.bezier_to(
                points(l),
                points(t - radius),
                points(l + radius - k),
                points(t),
                points(l),
                points(t - radius + k),
            )
            .ctx("border")?;
            p.line_to(points(l), points(b + radius)).ctx("border")?;
            p.bezier_to(
                points(l + radius),
                points(b),
                points(l),
                points(b + radius - k),
                points(l + radius - k),
                points(b),
            )
            .ctx("border")?;
            p.close_path().ctx("border")?;
            Ok(Some(p))
        }
    }
}

/// ✓ ✗ ● as one path object inside `rect` (v0.3 T2): a stroked check / cross, a filled dot.
fn quick_mark<'a>(
    doc: &'a OpenDoc<'_>,
    rect: Rect,
    mark: &str,
) -> Result<PdfPagePathObject<'a>, EngineError> {
    let colour = rgb(QUICK_MARK_COLOR, 255);
    let side = rect.width().min(rect.height());
    let (cx, cy) = ((rect.l + rect.r) / 2.0, (rect.b + rect.t) / 2.0);
    let (x0, y0) = (cx - side / 2.0, cy - side / 2.0);
    let at = |u: f32, v: f32| (points(x0 + u * side), points(y0 + v * side));
    let width = Some(points((side * 0.12).max(1.0)));
    let path = match mark {
        "dot" => PdfPagePathObject::new_circle(
            doc.pdf(),
            to_pdf_rect(Rect::new(
                cx - side * 0.3,
                cy - side * 0.3,
                cx + side * 0.3,
                cy + side * 0.3,
            )),
            None,
            None,
            Some(colour),
        )
        .ctx("quick mark")?,
        "cross" => {
            let (x, y) = at(0.15, 0.15);
            let mut p = PdfPagePathObject::new(doc.pdf(), x, y, Some(colour), width, None)
                .ctx("quick mark")?;
            let (x, y) = at(0.85, 0.85);
            p.line_to(x, y).ctx("quick mark")?;
            let (x, y) = at(0.15, 0.85);
            p.move_to(x, y).ctx("quick mark")?;
            let (x, y) = at(0.85, 0.15);
            p.line_to(x, y).ctx("quick mark")?;
            p
        }
        _ => {
            let (x, y) = at(0.12, 0.52);
            let mut p = PdfPagePathObject::new(doc.pdf(), x, y, Some(colour), width, None)
                .ctx("quick mark")?;
            let (x, y) = at(0.4, 0.2);
            p.line_to(x, y).ctx("quick mark")?;
            let (x, y) = at(0.88, 0.85);
            p.line_to(x, y).ctx("quick mark")?;
            p
        }
    };
    Ok(path)
}

/// The colour of a built-in stamp. The Korean set is 인주 red, whatever it says.
pub fn builtin_color(name: &str) -> Rgb {
    match name.to_ascii_lowercase().as_str() {
        "결재" | "승인" | "기밀" => KOREAN_SEAL_RED,
        "approved" | "final" | "completed" => [0, 140, 60],
        "draft" | "forcomment" | "notapproved" => [200, 120, 0],
        "confidential" | "urgent" | "void" => [200, 0, 0],
        _ => [40, 70, 160],
    }
}

/// 인주 red of the Korean stamp set (결재 / 승인 / 기밀). The UI's `stampCatalog.ts` uses the
/// same value for its preview.
pub const KOREAN_SEAL_RED: Rgb = [206, 32, 41];

fn text_width(t: &PdfPageTextObject<'_>) -> f32 {
    t.bounds()
        .map(|b| b.right().value - b.left().value)
        .unwrap_or(0.0)
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
    PdfRect::new_from_values(r.b.min(r.t), r.l.min(r.r), r.b.max(r.t), r.l.max(r.r))
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
            assert!(
                stroke[2] < 100.0,
                "barb x {} should be left of the tip",
                stroke[2]
            );
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
