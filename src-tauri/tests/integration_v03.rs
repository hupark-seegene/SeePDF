//! v0.3 integration — behaviours that only exist once two work packages meet.
//!
//! pkg1 (R2) splits a partly redacted text run into pieces, re-emitting later pieces as new
//! objects; pkg6 (V6) reads a tagged page in structure-tree order through each character's
//! `/MCID`. The pieces must keep their marked-content id, or the surviving words of a
//! redacted paragraph would drop to "untagged content, read last" in read aloud and the
//! screen-reader region.
//!
//! pkg1 (R5) keeps an edited paragraph's reading order with a `lopdf` pass after the write;
//! pkg3 (S1) saves a signed file incrementally only while nothing but PDFium has changed it.
//! On a signed file the reorder is skipped, so a paragraph edit still saves incrementally.
//!
//! pkg2 (P1) carries a source's outline, labels and fields into an import with a `lopdf`
//! rewrite, and (F2) switches radio groups off in 모든 필드 지우기 with one; both are skipped on
//! a signed file that still saves incrementally (pkg3 S1). pkg2 (P3) imports pages from
//! another open document; pkg3 (S5)'s rule that a restricted source cannot hand its pages to
//! an unrestricted document applies to it as to a file source.
//!
//! pkg7 (O2) turns a sideways page in the same `mutate` as its OCR layer, which makes that
//! mutate structural; pkg3 (S5)'s guard reads a structural mutate as "assemble" alone. A
//! turning OCR needs "modify" as well, or a document that forbids changes but allows page
//! assembly would gain a text layer.
//!
//! pkg7 (O5) writes the OCR layer in a glyphless CID font; pkg1 (R2) splits a partly redacted
//! text object and re-sets its text through the font's `/ToUnicode`, which must work on it.

mod common;

use common::*;
use seepdf_lib::engine::objects::paragraph;
use seepdf_lib::engine::redact;
use seepdf_lib::engine::registry;
use seepdf_lib::engine::render::tiles;
use seepdf_lib::engine::save;
use seepdf_lib::engine::text::{layer, structtree};
use seepdf_lib::engine::{form, ocr, pages, security};
use seepdf_lib::ipc::types::{
    OcrLine, OcrPage, OcrWord, PageIndex, PageOp, ParagraphEdit, ParagraphFlow, PermissionsRequest,
    ReadingOrder, Rect, RedactBatchMark, RedactOptions,
};
use seepdf_lib::ipc::ErrorCode;

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

// ---------------------------------------------------------------------------------------
// pkg1 (R5 paragraph reading order) × pkg3 (S1 incremental save of a signed file)
// ---------------------------------------------------------------------------------------

/// Three Helvetica paragraphs and one (dummy-valued) signature field, like pkg3's fixture.
fn signed_paragraphs_pdf() -> Vec<u8> {
    let content = "BT /F1 12 Tf 72 700 Td (Alpha paragraph opens the page) Tj ET\n\
                   BT /F1 12 Tf 72 600 Td (Bravo paragraph sits in the middle) Tj ET\n\
                   BT /F1 12 Tf 72 450 Td (Charlie paragraph closes the page) Tj ET\n";
    let stream = format!(
        "<< /Length {} >>\nstream\n{content}endstream",
        content.len()
    );
    pdf(&[
        (
            1,
            "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [6 0 R] /SigFlags 3 >> >>".into(),
        ),
        (2, "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into()),
        (
            3,
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] \
             /Resources << /Font << /F1 4 0 R >> >> /Contents 5 0 R /Annots [6 0 R] >>"
                .into(),
        ),
        (
            4,
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>"
                .into(),
        ),
        (5, stream),
        (
            6,
            "<< /Type /Annot /Subtype /Widget /FT /Sig /T (Signature1) /V 7 0 R \
             /Rect [0 0 0 0] /F 132 /P 3 0 R >>"
                .into(),
        ),
        (
            7,
            "<< /Type /Sig /Filter /Adobe.PPKLite /SubFilter /adbe.pkcs7.detached \
             /ByteRange [0 0 0 0] /Contents <30820100> >>"
                .into(),
        ),
    ])
}

