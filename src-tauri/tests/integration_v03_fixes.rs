//! v0.3 integration — regressions found by the cross-package verification (interaction lens),
//! each a behaviour that only breaks where two work packages meet.
//!
//! * pkg1 redaction × pkg3 S1: a redaction of a signed file must not survive in the signed
//!   revision an incremental save keeps.
//! * pkg8 X2 모아찍기 × pkg3 S5: the n-up document of a restricted file keeps its restrictions.
//! * pkg8 X3 / X6 text exports × pkg7 O2: a page OCR turned sideways exports as lines.
//! * pkg1 / pkg4 lopdf passes × pkg3 S2: encrypted documents go through the encryption-safe
//!   `registry::mutate_bytes` instead of being refused or skipped.
//! * pkg4 A7 annotation summary × pkg3 S5: a copy-forbidden document's text stays inside.
//! * pkg2 sanitize × attachments: an empty attachment tree is not "one attachment removed".

mod common;

use common::*;
use pdfium_render::prelude::*;
use seepdf_lib::engine::annot::create::create_in;
use seepdf_lib::engine::render::tiles;
use seepdf_lib::engine::text::layer;
use seepdf_lib::engine::{ocr, redact, registry, security};
use seepdf_lib::ipc::types::{
    AnnotSpec, OcrLine, OcrPage, OcrWord, PermissionsRequest, PolySpec, Rect, RedactBatchMark,
    RedactOptions,
};

