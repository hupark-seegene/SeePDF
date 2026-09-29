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

// The wrappers take PDFium handles (`FPDF_FORMHANDLE`, `FPDF_ANNOTATION`, …) as plain values
// and are safe by the rules above: every handle comes from `raw_handle()` on a live Rust value
// on the engine thread. Marking each one `unsafe fn` would only move the same promise to
// every call site.
#[allow(clippy::not_unsafe_ptr_arg_deref)]
pub mod annot;
pub mod consts;
pub mod doc;
#[allow(clippy::not_unsafe_ptr_arg_deref)]
pub mod form;
pub mod object;
pub mod outline;
pub mod page;
pub mod render;
pub mod save;
// v0.3 pkg3 (S1): `FPDFSignatureObj_GetSubFilter`, which pdfium-render does not expose.
pub mod sig;

use pdfium_render::prelude::{Pdfium, PdfiumLibraryBindings};

/// The bindings instance every high-level pdfium-render object calls through.
#[inline]
pub fn bindings(pdfium: &Pdfium) -> &'static dyn PdfiumLibraryBindings {
    pdfium.raw_bindings()
}
