//! Crop and resize pages — P2, `IPC_CONTRACT.md` §7.3a.
//!
//! Both operations follow the `page_ops` rules: **one call = one `registry::mutate` = one
//! generation = one undo step** (`undo.pageCrop`, `undo.pageResize`), and the returned
//! `DocInfo` already carries the new page geometry.
//!
//! ## Crop (`set_page_boxes`)
//!
//! `FPDFPage_SetCropBox` / `FPDFPage_SetMediaBox` (pdfium-render `PdfPageBoundaries`) write the
//! page dictionary and update PDFium's page dimensions; nothing in the content stream changes,
//! so a crop is fully reversible — 원래대로 (`crop: null`) sets the crop box back to the media
//! box. PDFium cannot *delete* a `/CropBox`, so "no crop" is stored as `CropBox = MediaBox`,
//! which every viewer treats identically.
//!
//! PDFium's box getters read only the page's own dictionary, never a box inherited from the
//! page tree. When a page inherits its `/MediaBox`, [`true_media`] asks PDFium's page
//! dimensions instead (`FPDF_GetPageBoundingBox` = media ∩ crop, with the crop widened first).
//!
//! ## Resize (`resize_pages`)
//!
//! The new page is `[0 0 w h]` (media = crop), in the page's own orientation. The content is
//! mapped from the old crop box by one uniform matrix — scaled to fit and centred
//! (`scaleContent`) or at 100 % and centred (`centerContent`) — with
//! `FPDFPage_TransFormWithClip` (pdfium-render `PdfPage::apply_matrix_with_clip`): PDFium puts
//! the page's content streams between two new ones, `q <old crop> re W* n <m> cm` … `Q`, and
//! transforms the page's shading patterns. Every page object stays an ordinary page object, so
//! text stays selectable, searchable and editable, and later edits regenerate correctly (PDFium
//! writes the inverse CTM in front of a regenerated stream).
//!
//! The alternative — importing the page as a Form XObject (`FPDF_NewXObjectFromPage`) and
//! placing it scaled — was rejected: every object would become read-only (`insideXObject`)
//! and the page's annotations would not come along.
//!
//! Annotations follow the content: `/QuadPoints` of markups and links and the `/InkList` of
//! ink are mapped first (a markup's appearance is cleared and regenerated from the new quads),
//! then `FPDFPage_TransformAnnots` maps every `/Rect` without touching appearance `BBox`es, so
//! an existing appearance scales with its rect. `/Popup` rects are mapped by hand (PDFium's
//! annotation list skips them). Keys PDFium cannot write (`/L`, `/Vertices`, `/CL`) keep their
//! old values; the appearance, which is what every viewer draws, follows.

use crate::engine::raw;
use crate::engine::raw::consts;
use crate::engine::raw::object::Matrix;
use crate::engine::registry::{self, MutateOpts};
use crate::engine::stamp::visual_to_user;
use crate::engine::types::EngineState;
use crate::ipc::error::PdfiumResultExt;
use crate::ipc::types::{
    ChangeReason, CropSpec, DocInfo, Margins, PageIndex, PageSelection, Rect, ResizeMode,
    ResizeTarget,
};
use crate::ipc::EngineError;
use pdfium_render::prelude::*;

/// Smallest page side or crop side, in points.
pub const MIN_SIDE: f32 = 1.0;
/// Largest page side PDF allows (ISO 32000 Annex C: 14 400 units).
pub const MAX_SIDE: f32 = 14_400.0;
/// A crop box wide enough to contain any media box — see [`true_media`].
const HUGE: f32 = 1.0e6;

// ---------------------------------------------------------------------------------------
// set_page_boxes
// ---------------------------------------------------------------------------------------

