//! `FPDFLink_LoadWebLinks` (v0.3, V5): the URLs PDFium detects in a page's **text** — an
//! address typed into the document, not a Link annotation. pdfium-render keeps its
//! `FPDF_TEXTPAGE` handle `pub(crate)`, so this loads a text page of its own from the page's
//! raw handle; PDFium builds it with the same algorithm, so its char indices are the text
//! layer's (`engine::text::layer`).

use pdfium_render::prelude::{PdfPage, PdfiumLibraryBindings, FPDF_PAGELINK};
use std::os::raw::{c_double, c_int, c_ushort};

/// One detected address. `rects` are `(left, top, right, bottom)` in PDF user space.
#[derive(Debug, Clone, PartialEq)]
pub struct RawWebLink {
    pub url: String,
    pub rects: Vec<(f64, f64, f64, f64)>,
    pub char_start: i32,
    pub char_count: i32,
}

/// Every web link of `page`, in text order. Empty when the page has no text.
pub fn web_links(bindings: &dyn PdfiumLibraryBindings, page: &PdfPage<'_>) -> Vec<RawWebLink> {
    // SAFETY: `page` is live for the borrow and we are on the engine thread; the text page
    // is closed below before returning.
    let text_page = unsafe { bindings.FPDFText_LoadPage(page.raw_handle()) };
    if text_page.is_null() {
        return Vec::new();
    }
    // SAFETY: `text_page` was just loaded and is closed below.
    let links = unsafe { bindings.FPDFLink_LoadWebLinks(text_page) };
    let out = if links.is_null() {
        Vec::new()
    } else {
        let out = read_links(bindings, links);
        // SAFETY: `links` came from `FPDFLink_LoadWebLinks` and is closed exactly once.
        unsafe { bindings.FPDFLink_CloseWebLinks(links) };
        out
    };
    // SAFETY: closed exactly once; nothing derived from it outlives this call.
    unsafe { bindings.FPDFText_ClosePage(text_page) };
    out
}

fn read_links(bindings: &dyn PdfiumLibraryBindings, links: FPDF_PAGELINK) -> Vec<RawWebLink> {
    // SAFETY: `links` is a live link page (see `web_links`).
    let count = unsafe { bindings.FPDFLink_CountWebLinks(links) }.max(0);
    let mut out = Vec::with_capacity(count as usize);
    for index in 0..count {
        let url = url_at(bindings, links, index);
        if url.is_empty() {
            continue;
        }
        // SAFETY: `index < count`.
        let rect_count = unsafe { bindings.FPDFLink_CountRects(links, index) }.max(0);
        let mut rects = Vec::with_capacity(rect_count as usize);
        for r in 0..rect_count {
            let (mut left, mut top, mut right, mut bottom): (
                c_double,
                c_double,
                c_double,
                c_double,
            ) = (0.0, 0.0, 0.0, 0.0);
            // SAFETY: four valid out-pointers to locals; `r < rect_count`.
            let ok = unsafe {
                bindings.FPDFLink_GetRect(
                    links,
                    index,
                    r,
                    &mut left,
                    &mut top,
                    &mut right,
                    &mut bottom,
                )
            };
            if ok != 0 {
                rects.push((left, top, right, bottom));
            }
        }
        let (mut start, mut chars): (c_int, c_int) = (-1, 0);
        // SAFETY: two valid out-pointers to locals.
        let ranged =
            unsafe { bindings.FPDFLink_GetTextRange(links, index, &mut start, &mut chars) };
        if ranged == 0 {
            start = -1;
            chars = 0;
        }
        out.push(RawWebLink {
            url,
            rects,
            char_start: start,
            char_count: chars,
        });
    }
    out
}

/// `FPDFLink_GetURL`: UTF-16 units including the terminator; a first call sizes the buffer.
fn url_at(bindings: &dyn PdfiumLibraryBindings, links: FPDF_PAGELINK, index: c_int) -> String {
    // SAFETY: a null buffer with length 0 only asks for the size.
    let units = unsafe { bindings.FPDFLink_GetURL(links, index, std::ptr::null_mut(), 0) };
    if units <= 1 {
        return String::new();
    }
    let mut buffer: Vec<c_ushort> = vec![0; units as usize];
    // SAFETY: `buffer` holds exactly `units` UTF-16 units.
    let written = unsafe { bindings.FPDFLink_GetURL(links, index, buffer.as_mut_ptr(), units) };
    let len = (written.max(0) as usize).min(buffer.len());
    let text = &buffer[..len];
    let text = text.strip_suffix(&[0]).unwrap_or(text);
    String::from_utf16_lossy(text)
}
