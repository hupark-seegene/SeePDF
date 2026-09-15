//! Redaction — `IPC_CONTRACT.md` §7.5, `ARCHITECTURE.md` §6.5.
//!
//! **The content is removed, not covered.** A black rectangle over text is not a redaction:
//! the characters are still in the content stream and every copy/paste, text extraction and
//! search finds them. So [`apply`] deletes the page objects and only then draws the box.
//!
//! What PDFium can and cannot do here (annotations spike §3.5):
//!
//! * `FPDFPage_CreateAnnot(FPDF_ANNOT_REDACT)` returns NULL — redaction annotations cannot be
//!   created, so there is no "mark now, apply later" object. The marks live in the UI.
//! * **A text object cannot be split.** Removing one word removes the whole run it belongs to
//!   ("Gal" takes "Andreas Gal" with it). [`preview`] reports exactly that as `collateral`, so
//!   the user sees it *before* the destructive step.
//! * Images and paths are removed when they lie **entirely** inside a mark. A partially
//!   covered image keeps its pixels under the black box; re-encoding the raster with the
//!   region blanked is P2 and is listed in `STAGE1A_NOTES.md`.
//!
//! Finally the page text is re-extracted and every marked string is counted again. If one
//! survives, the whole command fails with `verifyFailed` and `registry::mutate` rolls the
//! document back to the snapshot — a fake redaction is never written to disk.

use crate::engine::annot;
use crate::engine::registry::{self, MutateOpts, OpenDoc};
use crate::engine::text;
use crate::engine::types::EngineState;
use crate::ipc::error::PdfiumResultExt;
use crate::ipc::types::{
    ChangeReason, PageIndex, RedactImageObject, RedactOptions, RedactPreview, RedactTextObject,
    Rect,
};
use crate::ipc::{EngineError, ErrorCode};
use pdfium_render::prelude::{
    PdfColor, PdfPageObjectCommon, PdfPageObjectType, PdfPageObjectsCommon, PdfPoints, PdfRect,
};
use std::collections::BTreeMap;

/// One page object that a set of marks hits.
#[derive(Debug, Clone)]
struct Victim {
    index: u32,
    kind: PdfPageObjectType,
    rect: Rect,
    /// Text of the whole object (text objects only).
    text: String,
    /// The part that will be lost although it is outside every mark.
    collateral: String,
    fully_inside: bool,
}

/// `redact_preview` — what [`apply`] would remove, before anything is removed.
pub fn preview(
    doc: &mut OpenDoc<'_>,
    page_index: PageIndex,
    rects: &[Rect],
) -> Result<RedactPreview, EngineError> {
    if rects.is_empty() {
        return Err(EngineError::invalid("redaction needs at least one rect"));
    }
    let victims = find_victims(doc, page_index, rects)?;
    let annots = annot::list(doc, page_index)?;

    let mut text_objects = Vec::new();
    let mut image_objects = Vec::new();
    let mut collateral = Vec::new();
    for v in &victims {
        match v.kind {
            PdfPageObjectType::Text => {
                if !v.collateral.trim().is_empty() {
                    collateral.push(v.collateral.trim().to_string());
                }
                text_objects.push(RedactTextObject {
                    object_id: v.index,
                    text: v.text.clone(),
                    rect: v.rect,
                    fully_inside: v.fully_inside,
                });
            }
            PdfPageObjectType::Image => image_objects.push(RedactImageObject {
                object_id: v.index,
                rect: v.rect,
                fully_inside: v.fully_inside,
            }),
            _ => {}
        }
    }

    Ok(RedactPreview {
        page: page_index,
        text_objects,
        image_objects,
        annotations: annots
            .iter()
            .filter(|a| rects.iter().any(|r| r.intersects(&a.rect)))
            .map(|a| a.id.clone())
            .collect(),
        // A widget under a mark means the value would survive in the AcroForm dictionary,
        // which PDFium gives us no way to rewrite — the apply refuses instead of lying.
        form_fields: annots
            .iter()
            .filter(|a| {
                a.subtype == "Widget" && rects.iter().any(|r| r.intersects(&a.rect))
            })
            .map(|a| {
                if a.contents.is_empty() {
                    a.id.clone()
                } else {
                    a.contents.clone()
                }
            })
            .collect(),
        collateral,
    })
}

