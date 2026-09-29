//! 모아찍기 (N-up) and 소책자 (booklet) — v0.3 pkg8 (X2), `IPC_CONTRACT.md` §7.7b.
//!
//! `make_nup` builds a **new** document whose sheets each carry `perSheet` pages of the source,
//! with PDFium's own `FPDF_ImportNPagesToOne` (pdfium-render's `tile_into_new_document`): every
//! source page becomes a form XObject scaled to fit its cell and centred in it. That call only
//! lays pages out row by row, so any other order — 세로 방향 (down, then across) or a
//! saddle-stitch booklet — is produced by importing the pages into an intermediate document
//! in the order the cells will be filled, blank pages included.
//!
//! The source is first flattened for paper ([`super::print_bytes`] with the 주석 option), because
//! `FPDF_ImportNPagesToOne` copies page **content** only: annotations and form fields that were
//! not baked in would vanish from the sheet.
//!
//! `FPDF_ImportNPagesToOne` ignores a source page's `/Rotate`; the grid is chosen from the
//! first page's *display* size, so a rotated page is fitted as if unrotated. Mixed page sizes
//! are each fitted into the same cell size.

use crate::engine::export::{print_bytes, PrintAnnots};
use crate::engine::raw;
use crate::engine::types::EngineState;
use crate::ipc::error::PdfiumResultExt;
use crate::ipc::types::PageIndex;
use crate::ipc::{EngineError, ErrorCode};
use pdfium_render::prelude::*;
use serde::{Deserialize, Serialize};

/// A4 portrait in points.
pub const A4: (f32, f32) = (595.28, 841.89);
/// US Letter portrait in points.
pub const LETTER: (f32, f32) = (612.0, 792.0);

/// Which way the cells of one sheet fill.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum NupOrder {
    /// 가로 방향: left to right, then down (Z).
    #[default]
    Across,
    /// 세로 방향: top to bottom, then across (N).
    Down,
}

/// The sheet size.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum NupPaper {
    /// The first selected page's size.
    #[default]
    Auto,
    A4,
    Letter,
}

/// `make_nup`'s options.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NupOptions {
    /// 1, 2, 4, 6 or 9 pages per sheet (a booklet is always 2).
    pub per_sheet: u8,
    #[serde(default)]
    pub order: NupOrder,
    /// 소책자: saddle-stitch order, 2 pages per side, padded to a multiple of 4 with blanks.
    #[serde(default)]
    pub booklet: bool,
    #[serde(default)]
    pub paper: NupPaper,
    /// What of the markup is baked in first (인쇄 ▸ 주석); default everything that prints.
    #[serde(default)]
    pub annots: PrintAnnots,
}

/// `make_nup`'s answer.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NupResult {
    /// The file written: `outPath`, or a temp file in the print temp directory.
    pub path: String,
    /// Sheets in the result.
    pub page_count: u32,
}

/// The saddle-stitch order for `n` pages: sheet sides `[last, first]`, `[second, second-last]`,
/// … over `n` rounded up to a multiple of 4. `None` is a blank page. 8 pages →
/// `[8,1,2,7,6,3,4,5]` (1-based).
pub fn booklet_order(n: usize) -> Vec<Option<usize>> {
    if n == 0 {
        return Vec::new();
    }
    let m = n.div_ceil(4) * 4;
    let at = |i: usize| if i < n { Some(i) } else { None };
    let mut out = Vec::with_capacity(m);
    for s in 0..m / 4 {
        out.push(at(m - 1 - 2 * s));
        out.push(at(2 * s));
        out.push(at(2 * s + 1));
        out.push(at(m - 2 - 2 * s));
    }
    out
}

