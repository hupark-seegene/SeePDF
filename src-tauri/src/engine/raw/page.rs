//! Page-level raw calls: reorder, import and flatten.
//!
//! These three have no safe wrapper in pdfium-render 0.9.4 and are what Stage 1 (b)'s page
//! operations, duplicate and export paths are built on (`WORKPLAN.md` §3, `ARCHITECTURE.md`
//! §6.3). Every one of them acts on the **same** `FPDF_DOCUMENT` / `FPDF_PAGE` the
//! high-level API is using, through `raw_handle()`.
//!
//! **Caller contract (all three):** a structural change bypasses pdfium-render's
//! `PdfPageIndexCache`, so the caller must run inside `registry::mutate(..).structural()`,
//! which flushes the page LRU before the closure runs. [`flatten`] additionally invalidates
//! the one page it touched — reload it (`OpenDoc::invalidate_page` then `OpenDoc::page`)
//! before rendering or reading from it.

use crate::engine::raw::consts;
use crate::ipc::{EngineError, ErrorCode};
use pdfium_render::prelude::{
    PdfDocument, PdfPage, PdfiumLibraryBindings, FPDF_DOCUMENT, FPDF_PAGE, FS_MATRIX,
};
use std::os::raw::{c_int, c_ulong};

/// What `FPDFPage_Flatten` reported.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlattenOutcome {
    /// Annotations were merged into the page content stream.
    Flattened,
    /// The page had nothing to flatten (`FLATTEN_NOTHINGTODO`).
    NothingToDo,
}

/// Which annotations `FPDFPage_Flatten` keeps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlattenMode {
    /// `FLAT_NORMALDISPLAY` — keeps everything that is visible on screen. **Use this.**
    NormalDisplay,
    /// `FLAT_PRINT` — **deletes** annotations without the `/F 4` Print flag instead of
    /// flattening them (annotations spike §3.3). Only for an explicit "flatten for print".
    Print,
}

impl FlattenMode {
    fn as_flag(self) -> c_int {
        match self {
            FlattenMode::NormalDisplay => consts::FLAT_NORMALDISPLAY,
            FlattenMode::Print => consts::FLAT_PRINT,
        }
    }
}

/// `FPDF_MovePages` — moves `pages` (in the given order) so that they land, contiguously and
/// in that order, starting at `dest`.
///
/// PDFium's semantics are "ordered list": the pages named in `pages` are lifted out, the rest
/// close up, and the lifted block is re-inserted before the page that was at `dest` in the
/// **original** numbering. 0.02 ms for any move (pages spike §1).
///
/// Validation happens here rather than in PDFium (which only returns `false`): indices must be
/// unique, all `< page_count`, and `dest <= page_count - pages.len()`. A `false` return after
/// that means the document is in an unknown state and the caller must reload from the undo
/// snapshot — `registry::mutate` does exactly that when this returns `Err`.
pub fn move_pages(
    bindings: &dyn PdfiumLibraryBindings,
    doc: &PdfDocument<'_>,
    pages: &[u16],
    dest: u16,
) -> Result<(), EngineError> {
    if pages.is_empty() {
        return Ok(());
    }
    let count = page_count(bindings, doc);
    let mut seen: Vec<u16> = pages.to_vec();
    seen.sort_unstable();
    seen.dedup();
    if seen.len() != pages.len() {
        return Err(EngineError::invalid("move_pages: duplicate page index"));
    }
    if let Some(&max) = seen.last() {
        if max >= count {
            return Err(EngineError::invalid(format!(
                "move_pages: page {max} is out of range (page count {count})"
            )));
        }
    }
    if (dest as usize) + pages.len() > count as usize {
        return Err(EngineError::invalid(format!(
            "move_pages: destination {dest} + {} exceeds the page count {count}",
            pages.len()
        )));
    }
    let indices: Vec<c_int> = pages.iter().map(|&p| p as c_int).collect();
    // SAFETY: `doc` is live for this borrow, `indices` is a `len`-element array of validated
    // in-range page indices, and this runs on the engine thread that owns the document.
    let ok = unsafe {
        bindings.FPDF_MovePages(
            doc.raw_handle(),
            indices.as_ptr(),
            indices.len() as c_ulong,
            dest as c_int,
        )
    };
    if bindings.is_true(ok) {
        Ok(())
    } else {
        Err(EngineError::new(
            ErrorCode::Pdfium,
            "FPDF_MovePages failed; reload from the snapshot",
        ))
    }
}

