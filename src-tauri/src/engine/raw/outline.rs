//! Outline and destination reads that pdfium-render does not expose (P2).
//!
//! pdfium-render's `PdfBookmark` hides the **sign** of `/Count` (`children_len()` is its
//! absolute value, and `FPDFBookmark_GetCount` returns the raw `/Count`), and its bookmark
//! handle is `pub(crate)`, so the open / closed state of a node and a URI action cannot be read
//! through it. This walks the tree with the raw `FPDFBookmark_*` API instead, and reduces every
//! `FPDF_DEST` — of an outline node or of a Link annotation — to the contract's page +
//! [`OutlineDest`] with one mapping.

use crate::ipc::types::{LinkDest, OutlineDest, OutlineNode};
use pdfium_render::prelude::{
    FPDF_ACTION, FPDF_BOOKMARK, FPDF_BOOL, FPDF_DEST, FPDF_DOCUMENT, FS_FLOAT, PdfiumLibraryBindings,
};
use std::collections::HashSet;
use std::os::raw::{c_ulong, c_void};

/// Deeper than this is treated as a malformed (cyclic) tree and cut off.
pub const MAX_DEPTH: usize = 32;
/// The most nodes one outline read returns — a guard against a `/Next` loop PDFium follows.
pub const MAX_NODES: usize = 20_000;

// `FPDFAction_GetType` / `FPDFDest_GetView` values (bindgen 7881; not in the prelude).
const PDFACTION_GOTO: c_ulong = 1;
const PDFACTION_URI: c_ulong = 3;
const PDFDEST_VIEW_XYZ: c_ulong = 1;
const PDFDEST_VIEW_FITH: c_ulong = 3;
const PDFDEST_VIEW_FITV: c_ulong = 4;
const PDFDEST_VIEW_FITR: c_ulong = 5;
const PDFDEST_VIEW_FITBH: c_ulong = 7;
const PDFDEST_VIEW_FITBV: c_ulong = 8;

/// The whole outline as the contract's nested node list, depth-first, document order.
///
/// * `page` / `dest`: `FPDFBookmark_GetDest`, which already falls back to a GoTo action;
/// * `url`: a URI action (`FPDFAction_GetURIPath`), with `page` `None`;
/// * `open`: `Some(/Count > 0)` for a node with children, `None` for a leaf.
pub fn read(bindings: &dyn PdfiumLibraryBindings, document: FPDF_DOCUMENT) -> Vec<OutlineNode> {
    let mut seen: HashSet<usize> = HashSet::new();
    let mut budget = MAX_NODES;
    // SAFETY: `document` is live for the caller's borrow; a null parent asks for the first
    // top-level bookmark.
    let first = unsafe { bindings.FPDFBookmark_GetFirstChild(document, std::ptr::null_mut()) };
    level(bindings, document, first, 0, &mut seen, &mut budget)
}

fn level(
    bindings: &dyn PdfiumLibraryBindings,
    document: FPDF_DOCUMENT,
    first: FPDF_BOOKMARK,
    depth: usize,
    seen: &mut HashSet<usize>,
    budget: &mut usize,
) -> Vec<OutlineNode> {
    let mut out = Vec::new();
    if depth > MAX_DEPTH {
        return out;
    }
    let mut current = first;
    while !current.is_null() {
        // A bookmark handle is the item's dictionary pointer, so a revisit is a cycle.
        if !seen.insert(current as usize) || *budget == 0 {
            break;
        }
        *budget -= 1;
        out.push(node(bindings, document, current, depth, seen, budget));
        // SAFETY: `current` is a live bookmark of `document`.
        current = unsafe { bindings.FPDFBookmark_GetNextSibling(document, current) };
    }
    out
}

fn node(
    bindings: &dyn PdfiumLibraryBindings,
    document: FPDF_DOCUMENT,
    bookmark: FPDF_BOOKMARK,
    depth: usize,
    seen: &mut HashSet<usize>,
    budget: &mut usize,
) -> OutlineNode {
    // SAFETY (every call below): `bookmark` is a live bookmark of the live `document`.
    let first_child = unsafe { bindings.FPDFBookmark_GetFirstChild(document, bookmark) };
    let children = level(bindings, document, first_child, depth + 1, seen, budget);
    let count = unsafe { bindings.FPDFBookmark_GetCount(bookmark) };
    let dest = unsafe { bindings.FPDFBookmark_GetDest(document, bookmark) };
    let (page, view) = if dest.is_null() {
        (None, None)
    } else {
        (dest_page(bindings, document, dest), dest_view(bindings, dest))
    };
    let action = unsafe { bindings.FPDFBookmark_GetAction(bookmark) };
    let url = if page.is_none() { uri_of(bindings, document, action) } else { None };
    OutlineNode {
        title: title(bindings, bookmark).unwrap_or_default(),
        page,
        dest: if page.is_some() { view } else { None },
        url,
        open: if children.is_empty() { None } else { Some(count > 0) },
        children,
    }
}

fn title(bindings: &dyn PdfiumLibraryBindings, bookmark: FPDF_BOOKMARK) -> Option<String> {
    // SAFETY: a null buffer asks for the length; the second call gets exactly that many bytes.
    let len = unsafe { bindings.FPDFBookmark_GetTitle(bookmark, std::ptr::null_mut(), 0) } as usize;
    if len < 4 {
        return None;
    }
    let mut buffer = vec![0u8; len];
    let written = unsafe {
        bindings.FPDFBookmark_GetTitle(bookmark, buffer.as_mut_ptr() as *mut c_void, len as c_ulong)
    } as usize;
    super::doc::decode_utf16le(&buffer[..written.min(len)])
}