/// `apply_redactions` — the destructive step, verified **before and after**.
///
/// The order matters. `registry::mutate` does not undo a closure that fails half-way: its
/// contract is "return `Err` only if nothing happened" (`STAGE0_NOTES.md` §1.2), and deleting
/// a page object cannot be undone from inside the closure. So everything that can make the
/// redaction impossible is decided **first**, on an untouched document:
///
/// 1. a form field under a mark → `unsupported` (its value lives in the AcroForm dictionary,
///    which PDFium cannot rewrite);
/// 2. a marked run whose characters belong to no removable page object — text inside a Form
///    XObject, for instance — → `verifyFailed`, because covering it with a box would be a
///    fake redaction;
/// 3. only then are the objects removed, the boxes drawn and the content stream regenerated;
/// 4. the page text is re-extracted and counted again as a post-condition. Reaching that
///    branch means something removed less than it claimed, and [`apply_verified`] restores
///    the document from the byte snapshot it took.
///
/// Returns the number of page objects removed.
pub fn apply(
    doc: &mut OpenDoc<'_>,
    page_index: PageIndex,
    rects: &[Rect],
    options: &RedactOptions,
) -> Result<u32, EngineError> {
    if rects.is_empty() {
        return Err(EngineError::invalid("redaction needs at least one rect"));
    }
    let plan = preview(doc, page_index, rects)?;
    if !plan.form_fields.is_empty() {
        return Err(EngineError::new(
            ErrorCode::Unsupported,
            format!(
                "{} form field(s) are under the marks; flatten the form first",
                plan.form_fields.len()
            ),
        )
        .with_page(page_index));
    }
    let victims = find_victims(doc, page_index, rects)?;
    let mut to_remove: Vec<u32> = victims
        .iter()
        .filter(|v| match v.kind {
            PdfPageObjectType::Text => true,
            // Images and paths only when they are entirely inside a mark; a partial one keeps
            // its pixels under the box (P2: re-encode the raster with the region blanked).
            PdfPageObjectType::Image | PdfPageObjectType::Path => v.fully_inside,
            _ => false,
        })
        .map(|v| v.index)
        .collect();
    to_remove.sort_unstable();
    to_remove.dedup();
    let removable_text: Vec<Rect> = victims
        .iter()
        .filter(|v| v.kind == PdfPageObjectType::Text && to_remove.contains(&v.index))
        .map(|v| v.rect)
        .collect();

    // Pre-flight (2): what the marks cover, and whether each run can actually be deleted.
    let runs = marked_runs(doc, page_index, rects, &to_remove, &removable_text)?;
    if let Some(stuck) = runs.iter().find(|r| !r.removable) {
        return Err(EngineError::new(
            ErrorCode::VerifyFailed,
            format!(
                "{:?} cannot be removed — it belongs to no deletable page object (text inside a \
                 Form XObject, for example), so the document was left untouched",
                stuck.text
            ),
        )
        .with_page(page_index));
    }
    let marked: Vec<String> = runs.into_iter().map(|r| r.text).collect();
    let text_before = text::layer::page_text(doc, page_index)?.text.clone();

    // --- from here on the document is being changed -------------------------------------
    // 1. annotations under the marks go first (their indices are independent of objects).
    if !plan.annotations.is_empty() {
        annot::delete(doc, page_index, &plan.annotations)?;
    }

    // 2. page objects, descending — every removal shifts the indices above it.
    let mut removed = 0u32;
    {
        let page = doc.page(page_index)?;
        for index in to_remove.iter().rev() {
            page.objects_mut()
                .remove_object_at_index(*index as usize)
                .ctx(&format!("remove object {index}"))?;
            removed += 1;
        }
        // 3. the fill boxes, one per mark.
        let fill = PdfColor::new(options.fill[0], options.fill[1], options.fill[2], 255);
        for r in rects {
            page.objects_mut()
                .create_path_object_rect(to_pdf_rect(*r), None, None, Some(fill))
                .ctx("draw the redaction box")?;
        }
        // 4. page objects changed, so the content stream is regenerated exactly once.
        page.regenerate_content().ctx("regenerate page content")?;
    }
    doc.text.invalidate_page(page_index);
    doc.annots.remove(&page_index);
    doc.touched.insert(page_index);

    // 5. post-condition. `page_text` re-extracts from the regenerated content stream.
    let text_after = text::layer::page_text(doc, page_index)?.text.clone();
    if let Some(survivor) = first_survivor(&marked, &text_before, &text_after) {
        return Err(EngineError::new(
            ErrorCode::VerifyFailed,
            format!("redacted text {survivor:?} is still extractable; the page was restored"),
        )
        .with_page(page_index));
    }
    Ok(removed)
}

