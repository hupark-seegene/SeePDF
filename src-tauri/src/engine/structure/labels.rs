//! Page labels (P2): `set_page_labels` writes the catalog's `/PageLabels` number tree,
//! `get_page_labels` reads it back as ranges; `DocInfo.pageLabels` / `PageGeom.label` are what
//! PDFium computes from it (`FPDF_GetPageLabel`).
//!
//! ISO 32000 §12.4.2: each `/Nums` entry `key → << /S /P /St >>` labels the pages from `key` to
//! the next key; `/S` is `/D` decimal, `/r` `/R` roman, `/a` `/A` letters (a…z, then aa…zz — the
//! letter repeated, not base 26), absent = no number; `/P` a prefix; `/St` the first number
//! (default 1). The tree must cover page 0, so a first range that starts later gets a plain
//! decimal range at 0 in front of it — the label PDFium shows for such pages anyway.
//! [`format`] mirrors PDFium's own `MakeRoman` / `MakeLetters` exactly, and the rewrite is
//! checked page by page against `FPDF_GetPageLabel` before it replaces the document.

use super::{refuse_encrypted, verify_failed};
use crate::engine::raw;
use crate::engine::registry::{self, MutateOpts};
use crate::engine::save;
use crate::engine::types::EngineState;
use crate::ipc::types::{ChangeReason, DocInfo, PageLabelRange, PageLabelStyle};
use crate::ipc::EngineError;
use lopdf::{Dictionary, Document, Object, ObjectId};
use std::collections::HashSet;

/// The largest `/St` accepted. PDFium keeps label numbers in an `int` and wraps roman numerals
/// at 1 000 000; nobody numbers a document past this.
pub const MAX_FIRST: u32 = 1_000_000;

/// `set_page_labels` — replaces `/PageLabels` with `ranges` (an empty list removes it) as one
/// undo step (`undo.pageLabels`). `unsupported` on an encrypted document; `invalidArgument`
/// for a start past the last page, two ranges starting on the same page, or `first` outside
/// 1…[`MAX_FIRST`].
pub fn set_page_labels(
    st: &mut EngineState<'_>,
    doc_id: &str,
    ranges: &[PageLabelRange],
) -> Result<DocInfo, EngineError> {
    refuse_encrypted(st, doc_id)?;
    let page_count = st.doc(doc_id)?.page_count();
    let ranges = normalize(ranges, page_count)?;
    let expected: Vec<Option<String>> = if ranges.is_empty() {
        vec![None; page_count as usize]
    } else {
        labels_for(&ranges, page_count)
            .into_iter()
            .map(|l| Some(l).filter(|l| !l.is_empty()))
            .collect()
    };
    let opts = MutateOpts::new("undo.pageLabels", ChangeReason::Edit)
        .all_pages()
        .keeps_text();
    registry::mutate_bytes_checked(
        st,
        doc_id,
        opts,
        move |bytes, _| write_labels(bytes, &ranges),
        move |bindings, reopened| {
            for (page, want) in expected.iter().enumerate() {
                let got = raw::doc::page_label(bindings, reopened, page as u16);
                if &got != want {
                    return Err(verify_failed(format!(
                        "PDFium labels page {page} {got:?}, expected {want:?}"
                    )));
                }
            }
            Ok(())
        },
    )
}

/// `get_page_labels` — the document's `/PageLabels` as ranges, sorted by `start` (`[]` when it
/// has none). Read from the bytes the document was last loaded from: PDFium never writes
/// `/PageLabels`, so every edit since then left it as it was. `unsupported` when encrypted.
pub fn get_page_labels(st: &EngineState<'_>, doc_id: &str) -> Result<Vec<PageLabelRange>, EngineError> {
    refuse_encrypted(st, doc_id)?;
    let doc = st.doc(doc_id)?;
    let page_count = doc.page_count();
    let parsed = super::load(&doc.bytes)?;
    Ok(read_ranges(&parsed)
        .into_iter()
        .filter(|r| r.start < page_count)
        .collect())
}

