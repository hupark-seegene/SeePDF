//! Watermarks, headers and footers — P1-4, `IPC_CONTRACT.md` §7.4a (`add_stamp`).
//!
//! A stamp is ordinary page content, not an annotation: text becomes one text object per line
//! (Helvetica for Latin-1, the bundled Hangul subset otherwise — the same fonts and the same
//! once-per-document embedding as `objects::add_text`), an image becomes one Form XObject
//! that every stamped page references, so a logo on 300 pages is stored once.
//!
//! ## Geometry
//!
//! Everything is laid out in **visual space**: origin at the bottom-left of the page *as the
//! user sees it* (after `/Rotate`), y up, size = the crop box with width/height swapped for
//! 90° / 270°. The stamp box is rotated about its own centre by `rotateDeg`
//! (counter-clockwise, as seen) and its **rotated bounding box** is what anchor + margin place
//! (Stage 8: before, the unrotated box was anchored, so a 45° watermark in a corner poked past
//! the page edge). The result is mapped into the page's unrotated user space with
//! [`visual_to_user`]. That one matrix is why a "top-left" header stays top-left and upright
//! on a page with `/Rotate 90`.
//!
//! ## Opacity
//!
//! Text: fill and stroke alpha on the object (`FPDFPageObj_SetFillColor`), which PDFium writes
//! as an `/ExtGState` with `/ca` / `/CA`. Images: the alpha is baked into the image's own
//! alpha channel (an `/SMask`), because PDFium's content generator emits no graphics state for
//! form or image objects.
//!
//! ## Tokens
//!
//! `{{page}}`, `{{total}}`, `{{date}}`, `{{filename}}` and (P2) `{{bates}}` are expanded per page
//! in one pass ([`expand_tokens`]). A Bates number ([`bates_label`]) counts the **stamped** pages
//! in ascending order from `batesStart`, zero-padded to `batesDigits`, between `batesPrefix` and
//! `batesSuffix`; it is ordinary Helvetica text, so it is searchable and extractable.
//!
//! ## Marking
//!
//! Every object we add carries the marked-content tag `SeePDF:Stamp` (`FPDFPageObj_AddMark`)
//! with a `role` string param (Stage 8: `watermark` / `header` / `footer`), so
//! [`remove_stamps`] can find exactly what this command wrote — all of it, or one role.
//! Stamps written before Stage 8 carry the bare mark and are removed only by an
//! all-roles removal.

use crate::engine::annot::ScratchPage;
use crate::engine::raw;
use crate::engine::raw::object::{Mark, Matrix, XObject};
use crate::engine::registry::{self, MutateOpts};
use crate::engine::types::EngineState;
use crate::ipc::error::PdfiumResultExt;
use crate::ipc::types::{
    BatesOptions, ChangeReason, PageIndex, PageSelection, PageStampSource, PageStampSpec, Rect,
    RemoveStampsResult, StampAnchor, StampResult, StampRole,
};
use crate::ipc::{EngineError, ErrorCode};
use pdfium_render::prelude::*;

/// The content-mark name every stamp object carries.
pub const STAMP_MARK: &str = "SeePDF:Stamp";
/// The mark's string param naming the stamp's [`StampRole`] (Stage 8).
pub const ROLE_PARAM: &str = "role";
/// Line spacing, as a multiple of the font size (same as `add_text_object`).
const LINE_HEIGHT: f32 = 1.2;
/// Widest zero-padding a Bates number may ask for (P2).
pub const BATES_MAX_DIGITS: u8 = 12;
/// Longest Bates prefix / suffix, in characters.
pub const BATES_MAX_AFFIX: usize = 64;

