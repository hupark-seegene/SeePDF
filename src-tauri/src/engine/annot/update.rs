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

use crate::engine::annot::lopdf_annots::{self, DictEdits, Drawn, Shape};
use crate::engine::annot::{self, create, read, ScratchPage};
use crate::engine::raw::{self, annot::ColorKind, consts};
use crate::engine::registry::{self, MutateOpts, OpenDoc};
use crate::engine::types::EngineState;
use crate::ipc::types::{
    Annot, AnnotId, AnnotKind, AnnotOp, AnnotPatch, AnnotSpec, CalloutSpec, ChangeReason, InkSpec,
    LineSpec, MarkupSpec, PageIndex, Rect, ShapeSpec, TextAlign, TextBoxSpec,
};
use crate::ipc::{EngineError, ErrorCode};

/// `update_annotation` as one undoable edit (v0.3). The PDFium kinds go through
/// `registry::mutate` + [`update`]; the lopdf kinds (a real `/Line`, `/Polygon`, `/PolyLine`)
/// are redrawn in place by [`lopdf_annots::rewrite`]; a callout is rebuilt as a text box and
/// turned back into a callout (a coalesced second step); a dashed border (`/BS`, v0.3 A6) is
/// written by lopdf after the PDFium part, also coalesced — one undo entry each time.
pub fn update_in(
    st: &mut EngineState<'_>,
    doc_id: &str,
    page: PageIndex,
    id: &AnnotId,
    patch: &AnnotPatch,
) -> Result<Annot, EngineError> {
    let opts = || {
        MutateOpts::new("undo.annotEdit", ChangeReason::Edit)
            .page(page)
            .keeps_text()
    };
    let (previous, rewritable) = {
        let doc = st.doc_mut(doc_id)?;
        let previous = find(doc, page, id)?;
        (previous, !(doc.encrypted || doc.password.is_some()))
    };
    let lopdf_kind = is_lopdf_kind(&previous);
    let needs_lopdf = lopdf_kind
        || previous.kind == AnnotKind::Callout
        || patch.dashed.is_some_and(|d| d != previous.dashed)
        // a new width, or new heads (a SeePDF Ink line is rebuilt by PDFium), re-applies the dash
        || (previous.dashed && (patch.border_width.is_some() || patch.heads.is_some()));
    if needs_lopdf && !rewritable {
        return Err(EngineError::new(
            ErrorCode::Unsupported,
            "this change cannot be written into an encrypted document",
        ));
    }
    if lopdf_kind {
        let edits = DictEdits {
            contents: patch.contents.clone(),
            author: patch.author.clone(),
            locked: patch.locked,
            printed: patch.printed,
        };
        let ours = previous.editable == crate::ipc::types::Editability::Full;
        if ours {
            let drawn = drawn_after(&previous, patch)?;
            registry::mutate_bytes(st, doc_id, opts(), |bytes, _| {
                lopdf_annots::rewrite(bytes, page, id, &drawn, &edits)
            })?;
        } else {
            // Another application's line / polygon: its appearance is kept and only follows
            // the new `/Rect` (moveOnly); the geometry keys move with it.
            let m = rect_matrix(previous.rect, patch.rect.unwrap_or(previous.rect));
            registry::mutate_bytes(st, doc_id, opts(), |bytes, _| {
                lopdf_annots::move_foreign(bytes, page, id, m, &edits)
            })?;
        }
        return Ok(previous);
    }
    if previous.kind == AnnotKind::Callout {
        registry::mutate(st, doc_id, opts(), |doc| {
            rebuild(doc, page, &previous, patch)?;
            doc.touched.insert(page);
            doc.annots.remove(&page);
            Ok(())
        })?;
        let tip = match (&patch.callout, patch.rect) {
            (Some(line), _) => line.clone(),
            // A resized / moved box without a new leader: the tip stays, the leader re-attaches.
            (None, Some(rect)) => reattach(previous.callout.as_deref().unwrap_or_default(), rect),
            (None, None) => previous.callout.clone().unwrap_or_default(),
        };
        let color = patch.color.unwrap_or(previous.color);
        st.doc_mut(doc_id)?.history.refresh_last();
        registry::mutate_bytes(st, doc_id, opts().coalesced(), |bytes, _| {
            lopdf_annots::to_callout(bytes, page, id, &tip, color, create::CALLOUT_WIDTH)
        })?;
        return Ok(previous);
    }
    let previous = registry::mutate(st, doc_id, opts(), |doc| update(doc, page, id, patch))?;
    if needs_lopdf {
        let dashed = patch.dashed.unwrap_or(previous.dashed);
        let width = patch.border_width.unwrap_or(previous.border_width);
        st.doc_mut(doc_id)?.history.refresh_last();
        registry::mutate_bytes(st, doc_id, opts().coalesced(), |bytes, _| {
            lopdf_annots::set_border_style(bytes, page, id, width, dashed)
        })?;
        st.doc_mut(doc_id)?.touched.insert(page);
    }
    Ok(previous)
}

