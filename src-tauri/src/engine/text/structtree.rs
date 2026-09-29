//! `get_reading_order` (v0.3, V6): a tagged page's text in the order of its structure tree.
//!
//! PDFium extracts text in content-stream order; a tagged PDF says how it is meant to be read
//! (the order of the marked-content ids its structure tree references). Read aloud and the
//! screen-reader text region (H9) take the page's text-layer runs in that order. A page
//! without a structure tree — or whose tree references nothing this page draws — is one run
//! in content order (`tagged: false`), exactly what they used before.

use crate::engine::raw;
use crate::engine::registry::OpenDoc;
use crate::engine::text::layer;
use crate::ipc::types::ReadingOrder;
use crate::ipc::EngineError;
use std::collections::HashMap;

/// The page's reading order over its text-layer char indices.
pub fn reading_order(doc: &mut OpenDoc<'_>, page: u16) -> Result<ReadingOrder, EngineError> {
    let chars = layer::layer(doc, page)?.chars.len();
    let bindings = doc.bindings();
    let pdf_page = doc.page(page)?;
    let Some(tree) = raw::structtree::tree_mcids(bindings, pdf_page) else {
        return Ok(untagged(chars));
    };
    let marks = raw::structtree::char_mcids(bindings, pdf_page);
    if marks.len() != chars {
        // Never expected (same page, same algorithm) — but a mismatch must not misplace text.
        tracing::warn!(
            page,
            chars,
            marks = marks.len(),
            "reading order: text pages disagree"
        );
        return Ok(untagged(chars));
    }
    match order_runs(&marks, &tree) {
        Some(runs) => Ok(ReadingOrder { tagged: true, runs }),
        None => Ok(untagged(chars)),
    }
}

fn untagged(chars: usize) -> ReadingOrder {
    ReadingOrder {
        tagged: false,
        runs: if chars == 0 {
            Vec::new()
        } else {
            vec![[0, chars as u32]]
        },
    }
}

/// Orders the page's chars by the structure tree. Pure, so it is unit-tested.
///
/// * The chars split into runs of one MCID; a char with none (`-1`: a generated `\r\n`
///   between two runs, or untagged content) stays with the run before it — or forms a run of
///   its own at the start of the page.
/// * Runs are stably sorted by the first position of their MCID in `tree`; runs whose MCID
///   the tree does not reference (artifacts: running heads, page numbers) go after every
///   tagged run, in content order — never dropped.
/// * Adjacent runs that end up contiguous are merged.
///
/// `None` when no char carries an MCID the tree references (nothing to reorder by).
pub fn order_runs(char_mcids: &[i32], tree: &[i32]) -> Option<Vec<[u32; 2]>> {
    let mut rank: HashMap<i32, usize> = HashMap::new();
    for (i, &mcid) in tree.iter().enumerate() {
        rank.entry(mcid).or_insert(i);
    }
    // (mcid, start, end)
    let mut runs: Vec<(i32, u32, u32)> = Vec::new();
    for (i, &mcid) in char_mcids.iter().enumerate() {
        let i = i as u32;
        match runs.last_mut() {
            Some(last) if mcid == -1 || mcid == last.0 => last.2 = i + 1,
            _ => runs.push((mcid, i, i + 1)),
        }
    }
    if !runs.iter().any(|(m, _, _)| rank.contains_key(m)) {
        return None;
    }
    let untagged_rank = tree.len();
    // `sort_by_key` is stable: equal ranks keep content order.
    runs.sort_by_key(|(m, _, _)| rank.get(m).copied().unwrap_or(untagged_rank));
    let mut out: Vec<[u32; 2]> = Vec::with_capacity(runs.len());
    for (_, start, end) in runs {
        match out.last_mut() {
            Some(last) if last[1] == start => last[1] = end,
            _ => out.push([start, end]),
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runs_follow_the_tree_not_the_content_order() {
        // content: "BB\n" (mcid 1) then "AA" (mcid 0); the tree reads 0 then 1
        let marks = [1, 1, -1, 0, 0];
        assert_eq!(order_runs(&marks, &[0, 1]), Some(vec![[3, 5], [0, 3]]));
        // the tree in content order merges back into one run
        assert_eq!(order_runs(&marks, &[1, 0]), Some(vec![[0, 5]]));
    }

    #[test]
    fn untagged_content_goes_last_and_nothing_is_dropped() {
        // a running head (no mcid) first, then two tagged paragraphs in reverse, then a page
        // number (mcid 9, which the tree does not reference)
        let marks = [-1, -1, 2, 2, 1, 1, 9];
        let runs = order_runs(&marks, &[1, 2]).unwrap();
        assert_eq!(runs, vec![[4, 6], [2, 4], [0, 2], [6, 7]]);
        let covered: u32 = runs.iter().map(|r| r[1] - r[0]).sum();
        assert_eq!(covered, marks.len() as u32);
    }

    #[test]
    fn nothing_referenced_means_no_tagged_order() {
        assert_eq!(order_runs(&[-1, -1, 5], &[0, 1]), None);
        assert_eq!(order_runs(&[], &[0]), None);
    }
}