/// Sorted, validated ranges as they will read back: blank prefix → `None`, `first: 1` →
/// `None`, a decimal range at page 0 when the first starts later.
pub fn normalize(ranges: &[PageLabelRange], page_count: u16) -> Result<Vec<PageLabelRange>, EngineError> {
    let mut out: Vec<PageLabelRange> = Vec::with_capacity(ranges.len() + 1);
    let mut starts = HashSet::new();
    for r in ranges {
        if r.start >= page_count {
            return Err(EngineError::invalid(format!(
                "a page label range starts at page {} of a {page_count}-page document",
                r.start
            ))
            .with_page(r.start));
        }
        if !starts.insert(r.start) {
            return Err(EngineError::invalid(format!("two page label ranges start at page {}", r.start)));
        }
        if let Some(first) = r.first {
            if first == 0 || first > MAX_FIRST {
                return Err(EngineError::invalid(format!(
                    "a page label range must start numbering at 1…{MAX_FIRST}, not {first}"
                )));
            }
        }
        out.push(PageLabelRange {
            start: r.start,
            style: r.style,
            prefix: r.prefix.clone().filter(|p| !p.is_empty()),
            first: r.first.filter(|f| *f != 1),
        });
    }
    out.sort_by_key(|r| r.start);
    if out.first().is_some_and(|r| r.start > 0) {
        out.insert(0, PageLabelRange { start: 0, style: PageLabelStyle::Decimal, prefix: None, first: None });
    }
    Ok(out)
}

/// Every page's label for sorted, normalised `ranges` (page 0 covered).
pub fn labels_for(ranges: &[PageLabelRange], page_count: u16) -> Vec<String> {
    (0..page_count)
        .map(|page| {
            match ranges.iter().rev().find(|r| r.start <= page) {
                Some(r) => {
                    let number = r.first.unwrap_or(1) as u64 + (page - r.start) as u64;
                    format(r.style, r.prefix.as_deref(), number)
                }
                // PDFium's fallback for a page before the first key.
                None => (page as u64 + 1).to_string(),
            }
        })
        .collect()
}

/// One label: the prefix, then the number in `style` (nothing for `none`).
pub fn format(style: PageLabelStyle, prefix: Option<&str>, number: u64) -> String {
    let mut out = prefix.unwrap_or_default().to_string();
    match style {
        PageLabelStyle::Decimal => out.push_str(&number.to_string()),
        PageLabelStyle::Roman => out.push_str(&roman(number)),
        PageLabelStyle::RomanUpper => out.push_str(&roman(number).to_uppercase()),
        PageLabelStyle::Alpha => out.push_str(&letters(number)),
        PageLabelStyle::AlphaUpper => out.push_str(&letters(number).to_uppercase()),
        PageLabelStyle::NoNumber => {}
    }
    out
}

/// PDFium's `MakeRoman`: lower case, thousands as repeated `m`, wrapped at 1 000 000.
fn roman(number: u64) -> String {
    const TABLE: [(u64, &str); 13] = [
        (1000, "m"),
        (900, "cm"),
        (500, "d"),
        (400, "cd"),
        (100, "c"),
        (90, "xc"),
        (50, "l"),
        (40, "xl"),
        (10, "x"),
        (9, "ix"),
        (5, "v"),
        (4, "iv"),
        (1, "i"),
    ];
    let mut n = number % 1_000_000;
    let mut out = String::new();
    for (value, digits) in TABLE {
        while n >= value {
            out.push_str(digits);
            n -= value;
        }
    }
    out
}

/// PDFium's `MakeLetters`: a…z, then aa…zz, aaa… — the letter repeated `(n-1)/26 + 1` times
/// (modulo 1000, as PDFium caps it).
fn letters(number: u64) -> String {
    if number == 0 {
        return String::new();
    }
    let n = number - 1;
    let count = ((n / 26 + 1) % 1000) as usize;
    let letter = (b'a' + (n % 26) as u8) as char;
    std::iter::repeat_n(letter, count).collect()
}

fn style_name(style: PageLabelStyle) -> Option<&'static [u8]> {
    match style {
        PageLabelStyle::Decimal => Some(b"D"),
        PageLabelStyle::Roman => Some(b"r"),
        PageLabelStyle::RomanUpper => Some(b"R"),
        PageLabelStyle::Alpha => Some(b"a"),
        PageLabelStyle::AlphaUpper => Some(b"A"),
        PageLabelStyle::NoNumber => None,
    }
}