/// `set_page_boxes` — crop and/or media box of `pages`, one undo step `undo.pageCrop`.
///
/// `crop`: `None` = unchanged, `Some(None)` = reset to the media box, `Some(Some(spec))` = a
/// rect (the same for every page, clipped to each media box) or margins inset from each page's
/// current crop box as it is seen. `media`: `None` = unchanged, `Some(Some(rect))` = set;
/// `Some(None)` is `invalidArgument` (a page cannot lose its media box). A page whose explicit
/// crop box sticks out of a new media box has it clipped.
pub fn set_page_boxes(
    st: &mut EngineState<'_>,
    doc_id: &str,
    pages: &PageSelection,
    crop: Option<Option<CropSpec>>,
    media: Option<Option<Rect>>,
) -> Result<DocInfo, EngineError> {
    if matches!(media, Some(None)) {
        return Err(EngineError::invalid(
            "media: null is not supported — a page must keep a media box",
        ));
    }
    let media = media.flatten();
    if let Some(m) = media {
        check_box(&m, "media")?;
    }
    match crop {
        Some(Some(CropSpec::Rect(r))) => check_box(&r, "crop")?,
        Some(Some(CropSpec::Margins { margins })) => check_margins(&margins)?,
        _ => {}
    }
    let count = st.doc(doc_id)?.page_count();
    let list = resolve_pages(pages, count)?;
    if crop.is_none() && media.is_none() {
        return Ok(st.doc(doc_id)?.info());
    }

    let opts = MutateOpts::new("undo.pageCrop", ChangeReason::Pages).pages(list.clone());
    let targets = list.clone();
    registry::mutate(st, doc_id, opts, move |doc| {
        for &p in &targets {
            doc.invalidate_page_handle(p);
        }
        for &p in &targets {
            let mut page = open_page(doc.pdf(), p)?;
            let rotation = rotation_of(&page);
            let explicit_crop = explicit_box(page.boundaries().crop());
            let crop_now = visible_box(&page)?;

            let media_rect = match media {
                Some(m) => {
                    let m = normalize(m);
                    page.boundaries_mut()
                        .set_media(to_pdf(m))
                        .ctx(&format!("set the media box of page {p}"))?;
                    m
                }
                None => true_media(&mut page, p)?,
            };

            let new_crop: Option<Rect> = match crop {
                None => match (media, explicit_crop) {
                    // A new media box clips an explicit crop box that no longer fits in it.
                    (Some(_), Some(c)) => Some(intersect(&c, &media_rect).unwrap_or(media_rect)),
                    _ => None,
                },
                Some(None) => Some(media_rect),
                Some(Some(CropSpec::Rect(r))) => Some(
                    intersect(&normalize(r), &media_rect).ok_or_else(|| {
                        EngineError::invalid(format!(
                            "the crop box lies outside the media box of page {}",
                            p + 1
                        ))
                        .with_page(p)
                    })?,
                ),
                Some(Some(CropSpec::Margins { margins })) => {
                    let inset = margins_to_user(rotation, &crop_now, &margins)
                        .map_err(|e| e.with_page(p))?;
                    Some(intersect(&inset, &media_rect).ok_or_else(|| {
                        EngineError::invalid(format!(
                            "the crop box lies outside the media box of page {}",
                            p + 1
                        ))
                        .with_page(p)
                    })?)
                }
            };
            if let Some(c) = new_crop {
                if c.width() < MIN_SIDE || c.height() < MIN_SIDE {
                    return Err(EngineError::invalid(format!(
                        "the crop box of page {} would be smaller than {MIN_SIDE} pt",
                        p + 1
                    ))
                    .with_page(p));
                }
                page.boundaries_mut()
                    .set_crop(to_pdf(c))
                    .ctx(&format!("set the crop box of page {p}"))?;
            }
        }
        Ok(())
    })?;
    refine_geometry(st, doc_id, &list)?;
    Ok(st.doc(doc_id)?.info())
}

// ---------------------------------------------------------------------------------------
// resize_pages
// ---------------------------------------------------------------------------------------

