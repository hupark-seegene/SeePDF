//! Text layer, the page→device matrix, the binary payload and search.

mod common;

use common::*;
use pdfium_render::prelude::*;
use seepdf_lib::engine::render::geometry;
use seepdf_lib::engine::text::{layer, search, serialize};
use seepdf_lib::engine::Lane;

/// The numbers the text spike measured on `tracemonkey.pdf` page 1 (0-based page 0):
/// 5,087 chars — 792 of them pdfium-generated whitespace — grouped into 754 words and
/// 89 lines.
#[test]
fn text_layer_tracemonkey() {
    let doc = open("tracemonkey.pdf");
    let (chars, words, lines, generated, first_line, has_ids) =
        with_doc(&doc.doc_id, |d| {
            let l = layer::layer(d, 0)?;
            let first = l.lines[0];
            let text: String = l.chars
                [first.first_char as usize..(first.first_char + first.char_count) as usize]
                .iter()
                .map(|c| char::from_u32(c.codepoint).unwrap_or('?'))
                .collect();
            Ok((
                l.chars.len(),
                l.words.len(),
                l.lines.len(),
                l.chars.iter().filter(|c| c.is_generated()).count(),
                text,
                l.has_object_ids,
            ))
        })
        .expect("build the text layer");

    assert_eq!(chars, 5087, "char count");
    assert_eq!(words, 754, "word count");
    assert_eq!(lines, 89, "line count");
    assert_eq!(generated, 792, "pdfium-generated whitespace");
    assert!(
        first_line.starts_with("Trace-based Just-in-Time"),
        "line 0 was {first_line:?}"
    );
    assert!(has_ids, "per-char object ids are present");

    // The layer is cached per generation: a second call must not rebuild it.
    let same = with_doc(&doc.doc_id, |d| {
        let a = layer::layer(d, 0)?;
        let b = layer::layer(d, 0)?;
        Ok(std::sync::Arc::ptr_eq(&a, &b))
    })
    .expect("cached");
    assert!(same);
}

/// Our page→device matrix must agree with `FPDF_PageToDevice` within 1 px, for an unrotated
/// page and for a `/Rotate 90` one (text spike §2). Both sides of the IPC implement this
/// function; this is the test that keeps them honest.
#[test]
fn text_matrix_vs_pdfium() {
    let doc = open("rotation.pdf");
    let doc_id = doc.doc_id.clone();
    let geoms = doc.info.pages.clone();

    let diffs = engine()
        .call_blocking(Lane::Interactive, "test/matrix", move |st| {
            let mut worst: f32 = 0.0;
            let mut samples = 0;
            for page_index in [0u16, 1] {
                let geom = geoms[page_index as usize].clone();
                let scale = 2.0f32;
                let matrix = geometry::page_to_device(&geom, 0, scale);
                let config = PdfRenderConfig::new().scale_page_by_factor(scale);
                let d = st.doc_mut(&doc_id)?;
                let text_layer = layer::layer(d, page_index)?;
                let page = d.page(page_index)?;
                for char_entry in text_layer.chars.iter().filter(|c| !c.is_generated()).take(40) {
                    let (x, y) = (char_entry.loose.l, char_entry.baseline_y);
                    let mine = (
                        matrix[0] * x + matrix[2] * y + matrix[4],
                        matrix[1] * x + matrix[3] * y + matrix[5],
                    );
                    let theirs = page
                        .points_to_pixels(PdfPoints::new(x), PdfPoints::new(y), &config)
                        .map_err(|e| {
                            seepdf_lib::ipc::EngineError::pdfium("points_to_pixels", e)
                        })?;
                    worst = worst
                        .max((mine.0 - theirs.0 as f32).abs())
                        .max((mine.1 - theirs.1 as f32).abs());
                    samples += 1;
                }
            }
            Ok((worst, samples))
        })
        .expect("matrix comparison");

    assert!(diffs.1 >= 40, "only {} samples compared", diffs.1);
    assert!(
        diffs.0 <= 1.0,
        "worst disagreement with FPDF_PageToDevice was {} px",
        diffs.0
    );
}