/// A URI action's target, `None` for anything else (GoTo, Launch, JavaScript, …).
pub fn uri_of(
    bindings: &dyn PdfiumLibraryBindings,
    document: FPDF_DOCUMENT,
    action: FPDF_ACTION,
) -> Option<String> {
    if action.is_null() {
        return None;
    }
    // SAFETY: `action` is non-null and belongs to `document`; a null buffer asks for the length.
    unsafe {
        if bindings.FPDFAction_GetType(action) != PDFACTION_URI {
            return None;
        }
        let len = bindings.FPDFAction_GetURIPath(document, action, std::ptr::null_mut(), 0) as usize;
        if len < 2 {
            return None;
        }
        let mut buffer = vec![0u8; len];
        let written = bindings.FPDFAction_GetURIPath(
            document,
            action,
            buffer.as_mut_ptr() as *mut c_void,
            len as c_ulong,
        ) as usize;
        let end = written.min(len).saturating_sub(1);
        String::from_utf8(buffer[..end].to_vec()).ok()
    }
}

/// Whether `action` is a same-document GoTo.
pub fn is_goto(bindings: &dyn PdfiumLibraryBindings, action: FPDF_ACTION) -> bool {
    // SAFETY: checked for null; `FPDFAction_GetType` only reads the dictionary.
    !action.is_null() && unsafe { bindings.FPDFAction_GetType(action) } == PDFACTION_GOTO
}

/// `FPDFDest_GetDestPageIndex`, `None` when the destination names no page of this document.
pub fn dest_page(
    bindings: &dyn PdfiumLibraryBindings,
    document: FPDF_DOCUMENT,
    dest: FPDF_DEST,
) -> Option<u16> {
    // SAFETY: `dest` is a live destination of `document`.
    let index = unsafe { bindings.FPDFDest_GetDestPageIndex(document, dest) };
    u16::try_from(index).ok()
}

/// `FPDFDest_GetView` + `FPDFDest_GetLocationInPage`, reduced to what a scroller can act on —
/// the mapping `STAGE1C_NOTES.md` §7.1 introduced for the outline:
///
/// * `/XYZ left top zoom` → x / y / zoom (a zoom of 0 means "retain" → `None`);
/// * `/FitH top`, `/FitBH top` → y; `/FitV left`, `/FitBV left` → x;
/// * `/FitR l b r t` → x = l, y = t;
/// * `/Fit`, `/FitB`, anything unknown, or every value absent → `None` (a plain page jump).
pub fn dest_view(bindings: &dyn PdfiumLibraryBindings, dest: FPDF_DEST) -> Option<OutlineDest> {
    let (mut has_x, mut has_y, mut has_zoom): (FPDF_BOOL, FPDF_BOOL, FPDF_BOOL) = (0, 0, 0);
    let (mut x, mut y, mut zoom): (FS_FLOAT, FS_FLOAT, FS_FLOAT) = (0.0, 0.0, 0.0);
    // SAFETY: `dest` is live; every out-parameter is a valid local.
    let located = unsafe {
        bindings.FPDFDest_GetLocationInPage(
            dest,
            &mut has_x,
            &mut has_y,
            &mut has_zoom,
            &mut x,
            &mut y,
            &mut zoom,
        )
    };
    let location = if bindings.is_true(located) {
        (
            bindings.is_true(has_x).then_some(x),
            bindings.is_true(has_y).then_some(y),
            bindings.is_true(has_zoom).then_some(zoom).filter(|z| *z != 0.0),
        )
    } else {
        (None, None, None)
    };

    let mut count: c_ulong = 0;
    let mut params: [FS_FLOAT; 4] = [0.0; 4];
    // SAFETY: `params` holds the 4 floats the API may write.
    let view = unsafe { bindings.FPDFDest_GetView(dest, &mut count, params.as_mut_ptr()) };
    let out = match (view, count) {
        (PDFDEST_VIEW_XYZ, 3) => OutlineDest {
            x: location.0,
            y: location.1,
            zoom: location.2.filter(|z| *z > 0.0),
        },
        (PDFDEST_VIEW_FITH | PDFDEST_VIEW_FITBH, 1) => OutlineDest { x: None, y: Some(params[0]), zoom: None },
        (PDFDEST_VIEW_FITV | PDFDEST_VIEW_FITBV, 1) => OutlineDest { x: Some(params[0]), y: None, zoom: None },
        (PDFDEST_VIEW_FITR, 4) => OutlineDest { x: Some(params[0]), y: Some(params[3]), zoom: None },
        _ => return None,
    };
    if out.x.is_none() && out.y.is_none() && out.zoom.is_none() {
        return None;
    }
    Some(out)
}

/// A destination as a Link annotation's [`LinkDest`]: `None` when it names no page.
pub fn link_dest(
    bindings: &dyn PdfiumLibraryBindings,
    document: FPDF_DOCUMENT,
    dest: FPDF_DEST,
) -> Option<LinkDest> {
    if dest.is_null() {
        return None;
    }
    let page = dest_page(bindings, document, dest)?;
    let view = dest_view(bindings, dest).unwrap_or_default();
    Some(LinkDest { page, x: view.x, y: view.y, zoom: view.zoom })
}