/// Reorders `seq` so that a row-major fill of `cols × rows` cells reads down each column first.
/// The last sheet is padded with blanks when it is short, so its columns stay columns.
pub fn down_order<T: Copy>(seq: &[Option<T>], cols: usize, rows: usize) -> Vec<Option<T>> {
    let per = cols * rows;
    if per <= 1 || rows <= 1 {
        return seq.to_vec();
    }
    let mut out = Vec::with_capacity(seq.len().div_ceil(per) * per);
    for chunk in seq.chunks(per) {
        for r in 0..rows {
            for c in 0..cols {
                out.push(chunk.get(c * rows + r).copied().flatten());
            }
        }
    }
    // Trailing blanks of the last sheet add nothing.
    while matches!(out.last(), Some(None)) {
        out.pop();
    }
    out
}

/// The grid and sheet size that print `per` pages of `src_w × src_h` largest: every
/// `cols × rows == per` on the sheet in both orientations, portrait winning a tie.
/// Returns `(cols, rows, sheet_w, sheet_h)`.
pub fn choose_grid(per: u8, src: (f32, f32), paper: (f32, f32)) -> (u8, u8, f32, f32) {
    let (pw, ph) = (paper.0.min(paper.1), paper.0.max(paper.1));
    let per = per.max(1);
    let mut best = (1u8, per, pw, ph);
    let mut best_scale = 0.0f32;
    for cols in 1..=per {
        if !per.is_multiple_of(cols) {
            continue;
        }
        let rows = per / cols;
        for (w, h) in [(pw, ph), (ph, pw)] {
            let scale = (w / cols as f32 / src.0.max(1.0)).min(h / rows as f32 / src.1.max(1.0));
            if scale > best_scale * 1.0001 {
                best_scale = scale;
                best = (cols, rows, w, h);
            }
        }
    }
    best
}

