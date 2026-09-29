//! v0.3 pkg6 — plain-text web links (V5, `get_web_links`) and the structure tree's reading
//! order (V6, `get_reading_order`), against the real PDFium.
//!
//! Both fixtures are built here, byte for byte, so the test says exactly what the page holds:
//! a page with typed (not linked) addresses, and a tagged page whose structure tree reads the
//! lower paragraph first — the opposite of the content-stream and y order.

mod common;

use common::*;
use seepdf_lib::engine::registry;
use seepdf_lib::engine::text::{layer, structtree, weblinks};
use seepdf_lib::ipc::types::ReadingOrder;

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

fn stream(content: &str) -> String {
    format!(
        "<< /Length {} >>\nstream\n{content}\nendstream",
        content.len()
    )
}

fn open_bytes(bytes: Vec<u8>) -> String {
    with_state(move |st| registry::open(st, None, bytes, None))
        .expect("open generated pdf")
        .doc_id
}

fn close(doc_id: String) {
    let _ = with_state(move |st| registry::close(st, &doc_id));
}

/// The text of the layer chars `[start, end)`.
fn text_of(doc_id: &str, run: [u32; 2]) -> String {
    with_doc(doc_id, move |d| {
        let layer = layer::layer(d, 0)?;
        Ok(layer.chars[run[0] as usize..run[1] as usize]
            .iter()
            .filter_map(|c| char::from_u32(c.codepoint))
            .collect())
    })
    .unwrap()
}

#[test]
fn web_links_are_found_in_plain_page_text() {
    let content = "BT /F1 12 Tf 72 700 Td (Visit https://example.com/seepdf for details) Tj ET\n\
                   BT /F1 12 Tf 72 650 Td (Write to hello@example.com any time) Tj ET";
    let doc_id = open_bytes(pdf(&[
        (1, "<< /Type /Catalog /Pages 2 0 R >>".into()),
        (2, "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into()),
        (
            3,
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] \
             /Resources << /Font << /F1 4 0 R >> >> /Contents 5 0 R >>"
                .into(),
        ),
        (
            4,
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into(),
        ),
        (5, stream(content)),
    ]));

    let links = with_doc(&doc_id, |d| weblinks::web_links(d, 0)).unwrap();
    let web = links
        .iter()
        .find(|l| l.url == "https://example.com/seepdf")
        .unwrap_or_else(|| panic!("no https link in {links:?}"));
    // one line, on the first baseline (y 700, 12 pt), right of "Visit "
    assert_eq!(web.rects.len(), 1);
    let r = web.rects[0];
    assert!(
        r.b > 690.0 && r.b < 705.0 && r.t > 705.0 && r.t < 716.0,
        "{r:?}"
    );
    assert!(r.l > 95.0 && r.r > r.l + 100.0 && r.r < 330.0, "{r:?}");
    // the char range is the address in the text layer
    let chars = web.char_count;
    assert_eq!(chars as usize, "https://example.com/seepdf".len());
    let text = text_of(&doc_id, [web.char_start, web.char_start + chars]);
    assert_eq!(text, "https://example.com/seepdf");
    // an e-mail address comes back as mailto:, on the second line
    let mail = links
        .iter()
        .find(|l| l.url.starts_with("mailto:"))
        .unwrap_or_else(|| panic!("no mailto link in {links:?}"));
    assert_eq!(mail.url, "mailto:hello@example.com");
    assert!(mail.rects[0].t < 665.0 && mail.rects[0].b > 640.0);
    close(doc_id);
}

#[test]
fn a_page_without_addresses_has_no_web_links() {
    let doc = open("rotation.pdf");
    let links = with_doc(&doc.doc_id, |d| weblinks::web_links(d, 0)).unwrap();
    assert!(links.is_empty(), "{links:?}");
}

/// The tagged fixture: "Second paragraph" is drawn first, at the top (MCID 1); "First
/// paragraph" second, lower down (MCID 0). The structure tree reads MCID 0, then 1.
fn tagged_pdf() -> Vec<u8> {
    let content =
        "/P << /MCID 1 >> BDC BT /F1 18 Tf 72 700 Td (Second paragraph on top) Tj ET EMC\n\
                   /P << /MCID 0 >> BDC BT /F1 18 Tf 72 400 Td (First paragraph below) Tj ET EMC";
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
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into(),
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
        (9, stream(content)),
        (10, "<< /Nums [0 [7 0 R 8 0 R]] >>".into()),
    ])
}

#[test]
fn a_tagged_page_reads_in_structure_order_not_y_order() {
    let doc_id = open_bytes(tagged_pdf());
    let tagged_doc = with_doc(&doc_id, |d| Ok(d.tagged)).unwrap();
    assert!(tagged_doc, "the catalog says /MarkInfo /Marked true");

    // content (and y) order: the top paragraph first
    let all = with_doc(&doc_id, |d| Ok(layer::layer(d, 0)?.text.clone())).unwrap();
    assert!(all.trim_start().starts_with("Second"), "{all:?}");

    let order: ReadingOrder = with_doc(&doc_id, |d| structtree::reading_order(d, 0)).unwrap();
    assert!(order.tagged);
    assert!(order.runs.len() >= 2, "{order:?}");
    let first = text_of(&doc_id, order.runs[0]);
    let second = text_of(&doc_id, order.runs[1]);
    assert!(first.starts_with("First paragraph below"), "{first:?}");
    assert!(second.starts_with("Second paragraph on top"), "{second:?}");
    // every char is read exactly once
    let covered: u32 = order.runs.iter().map(|r| r[1] - r[0]).sum();
    let chars = with_doc(&doc_id, |d| Ok(layer::layer(d, 0)?.chars.len())).unwrap();
    assert_eq!(covered as usize, chars);
    close(doc_id);
}

#[test]
fn an_untagged_page_is_one_run_in_content_order() {
    let doc = open("tracemonkey.pdf");
    let order = with_doc(&doc.doc_id, |d| structtree::reading_order(d, 0)).unwrap();
    let chars = with_doc(&doc.doc_id, |d| Ok(layer::layer(d, 0)?.chars.len())).unwrap();
    assert_eq!(
        order,
        ReadingOrder {
            tagged: false,
            runs: vec![[0, chars as u32]]
        }
    );
}
