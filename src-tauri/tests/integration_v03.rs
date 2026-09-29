//! v0.3 integration — behaviours that only exist once two work packages meet.
//!
//! pkg1 (R2) splits a partly redacted text run into pieces, re-emitting later pieces as new
//! objects; pkg6 (V6) reads a tagged page in structure-tree order through each character's
//! `/MCID`. The pieces must keep their marked-content id, or the surviving words of a
//! redacted paragraph would drop to "untagged content, read last" in read aloud and the
//! screen-reader region.

mod common;

use common::*;
use seepdf_lib::engine::redact;
use seepdf_lib::engine::registry;
use seepdf_lib::engine::render::tiles;
use seepdf_lib::engine::text::{layer, structtree};
use seepdf_lib::ipc::types::{PageIndex, ReadingOrder, Rect, RedactBatchMark, RedactOptions};

/// A minimal PDF from `(number, body)` objects with a correct xref table; object 1 is the
/// catalog.
fn pdf(objects: &[(u32, String)]) -> Vec<u8> {
    let mut out: Vec<u8> = b"%PDF-1.7\n%\xE2\xE3\xCF\xD3\n".to_vec();
    let mut offsets = vec![0usize; objects.len() + 1];
    for (number, body) in objects {
        offsets[*number as usize] = out.len();
        out.extend_from_slice(format!("{number} 0 obj\n{body}\nendobj\n").as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(
        format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes(),
    );
    for offset in offsets.iter().skip(1) {
        out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
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

/// pkg6's tagged fixture: "Second paragraph on top" is drawn first, at the top (MCID 1);
/// "First paragraph below here" second, lower down (MCID 0). The tree reads MCID 0, then 1.
fn tagged_pdf() -> Vec<u8> {
    let content =
        "/P << /MCID 1 >> BDC BT /F1 18 Tf 72 700 Td (Second paragraph on top) Tj ET EMC\n\
         /P << /MCID 0 >> BDC BT /F1 18 Tf 72 400 Td (First paragraph below here) Tj ET EMC";
    let stream = format!(
        "<< /Length {} >>\nstream\n{content}\nendstream",
        content.len()
    );
    pdf(&[
        (
            1,
            "<< /Type /Catalog /Pages 2 0 R /StructTreeRoot 5 0 R /MarkInfo << /Marked true >> >>"
                .into(),
        ),
        (2, "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into()),
        (
            3,
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] \
             /Resources << /Font << /F1 4 0 R >> >> /Contents 9 0 R /StructParents 0 >>"
                .into(),
        ),
        (
            4,
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>"
                .into(),
        ),
        (
            5,
            "<< /Type /StructTreeRoot /K [6 0 R] /ParentTree 10 0 R /ParentTreeNextKey 1 >>".into(),
        ),
        (
            6,
            "<< /Type /StructElem /S /Document /P 5 0 R /K [7 0 R 8 0 R] >>".into(),
        ),
        (
            7,
            "<< /Type /StructElem /S /P /P 6 0 R /Pg 3 0 R /K 0 >>".into(),
        ),
        (
            8,
            "<< /Type /StructElem /S /P /P 6 0 R /Pg 3 0 R /K 1 >>".into(),
        ),
        (9, stream),
        (10, "<< /Nums [0 [7 0 R 8 0 R]] >>".into()),
    ])
}

fn open_bytes(bytes: Vec<u8>) -> String {
    with_state(move |st| registry::open(st, None, bytes, None))
        .expect("open generated pdf")
        .doc_id
}

fn close(doc_id: String) {
    let _ = with_state(move |st| registry::close(st, &doc_id));
}

/// The tight box of the first occurrence of `needle` on page 0, inflated by half a point.
fn word_rect(doc_id: &str, needle: &str) -> Rect {
    let needle = needle.to_string();
    with_doc(doc_id, move |d| {
        let layer = layer::layer(d, 0)?;
        let text: String = layer
            .chars
            .iter()
            .map(|c| char::from_u32(c.codepoint).unwrap_or('\u{fffd}'))
            .collect();
        let at = text.find(&needle).expect("needle on the page");
        let start = text[..at].chars().count();
        let rect = layer
            .chars
            .iter()
            .skip(start)
            .take(needle.chars().count())
            .map(|c| c.tight)
            .reduce(|a, b| a.union(&b))
            .expect("chars");
        Ok(Rect::new(
            rect.l - 0.5,
            rect.b - 0.5,
            rect.r + 0.5,
            rect.t + 0.5,
        ))
    })
    .unwrap()
}

/// The reading order's runs as text.
fn runs_text(doc_id: &str) -> (bool, Vec<String>) {
    with_doc(doc_id, |d| {
        let order: ReadingOrder = structtree::reading_order(d, 0)?;
        let layer = layer::layer(d, 0)?;
        let runs = order
            .runs
            .iter()
            .map(|r| {
                layer.chars[r[0] as usize..r[1] as usize]
                    .iter()
                    .filter_map(|c| char::from_u32(c.codepoint))
                    .collect::<String>()
            })
            .collect();
        Ok((order.tagged, runs))
    })
    .unwrap()
}

fn save_bytes(doc_id: &str) -> Vec<u8> {
    let doc_id = doc_id.to_string();
    with_state(move |st| {
        tiles::generate_appearances(st, &doc_id)?;
        Ok(st.doc(&doc_id)?.to_bytes()?.to_vec())
    })
    .expect("save to bytes")
}

/// pkg1 R2 × pkg6 V6: a word redacted out of the middle of a tagged paragraph splits the run
/// in two; both pieces keep MCID 0, so the structure order still reads the lower paragraph
/// first and its surviving words stay together — in memory and in the saved file.
#[test]
fn a_split_redaction_keeps_the_structure_reading_order() {
    let doc_id = open_bytes(tagged_pdf());
    let mark = word_rect(&doc_id, "paragraph below");
    let page: PageIndex = 0;
    {
        let doc_id = doc_id.clone();
        let plan = with_state(move |st| redact::preview_checked(st, &doc_id, page, &[mark]))
            .expect("preview");
        assert!(
            plan.text_objects.iter().any(|o| o.split) && plan.collateral.is_empty(),
            "the run is split, nothing else goes: {plan:?}"
        );
    }
    {
        let doc_id = doc_id.clone();
        let result = with_state(move |st| {
            redact::apply_batch(
                st,
                &doc_id,
                &[RedactBatchMark {
                    page,
                    rects: vec![mark],
                }],
                &RedactOptions {
                    fill: [0, 0, 0],
                    overlay_text: None,
                    ungroup: false,
                },
            )
        })
        .expect("apply");
        assert!(
            result.verified && result.collateral.is_empty(),
            "{result:?}"
        );
    }

    let check = |doc_id: &str, when: &str| {
        let (tagged, runs) = runs_text(doc_id);
        assert!(tagged, "{when}: still read through the structure tree");
        let joined: String = runs.concat();
        assert!(!joined.contains("below"), "{when}: redacted: {runs:?}");
        let first = runs
            .iter()
            .position(|r| r.contains("First"))
            .expect("First survives");
        let here = runs
            .iter()
            .position(|r| r.contains("here"))
            .expect("here survives");
        let second = runs
            .iter()
            .position(|r| r.contains("Second"))
            .expect("Second survives");
        assert!(
            first <= here && here < second,
            "{when}: the redacted paragraph's pieces (MCID 0) are read before MCID 1, \
             not dropped to the untagged tail: {runs:?}"
        );
    };
    check(&doc_id, "in memory");
    let saved = open_bytes(save_bytes(&doc_id));
    check(&saved, "after save");
    close(saved);
    close(doc_id);
}
