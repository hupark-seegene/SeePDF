//! v0.3.1 real-app QA: paragraphs shaped the way Word / LibreOffice write them, which the user's
//! grouped Korean spec reaches once 그룹 해제 has made its text editable.
//!
//! - A red line above a black one (paragraph spacing, not line spacing, between them) is two
//!   paragraphs: merged, an edit of the red line turned it black and slid the strikethrough of the
//!   black line under other words.
//! - The cells of one table column (row borders between them) are not a paragraph: merged, an
//!   edit reflowed the whole column across the rows.
//! - `, ` `: ` or a Hangul syllable written as a run of its own right after a word read back
//!   without a stray space ("저장되며 , 접근" and "2026 년" were written back into the page).

mod common;
use common::*;

use seepdf_lib::engine::objects::paragraph;
use seepdf_lib::engine::registry;
use seepdf_lib::ipc::types::ParagraphProbe;

fn pdf_with(content: &str) -> Vec<u8> {
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] \
         /Resources << /Font << /F1 5 0 R /F2 6 0 R >> >> /Contents 4 0 R >>"
            .to_string(),
        format!(
            "<< /Length {} >>\nstream\n{content}endstream",
            content.len()
        ),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>"
            .to_string(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Times-Roman /Encoding /WinAnsiEncoding >>"
            .to_string(),
    ];
    let mut pdf = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, body) in objects.iter().enumerate() {
        offsets.push(pdf.len());
        pdf.extend_from_slice(format!("{} 0 obj\n{body}\nendobj\n", i + 1).as_bytes());
    }
    let xref = pdf.len();
    pdf.extend_from_slice(
        format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes(),
    );
    for off in offsets {
        pdf.extend_from_slice(format!("{off:010} 00000 n \n").as_bytes());
    }
    pdf.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            objects.len() + 1
        )
        .as_bytes(),
    );
    pdf
}

fn open_content(content: &str) -> TestDoc {
    let bytes = pdf_with(content);
    let info = with_state(move |st| registry::open(st, None, bytes, None)).expect("open bytes");
    let doc_id = info.doc_id.clone();
    TestDoc { info, doc_id }
}

fn probe(doc: &TestDoc, x: f32, y: f32) -> ParagraphProbe {
    with_doc(&doc.doc_id, move |d| paragraph::probe(d, 0, [x, y]))
        .expect("probe_paragraph")
        .unwrap_or_else(|| panic!("no paragraph at ({x}, {y})"))
}

/// Word's "변경" line in red, then a black line struck through, 21 pt apart (a paragraph's
/// spacing after; the lines of each paragraph would be 13.2 pt apart).
#[test]
fn a_red_line_and_a_black_line_are_two_paragraphs() {
    let doc = open_content(
        "BT /F1 11 Tf 1 0 0 rg 72 700 Td (Changed: the upload limit rises to 5 GB) Tj ET\n\
         BT /F1 11 Tf 0 0 0 rg 72 679 Td (Removed: a text message after each upload) Tj ET\n\
         0 0 0 RG 0.8 w 72 682.5 m 290 682.5 l S\n",
    );
    let red = probe(&doc, 100.0, 703.0);
    assert_eq!(red.text, "Changed: the upload limit rises to 5 GB");
    assert_eq!(red.color, [255, 0, 0]);
    assert_eq!(red.lines, 1);
    let black = probe(&doc, 100.0, 682.0);
    assert_eq!(black.text, "Removed: a text message after each upload");
    assert_eq!(black.color, [0, 0, 0]);
}

