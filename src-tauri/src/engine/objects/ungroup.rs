//! R4 그룹 해제 — replacing a Form XObject on the page by the objects inside it.
//!
//! Office exports and many PDF producers wrap the whole page (or its body) in one Form XObject.
//! PDFium reads the text inside, but `FPDFPage_GenerateContent` only rewrites the page stream
//! and re-emits `/Form Do`, so an edit of a child is lost on save (text spike §8.7) and a
//! redaction cannot delete it.
//!
//! ## `ungroup_object` (v0.3.1)
//!
//! 1. **Which group.** With `at` (the click of 텍스트 수정 / a double-click), [`group_hit`] finds
//!    the text under the point on the text page and walks down the nested forms to the one that
//!    holds it: every form on that path is ungrouped, in one undo step. Without `at` (the
//!    Inspector's 그룹 해제), the group and every nested group that draws text.
//! 2. **How.** At the content-stream level ([`super::inline`]): the `Do` is replaced by the
//!    form's own operators, byte for byte, so nothing PDFium's content generator cannot write
//!    (inline images, `Tc` / `Tw` / `Tz`, optional-content marks, fonts that share a
//!    `BaseFont`, the clip around the `Do`) is lost. `registry::mutate_bytes_checked`: one undo
//!    step `undo.ungroup`, rolled back on any failure.
//! 3. **Verified.** The page is rendered before, and the rewritten file's page after; the
//!    ungroup only stands when they look the same ([`LOOK_MEAN`], [`LOOK_SHARE`]). An Office /
//!    Hancom group (`/Group /S /Transparency` over opaque content) looks the same and passes; a
//!    group whose look depends on it staying a group (an alpha, blend or soft mask applied to
//!    the group as a whole) is tried again with only its **text** lifted out
//!    ([`inline::Mode::TextOnly`]), and when that changes the look too the document is left as
//!    it was: `unsupported`, detail `lookChanged`.
//! 4. **Fallback.** A page whose content cannot be read or rewritten (an encryption lopdf
//!    cannot write back, a content stream it cannot decode, a `Do` it cannot locate) is
//!    ungrouped the v0.3.0 way — PDFium's object API, [`ungroup_at`] — under the same render
//!    check inside `registry::mutate`.
//!
//! [`ungroup_at`] stays the redaction's way (`redact::ungroup_all`): moving PDFium objects, which
//! the redaction then edits in memory before it regenerates the page.

use super::inline::{self, Mat, Mode, Plan};
use super::{check_generation, relist};
use crate::engine::annot::ScratchPage;
use crate::engine::raw::object::Matrix;
use crate::engine::redact::raw;
use crate::engine::registry::{self, MutateOpts};
use crate::engine::types::EngineState;
use crate::ipc::error::PdfiumResultExt;
use crate::ipc::types::{
    ChangeReason, DocGeneration, ObjectId, PageIndex, Point, Rect, UngroupResult,
};
use crate::ipc::{EngineError, ErrorCode};
use pdfium_render::prelude::*;
use std::collections::HashMap;

/// Mean absolute channel difference (0–255) the whole page may change by.
pub const LOOK_MEAN: f64 = 0.35;
/// Share of pixels whose largest channel difference exceeds [`LOOK_DIFF`].
pub const LOOK_SHARE: f64 = 0.0003;
pub const LOOK_DIFF: u8 = 40;
/// Pixel budget of the comparison render.
const LOOK_PIXELS: f32 = 3.0e6;

// ---------------------------------------------------------------------------------------
// The v0.3.0 object move (the redaction's ungroup and the fallback)
// ---------------------------------------------------------------------------------------