fn page_text(doc_id: &str) -> String {
    with_doc(doc_id, |d| Ok(layer::page_text(d, 0)?.text.clone())).unwrap()
}

fn incremental_save(doc_id: &str) -> Option<bool> {
    with_doc(doc_id, |d| Ok(d.info().incremental_save)).unwrap()
}

/// Edits the paragraph under `(x, y)` on page 0 to `text`.
fn edit_paragraph_at(doc_id: &str, x: f32, y: f32, text: &str) {
    let probe = with_doc(doc_id, move |d| paragraph::probe(d, 0, [x, y]))
        .expect("probe_paragraph")
        .expect("a paragraph under the point");
    let edit = ParagraphEdit {
        object_ids: probe.object_ids.clone(),
        text: text.to_string(),
        width: None,
        font_size_pt: None,
        color: None,
        align: None,
        flow: Some(ParagraphFlow::Push),
        dry_run: false,
    };
    let (doc_id, generation) = (doc_id.to_string(), probe.doc_generation);
    with_state(move |st| paragraph::edit(st, &doc_id, 0, generation, edit, false))
        .expect("edit_paragraph");
}

/// A paragraph edit on a signed file must not run R5's `lopdf` reorder: that rewrite would
/// turn the next save into a full rewrite and invalidate the signature. The edit lands, the
/// document stays incrementally saveable, and the saved file still starts with the signed
/// bytes. The same edit on an unsigned file still keeps the reading order (R5).
#[test]
fn a_paragraph_edit_on_a_signed_file_still_saves_incrementally() {
    let original = signed_paragraphs_pdf();
    let dir = fixture("out").join("v03-integration");
    std::fs::create_dir_all(&dir).expect("create fixtures/out/v03-integration");
    let path = dir.join("signed-paragraph-edit.pdf");
    std::fs::write(&path, &original).expect("write the fixture");

    let (owned, bytes) = (path.clone(), original.clone());
    let info = with_state(move |st| registry::open(st, Some(owned), bytes, None)).expect("open");
    assert_eq!(info.signatures.len(), 1, "the fixture is signed");
    let doc_id = info.doc_id.clone();
    assert_eq!(incremental_save(&doc_id), Some(true));

    edit_paragraph_at(
        &doc_id,
        100.0,
        603.0,
        "Delta paragraph replaces the middle one",
    );
    let text = page_text(&doc_id);
    assert!(
        text.contains("Delta paragraph") && !text.contains("Bravo"),
        "{text:?}"
    );
    assert_eq!(
        incremental_save(&doc_id),
        Some(true),
        "the paragraph edit kept the signed file incrementally saveable"
    );

    let d = doc_id.clone();
    with_state(move |st| save::save_with(st, &d, None, &save::SaveOptions::default()).map(|_| ()))
        .expect("save");
    let saved = std::fs::read(&path).expect("read the saved file");
    assert!(saved.len() > original.len(), "an update was appended");
    assert!(
        saved.starts_with(&original),
        "the signed revision stays byte-identical"
    );
    let reopened = open_bytes(saved);
    assert!(page_text(&reopened).contains("Delta paragraph"));
    close(reopened);
    close(doc_id);

    // Unsigned (the same page without the signature): R5 still restores the reading order.
    let unsigned = open_bytes(pdf(&[
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
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>"
                .into(),
        ),
        (5, {
            let content = "BT /F1 12 Tf 72 700 Td (Alpha paragraph opens the page) Tj ET\n\
                           BT /F1 12 Tf 72 600 Td (Bravo paragraph sits in the middle) Tj ET\n\
                           BT /F1 12 Tf 72 450 Td (Charlie paragraph closes the page) Tj ET\n";
            format!(
                "<< /Length {} >>\nstream\n{content}endstream",
                content.len()
            )
        }),
    ]));
    edit_paragraph_at(
        &unsigned,
        100.0,
        603.0,
        "Delta paragraph replaces the middle one",
    );
    let text = page_text(&unsigned);
    let (a, d, c) = (
        text.find("Alpha").unwrap(),
        text.find("Delta").unwrap(),
        text.find("Charlie").unwrap(),
    );
    assert!(
        a < d && d < c,
        "R5 keeps the unsigned edit second: {text:?}"
    );
    close(unsigned);
}