/// `add_stamp` — one undo step for the whole operation.
pub fn add_stamp(
    st: &mut EngineState<'_>,
    doc_id: &str,
    spec: &PageStampSpec,
) -> Result<StampResult, EngineError> {
    validate(spec)?;
    let (page_count, file_stem) = {
        let doc = st.doc(doc_id)?;
        // `displayName` first: a recovered copy stamps the original name, not `<uuid>`.
        let stem = if doc.path.is_some() || doc.display_name.is_some() {
            doc.name_stem()
        } else {
            "Untitled".to_string()
        };
        (doc.page_count(), stem)
    };
    let pages = resolve_pages(&spec.pages, page_count)?;

    // An image stamp is drawn once on a one-page scratch document and copied into the target
    // as a single Form XObject.
    let image_source = match &spec.source {
        PageStampSource::Image { path, width_pt } => {
            Some(image_page(st.pdfium, path, *width_pt, spec.opacity)?)
        }
        PageStampSource::Text { .. } => None,
    };

    let label = match spec.role {
        StampRole::Watermark => "undo.watermark",
        StampRole::Header | StampRole::Footer => "undo.headerFooter",
    };
    let opts = MutateOpts::new(label, ChangeReason::Edit);
    let opts = match &spec.pages {
        PageSelection::All(_) => opts.all_pages(),
        PageSelection::List(_) => opts.pages(pages.clone()),
    };
    let date = chrono::Local::now().format("%Y-%m-%d").to_string();
    let total = page_count;
    let spec = spec.clone();

    let stamped = registry::mutate(st, doc_id, opts, move |doc| {
        for &p in &pages {
            doc.invalidate_page_handle(p);
        }
        // Text: resolve every page's lines and their fonts first — font loading needs
        // `&mut OpenDoc`, page work below only `&PdfDocument`.
        let mut texts: Vec<Vec<(String, PdfFontToken)>> = Vec::new();
        if let PageStampSource::Text { text, .. } = &spec.source {
            {
                // Re-use a copy of the bundled font this document already embeds.
                let scratch = ScratchPage::open(doc, pages[0])?;
                doc.adopt_hangul_token(&scratch.page);
            }
            for (n, &p) in pages.iter().enumerate() {
                let resolved = expand_tokens(
                    text,
                    &Tokens {
                        page: p + 1,
                        total,
                        date: &date,
                        filename: &file_stem,
                        bates: &bates_label(&spec.bates, n),
                    },
                );
                let mut lines = Vec::new();
                for line in resolved.replace("\r\n", "\n").split('\n') {
                    if line.trim().is_empty() {
                        // Keeps the line's slot in the layout; no object is created.
                        lines.push((String::new(), doc.hangul_token_for("A")?.0));
                    } else {
                        let (token, _) = doc.hangul_token_for(line)?;
                        lines.push((line.to_string(), token));
                    }
                }
                texts.push(lines);
            }
        }

        let bindings = doc.bindings();
        let document = doc.pdf();
        let xobject = match &image_source {
            Some(source) => Some((
                XObject::from_page(bindings, document, &source.doc, 0)?,
                source.width,
                source.height,
            )),
            None => None,
        };
        for (i, &p) in pages.iter().enumerate() {
            let mut page = document
                .pages()
                .get(p as PdfPageIndex)
                .ctx(&format!("load page {p}"))?;
            page.set_content_regeneration_strategy(PdfPageContentRegenerationStrategy::Manual);
            let geom = registry::geom_from_page(p, &page)
                .ok_or_else(|| EngineError::new(ErrorCode::Pdfium, "read page geometry"))?;
            let space = VisualSpace::new(geom.rotation, geom.crop);
            match (&spec.source, &xobject) {
                (
                    PageStampSource::Text {
                        font_size_pt,
                        color,
                        ..
                    },
                    _,
                ) => {
                    place_text(
                        bindings,
                        document,
                        &mut page,
                        &texts[i],
                        *font_size_pt,
                        *color,
                        &spec,
                        &space,
                    )?;
                }
                (PageStampSource::Image { .. }, Some((xobject, w, h))) => {
                    // The scratch page was `w × h` points, so the Form XObject already has the
                    // stamp's size and only needs placing.
                    let matrix = placement(&spec, &space, *w, *h);
                    let role = [(ROLE_PARAM, spec.role.as_str())];
                    xobject.place(
                        &page,
                        matrix,
                        Some(Mark {
                            name: STAMP_MARK,
                            params: &role,
                        }),
                    )?;
                }
                (PageStampSource::Image { .. }, None) => unreachable!("image source prepared"),
            }
            page.regenerate_content().ctx("regenerate page content")?;
        }
        drop(xobject);
        Ok(pages.len() as u32)
    })?;
    Ok(StampResult {
        info: st.doc(doc_id)?.info(),
        pages_stamped: stamped,
    })
}