/// How the colours of a moved child are carried over: PDFium's content generator only writes
/// `rg` / `RG` for DeviceRGB and DeviceGray colours and silently drops an ICC, `/CalRGB` or
/// `/Separation` colour, which would paint the child black.
fn normalise_colours(bindings: &dyn PdfiumLibraryBindings, child: FPDF_PAGEOBJECT) {
    let (mut r, mut g, mut b, mut a) = (0u32, 0u32, 0u32, 0u32);
    // SAFETY: `child` is a live, unowned page object; the out-params outlive the calls.
    unsafe {
        if bindings
            .is_true(bindings.FPDFPageObj_GetFillColor(child, &mut r, &mut g, &mut b, &mut a))
        {
            bindings.FPDFPageObj_SetFillColor(child, r, g, b, a);
        }
        if bindings
            .is_true(bindings.FPDFPageObj_GetStrokeColor(child, &mut r, &mut g, &mut b, &mut a))
        {
            bindings.FPDFPageObj_SetStrokeColor(child, r, g, b, a);
        }
    }
}

/// Moves the children of the form object at `index` of `page` onto the page, in place, and
/// removes the form (`FPDFFormObj_RemoveObject`, the form matrix applied to each child and its
/// clip, colours re-set as RGB, `FPDFPage_InsertObjectAtIndex`). Returns the number of children
/// (now at `index..index + n`). The caller regenerates the page content.
///
/// `lossy` = go ahead even when the form carries transparency; otherwise that is refused
/// (`unsupported`, detail `groupTransparency`) — its children would come out opaque.
pub fn ungroup_at(
    bindings: &dyn PdfiumLibraryBindings,
    page: &PdfPage<'_>,
    index: usize,
    lossy: bool,
) -> Result<usize, EngineError> {
    let form = raw::object_at(bindings, page, index)?;
    if raw::object_type(bindings, form) != raw::OBJ_FORM {
        return Err(not_a_group(index));
    }
    if !lossy && raw::has_transparency(bindings, form) {
        return Err(EngineError::new(
            ErrorCode::Unsupported,
            "this group is drawn with transparency, which its objects would lose",
        )
        .with_detail("groupTransparency"));
    }
    let m: Matrix = raw::matrix(bindings, form);
    let children = raw::form_children(bindings, form);
    for (k, &child) in children.iter().enumerate() {
        // SAFETY: `child` belongs to the live `form`; ownership passes to us.
        let ok = unsafe { bindings.FPDFFormObj_RemoveObject(form, child) };
        if !bindings.is_true(ok) {
            return Err(EngineError::new(
                ErrorCode::Pdfium,
                "FPDFFormObj_RemoveObject failed",
            ));
        }
        normalise_colours(bindings, child);
        let [a, b, c, d, e, f] = m.map(f64::from);
        // SAFETY: `child` is live and unowned; both calls take plain numbers.
        unsafe {
            bindings.FPDFPageObj_Transform(child, a, b, c, d, e, f);
            bindings.FPDFPageObj_TransformClipPath(child, a, b, c, d, e, f);
        }
        raw::insert_at(bindings, page, child, index + k)?;
    }
    let form = raw::object_at(bindings, page, index + children.len())?;
    raw::remove_and_destroy(bindings, page, form)?;
    Ok(children.len())
}

fn not_a_group(index: usize) -> EngineError {
    EngineError::invalid(format!("object {index} is not a group (Form XObject)"))
        .with_detail("notAGroup")
}

// ---------------------------------------------------------------------------------------
// Which text is under a point
// ---------------------------------------------------------------------------------------

/// Text inside a group, under a point.
#[derive(Debug, Clone)]
pub struct GroupHit {
    /// The top-level form (page object index).
    pub top: usize,
    /// The nested forms down to the one holding the text: child indices, outermost first.
    /// Empty when the top-level form holds it.
    pub chain: Vec<usize>,
    /// The form directly holding the text, and the text object (live while the page is).
    pub parent: FPDF_PAGEOBJECT,
    pub text: FPDF_PAGEOBJECT,
    /// Page-space box of the text object's characters.
    pub rect: Rect,
    /// Its characters, as the text page reads them.
    pub chars: String,
}

type Owner = (usize, Vec<usize>, FPDF_PAGEOBJECT, FPDF_PAGEOBJECT);

