//! The only module in the tree that contains `unsafe`.
//!
//! Every function here is a safe wrapper around a PDFium FFI call that pdfium-render 0.9.4
//! does not expose, driven through the **same** bindings instance and the **same** handles
//! the high-level API uses (that is the whole point of the `raw_handle()` /
//! `raw_bindings()` patch, `ARCHITECTURE.md` §1.3).
//!
//! Stage 0 ships only the form-page hooks the registry needs (`form.rs`) and the constant
//! table. Stage 1 (a) owns this module and adds `annot.rs`, `page.rs` (`FPDF_MovePages`,
//! `FPDF_ImportPagesByIndex`, `FPDFPage_Flatten`) and `save.rs` (`FPDF_SaveAsCopy` with
//! flags) beside them; Stage 1 (b) calls those wrappers.
//!
//! Rules for anything added here:
//! * take the handles from `raw_handle()` on a live Rust value, never store them;
//! * stay on the engine thread — PDFium has no thread safety of its own;
//! * keep `unsafe` to the single FFI line and return `Result<_, EngineError>`.

pub mod annot;
pub mod consts;
pub mod doc;
pub mod form;
pub mod page;
pub mod save;

use pdfium_render::prelude::{Pdfium, PdfiumLibraryBindings};

/// The bindings instance every high-level pdfium-render object calls through.
#[inline]
pub fn bindings(pdfium: &Pdfium) -> &'static dyn PdfiumLibraryBindings {
    pdfium.raw_bindings()
}
