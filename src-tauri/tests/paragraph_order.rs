//! v0.3 pkg1 (R5) — a paragraph edit keeps the text's reading order; rotated text reflows in
//! its own rotated frame. Real PDFium, on PDFs generated here (Helvetica 12 pt, leading 14.4).

mod common;
use common::*;

use seepdf_lib::engine::objects::paragraph;
use seepdf_lib::engine::registry;
use seepdf_lib::engine::render::tiles;
use seepdf_lib::engine::text::layer;
use seepdf_lib::ipc::types::{ParagraphEdit, ParagraphFlow, ParagraphProbe, TextEditStrategy};

/// A one-page 612 × 792 PDF with `content` and F1 = Helvetica.
fn pdf(content: &str) -> Vec<u8> {
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] \
         /Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R >>"
            .to_string(),
        format!(
            "<< /Length {} >>\nstream\n{content}endstream",
            content.len()
        ),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>"
            .to_string(),
    ];
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, body) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{body}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(
        format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes(),
    );
    for off in offsets {
        out.extend_from_slice(format!("{off:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            objects.len() + 1
        )
        .as_bytes(),
    );
    out
}

/// `lines` as one paragraph, first baseline at `(x, y)`, with the text matrix `m` (rotation).
fn para(m: [f32; 4], x: f32, y: f32, lines: &[&str]) -> String {
    let mut s = format!(
        "BT /F1 12 Tf 14.4 TL {} {} {} {} {x} {y} Tm\n",
        m[0], m[1], m[2], m[3]
    );
    for (i, line) in lines.iter().enumerate() {
        if i > 0 {
            s.push_str("T* ");
        }
        s.push_str(&format!("({line}) Tj\n"));
    }
    s.push_str("ET\n");
    s
}

const UPRIGHT: [f32; 4] = [1.0, 0.0, 0.0, 1.0];

fn open_bytes(bytes: Vec<u8>) -> TestDoc {
    let info = with_state(move |st| registry::open(st, None, bytes, None)).expect("open");
    let doc_id = info.doc_id.clone();
    TestDoc { info, doc_id }
}

fn probe(doc_id: &str, x: f32, y: f32) -> ParagraphProbe {
    let doc_id = doc_id.to_string();
    with_doc(&doc_id, move |d| paragraph::probe(d, 0, [x, y]))
        .expect("probe_paragraph")
        .unwrap_or_else(|| panic!("no paragraph at ({x}, {y})"))
}

fn edit(doc_id: &str, p: &ParagraphProbe, text: &str, flow: ParagraphFlow) {
    let doc_id = doc_id.to_string();
    let e = ParagraphEdit {
        object_ids: p.object_ids.clone(),
        text: text.to_string(),
        width: None,
        font_size_pt: None,
        color: None,
        align: None,
        flow: Some(flow),
        dry_run: false,
    };
    let generation = p.doc_generation;
    with_state(move |st| paragraph::edit(st, &doc_id, 0, generation, e, false))
        .expect("edit_paragraph");
}

fn page_text(doc_id: &str) -> String {
    let doc_id = doc_id.to_string();
    with_doc(&doc_id, |d| Ok(layer::page_text(d, 0)?.text.clone())).unwrap()
}

fn save_bytes(doc_id: &str) -> Vec<u8> {
    let doc_id = doc_id.to_string();
    with_state(move |st| {
        tiles::generate_appearances(st, &doc_id)?;
        Ok(st.doc(&doc_id)?.to_bytes()?.to_vec())
    })
    .unwrap()
}

fn render(doc_id: &str) -> Vec<u8> {
    let doc_id = doc_id.to_string();
    with_state(move |st| tiles::render_raw_buffer(st, &doc_id, 0, 1.0, None)).unwrap()
}

fn in_order(text: &str, needles: &[&str]) -> bool {
    let at: Vec<Option<usize>> = needles.iter().map(|n| text.find(n)).collect();
    at.iter().all(Option::is_some) && at.windows(2).all(|w| w[0] < w[1])
}

