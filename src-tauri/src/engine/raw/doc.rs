//! Document-level raw reads that pdfium-render either does not expose or only exposes
//! behind an `FPDF_LoadPage` we do not want to pay at open time.

use pdfium_render::prelude::{PdfDocument, PdfiumLibraryBindings};
use std::os::raw::{c_ulong, c_void};

/// `/PageLabels` entry for a page, **without** loading the page.
///
/// `PdfPage::label()` reads the same value, but only after `FPDF_LoadPage` (0.46–26 ms);
/// this is the document-level call, so a 500-page label pass stays in the microseconds.
pub fn page_label(
    bindings: &dyn PdfiumLibraryBindings,
    doc: &PdfDocument<'_>,
    index: u16,
) -> Option<String> {
    let handle = doc.raw_handle();
    // SAFETY: `handle` is live for the borrow of `doc`; a null buffer asks for the length.
    let len = unsafe {
        bindings.FPDF_GetPageLabel(handle, index as i32, std::ptr::null_mut(), 0) as usize
    };
    if len < 4 {
        // 0 = absent; 2 = just the UTF-16 terminator.
        return None;
    }
    let mut buffer = vec![0u8; len];
    // SAFETY: `buffer` is `len` bytes, exactly what the call above asked for.
    let written = unsafe {
        bindings.FPDF_GetPageLabel(
            handle,
            index as i32,
            buffer.as_mut_ptr() as *mut c_void,
            len as c_ulong,
        ) as usize
    };
    if written == 0 {
        return None;
    }
    decode_utf16le(&buffer[..written.min(len)])
}

/// PDFium string buffers are UTF-16LE with a trailing NUL.
pub fn decode_utf16le(bytes: &[u8]) -> Option<String> {
    let units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .take_while(|&u| u != 0)
        .collect();
    if units.is_empty() {
        return None;
    }
    String::from_utf16(&units).ok()
}

/// `FPDF_GetMetaText` — one `/Info` entry (`"Title"`, `"ModDate"`, …), `None` when absent or
/// empty.
///
/// Used instead of `PdfMetadata` because pdfium-render asks for the tag
/// `"ModificationDate"`, which does not exist (`/Info` says `ModDate`), so the modification
/// date always read back as missing.
pub fn meta_text(
    bindings: &dyn PdfiumLibraryBindings,
    doc: &PdfDocument<'_>,
    tag: &str,
) -> Option<String> {
    let handle = doc.raw_handle();
    // SAFETY: `handle` is live for the borrow of `doc`; a null buffer asks for the length.
    let len = unsafe { bindings.FPDF_GetMetaText(handle, tag, std::ptr::null_mut(), 0) as usize };
    if len < 4 {
        return None;
    }
    let mut buffer = vec![0u8; len];
    // SAFETY: `buffer` is `len` bytes, exactly what the call above asked for.
    let written = unsafe {
        bindings.FPDF_GetMetaText(
            handle,
            tag,
            buffer.as_mut_ptr() as *mut c_void,
            len as c_ulong,
        ) as usize
    };
    decode_utf16le(&buffer[..written.min(len)])
}