// ---------------------------------------------------------------------------------------
// pkg2 (P1 import carry-over, F2 radio reset, P3 pages between windows) × pkg3 (S1, S5)
// ---------------------------------------------------------------------------------------

/// [`signed_paragraphs_pdf`]'s page with a radio group `Choice` (buttons `A` on, `B` off, no
/// `/DV`) next to the signature field.
fn signed_radio_pdf() -> Vec<u8> {
    let content = "BT /F1 12 Tf 72 700 Td (Signed form with a radio group) Tj ET\n";
    let ap = "<< /Type /XObject /Subtype /Form /BBox [0 0 18 18] /Length 0 >>\nstream\n\nendstream";
    pdf(&[
        (
            1,
            "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [6 0 R 8 0 R] /SigFlags 3 >> >>"
                .into(),
        ),
        (2, "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into()),
        (
            3,
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] \
             /Resources << /Font << /F1 4 0 R >> >> /Contents 5 0 R \
             /Annots [6 0 R 9 0 R 10 0 R] >>"
                .into(),
        ),
        (
            4,
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>"
                .into(),
        ),
        (
            5,
            format!(
                "<< /Length {} >>\nstream\n{content}endstream",
                content.len()
            ),
        ),
        (
            6,
            "<< /Type /Annot /Subtype /Widget /FT /Sig /T (Signature1) /V 7 0 R \
             /Rect [0 0 0 0] /F 132 /P 3 0 R >>"
                .into(),
        ),
        (
            7,
            "<< /Type /Sig /Filter /Adobe.PPKLite /SubFilter /adbe.pkcs7.detached \
             /ByteRange [0 0 0 0] /Contents <30820100> >>"
                .into(),
        ),
        (
            8,
            "<< /FT /Btn /Ff 49152 /T (Choice) /V /A /Kids [9 0 R 10 0 R] >>".into(),
        ),
        (
            9,
            "<< /Type /Annot /Subtype /Widget /Parent 8 0 R /Rect [72 600 90 618] /F 4 \
             /AS /A /AP << /N << /A 11 0 R /Off 12 0 R >> >> /P 3 0 R >>"
                .into(),
        ),
        (
            10,
            "<< /Type /Annot /Subtype /Widget /Parent 8 0 R /Rect [100 600 118 618] /F 4 \
             /AS /Off /AP << /N << /B 13 0 R /Off 14 0 R >> >> /P 3 0 R >>"
                .into(),
        ),
        (11, ap.into()),
        (12, ap.into()),
        (13, ap.into()),
        (14, ap.into()),
    ])
}

/// Writes `bytes` to `fixtures/out/v03-integration/<name>` and opens it from there (a signed
/// document saves incrementally only when it has a path).
fn open_signed(name: &str, bytes: &[u8]) -> (String, std::path::PathBuf) {
    let dir = fixture("out").join("v03-integration");
    std::fs::create_dir_all(&dir).expect("create fixtures/out/v03-integration");
    let path = dir.join(name);
    std::fs::write(&path, bytes).expect("write the fixture");
    let (owned, bytes) = (path.clone(), bytes.to_vec());
    let info = with_state(move |st| registry::open(st, Some(owned), bytes, None)).expect("open");
    assert_eq!(info.signatures.len(), 1, "the fixture is signed");
    assert_eq!(incremental_save(&info.doc_id), Some(true));
    (info.doc_id, path)
}

fn save_in_place(doc_id: &str) {
    let d = doc_id.to_string();
    with_state(move |st| save::save_with(st, &d, None, &save::SaveOptions::default()).map(|_| ()))
        .expect("save");
}

fn has_outline(doc_id: &str) -> bool {
    with_doc(doc_id, |d| Ok(d.info().has_outline)).unwrap()
}