/// Three paragraphs; paragraph 2 is edited (and grows). The page text reads 1, 2, 3 — in
/// memory and after save + reopen — the page renders the new paragraph where it was, it is
/// one undo step, and no SeePDF marker is left in the file.
#[test]
fn paragraph_edit_keeps_reading_order() {
    let mut content = para(
        UPRIGHT,
        72.0,
        700.0,
        &["Alpha one starts the page and", "alpha two ends it."],
    );
    content.push_str(&para(
        UPRIGHT,
        72.0,
        600.0,
        &["Bravo one is the middle part", "bravo two ends it."],
    ));
    content.push_str(&para(
        UPRIGHT,
        72.0,
        450.0,
        &["Charlie one closes the page", "charlie two ends it."],
    ));
    let doc = open_bytes(pdf(&content));
    let before = page_text(&doc.doc_id);
    assert!(in_order(
        &before,
        &["Alpha one", "Bravo one", "Charlie one"]
    ));

    let p = probe(&doc.doc_id, 100.0, 603.0);
    assert!(p.text.starts_with("Bravo one"), "{:?}", p.text);
    edit(
        &doc.doc_id,
        &p,
        "Delta replaces the middle paragraph with a somewhat longer text that needs one more line.",
        ParagraphFlow::Push,
    );
    let after = page_text(&doc.doc_id);
    assert!(
        in_order(&after, &["Alpha one", "Delta replaces", "Charlie one"]),
        "the edited paragraph stays second: {after:?}"
    );
    assert!(!after.contains("Bravo"));
    let info = with_doc(&doc.doc_id, |d| Ok(d.info())).unwrap();
    assert_eq!(info.undo_label.as_deref(), Some("undo.paragraphEdit"));

    let bytes = save_bytes(&doc.doc_id);
    assert!(
        !bytes
            .windows(12)
            .any(|w| w == b"SeePDFOrderA" || w == b"SeePDFOrderP"),
        "no order marker is left in the file"
    );
    let saved = open_bytes(bytes);
    let text = page_text(&saved.doc_id);
    assert!(
        in_order(&text, &["Alpha one", "Delta replaces", "Charlie one"]),
        "{text:?}"
    );
    assert_eq!(
        render(&saved.doc_id),
        render(&doc.doc_id),
        "the reorder does not move a pixel"
    );

    // One undo brings paragraph 2 back.
    let d = doc.doc_id.clone();
    with_state(move |st| registry::undo(st, &d, false)).expect("undo");
    assert!(in_order(
        &page_text(&doc.doc_id),
        &["Alpha one", "Bravo one", "Charlie one"]
    ));
}

/// A 90° rotated paragraph (reading bottom to top) is probed in its own frame, edited, and
/// the new text is drawn rotated in the same place.
#[test]
fn rotated_paragraph_edits_in_place() {
    let rot = [0.0, 1.0, -1.0, 0.0];
    let mut content = para(UPRIGHT, 72.0, 700.0, &["An upright heading line"]);
    // Baselines at x = 300 and 314.4 (T* moves along the rotated y, i.e. towards +x).
    content.push_str(&para(
        rot,
        300.0,
        200.0,
        &["Rotated one reads upwards and", "rotated two ends it."],
    ));
    let doc = open_bytes(pdf(&content));
    let p = probe(&doc.doc_id, 296.0, 260.0);
    assert_eq!(p.strategy, TextEditStrategy::InPlace, "{p:?}");
    assert!(p.text.starts_with("Rotated one"), "{:?}", p.text);
    assert_eq!(p.lines, 2);

    edit(
        &doc.doc_id,
        &p,
        "Turned text now says something else.",
        ParagraphFlow::Push,
    );
    let saved = open_bytes(save_bytes(&doc.doc_id));
    let text = page_text(&saved.doc_id);
    let flat = text.replace("\r\n", " ");
    assert!(
        flat.contains("Turned text now says something else."),
        "{text:?}"
    );
    assert!(!text.contains("Rotated one"));
    // Drawn rotated, in the old paragraph's place: every new glyph box sits in the band the
    // old text occupied (x around the first baseline, y from 200 upwards).
    let boxes = {
        let doc_id = saved.doc_id.clone();
        with_doc(&doc_id, |d| {
            let l = layer::layer(d, 0)?;
            let chars: Vec<char> = l
                .chars
                .iter()
                .map(|c| char::from_u32(c.codepoint).unwrap_or('?'))
                .collect();
            let text: String = chars.iter().collect();
            let start = text[..text.find("Turned").expect("the new text")]
                .chars()
                .count();
            Ok(l.chars[start..start + 6]
                .iter()
                .map(|c| c.tight)
                .collect::<Vec<_>>())
        })
        .unwrap()
    };
    assert!(!boxes.is_empty());
    for r in &boxes {
        assert!(r.l > 280.0 && r.r < 320.0, "rotated glyph box x {r:?}");
        assert!(r.b >= 195.0 && r.t < 520.0, "rotated glyph box y {r:?}");
        assert!(
            r.t - r.b > r.r - r.l || r.t - r.b > 3.0,
            "glyphs run up the page: {r:?}"
        );
    }
    // The upright heading was not touched.
    assert!(text.contains("An upright heading line"));
}
