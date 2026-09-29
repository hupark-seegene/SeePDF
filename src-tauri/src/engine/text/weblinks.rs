//! `get_web_links` (v0.3, V5): plain-text URLs made clickable in 읽기 mode.
//!
//! PDFium's own detector (`FPDFLink_LoadWebLinks`, `engine::raw::weblinks`) finds `http(s)://`,
//! `www.` addresses and e-mail addresses (returned as `mailto:`) in the page text. Nothing is
//! written to the document: the frontend draws hit boxes over the rects and follows a click
//! exactly as it follows a Link annotation's URI (confirm first, http(s) / mailto only).

use crate::engine::raw;
use crate::engine::registry::OpenDoc;
use crate::ipc::types::{Rect, WebLink};
use crate::ipc::EngineError;

/// The most links one page reports (a page of nothing but addresses is still bounded).
pub const MAX_LINKS: usize = 2_000;

/// Every web link PDFium detects in the text of `page`, in text order.
pub fn web_links(doc: &mut OpenDoc<'_>, page: u16) -> Result<Vec<WebLink>, EngineError> {
    let bindings = doc.bindings();
    let pdf_page = doc.page(page)?;
    let found = raw::weblinks::web_links(bindings, pdf_page);
    Ok(found
        .into_iter()
        .filter_map(|link| {
            let rects: Vec<Rect> = link
                .rects
                .iter()
                .map(|&(left, top, right, bottom)| Rect {
                    l: left.min(right) as f32,
                    b: bottom.min(top) as f32,
                    r: left.max(right) as f32,
                    t: bottom.max(top) as f32,
                })
                .filter(|r| r.r > r.l && r.t > r.b)
                .collect();
            (!rects.is_empty()).then(|| WebLink {
                url: link.url,
                rects,
                char_start: link.char_start.max(0) as u32,
                char_count: link.char_count.max(0) as u32,
            })
        })
        .take(MAX_LINKS)
        .collect())
}