/// The §10.1 binary layout round-trips: header, section offsets and the page text.
#[test]
fn text_binary_layer_roundtrip() {
    let doc = open("tracemonkey.pdf");
    let (buffer, expected_text) = with_doc(&doc.doc_id, |d| {
        let l = layer::layer(d, 0)?;
        Ok((serialize::serialize(&l), l.text.clone()))
    })
    .expect("serialize");

    let header = serialize::parse_header(&buffer).expect("STXL header");
    assert_eq!(header.version, 1);
    assert_eq!(header.flags & serialize::FLAG_OBJECT_IDS, 1);
    assert_eq!(header.page, 0);
    assert_eq!(header.char_count, 5087);
    assert_eq!(header.word_count, 754);
    assert_eq!(header.line_count, 89);

    let (chars_at, words_at, lines_at, text_at) = serialize::section_offsets(&header);
    assert_eq!(chars_at, 64);
    assert_eq!(words_at, 64 + 5087 * 32);
    assert_eq!(lines_at, words_at + 754 * 12);
    assert!(buffer.len() >= text_at + 4 + expected_text.len());
    assert_eq!(serialize::parse_text(&buffer).as_deref(), Some(expected_text.as_str()));

    // First char: 'T' of "Trace-based", loose box from the spike.
    let f32_at = |o: usize| f32::from_le_bytes(buffer[o..o + 4].try_into().unwrap());
    let codepoint = u32::from_le_bytes(buffer[64..68].try_into().unwrap());
    assert_eq!(char::from_u32(codepoint), Some('T'));
    assert!((f32_at(68) - 80.52).abs() < 0.1, "loose.l was {}", f32_at(68));
    assert!((f32_at(80) - 713.04).abs() < 0.1, "loose.t was {}", f32_at(80));
}

/// Our Rust search must return exactly the hit counts PDFium's own `FPDFText_FindStart`
/// finds (62 / 1 / 4 for "monkey" with default / match-case / whole-word), but with char
/// indices, which pdfium's search does not expose (text spike §3).
#[test]
fn search_counts_match_pdfium() {
    let doc = open("tracemonkey.pdf");
    let doc_id = doc.doc_id.clone();
    let page_count = doc.info.page_count;

    for (match_case, whole_word, expected) in
        [(false, false, 62usize), (true, false, 1), (false, true, 4)]
    {
        let doc_id = doc_id.clone();
        let (ours, theirs) = engine()
            .call_blocking(Lane::Background, "test/search", move |st| {
                let query = search::Query::new("monkey", match_case, whole_word)
                    .expect("non-empty query");
                let mut ours = 0usize;
                let mut theirs = 0usize;
                for page in 0..page_count {
                    let d = st.doc_mut(&doc_id)?;
                    ours += search::search_page(d, page, &query)?.len();

                    let d = st.doc_mut(&doc_id)?;
                    let pdf_page = d.page(page)?;
                    let text = pdf_page
                        .text()
                        .map_err(|e| seepdf_lib::ipc::EngineError::pdfium("text()", e))?;
                    let options = PdfSearchOptions::new()
                        .match_case(match_case)
                        .match_whole_word(whole_word);
                    let find = text
                        .search("monkey", &options)
                        .map_err(|e| seepdf_lib::ipc::EngineError::pdfium("search", e))?;
                    while find.find_next().is_some() {
                        theirs += 1;
                    }
                }
                Ok((ours, theirs))
            })
            .expect("search");

        assert_eq!(
            theirs, expected,
            "pdfium itself changed: match_case={match_case} whole_word={whole_word}"
        );
        assert_eq!(
            ours, expected,
            "our search disagrees with pdfium: match_case={match_case} whole_word={whole_word}"
        );
    }
}

/// Every hit carries a usable char index, line rects and ±40 chars of context.
#[test]
fn search_hits_carry_indices_and_rects() {
    let doc = open("tracemonkey.pdf");
    let hits = with_doc(&doc.doc_id, |d| {
        let query = search::Query::new("trace", false, false).expect("query");
        search::search_page(d, 0, &query)
    })
    .expect("search page 0");

    assert!(!hits.is_empty());
    for hit in &hits {
        assert_eq!(hit.char_length, 5);
        assert!(!hit.rects.is_empty(), "a hit without rects cannot be drawn");
        for rect in &hit.rects {
            assert!(rect.width() > 0.0 && rect.height() > 0.0);
        }
        let matched: String = hit
            .context
            .chars()
            .skip(hit.context_match[0] as usize)
            .take(hit.context_match[1] as usize)
            .collect();
        assert_eq!(matched.to_lowercase(), "trace");
    }
}

/// `get_page_text` is the cheap path: code points only, cached for the document's lifetime.
#[test]
fn text_page_text_matches_the_layer() {
    let doc = open("tracemonkey.pdf");
    let equal = with_doc(&doc.doc_id, |d| {
        let cheap = layer::page_text(d, 0)?;
        let full = layer::layer(d, 0)?;
        Ok(cheap.text == full.text && cheap.chars.len() == full.chars.len())
    })
    .expect("page text");
    assert!(equal, "the two caches must describe the same characters");
}