fn style_of(name: Option<&[u8]>) -> PageLabelStyle {
    match name {
        Some(b"D") => PageLabelStyle::Decimal,
        Some(b"r") => PageLabelStyle::Roman,
        Some(b"R") => PageLabelStyle::RomanUpper,
        Some(b"a") => PageLabelStyle::Alpha,
        Some(b"A") => PageLabelStyle::AlphaUpper,
        _ => PageLabelStyle::NoNumber,
    }
}

/// The lopdf rewrite: the old tree (and its `/Kids` nodes) out, `<< /Nums [...] >>` in.
fn write_labels(bytes: &[u8], ranges: &[PageLabelRange]) -> Result<Vec<u8>, EngineError> {
    let mut doc = super::load(bytes)?;
    let old = doc
        .catalog_mut()
        .map_err(|e| save::lopdf_error("catalog", e))?
        .remove(b"PageLabels");
    delete_tree(&mut doc, old);
    if !ranges.is_empty() {
        let mut nums = Vec::with_capacity(ranges.len() * 2);
        for r in ranges {
            let mut label = Dictionary::new();
            if let Some(name) = style_name(r.style) {
                label.set("S", Object::Name(name.to_vec()));
            }
            if let Some(prefix) = &r.prefix {
                label.set("P", save::pdf_text_string(prefix));
            }
            if let Some(first) = r.first.filter(|f| *f != 1) {
                label.set("St", Object::Integer(first as i64));
            }
            nums.push(Object::Integer(r.start as i64));
            nums.push(Object::Dictionary(label));
        }
        let mut tree = Dictionary::new();
        tree.set("Nums", Object::Array(nums));
        doc.catalog_mut()
            .map_err(|e| save::lopdf_error("catalog", e))?
            .set("PageLabels", Object::Dictionary(tree));
    }
    super::write(doc)
}

/// Deletes an old number tree's indirect nodes (root and `/Kids`).
fn delete_tree(doc: &mut Document, root: Option<Object>) {
    let mut stack: Vec<Object> = root.into_iter().collect();
    let mut seen: HashSet<ObjectId> = HashSet::new();
    while let Some(node) = stack.pop() {
        let dict = match node {
            Object::Reference(id) => {
                if !seen.insert(id) {
                    continue;
                }
                match doc.objects.remove(&id) {
                    Some(Object::Dictionary(d)) => d,
                    _ => continue,
                }
            }
            Object::Dictionary(d) => d,
            _ => continue,
        };
        if let Ok(Object::Array(kids)) = dict.get(b"Kids") {
            stack.extend(kids.iter().cloned());
        }
    }
}

