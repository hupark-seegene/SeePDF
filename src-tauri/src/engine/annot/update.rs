//! `update_annotation` — `IPC_CONTRACT.md` §7.1.
//!
//! The order is **`SetAP(NULL)` first**, then the setters, then let the next render
//! regenerate the appearance (`ARCHITECTURE.md` §6.1). Without it `FPDFAnnot_SetColor`
//! returns false on every annotation that has been rendered or read from disk, and
//! pdfium-render's high-level fallback segfaults (annotations spike §5.1).
//!
//! **Stamps are the exception**: their appearance stream *is* their content — the image and
//! the text objects live inside it, put there by `FPDFAnnot_AppendObject` — and PDFium never
//! regenerates a Stamp appearance. Clearing it would erase the annotation, so a Stamp-backed
//! kind (stamp, image signature, text box) keeps its AP and changes only `/Rect`, the
//! dictionary strings and the flags in place; anything visual goes through the rebuild below.
//!
//! Two patches cannot be applied in place and are done as **delete + re-create with the same
//! `/NM`**:
//!
//! * a markup `rects` list of a different length — PDFium can append and replace quads but
//!   has no way to remove one;
//! * a text box's `text` / `fontSize` / `color` / `fillColor` — the glyphs are baked page
//!   objects inside the stamp's appearance stream and `FPDFAnnot_UpdateObject` is never
//!   called by pdfium-render, so they have to be rebuilt.

use crate::engine::annot::{self, create, read, ScratchPage};
use crate::engine::raw::{self, annot::ColorKind, consts};
use crate::engine::registry::OpenDoc;
use crate::ipc::types::{
    Annot, AnnotId, AnnotKind, AnnotPatch, AnnotSpec, InkSpec, LineSpec, MarkupSpec, PageIndex,
    Rect, ShapeSpec, TextAlign, TextBoxSpec,
};
use crate::ipc::EngineError;

/// Applies `patch` to the annotation `id` on `page_index` and returns its previous state.
pub fn update(
    doc: &mut OpenDoc<'_>,
    page_index: PageIndex,
    id: &AnnotId,
    patch: &AnnotPatch,
) -> Result<Annot, EngineError> {
    let previous = find(doc, page_index, id)?;
    if needs_rebuild(&previous, patch) {
        rebuild(doc, page_index, &previous, patch)?;
    } else {
        in_place(doc, page_index, id, patch)?;
    }
    doc.touched.insert(page_index);
    doc.annots.remove(&page_index);
    Ok(previous)
}

/// The annotation as it is now, for `AnnotResult::previous` and for the rebuild path.
pub fn find(
    doc: &mut OpenDoc<'_>,
    page_index: PageIndex,
    id: &AnnotId,
) -> Result<Annot, EngineError> {
    let bindings = doc.bindings();
    let document = doc.pdf().raw_handle();
    let scratch = ScratchPage::open(doc, page_index)?;
    let index = annot::index_of(bindings, &scratch.page, id)?;
    let a = raw::annot::get(bindings, &scratch.page, index)?;
    let out = read::read_one(&a, id.clone(), page_index, document);
    drop(a);
    drop(scratch);
    Ok(out)
}

fn needs_rebuild(previous: &Annot, patch: &AnnotPatch) -> bool {
    if read::is_markup(previous.kind) {
        if let Some(rects) = &patch.rects {
            let current = previous.quads.as_ref().map(|q| q.len()).unwrap_or(0);
            if rects.len() != current {
                return true;
            }
        }
    }
    if previous.kind == AnnotKind::Textbox {
        return patch.text.is_some()
            || patch.font_size.is_some()
            || patch.color.is_some()
            || patch.fill_color.is_some();
    }
    false
}