/// [`apply`] wrapped in the rollback the contract promises.
///
/// A byte snapshot is taken first (5 ms/MB — this is a destructive, user-initiated command,
/// not a hot path) and restored whenever the redaction reports `verifyFailed`, so the
/// document is byte-for-byte what it was whichever of the two verification points fired.
/// This is the entry point the command and the tests use.
pub fn apply_verified(
    st: &mut EngineState<'_>,
    doc_id: &str,
    page_index: PageIndex,
    rects: &[Rect],
    options: &RedactOptions,
) -> Result<u32, EngineError> {
    let snapshot = st.doc(doc_id)?.to_bytes()?;
    let result = registry::mutate(
        st,
        doc_id,
        MutateOpts::new("undo.redact", ChangeReason::Redact).page(page_index),
        |doc| apply(doc, page_index, rects, options),
    );
    match result {
        Err(e) if e.code == ErrorCode::VerifyFailed => {
            registry::replace(st, doc_id, snapshot)?;
            Err(e)
        }
        other => other,
    }
}

/// One contiguous run of characters under the marks, and whether it can actually be deleted.
struct MarkedRun {
    text: String,
    removable: bool,
}

/// Every contiguous run of characters the marks cover, in reading order.
///
/// This is what [`apply`] verifies against, and it is deliberately independent of the objects
/// that happen to overlap the marks: if a run's characters belong to no removable page object
/// (text inside a Form XObject, a glyph whose owner could not be identified) the run is listed
/// as **not removable** and the command refuses before touching anything.
///
/// `TextLayer::object_id` is `u32::MAX` when a character could not be attributed to a
/// top-level object; such a character is still treated as removable when it sits inside the
/// bounds of a text object that *is* being removed, which is what makes the handful of
/// fingerprint misses on an ordinary page harmless.
fn marked_runs(
    doc: &mut OpenDoc<'_>,
    page_index: PageIndex,
    rects: &[Rect],
    to_remove: &[u32],
    removable_text: &[Rect],
) -> Result<Vec<MarkedRun>, EngineError> {
    let layer = text::layer::layer(doc, page_index)?;
    let mut runs: Vec<MarkedRun> = Vec::new();
    let mut text = String::new();
    let mut removable = true;
    for c in &layer.chars {
        if c.is_generated() {
            continue;
        }
        if rects.iter().any(|r| r.intersects(&c.tight)) {
            text.push(char::from_u32(c.codepoint).unwrap_or('\u{fffd}'));
            let owned = if c.object_id == u32::MAX {
                removable_text.iter().any(|r| contains(r, &c.tight))
            } else {
                to_remove.contains(&c.object_id)
            };
            removable &= owned;
        } else if !text.is_empty() {
            runs.push(MarkedRun {
                text: std::mem::take(&mut text),
                removable,
            });
            removable = true;
        }
    }
    if !text.is_empty() {
        runs.push(MarkedRun { text, removable });
    }
    for run in &mut runs {
        run.text = run.text.trim().to_string();
    }
    runs.retain(|r| !r.text.is_empty());
    Ok(runs)
}

/// The marked string whose occurrence count did not drop, if any./// The marked string whose occurrence count did not drop, if any.
///
/// Counting rather than "is it absent" is what makes this correct for a word that also occurs
/// somewhere else on the page: redacting one "the" must not require every "the" to vanish.
fn first_survivor<'a>(
    marked: &'a [String],
    before: &str,
    after: &str,
) -> Option<&'a str> {
    marked.iter().map(String::as_str).find(|needle| {
        let was = before.matches(needle).count();
        let now = after.matches(needle).count();
        was > 0 && now >= was
    })
}