/// `FPDF_ImportPagesByIndex` — copies `indices` of `src` into `dest` at position `at`.
///
/// `src` and `dest` may be the **same** document, which is how a page is duplicated
/// (`import_pages_by_index(b, doc, doc, &[i], i + 1)`, verified in pages spike §2).
///
/// Import keeps widget annotations but **drops `/AcroForm`, the outline and the document
/// metadata**, so extract/split must copy the original and delete the other pages instead of
/// importing into a fresh document (`ARCHITECTURE.md` §6.3).
pub fn import_pages_by_index(
    bindings: &dyn PdfiumLibraryBindings,
    dest: &PdfDocument<'_>,
    src: &PdfDocument<'_>,
    indices: &[u16],
    at: u16,
) -> Result<(), EngineError> {
    if indices.is_empty() {
        return Ok(());
    }
    let src_count = page_count(bindings, src);
    if let Some(&max) = indices.iter().max() {
        if max >= src_count {
            return Err(EngineError::invalid(format!(
                "import_pages_by_index: source page {max} is out of range (page count {src_count})"
            )));
        }
    }
    let dest_count = page_count(bindings, dest);
    if at > dest_count {
        return Err(EngineError::invalid(format!(
            "import_pages_by_index: destination {at} is past the end (page count {dest_count})"
        )));
    }
    let list: Vec<c_int> = indices.iter().map(|&p| p as c_int).collect();
    // SAFETY: both documents are live for their borrows, `list` is a `len`-element array of
    // validated in-range source indices, and this runs on the engine thread owning both.
    let ok = unsafe {
        bindings.FPDF_ImportPagesByIndex(
            dest.raw_handle(),
            src.raw_handle(),
            list.as_ptr(),
            list.len() as c_ulong,
            at as c_int,
        )
    };
    if bindings.is_true(ok) {
        Ok(())
    } else {
        Err(EngineError::new(
            ErrorCode::Pdfium,
            "FPDF_ImportPagesByIndex failed",
        ))
    }
}

/// `FPDFPage_Flatten` + `FPDFPage_GenerateContent` — merges the page's annotations and form
/// fields into its content stream.
///
/// Always call this with [`FlattenMode::NormalDisplay`]: `PdfPage::flatten()` and the crate's
/// `flatten` feature are `FLAT_PRINT`, which *deletes* annotations that lack the `/F 4` Print
/// flag (annotations spike §3.3) — that is why the feature is off in `Cargo.toml`.
///
/// **The `FPDF_PAGE` is invalid afterwards**: PDFium rebuilds the page object. Drop the
/// `PdfPage` and reload it (`OpenDoc::invalidate_page` + `OpenDoc::page`) before rendering,
/// reading text or touching objects.
pub fn flatten(
    bindings: &dyn PdfiumLibraryBindings,
    page: &PdfPage<'_>,
    mode: FlattenMode,
) -> Result<FlattenOutcome, EngineError> {
    let handle = page.raw_handle();
    // SAFETY: `handle` is live for the borrow of `page`, and the page belongs to a document
    // owned by this thread.
    let rc = unsafe { bindings.FPDFPage_Flatten(handle, mode.as_flag()) };
    match rc {
        consts::FLATTEN_SUCCESS => {
            // SAFETY: as above; the page object is still the flattened one until it is closed.
            let ok = unsafe { bindings.FPDFPage_GenerateContent(handle) };
            if !bindings.is_true(ok) {
                return Err(EngineError::new(
                    ErrorCode::Pdfium,
                    "FPDFPage_GenerateContent after flatten failed",
                ));
            }
            Ok(FlattenOutcome::Flattened)
        }
        consts::FLATTEN_NOTHINGTODO => Ok(FlattenOutcome::NothingToDo),
        _ => Err(EngineError::new(
            ErrorCode::Pdfium,
            "FPDFPage_Flatten failed",
        )),
    }
}

/// `FPDFPage_TransformAnnots` — maps the `/Rect` of every annotation on `page` through `m`
/// (`[a, b, c, d, e, f]`). P2 page resize.
///
/// Only `/Rect` changes, written straight into the dictionary: unlike `FPDFAnnot_SetRect` it
/// never rewrites an appearance `BBox`, so an existing appearance is fitted into the mapped rect
/// and scales with it. `/QuadPoints` and `/InkList` are the caller's job (they are not touched),
/// and `/Popup` annotations are skipped by PDFium's annotation list. Building that list also
/// generates an appearance for any annotation that has none.
pub fn transform_annots(bindings: &dyn PdfiumLibraryBindings, page: &PdfPage<'_>, m: [f32; 6]) {
    // SAFETY: `page` is live for this borrow and owned by this thread; the call only reads and
    // writes that page's annotation dictionaries.
    unsafe {
        bindings.FPDFPage_TransformAnnots(
            page.raw_handle(),
            m[0] as f64,
            m[1] as f64,
            m[2] as f64,
            m[3] as f64,
            m[4] as f64,
            m[5] as f64,
        )
    }
}

