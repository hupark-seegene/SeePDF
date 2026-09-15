//! Form-fill environment hooks the page LRU needs.
//!
//! pdfium-render initialises the form-fill environment at document load
//! (`PdfDocument::form()` is `Some` when there is one) but **never** calls
//! `FORM_OnAfterLoadPage` / `FORM_OnBeforeClosePage`, without which widget appearances are
//! not drawn by `FPDF_FFLDraw` and field edits are not committed (annotations spike §3.4).
//! The registry calls these on every page open and eviction.

use pdfium_render::prelude::{FPDF_FORMHANDLE, PdfPage, PdfiumLibraryBindings};

/// Must be called after `FPDF_LoadPage` on a document that has a form.
pub fn on_after_load_page(
    bindings: &dyn PdfiumLibraryBindings,
    page: &PdfPage<'_>,
    form: FPDF_FORMHANDLE,
) {
    // SAFETY: `page.raw_handle()` is a live FPDF_PAGE of the document `form` belongs to,
    // and this runs on the engine thread that owns both.
    unsafe { bindings.FORM_OnAfterLoadPage(page.raw_handle(), form) }
}

/// Must be called before `FPDF_ClosePage` on a document that has a form.
pub fn on_before_close_page(
    bindings: &dyn PdfiumLibraryBindings,
    page: &PdfPage<'_>,
    form: FPDF_FORMHANDLE,
) {
    // SAFETY: as above; called immediately before the page is dropped.
    unsafe { bindings.FORM_OnBeforeClosePage(page.raw_handle(), form) }
}

/// Commits the focused field's `/V` **and** regenerates its `/AP`. Called before every save
/// and before any structural change (`ARCHITECTURE.md` §8).
pub fn force_to_kill_focus(bindings: &dyn PdfiumLibraryBindings, form: FPDF_FORMHANDLE) -> bool {
    // SAFETY: `form` is the live form handle of a document owned by this thread.
    unsafe { bindings.FORM_ForceToKillFocus(form) != 0 }
}

/// 0 disables PDFium's own blue field wash; the viewer draws highlighting itself via the
/// `hl=1` tile parameter.
pub fn set_field_highlight_alpha(
    bindings: &dyn PdfiumLibraryBindings,
    form: FPDF_FORMHANDLE,
    alpha: u8,
) {
    // SAFETY: `form` is the live form handle of a document owned by this thread.
    unsafe { bindings.FPDF_SetFormFieldHighlightAlpha(form, alpha) }
}