fn collect_owners(
    bindings: &dyn PdfiumLibraryBindings,
    form: FPDF_PAGEOBJECT,
    top: usize,
    chain: &mut Vec<usize>,
    out: &mut HashMap<raw::HandleKey, Owner>,
) {
    for (ci, child) in raw::form_children(bindings, form).into_iter().enumerate() {
        match raw::object_type(bindings, child) {
            raw::OBJ_TEXT => {
                out.insert(raw::key(child), (top, chain.clone(), form, child));
            }
            raw::OBJ_FORM if chain.len() < 16 => {
                chain.push(ci);
                collect_owners(bindings, child, top, chain, out);
                chain.pop();
            }
            _ => {}
        }
    }
}

/// Does the page have any text inside a group (cheap: no text page)?
pub fn has_grouped_text(bindings: &dyn PdfiumLibraryBindings, page: &PdfPage<'_>) -> bool {
    fn walk(bindings: &dyn PdfiumLibraryBindings, form: FPDF_PAGEOBJECT, depth: usize) -> bool {
        raw::form_children(bindings, form).into_iter().any(|c| {
            match raw::object_type(bindings, c) {
                raw::OBJ_TEXT => true,
                raw::OBJ_FORM if depth < 16 => walk(bindings, c, depth + 1),
                _ => false,
            }
        })
    }
    (0..raw::object_count(bindings, page)).any(|i| {
        raw::object_at(bindings, page, i)
            .is_ok_and(|h| raw::object_type(bindings, h) == raw::OBJ_FORM && walk(bindings, h, 0))
    })
}

/// The grouped text under `at` (page points): the character whose box is nearest (within
/// half its height, at least 2 pt), the topmost one when several contain the point. `only`:
/// look inside that top-level form alone.
pub fn group_hit(
    bindings: &dyn PdfiumLibraryBindings,
    page: &PdfPage<'_>,
    at: Point,
    only: Option<usize>,
) -> Result<Option<GroupHit>, EngineError> {
    let mut owners: HashMap<raw::HandleKey, Owner> = HashMap::new();
    for i in 0..raw::object_count(bindings, page) {
        if only.is_some_and(|o| o != i) {
            continue;
        }
        let h = raw::object_at(bindings, page, i)?;
        if raw::object_type(bindings, h) == raw::OBJ_FORM {
            collect_owners(bindings, h, i, &mut Vec::new(), &mut owners);
        }
    }
    if owners.is_empty() {
        return Ok(None);
    }
    let tp = raw::TextPage::load(bindings, page)?;
    let chars = tp.chars();
    let [x, y] = at;
    let mut best: Option<(f32, usize)> = None;
    for (n, ch) in chars.iter().enumerate() {
        if ch.generated || !owners.contains_key(&ch.object) {
            continue;
        }
        let r = ch.tight;
        let (l, rr) = (r.l.min(r.r), r.l.max(r.r));
        let (b, t) = (r.b.min(r.t), r.b.max(r.t));
        let tol = (0.5 * (t - b)).max(2.0);
        let dx = (l - x).max(0.0).max(x - rr);
        let dy = (b - y).max(0.0).max(y - t);
        let d = dx.hypot(dy);
        if d <= tol && best.is_none_or(|(bd, _)| d <= bd) {
            best = Some((d, n));
        }
    }
    let Some((_, n)) = best else {
        return Ok(None);
    };
    let key = chars[n].object;
    let (top, chain, parent, text) = owners[&key].clone();
    let mut rect: Option<Rect> = None;
    let mut s = String::new();
    for ch in chars.iter().filter(|c| c.object == key) {
        let r = ch.tight;
        rect = Some(match rect {
            None => r,
            Some(u) => Rect::new(u.l.min(r.l), u.b.min(r.b), u.r.max(r.r), u.t.max(r.t)),
        });
        if let Some(c) = char::from_u32(ch.unicode) {
            s.push(c);
        }
    }
    Ok(Some(GroupHit {
        top,
        chain,
        parent,
        text,
        rect: rect.unwrap_or(Rect::ZERO),
        chars: s,
    }))
}

