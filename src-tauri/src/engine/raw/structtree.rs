//! The page structure tree (tags) for reading order — v0.3, V6.
//!
//! PDFium reads a tagged PDF's structure (`FPDF_StructTree_*`) but pdfium-render 0.9.4 wraps
//! none of it, and it cannot write tags at all (BK6) — this is read-only. Two raw reads:
//!
//! * [`tree_mcids`] — the marked-content ids the page's structure tree references, in reading
//!   order: a depth-first walk of `/K`, a kid element's subtree in place of the element and a
//!   marked-content kid as its MCID, so interleaved `[elem, 3, elem]` kids keep their order.
//! * [`char_mcids`] — per text-page char, the MCID of the text object that drew it
//!   (`FPDFText_GetTextObject` → `FPDFPageObj_GetMarkedContentID`), `-1` for a generated char
//!   or untagged content. Its own `FPDF_TEXTPAGE`, so indices are the text layer's.

use pdfium_render::prelude::{PdfPage, PdfiumLibraryBindings, FPDF_STRUCTELEMENT};
use std::collections::HashSet;

/// Deeper than this is a malformed tree and cut off.
pub const MAX_DEPTH: usize = 64;
/// The most elements one walk visits.
pub const MAX_ELEMENTS: usize = 100_000;

/// The MCIDs of the page's structure tree in reading order, or `None` when the page has no
/// structure tree (or one with no marked content on this page).
pub fn tree_mcids(bindings: &dyn PdfiumLibraryBindings, page: &PdfPage<'_>) -> Option<Vec<i32>> {
    // SAFETY: `page` is live for the borrow, engine thread; the tree is closed below.
    let tree = unsafe { bindings.FPDF_StructTree_GetForPage(page.raw_handle()) };
    if tree.is_null() {
        return None;
    }
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    let mut budget = MAX_ELEMENTS;
    // SAFETY: `tree` is live until `FPDF_StructTree_Close` below.
    let count = unsafe { bindings.FPDF_StructTree_CountChildren(tree) }.max(0);
    for i in 0..count {
        // SAFETY: `i < count`; the element is owned by the tree.
        let child = unsafe { bindings.FPDF_StructTree_GetChildAtIndex(tree, i) };
        walk(bindings, child, 0, &mut seen, &mut budget, &mut out);
    }
    // SAFETY: closed exactly once; no element handle outlives it.
    unsafe { bindings.FPDF_StructTree_Close(tree) };
    (!out.is_empty()).then_some(out)
}

fn walk(
    bindings: &dyn PdfiumLibraryBindings,
    element: FPDF_STRUCTELEMENT,
    depth: usize,
    seen: &mut HashSet<usize>,
    budget: &mut usize,
    out: &mut Vec<i32>,
) {
    if element.is_null() || depth > MAX_DEPTH || *budget == 0 || !seen.insert(element as usize) {
        return;
    }
    *budget -= 1;
    // SAFETY: `element` is a live element of the open tree.
    let kids = unsafe { bindings.FPDF_StructElement_CountChildren(element) }.max(0);
    for k in 0..kids {
        // SAFETY: `k < kids`; null for a kid that is marked content, not an element.
        let child = unsafe { bindings.FPDF_StructElement_GetChildAtIndex(element, k) };
        if !child.is_null() {
            walk(bindings, child, depth + 1, seen, budget, out);
            continue;
        }
        // SAFETY: as above; -1 for a kid that is neither an element nor page content.
        let mcid = unsafe { bindings.FPDF_StructElement_GetChildMarkedContentID(element, k) };
        if mcid >= 0 {
            out.push(mcid);
        }
    }
}

/// Per char of the page's text, the MCID of the object that drew it (`-1` = none).
pub fn char_mcids(bindings: &dyn PdfiumLibraryBindings, page: &PdfPage<'_>) -> Vec<i32> {
    // SAFETY: `page` is live, engine thread; closed below.
    let text_page = unsafe { bindings.FPDFText_LoadPage(page.raw_handle()) };
    if text_page.is_null() {
        return Vec::new();
    }
    // SAFETY: `text_page` is live until closed below.
    let count = unsafe { bindings.FPDFText_CountChars(text_page) }.max(0);
    let mut out = Vec::with_capacity(count as usize);
    for i in 0..count {
        // SAFETY: `i < count`; the object (if any) belongs to the page and is not owned by us.
        let object = unsafe { bindings.FPDFText_GetTextObject(text_page, i) };
        let mcid = if object.is_null() {
            -1
        } else {
            // SAFETY: a live page object of `page`.
            unsafe { bindings.FPDFPageObj_GetMarkedContentID(object) }
        };
        out.push(mcid.max(-1));
    }
    // SAFETY: closed exactly once.
    unsafe { bindings.FPDFText_ClosePage(text_page) };
    out
}
