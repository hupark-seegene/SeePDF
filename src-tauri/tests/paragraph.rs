//! Stage 7 — paragraph probe + reflowing edit (`IPC_CONTRACT.md` §7.4b), real PDFium.
//!
//! Most tests run on a PDF generated here with known lines (Helvetica 12 pt, leading 14.4),
//! so every expectation is exact; one runs on tracemonkey's body text (embedded Type1 subset
//! fonts without a space glyph). Text assertions go through save → reopen → extract.

mod common;
use common::*;

use seepdf_lib::engine::objects::paragraph;
use seepdf_lib::engine::registry;
use seepdf_lib::engine::save;
use seepdf_lib::engine::text::layer;
use seepdf_lib::ipc::types::{
    NotEditableReason, ParagraphAlign, ParagraphEdit, ParagraphEditResult, ParagraphProbe,
    TextEditStrategy,
};
use seepdf_lib::ipc::{EngineError, ErrorCode};

// ---------------------------------------------------------------------------------------
// The generated fixture
// ---------------------------------------------------------------------------------------

const PARA_A: [&str; 4] = [
    "The quick brown fox jumps over the lazy dog and keeps",
    "running across the wide green field until the sun com-",
    "pletely sets behind the distant hills and it gets quiet",
    "at last.",
];
const PARA_A_TEXT: &str = "The quick brown fox jumps over the lazy dog and keeps running across \
    the wide green field until the sun completely sets behind the distant hills and it gets \
    quiet at last.";
const PARA_C_TEXT: &str = "Second paragraph starts here with enough words to wrap and ends here.";

/// A one-page PDF: an 18 pt heading, paragraph A (4 lines at 700 → 656.8, one object per
/// line, a hyphenated break), paragraph C (2 lines at 620, its first line two objects) and a
/// line of text rotated 90°.
fn generated_pdf() -> Vec<u8> {
    let mut content = String::new();
    content.push_str("BT /F2 18 Tf 72 740 Td (A Heading Line) Tj ET\n");
    content.push_str("BT /F1 12 Tf 14.4 TL 72 700 Td\n");
    for (i, line) in PARA_A.iter().enumerate() {
        if i > 0 {
            content.push_str("T* ");
        }
        content.push_str(&format!("({line}) Tj\n"));
    }
    content.push_str("ET\n");
    content.push_str("BT /F1 12 Tf 72 620 Td (Second paragraph) Tj ET\n");
    content.push_str("BT /F1 12 Tf 175 620 Td (starts here with enough words to wrap) Tj ET\n");
    content.push_str("BT /F1 12 Tf 72 605.6 Td (and ends here.) Tj ET\n");
    content.push_str("BT /F1 12 Tf 0 1 -1 0 560 300 Tm (Rotated words here) Tj ET\n");

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
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica-Bold /Encoding /WinAnsiEncoding >>"
            .to_string(),
    ];
    let mut pdf = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, body) in objects.iter().enumerate() {
        offsets.push(pdf.len());
        pdf.extend_from_slice(format!("{} 0 obj\n{body}\nendobj\n", i + 1).as_bytes());
    }
    let xref = pdf.len();
    pdf.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
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

fn open_bytes(bytes: Vec<u8>) -> TestDoc {
    let info = with_state(move |st| registry::open(st, None, bytes, None)).expect("open bytes");
    let doc_id = info.doc_id.clone();
    TestDoc { info, doc_id }
}

fn open_generated() -> TestDoc {
    open_bytes(generated_pdf())
}

// ---------------------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------------------

fn probe(doc_id: &str, page: u16, x: f32, y: f32) -> Option<ParagraphProbe> {
    let doc_id = doc_id.to_string();
    with_doc(&doc_id, move |d| paragraph::probe(d, page, [x, y])).expect("probe_paragraph")
}

fn generation(doc_id: &str) -> u32 {
    with_doc(doc_id, |d| Ok(d.generation)).unwrap()
}

fn edit_with(
    doc_id: &str,
    page: u16,
    expect: u32,
    edit: ParagraphEdit,
    allow: bool,
) -> Result<ParagraphEditResult, EngineError> {
    let doc_id = doc_id.to_string();
    with_state(move |st| paragraph::edit(st, &doc_id, page, expect, edit, allow))
}