// ---------------------------------------------------------------------------------------
// Looking the same
// ---------------------------------------------------------------------------------------

/// A page render for comparing (no annotations, no form fields: the page content only).
pub struct Look {
    width: i32,
    height: i32,
    pixels: Vec<u8>,
}

/// The render scale for comparing `page`: 2× or less, within [`LOOK_PIXELS`].
pub fn look_scale(page: &PdfPage<'_>) -> f32 {
    let area = (page.width().value * page.height().value).max(1.0);
    (LOOK_PIXELS / area).sqrt().min(2.0)
}

pub fn render_look(page: &PdfPage<'_>, scale: f32) -> Result<Look, EngineError> {
    let config = PdfRenderConfig::new()
        .scale_page_by_factor(scale)
        .render_form_data(false)
        .render_annotations(false)
        .use_lcd_text_rendering(false)
        .set_format(PdfBitmapFormat::BGRA)
        .set_clear_color(PdfColor::WHITE);
    let bitmap = page
        .render_with_config(&config)
        .ctx("render the page to compare")?;
    Ok(Look {
        width: bitmap.width(),
        height: bitmap.height(),
        pixels: bitmap.as_raw_bytes(),
    })
}

/// `(mean channel difference, share of pixels off by more than LOOK_DIFF)`.
pub fn look_diff(a: &Look, b: &Look) -> (f64, f64) {
    if a.width != b.width || a.height != b.height || a.pixels.len() != b.pixels.len() {
        return (255.0, 1.0);
    }
    let (mut sum, mut off, mut n) = (0u64, 0u64, 0u64);
    for (p, q) in a.pixels.chunks_exact(4).zip(b.pixels.chunks_exact(4)) {
        let mut worst = 0u8;
        for k in 0..3 {
            let d = p[k].abs_diff(q[k]);
            sum += d as u64;
            worst = worst.max(d);
        }
        if worst > LOOK_DIFF {
            off += 1;
        }
        n += 1;
    }
    if n == 0 {
        return (0.0, 0.0);
    }
    (sum as f64 / (3 * n) as f64, off as f64 / n as f64)
}

pub fn looks_same(a: &Look, b: &Look) -> bool {
    let (mean, share) = look_diff(a, b);
    mean <= LOOK_MEAN && share <= LOOK_SHARE
}

fn look_changed(partial: bool) -> EngineError {
    EngineError::new(
        ErrorCode::Unsupported,
        if partial {
            "ungrouping this group would change how the page looks, even with only its text \
             taken out (transparency or blending applied to the group as a whole); the group \
             was left as it was"
        } else {
            "ungrouping this group would change how the page looks; the group was left as it was"
        },
    )
    .with_detail("lookChanged")
}

/// What a top-level object looks like, to check that the rest of the page came through.
#[derive(Debug, Clone, Copy)]
struct Placement {
    kind: i32,
    matrix: Matrix,
}

fn placements(bindings: &dyn PdfiumLibraryBindings, page: &PdfPage<'_>) -> Vec<Placement> {
    (0..raw::object_count(bindings, page))
        .map(|i| match raw::object_at(bindings, page, i) {
            Ok(h) => Placement {
                kind: raw::object_type(bindings, h),
                matrix: raw::matrix(bindings, h),
            },
            Err(_) => Placement {
                kind: -1,
                matrix: [0.0; 6],
            },
        })
        .collect()
}

fn same_placement(a: &Placement, b: &Placement) -> bool {
    a.kind == b.kind && inline::close(&a.matrix.map(f64::from), &b.matrix.map(f64::from))
}

// ---------------------------------------------------------------------------------------
// ungroup_object
// ---------------------------------------------------------------------------------------

/// What the live page says about the group to ungroup.
struct Analysis {
    ordinal: usize,
    matrix: Mat,
    chain: Vec<(usize, Mat)>,
    /// Child indices of the chain (for the object-move fallback).
    chain_children: Vec<usize>,
    /// The form holding the text draws something else too (a text-only ungroup is worth trying).
    mixed: bool,
    placements: Vec<Placement>,
    scale: f32,
    before: Look,
}