/// Builds the n-up / booklet document for `pages` (all when `None` or empty) and returns its
/// bytes and sheet count.
pub fn make_nup_bytes(
    st: &mut EngineState<'_>,
    doc_id: &str,
    pages: Option<&[PageIndex]>,
    opts: &NupOptions,
) -> Result<(Vec<u8>, u32), EngineError> {
    let per = if opts.booklet { 2 } else { opts.per_sheet };
    if !matches!(per, 1 | 2 | 4 | 6 | 9) {
        return Err(EngineError::invalid(format!(
            "perSheet must be 1, 2, 4, 6 or 9 (got {per})"
        )));
    }
    let selected = super::check_pages(st, doc_id, pages.unwrap_or(&[]))?;
    let first = *selected
        .first()
        .ok_or_else(|| EngineError::invalid("no pages to lay out"))?;
    let geom = st.doc(doc_id)?.geom(first)?.clone();
    let src = (geom.width_pt, geom.height_pt);
    let paper = match opts.paper {
        NupPaper::Auto => src,
        NupPaper::A4 => A4,
        NupPaper::Letter => LETTER,
    };
    let (cols, rows, sheet_w, sheet_h) = if opts.booklet {
        // Two pages side by side on a landscape sheet.
        let (pw, ph) = (paper.0.min(paper.1), paper.0.max(paper.1));
        (2, 1, ph, pw)
    } else {
        choose_grid(per, src, paper)
    };

    let password = st.doc(doc_id)?.password.clone();
    let flat = print_bytes(st, doc_id, Some(&selected), opts.annots)?;
    let bindings = raw::bindings(st.pdfium);
    let source = st
        .pdfium
        .load_pdf_from_byte_vec(flat, password.as_deref())
        .map_err(|e| EngineError::pdfium("reopen for n-up", e))?;
    let n = raw::page::page_count(bindings, &source) as usize;

    let mut seq: Vec<Option<usize>> = if opts.booklet {
        booklet_order(n)
    } else {
        (0..n).map(Some).collect()
    };
    if opts.order == NupOrder::Down && !opts.booklet {
        seq = down_order(&seq, cols as usize, rows as usize);
    }

    let mut ordered = st
        .pdfium
        .create_new_pdf()
        .map_err(|e| EngineError::pdfium("create the n-up source", e))?;
    let mut i = 0;
    while i < seq.len() {
        match seq[i] {
            Some(_) => {
                // One import per run of real pages.
                let run: Vec<u16> = seq[i..].iter().map_while(|p| p.map(|p| p as u16)).collect();
                let at = raw::page::page_count(bindings, &ordered);
                raw::page::import_pages_by_index(bindings, &ordered, &source, &run, at)?;
                i += run.len();
            }
            None => {
                ordered
                    .pages_mut()
                    .create_page_at_end(PdfPagePaperSize::Custom(
                        PdfPoints::new(src.0),
                        PdfPoints::new(src.1),
                    ))
                    .ctx("add a blank page")?;
                i += 1;
            }
        }
    }
    drop(source);

    let sheets = ordered
        .pages()
        .tile_into_new_document(
            rows,
            cols,
            PdfPagePaperSize::Custom(PdfPoints::new(sheet_w), PdfPoints::new(sheet_h)),
        )
        .map_err(|e| EngineError::pdfium("FPDF_ImportNPagesToOne", e))?;
    let count = raw::page::page_count(bindings, &sheets) as u32;
    if count == 0 {
        return Err(EngineError::new(
            ErrorCode::Pdfium,
            "FPDF_ImportNPagesToOne produced no pages",
        ));
    }
    let bytes = raw::save::save_as_copy(bindings, &sheets, raw::save::SaveFlags::NoIncremental)?;
    Ok((bytes, count))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn booklet_of_eight() {
        let order: Vec<usize> = booklet_order(8)
            .into_iter()
            .map(|p| p.unwrap() + 1)
            .collect();
        assert_eq!(order, vec![8, 1, 2, 7, 6, 3, 4, 5]);
    }

    #[test]
    fn booklet_pads_with_blanks() {
        let order = booklet_order(6);
        assert_eq!(order.len(), 8);
        assert_eq!(
            order,
            vec![
                None,
                Some(0),
                Some(1),
                None,
                Some(5),
                Some(2),
                Some(3),
                Some(4)
            ]
        );
        assert!(booklet_order(0).is_empty());
    }

    #[test]
    fn down_order_reads_columns() {
        let seq: Vec<Option<u32>> = (1..=4).map(Some).collect();
        // 2 × 2: cells row-major get 1,3 / 2,4.
        assert_eq!(
            down_order(&seq, 2, 2),
            vec![Some(1), Some(3), Some(2), Some(4)]
        );
        // A short last sheet keeps its columns: 5 pages, 2 × 2 → sheet 2 is [5, -, -, -].
        let seq: Vec<Option<u32>> = (1..=5).map(Some).collect();
        assert_eq!(
            down_order(&seq, 2, 2),
            vec![Some(1), Some(3), Some(2), Some(4), Some(5)]
        );
    }

    #[test]
    fn grid_prefers_the_larger_cell() {
        // Portrait A4 pages: 2-up goes side by side on a landscape sheet, 4-up is 2 × 2
        // portrait, 6-up 3 × 2 landscape (cells 0.353× vs 0.333× for 2 × 3 portrait), 9-up
        // 3 × 3.
        let (c, r, w, h) = choose_grid(2, A4, A4);
        assert_eq!((c, r), (2, 1));
        assert!(w > h, "2-up is landscape");
        let (c, r, w, h) = choose_grid(4, A4, A4);
        assert_eq!((c, r), (2, 2));
        assert!(w < h);
        let (c, r, w, h) = choose_grid(6, A4, A4);
        assert_eq!((c, r), (3, 2));
        assert!(w > h);
        assert_eq!(choose_grid(9, A4, A4).0, 3);
        // Landscape slides 2-up stack on a portrait sheet.
        let (c, r, w, h) = choose_grid(2, (842.0, 595.0), A4);
        assert_eq!((c, r), (1, 2));
        assert!(w < h);
    }
}