/// A table column: three cells 17 pt apart, a 0.2 pt row border between each two (a thin filled
/// rectangle, as LibreOffice draws it), and a red word inside a cell (a mixed line joins nothing
/// by colour).
#[test]
fn the_cells_of_a_table_column_are_not_one_paragraph() {
    let mut content = String::new();
    for (i, cell) in [
        "(Scan every upload for viruses) Tj",
        "(Keep the upload log for 90 days ) Tj 1 0 0 rg (\\(180 days\\)) Tj",
        "(Admins can delete any file) Tj",
    ]
    .iter()
    .enumerate()
    {
        let y = 600.0 - 17.0 * i as f32;
        content.push_str(&format!("BT /F1 9.5 Tf 0 0 0 rg 134 {y} Td {cell} ET\n"));
        // the row border under this row: 5.3 pt under the baseline
        content.push_str(&format!("0 g 56 {} 511 0.2 re f\n", y - 5.3));
    }
    let doc = open_content(&content);
    let middle = probe(&doc, 150.0, 586.0);
    assert_eq!(middle.text, "Keep the upload log for 90 days (180 days)");
    assert_eq!(middle.lines, 1);
    let first = probe(&doc, 150.0, 603.0);
    assert_eq!(first.text, "Scan every upload for viruses");
    let last = probe(&doc, 150.0, 569.0);
    assert_eq!(last.text, "Admins can delete any file");
}

/// Rules that are not row borders keep a paragraph together: an underline under a line, and a
/// strikethrough through the next one.
#[test]
fn underlines_and_strikethroughs_do_not_split_a_paragraph() {
    let doc = open_content(
        "BT /F1 11 Tf 13.2 TL 0 0 0 rg 72 700 Td (The first line of one paragraph is underlined) Tj \
         T* (and its second line is struck through all the way) Tj \
         T* (and the third line is plain.) Tj ET\n\
         0 0 0 RG 0.6 w 72 698.6 m 300 698.6 l S\n\
         0 0 0 RG 0.6 w 72 690.3 m 320 690.3 l S\n",
    );
    let p = probe(&doc, 100.0, 703.0);
    assert_eq!(p.lines, 3, "{}", p.text);
    assert_eq!(
        p.text,
        "The first line of one paragraph is underlined and its second line is struck through all \
         the way and the third line is plain."
    );
}

/// `, ` after a word, written as a run of its own (another font) just after the word's advance:
/// no word space before it. A real space written as a gap still reads as one.
#[test]
fn no_stray_space_before_punctuation_in_a_run_of_its_own() {
    // "Stored" in Helvetica 11 is 32.406 pt wide; the comma run starts 0.85 pt after it
    // (LibreOffice's spacing), the next word one space (3.06 pt) after the comma's advance.
    let doc = open_content(
        "BT /F1 11 Tf 0 0 0 rg 72 700 Td (Stored) Tj ET\n\
         BT /F2 11 Tf 0 0 0 rg 105.256 700 Td (,) Tj ET\n\
         BT /F1 11 Tf 0 0 0 rg 111.066 700 Td (access by department) Tj ET\n\
         BT /F1 11 Tf 0 0 0 rg 72 650 Td (Stored) Tj ET\n\
         BT /F2 11 Tf 0 0 0 rg 105.256 650 Td (:) Tj ET\n\
         BT /F1 11 Tf 0 0 0 rg 111.066 650 Td (plain text) Tj ET\n",
    );
    assert_eq!(
        probe(&doc, 80.0, 703.0).text,
        "Stored, access by department"
    );
    assert_eq!(probe(&doc, 80.0, 653.0).text, "Stored: plain text");
}

/// A word, then a run of another font right after its advance whose first glyph has a wide left
/// side bearing (Times' "1", like the Hangul "년" after "2026" in the user's document): one word.
/// A run that starts with an offset as wide as a space (a leading `TJ` number) is a new word.
#[test]
fn a_side_bearing_is_not_a_word_space() {
    // "Page" in Helvetica 11 is 25.685 pt wide; the Times run starts 0.46 pt after it.
    let doc = open_content(
        "BT /F1 11 Tf 0 0 0 rg 72 700 Td (Page) Tj ET\n\
         BT /F2 11 Tf 0 0 0 rg 98.145 700 Td (10 of the manual) Tj ET\n\
         BT /F1 11 Tf 0 0 0 rg 72 650 Td (Page) Tj ET\n\
         BT /F2 11 Tf 0 0 0 rg 98.145 650 Td [-300 (10 of the guide)] TJ ET\n",
    );
    assert_eq!(probe(&doc, 80.0, 703.0).text, "Page10 of the manual");
    assert_eq!(probe(&doc, 80.0, 653.0).text, "Page 10 of the guide");
}