/// Classifies every page object against the marks.
fn find_victims(
    doc: &mut OpenDoc<'_>,
    page_index: PageIndex,
    rects: &[Rect],
) -> Result<Vec<Victim>, EngineError> {
    // The text layer gives per-character ink boxes (`tight`) and the owning object index, so
    // the marked and the collateral halves of a run can be separated exactly.
    let layer = text::layer::layer(doc, page_index)?;
    let mut marked: BTreeMap<u32, (String, String)> = BTreeMap::new();
    if layer.has_object_ids {
        for c in &layer.chars {
            if c.object_id == u32::MAX || c.is_generated() {
                continue;
            }
            let ch = char::from_u32(c.codepoint).unwrap_or('\u{fffd}');
            let hit = rects.iter().any(|r| r.intersects(&c.tight));
            let entry = marked.entry(c.object_id).or_default();
            if hit {
                entry.0.push(ch);
            } else {
                entry.1.push(ch);
            }
        }
        marked.retain(|_, (inside, _)| !inside.is_empty());
    }

    let page = doc.page(page_index)?;
    let count = page.objects().len();
    let mut out = Vec::new();
    for index in 0..count {
        let object = page.objects().get(index).ctx("read page object")?;
        let kind = object.object_type();
        let Ok(bounds) = object.bounds() else {
            continue;
        };
        let rect = quad_bounds(&bounds);
        let overlaps = rects.iter().any(|r| r.intersects(&rect));
        let fully_inside = rects.iter().any(|r| contains(r, &rect));
        let by_layer = marked.get(&(index as u32));
        if !overlaps && by_layer.is_none() {
            continue;
        }
        let (text, collateral) = match (kind, by_layer) {
            (PdfPageObjectType::Text, Some((inside, outside))) => {
                (format!("{inside}{outside}"), outside.clone())
            }
            (PdfPageObjectType::Text, None) => {
                // No object ids in this layer (rare): fall back to the whole run.
                let text = object
                    .as_text_object()
                    .map(|t| t.text())
                    .unwrap_or_default();
                let collateral = if fully_inside {
                    String::new()
                } else {
                    text.clone()
                };
                (text, collateral)
            }
            _ => (String::new(), String::new()),
        };
        out.push(Victim {
            index: index as u32,
            kind,
            rect,
            text,
            collateral,
            fully_inside,
        });
    }
    Ok(out)
}

/// `PdfQuadPoints` → the contract's rect, order-agnostic (`to_rect()` assumes one corner
/// order and returns an empty rect for the other).
fn quad_bounds(q: &pdfium_render::prelude::PdfQuadPoints) -> Rect {
    let xs = [
        q.x1().value,
        q.x2().value,
        q.x3().value,
        q.x4().value,
    ];
    let ys = [
        q.y1().value,
        q.y2().value,
        q.y3().value,
        q.y4().value,
    ];
    Rect::new(
        xs.iter().copied().fold(f32::MAX, f32::min),
        ys.iter().copied().fold(f32::MAX, f32::min),
        xs.iter().copied().fold(f32::MIN, f32::max),
        ys.iter().copied().fold(f32::MIN, f32::max),
    )
}

fn contains(outer: &Rect, inner: &Rect) -> bool {
    outer.l <= inner.l && outer.b <= inner.b && outer.r >= inner.r && outer.t >= inner.t
}

/// `PdfRect::new_from_values(bottom, left, top, right)` — the unusual argument order.
fn to_pdf_rect(r: Rect) -> PdfRect {
    PdfRect::new(
        PdfPoints::new(r.b.min(r.t)),
        PdfPoints::new(r.l.min(r.r)),
        PdfPoints::new(r.b.max(r.t)),
        PdfPoints::new(r.l.max(r.r)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_survivor_is_one_whose_count_did_not_drop() {
        let marked = vec!["Gal".to_string(), "the".to_string()];
        // "Gal" gone, one "the" of two gone: nothing survived.
        assert_eq!(
            first_survivor(&marked, "Andreas Gal and the the", "and the"),
            None
        );
        // "Gal" still there.
        assert_eq!(
            first_survivor(&marked, "Andreas Gal and the", "Andreas Gal and"),
            Some("Gal")
        );
    }

    #[test]
    fn containment_is_strict() {
        let outer = Rect::new(0.0, 0.0, 10.0, 10.0);
        assert!(contains(&outer, &Rect::new(1.0, 1.0, 9.0, 9.0)));
        assert!(!contains(&outer, &Rect::new(1.0, 1.0, 11.0, 9.0)));
    }
}