/// P1 × S1: 파일에서 페이지 삽입 into a signed file takes PDFium's plain import — the carry-over
/// is a `lopdf` rewrite that would force a full save — so the signed revision survives the
/// save. The same insert into an unsigned file still carries the source's bookmarks (P1).
#[test]
fn inserting_pages_into_a_signed_file_still_saves_incrementally() {
    let original = signed_paragraphs_pdf();
    let (doc_id, path) = open_signed("signed-insert-from.pdf", &original);
    let source = fixture("gen/outline-labels.pdf").display().to_string();
    let insert = |doc_id: &str| {
        let (d, source) = (doc_id.to_string(), source.clone());
        with_state(move |st| {
            pages::apply_ops(
                st,
                &d,
                vec![PageOp::InsertFrom {
                    at: 1,
                    path: source,
                    range: Some("1-2".into()),
                    password: None,
                }],
            )
        })
        .expect("insertFrom")
    };
    let info = insert(&doc_id);
    assert_eq!(info.page_count, 3);
    assert_eq!(
        incremental_save(&doc_id),
        Some(true),
        "the import kept the signed file incrementally saveable"
    );
    assert!(!has_outline(&doc_id), "no carry-over on a signed file");
    save_in_place(&doc_id);
    let saved = std::fs::read(&path).expect("read the saved file");
    assert!(
        saved.starts_with(&original),
        "the signed revision stays byte-identical"
    );
    let reopened = open_bytes(saved);
    assert_eq!(
        with_doc(&reopened, |d| Ok(d.page_count())).unwrap(),
        3,
        "the inserted pages were saved"
    );
    close(reopened);
    close(doc_id);

    // Unsigned: the carry-over still brings the source's bookmarks along.
    let unsigned = open("tracemonkey.pdf");
    assert!(!has_outline(&unsigned.doc_id));
    insert(&unsigned.doc_id);
    assert!(has_outline(&unsigned.doc_id), "P1 carried the outline");
}

/// F2 × S1: 모든 필드 지우기 on a signed file leaves a radio group without `/DV` as it is (its
/// switch-off is a `lopdf` rewrite) and resets everything else through PDFium, so the file
/// still saves incrementally.
#[test]
fn resetting_a_signed_form_keeps_the_signature() {
    let original = signed_radio_pdf();
    let (doc_id, path) = open_signed("signed-radio-reset.pdf", &original);
    let radio_on = |doc_id: &str| {
        with_doc(doc_id, |d| {
            Ok(form::list(d, None)?
                .into_iter()
                .filter(|f| f.name == "Choice")
                .filter_map(|f| f.checked)
                .collect::<Vec<bool>>())
        })
        .unwrap()
    };
    assert_eq!(radio_on(&doc_id), vec![true, false]);
    let d = doc_id.clone();
    with_state(move |st| form::reset_form(st, &d)).expect("reset_form");
    assert_eq!(
        incremental_save(&doc_id),
        Some(true),
        "reset kept the signed file incrementally saveable"
    );
    assert_eq!(
        radio_on(&doc_id),
        vec![true, false],
        "the radio group is kept"
    );
    save_in_place(&doc_id);
    let saved = std::fs::read(&path).expect("read the saved file");
    assert!(saved.starts_with(&original));
    close(doc_id);

    // Unsigned (the same bytes opened without a path, so no incremental base): the group is
    // switched off as pkg2 F2 does.
    let unsigned = open_bytes(original);
    let d = unsigned.clone();
    with_state(move |st| form::reset_form(st, &d)).expect("reset_form");
    assert_eq!(radio_on(&unsigned), vec![false, false]);
    close(unsigned);
}

/// P3 × S5: pages dragged out of a restricted document cannot land in an unrestricted one —
/// the rule `insertFrom` / 병합 apply to a restricted file.
#[test]
fn pages_from_a_restricted_open_document_are_refused() {
    let restricted =
        try_open("gen/encrypted-rc4-40.pdf", Some("user")).expect("open with the password");
    assert!(
        !restricted.info.permissions.print,
        "the fixture is restricted"
    );
    let dst = open("tracemonkey.pdf");
    let before = dst.info.page_count;
    let err = with_state({
        let (s, d) = (restricted.doc_id.clone(), dst.doc_id.clone());
        move |st| pages::import_pages_from_doc(st, &s, &[0], &d, 0)
    })
    .expect_err("a restricted source is refused");
    assert_eq!(err.code, ErrorCode::PermissionDenied);
    assert_eq!(err.detail.as_deref(), Some("security"));
    assert_eq!(
        with_doc(&dst.doc_id, |d| Ok(d.page_count())).unwrap(),
        before,
        "the target is untouched"
    );
}

