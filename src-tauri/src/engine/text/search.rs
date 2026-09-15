//! Rust search over the cached per-page text — `ARCHITECTURE.md` §5.
//!
//! **Not** `FPDFText_FindStart`: pdfium's search returns rects with no char index (text
//! spike §3), and we need indices to drive selection and "find → replace". Matching runs
//! over the case-folded char array (65 µs/page); rects come from the full text layer, which
//! is only built for pages that actually have hits.

use crate::engine::registry::OpenDoc;
use crate::engine::text::layer::{self, fold};
use crate::ipc::types::SearchHit;
use crate::ipc::EngineError;

/// ±40 chars of context for the results list.
const CONTEXT: usize = 40;

#[derive(Debug, Clone)]
pub struct Query {
    pub needle: Vec<char>,
    pub match_case: bool,
    pub whole_word: bool,
}

impl Query {
    pub fn new(query: &str, match_case: bool, whole_word: bool) -> Option<Self> {
        let needle: Vec<char> = if match_case {
            query.chars().collect()
        } else {
            query.chars().map(fold).collect()
        };
        if needle.is_empty() {
            return None;
        }
        Some(Self {
            needle,
            match_case,
            whole_word,
        })
    }
}

/// Char indices of every match on one page.
pub fn match_indices(haystack: &[char], query: &Query) -> Vec<u32> {
    let n = query.needle.len();
    if n == 0 || haystack.len() < n {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut i = 0usize;
    while i + n <= haystack.len() {
        let matched = haystack[i..i + n] == query.needle[..]
            && (!query.whole_word || is_whole_word(haystack, i, n));
        if matched {
            out.push(i as u32);
            i += n;
        } else {
            i += 1;
        }
    }
    out
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

fn is_whole_word(haystack: &[char], start: usize, len: usize) -> bool {
    let before_ok = start == 0 || !is_word_char(haystack[start - 1]);
    let end = start + len;
    let after_ok = end >= haystack.len() || !is_word_char(haystack[end]);
    before_ok && after_ok
}

/// Searches one page, building the text layer only when there is at least one hit.
pub fn search_page(
    doc: &mut OpenDoc<'_>,
    page: u16,
    query: &Query,
) -> Result<Vec<SearchHit>, EngineError> {
    let page_text = layer::page_text(doc, page)?;
    let haystack: &[char] = if query.match_case {
        &page_text.chars
    } else {
        &page_text.folded
    };
    let starts = match_indices(haystack, query);
    if starts.is_empty() {
        return Ok(Vec::new());
    }
    let length = query.needle.len() as u32;
    let text_layer = layer::layer(doc, page)?;
    let mut hits = Vec::with_capacity(starts.len());
    for start in starts {
        let (context, context_match) =
            context_around(&page_text.chars, start as usize, length as usize);
        hits.push(SearchHit {
            page,
            char_start: start,
            char_length: length,
            rects: text_layer.range_rects(start, length),
            context,
            context_match,
        });
    }
    Ok(hits)
}

fn context_around(chars: &[char], start: usize, len: usize) -> (String, [u32; 2]) {
    let from = start.saturating_sub(CONTEXT);
    let to = (start + len + CONTEXT).min(chars.len());
    let context: String = chars[from..to]
        .iter()
        .map(|&c| if c == '\r' || c == '\n' { ' ' } else { c })
        .collect();
    (context, [(start - from) as u32, len as u32])
}

/// Page visit order: outward from `from_page`, so the first hits are the nearest ones.
pub fn visit_order(page_count: u16, from_page: u16) -> Vec<u16> {
    let mut order = Vec::with_capacity(page_count as usize);
    if page_count == 0 {
        return order;
    }
    let from = from_page.min(page_count - 1);
    order.push(from);
    let mut offset = 1i32;
    while order.len() < page_count as usize {
        let forward = from as i32 + offset;
        if forward < page_count as i32 {
            order.push(forward as u16);
        }
        let backward = from as i32 - offset;
        if backward >= 0 && order.len() < page_count as usize {
            order.push(backward as u16);
        }
        offset += 1;
        if offset > page_count as i32 {
            break;
        }
    }
    order
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chars(s: &str) -> Vec<char> {
        s.chars().collect()
    }

    #[test]
    fn finds_case_insensitive_and_whole_word() {
        let haystack = chars("trace monkey Monkey monkeys");
        let q = Query::new("monkey", false, false).unwrap();
        let folded: Vec<char> = haystack.iter().map(|&c| fold(c)).collect();
        assert_eq!(match_indices(&folded, &q), vec![6, 13, 20]);

        let q = Query::new("monkey", true, false).unwrap();
        assert_eq!(match_indices(&haystack, &q), vec![6, 20]);

        let q = Query::new("monkey", false, true).unwrap();
        assert_eq!(match_indices(&folded, &q), vec![6, 13]);
    }

    #[test]
    fn visits_pages_outward_from_the_current_one() {
        assert_eq!(visit_order(5, 2), vec![2, 3, 1, 4, 0]);
        assert_eq!(visit_order(3, 0), vec![0, 1, 2]);
        assert_eq!(visit_order(1, 0), vec![0]);
        assert!(visit_order(0, 0).is_empty());
    }
}
