//! SeePDF additive raw-handle accessors.
//!
//! This file is the whole of the SeePDF patch to pdfium-render 0.9.4 (see
//! `docs/pdfium-patch.diff` and `docs/ARCHITECTURE.md` §1.3). It adds no behaviour:
//! every function here simply re-exports an existing `pub(crate)` handle, or the
//! already-initialised global `PdfiumLibraryBindings` instance, as `pub`.
//!
//! Rationale: `FPDF_MovePages`, `FPDF_ImportPagesByIndex`, `FPDFPage_Flatten`,
//! `FPDF_SaveAsCopy` with flags, the whole `FPDFAnnot_*` edit pipeline and the
//! `FORM_*` fill environment have no safe wrapper in 0.9.4, and the handles they need
//! are `pub(crate)`. Without these five accessors the only alternative is a *second*
//! `bind_to_library()` driving a separate `FPDF_DOCUMENT` of the same bytes, which
//! costs a `save_to_bytes()` + reload (5 ms/MB) per edit.
//!
//! Everything returned here is a raw PDFium handle: it is only valid while the Rust
//! value it came from is alive, and PDFium is not thread-safe, so all use must stay on
//! the one thread that owns the `Pdfium` instance.

use crate::bindgen::{FPDF_ANNOTATION, FPDF_DOCUMENT, FPDF_FORMHANDLE, FPDF_PAGE};
use crate::bindings::PdfiumLibraryBindings;
use crate::pdf::document::form::PdfForm;
use crate::pdf::document::page::annotation::private::internal::PdfPageAnnotationPrivate;
use crate::pdf::document::page::annotation::PdfPageAnnotation;
use crate::pdf::document::page::PdfPage;
use crate::pdf::document::PdfDocument;
use crate::pdfium::Pdfium;

impl PdfDocument<'_> {
    /// Returns the raw `FPDF_DOCUMENT` handle backing this [PdfDocument].
    ///
    /// The handle is valid only for as long as this [PdfDocument] is alive.
    #[inline]
    pub fn raw_handle(&self) -> FPDF_DOCUMENT {
        self.handle()
    }
}

impl PdfPage<'_> {
    /// Returns the raw `FPDF_PAGE` handle backing this [PdfPage].
    ///
    /// The handle is valid only for as long as this [PdfPage] is alive.
    #[inline]
    pub fn raw_handle(&self) -> FPDF_PAGE {
        self.page_handle()
    }

    /// Returns the raw `FPDF_DOCUMENT` handle of the document containing this [PdfPage].
    #[inline]
    pub fn raw_document_handle(&self) -> FPDF_DOCUMENT {
        self.document_handle()
    }
}

impl PdfForm<'_> {
    /// Returns the raw `FPDF_FORMHANDLE` of this [PdfForm]'s form fill environment.
    ///
    /// The handle is valid only for as long as the containing document is alive.
    #[inline]
    pub fn raw_handle(&self) -> FPDF_FORMHANDLE {
        self.handle()
    }
}

impl PdfPageAnnotation<'_> {
    /// Returns the raw `FPDF_ANNOTATION` handle backing this [PdfPageAnnotation].
    ///
    /// The handle is valid only for as long as this [PdfPageAnnotation] is alive.
    #[inline]
    pub fn raw_handle(&self) -> FPDF_ANNOTATION {
        PdfPageAnnotationPrivate::handle(self)
    }
}

impl Pdfium {
    /// Returns the `PdfiumLibraryBindings` instance this crate is using.
    ///
    /// This is the *same* instance every high-level object calls through — not a second
    /// `bind_to_library()` — so raw calls made through it operate on the same PDFium
    /// global state, and on handles obtained from `raw_handle()` above.
    #[inline]
    pub fn raw_bindings(&self) -> &'static dyn PdfiumLibraryBindings {
        crate::pdfium::seepdf_global_bindings()
    }
}