// ---------------------------------------------------------------------------------------
// pkg7 (O2 페이지 회전 자동 감지) × pkg3 (S5 permission flags)
// ---------------------------------------------------------------------------------------

/// tracemonkey.pdf written with an owner password and `permissions`, reopened without one.
fn restricted_copy(name: &str, permissions: PermissionsRequest) -> String {
    let dir = fixture("out").join("v03-integration");
    std::fs::create_dir_all(&dir).expect("create fixtures/out/v03-integration");
    let target = dir.join(format!("{name}-{}.pdf", std::process::id()));
    let _ = std::fs::remove_file(&target);
    let src = open("tracemonkey.pdf");
    let path = target.display().to_string();
    let d = src.doc_id.clone();
    with_state(move |st| security::set_password(st, &d, &path, None, "owner-pw", permissions))
        .expect("set_password");
    drop(src);
    let bytes = std::fs::read(&target).expect("read the protected file");
    open_bytes(bytes)
}

/// One English word recognised on page 0 at `rotation` (150 DPI image of a Letter page).
fn one_word_ocr(rotation: u16) -> OcrPage {
    let (w, h) = if rotation % 180 == 0 {
        (1275, 1650)
    } else {
        (1650, 1275)
    };
    let word = OcrWord {
        text: "Scanned".to_string(),
        bbox: [200.0, 200.0, 420.0, 250.0],
        confidence: 92.0,
    };
    OcrPage {
        page: 0,
        dpi: 150,
        width_px: w,
        height_px: h,
        rotation,
        lines: vec![OcrLine {
            text: word.text.clone(),
            bbox: word.bbox,
            baseline: None,
            row_height_px: Some(50.0),
            words: vec![word],
        }],
    }
}

fn ocr_apply(
    doc_id: &str,
    page: OcrPage,
    set_rotation: Option<u16>,
) -> Result<(), seepdf_lib::ipc::EngineError> {
    let doc_id = doc_id.to_string();
    with_state(move |st| {
        let mut progress = |_: usize, _: u16| {};
        ocr::apply_rotated_cancellable(
            st,
            &doc_id,
            &[page],
            &[set_rotation],
            false,
            &mut progress,
            &|| false,
        )
    })
    .map(|_| ())
}

fn rotation_of(doc_id: &str) -> u16 {
    with_doc(doc_id, |d| Ok(d.geom(0)?.rotation)).unwrap()
}

/// "assemble" allowed, "modify" forbidden: OCR is refused whether or not it turns the page,
/// and the page keeps its rotation. "modify" allowed, "assemble" forbidden: the layer alone
/// goes in, a layer that would turn the page is refused as "assemble".
#[test]
fn a_turning_ocr_needs_both_modify_and_assemble() {
    let no_modify = restricted_copy(
        "ocr-no-modify",
        PermissionsRequest {
            modify: false,
            ..PermissionsRequest::default()
        },
    );
    for turn in [None, Some(90)] {
        let err = ocr_apply(&no_modify, one_word_ocr(turn.unwrap_or(0)), turn)
            .expect_err("a document that forbids changes gains no OCR layer");
        assert_eq!(err.code, ErrorCode::PermissionDenied, "{turn:?}");
        assert_eq!(err.detail.as_deref(), Some("modify"), "{turn:?}");
    }
    assert_eq!(rotation_of(&no_modify), 0, "the page did not turn");
    assert!(
        !page_text(&no_modify).contains("Scanned"),
        "and has no OCR layer"
    );
    close(no_modify);

    let no_assemble = restricted_copy(
        "ocr-no-assemble",
        PermissionsRequest {
            assemble: false,
            ..PermissionsRequest::default()
        },
    );
    let err = ocr_apply(&no_assemble, one_word_ocr(90), Some(90))
        .expect_err("turning the page needs assemble");
    assert_eq!(err.code, ErrorCode::PermissionDenied);
    assert_eq!(err.detail.as_deref(), Some("assemble"));
    assert_eq!(rotation_of(&no_assemble), 0);
    ocr_apply(&no_assemble, one_word_ocr(0), None).expect("the layer alone is a modification");
    assert!(page_text(&no_assemble).contains("Scanned"));
    close(no_assemble);
}