/// `/PageLabels` as ranges, `/Nums` and `/Kids` both followed.
fn read_ranges(doc: &Document) -> Vec<PageLabelRange> {
    let Ok(catalog) = doc.catalog() else {
        return Vec::new();
    };
    let Ok(root) = catalog.get(b"PageLabels") else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut stack: Vec<&Object> = vec![root];
    let mut seen: HashSet<ObjectId> = HashSet::new();
    while let Some(node) = stack.pop() {
        if let Object::Reference(id) = node {
            if !seen.insert(*id) {
                continue;
            }
        }
        let Ok((_, Object::Dictionary(dict))) = doc.dereference(node) else {
            continue;
        };
        if let Ok((_, Object::Array(nums))) = dict.get(b"Nums").and_then(|n| doc.dereference(n)) {
            for pair in nums.chunks_exact(2) {
                let Ok(start) = pair[0].as_i64() else { continue };
                let Ok((_, Object::Dictionary(label))) = doc.dereference(&pair[1]) else { continue };
                let Ok(start) = u16::try_from(start) else { continue };
                let style = style_of(
                    label
                        .get(b"S")
                        .ok()
                        .and_then(|s| doc.dereference(s).ok())
                        .and_then(|(_, s)| s.as_name().ok()),
                );
                let prefix = label
                    .get(b"P")
                    .ok()
                    .and_then(|p| doc.dereference(p).ok())
                    .and_then(|(_, p)| lopdf::decode_text_string(p).ok())
                    .filter(|p| !p.is_empty());
                let first = label
                    .get(b"St")
                    .ok()
                    .and_then(|s| s.as_i64().ok())
                    .and_then(|s| u32::try_from(s).ok())
                    .filter(|s| *s > 1);
                out.push(PageLabelRange { start, style, prefix, first });
            }
        }
        if let Ok((_, Object::Array(kids))) = dict.get(b"Kids").and_then(|k| doc.dereference(k)) {
            stack.extend(kids.iter());
        }
    }
    out.sort_by_key(|r| r.start);
    out.dedup_by_key(|r| r.start);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::ErrorCode;

    fn range(start: u16, style: PageLabelStyle, prefix: Option<&str>, first: Option<u32>) -> PageLabelRange {
        PageLabelRange { start, style, prefix: prefix.map(str::to_owned), first }
    }

    #[test]
    fn roman_and_letters_match_pdfium() {
        assert_eq!(roman(1), "i");
        assert_eq!(roman(4), "iv");
        assert_eq!(roman(9), "ix");
        assert_eq!(roman(14), "xiv");
        assert_eq!(roman(1994), "mcmxciv");
        assert_eq!(roman(2026), "mmxxvi");
        assert_eq!(letters(1), "a");
        assert_eq!(letters(26), "z");
        assert_eq!(letters(27), "aa");
        assert_eq!(letters(28), "bb");
        assert_eq!(letters(53), "aaa");
        assert_eq!(format(PageLabelStyle::RomanUpper, Some("p. "), 3), "p. III");
        assert_eq!(format(PageLabelStyle::AlphaUpper, Some("App-"), 2), "App-B");
        assert_eq!(format(PageLabelStyle::NoNumber, Some("Cover"), 9), "Cover");
        assert_eq!(format(PageLabelStyle::NoNumber, None, 1), "");
    }

    #[test]
    fn normalize_sorts_fills_page_zero_and_validates() {
        let out = normalize(
            &[
                range(4, PageLabelStyle::Decimal, Some(""), Some(1)),
                range(2, PageLabelStyle::Roman, None, Some(3)),
            ],
            6,
        )
        .unwrap();
        assert_eq!(out.len(), 3, "a decimal range at page 0 is added");
        assert_eq!(out[0], range(0, PageLabelStyle::Decimal, None, None));
        assert_eq!(out[1], range(2, PageLabelStyle::Roman, None, Some(3)));
        assert_eq!(out[2], range(4, PageLabelStyle::Decimal, None, None));
        assert_eq!(labels_for(&out, 6), vec!["1", "2", "iii", "iv", "1", "2"]);

        let bad = |r: Vec<PageLabelRange>| normalize(&r, 6).unwrap_err().code;
        assert_eq!(bad(vec![range(6, PageLabelStyle::Decimal, None, None)]), ErrorCode::InvalidArgument);
        assert_eq!(
            bad(vec![range(1, PageLabelStyle::Decimal, None, None), range(1, PageLabelStyle::Roman, None, None)]),
            ErrorCode::InvalidArgument
        );
        assert_eq!(bad(vec![range(0, PageLabelStyle::Decimal, None, Some(0))]), ErrorCode::InvalidArgument);
        assert!(normalize(&[], 6).unwrap().is_empty());
    }

    #[test]
    fn labels_roundtrip_through_the_number_tree() {
        let ranges = normalize(
            &[
                range(0, PageLabelStyle::Roman, None, None),
                range(3, PageLabelStyle::Decimal, None, Some(5)),
                range(5, PageLabelStyle::AlphaUpper, Some("부록 "), None),
            ],
            8,
        )
        .unwrap();
        let mut doc = Document::with_version("1.7");
        let catalog = doc.add_object(Dictionary::from_iter(vec![("Type", Object::Name(b"Catalog".to_vec()))]));
        doc.trailer.set("Root", Object::Reference(catalog));
        let mut tree = Dictionary::new();
        let mut nums = Vec::new();
        for r in &ranges {
            let mut label = Dictionary::new();
            if let Some(name) = style_name(r.style) {
                label.set("S", Object::Name(name.to_vec()));
            }
            if let Some(p) = &r.prefix {
                label.set("P", save::pdf_text_string(p));
            }
            if let Some(f) = r.first {
                label.set("St", Object::Integer(f as i64));
            }
            nums.push(Object::Integer(r.start as i64));
            nums.push(Object::Dictionary(label));
        }
        tree.set("Nums", Object::Array(nums));
        doc.catalog_mut().unwrap().set("PageLabels", Object::Dictionary(tree));
        assert_eq!(read_ranges(&doc), ranges);
        assert_eq!(
            labels_for(&ranges, 8),
            vec!["i", "ii", "iii", "5", "6", "부록 A", "부록 B", "부록 C"]
        );
    }
}