/// `FPDF_GetPageCount` without going through `PdfPages` (which builds an index cache).
pub fn page_count(bindings: &dyn PdfiumLibraryBindings, doc: &PdfDocument<'_>) -> u16 {
    // SAFETY: `doc` is live for this borrow.
    let n = unsafe { bindings.FPDF_GetPageCount(doc.raw_handle()) };
    n.clamp(0, u16::MAX as c_int) as u16
}

/// A document made with `FPDF_CreateNewDocument`, closed on drop.
struct ScratchDoc<'b> {
    bindings: &'b dyn PdfiumLibraryBindings,
    handle: FPDF_DOCUMENT,
}

impl Drop for ScratchDoc<'_> {
    fn drop(&mut self) {
        // SAFETY: the handle came from `FPDF_CreateNewDocument` and is closed exactly once.
        unsafe { self.bindings.FPDF_CloseDocument(self.handle) };
    }
}

/// A page of a [`ScratchDoc`], closed on drop.
struct ScratchPageHandle<'b> {
    bindings: &'b dyn PdfiumLibraryBindings,
    handle: FPDF_PAGE,
}

impl Drop for ScratchPageHandle<'_> {
    fn drop(&mut self) {
        // SAFETY: the handle came from `FPDF_LoadPage` and is closed exactly once, before its
        // document.
        unsafe { self.bindings.FPDF_ClosePage(self.handle) };
    }
}

fn load_scratch_page<'b>(
    bindings: &'b dyn PdfiumLibraryBindings,
    doc: &ScratchDoc<'_>,
) -> Result<ScratchPageHandle<'b>, EngineError> {
    // SAFETY: the scratch document is live and holds one page.
    let handle = unsafe { bindings.FPDF_LoadPage(doc.handle, 0) };
    if handle.is_null() {
        return Err(EngineError::new(ErrorCode::Pdfium, "load the scratch page"));
    }
    Ok(ScratchPageHandle { bindings, handle })
}

/// What one object looks like to [`rehearse_rewrite`]: where it is drawn, how large, and
/// through which clip.
#[derive(Debug, Clone, Copy)]
struct Placement {
    kind: c_int,
    matrix: [f32; 6],
    /// Text objects: the font size (`Tf`); 0 otherwise.
    font_size: f32,
    /// The clip: how many paths, how many segments in all, and the bounds of their points.
    clip: (c_int, c_int, [f32; 4]),
}

impl Placement {
    /// The same object, drawn in the same place at the same size. The matrix is written back
    /// with a few decimals, so tiny differences are float formatting, not a move.
    fn same_place(&self, other: &Placement) -> bool {
        let close = |a: f32, b: f32, tol: f32| (a - b).abs() <= tol * a.abs().max(b.abs()).max(1.0);
        self.kind == other.kind
            && (0..4).all(|k| close(self.matrix[k], other.matrix[k], 1e-3))
            && (4..6).all(|k| (self.matrix[k] - other.matrix[k]).abs() <= 0.05)
            && close(self.font_size, other.font_size, 1e-3)
    }

    /// …and clipped the same way.
    fn same_clip(&self, other: &Placement) -> bool {
        let (a, b) = (&self.clip, &other.clip);
        a.0 == b.0 && a.1 == b.1 && (0..4).all(|k| (a.2[k] - b.2[k]).abs() <= 0.05)
    }
}

