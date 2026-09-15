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

// ---------------------------------------------------------------------------------------
// Stage 1 (a): the FORM_* event drivers that actually change a field's value.
//
// `PdfFormTextField::set_value` writes `/V` and nothing else, so the field stays visually
// empty in PDFium, Preview and every viewer that trusts `/AP` (annotations spike §3.4).
// Only the form-fill environment regenerates the appearance stream, and only
// `FORM_ForceToKillFocus` commits it.
// ---------------------------------------------------------------------------------------

use pdfium_render::prelude::{FPDF_ANNOTATION, FPDF_WIDESTRING};
use std::os::raw::c_int;

/// `FORM_SetFocusedAnnot` — focuses a widget without synthesising a mouse click.
///
/// The `ARCHITECTURE.md` §6.4 preferred path (probe result in `STAGE1A_NOTES.md`): focus →
/// [`select_all_text`] → [`replace_selection`] → [`force_to_kill_focus`].
pub fn set_focused_annot(
    bindings: &dyn PdfiumLibraryBindings,
    form: FPDF_FORMHANDLE,
    annot: FPDF_ANNOTATION,
) -> bool {
    // SAFETY: `form` and `annot` belong to a document owned by this thread and are live.
    unsafe { bindings.FORM_SetFocusedAnnot(form, annot) != 0 }
}

/// `FORM_SelectAllText` — selects the whole value of the focused text field, so the next
/// [`replace_selection`] overwrites rather than appends.
pub fn select_all_text(
    bindings: &dyn PdfiumLibraryBindings,
    form: FPDF_FORMHANDLE,
    page: &PdfPage<'_>,
) -> bool {
    // SAFETY: the page is live and belongs to `form`'s document.
    unsafe { bindings.FORM_SelectAllText(form, page.raw_handle()) != 0 }
}

/// `FORM_ReplaceSelection` — types a whole UTF-16 string into the focused field in one call.
/// Returns nothing in PDFium's API; verify with `FPDFAnnot_GetFormFieldValue`.
pub fn replace_selection(
    bindings: &dyn PdfiumLibraryBindings,
    form: FPDF_FORMHANDLE,
    page: &PdfPage<'_>,
    text: &str,
) {
    let mut utf16: Vec<u16> = text.encode_utf16().collect();
    utf16.push(0);
    // SAFETY: `utf16` is NUL-terminated and outlives the call; the page is live.
    unsafe {
        bindings.FORM_ReplaceSelection(
            form,
            page.raw_handle(),
            utf16.as_ptr() as FPDF_WIDESTRING,
        )
    }
}

/// `FORM_OnLButtonDown` + `FORM_OnLButtonUp` at one point in **page user space** (y-up).
///
/// The verified fallback focus path, and the only way to toggle a checkbox or radio button.
/// Returns `(down, up)`; PDFium answers `false` for `down` on some widgets whose hit box is
/// at the page edge while the click still takes effect, so never treat `false` as fatal.
pub fn click(
    bindings: &dyn PdfiumLibraryBindings,
    form: FPDF_FORMHANDLE,
    page: &PdfPage<'_>,
    x: f32,
    y: f32,
) -> (bool, bool) {
    let handle = page.raw_handle();
    // SAFETY: the page is live and belongs to `form`'s document; the coordinates are plain
    // doubles PDFium hit-tests itself.
    unsafe {
        let down = bindings.FORM_OnLButtonDown(form, handle, 0, x as f64, y as f64) != 0;
        let up = bindings.FORM_OnLButtonUp(form, handle, 0, x as f64, y as f64) != 0;
        (down, up)
    }
}

/// `FORM_OnChar` for every UTF-16 code unit of `text` — the verified typing path.
pub fn type_text(
    bindings: &dyn PdfiumLibraryBindings,
    form: FPDF_FORMHANDLE,
    page: &PdfPage<'_>,
    text: &str,
) {
    let handle = page.raw_handle();
    for unit in text.encode_utf16() {
        // SAFETY: the page is live and belongs to `form`'s document.
        unsafe { bindings.FORM_OnChar(form, handle, unit as c_int, 0) };
    }
}

/// `FORM_OnChar(8)` — one backspace, `count` times. Clears a field before typing when
/// `FORM_SelectAllText` is not available (checked by the caller).
pub fn backspace(
    bindings: &dyn PdfiumLibraryBindings,
    form: FPDF_FORMHANDLE,
    page: &PdfPage<'_>,
    count: usize,
) {
    let handle = page.raw_handle();
    for _ in 0..count {
        // SAFETY: the page is live and belongs to `form`'s document.
        unsafe { bindings.FORM_OnChar(form, handle, 8, 0) };
    }
}

/// `FORM_SetIndexSelected` — selects or deselects one option of the focused combo/list box.
pub fn set_index_selected(
    bindings: &dyn PdfiumLibraryBindings,
    form: FPDF_FORMHANDLE,
    page: &PdfPage<'_>,
    index: usize,
    selected: bool,
) -> bool {
    // SAFETY: the page is live and belongs to `form`'s document; PDFium range-checks `index`.
    unsafe {
        bindings.FORM_SetIndexSelected(
            form,
            page.raw_handle(),
            index as c_int,
            bindings.bool_to_pdfium(selected),
        ) != 0
    }
}

/// `FORM_IsIndexSelected`.
pub fn is_index_selected(
    bindings: &dyn PdfiumLibraryBindings,
    form: FPDF_FORMHANDLE,
    page: &PdfPage<'_>,
    index: usize,
) -> bool {
    // SAFETY: the page is live and belongs to `form`'s document.
    unsafe { bindings.FORM_IsIndexSelected(form, page.raw_handle(), index as c_int) != 0 }
}