/// `resize_pages` — gives `pages` a new size (media = crop = `[0 0 w h]`) and maps the content
/// and the annotations into it; one undo step `undo.pageResize`. A named size keeps each page's
/// orientation (a landscape page gets landscape A4); `{ w, h }` is the size as seen.
pub fn resize_pages(
    st: &mut EngineState<'_>,
    doc_id: &str,
    pages: &PageSelection,
    size: ResizeTarget,
    mode: ResizeMode,
) -> Result<DocInfo, EngineError> {
    if let ResizeTarget::Size { w, h } = size {
        for (v, name) in [(w, "w"), (h, "h")] {
            if !v.is_finite() || !(MIN_SIDE..=MAX_SIDE).contains(&v) {
                return Err(EngineError::invalid(format!(
                    "page size {name} = {v} pt is outside {MIN_SIDE}..={MAX_SIDE}"
                )));
            }
        }
    }
    let count = st.doc(doc_id)?.page_count();
    let list = resolve_pages(pages, count)?;

    let opts = MutateOpts::new("undo.pageResize", ChangeReason::Pages).pages(list.clone());
    let targets = list.clone();
    registry::mutate(st, doc_id, opts, move |doc| {
        for &p in &targets {
            doc.invalidate_page_handle(p);
        }
        let bindings = doc.bindings();
        let mut touched: Vec<PageIndex> = Vec::new();
        for &p in &targets {
            let mut page = open_page(doc.pdf(), p)?;
            let rotation = rotation_of(&page);
            let crop = visible_box(&page)?;
            let (uw, uh) = target_user_size(rotation, &crop, size);
            let plan = resize_plan(&crop, uw, uh, mode);

            if !is_identity(plan.matrix) {
                if raw::object::object_count(bindings, &page) > 0 {
                    page.apply_matrix_with_clip(
                        PdfMatrix::new(
                            plan.matrix[0],
                            plan.matrix[1],
                            plan.matrix[2],
                            plan.matrix[3],
                            plan.matrix[4],
                            plan.matrix[5],
                        ),
                        to_pdf(plan.clip),
                    )
                    .ctx(&format!("transform the content of page {p}"))?;
                    // pdfium-render's `reload_in_place()` gives the page a fresh `FPDF_PAGE`
                    // but its boundaries / objects / annotations collections keep the closed
                    // one — touching them is a use-after-free. Reopen the page instead.
                    drop(page);
                    page = open_page(doc.pdf(), p)?;
                }
                if transform_annotations(bindings, &page, plan.matrix)? {
                    touched.push(p);
                }
            }

            let new_box = Rect::new(0.0, 0.0, uw, uh);
            // Trim / bleed / art boxes the page defines follow the content, clipped to the page.
            let extra = [
                explicit_box(page.boundaries().trim()),
                explicit_box(page.boundaries().bleed()),
                explicit_box(page.boundaries().art()),
            ];
            {
                let boxes = page.boundaries_mut();
                boxes
                    .set_media(to_pdf(new_box))
                    .ctx(&format!("set the media box of page {p}"))?;
                boxes
                    .set_crop(to_pdf(new_box))
                    .ctx(&format!("set the crop box of page {p}"))?;
                for (i, found) in extra.iter().enumerate() {
                    let Some(r) = found else {
                        continue;
                    };
                    let mapped = intersect(&map_rect(plan.matrix, r), &new_box).unwrap_or(new_box);
                    let set = match i {
                        0 => boxes.set_trim(to_pdf(mapped)),
                        1 => boxes.set_bleed(to_pdf(mapped)),
                        _ => boxes.set_art(to_pdf(mapped)),
                    };
                    set.ctx(&format!("set a page box of page {p}"))?;
                }
            }
        }
        // Markup appearances were regenerated from the new quads; the pre-save render pass
        // writes whatever PDFium still owes.
        doc.touched.extend(touched);
        Ok(())
    })?;
    refine_geometry(st, doc_id, &list)?;
    Ok(st.doc(doc_id)?.info())
}