// ---------------------------------------------------------------------------------------
// pkg7 (O5 glyphless OCR font) × pkg1 (R2 word-level redaction)
// ---------------------------------------------------------------------------------------

/// A Korean OCR line written in pkg7's glyphless CID font (CID = code point, identity
/// `/ToUnicode`, text set through `FPDFText_SetCharcodes`) meets pkg1's word-level redaction,
/// which splits a partly marked object and re-sets its text through the font's `/ToUnicode`:
/// only the marked word goes, the rest of the line still extracts — in memory and after save.
#[test]
fn a_word_redacted_out_of_a_glyphless_ocr_line_leaves_the_rest() {
    let doc = open("tracemonkey.pdf");
    let (w, h) = with_doc(&doc.doc_id, |d| {
        let g = d.geom(0)?;
        Ok((g.width_pt, g.height_pt))
    })
    .unwrap();
    // 150 DPI image of the page; the line sits in the top margin, clear of the page's text.
    let (wpx, hpx) = ((w * 150.0 / 72.0) as u32, (h * 150.0 / 72.0) as u32);
    let words = [
        ("검색", [200.0, 40.0, 300.0, 90.0]),
        ("가능한", [320.0, 40.0, 470.0, 90.0]),
        ("한글", [490.0, 40.0, 590.0, 90.0]),
        ("문서", [610.0, 40.0, 710.0, 90.0]),
    ];
    let words: Vec<OcrWord> = words
        .iter()
        .map(|(text, bbox)| OcrWord {
            text: text.to_string(),
            bbox: *bbox,
            confidence: 92.0,
        })
        .collect();
    let page = OcrPage {
        page: 0,
        dpi: 150,
        width_px: wpx,
        height_px: hpx,
        rotation: 0,
        lines: vec![OcrLine {
            text: "검색 가능한 한글 문서".into(),
            bbox: [200.0, 40.0, 710.0, 90.0],
            baseline: None,
            row_height_px: Some(50.0),
            words,
        }],
    };
    ocr_apply(&doc.doc_id, page, None).expect("ocr_apply");
    assert!(page_text(&doc.doc_id).contains("한글"));

    let mark = word_rect(&doc.doc_id, "한글");
    let d = doc.doc_id.clone();
    let plan = with_state(move |st| redact::preview_checked(st, &d, 0, &[mark])).expect("preview");
    // The OCR layer is one object per word ("한글 " with its space); the half-point margin of
    // the mark reaches into its neighbour's trailing space, so that glyphless object is split
    // (R2 re-sets its text through the identity `/ToUnicode`) rather than removed.
    assert!(
        plan.text_objects.iter().any(|o| o.split) && plan.collateral.is_empty(),
        "{plan:?}"
    );
    let d = doc.doc_id.clone();
    let result = with_state(move |st| {
        redact::apply_batch(
            st,
            &d,
            &[RedactBatchMark {
                page: 0,
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
    assert!(result.verified, "{result:?}");

    let check = |doc_id: &str, when: &str| {
        let text = page_text(doc_id);
        assert!(!text.contains("한글"), "{when}: redacted: {text:?}");
        for kept in ["검색", "가능한", "문서"] {
            assert!(text.contains(kept), "{when}: '{kept}' survives: {text:?}");
        }
    };
    check(&doc.doc_id, "in memory");
    let saved = open_bytes(save_bytes(&doc.doc_id));
    check(&saved, "after save");
    close(saved);
}