fn validate(spec: &PageStampSpec) -> Result<(), EngineError> {
    if !spec.margin_pt.is_finite() || spec.margin_pt < 0.0 {
        return Err(EngineError::invalid("marginPt must be >= 0"));
    }
    if !spec.rotate_deg.is_finite() || !(-180.0..=180.0).contains(&spec.rotate_deg) {
        return Err(EngineError::invalid("rotateDeg must be within -180..180"));
    }
    if !spec.opacity.is_finite() || !(0.0..=1.0).contains(&spec.opacity) {
        return Err(EngineError::invalid("opacity must be within 0..1"));
    }
    let bates = &spec.bates;
    if bates.bates_digits == 0 || bates.bates_digits > BATES_MAX_DIGITS {
        return Err(EngineError::invalid(format!(
            "batesDigits must be within 1..={BATES_MAX_DIGITS}"
        )));
    }
    if bates.bates_prefix.chars().count() > BATES_MAX_AFFIX
        || bates.bates_suffix.chars().count() > BATES_MAX_AFFIX
    {
        return Err(EngineError::invalid(format!(
            "batesPrefix / batesSuffix are limited to {BATES_MAX_AFFIX} characters"
        )));
    }
    if bates.bates_start > 999_999_999_999_999 {
        return Err(EngineError::invalid("batesStart is out of range"));
    }
    match &spec.source {
        PageStampSource::Text {
            text, font_size_pt, ..
        } => {
            if text.trim().is_empty() {
                return Err(EngineError::invalid("the stamp text is empty"));
            }
            if !font_size_pt.is_finite() || *font_size_pt <= 0.0 || *font_size_pt > 1638.0 {
                return Err(EngineError::invalid(format!(
                    "font size {font_size_pt} pt is out of range"
                )));
            }
        }
        PageStampSource::Image { path, width_pt } => {
            if !width_pt.is_finite() || *width_pt <= 0.0 || *width_pt > 14_400.0 {
                return Err(EngineError::invalid(format!(
                    "image width {width_pt} pt is out of range"
                )));
            }
            if path.is_empty() {
                return Err(EngineError::invalid("the image path is empty"));
            }
        }
    }
    Ok(())
}

fn resolve_pages(selection: &PageSelection, count: u16) -> Result<Vec<PageIndex>, EngineError> {
    let mut pages: Vec<PageIndex> = match selection {
        PageSelection::All(_) => (0..count).collect(),
        PageSelection::List(list) => list.clone(),
    };
    pages.sort_unstable();
    pages.dedup();
    if pages.is_empty() {
        return Err(EngineError::invalid("no pages to stamp"));
    }
    if let Some(&max) = pages.last() {
        if max >= count {
            return Err(EngineError::invalid(format!(
                "page {max} is out of range (page count {count})"
            ))
            .with_page(max));
        }
    }
    Ok(pages)
}

/// `{{page}}` (1-based), `{{total}}`, `{{date}}`, `{{filename}}`; anything else stays literal.
/// `{{bates}}` expands to nothing here — see [`expand_tokens`].
pub fn substitute(template: &str, page: u16, total: u16, date: &str, filename: &str) -> String {
    expand_tokens(
        template,
        &Tokens {
            page,
            total,
            date,
            filename,
            bates: "",
        },
    )
}

/// The values the stamp tokens expand to on one page.
pub struct Tokens<'a> {
    /// 1-based.
    pub page: u16,
    pub total: u16,
    pub date: &'a str,
    pub filename: &'a str,
    /// The page's Bates number ([`bates_label`]).
    pub bates: &'a str,
}