/// Maps the annotations of `page` through `m` (module docs). Returns whether a markup
/// appearance was cleared, i.e. whether the page needs its pre-save render.
///
/// Order matters. `FPDFPage_TransformAnnots` runs **first**, on the untouched annotations: for
/// a text markup whose appearance PDFium generated, PDFium's `GetRect()` is the bounding box of
/// the *quads*, so mapping the quads first would map that rect twice. Afterwards the markups'
/// appearances are cleared, their quads mapped and their `/Rect` set to the mapped original
/// (no appearance left, so `FPDFAnnot_SetRect` touches no `BBox`); PDFium regenerates the
/// appearance from the new quads on the next render.
fn transform_annotations(
    bindings: &'static dyn PdfiumLibraryBindings,
    page: &PdfPage<'_>,
    m: Matrix,
) -> Result<bool, EngineError> {
    let before: Vec<(i32, Option<Rect>)> = (0..raw::annot::count(bindings, page))
        .map(|index| {
            raw::annot::get(bindings, page, index)
                .map(|a| (a.subtype(), a.rect()))
                .unwrap_or((consts::FPDF_ANNOT_UNKNOWN, None))
        })
        .collect();
    raw::page::transform_annots(bindings, page, m);

    let mut markup = false;
    for (index, (subtype, rect)) in before.into_iter().enumerate() {
        let mut a = raw::annot::get(bindings, page, index)?;
        let mapped = rect.map(|r| map_rect(m, &r));
        match subtype {
            consts::FPDF_ANNOT_HIGHLIGHT
            | consts::FPDF_ANNOT_UNDERLINE
            | consts::FPDF_ANNOT_SQUIGGLY
            | consts::FPDF_ANNOT_STRIKEOUT => {
                a.clear_ap();
                a.transform_quads(m);
                if let Some(r) = mapped {
                    a.set_rect(r);
                }
                markup = true;
            }
            consts::FPDF_ANNOT_LINK => {
                a.transform_quads(m);
            }
            consts::FPDF_ANNOT_INK => {
                let strokes = a.ink_paths();
                if !strokes.is_empty() {
                    a.remove_ink_list();
                    for stroke in strokes {
                        let mapped: Vec<f32> = stroke
                            .chunks_exact(2)
                            .flat_map(|pt| {
                                let (x, y) = apply(m, pt[0], pt[1]);
                                [x, y]
                            })
                            .collect();
                        if mapped.len() >= 4 {
                            a.add_ink_stroke(&mapped)?;
                        }
                    }
                }
            }
            // PDFium's annotation list skips popups, so `TransformAnnots` did not map them.
            consts::FPDF_ANNOT_POPUP => {
                if let Some(r) = mapped {
                    a.set_rect(r);
                }
            }
            _ => {}
        }
    }
    Ok(markup)
}

// ---------------------------------------------------------------------------------------
// Geometry (pure; unit-tested)
// ---------------------------------------------------------------------------------------

/// The content matrix and clip of one resized page.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ResizePlan {
    /// Old user space → new user space: `[s, 0, 0, s, tx, ty]`.
    pub matrix: Matrix,
    /// Where the old crop box lands, clipped to the new page (new user space).
    pub clip: Rect,
}

/// Maps the old crop box `crop` into a `w × h` page at the origin.
pub fn resize_plan(crop: &Rect, w: f32, h: f32, mode: ResizeMode) -> ResizePlan {
    let (cw, ch) = (crop.width().max(MIN_SIDE), crop.height().max(MIN_SIDE));
    let s = match mode {
        ResizeMode::ScaleContent => (w / cw).min(h / ch),
        ResizeMode::CenterContent => 1.0,
    };
    let tx = (w - cw * s) / 2.0 - crop.l * s;
    let ty = (h - ch * s) / 2.0 - crop.b * s;
    let matrix = [s, 0.0, 0.0, s, tx, ty];
    let page = Rect::new(0.0, 0.0, w, h);
    let clip = intersect(&map_rect(matrix, crop), &page).unwrap_or(page);
    ResizePlan { matrix, clip }
}

/// The new page's size in **unrotated user space** for a page with `/Rotate` = `rotation`
/// and visible box `crop`.
pub fn target_user_size(rotation: u16, crop: &Rect, target: ResizeTarget) -> (f32, f32) {
    let quarter = rotation % 180 == 90;
    let (vw, vh) = if quarter {
        (crop.height(), crop.width())
    } else {
        (crop.width(), crop.height())
    };
    let (tw, th) = match target {
        ResizeTarget::Named(name) => {
            let (pw, ph) = name.size_pt();
            if vw > vh {
                (ph, pw)
            } else {
                (pw, ph)
            }
        }
        ResizeTarget::Size { w, h } => (w, h),
    };
    if quarter {
        (th, tw)
    } else {
        (tw, th)
    }
}