fn in_place(
    doc: &mut OpenDoc<'_>,
    page_index: PageIndex,
    id: &AnnotId,
    patch: &AnnotPatch,
) -> Result<(), EngineError> {
    let bindings = doc.bindings();
    let scratch = ScratchPage::open(doc, page_index)?;
    let index = annot::index_of(bindings, &scratch.page, id)?;
    let mut a = raw::annot::get(bindings, &scratch.page, index)?;
    let subtype = a.subtype();
    let kind = annot::kind_of(subtype, a.string("Subj").as_deref());
    // A Stamp's appearance stream **is** its content: the image and the text objects live
    // inside it, put there by `FPDFAnnot_AppendObject`, and PDFium never regenerates a Stamp
    // appearance. Clearing it would erase the annotation. Everything else gets its appearance
    // back on the next render, which is why the AP goes first there.
    let stamp_backed = subtype == consts::FPDF_ANNOT_STAMP;
    // A reply (P2 threads) is drawn by nobody: its appearance is an empty stream, so the note
    // icon PDFium would generate never lands on top of its parent. It is written back below.
    let reply = read::in_reply_to(&a).is_some();
    if !stamp_backed {
        a.clear_ap();
    }

    // Opacity is the alpha of the last SetColor call, so colour and opacity are one step.
    let alpha = patch
        .opacity
        .map(|o| (o.clamp(0.0, 1.0) * 255.0).round() as u8)
        .or_else(|| a.opacity().map(|o| (o * 255.0).round() as u8))
        .unwrap_or(255);
    let stroke = patch
        .color
        .or_else(|| annot::read_color_key(&a, annot::KEY_COLOR))
        .or_else(|| a.color(ColorKind::Stroke).map(|(c, _)| c));
    let current_fill = annot::read_color_key(&a, annot::KEY_FILL)
        .or_else(|| a.color(ColorKind::Interior).map(|(c, _)| c));
    let fill = match patch.fill_color {
        Some(v) => v,
        None => current_fill,
    };
    if !stamp_backed {
        if let Some(c) = stroke {
            a.set_color(ColorKind::Stroke, c, alpha);
        }
        match fill {
            Some(fill) => {
                // The same alpha for /C and /IC: every SetColor call rewrites /CA.
                a.set_color(ColorKind::Interior, fill, alpha);
            }
            None => {
                // PDFium has no "remove /IC"; a fully transparent interior is the closest.
                a.set_color(ColorKind::Interior, [255, 255, 255], 0);
            }
        }
        if let Some(w) = patch.border_width {
            a.set_border_width(w);
        }
    }
    if let Some(c) = stroke {
        // The mirror must follow, or the next read (after the AP is regenerated) is stale.
        annot::record_colors(&mut a, c, fill);
    }

    // Geometry.
    if let Some(rects) = &patch.rects {
        for (i, r) in rects.iter().enumerate() {
            a.set_quad(i, *r);
        }
        a.set_rect(
            rects
                .iter()
                .copied()
                .reduce(|x, y| x.union(&y))
                .unwrap_or(Rect::ZERO),
        );
    }
    if let Some(paths) = &patch.paths {
        a.remove_ink_list();
        let width = patch
            .border_width
            .or_else(|| a.border_width())
            .unwrap_or(1.0);
        for path in paths {
            if path.len() >= 4 {
                a.add_ink_stroke(path)?;
            }
        }
        a.set_rect(ink_bounds(paths, width));
    }
    if matches!(kind, AnnotKind::Line | AnnotKind::Arrow)
        && (patch.p1.is_some() || patch.p2.is_some())
    {
        let current = a.ink_paths();
        let first = current.first().cloned().unwrap_or_default();
        let p1 = patch.p1.unwrap_or_else(|| {
            [
                first.first().copied().unwrap_or(0.0),
                first.get(1).copied().unwrap_or(0.0),
            ]
        });
        let p2 = patch.p2.unwrap_or_else(|| {
            [
                first
                    .get(first.len().wrapping_sub(2))
                    .copied()
                    .unwrap_or(0.0),
                first.last().copied().unwrap_or(0.0),
            ]
        });
        let width = patch
            .border_width
            .or_else(|| a.border_width())
            .unwrap_or(1.0);
        let arrow = kind == AnnotKind::Arrow;
        let paths = crate::engine::annot::update::line_paths(p1, p2, width, arrow, current.len());
        a.remove_ink_list();
        for path in &paths {
            a.add_ink_stroke(path)?;
        }
        a.set_rect(ink_bounds(&paths, width));
    }
    if let Some(rect) = patch.rect {
        // Last, so it wins over a bounds computed from the geometry above.
        a.set_rect(rect);
    }

    // Text properties.
    if let Some(c) = &patch.contents {
        a.set_string("Contents", c);
    }
    if let Some(t) = &patch.author {
        a.set_string("T", t);
    }
    if let Some(locked) = patch.locked {
        let flags = a.flags();
        a.set_flags(if locked {
            flags | consts::FPDF_ANNOT_FLAG_LOCKED
        } else {
            flags & !consts::FPDF_ANNOT_FLAG_LOCKED
        });
    }
    a.set_string("M", &annot::pdf_date_now());
    if reply && !stamp_backed {
        a.set_ap("");
    }
    drop(a);
    drop(scratch);
    Ok(())
}

/// Delete + re-create with the same `/NM` (see the module docs for when this is needed).
fn rebuild(
    doc: &mut OpenDoc<'_>,
    page_index: PageIndex,
    previous: &Annot,
    patch: &AnnotPatch,
) -> Result<(), EngineError> {
    let spec = respec(previous, patch)?;
    // No reply cascade: the re-created annotation keeps its `/NM`, so its thread stays attached.
    annot::delete_with(doc, page_index, std::slice::from_ref(&previous.id), false)?;
    create::create(doc, page_index, &spec, Some(previous.id.clone()))?;
    // Contents and author are not part of every spec, so re-apply what the patch did not set.
    // A text box is the exception: `create` already wrote `/Contents` from the merged text,
    // and restoring the *previous* contents here would undo the edit.
    let contents = if previous.kind == AnnotKind::Textbox {
        None
    } else {
        patch.contents.clone().or(Some(previous.contents.clone()))
    };
    let author = patch.author.clone().or_else(|| previous.author.clone());
    if contents.is_some() || author.is_some() {
        in_place(
            doc,
            page_index,
            &previous.id,
            &AnnotPatch {
                contents,
                author,
                ..AnnotPatch::default()
            },
        )?;
    }
    Ok(())
}