/// A callout leader after its box moved to `rect`: the tip (and a knee) stay, the end moves to
/// the middle of the box side nearer the tip.
pub fn reattach(callout: &[f32], rect: Rect) -> Vec<f32> {
    if callout.len() < 4 {
        return callout.to_vec();
    }
    let tip_x = callout[0];
    let x = if tip_x <= (rect.l + rect.r) / 2.0 {
        rect.l
    } else {
        rect.r
    };
    let mut out = callout[..callout.len() - 2].to_vec();
    out.extend_from_slice(&[x, (rect.b + rect.t) / 2.0]);
    out
}

/// The kinds whose geometry only lopdf can write: a real `/Line` (not SeePDF's Ink line), a
/// `/Polygon`, a `/PolyLine`.
pub fn is_lopdf_kind(a: &Annot) -> bool {
    matches!(a.subtype.as_str(), "Line" | "Polygon" | "PolyLine")
}

/// The matrix that maps `from` onto `to` (scale about the origin, then translate).
fn rect_matrix(from: Rect, to: Rect) -> [f32; 6] {
    let sx = if from.width().abs() > 1e-3 {
        to.width() / from.width()
    } else {
        1.0
    };
    let sy = if from.height().abs() > 1e-3 {
        to.height() / from.height()
    } else {
        1.0
    };
    [sx, 0.0, 0.0, sy, to.l - from.l * sx, to.b - from.b * sy]
}