/// The placement of every top-level object of `page`.
///
/// SAFETY (caller): `page` is a live page handle on this thread.
unsafe fn placements(bindings: &dyn PdfiumLibraryBindings, page: FPDF_PAGE) -> Vec<Placement> {
    let n = bindings.FPDFPage_CountObjects(page).max(0) as usize;
    (0..n)
        .map(|i| {
            let object = bindings.FPDFPage_GetObject(page, i as c_int);
            if object.is_null() {
                return Placement {
                    kind: -1,
                    matrix: [0.0; 6],
                    font_size: 0.0,
                    clip: (0, 0, [0.0; 4]),
                };
            }
            let kind = bindings.FPDFPageObj_GetType(object);
            let mut m = FS_MATRIX {
                a: 0.0,
                b: 0.0,
                c: 0.0,
                d: 0.0,
                e: 0.0,
                f: 0.0,
            };
            if !bindings.is_true(bindings.FPDFPageObj_GetMatrix(object, &mut m)) {
                m = FS_MATRIX {
                    a: 1.0,
                    b: 0.0,
                    c: 0.0,
                    d: 1.0,
                    e: 0.0,
                    f: 0.0,
                };
            }
            let mut font_size: f32 = 0.0;
            if kind == FPDF_PAGEOBJ_TEXT
                && !bindings.is_true(bindings.FPDFTextObj_GetFontSize(object, &mut font_size))
            {
                font_size = 0.0;
            }
            let mut clip = (0, 0, [f32::MAX, f32::MAX, f32::MIN, f32::MIN]);
            let handle = bindings.FPDFPageObj_GetClipPath(object);
            if !handle.is_null() {
                clip.0 = bindings.FPDFClipPath_CountPaths(handle).max(0);
                for path in 0..clip.0 {
                    let segments = bindings.FPDFClipPath_CountPathSegments(handle, path).max(0);
                    clip.1 += segments;
                    for k in 0..segments {
                        let segment = bindings.FPDFClipPath_GetPathSegment(handle, path, k);
                        let (mut x, mut y) = (0.0f32, 0.0f32);
                        if !segment.is_null()
                            && bindings
                                .is_true(bindings.FPDFPathSegment_GetPoint(segment, &mut x, &mut y))
                        {
                            let b = &mut clip.2;
                            (b[0], b[1], b[2], b[3]) =
                                (b[0].min(x), b[1].min(y), b[2].max(x), b[3].max(y));
                        }
                    }
                }
            }
            if clip.1 == 0 {
                clip.2 = [0.0; 4];
            }
            Placement {
                kind,
                matrix: [m.a, m.b, m.c, m.d, m.e, m.f],
                font_size,
                clip,
            }
        })
        .collect()
}

/// `FPDF_PAGEOBJ_*` (public/fpdf_edit.h).
const FPDF_PAGEOBJ_TEXT: c_int = 1;
const FPDF_PAGEOBJ_IMAGE: c_int = 3;
const FPDF_PAGEOBJ_SHADING: c_int = 4;

/// What rewriting some of a page's content streams would do to its objects.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Rehearsal {
    /// Objects the regenerated content no longer has (indices of the page as it is).
    pub dropped: Vec<usize>,
    /// Of those, the ones PDFium can never write back: inline images and shading objects.
    pub lost: Vec<usize>,
    /// Objects that come back somewhere else, at another size or through another clip.
    pub disturbed: Vec<usize>,
    /// How many top-level objects the page has.
    pub objects: usize,
}

impl Rehearsal {
    fn clean(&self) -> bool {
        self.dropped.is_empty() && self.disturbed.is_empty()
    }
}