fn analyse(
    st: &mut EngineState<'_>,
    doc_id: &str,
    page_index: PageIndex,
    object_id: ObjectId,
    at: Option<Point>,
) -> Result<Analysis, EngineError> {
    let doc = st.doc_mut(doc_id)?;
    let bindings = doc.bindings();
    let page = doc.page(page_index)?;
    let count = raw::object_count(bindings, page);
    if object_id as usize >= count {
        return Err(
            EngineError::not_found(format!("object {object_id} of {count}")).with_page(page_index),
        );
    }
    let index = object_id as usize;
    let form = raw::object_at(bindings, page, index)?;
    if raw::object_type(bindings, form) != raw::OBJ_FORM {
        return Err(not_a_group(index));
    }
    let ordinal = (0..index)
        .filter(|&i| {
            raw::object_at(bindings, page, i)
                .is_ok_and(|h| raw::object_type(bindings, h) == raw::OBJ_FORM)
        })
        .count();
    let hit = match at {
        Some(p) => group_hit(bindings, page, p, Some(index))?,
        None => None,
    };
    let mut chain = Vec::new();
    let mut chain_children = Vec::new();
    let mut holder = form;
    if let Some(hit) = &hit {
        for &ci in &hit.chain {
            let children = raw::form_children(bindings, holder);
            let Some(&child) = children.get(ci) else {
                break;
            };
            let ord = children[..ci]
                .iter()
                .filter(|&&c| raw::object_type(bindings, c) == raw::OBJ_FORM)
                .count();
            chain.push((ord, raw::matrix(bindings, child).map(f64::from)));
            chain_children.push(ci);
            holder = child;
        }
    }
    let mixed = raw::form_children(bindings, holder)
        .into_iter()
        .any(|c| raw::object_type(bindings, c) != raw::OBJ_TEXT);
    let scale = look_scale(page);
    let before = render_look(page, scale)?;
    Ok(Analysis {
        ordinal,
        matrix: raw::matrix(bindings, form).map(f64::from),
        chain,
        chain_children,
        mixed,
        placements: placements(bindings, page),
        scale,
        before,
    })
}

/// The rest of the page came through unchanged, and the page looks the same.
fn verify(
    bindings: &dyn PdfiumLibraryBindings,
    page: &PdfPage<'_>,
    index: usize,
    a: &Analysis,
    partial: bool,
) -> Result<(), EngineError> {
    let now = placements(bindings, page);
    let tail = a.placements.len() - index - 1;
    let intact = now.len() >= index + tail
        && (0..index).all(|i| same_placement(&now[i], &a.placements[i]))
        && (0..tail)
            .all(|j| same_placement(&now[now.len() - tail + j], &a.placements[index + 1 + j]));
    if !intact {
        return Err(EngineError::new(
            ErrorCode::VerifyFailed,
            "the rest of the page did not come through the ungroup unchanged",
        )
        .with_detail("groupNotFound"));
    }
    let after = render_look(page, a.scale)?;
    let (mean, share) = look_diff(&a.before, &after);
    tracing::debug!(mean, share, partial, "ungroup look check");
    if mean <= LOOK_MEAN && share <= LOOK_SHARE {
        Ok(())
    } else {
        Err(look_changed(partial))
    }
}