/// What a lopdf annotation looks like after `patch`.
fn drawn_after(previous: &Annot, patch: &AnnotPatch) -> Result<Drawn, EngineError> {
    let pts = previous.line_points.unwrap_or([0.0; 4]);
    let shape = match previous.kind {
        AnnotKind::Line | AnnotKind::Arrow => Shape::Line {
            p1: patch.p1.unwrap_or([pts[0], pts[1]]),
            p2: patch.p2.unwrap_or([pts[2], pts[3]]),
            heads: patch
                .heads
                .or(previous.heads)
                .unwrap_or([false, previous.kind == AnnotKind::Arrow]),
        },
        AnnotKind::Polygon => Shape::Polygon {
            vertices: patch
                .vertices
                .clone()
                .or_else(|| previous.vertices.clone())
                .unwrap_or_default(),
            cloudy: previous.cloudy,
        },
        AnnotKind::Polyline => Shape::Polyline {
            vertices: patch
                .vertices
                .clone()
                .or_else(|| previous.vertices.clone())
                .unwrap_or_default(),
        },
        other => {
            return Err(EngineError::unsupported(&format!(
                "redrawing a {other:?} annotation with lopdf"
            )))
        }
    };
    Ok(Drawn {
        shape,
        color: patch.color.unwrap_or(previous.color),
        fill: match patch.fill_color {
            Some(v) => v,
            None => previous.fill_color,
        },
        width: patch.border_width.unwrap_or(previous.border_width),
        opacity: patch.opacity.unwrap_or(previous.opacity),
        dashed: patch.dashed.unwrap_or(previous.dashed),
        measure: previous.measure,
    })
}

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
            || patch.fill_color.is_some()
            || patch.align.is_some_and(|a| Some(a) != previous.align);
    }
    // v0.3 A1: new heads on a SeePDF ink line = new strokes and maybe a new `/Subj`.
    if matches!(previous.kind, AnnotKind::Line | AnnotKind::Arrow) {
        return patch.heads.is_some_and(|h| Some(h) != previous.heads);
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
        let heads = a
            .string(annot::KEY_HEADS)
            .as_deref()
            .and_then(read::parse_heads)
            .unwrap_or_else(|| read::heads_from_strokes(&current, kind == AnnotKind::Arrow));
        let paths = line_paths(p1, p2, width, heads);
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
    // v0.3 A10: 인쇄 — the `/F` Print bit.
    if let Some(printed) = patch.printed {
        let flags = a.flags();
        a.set_flags(if printed {
            flags | consts::FPDF_ANNOT_FLAG_PRINT
        } else {
            flags & !consts::FPDF_ANNOT_FLAG_PRINT
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
    // The re-created annotation has the default flags (Print): the old ones come back.
    let printed = patch.printed.unwrap_or(previous.printed);
    let locked = patch.locked.unwrap_or(previous.locked);
    in_place(
        doc,
        page_index,
        &previous.id,
        &AnnotPatch {
            contents,
            author,
            printed: (!printed).then_some(false),
            locked: locked.then_some(true),
            ..AnnotPatch::default()
        },
    )?;
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
            // v0.3 A1: the patch, then what it was, then left.
            align: patch.align.or(previous.align).unwrap_or(TextAlign::Left),
            fill_color,
        })),
        AnnotKind::Callout => Ok(AnnotSpec::Callout(CalloutSpec {
            rect,
            text: patch
                .text
                .clone()
                .or_else(|| previous.text.clone())
                .unwrap_or_else(|| previous.contents.clone()),
            font_size: patch.font_size.or(previous.font_size).unwrap_or(12.0),
            color,
            align: patch.align.or(previous.align).unwrap_or(TextAlign::Left),
            fill_color,
            callout: patch
                .callout
                .clone()
                .or_else(|| previous.callout.clone())
                .unwrap_or_default(),
        })),
        AnnotKind::Square => Ok(AnnotSpec::Square(ShapeSpec {
            rect,
            color,
            fill_color,
            width,
            opacity,
            // `rebuild` runs inside a PDFium mutate: `create` cannot write `/BS` there.
            dashed: false,
        })),
        AnnotKind::Circle => Ok(AnnotSpec::Circle(ShapeSpec {
            rect,
            color,
            fill_color,
            width,
            opacity,
            // `rebuild` runs inside a PDFium mutate: `create` cannot write `/BS` there.
            dashed: false,
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
            let heads = patch
                .heads
                .or(previous.heads)
                .unwrap_or([false, previous.kind == AnnotKind::Arrow]);
            let spec = LineSpec {
                p1: patch.p1.unwrap_or([pts[0], pts[1]]),
                p2: patch.p2.unwrap_or([pts[2], pts[3]]),
                color,
                width,
                opacity,
                heads: Some(heads),
                measure: previous.measure,
                // a rebuilt SeePDF Ink line (PDFium): `/BS` is lopdf's, see `update_in`
                dashed: false,
            };
            // v0.3 A1: any head makes it an arrow (`/Subj` SeePDF:Arrow), none a line.
            Ok(if heads[0] || heads[1] {
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
/// the arrow heads it had (the `SeePDFHeads` mirror, or read off the old strokes).
pub(crate) fn line_paths(
    p1: [f32; 2],
    p2: [f32; 2],
    width: f32,
    heads: [bool; 2],
) -> Vec<Vec<f32>> {
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

/// v0.3 A3 / A4 `annotation_batch`: `ops` in order on `page`, folded into **one** undo step
/// (`undo.annotCreate` when every op creates, `undo.annotDelete` when every op deletes,
/// `undo.annotEdit` otherwise) — a partial-eraser scrub, a pen stroke split into pressure
/// bands. All or nothing: when an op fails, the ones before it are undone (and not offered
/// as a redo) and the error is returned. Returns the `/NM` of each created annotation.
pub fn batch_in(
    st: &mut EngineState<'_>,
    doc_id: &str,
    page: PageIndex,
    ops: &[AnnotOp],
    author: Option<&str>,
) -> Result<Vec<AnnotId>, EngineError> {
    if ops.is_empty() {
        return Err(EngineError::invalid("ops is empty"));
    }
    let label = if ops.iter().all(|o| matches!(o, AnnotOp::Create { .. })) {
        "undo.annotCreate"
    } else if ops.iter().all(|o| matches!(o, AnnotOp::Delete { .. })) {
        "undo.annotDelete"
    } else {
        "undo.annotEdit"
    };
    // Held trimming + a squash after every op: the batch is one entry the whole time, so
    // depth trimming (3 on a large document, 50 otherwise) cannot drop its pre-batch
    // snapshot and the rollback below undoes the whole batch.
    let mark = st.doc_mut(doc_id)?.history.begin_batch();
    let mut created = Vec::new();
    let mut failed = None;
    for op in ops {
        let done = match op {
            AnnotOp::Create { spec, id } => {
                create::create_in(st, doc_id, page, spec, id.clone(), author).map(|id| {
                    created.push(id);
                })
            }
            AnnotOp::Update { id, patch } => update_in(st, doc_id, page, id, patch).map(|_| ()),
            AnnotOp::Delete { ids } => registry::mutate(
                st,
                doc_id,
                MutateOpts::new("undo.annotDelete", ChangeReason::Edit)
                    .page(page)
                    .keeps_text(),
                |doc| annot::delete(doc, page, ids),
            )
            .map(|_| ()),
        };
        if let Ok(d) = st.doc_mut(doc_id) {
            d.history.squash_since(mark, label);
        }
        if let Err(e) = done {
            failed = Some(e);
            break;
        }
    }
    let Some(e) = failed else {
        st.doc_mut(doc_id)?.history.end_batch(mark, label);
        return Ok(created);
    };
    // Roll back before trimming resumes: on a depth-3 document the batch entry would
    // otherwise push the oldest pre-batch entry out for an edit that is then undone.
    let stored = st.doc_mut(doc_id)?.history.squash_since(mark, label);
    let rolled = if stored {
        registry::undo(st, doc_id, false).map(|_| ())
    } else {
        Ok(())
    };
    let history = &mut st.doc_mut(doc_id)?.history;
    if stored && rolled.is_ok() {
        history.discard_last_redo();
    }
    history.close_batch();
    rolled.map_err(|u| {
        e.clone()
            .with_detail(format!("rollback failed: {}", u.message))
    })?;
    Err(e)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_moved_callout_box_keeps_the_tip_and_reattaches_the_leader() {
        let box_right = Rect::new(200.0, 100.0, 300.0, 140.0);
        assert_eq!(
            reattach(&[50.0, 50.0, 120.0, 90.0], box_right),
            vec![50.0, 50.0, 200.0, 120.0]
        );
        // tip right of the box: the right side; a knee is kept
        assert_eq!(
            reattach(&[500.0, 50.0, 450.0, 80.0, 400.0, 80.0], box_right),
            vec![500.0, 50.0, 450.0, 80.0, 300.0, 120.0]
        );
        assert_eq!(reattach(&[1.0, 2.0], box_right), vec![1.0, 2.0]);
    }
}