fn out_dir() -> std::path::PathBuf {
    let d = fixture("out").join("v03-integration-fixes");
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn open_bytes_pw(bytes: Vec<u8>, pw: Option<&str>) -> seepdf_lib::ipc::types::DocInfo {
    let pw = pw.map(str::to_string);
    with_state(move |st| registry::open(st, None, bytes, pw)).expect("open")
}

fn close(doc_id: String) {
    let _ = with_state(move |st| registry::close(st, &doc_id));
}

fn page_text(doc_id: &str) -> String {
    with_doc(doc_id, |d| Ok(layer::page_text(d, 0)?.text.clone())).unwrap()
}

fn save_bytes(doc_id: &str) -> Vec<u8> {
    let doc_id = doc_id.to_string();
    with_state(move |st| {
        tiles::generate_appearances(st, &doc_id)?;
        Ok(st.doc(&doc_id)?.to_bytes()?.to_vec())
    })
    .expect("save to bytes")
}

/// `fixture_name` written by `set_password` with `user` (open password, if any), owner
/// password `owner-pw` and `permissions`; the file's bytes.
fn protected(
    fixture_name: &str,
    tag: &str,
    user: Option<&str>,
    permissions: PermissionsRequest,
) -> Vec<u8> {
    let target = out_dir().join(format!("{tag}-{}.pdf", std::process::id()));
    let _ = std::fs::remove_file(&target);
    let src = open(fixture_name);
    let path = target.display().to_string();
    let d = src.doc_id.clone();
    let user = user.map(str::to_string);
    with_state(move |st| {
        security::set_password(st, &d, &path, user.as_deref(), "owner-pw", permissions)
    })
    .expect("set_password");
    drop(src);
    std::fs::read(&target).expect("read")
}

fn protected_bytes(bytes: Vec<u8>, tag: &str, user: Option<&str>) -> Vec<u8> {
    let plain = open_bytes_pw(bytes, None);
    let target = out_dir().join(format!("{tag}-{}.pdf", std::process::id()));
    let (d, p) = (plain.doc_id.clone(), target.display().to_string());
    let user = user.map(str::to_string);
    with_state(move |st| {
        security::set_password(
            st,
            &d,
            &p,
            user.as_deref(),
            "owner-pw",
            PermissionsRequest::default(),
        )
    })
    .expect("set_password");
    close(plain.doc_id);
    std::fs::read(&target).unwrap()
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

fn redact_rects(
    doc_id: &str,
    rects: Vec<Rect>,
) -> Result<seepdf_lib::ipc::types::RedactBatchResult, seepdf_lib::ipc::EngineError> {
    let d = doc_id.to_string();
    with_state(move |st| {
        redact::apply_batch(
            st,
            &d,
            &[RedactBatchMark { page: 0, rects }],
            &RedactOptions {
                fill: [0, 0, 0],
                overlay_text: None,
                ungroup: false,
            },
        )
    })
}

fn raw_pdf(objects: &[(u32, String)]) -> Vec<u8> {
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

const THREE_PARAGRAPHS: &str = "BT /F1 12 Tf 72 700 Td (Alpha paragraph opens the page) Tj ET\n\
                                BT /F1 12 Tf 72 600 Td (Bravo paragraph sits in the middle) Tj ET\n\
                                BT /F1 12 Tf 72 450 Td (Charlie paragraph closes the page) Tj ET\n";

fn three_paragraphs() -> Vec<u8> {
    raw_pdf(&[
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
        (
            5,
            format!(
                "<< /Length {} >>\nstream\n{THREE_PARAGRAPHS}endstream",
                THREE_PARAGRAPHS.len()
            ),
        ),
    ])
}

/// The three paragraphs with a (detection-only) signature field, as `integration_v03`'s
/// signed fixture.
fn signed_paragraphs() -> Vec<u8> {
    raw_pdf(&[
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
        (
            5,
            format!(
                "<< /Length {} >>\nstream\n{THREE_PARAGRAPHS}endstream",
                THREE_PARAGRAPHS.len()
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
    ])
}

// ---------------------------------------------------------------------------------------
// pkg1 redaction × pkg3 S1 incremental save of a signed file
// ---------------------------------------------------------------------------------------

/// Redacting a signed file and saving it must not keep the redacted text in the file: the
/// save is a full rewrite (the signature goes, and `incrementalSave` says so before the save,
/// so the UI warns), and undo goes back to the incremental state.
#[test]
fn redaction_on_a_signed_file_leaves_no_trace_in_the_saved_file() {
    let original = signed_paragraphs();
    let path = out_dir().join(format!("signed-redact-{}.pdf", std::process::id()));
    std::fs::write(&path, &original).unwrap();
    let (p, b) = (path.clone(), original.clone());
    let info = with_state(move |st| registry::open(st, Some(p), b, None)).expect("open");
    let doc = info.doc_id.clone();
    assert_eq!(info.signatures.len(), 1);
    assert_eq!(info.incremental_save, Some(true));

    // Undo of a redaction goes back to the pristine (incremental) state.
    let mark = word_rect(&doc, "Bravo paragraph sits in the middle");
    assert!(redact_rects(&doc, vec![mark]).unwrap().verified);
    let flag = with_doc(&doc, |d| Ok(d.info().incremental_save)).unwrap();
    assert_eq!(
        flag,
        Some(false),
        "a redacted signed file saves as a full rewrite"
    );
    let d = doc.clone();
    with_state(move |st| registry::undo(st, &d, false)).expect("undo");
    assert!(page_text(&doc).contains("Bravo"));
    let flag = with_doc(&doc, |d| Ok(d.info().incremental_save)).unwrap();
    assert_eq!(flag, Some(true), "undo is back to the signed revision");

    // The whole run, and a word out of the middle of another one (R2 split).
    let marks = vec![
        word_rect(&doc, "Bravo paragraph sits in the middle"),
        word_rect(&doc, "closes"),
    ];
    assert!(redact_rects(&doc, marks).unwrap().verified);
    assert!(!page_text(&doc).contains("Bravo"), "gone from the page");
    let d = doc.clone();
    with_state(move |st| {
        seepdf_lib::engine::save::save_with(
            st,
            &d,
            None,
            &seepdf_lib::engine::save::SaveOptions::default(),
        )
        .map(|_| ())
    })
    .expect("save");
    let saved = std::fs::read(&path).unwrap();
    assert!(
        !saved.starts_with(&original),
        "not appended to the signed bytes"
    );
    for word in [&b"Bravo"[..], b"closes"] {
        assert!(
            !saved.windows(word.len()).any(|w| w == word),
            "{} is still in the saved file",
            String::from_utf8_lossy(word)
        );
    }
    let reopened = open_bytes_pw(saved, None).doc_id;
    let text = page_text(&reopened);
    assert!(
        text.contains("Alpha") && text.contains("Charlie"),
        "{text:?}"
    );
    close(reopened);
    close(doc);
}

// ---------------------------------------------------------------------------------------
// pkg8 X2 모아찍기 × pkg3 S5 permissions
// ---------------------------------------------------------------------------------------

fn nup_of(doc_id: &str) -> Vec<u8> {
    use seepdf_lib::engine::export::{nup, PrintAnnots};
    let opts = nup::NupOptions {
        per_sheet: 2,
        order: nup::NupOrder::Across,
        booklet: false,
        paper: nup::NupPaper::Auto,
        annots: PrintAnnots::All,
    };
    let d = doc_id.to_string();
    with_state(move |st| nup::make_nup_bytes(st, &d, Some(&[0, 1]), &opts))
        .expect("n-up")
        .0
}

/// The n-up document of a copy / modify-forbidden file is encrypted with the same permission
/// bits (it opens restricted, without a password); that of a file with an open password needs
/// that password. An unencrypted source gives an unencrypted n-up.
#[test]
fn nup_of_a_restricted_document_keeps_its_restrictions() {
    let restricted = PermissionsRequest {
        modify: false,
        extract_text: false,
        annotate: false,
        fill_forms: false,
        assemble: false,
        ..PermissionsRequest::default()
    };
    let src = open_bytes_pw(
        protected("tracemonkey.pdf", "nup-restricted", None, restricted),
        None,
    );
    assert!(src.encrypted && !src.permissions.extract_text && !src.permissions.modify);
    let out = nup_of(&src.doc_id);
    let sheets = open_bytes_pw(out, None);
    assert!(sheets.encrypted, "the n-up is encrypted");
    assert_eq!(sheets.permissions, src.permissions, "same permission bits");
    assert!(sheets.permissions.print);
    close(sheets.doc_id);
    close(src.doc_id);

    let src = open_bytes_pw(
        protected(
            "tracemonkey.pdf",
            "nup-userpw",
            Some("user-pw"),
            PermissionsRequest::default(),
        ),
        Some("user-pw"),
    );
    let out = nup_of(&src.doc_id);
    let bytes = out.clone();
    assert!(
        with_state(move |st| registry::open(st, None, bytes, None)).is_err(),
        "the n-up needs the open password"
    );
    let sheets = open_bytes_pw(out, Some("user-pw"));
    assert!(sheets.encrypted && sheets.permissions.extract_text);
    assert!(page_text(&sheets.doc_id).contains("Trace"));
    close(sheets.doc_id);
    close(src.doc_id);

    let plain = open("tracemonkey.pdf");
    let sheets = open_bytes_pw(nup_of(&plain.doc_id), None);
    assert!(!sheets.encrypted);
    close(sheets.doc_id);
}

// ---------------------------------------------------------------------------------------
// pkg4 A7 annotation summary × pkg3 S5 copy permission
// ---------------------------------------------------------------------------------------

/// A copy-forbidden document's highlight is exported with its comment but without the
/// highlighted text; with the permission the quote is there.
#[test]
fn annotation_summary_of_a_copy_forbidden_document_leaves_out_the_quote() {
    use seepdf_lib::engine::export::summary;
    use seepdf_lib::ipc::types::{Locale, SummaryFormat};
    let quotes = |doc: &str| -> Vec<String> {
        let d = doc.to_string();
        with_state(move |st| summary::collect(st, &d, None, Locale::Ko))
            .unwrap()
            .into_iter()
            .map(|r| r.quote)
            .collect()
    };
    let plain = open("annotation-highlight.pdf");
    let allowed = quotes(&plain.doc_id);
    assert!(
        allowed.iter().any(|q| q.contains("Lorem")),
        "the fixture's highlight quotes its text: {allowed:?}"
    );

    let doc = open_bytes_pw(
        protected(
            "annotation-highlight.pdf",
            "summary-nocopy",
            None,
            PermissionsRequest {
                extract_text: false,
                ..PermissionsRequest::default()
            },
        ),
        None,
    );
    assert!(!doc.permissions.extract_text);
    let forbidden = quotes(&doc.doc_id);
    assert_eq!(
        forbidden.len(),
        allowed.len(),
        "every annotation is still exported"
    );
    assert!(forbidden.iter().all(String::is_empty), "{forbidden:?}");
    let out = out_dir().join("summary-nocopy.csv");
    let (d, p) = (doc.doc_id.clone(), out.display().to_string());
    let r = with_state(move |st| {
        summary::export_annotation_summary(st, &d, &p, SummaryFormat::Csv, None, Locale::Ko)
    })
    .expect("export");
    assert_eq!(r.count as usize, allowed.len());
    let csv = std::fs::read_to_string(&out).unwrap();
    assert!(!csv.contains("Lorem"), "{csv}");
    close(doc.doc_id);
}

// ---------------------------------------------------------------------------------------
// pkg3 S3 문서 정리 × 첨부 panel
// ---------------------------------------------------------------------------------------

/// An attachment added and deleted again leaves PDFium's empty `/EmbeddedFiles` tree: 문서 정리
/// finds nothing (0 attachments, no undo step, same generation). With a real attachment it
/// counts exactly one.
#[test]
fn sanitize_after_the_only_attachment_was_deleted_finds_nothing() {
    use seepdf_lib::engine::{attachments, sanitize};
    use seepdf_lib::ipc::types::SanitizeOptions;
    let options = SanitizeOptions {
        javascript: true,
        attachments: true,
        actions: true,
        metadata: false,
        hidden_layers: true,
    };
    let doc = open("tracemonkey.pdf");
    let attachment = fixture("alphatrans.pdf").display().to_string();
    let (d, a) = (doc.doc_id.clone(), attachment.clone());
    let list = with_state(move |st| attachments::add(st, &d, &a, None)).unwrap();
    assert_eq!(list.len(), 1);
    let d = doc.doc_id.clone();
    assert!(with_state(move |st| attachments::delete(st, &d, 0))
        .unwrap()
        .is_empty());
    let (generation, depth) =
        with_doc(&doc.doc_id, |d| Ok((d.generation, d.history.undo_depth()))).unwrap();
    let d = doc.doc_id.clone();
    let result = with_state(move |st| sanitize::sanitize(st, &d, &options)).unwrap();
    assert_eq!(result.removed.attachments, 0, "{:?}", result.removed);
    assert_eq!(result.info.doc_generation, generation, "no new generation");
    let after = with_doc(&doc.doc_id, |d| Ok(d.history.undo_depth())).unwrap();
    assert_eq!(after, depth, "no undo step");

    let (d, a) = (doc.doc_id.clone(), attachment);
    with_state(move |st| attachments::add(st, &d, &a, None)).unwrap();
    let d = doc.doc_id.clone();
    let result = with_state(move |st| sanitize::sanitize(st, &d, &options)).unwrap();
    assert_eq!(result.removed.attachments, 1, "{:?}", result.removed);
}

// ---------------------------------------------------------------------------------------
// pkg8 X3 / X6 text exports × pkg7 O2 auto-rotate
// ---------------------------------------------------------------------------------------

fn busy_rgb(w: u32, h: u32) -> image::RgbImage {
    let mut img = image::RgbImage::new(w, h);
    for (x, y, px) in img.enumerate_pixels_mut() {
        *px = image::Rgb([
            40 + ((x * 7 + y * 3) % 200) as u8,
            40 + ((x * 3 + y * 11) % 200) as u8,
            40 + ((x * 13 + y * 5) % 200) as u8,
        ]);
    }
    img
}

/// A 600 × 800 pt page covered by one 300 × 400 px image (a scan).
fn scan_doc() -> String {
    let bytes = with_state(move |st| {
        let pdfium = st.pdfium;
        let mut document = pdfium.create_new_pdf().expect("new pdf");
        {
            let mut page = document
                .pages_mut()
                .create_page_at_end(PdfPagePaperSize::Custom(
                    PdfPoints::new(600.0),
                    PdfPoints::new(800.0),
                ))
                .expect("page");
            let img = image::DynamicImage::ImageRgb8(busy_rgb(300, 400));
            let mut object = PdfPageImageObject::new(&document, &img).expect("image");
            object
                .apply_matrix(PdfMatrix::new(600.0, 0.0, 0.0, 800.0, 0.0, 0.0))
                .expect("matrix");
            page.objects_mut().add_image_object(object).expect("add");
            page.regenerate_content().expect("regenerate");
        }
        Ok(document.save_to_bytes().expect("save"))
    })
    .expect("build");
    open_bytes_pw(bytes, None).doc_id
}

fn ocr_line(text: &str, y: f32) -> OcrLine {
    let mut x = 200.0;
    let words: Vec<OcrWord> = text
        .split(' ')
        .map(|w| {
            let wpx = 40.0 * w.chars().count() as f32;
            let b = [x, y, x + wpx, y + 60.0];
            x += wpx + 40.0;
            OcrWord {
                text: w.into(),
                bbox: b,
                confidence: 92.0,
            }
        })
        .collect();
    OcrLine {
        text: text.into(),
        bbox: [200.0, y, x - 40.0, y + 60.0],
        baseline: None,
        row_height_px: Some(60.0),
        words,
    }
}

/// A scan OCR'd with two lines; at 90° / 270° O2 turns the page (`setRotation`) and writes
/// the layer turned. 텍스트 흐름 (DOCX / HWPX / HTML / MD) and 레이아웃 유지 read the two
/// lines, in order, whatever the turn.
#[test]
fn text_exports_of_an_ocr_turned_page_read_as_lines() {
    use seepdf_lib::engine::export::{self, textflow};
    for rot in [0u16, 90, 270] {
        let doc = scan_doc();
        let (w, h) = if rot % 180 == 90 {
            (1667, 1250)
        } else {
            (1250, 1667)
        };
        let page = OcrPage {
            page: 0,
            dpi: 150,
            width_px: w,
            height_px: h,
            rotation: rot,
            lines: vec![
                ocr_line("Alpha Bravo Charlie", 300.0),
                ocr_line("Delta Echo Foxtrot", 400.0),
            ],
        };
        let d = doc.clone();
        let set = if rot == 0 { None } else { Some(rot) };
        with_state(move |st| {
            let mut progress = |_: usize, _: u16| {};
            ocr::apply_rotated_cancellable(st, &d, &[page], &[set], false, &mut progress, &|| false)
        })
        .expect("ocr");
        let d = doc.clone();
        let blocks = with_state(move |st| {
            let mut flow = textflow::FlowDoc::default();
            textflow::collect_page(st.doc_mut(&d)?, 0, &mut flow)?;
            Ok(flow
                .blocks
                .into_iter()
                .filter_map(|b| match b {
                    textflow::Block::Text { text, .. } => Some(text),
                    _ => None,
                })
                .collect::<Vec<_>>())
        })
        .unwrap();
        let joined = blocks.join(" | ");
        let (a, d) = (
            joined.find("Alpha Bravo Charlie"),
            joined.find("Delta Echo Foxtrot"),
        );
        assert!(
            a.is_some() && d.is_some() && a < d,
            "rot {rot}: text flow = {blocks:?}"
        );

        let out = out_dir().join(format!("layout-rot{rot}.txt"));
        let (d, o) = (doc.clone(), out.display().to_string());
        with_state(move |st| export::export_text_with(st, &d, &[0], &o, true).map(|_| ()))
            .expect("layout text");
        let layout = std::fs::read_to_string(&out).unwrap();
        let lines: Vec<&str> = layout
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .collect();
        assert_eq!(
            lines,
            ["Alpha Bravo Charlie", "Delta Echo Foxtrot"],
            "rot {rot}: layout text = {layout:?}"
        );
        close(doc);
    }
}

// ---------------------------------------------------------------------------------------
// pkg1 / pkg4 lopdf passes × pkg3 S2 encrypted rewrites
// ---------------------------------------------------------------------------------------

fn annot_subtypes(doc_id: &str) -> Vec<String> {
    with_doc(doc_id, |d| seepdf_lib::engine::annot::list(d, 0))
        .unwrap()
        .into_iter()
        .map(|a| a.subtype)
        .collect()
}

/// A polygon and a line on an AES-256 document with an open password are the real `/Polygon`
/// and `/Line` (not refused, not an Ink line), and the file stays encrypted.
#[test]
fn polygon_and_line_on_a_password_protected_document() {
    use seepdf_lib::ipc::types::LineSpec;
    let bytes = protected(
        "tracemonkey.pdf",
        "poly-aes",
        Some("user-pw"),
        PermissionsRequest::default(),
    );
    let info = open_bytes_pw(bytes, Some("user-pw"));
    assert!(info.encrypted);
    let polygon = AnnotSpec::Polygon(PolySpec {
        vertices: vec![100.0, 400.0, 200.0, 400.0, 150.0, 480.0],
        color: [0, 120, 0],
        fill_color: None,
        width: 1.0,
        opacity: 1.0,
        cloudy: true,
        dashed: false,
        measure: None,
    });
    let line = AnnotSpec::Line(LineSpec {
        p1: [100.0, 300.0],
        p2: [300.0, 320.0],
        color: [200, 0, 0],
        width: 2.0,
        opacity: 1.0,
        dashed: false,
        heads: None,
        measure: None,
    });
    for spec in [polygon, line] {
        let d = info.doc_id.clone();
        with_state(move |st| create_in(st, &d, 0, &spec, None, Some("검토자")))
            .expect("created on an encrypted document");
    }
    let re = open_bytes_pw(save_bytes(&info.doc_id), Some("user-pw"));
    assert!(re.encrypted, "still encrypted");
    assert_eq!(re.permissions, info.permissions);
    let subtypes = annot_subtypes(&re.doc_id);
    assert!(
        subtypes.iter().any(|s| s == "Polygon") && subtypes.iter().any(|s| s == "Line"),
        "{subtypes:?}"
    );
    close(re.doc_id);
    close(info.doc_id);
}

/// annotation-line.pdf with an open password: its (third-party) `/Line` takes a 메모 or colour
/// patch without an error, as it did in v0.2, and stays a `/Line` in the encrypted file.
#[test]
fn editing_a_line_annotation_of_a_password_protected_document() {
    use seepdf_lib::engine::annot::update::update_in;
    use seepdf_lib::ipc::types::AnnotPatch;
    let bytes = protected(
        "annotation-line.pdf",
        "line-aes",
        Some("user-pw"),
        PermissionsRequest::default(),
    );
    let info = open_bytes_pw(bytes, Some("user-pw"));
    let annots = with_doc(&info.doc_id, |d| seepdf_lib::engine::annot::list(d, 0)).unwrap();
    let line = annots
        .iter()
        .find(|a| a.subtype == "Line")
        .expect("a /Line")
        .clone();
    for patch in [
        AnnotPatch {
            contents: Some("메모".into()),
            ..AnnotPatch::default()
        },
        AnnotPatch {
            color: Some([200, 0, 0]),
            ..AnnotPatch::default()
        },
    ] {
        let (d, id) = (info.doc_id.clone(), line.id.clone());
        with_state(move |st| update_in(st, &d, 0, &id, &patch))
            .expect("edit on an encrypted document");
    }
    let re = open_bytes_pw(save_bytes(&info.doc_id), Some("user-pw"));
    assert!(re.encrypted);
    let after = with_doc(&re.doc_id, |d| seepdf_lib::engine::annot::list(d, 0)).unwrap();
    let edited = after
        .iter()
        .find(|a| a.subtype == "Line")
        .expect("still a /Line");
    // Another application's line keeps its own appearance (move-only): the 메모 is what sticks.
    assert_eq!(edited.contents, "메모");
    close(re.doc_id);
    close(info.doc_id);
}

/// R5 on an encrypted document: the edited paragraph keeps its place in the reading order, in
/// memory and in the saved (still encrypted) file.
#[test]
fn paragraph_edit_on_a_password_protected_document_keeps_the_reading_order() {
    use seepdf_lib::engine::objects::paragraph;
    use seepdf_lib::ipc::types::{ParagraphEdit, ParagraphFlow};
    let bytes = protected_bytes(three_paragraphs(), "para-aes", Some("user-pw"));
    let info = open_bytes_pw(bytes, Some("user-pw"));
    assert!(info.encrypted);
    let doc = info.doc_id.clone();
    let probe = with_doc(&doc, |d| paragraph::probe(d, 0, [100.0, 603.0]))
        .expect("probe")
        .expect("paragraph");
    let edit = ParagraphEdit {
        object_ids: probe.object_ids.clone(),
        text: "Delta paragraph replaces the middle one".into(),
        width: None,
        font_size_pt: None,
        color: None,
        align: None,
        flow: Some(ParagraphFlow::Push),
        dry_run: false,
    };
    let (d, g) = (doc.clone(), probe.doc_generation);
    with_state(move |st| paragraph::edit(st, &d, 0, g, edit, false)).expect("edit");
    let check = |id: &str, when: &str| {
        let text = page_text(id);
        let (a, dd, c) = (text.find("Alpha"), text.find("Delta"), text.find("Charlie"));
        assert!(
            a.is_some() && dd.is_some() && c.is_some() && !text.contains("Bravo"),
            "{when}: {text:?}"
        );
        assert!(a < dd && dd < c, "{when}: reading order kept: {text:?}");
    };
    check(&doc, "in memory");
    let re = open_bytes_pw(save_bytes(&doc), Some("user-pw"));
    assert!(re.encrypted);
    check(&re.doc_id, "saved");
    close(re.doc_id);
    close(doc);
}

/// R1/R2 round 2 (split-stream join, `Tc` / `Tw` restore) on an encrypted document: one word
/// redacted out of 160F-2019.pdf (whose content streams break mid-object) with an open
/// password is applied as on the plain file — no other line is lost.
#[test]
fn word_redaction_on_a_password_protected_split_stream_page() {
    let encrypted = protected(
        "160F-2019.pdf",
        "160f-aes",
        Some("user-pw"),
        PermissionsRequest::default(),
    );
    for (label, bytes, pw) in [
        (
            "plain",
            std::fs::read(fixture("160F-2019.pdf")).unwrap(),
            None,
        ),
        ("encrypted", encrypted, Some("user-pw")),
    ] {
        let doc = open_bytes_pw(bytes, pw).doc_id;
        let word = "rémunérations";
        let before = page_text(&doc);
        let marks = {
            let w = word.to_string();
            with_doc(&doc, move |d| {
                let layer = layer::layer(d, 0)?;
                let spelled = |s: u32, n: u32| -> String {
                    layer.chars[s as usize..(s + n) as usize]
                        .iter()
                        .map(|c| char::from_u32(c.codepoint).unwrap_or('?'))
                        .collect()
                };
                let wd = layer
                    .words
                    .iter()
                    .find(|x| spelled(x.first_char, x.char_count) == w)
                    .expect("word");
                Ok(layer
                    .range_rects(wd.first_char, wd.char_count)
                    .into_iter()
                    .map(|r| {
                        let dx = 0.25f32.min((r.r - r.l) * 0.1);
                        Rect::new(r.l + dx, r.b, r.r - dx, r.t)
                    })
                    .collect::<Vec<_>>())
            })
            .unwrap()
        };
        let r = redact_rects(&doc, marks).unwrap_or_else(|e| panic!("{label}: {e:?}"));
        assert!(r.verified, "{label}");
        let saved = open_bytes_pw(save_bytes(&doc), pw);
        assert_eq!(saved.encrypted, pw.is_some(), "{label}");
        let after = page_text(&saved.doc_id);
        assert!(
            after.matches(word).count() < before.matches(word).count(),
            "{label}: redacted"
        );
        let lost: Vec<&str> = before
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.contains(word))
            .filter(|l| !after.lines().any(|a| a.trim() == *l))
            .collect();
        assert!(lost.is_empty(), "{label}: lines lost: {lost:?}");
        close(saved.doc_id);
        close(doc);
    }
}