/// Rebuilds the `AnnotSpec` that would produce `previous` with `patch` applied.
fn respec(previous: &Annot, patch: &AnnotPatch) -> Result<AnnotSpec, EngineError> {
    let color = patch.color.unwrap_or(previous.color);
    let opacity = patch.opacity.unwrap_or(previous.opacity);
    let width = patch.border_width.unwrap_or(previous.border_width);
    let fill_color = match patch.fill_color {
        Some(v) => v,
        None => previous.fill_color,
    };
    let rect = patch.rect.unwrap_or(previous.rect);
    match previous.kind {
        AnnotKind::Highlight
        | AnnotKind::Underline
        | AnnotKind::Strikeout
        | AnnotKind::Squiggly => {
            let rects = patch
                .rects
                .clone()
                .or_else(|| previous.quads.clone())
                .unwrap_or_else(|| vec![previous.rect]);
            let markup = MarkupSpec {
                rects,
                color,
                opacity,
                contents: patch.contents.clone().or(Some(previous.contents.clone())),
            };
            Ok(match previous.kind {
                AnnotKind::Highlight => AnnotSpec::Highlight(markup),
                AnnotKind::Underline => AnnotSpec::Underline(markup),
                AnnotKind::Strikeout => AnnotSpec::Strikeout(markup),
                _ => AnnotSpec::Squiggly(markup),
            })
        }
        AnnotKind::Textbox => Ok(AnnotSpec::Textbox(TextBoxSpec {
            rect,
            text: patch
                .text
                .clone()
                .or_else(|| previous.text.clone())
                .unwrap_or_else(|| previous.contents.clone()),
            font_size: patch.font_size.or(previous.font_size).unwrap_or(12.0),
            color,
            align: TextAlign::Left,
            fill_color,
        })),
        AnnotKind::Square => Ok(AnnotSpec::Square(ShapeSpec {
            rect,
            color,
            fill_color,
            width,
            opacity,
        })),
        AnnotKind::Circle => Ok(AnnotSpec::Circle(ShapeSpec {
            rect,
            color,
            fill_color,
            width,
            opacity,
        })),
        AnnotKind::Ink | AnnotKind::Signature => Ok(AnnotSpec::Ink(InkSpec {
            paths: patch
                .paths
                .clone()
                .or_else(|| previous.ink_paths.clone())
                .unwrap_or_default(),
            color,
            width,
            opacity,
        })),
        AnnotKind::Line | AnnotKind::Arrow => {
            let pts = previous.line_points.unwrap_or([0.0, 0.0, 0.0, 0.0]);
            let spec = LineSpec {
                p1: patch.p1.unwrap_or([pts[0], pts[1]]),
                p2: patch.p2.unwrap_or([pts[2], pts[3]]),
                color,
                width,
                opacity,
                heads: None,
            };
            Ok(if previous.kind == AnnotKind::Arrow {
                AnnotSpec::Arrow(spec)
            } else {
                AnnotSpec::Line(spec)
            })
        }
        other => Err(EngineError::unsupported(&format!(
            "rebuilding a {other:?} annotation"
        ))),
    }
}

/// The ink strokes of a line / arrow after its endpoints moved: the segment itself, plus
/// the same arrow heads the old geometry had (1 stroke = bare line, 3 = one head, 5 = two).
pub(crate) fn line_paths(
    p1: [f32; 2],
    p2: [f32; 2],
    width: f32,
    arrow: bool,
    previous_stroke_count: usize,
) -> Vec<Vec<f32>> {
    let heads = match previous_stroke_count {
        5 => [true, true],
        3 => [false, true],
        1 => [false, false],
        _ => [false, arrow],
    };
    let mut paths = vec![vec![p1[0], p1[1], p2[0], p2[1]]];
    if heads[1] {
        paths.extend(create::arrow_head(p2, p1, width));
    }
    if heads[0] {
        paths.extend(create::arrow_head(p1, p2, width));
    }
    paths
}

fn ink_bounds(paths: &[Vec<f32>], width: f32) -> Rect {
    let pad = (width / 2.0).max(1.0);
    let mut acc: Option<Rect> = None;
    for path in paths {
        for p in path.chunks_exact(2) {
            let point = Rect::new(p[0], p[1], p[0], p[1]);
            acc = Some(match acc {
                Some(r) => r.union(&point),
                None => point,
            });
        }
    }
    let r = acc.unwrap_or(Rect::ZERO);
    Rect::new(r.l - pad, r.b - pad, r.r + pad, r.t + pad)
}