/// One left-to-right pass over `template`: `{{page}}`, `{{total}}`, `{{date}}`,
/// `{{filename}}` and `{{bates}}` are replaced, anything else (an unknown `{{x}}`, a lone
/// `{{`) stays literal. A single pass, so a file name or Bates prefix that itself contains
/// `{{page}}` is written as typed rather than expanded a second time.
pub fn expand_tokens(template: &str, tokens: &Tokens<'_>) -> String {
    let mut out = String::with_capacity(template.len() + 16);
    let mut rest = template;
    while let Some(open) = rest.find("{{") {
        out.push_str(&rest[..open]);
        let after = &rest[open + 2..];
        let Some(close) = after.find("}}") else {
            out.push_str(&rest[open..]);
            return out;
        };
        let value = match &after[..close] {
            "page" => Some(tokens.page.to_string()),
            "total" => Some(tokens.total.to_string()),
            "date" => Some(tokens.date.to_string()),
            "filename" => Some(tokens.filename.to_string()),
            "bates" => Some(tokens.bates.to_string()),
            _ => None,
        };
        match value {
            Some(v) => {
                out.push_str(&v);
                rest = &after[close + 2..];
            }
            None => {
                // Keep the braces and continue right after them, so `{{{{page}}` still finds
                // the inner token.
                out.push_str("{{");
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// P2 Bates number of the `n`-th stamped page (0-based, ascending page order):
/// prefix + (`start` + n, zero-padded) + suffix. A number wider than the padding is written in
/// full rather than cut.
pub fn bates_label(opts: &BatesOptions, n: usize) -> String {
    let number = opts.bates_start.saturating_add(n as u64);
    let width = opts.bates_digits as usize;
    format!("{}{:0width$}{}", opts.bates_prefix, number, opts.bates_suffix)
}

// ---------------------------------------------------------------------------------------
// Geometry
// ---------------------------------------------------------------------------------------

/// `a × b` in PDF's row-vector convention: apply `a` first, then `b`.
fn mul(a: Matrix, b: Matrix) -> Matrix {
    [
        a[0] * b[0] + a[1] * b[2],
        a[0] * b[1] + a[1] * b[3],
        a[2] * b[0] + a[3] * b[2],
        a[2] * b[1] + a[3] * b[3],
        a[4] * b[0] + a[5] * b[2] + b[4],
        a[4] * b[1] + a[5] * b[3] + b[5],
    ]
}

fn translate(x: f32, y: f32) -> Matrix {
    [1.0, 0.0, 0.0, 1.0, x, y]
}

/// Counter-clockwise by `deg` in a y-up space.
fn rotate(deg: f32) -> Matrix {
    let (s, c) = deg.to_radians().sin_cos();
    [c, s, -s, c, 0.0, 0.0]
}

/// The page as the user sees it.
pub struct VisualSpace {
    pub width: f32,
    pub height: f32,
    /// Visual → unrotated user space.
    pub to_user: Matrix,
}

impl VisualSpace {
    pub fn new(rotation: u16, crop: Rect) -> Self {
        let (w, h) = (crop.width(), crop.height());
        let (width, height) = if rotation % 180 == 90 { (h, w) } else { (w, h) };
        Self {
            width,
            height,
            to_user: visual_to_user(rotation, crop),
        }
    }
}

/// Maps visual coordinates `(u, v)` (origin bottom-left of the displayed page, y up) to the
/// page's unrotated user space, for `/Rotate` = `rotation` (clockwise, as PDF defines it).
///
/// * 0:   `x = l + u`, `y = b + v`
/// * 90:  `x = r − v`, `y = b + u` — the displayed bottom-left is the user-space bottom-right
/// * 180: `x = r − u`, `y = t − v`
/// * 270: `x = l + v`, `y = t − u`
pub fn visual_to_user(rotation: u16, crop: Rect) -> Matrix {
    match rotation % 360 {
        90 => [0.0, 1.0, -1.0, 0.0, crop.r, crop.b],
        180 => [-1.0, 0.0, 0.0, -1.0, crop.r, crop.t],
        270 => [0.0, -1.0, 1.0, 0.0, crop.l, crop.t],
        _ => [1.0, 0.0, 0.0, 1.0, crop.l, crop.b],
    }
}

/// Bottom-left of a `w × h` box placed by `anchor` + `margin` on the visual page.
pub fn anchor_origin(
    anchor: StampAnchor,
    margin: f32,
    space: &VisualSpace,
    w: f32,
    h: f32,
) -> (f32, f32) {
    use StampAnchor::*;
    let x = match anchor {
        Tl | Ml | Bl => margin,
        Tc | Mc | Bc => (space.width - w) / 2.0,
        Tr | Mr | Br => space.width - margin - w,
    };
    let y = match anchor {
        Tl | Tc | Tr => space.height - margin - h,
        Ml | Mc | Mr => (space.height - h) / 2.0,
        Bl | Bc | Br => margin,
    };
    (x, y)
}

/// Width and height of the axis-aligned bounding box of a `w × h` box rotated by `deg`.
pub fn rotated_extent(w: f32, h: f32, deg: f32) -> (f32, f32) {
    let (s, c) = deg.to_radians().sin_cos();
    let (s, c) = (s.abs(), c.abs());
    (w * c + h * s, w * s + h * c)
}

/// Box-local → user space: rotate about the box centre, place the **rotated bounding box** by
/// anchor + margin (so a rotated corner stamp stays inside the margin), un-rotate the page.
fn placement(spec: &PageStampSpec, space: &VisualSpace, w: f32, h: f32) -> Matrix {
    let (bw, bh) = rotated_extent(w, h, spec.rotate_deg);
    let (x, y) = anchor_origin(spec.anchor, spec.margin_pt, space, bw, bh);
    let about_centre = mul(
        mul(translate(-w / 2.0, -h / 2.0), rotate(spec.rotate_deg)),
        translate(x + bw / 2.0, y + bh / 2.0),
    );
    mul(about_centre, space.to_user)
}

fn to_pdf(m: Matrix) -> PdfMatrix {
    PdfMatrix::new(m[0], m[1], m[2], m[3], m[4], m[5])
}

// ---------------------------------------------------------------------------------------
// Text
// ---------------------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn place_text<'p>(
    bindings: &dyn PdfiumLibraryBindings,
    document: &PdfDocument<'p>,
    page: &mut PdfPage<'p>,
    lines: &[(String, PdfFontToken)],
    size: f32,
    color: [u8; 3],
    spec: &PageStampSpec,
    space: &VisualSpace,
) -> Result<(), EngineError> {
    let alpha = (spec.opacity * 255.0).round().clamp(0.0, 255.0) as u8;
    let fill = PdfColor::new(color[0], color[1], color[2], alpha);

    // Create and measure every line at the origin first: the box needs the widest one.
    let mut objects: Vec<Option<(PdfPageTextObject<'p>, f32, f32)>> = Vec::new();
    let mut box_w: f32 = 0.0;
    for (line, token) in lines {
        if line.is_empty() {
            objects.push(None);
            continue;
        }
        let mut object = PdfPageTextObject::new(document, line, *token, PdfPoints::new(size))
            .ctx("create text object")?;
        object.set_fill_color(fill).ctx("set_fill_color")?;
        object.set_stroke_color(fill).ctx("set_stroke_color")?;
        let natural = object.bounds().ctx("measure text object")?.to_rect();
        let (left, width) = (natural.left().value, natural.width().value);
        box_w = box_w.max(width);
        objects.push(Some((object, left, width)));
    }
    let box_h = lines.len() as f32 * size * LINE_HEIGHT;
    let place = placement(spec, space, box_w, box_h);

    let mut baseline = box_h - size;
    for entry in objects {
        if let Some((mut object, left, width)) = entry {
            use StampAnchor::*;
            let x = match spec.anchor {
                Tl | Ml | Bl => 0.0,
                Tc | Mc | Bc => (box_w - width) / 2.0,
                Tr | Mr | Br => box_w - width,
            } - left;
            let m = mul(translate(x, baseline), place);
            object.apply_matrix(to_pdf(m)).ctx("place text object")?;
            page.objects_mut()
                .add_text_object(object)
                .ctx("add text object")?;
            let index = raw::object::object_count(bindings, page).saturating_sub(1);
            let role = [(ROLE_PARAM, spec.role.as_str())];
            raw::object::add_mark(
                bindings,
                document,
                page,
                index,
                Mark {
                    name: STAMP_MARK,
                    params: &role,
                },
            )?;
        }
        baseline -= size * LINE_HEIGHT;
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------
// remove_stamps (Stage 8)
// ---------------------------------------------------------------------------------------

/// `remove_stamps` — deletes the page objects carrying the `SeePDF:Stamp` mark on `pages`
/// (default: every page), of `role` only when given. One undo step `undo.removeStamps`;
/// finding nothing is not an error and changes nothing (no undo step, same generation).
///
/// Stamps written before Stage 8 have no `role` param, so a role-filtered removal leaves them
/// alone; an all-roles removal takes them too.
pub fn remove_stamps(
    st: &mut EngineState<'_>,
    doc_id: &str,
    pages: Option<&PageSelection>,
    role: Option<StampRole>,
) -> Result<RemoveStampsResult, EngineError> {
    let count = st.doc(doc_id)?.page_count();
    let pages = match pages {
        None | Some(PageSelection::All(_)) => (0..count).collect::<Vec<_>>(),
        Some(selection) => resolve_pages(selection, count)?,
    };

    // Read-only pass first: which pages have anything to remove.
    let mut hits: Vec<PageIndex> = Vec::new();
    {
        let doc = st.doc_mut(doc_id)?;
        let bindings = doc.bindings();
        for &p in &pages {
            let page = open_direct(doc.pdf(), p)?;
            if !stamp_indices(bindings, &page, role).is_empty() {
                hits.push(p);
            }
        }
    }
    if hits.is_empty() {
        return Ok(RemoveStampsResult {
            info: st.doc(doc_id)?.info(),
            removed: 0,
        });
    }

    let opts = MutateOpts::new("undo.removeStamps", ChangeReason::Edit).pages(hits.clone());
    let removed = registry::mutate(st, doc_id, opts, move |doc| {
        let mut removed = 0u32;
        for &p in &hits {
            let mut scratch = ScratchPage::open(doc, p)?;
            let indices = stamp_indices(doc.bindings(), &scratch.page, role);
            for &index in indices.iter().rev() {
                scratch
                    .page
                    .objects_mut()
                    .remove_object_at_index(index)
                    .ctx(&format!("remove stamp object {index}"))?;
                removed += 1;
            }
            scratch
                .page
                .regenerate_content()
                .ctx("regenerate page content")?;
        }
        Ok(removed)
    })?;
    Ok(RemoveStampsResult {
        info: st.doc(doc_id)?.info(),
        removed,
    })
}

fn open_direct<'p>(document: &PdfDocument<'p>, p: PageIndex) -> Result<PdfPage<'p>, EngineError> {
    let mut page = document
        .pages()
        .get(p as PdfPageIndex)
        .ctx(&format!("load page {p}"))?;
    page.set_content_regeneration_strategy(PdfPageContentRegenerationStrategy::Manual);
    Ok(page)
}

/// Every stamp on `page`: its top-level object index (ascending) and its role (`None` when the
/// `SeePDF:Stamp` mark carries no role this build knows).
pub fn stamp_roles(
    bindings: &dyn PdfiumLibraryBindings,
    page: &PdfPage<'_>,
) -> Vec<(usize, Option<StampRole>)> {
    (0..raw::object::object_count(bindings, page))
        .filter_map(|i| {
            raw::object::mark_param(bindings, page, i, STAMP_MARK, ROLE_PARAM)
                .map(|found| (i, found.as_deref().and_then(StampRole::from_name)))
        })
        .collect()
}

/// Indices (ascending) of the top-level objects on `page` that are stamps of `role` (any role
/// when `None`).
pub fn stamp_indices(
    bindings: &dyn PdfiumLibraryBindings,
    page: &PdfPage<'_>,
    role: Option<StampRole>,
) -> Vec<usize> {
    (0..raw::object::object_count(bindings, page))
        .filter(
            |&i| match raw::object::mark_param(bindings, page, i, STAMP_MARK, ROLE_PARAM) {
                None => false,
                Some(found) => match role {
                    None => true,
                    Some(role) => found.as_deref().and_then(StampRole::from_name) == Some(role),
                },
            },
        )
        .collect()
}

// ---------------------------------------------------------------------------------------
// Image
// ---------------------------------------------------------------------------------------

/// A one-page scratch document holding the image at `width × height` points.
struct ImageSource<'p> {
    doc: PdfDocument<'p>,
    width: f32,
    height: f32,
}

fn image_page<'p>(
    pdfium: &'p Pdfium,
    path: &str,
    width_pt: f32,
    opacity: f32,
) -> Result<ImageSource<'p>, EngineError> {
    let source = std::path::Path::new(path);
    if !source.is_file() {
        return Err(EngineError::not_found(format!("{}", source.display())));
    }
    let image = image::open(source).map_err(|e| {
        EngineError::new(
            ErrorCode::InvalidArgument,
            format!("{}: {e}", source.display()),
        )
    })?;
    if image.width() == 0 || image.height() == 0 {
        return Err(EngineError::invalid("the image is empty"));
    }
    let height_pt = width_pt * image.height() as f32 / image.width() as f32;
    let mut rgba = image.to_rgba8();
    if opacity < 1.0 {
        for px in rgba.pixels_mut() {
            px.0[3] = (px.0[3] as f32 * opacity).round() as u8;
        }
    }
    let image = image::DynamicImage::ImageRgba8(rgba);

    let mut doc = pdfium.create_new_pdf().ctx("create scratch document")?;
    let mut page = doc
        .pages_mut()
        .create_page_at_end(PdfPagePaperSize::Custom(
            PdfPoints::new(width_pt),
            PdfPoints::new(height_pt),
        ))
        .ctx("create scratch page")?;
    let mut object = PdfPageImageObject::new(&doc, &image).ctx("create image object")?;
    object
        .scale(width_pt, height_pt)
        .ctx("scale image object")?;
    page.objects_mut()
        .add_image_object(object)
        .ctx("add image object")?;
    page.regenerate_content().ctx("regenerate scratch page")?;
    drop(page);
    Ok(ImageSource {
        doc,
        width: width_pt,
        height: height_pt,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn apply(m: Matrix, u: f32, v: f32) -> (f32, f32) {
        (m[0] * u + m[2] * v + m[4], m[1] * u + m[3] * v + m[5])
    }

    #[test]
    fn visual_corners_map_to_the_displayed_corners() {
        let crop = Rect::new(0.0, 0.0, 600.0, 800.0);
        // /Rotate 90: displayed bottom-left is user bottom-right, displayed top-left is
        // user bottom-left.
        let m = visual_to_user(90, crop);
        assert_eq!(apply(m, 0.0, 0.0), (600.0, 0.0));
        assert_eq!(apply(m, 0.0, 600.0), (0.0, 0.0));
        let m = visual_to_user(180, crop);
        assert_eq!(apply(m, 0.0, 0.0), (600.0, 800.0));
        let m = visual_to_user(270, crop);
        assert_eq!(apply(m, 0.0, 0.0), (0.0, 800.0));
        assert_eq!(apply(m, 800.0, 0.0), (0.0, 0.0));
        let m = visual_to_user(0, Rect::new(10.0, 20.0, 610.0, 820.0));
        assert_eq!(apply(m, 0.0, 0.0), (10.0, 20.0));
    }

    #[test]
    fn tokens_are_replaced_and_unknown_ones_stay() {
        assert_eq!(
            substitute(
                "{{page}} / {{total}} {{filename}} {{x}}",
                3,
                9,
                "2026-09-28",
                "a"
            ),
            "3 / 9 a {{x}}"
        );
        assert_eq!(
            substitute("{{date}}", 1, 1, "2026-09-28", "a"),
            "2026-09-28"
        );
    }

    #[test]
    fn bates_numbers_are_padded_and_count_stamped_pages() {
        let opts = BatesOptions {
            bates_start: 101,
            bates_digits: 6,
            bates_prefix: "ABC".into(),
            bates_suffix: "-K".into(),
        };
        assert_eq!(bates_label(&opts, 0), "ABC000101-K");
        assert_eq!(bates_label(&opts, 13), "ABC000114-K");
        let narrow = BatesOptions {
            bates_digits: 2,
            ..BatesOptions::default()
        };
        assert_eq!(bates_label(&narrow, 0), "01");
        assert_eq!(bates_label(&narrow, 999), "1000", "wider than the padding, never cut");
        let tokens = Tokens {
            page: 2,
            total: 14,
            date: "2026-09-28",
            filename: "{{page}}",
            bates: "ABC000102",
        };
        assert_eq!(
            expand_tokens("{{bates}} · {{page}}/{{total}} {{filename}} {{nope}} {{", &tokens),
            "ABC000102 · 2/14 {{page}} {{nope}} {{"
        );
        assert_eq!(expand_tokens("{{{{page}}", &tokens), "{{2");
    }

    #[test]
    fn rotated_box_is_anchored_by_its_bounding_box() {
        let space = VisualSpace::new(0, Rect::new(0.0, 0.0, 600.0, 800.0));
        let (w, h) = (300.0, 60.0);
        for anchor in [
            StampAnchor::Tl,
            StampAnchor::Tc,
            StampAnchor::Tr,
            StampAnchor::Ml,
            StampAnchor::Mr,
            StampAnchor::Bl,
            StampAnchor::Bc,
            StampAnchor::Br,
        ] {
            for deg in [-180.0, -135.0, -45.0, 30.0, 45.0, 90.0, 135.0] {
                let spec = PageStampSpec {
                    role: StampRole::Watermark,
                    source: PageStampSource::Text {
                        text: "x".into(),
                        font_size_pt: 10.0,
                        color: [0, 0, 0],
                    },
                    anchor,
                    margin_pt: 20.0,
                    rotate_deg: deg,
                    opacity: 1.0,
                    pages: PageSelection::All(crate::ipc::types::AllPages::All),
                    bates: Default::default(),
                };
                let m = placement(&spec, &space, w, h);
                for (u, v) in [(0.0, 0.0), (w, 0.0), (0.0, h), (w, h)] {
                    let (x, y) = apply(m, u, v);
                    assert!(
                        x >= 20.0 - 1e-2
                            && x <= 580.0 + 1e-2
                            && y >= 20.0 - 1e-2
                            && y <= 780.0 + 1e-2,
                        "{anchor:?} {deg}°: corner ({x}, {y}) outside the margin"
                    );
                }
            }
        }
        assert_eq!(rotated_extent(10.0, 4.0, 0.0), (10.0, 4.0));
        let (bw, bh) = rotated_extent(10.0, 4.0, 90.0);
        assert!((bw - 4.0).abs() < 1e-4 && (bh - 10.0).abs() < 1e-4);
    }

    #[test]
    fn rotation_about_the_centre_keeps_the_centre() {
        let space = VisualSpace::new(0, Rect::new(0.0, 0.0, 600.0, 800.0));
        let spec = PageStampSpec {
            role: StampRole::Watermark,
            source: PageStampSource::Text {
                text: "x".into(),
                font_size_pt: 10.0,
                color: [0, 0, 0],
            },
            anchor: StampAnchor::Mc,
            margin_pt: 0.0,
            rotate_deg: 45.0,
            opacity: 1.0,
            pages: PageSelection::All(crate::ipc::types::AllPages::All),
            bates: Default::default(),
        };
        let m = placement(&spec, &space, 200.0, 40.0);
        let (x, y) = apply(m, 100.0, 20.0);
        assert!((x - 300.0).abs() < 1e-3 && (y - 400.0).abs() < 1e-3);
    }
}
