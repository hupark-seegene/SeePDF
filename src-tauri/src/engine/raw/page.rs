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
use pdfium_render::prelude::{PdfDocument, PdfPage, PdfiumLibraryBindings};
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

/// `FPDF_GetPageCount` without going through `PdfPages` (which builds an index cache).
pub fn page_count(bindings: &dyn PdfiumLibraryBindings, doc: &PdfDocument<'_>) -> u16 {
    // SAFETY: `doc` is live for this borrow.
    let n = unsafe { bindings.FPDF_GetPageCount(doc.raw_handle()) };
    n.clamp(0, u16::MAX as c_int) as u16
}