/// Stage 9 — what PDFium's content generator would do to page `page_index` if the content
/// streams holding the objects `dirty` were regenerated, which is what `edit_paragraph` does
/// (it removes the paragraph's objects and moves the ones below; its new text goes into a
/// stream of its own).
///
/// The generator writes each object of a regenerated stream whole — `q`, colours, clip,
/// matrix, the object, `Q` — so a regenerated stream is self-contained. Two things still go
/// wrong, and nothing in the public API says when:
///
/// * it never writes an inline image (`BI … ID … EI`) or a shading object (`sh`) back, so
///   regenerating a stream that holds one loses it (`lost`);
/// * a page's content streams are one stream cut into pieces, and a producer may cut anywhere:
///   160F-2019.pdf ends a piece with `… 5.4 0 0 5.4 507.6 447.96 Tm` and starts the next with
///   `[(6)-0.6 (\))]TJ`, and opens a clip (`q … re W n`) in one piece that the next one
///   closes. A regenerated piece no longer leaves that state behind — the next piece's text
///   is drawn with another matrix, and the rest of the page under a clip that is never closed
///   (it renders blank). The objects come back moved or clipped (`disturbed`) or not at all
///   (`dropped`).
///
/// So the rewrite is rehearsed on a copy of the page in a throw-away document
/// (`FPDF_ImportPagesByIndex`): each dirty object is taken off and put back at its index
/// (which marks its stream for regeneration), the content is generated, the copy is re-parsed
/// and every object is compared with what it was. The open document is never touched.
pub fn rehearse_rewrite(
    bindings: &dyn PdfiumLibraryBindings,
    doc: &PdfDocument<'_>,
    page_index: u16,
    dirty: &[usize],
) -> Result<Rehearsal, EngineError> {
    // SAFETY: no arguments; the handle is closed by the guard.
    let handle = unsafe { bindings.FPDF_CreateNewDocument() };
    if handle.is_null() {
        return Err(EngineError::new(
            ErrorCode::Pdfium,
            "create a scratch document",
        ));
    }
    let scratch = ScratchDoc { bindings, handle };
    let list = [page_index as c_int];
    // SAFETY: both documents are live; `list` is a one-element array; the caller checked the
    // index against the open document.
    let ok = unsafe {
        bindings.FPDF_ImportPagesByIndex(scratch.handle, doc.raw_handle(), list.as_ptr(), 1, 0)
    };
    if !bindings.is_true(ok) {
        return Err(EngineError::new(
            ErrorCode::Pdfium,
            "copy the page to rehearse its rewrite",
        ));
    }
    let before = {
        let page = load_scratch_page(bindings, &scratch)?;
        // SAFETY (whole block): `page.handle` is live; every object handle is fetched from it
        // by index right before use, and a removed object is put back at once (ownership
        // returns to the page; PDFium frees it if that fails).
        unsafe {
            let before = placements(bindings, page.handle);
            for &i in dirty.iter().filter(|&&i| i < before.len()) {
                let object = bindings.FPDFPage_GetObject(page.handle, i as c_int);
                if !object.is_null()
                    && bindings.is_true(bindings.FPDFPage_RemoveObject(page.handle, object))
                {
                    bindings.FPDFPage_InsertObjectAtIndex(page.handle, object, i);
                }
            }
            if !bindings.is_true(bindings.FPDFPage_GenerateContent(page.handle)) {
                return Err(EngineError::new(
                    ErrorCode::Pdfium,
                    "rehearse the page rewrite",
                ));
            }
            before
        }
    };
    let page = load_scratch_page(bindings, &scratch)?;
    // SAFETY: `page.handle` is live.
    let after = unsafe { placements(bindings, page.handle) };
    // The regenerated page keeps the objects in order: walk both lists, pairing objects by
    // their place, and tell a missing object (the next one pairs) from a disturbed one.
    let mut out = Rehearsal {
        objects: before.len(),
        ..Rehearsal::default()
    };
    let (mut i, mut j) = (0, 0);
    while i < before.len() {
        if j < after.len() && before[i].same_place(&after[j]) {
            if !before[i].same_clip(&after[j]) {
                out.disturbed.push(i);
            }
            i += 1;
            j += 1;
        } else if j >= after.len() || (i + 1 < before.len() && before[i + 1].same_place(&after[j]))
        {
            out.dropped.push(i);
            if matches!(before[i].kind, FPDF_PAGEOBJ_IMAGE | FPDF_PAGEOBJ_SHADING) {
                out.lost.push(i);
            }
            i += 1;
        } else {
            out.disturbed.push(i);
            i += 1;
            j += 1;
        }
    }
    Ok(out)
}

/// How many times [`rewrite_set`] widens the rewrite to the objects it disturbed before it
/// regenerates the whole page instead.
const REWRITE_ROUNDS: usize = 2;

/// The objects whose content streams a rewrite of `dirty` must regenerate as well, so that
/// nothing it keeps is lost, moved or clipped away (see [`rehearse_rewrite`]): `Some(extra)`
/// (disjoint from `dirty`, ascending; usually empty), or `None` when no rewrite keeps the page
/// intact — an inline image or a shading would be lost.
///
/// A disturbed or dropped object is rewritten too (its own stream then writes it whole); when
/// that does not settle it — the pieces of the page share state all along, like 160F's — every
/// stream is regenerated: then no piece depends on another.
pub fn rewrite_set(
    bindings: &dyn PdfiumLibraryBindings,
    doc: &PdfDocument<'_>,
    page_index: u16,
    dirty: &[usize],
) -> Result<Option<Vec<usize>>, EngineError> {
    let own: std::collections::HashSet<usize> = dirty.iter().copied().collect();
    let extra = |set: &std::collections::BTreeSet<usize>| -> Vec<usize> {
        set.iter().copied().filter(|i| !own.contains(i)).collect()
    };
    let mut set: std::collections::BTreeSet<usize> = dirty.iter().copied().collect();
    let mut objects = 0;
    for _ in 0..=REWRITE_ROUNDS {
        let all: Vec<usize> = set.iter().copied().collect();
        let r = rehearse_rewrite(bindings, doc, page_index, &all)?;
        if !r.lost.is_empty() {
            return Ok(None);
        }
        if r.clean() {
            return Ok(Some(extra(&set)));
        }
        objects = r.objects;
        set.extend(r.dropped);
        set.extend(r.disturbed);
    }
    let whole: std::collections::BTreeSet<usize> = (0..objects).collect();
    let all: Vec<usize> = whole.iter().copied().collect();
    let r = rehearse_rewrite(bindings, doc, page_index, &all)?;
    Ok(r.clean().then(|| extra(&whole)))
}