fn edit_of(p: &ParagraphProbe, text: &str) -> ParagraphEdit {
    ParagraphEdit {
        object_ids: p.object_ids.clone(),
        text: text.to_string(),
        width: None,
        font_size_pt: None,
        color: None,
        align: None,
    }
}

fn norm(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Whitespace-normalised text of `page` after save + reopen.
fn saved_text(doc_id: &str, page: u16) -> String {
    let id = doc_id.to_string();
    let bytes = with_state(move |st| save::serialize(st, &id)).expect("serialize");
    let reopened = open_bytes(bytes);
    let id = reopened.doc_id.clone();
    let text = with_doc(&id, move |d| Ok(layer::page_text(d, page)?.text.clone())).unwrap();
    norm(&text)
}

fn page_text(doc_id: &str, page: u16) -> String {
    let id = doc_id.to_string();
    norm(&with_doc(&id, move |d| Ok(layer::page_text(d, page)?.text.clone())).unwrap())
}

// ---------------------------------------------------------------------------------------
// Detection
// ---------------------------------------------------------------------------------------

#[test]
fn paragraph_detects_generated_paragraphs() {
    let doc = open_generated();

    // Middle of paragraph A's second line.
    let a = probe(&doc.doc_id, 0, 200.0, 690.0).expect("text there");
    assert_eq!(a.lines, 4, "{a:?}");
    assert_eq!(a.object_ids.len(), 4, "one object per line");
    assert_eq!(a.text, PARA_A_TEXT, "soft wraps joined, com-/pletely de-hyphenated");
    assert_eq!(a.font_name, "Helvetica");
    assert!((a.font_size_pt - 12.0).abs() < 0.01);
    assert!((a.line_height_pt - 14.4).abs() < 0.05, "{}", a.line_height_pt);
    assert_eq!(a.align, ParagraphAlign::Left);
    assert!(a.first_line_indent_pt.abs() < 0.01);
    assert_eq!(a.color, [0, 0, 0]);
    assert!(!a.mixed_styles);
    assert_eq!(a.strategy, TextEditStrategy::InPlace);
    assert!(a.substitute_font.is_none() && a.reason.is_none());
    assert!(a.rect.l >= 71.0 && a.rect.l <= 73.0 && a.rect.t < 715.0 && a.rect.b > 650.0);
    assert_eq!(a.doc_generation, generation(&doc.doc_id));

    // The same paragraph from its last (short) line.
    let a_last = probe(&doc.doc_id, 0, 80.0, 660.0).expect("text there");
    assert_eq!(a_last.object_ids, a.object_ids);

    // Paragraph C: separate (blank gap), its split first line joined with a space.
    let c = probe(&doc.doc_id, 0, 100.0, 623.0).expect("text there");
    assert_eq!(c.lines, 2);
    assert_eq!(c.object_ids.len(), 3);
    assert_eq!(c.text, PARA_C_TEXT);

    // The heading is its own paragraph (different size).
    let h = probe(&doc.doc_id, 0, 100.0, 745.0).expect("heading");
    assert_eq!(h.lines, 1);
    assert_eq!(h.text, "A Heading Line");
    assert!((h.font_size_pt - 18.0).abs() < 0.01);

    // Empty area → null.
    assert!(probe(&doc.doc_id, 0, 300.0, 200.0).is_none());

    // Rotated text → refused.
    let r = probe(&doc.doc_id, 0, 555.0, 350.0).expect("rotated text is text");
    assert_eq!(r.strategy, TextEditStrategy::Refused);
    assert_eq!(r.reason, Some(NotEditableReason::RotatedText));

    // The probe never mutates.
    assert_eq!(generation(&doc.doc_id), a.doc_generation);
    assert!(!with_doc(&doc.doc_id, |d| Ok(d.dirty())).unwrap());
}

/// tracemonkey's body is LaTeX: embedded Type1 subsets without a space glyph, words as
/// separate runs, paragraphs separated by a first-line indent only.
#[test]
fn paragraph_detects_tracemonkey_body_text() {
    let doc = open("tracemonkey.pdf");
    // Left column, abstract body.
    let p = probe(&doc.doc_id, 0, 150.0, 380.0).expect("text in the left column");
    assert!(p.lines >= 3, "{p:?}");
    assert!(p.text.split_whitespace().count() > 20, "{p:?}");
    assert!(p.rect.r < 320.0, "stays in the left column: {:?}", p.rect);
    assert_ne!(p.strategy, TextEditStrategy::Refused, "{p:?}");
    assert!(!p.text.contains('\n'));

    // Reflow the same text in place: the subset draws every glyph (no space → one object
    // per word), the paragraph keeps its line count and nothing is lost.
    let before = page_text(&doc.doc_id, 0);
    let generation = p.doc_generation;
    let text = p.text.clone();
    let result = edit_with(&doc.doc_id, 0, generation, edit_of(&p, &text), false)
        .expect("in-place reflow of the original text");
    assert!(result.lines.abs_diff(p.lines) <= 1, "{} vs {}", result.lines, p.lines);
    let after = saved_text(&doc.doc_id, 0);
    let first_words: String = text.split_whitespace().take(6).collect::<Vec<_>>().join(" ");
    assert!(after.contains(&first_words), "{first_words:?} in {after:?}");
    assert!(before.len().abs_diff(after.len()) < 40);
}

// ---------------------------------------------------------------------------------------
// Editing
// ---------------------------------------------------------------------------------------

#[test]
fn paragraph_edit_shorter_and_longer() {
    let doc = open_generated();
    let a = probe(&doc.doc_id, 0, 200.0, 690.0).unwrap();
    let c_before = probe(&doc.doc_id, 0, 100.0, 623.0).unwrap();

    // Shorter: one line, no overflow, the rest of the page untouched.
    let short = edit_with(&doc.doc_id, 0, a.doc_generation, edit_of(&a, "Short text now."), false)
        .expect("edit shorter");
    assert_eq!(short.lines, 1);
    assert_eq!(short.overflow_pt, 0.0);
    assert!(short.rect.t <= a.rect.t + 1.0 && (short.rect.l - a.rect.l).abs() < 1.5);
    let text = saved_text(&doc.doc_id, 0);
    assert!(text.contains("Short text now."), "{text}");
    assert!(!text.contains("quick brown"), "{text}");
    assert!(text.contains("A Heading Line") && text.contains("Second paragraph"));
    let c_after = probe(&doc.doc_id, 0, 100.0, 623.0).unwrap();
    assert_eq!(c_after.text, c_before.text);
    assert_eq!(c_after.rect, c_before.rect, "nothing else on the page moves");

    // Longer: more lines, overflow below the original rect, text extractable in order.
    let a2 = probe(&doc.doc_id, 0, 80.0, 705.0).unwrap();
    assert_eq!(a2.text, "Short text now.");
    let long = format!("{PARA_A_TEXT} {PARA_A_TEXT}");
    // Wide enough that the longer paragraph stays clear of paragraph C (overlapping text is
    // exactly the overflow the UI warns about, and re-detecting it is ambiguous).
    let mut e = edit_of(&a2, &long);
    e.width = Some(460.0);
    let result = edit_with(&doc.doc_id, 0, a2.doc_generation, e, false).expect("edit longer");
    assert!(result.lines >= 4 && result.lines > a2.lines, "{}", result.lines);
    assert!(result.rect.b > 630.0, "clear of paragraph C: {:?}", result.rect);
    assert!(result.overflow_pt > 0.0);
    let text = saved_text(&doc.doc_id, 0);
    assert!(text.contains(&norm(&long)), "in order: {text}");
    let again = probe(&doc.doc_id, 0, 80.0, 705.0).unwrap();
    assert_eq!(again.text, long);
    assert_eq!(again.lines, result.lines);
    assert!((again.line_height_pt - 14.4).abs() < 0.05);
}

#[test]
fn paragraph_edit_korean_needs_consent() {
    let doc = open_generated();
    let a = probe(&doc.doc_id, 0, 200.0, 690.0).unwrap();
    let korean = "안녕하세요 반갑습니다. 한글 문단 편집을 시험합니다. 줄바꿈은 띄어쓰기에서 일어나야 합니다.";

    let refused = edit_with(&doc.doc_id, 0, a.doc_generation, edit_of(&a, korean), false)
        .expect_err("Helvetica cannot draw Hangul");
    assert_eq!(refused.code, ErrorCode::FontCoverage);
    assert_eq!(generation(&doc.doc_id), a.doc_generation, "a refused edit changes nothing");

    let done = edit_with(&doc.doc_id, 0, a.doc_generation, edit_of(&a, korean), true)
        .expect("with consent");
    assert!(done.lines >= 1);
    let fonts: Vec<String> = done
        .objects
        .objects
        .iter()
        .filter_map(|o| o.font_name.clone())
        .collect();
    assert!(fonts.iter().any(|f| f.contains("SeePDF")), "{fonts:?}");
    let text = saved_text(&doc.doc_id, 0);
    assert!(text.contains("안녕하세요 반갑습니다."), "{text}");

    // Probing the new paragraph: its own (bundled) font covers it.
    let p = probe(&doc.doc_id, 0, 80.0, 705.0).unwrap();
    assert_eq!(p.text, korean);
    assert_eq!(p.strategy, TextEditStrategy::InPlace);

    // The probe of Helvetica text reports what a Korean edit would need.
    let c = probe(&doc.doc_id, 0, 100.0, 623.0).unwrap();
    assert_eq!(c.strategy, TextEditStrategy::InPlace);
}

#[test]
fn paragraph_edit_alignments() {
    let doc = open_generated();
    let text = format!("{PARA_A_TEXT} And a little more text so there are enough lines.");
    for align in [ParagraphAlign::Justify, ParagraphAlign::Center, ParagraphAlign::Right] {
        let p = probe(&doc.doc_id, 0, 80.0, 705.0).unwrap();
        let mut e = edit_of(&p, &text);
        e.align = Some(align);
        e.width = Some(300.0);
        let result = edit_with(&doc.doc_id, 0, p.doc_generation, e, false)
            .unwrap_or_else(|err| panic!("{align:?}: {err}"));
        assert!(result.lines >= 3, "{align:?}: {}", result.lines);
        assert!(result.rect.r <= p.rect.l + 300.0 + 2.0, "{align:?}: {:?}", result.rect);
        if align == ParagraphAlign::Right {
            // Every new line ends at the box's right edge (ink within 2 pt of the advance).
            let box_r = p.rect.l + 300.0;
            let new_lines: Vec<_> = result
                .objects
                .objects
                .iter()
                .filter(|o| o.text.is_some() && o.rect.intersects(&result.rect))
                .collect();
            assert_eq!(new_lines.len() as u32, result.lines);
            for o in new_lines {
                assert!((o.rect.r - box_r).abs() < 2.0, "{:?} vs {box_r}", o.rect);
            }
            continue;
        }
        let again = probe(&doc.doc_id, 0, 80.0, 705.0)
            .or_else(|| probe(&doc.doc_id, 0, 300.0, 705.0))
            .expect("the paragraph is still there");
        assert_eq!(again.align, align, "{again:?}");
        assert_eq!(again.text, text);
        assert_eq!(again.lines, result.lines);
    }
}

#[test]
fn paragraph_edit_undo_and_stale() {
    let doc = open_generated();
    let a = probe(&doc.doc_id, 0, 200.0, 690.0).unwrap();

    let stale = edit_with(&doc.doc_id, 0, a.doc_generation + 1, edit_of(&a, "x"), false)
        .expect_err("wrong generation");
    assert_eq!(stale.code, ErrorCode::Stale);

    let mut e = edit_of(&a, "Replaced paragraph text.");
    e.font_size_pt = Some(15.0);
    e.color = Some([200, 0, 0]);
    edit_with(&doc.doc_id, 0, a.doc_generation, e, false).expect("edit");
    let label = with_doc(&doc.doc_id, |d| Ok(d.history.undo_label())).unwrap();
    assert_eq!(label.as_deref(), Some("undo.paragraphEdit"));
    let p = probe(&doc.doc_id, 0, 80.0, 705.0).unwrap();
    assert_eq!(p.text, "Replaced paragraph text.");
    assert!((p.font_size_pt - 15.0).abs() < 0.01);
    assert_eq!(p.color, [200, 0, 0]);

    let id = doc.doc_id.clone();
    with_state(move |st| registry::undo(st, &id, false)).expect("undo");
    let back = probe(&doc.doc_id, 0, 200.0, 690.0).unwrap();
    assert_eq!(back.text, PARA_A_TEXT, "one undo step restores the paragraph");
    assert_eq!(back.object_ids, a.object_ids);
}