/// Margins (as the page is seen) inset from `crop`, as a rect in unrotated user space.
pub fn margins_to_user(rotation: u16, crop: &Rect, m: &Margins) -> Result<Rect, EngineError> {
    let (vw, vh) = if rotation % 180 == 90 {
        (crop.height(), crop.width())
    } else {
        (crop.width(), crop.height())
    };
    let (u0, v0, u1, v1) = (m.left, m.bottom, vw - m.right, vh - m.top);
    if u1 - u0 < MIN_SIDE || v1 - v0 < MIN_SIDE {
        return Err(EngineError::invalid(format!(
            "the margins leave less than {MIN_SIDE} pt of the page"
        )));
    }
    let to_user = visual_to_user(rotation, *crop);
    let (x0, y0) = apply(to_user, u0, v0);
    let (x1, y1) = apply(to_user, u1, v1);
    Ok(normalize(Rect::new(x0, y0, x1, y1)))
}

pub fn apply(m: Matrix, x: f32, y: f32) -> (f32, f32) {
    (m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5])
}

/// The axis-aligned bounds of `r` mapped through `m`.
pub fn map_rect(m: Matrix, r: &Rect) -> Rect {
    let corners = [
        apply(m, r.l, r.b),
        apply(m, r.r, r.b),
        apply(m, r.l, r.t),
        apply(m, r.r, r.t),
    ];
    let mut out = Rect::new(f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for (x, y) in corners {
        out.l = out.l.min(x);
        out.b = out.b.min(y);
        out.r = out.r.max(x);
        out.t = out.t.max(y);
    }
    out
}

pub fn normalize(r: Rect) -> Rect {
    Rect::new(r.l.min(r.r), r.b.min(r.t), r.l.max(r.r), r.b.max(r.t))
}

/// The overlap of two rects, or `None` when it has no area.
pub fn intersect(a: &Rect, b: &Rect) -> Option<Rect> {
    let r = Rect::new(a.l.max(b.l), a.b.max(b.b), a.r.min(b.r), a.t.min(b.t));
    (r.r > r.l && r.t > r.b).then_some(r)
}

fn is_identity(m: Matrix) -> bool {
    (m[0] - 1.0).abs() < 1e-6
        && m[1].abs() < 1e-6
        && m[2].abs() < 1e-6
        && (m[3] - 1.0).abs() < 1e-6
        && m[4].abs() < 1e-3
        && m[5].abs() < 1e-3
}

fn check_box(r: &Rect, what: &str) -> Result<(), EngineError> {
    let finite = [r.l, r.b, r.r, r.t].iter().all(|v| v.is_finite() && v.abs() <= HUGE);
    let r = normalize(*r);
    if !finite || r.width() < MIN_SIDE || r.height() < MIN_SIDE {
        return Err(EngineError::invalid(format!(
            "{what} box must be finite and at least {MIN_SIDE} pt on each side"
        )));
    }
    if r.width() > MAX_SIDE || r.height() > MAX_SIDE {
        return Err(EngineError::invalid(format!(
            "{what} box is larger than {MAX_SIDE} pt"
        )));
    }
    Ok(())
}

fn check_margins(m: &Margins) -> Result<(), EngineError> {
    if [m.top, m.right, m.bottom, m.left]
        .iter()
        .any(|v| !v.is_finite() || *v < 0.0)
    {
        return Err(EngineError::invalid("crop margins must be finite and >= 0"));
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------
// Page helpers
// ---------------------------------------------------------------------------------------

fn resolve_pages(selection: &PageSelection, count: u16) -> Result<Vec<PageIndex>, EngineError> {
    let mut pages: Vec<PageIndex> = match selection {
        PageSelection::All(_) => (0..count).collect(),
        PageSelection::List(list) => list.clone(),
    };
    pages.sort_unstable();
    pages.dedup();
    if pages.is_empty() {
        return Err(EngineError::invalid("no page was selected"));
    }
    if let Some(&max) = pages.last() {
        if max >= count {
            return Err(
                EngineError::invalid(format!("page {max} is outside 0..{count}")).with_page(max),
            );
        }
    }
    Ok(pages)
}

fn open_page<'p>(document: &PdfDocument<'p>, p: PageIndex) -> Result<PdfPage<'p>, EngineError> {
    let mut page = document
        .pages()
        .get(p as PdfPageIndex)
        .ctx(&format!("load page {p}"))?;
    // Boxes are page attributes and the content transform writes its own streams: nothing may
    // regenerate the content from the object list on drop.
    page.set_content_regeneration_strategy(PdfPageContentRegenerationStrategy::Manual);
    Ok(page)
}

fn rotation_of(page: &PdfPage<'_>) -> u16 {
    page.rotation()
        .map(|r| r.as_degrees() as i32)
        .unwrap_or(0)
        .rem_euclid(360) as u16
}

fn from_pdf(r: &PdfRect) -> Rect {
    normalize(Rect::new(
        r.left().value,
        r.bottom().value,
        r.right().value,
        r.top().value,
    ))
}

/// `Rect { l, b, r, t }` → `PdfRect::new_from_values(bottom, left, top, right)`.
fn to_pdf(r: Rect) -> PdfRect {
    PdfRect::new_from_values(r.b, r.l, r.t, r.r)
}

/// A box the page dictionary defines itself (`None` when absent or inherited).
fn explicit_box(found: Result<PdfPageBoundaryBox, PdfiumError>) -> Option<Rect> {
    found.ok().map(|b| from_pdf(&b.bounds))
}

/// What the page shows: its own crop box, else its own media box, else PDFium's page box
/// (which honours inherited boxes).
fn visible_box(page: &PdfPage<'_>) -> Result<Rect, EngineError> {
    if let Some(r) = explicit_box(page.boundaries().crop())
        .or_else(|| explicit_box(page.boundaries().media()))
    {
        return Ok(r);
    }
    let bounding = page.boundaries().bounding().ctx("read the page box")?;
    Ok(from_pdf(&bounding.bounds))
}

/// The page's media box, inherited or not.
///
/// `FPDFPage_GetMediaBox` only sees the page's own dictionary. For an inherited one, the crop
/// box is widened to [`HUGE`] so that PDFium's page box (media ∩ crop) *is* the media box, and
/// then restored — an absent crop box comes back as an explicit one equal to the media box,
/// which is the same page.
fn true_media(page: &mut PdfPage<'_>, p: PageIndex) -> Result<Rect, EngineError> {
    if let Some(m) = explicit_box(page.boundaries().media()) {
        return Ok(m);
    }
    let saved = explicit_box(page.boundaries().crop());
    page.boundaries_mut()
        .set_crop(to_pdf(Rect::new(-HUGE, -HUGE, HUGE, HUGE)))
        .ctx(&format!("probe the media box of page {p}"))?;
    let media = from_pdf(
        &page
            .boundaries()
            .bounding()
            .ctx(&format!("read the media box of page {p}"))?
            .bounds,
    );
    page.boundaries_mut()
        .set_crop(to_pdf(saved.unwrap_or(media)))
        .ctx(&format!("restore the crop box of page {p}"))?;
    Ok(media)
}

/// `mutate` re-reads the exact crop box and rotation of at most 32 named pages; a crop of
/// "all pages" in a long document refines the rest here, so `DocInfo.pages` is exact for every
/// page this call changed.
fn refine_geometry(
    st: &mut EngineState<'_>,
    doc_id: &str,
    pages: &[PageIndex],
) -> Result<(), EngineError> {
    if pages.len() <= 32 {
        return Ok(());
    }
    let doc = st.doc_mut(doc_id)?;
    for &p in pages {
        if p < doc.page_count() {
            doc.invalidate_page_handle(p);
            let _ = doc.page(p)?;
        }
    }
    // Publish the refined geometry to the protocol thread as well.
    let summary = doc.summary();
    st.shared.docs.write().insert(doc_id.to_string(), summary);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::types::PaperName;

    const EPS: f32 = 1e-3;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < EPS
    }

    #[test]
    fn a4_to_letter_scales_uniformly_and_centres() {
        let a4 = Rect::new(0.0, 0.0, 595.28, 841.89);
        let plan = resize_plan(&a4, 612.0, 792.0, ResizeMode::ScaleContent);
        let s = (612.0f32 / 595.28).min(792.0 / 841.89);
        assert!(close(plan.matrix[0], s) && close(plan.matrix[3], s));
        let moved = map_rect(plan.matrix, &a4);
        // height-bound: fills the height, centred horizontally
        assert!(close(moved.b, 0.0) && close(moved.t, 792.0));
        assert!(close(moved.l, 612.0 - moved.r), "centred: {moved:?}");
        assert!(close(moved.width() / moved.height(), 595.28 / 841.89));
        assert_eq!(plan.clip, moved);
    }

    #[test]
    fn centre_mode_keeps_the_size_and_clips_to_the_page() {
        let crop = Rect::new(50.0, 60.0, 650.0, 860.0); // 600 × 800 away from the origin
        let plan = resize_plan(&crop, 400.0, 500.0, ResizeMode::CenterContent);
        assert!(close(plan.matrix[0], 1.0));
        let moved = map_rect(plan.matrix, &crop);
        assert!(close(moved.l, -100.0) && close(moved.r, 500.0));
        assert!(close(moved.b, -150.0) && close(moved.t, 650.0));
        assert_eq!(plan.clip, Rect::new(0.0, 0.0, 400.0, 500.0));
    }

    #[test]
    fn named_sizes_keep_the_page_orientation() {
        let portrait = Rect::new(0.0, 0.0, 612.0, 792.0);
        let landscape = Rect::new(0.0, 0.0, 792.0, 612.0);
        let a4 = ResizeTarget::Named(PaperName::A4);
        assert_eq!(target_user_size(0, &portrait, a4), (595.28, 841.89));
        assert_eq!(target_user_size(0, &landscape, a4), (841.89, 595.28));
        // /Rotate 90 on a portrait user box = landscape as seen: landscape A4 as seen is a
        // portrait user box again.
        assert_eq!(target_user_size(90, &portrait, a4), (595.28, 841.89));
        // `{ w, h }` is as seen, so /Rotate 90 swaps it into user space
        let explicit = ResizeTarget::Size { w: 300.0, h: 200.0 };
        assert_eq!(target_user_size(90, &portrait, explicit), (200.0, 300.0));
        assert_eq!(target_user_size(0, &portrait, explicit), (300.0, 200.0));
    }

    #[test]
    fn margins_follow_the_displayed_edges() {
        let crop = Rect::new(0.0, 0.0, 600.0, 800.0);
        let m = Margins {
            top: 10.0,
            right: 20.0,
            bottom: 30.0,
            left: 40.0,
        };
        assert_eq!(
            margins_to_user(0, &crop, &m).unwrap(),
            Rect::new(40.0, 30.0, 580.0, 790.0)
        );
        // /Rotate 90: the displayed top edge is user x = 0 … the displayed left edge is user y = 0
        let r = margins_to_user(90, &crop, &m).unwrap();
        assert_eq!(r, Rect::new(10.0, 40.0, 570.0, 780.0));
        let too_much = Margins {
            top: 400.0,
            right: 0.0,
            bottom: 400.0,
            left: 0.0,
        };
        assert!(margins_to_user(0, &crop, &too_much).is_err());
    }

    #[test]
    fn boxes_are_validated() {
        assert!(check_box(&Rect::new(0.0, 0.0, 100.0, 100.0), "crop").is_ok());
        assert!(check_box(&Rect::new(100.0, 100.0, 0.0, 0.0), "crop").is_ok(), "normalised");
        assert!(check_box(&Rect::new(0.0, 0.0, 0.5, 100.0), "crop").is_err());
        assert!(check_box(&Rect::new(0.0, 0.0, f32::NAN, 100.0), "crop").is_err());
        assert!(check_box(&Rect::new(0.0, 0.0, 20_000.0, 100.0), "media").is_err());
        assert!(check_margins(&Margins { top: -1.0, right: 0.0, bottom: 0.0, left: 0.0 }).is_err());
        assert_eq!(
            intersect(&Rect::new(0.0, 0.0, 10.0, 10.0), &Rect::new(5.0, 5.0, 20.0, 20.0)),
            Some(Rect::new(5.0, 5.0, 10.0, 10.0))
        );
        assert_eq!(
            intersect(&Rect::new(0.0, 0.0, 10.0, 10.0), &Rect::new(10.0, 0.0, 20.0, 20.0)),
            None
        );
    }
}