/// The v0.3.0 way under the same checks: PDFium moves the objects (every form on the chain;
/// without a chain, the group and the nested groups that draw text).
fn move_objects(
    st: &mut EngineState<'_>,
    doc_id: &str,
    page_index: PageIndex,
    index: usize,
    a: &Analysis,
    deep: bool,
) -> Result<(), EngineError> {
    registry::mutate(
        st,
        doc_id,
        MutateOpts::new("undo.ungroup", ChangeReason::Edit).page(page_index),
        |doc| {
            let bindings = doc.bindings();
            {
                let mut scratch = ScratchPage::open(doc, page_index)?;
                let n = ungroup_at(bindings, &scratch.page, index, true)?;
                let mut at = index;
                for &ci in &a.chain_children {
                    at += ci;
                    ungroup_at(bindings, &scratch.page, at, true)?;
                }
                if deep {
                    let mut end = index + n;
                    let mut i = index;
                    while i < end {
                        let h = raw::object_at(bindings, &scratch.page, i)?;
                        let text = raw::object_type(bindings, h) == raw::OBJ_FORM
                            && raw::form_children(bindings, h)
                                .iter()
                                .any(|&c| raw::object_type(bindings, c) == raw::OBJ_TEXT);
                        if text {
                            let k = ungroup_at(bindings, &scratch.page, i, true)?;
                            end = end + k - 1;
                            continue;
                        }
                        i += 1;
                    }
                }
                scratch
                    .page
                    .regenerate_content()
                    .ctx("regenerate page content")?;
            }
            // A fresh parse of what was written.
            let page = doc.page(page_index)?;
            verify(bindings, page, index, a, false)
        },
    )
}

/// `ungroup_object` — the explicit 그룹 해제 (module docs).
pub fn ungroup(
    st: &mut EngineState<'_>,
    doc_id: &str,
    page_index: PageIndex,
    object_id: ObjectId,
    expect_generation: DocGeneration,
    at: Option<Point>,
) -> Result<UngroupResult, EngineError> {
    check_generation(st.doc(doc_id)?, expect_generation)?;
    st.doc(doc_id)?.geom(page_index)?;
    let before_count = {
        let doc = st.doc_mut(doc_id)?;
        let bindings = doc.bindings();
        raw::object_count(bindings, doc.page(page_index)?)
    };
    let a = analyse(st, doc_id, page_index, object_id, at)?;
    let index = object_id as usize;
    let deep = at.is_none();
    let modes: &[Mode] = if a.mixed {
        &[Mode::Full, Mode::TextOnly]
    } else {
        &[Mode::Full]
    };
    let mut done: Option<Mode> = None;
    let mut last: Option<EngineError> = None;
    let mut fallback = false;
    for &mode in modes {
        let plan = Plan {
            page_index,
            ordinal: a.ordinal,
            matrix: a.matrix,
            chain: a.chain.clone(),
            mode,
            deep,
        };
        let partial = mode == Mode::TextOnly;
        let result = registry::mutate_bytes_checked(
            st,
            doc_id,
            MutateOpts::new("undo.ungroup", ChangeReason::Edit).page(page_index),
            |bytes, _| inline::rewrite(bytes, &plan).map(|r| r.bytes),
            |bindings, reopened| {
                let page = reopened
                    .pages()
                    .get(page_index as PdfPageIndex)
                    .map_err(|e| EngineError::pdfium("reopen the page", e))?;
                verify(bindings, &page, index, &a, partial)
            },
        );
        match result {
            Ok(_) => {
                done = Some(mode);
                break;
            }
            Err(e) if e.detail.as_deref() == Some("lookChanged") => last = Some(e),
            Err(e)
                if matches!(e.detail.as_deref(), Some("groupNotFound" | "content"))
                    || crate::engine::security::cannot_rewrite(&e) =>
            {
                tracing::warn!(error = %e.message, "ungroup: content-level rewrite not possible, moving objects");
                fallback = true;
                last = Some(e);
                break;
            }
            Err(e) => return Err(e),
        }
    }
    if done.is_none() && fallback {
        move_objects(st, doc_id, page_index, index, &a, deep)?;
        done = Some(Mode::Full);
    }
    let Some(mode) = done else {
        return Err(last.unwrap_or_else(|| look_changed(false)));
    };
    let list = relist(st, doc_id, page_index)?;
    let after_count = list.objects.len();
    let end = (after_count + index + 1).saturating_sub(before_count);
    Ok(UngroupResult {
        doc_generation: list.doc_generation,
        new_object_ids: (object_id..end.max(index) as ObjectId).collect(),
        objects: list.objects,
        partial: mode == Mode::TextOnly,
    })
}
