//! P1-1 (문서 속성 편집 / 메타데이터 제거) and P1-3 (암호 설정) — `IPC_CONTRACT.md` §7.5.
//!
//! Every file this writes is reopened with PDFium (what the app reads) and, where the claim is
//! about the file structure (no XMP, V5/R6 encryption dictionary), inspected with `lopdf`.

mod common;
use common::*;

use seepdf_lib::engine::registry;
use seepdf_lib::engine::render::tiles;
use seepdf_lib::engine::save;
use seepdf_lib::engine::security;
use seepdf_lib::ipc::types::{DocInfo, DocMeta, PermissionsRequest, SecurityRevision};
use seepdf_lib::ipc::{EngineError, ErrorCode};
use std::path::PathBuf;

fn out_dir() -> PathBuf {
    let dir = fixture("out").join("stage3");
    std::fs::create_dir_all(&dir).expect("create fixtures/out/stage3");
    dir
}

fn set_metadata(doc_id: &str, meta: DocMeta) -> Result<DocInfo, EngineError> {
    let doc_id = doc_id.to_string();
    with_state(move |st| security::set_metadata(st, &doc_id, &meta))
}

fn remove_metadata(doc_id: &str) -> Result<DocInfo, EngineError> {
    let doc_id = doc_id.to_string();
    with_state(move |st| security::remove_metadata(st, &doc_id))
}

fn protect(
    doc_id: &str,
    name: &str,
    user: Option<&str>,
    owner: &str,
    permissions: PermissionsRequest,
) -> Result<Vec<u8>, EngineError> {
    let target = out_dir().join(name);
    let _ = std::fs::remove_file(&target);
    let (doc_id, user, owner) = (
        doc_id.to_string(),
        user.map(str::to_owned),
        owner.to_string(),
    );
    let path = target.display().to_string();
    let written = with_state(move |st| {
        security::set_password(st, &doc_id, &path, user.as_deref(), &owner, permissions)
    })?;
    let bytes = std::fs::read(&target).expect("read the protected file");
    assert_eq!(written.bytes, bytes.len() as u64);
    Ok(bytes)
}

fn reopen(bytes: Vec<u8>, password: Option<&str>) -> Result<TestDoc, EngineError> {
    let password = password.map(str::to_owned);
    let info = with_state(move |st| registry::open(st, None, bytes, password))?;
    let doc_id = info.doc_id.clone();
    Ok(TestDoc { info, doc_id })
}

/// Whole page 0 at 1x, RGBA.
fn render_page0(doc_id: &str) -> (u32, u32, Vec<u8>) {
    let doc_id = doc_id.to_string();
    let buffer = with_state(move |st| tiles::render_raw_buffer(st, &doc_id, 0, 1.0, None))
        .expect("render page");
    let width = u32::from_le_bytes(buffer[8..12].try_into().unwrap());
    let height = u32::from_le_bytes(buffer[12..16].try_into().unwrap());
    (width, height, buffer[32..].to_vec())
}

/// Share of pixels that differ by more than 24 in any channel (same rule as tests/export.rs).
fn difference(a: &(u32, u32, Vec<u8>), b: &(u32, u32, Vec<u8>)) -> f64 {
    assert_eq!(
        (a.0, a.1),
        (b.0, b.1),
        "the two renders must be the same size"
    );
    let differing =
        a.2.chunks_exact(4)
            .zip(b.2.chunks_exact(4))
            .filter(|(pa, pb)| {
                pa.iter()
                    .zip(*pb)
                    .any(|(x, y)| (*x as i32 - *y as i32).abs() > 24)
            })
            .count();
    differing as f64 / (a.2.len() / 4) as f64
}

fn catalog_has_xmp(bytes: &[u8]) -> bool {
    let doc = lopdf::Document::load_mem(bytes).expect("lopdf parses the file");
    doc.catalog().expect("catalog").has(b"Metadata")
}

fn korean_meta() -> DocMeta {
    DocMeta {
        title: Some("SeePDF 제목 테스트".into()),
        author: Some("홍길동".into()),
        subject: Some("P1-1 메타데이터".into()),
        keywords: Some("pdf, 한글, test".into()),
        ..DocMeta::default()
    }
}

fn assert_korean_meta(meta: &DocMeta) {
    assert_eq!(meta.title.as_deref(), Some("SeePDF 제목 테스트"));
    assert_eq!(meta.author.as_deref(), Some("홍길동"));
    assert_eq!(meta.subject.as_deref(), Some("P1-1 메타데이터"));
    assert_eq!(meta.keywords.as_deref(), Some("pdf, 한글, test"));
}

// ---------------------------------------------------------------------------------------
// P1-1 metadata
// ---------------------------------------------------------------------------------------

#[test]
fn metadata_set_persists_and_undoes() {
    let doc = open("tracemonkey.pdf");
    let before = doc.info.meta.clone();
    assert_ne!(before.title.as_deref(), Some("SeePDF 제목 테스트"));

    let info = set_metadata(&doc.doc_id, korean_meta()).expect("set_metadata");
    assert_korean_meta(&info.meta);
    assert!(info.dirty, "a metadata edit makes the document dirty");
    assert!(info.can_undo, "a metadata edit is one undo step");
    assert_eq!(info.undo_label.as_deref(), Some("undo.metadataEdit"));
    assert_eq!(info.page_count, 14);
    assert!(info.doc_generation > doc.info.doc_generation);
    // Fields left `None` are kept; ModDate is stamped.
    assert_eq!(info.meta.creator, before.creator);
    assert_eq!(info.meta.producer, before.producer);
    let modified = info.meta.modified.clone().expect("ModDate is set");
    assert!(modified.starts_with("D:20"), "{modified}");

    // Save As, then a fresh open reads the Korean strings back through FPDF_GetMetaText.
    let target = out_dir().join("metadata-set.pdf");
    let _ = std::fs::remove_file(&target);
    let doc_id = doc.doc_id.clone();
    let path = target.display().to_string();
    with_state(move |st| save::save(st, &doc_id, Some(&path), false)).expect("save as");
    let saved = reopen(std::fs::read(&target).unwrap(), None).expect("reopen the saved file");
    assert_korean_meta(&saved.info.meta);
    assert_eq!(saved.info.page_count, 14);

    // Undo restores the previous /Info.
    let doc_id = doc.doc_id.clone();
    let undone = with_state(move |st| registry::undo(st, &doc_id, false)).expect("undo");
    assert_eq!(undone.meta.title, before.title);
    assert_eq!(undone.meta.author, before.author);
    assert_eq!(undone.meta.subject, before.subject);
    assert!(undone.can_redo);

    // An empty string removes that one key.
    let info = set_metadata(
        &doc.doc_id,
        DocMeta {
            title: Some("임시".into()),
            ..DocMeta::default()
        },
    )
    .unwrap();
    assert_eq!(info.meta.title.as_deref(), Some("임시"));
    let info = set_metadata(
        &doc.doc_id,
        DocMeta {
            title: Some(String::new()),
            ..DocMeta::default()
        },
    )
    .unwrap();
    assert_eq!(info.meta.title, None);
}

#[test]
fn metadata_remove_clears_info_and_xmp() {
    let original = std::fs::read(fixture("160F-2019.pdf")).unwrap();
    assert!(
        catalog_has_xmp(&original),
        "the fixture must carry an XMP packet"
    );
    let doc = open("160F-2019.pdf");
    let m = &doc.info.meta;
    assert!(
        [&m.title, &m.author, &m.creator, &m.producer, &m.created]
            .iter()
            .any(|f| f.is_some()),
        "the fixture must carry some /Info: {m:?}"
    );

    let info = remove_metadata(&doc.doc_id).expect("remove_metadata");
    let m = &info.meta;
    for (name, field) in [
        ("title", &m.title),
        ("author", &m.author),
        ("subject", &m.subject),
        ("keywords", &m.keywords),
        ("creator", &m.creator),
        ("producer", &m.producer),
        ("created", &m.created),
        ("modified", &m.modified),
    ] {
        assert_eq!(field, &None, "{name} must be gone");
    }
    assert!(info.dirty && info.can_undo);
    assert_eq!(info.undo_label.as_deref(), Some("undo.metadataRemove"));
    assert_eq!(info.page_count, doc.info.page_count);

    let target = out_dir().join("metadata-removed.pdf");
    let _ = std::fs::remove_file(&target);
    let doc_id = doc.doc_id.clone();
    let path = target.display().to_string();
    with_state(move |st| save::save(st, &doc_id, Some(&path), false)).expect("save as");
    let bytes = std::fs::read(&target).unwrap();
    assert!(
        !catalog_has_xmp(&bytes),
        "the catalog must not reference an XMP stream"
    );
    let parsed = lopdf::Document::load_mem(&bytes).unwrap();
    if let Ok(info) = parsed.trailer.get(b"Info").and_then(|o| o.as_reference()) {
        let dict = parsed.get_dictionary(info).expect("/Info dictionary");
        for key in [
            &b"Title"[..],
            b"Author",
            b"Subject",
            b"Keywords",
            b"Creator",
            b"CreationDate",
        ] {
            assert!(
                !dict.has(key),
                "/Info still has {}",
                String::from_utf8_lossy(key)
            );
        }
    }
    let saved = reopen(bytes, None).expect("reopen");
    assert_eq!(saved.info.meta.title, None);
    assert_eq!(saved.info.meta.author, None);
    assert!(saved.info.has_form, "the form survives the rewrite");
}

/// v0.3 S2: an encrypted document is no longer refused — the rewrite keeps its encryption
/// (see `structure_edits_on_rc4_and_after_unlock` and the other S2 tests below).
#[test]
fn metadata_on_encrypted_keeps_the_password() {
    let doc = try_open("gen/encrypted-rc4-40.pdf", Some("user")).expect("open with the password");
    let info = remove_metadata(&doc.doc_id).expect("remove_metadata on RC4");
    assert!(info.encrypted && info.meta.title.is_none());
    let bytes = with_doc(&doc.doc_id, |d| Ok(d.to_bytes()?.to_vec())).unwrap();
    assert!(
        reopen(bytes.clone(), None).is_err(),
        "still needs the password"
    );
    reopen(bytes, Some("user")).expect("opens with the same password");
    let err = set_metadata("no-such-doc", korean_meta()).expect_err("unknown doc");
    assert_eq!(err.code, ErrorCode::NotFound);
}

// ---------------------------------------------------------------------------------------
// P1-3 password
// ---------------------------------------------------------------------------------------

fn restricted() -> PermissionsRequest {
    PermissionsRequest {
        print: false,
        extract_text: false,
        ..PermissionsRequest::default()
    }
}

#[test]
fn password_aes256_roundtrip() {
    let doc = open("tracemonkey.pdf");
    let bytes = protect(
        &doc.doc_id,
        "protected-aes256.pdf",
        Some("user1"),
        "owner1",
        restricted(),
    )
    .expect("set_password");

    let parsed = lopdf::Document::load_mem(&bytes).expect("lopdf parses the file");
    assert!(parsed.is_encrypted());
    let encrypt = parsed.get_encrypted().expect("/Encrypt");
    assert_eq!(encrypt.get(b"V").and_then(|o| o.as_i64()).unwrap(), 5);
    assert_eq!(encrypt.get(b"R").and_then(|o| o.as_i64()).unwrap(), 6);

    let err = reopen(bytes.clone(), None)
        .err()
        .expect("no password must fail");
    assert!(err.is_password(), "{err:?}");
    let err = reopen(bytes.clone(), Some("nope"))
        .err()
        .expect("a wrong password must fail");
    assert!(err.is_password(), "{err:?}");

    let opened = reopen(bytes.clone(), Some("user1")).expect("opens with the user password");
    assert_eq!(opened.info.page_count, 14);
    assert!(opened.info.encrypted);
    let p = opened.info.permissions;
    assert!(!p.print, "{p:?}");
    assert!(!p.extract_text, "{p:?}");
    assert!(
        p.annotate && p.fill_forms && p.assemble && p.modify,
        "{p:?}"
    );
    // Stage 4: R6 is reported as such, not `unknown`.
    assert_eq!(p.revision, SecurityRevision::R6);

    let diff = difference(&render_page0(&doc.doc_id), &render_page0(&opened.doc_id));
    assert!(
        diff <= 0.005,
        "page 0 differs in {:.3} % of pixels",
        diff * 100.0
    );

    // The open document itself is untouched.
    let (encrypted, dirty) = with_doc(&doc.doc_id, |d| Ok((d.encrypted, d.dirty()))).unwrap();
    assert!(!encrypted && !dirty);
}

#[test]
fn password_owner_only() {
    let doc = open("tracemonkey.pdf");
    let bytes = protect(
        &doc.doc_id,
        "protected-owner-only.pdf",
        None,
        "owner1",
        restricted(),
    )
    .expect("set_password");
    let opened = reopen(bytes.clone(), None).expect("opens without a password");
    assert_eq!(opened.info.page_count, 14);
    assert!(opened.info.encrypted);
    assert!(!opened.info.permissions.print);
    assert!(!opened.info.permissions.extract_text);
    assert!(opened.info.permissions.annotate);
    assert_eq!(opened.info.permissions.revision, SecurityRevision::R6);
    // An empty user password is the same as none.
    let bytes = protect(
        &doc.doc_id,
        "protected-owner-only-2.pdf",
        Some(""),
        "owner1",
        restricted(),
    )
    .expect("set_password");
    reopen(bytes, None).expect("opens without a password");
}

#[test]
fn password_reprotect_encrypted_input() {
    // v0.3 S5: the RC4 fixture is restricted (/P -21: no printing), so re-protecting its
    // user-password open would drop the restriction — refused until unlocked.
    let restricted =
        try_open("gen/encrypted-rc4-40.pdf", Some("user")).expect("open with the password");
    assert!(!restricted.info.permissions.print);
    let err = protect(
        &restricted.doc_id,
        "never-reprotected.pdf",
        Some("n3w"),
        "owner-n3w",
        PermissionsRequest::default(),
    )
    .expect_err("a restricted open cannot change its security");
    assert_eq!(err.code, ErrorCode::PermissionDenied);

    // An encrypted input with every permission (AES-256, open password "user") re-protects.
    let plain = open("tracemonkey.pdf");
    let source = protect(
        &plain.doc_id,
        "reprotect-source.pdf",
        Some("user"),
        "owner",
        PermissionsRequest::default(),
    )
    .expect("set_password");
    let doc = reopen(source, Some("user")).expect("open with the password");
    let bytes = protect(
        &doc.doc_id,
        "reprotected.pdf",
        Some("n3w"),
        "owner-n3w",
        PermissionsRequest::default(),
    )
    .expect("set_password on an encrypted input");
    let opened = reopen(bytes.clone(), Some("n3w")).expect("opens with the new password");
    assert_eq!(opened.info.page_count, 14);
    let err = reopen(bytes.clone(), Some("user"))
        .err()
        .expect("the old password must fail");
    assert!(err.is_password(), "{err:?}");
    let err = reopen(bytes, None).err().expect("no password must fail");
    assert!(err.is_password(), "{err:?}");
}

#[test]
fn password_keeps_acroform() {
    let doc = open("160F-2019.pdf");
    assert!(doc.info.has_form);
    let fields_before = with_doc(&doc.doc_id, |d| {
        Ok(seepdf_lib::engine::form::list(d, None)?.len())
    })
    .unwrap();
    assert!(fields_before > 0);

    let bytes = protect(
        &doc.doc_id,
        "protected-form.pdf",
        Some("form"),
        "owner",
        PermissionsRequest::default(),
    )
    .expect("set_password");
    let opened = reopen(bytes, Some("form")).expect("opens");
    assert!(opened.info.has_form);
    assert_eq!(opened.info.page_count, doc.info.page_count);
    let fields_after = with_doc(&opened.doc_id, |d| {
        Ok(seepdf_lib::engine::form::list(d, None)?.len())
    })
    .unwrap();
    assert_eq!(fields_after, fields_before);
}

#[test]
fn password_rejects_empty_owner() {
    let doc = open("tracemonkey.pdf");
    let err = protect(
        &doc.doc_id,
        "never-written.pdf",
        Some("user"),
        "",
        PermissionsRequest::default(),
    )
    .expect_err("an empty owner password is invalid");
    assert_eq!(err.code, ErrorCode::InvalidArgument);
    let long = "x".repeat(128);
    let err = protect(
        &doc.doc_id,
        "never-written.pdf",
        Some(&long),
        "owner",
        PermissionsRequest::default(),
    )
    .expect_err("a 128-byte password is invalid");
    assert_eq!(err.code, ErrorCode::InvalidArgument);
    assert!(!out_dir().join("never-written.pdf").exists());
}

/// `permissions: Partial<Permissions>` — a flag the frontend leaves out is allowed, and the
/// read-only `revision` is ignored.
#[test]
fn permissions_request_partial_defaults_to_allowed() {
    let p: PermissionsRequest =
        serde_json::from_str(r#"{"print":false,"extractText":false,"revision":"r4"}"#).unwrap();
    assert_eq!(
        p,
        PermissionsRequest {
            print: false,
            extract_text: false,
            ..PermissionsRequest::default()
        }
    );
    let p: PermissionsRequest = serde_json::from_str("{}").unwrap();
    assert_eq!(p, PermissionsRequest::default());
    let bits = security::lopdf_permissions(p);
    assert!(bits
        .contains(lopdf::Permissions::COPYABLE_FOR_ACCESSIBILITY | lopdf::Permissions::PRINTABLE));
    let bits = security::lopdf_permissions(restricted());
    assert!(!bits.contains(lopdf::Permissions::PRINTABLE));
    assert!(!bits.contains(lopdf::Permissions::COPYABLE));
    assert!(bits.contains(lopdf::Permissions::COPYABLE_FOR_ACCESSIBILITY));
}

/// PDF text strings and dates as PDFium reads them back.
#[test]
fn pdf_text_string_and_date_format() {
    match save::pdf_text_string("Plain ASCII") {
        lopdf::Object::String(bytes, lopdf::StringFormat::Literal) => {
            assert_eq!(bytes, b"Plain ASCII")
        }
        other => panic!("{other:?}"),
    }
    match save::pdf_text_string("한") {
        lopdf::Object::String(bytes, _) => assert_eq!(bytes, vec![0xFE, 0xFF, 0xD5, 0x5C]),
        other => panic!("{other:?}"),
    }
    let date = save::pdf_date_now();
    // D:YYYYMMDDHHmmSS+HH'mm'
    assert_eq!(date.len(), 23, "{date}");
    assert!(date.starts_with("D:20") && date.ends_with('\''), "{date}");
    assert!(matches!(date.as_bytes()[16], b'+' | b'-'), "{date}");
}

// =======================================================================================
// v0.3 pkg3-security-save-integrity — S1 signatures, S2 encrypted rewrites, S3 sanitize,
// S4 attachments, S5 permissions
// =======================================================================================

use lopdf::encryption::crypt_filters::Aes128CryptFilter;
use lopdf::{dictionary, Object, ObjectId, Stream, StringFormat};
use seepdf_lib::engine::registry::MutateOpts;
use seepdf_lib::engine::{annot, attachments, export, sanitize, structure};
use seepdf_lib::ipc::types::{
    AnnotSpec, ChangeReason, MarkupSpec, OutlineNode, PageLabelRange, PageLabelStyle, Rect,
    SanitizeOptions,
};

fn v3_dir() -> PathBuf {
    let dir = fixture("out").join("v03-security");
    std::fs::create_dir_all(&dir).expect("create fixtures/out/v03-security");
    dir
}

/// A one-page Helvetica document built with lopdf; `extra` adds whatever the test needs to
/// the document, its page and its catalog before it is written.
fn build_pdf(extra: impl FnOnce(&mut lopdf::Document, ObjectId, ObjectId)) -> Vec<u8> {
    let mut doc = lopdf::Document::with_version("1.7");
    let pages_id = doc.new_object_id();
    let font_id = doc.add_object(dictionary! {
        "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica",
    });
    let content = b"BT /F1 24 Tf 72 700 Td (SeePDF security fixture) Tj ET".to_vec();
    let content_id = doc.add_object(Stream::new(dictionary! {}, content));
    let page_id = doc.add_object(dictionary! {
        "Type" => "Page",
        "Parent" => pages_id,
        "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
        "Contents" => content_id,
        "Resources" => dictionary! { "Font" => dictionary! { "F1" => font_id } },
    });
    doc.objects.insert(
        pages_id,
        Object::Dictionary(dictionary! {
            "Type" => "Pages", "Kids" => vec![page_id.into()], "Count" => 1,
        }),
    );
    let catalog_id = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
    doc.trailer.set("Root", catalog_id);
    let id = Object::String(b"SeePDF-pkg3-0001".to_vec(), StringFormat::Hexadecimal);
    doc.trailer.set("ID", Object::Array(vec![id.clone(), id]));
    extra(&mut doc, page_id, catalog_id);
    let mut out = Vec::new();
    doc.save_to(&mut out).expect("write the fixture");
    out
}

/// A document with one signed signature field (`/Contents` is a dummy blob: PDFium detects
/// signatures, it does not validate them — and neither does SeePDF).
fn signed_pdf() -> Vec<u8> {
    build_pdf(|doc, page, catalog| {
        let sig = doc.add_object(dictionary! {
            "Type" => "Sig",
            "Filter" => "Adobe.PPKLite",
            "SubFilter" => "adbe.pkcs7.detached",
            "Reason" => save::pdf_text_string("계약 승인"),
            "M" => Object::string_literal("D:20260901120000+09'00'"),
            "ByteRange" => vec![0.into(), 0.into(), 0.into(), 0.into()],
            "Contents" => Object::String(vec![0x30, 0x82, 0x01, 0x00], StringFormat::Hexadecimal),
        });
        let field = doc.add_object(dictionary! {
            "Type" => "Annot",
            "Subtype" => "Widget",
            "FT" => "Sig",
            "T" => Object::string_literal("Signature1"),
            "V" => sig,
            "Rect" => vec![0.into(), 0.into(), 0.into(), 0.into()],
            "F" => 132,
            "P" => page,
        });
        let empty_field = doc.add_object(dictionary! {
            "Type" => "Annot",
            "Subtype" => "Widget",
            "FT" => "Sig",
            "T" => Object::string_literal("Unsigned"),
            "Rect" => vec![0.into(), 0.into(), 0.into(), 0.into()],
            "F" => 132,
            "P" => page,
        });
        doc.get_dictionary_mut(page)
            .unwrap()
            .set("Annots", vec![field.into(), empty_field.into()]);
        let catalog = doc.get_dictionary_mut(catalog).unwrap();
        catalog.set(
            "AcroForm",
            dictionary! { "Fields" => vec![field.into(), empty_field.into()], "SigFlags" => 3 },
        );
    })
}

/// Opens `bytes` written to `fixtures/out/v03-security/<name>` from that path (a signed
/// document only keeps its incremental base when it has a path).
fn open_written(name: &str, bytes: &[u8], password: Option<&str>) -> (TestDoc, PathBuf) {
    let path = v3_dir().join(name);
    std::fs::write(&path, bytes).expect("write the fixture");
    let owned = path.clone();
    let bytes = bytes.to_vec();
    let password = password.map(str::to_owned);
    let info = with_state(move |st| registry::open(st, Some(owned), bytes, password))
        .expect("open the written fixture");
    let doc_id = info.doc_id.clone();
    (TestDoc { info, doc_id }, path)
}

fn highlight(doc_id: &str) -> Result<String, EngineError> {
    let doc_id = doc_id.to_string();
    with_state(move |st| {
        registry::mutate(
            st,
            &doc_id,
            MutateOpts::new("undo.annotCreate", ChangeReason::Edit).page(0),
            |doc| {
                annot::create::create(
                    doc,
                    0,
                    &AnnotSpec::Highlight(MarkupSpec {
                        rects: vec![Rect::new(72.0, 695.0, 300.0, 720.0)],
                        color: [255, 235, 0],
                        opacity: 0.6,
                        contents: Some("형광펜".into()),
                    }),
                    None,
                )
            },
        )
    })
}

fn annot_count(doc_id: &str) -> usize {
    with_doc(doc_id, |d| Ok(annot::list(d, 0)?.len())).unwrap()
}

fn save_in_place(doc_id: &str) -> Result<(), EngineError> {
    let doc_id = doc_id.to_string();
    with_state(move |st| {
        save::save_with(st, &doc_id, None, &save::SaveOptions::default()).map(|_| ())
    })
}

fn info_of(doc_id: &str) -> DocInfo {
    with_doc(doc_id, |d| Ok(d.info())).unwrap()
}

// ---------------------------------------------------------------------------------------
// S1
// ---------------------------------------------------------------------------------------

#[test]
fn signed_document_is_detected() {
    let (doc, _) = open_written("signed-detect.pdf", &signed_pdf(), None);
    let sigs = &doc.info.signatures;
    assert_eq!(
        sigs.len(),
        1,
        "the unsigned field is not a signature: {sigs:?}"
    );
    let s = &sigs[0];
    assert_eq!(s.field_name.as_deref(), Some("Signature1"));
    assert_eq!(s.reason.as_deref(), Some("계약 승인"));
    assert_eq!(s.sub_filter.as_deref(), Some("adbe.pkcs7.detached"));
    assert!(
        s.time
            .as_deref()
            .is_some_and(|t| t.starts_with("D:20260901")),
        "{s:?}"
    );
    assert_eq!(doc.info.incremental_save, Some(true));

    let plain = open("tracemonkey.pdf");
    assert!(plain.info.signatures.is_empty());
    assert_eq!(plain.info.incremental_save, None);
    let json = serde_json::to_value(&plain.info).unwrap();
    assert!(json.get("signatures").is_none() && json.get("incrementalSave").is_none());
}

#[test]
fn signed_document_saves_incrementally() {
    let original = signed_pdf();
    let (doc, path) = open_written("signed-incremental.pdf", &original, None);
    highlight(&doc.doc_id).expect("annotate");
    assert_eq!(info_of(&doc.doc_id).incremental_save, Some(true));
    save_in_place(&doc.doc_id).expect("save");

    let first = std::fs::read(&path).unwrap();
    assert!(first.len() > original.len(), "an update was appended");
    assert!(
        first.starts_with(&original),
        "the signed revision must stay byte-identical"
    );
    // The saved file still has the signature and the new annotation.
    let (again, _) = open_written("signed-incremental-reopen.pdf", &first, None);
    assert_eq!(again.info.signatures.len(), 1);
    assert_eq!(annot_count(&again.doc_id), annot_count(&doc.doc_id));

    // A second save appends to the first.
    highlight(&doc.doc_id).expect("annotate again");
    save_in_place(&doc.doc_id).expect("save again");
    let second = std::fs::read(&path).unwrap();
    assert!(second.starts_with(&first) && second.starts_with(&original));
}

#[test]
fn signed_document_after_a_rewrite_saves_in_full() {
    let original = signed_pdf();
    let (doc, path) = open_written("signed-rewrite.pdf", &original, None);
    set_metadata(&doc.doc_id, korean_meta()).expect("a lopdf rewrite");
    assert_eq!(
        info_of(&doc.doc_id).incremental_save,
        Some(false),
        "after a lopdf rewrite only a full save is possible"
    );
    // Undo back to the file as it is on disk: incremental again.
    let d = doc.doc_id.clone();
    with_state(move |st| registry::undo(st, &d, false)).expect("undo");
    assert_eq!(info_of(&doc.doc_id).incremental_save, Some(true));
    let d = doc.doc_id.clone();
    with_state(move |st| registry::undo(st, &d, true)).expect("redo");
    save_in_place(&doc.doc_id).expect("full save");
    let saved = std::fs::read(&path).unwrap();
    assert!(!saved.starts_with(&original), "a full rewrite");
    // The saved file is the new base: the next PDFium-only edit is incremental again.
    assert_eq!(info_of(&doc.doc_id).incremental_save, Some(true));
}

/// Verification round 1: the viewer lists annotations on open, which stamps `/NM` on the
/// signature widget (`ids_stamped`). The first edit's undo snapshot is then serialised, and it
/// must still start with the signed bytes, or one edit + undo turns saving into a rewrite.
#[test]
fn signed_document_viewed_edited_and_undone_stays_incremental() {
    let original = signed_pdf();
    let (doc, path) = open_written("signed-viewed-undo.pdf", &original, None);
    let id = doc.doc_id.clone();
    let undo = |redo: bool| {
        let d = id.clone();
        with_state(move |st| registry::undo(st, &d, redo)).expect("undo / redo");
    };
    let widgets = annot_count(&id); // the signature widgets, now stamped with an /NM
    assert!(widgets >= 1);
    assert_eq!(info_of(&id).incremental_save, Some(true));

    // One edit + undo.
    highlight(&id).expect("annotate");
    undo(false);
    assert_eq!(
        info_of(&id).incremental_save,
        Some(true),
        "one edit + undo on a viewed signed document"
    );
    assert_eq!(annot_count(&id), widgets);

    // Edit, edit, undo, redo.
    highlight(&id).expect("annotate");
    highlight(&id).expect("annotate again");
    undo(false);
    assert_eq!(
        info_of(&id).incremental_save,
        Some(true),
        "after edit, edit, undo"
    );
    undo(true);
    assert_eq!(info_of(&id).incremental_save, Some(true), "after redo");
    assert_eq!(annot_count(&id), widgets + 2);

    // A lopdf rewrite is a full save; undoing it restores the appended snapshot.
    set_metadata(&id, korean_meta()).expect("a lopdf rewrite");
    assert_eq!(info_of(&id).incremental_save, Some(false));
    undo(false);
    assert_eq!(
        info_of(&id).incremental_save,
        Some(true),
        "undo of a rewrite"
    );

    save_in_place(&id).expect("save");
    let saved = std::fs::read(&path).unwrap();
    assert!(
        saved.starts_with(&original),
        "the signed revision must stay byte-identical"
    );
    let (again, _) = open_written("signed-viewed-undo-reopen.pdf", &saved, None);
    assert_eq!(again.info.signatures.len(), 1);
    assert_eq!(annot_count(&again.doc_id), widgets + 2);
}

// ---------------------------------------------------------------------------------------
// S5
// ---------------------------------------------------------------------------------------

fn protect_bytes(user: Option<&str>, owner: &str, permissions: PermissionsRequest) -> Vec<u8> {
    let doc = open("tracemonkey.pdf");
    protect(
        &doc.doc_id,
        &format!("perm-{}.pdf", uuid_like()),
        user,
        owner,
        permissions,
    )
    .expect("set_password")
}

fn uuid_like() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    format!(
        "{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    )
}

fn unlock(doc_id: &str, password: &str) -> Result<DocInfo, EngineError> {
    let (doc_id, password) = (doc_id.to_string(), password.to_string());
    with_state(move |st| security::unlock(st, &doc_id, &password))
}

#[test]
fn permissions_are_enforced_and_unlock_restores_them() {
    let bytes = protect_bytes(
        None,
        "owner-pw",
        PermissionsRequest {
            annotate: false,
            ..PermissionsRequest::default()
        },
    );
    let doc = reopen(bytes, None).expect("opens without a password");
    assert!(!doc.info.permissions.annotate);

    let err = highlight(&doc.doc_id).expect_err("annotate is forbidden");
    assert_eq!(err.code, ErrorCode::PermissionDenied);
    assert_eq!(err.detail.as_deref(), Some("annotate"));
    // A permitted edit (assemble) goes through and is one undo step.
    let d = doc.doc_id.clone();
    with_state(move |st| {
        seepdf_lib::engine::pages::apply_ops(
            st,
            &d,
            vec![seepdf_lib::ipc::types::PageOp::Rotate {
                pages: vec![0],
                delta: 90,
            }],
        )
    })
    .expect("rotate is allowed");

    assert_eq!(
        unlock(&doc.doc_id, "wrong").unwrap_err().code,
        ErrorCode::PasswordWrong
    );
    let info = unlock(&doc.doc_id, "owner-pw").expect("unlock");
    assert_eq!(info.doc_id, doc.doc_id, "same docId");
    assert!(info.permissions.annotate && info.permissions.print);
    assert!(info.can_undo, "the undo history survives the unlock");
    assert!(info.dirty);
    highlight(&doc.doc_id).expect("annotate after unlocking");

    let plain = open("tracemonkey.pdf");
    assert_eq!(
        unlock(&plain.doc_id, "x").unwrap_err().code,
        ErrorCode::InvalidArgument
    );
}

#[test]
fn print_and_copy_permissions_are_enforced() {
    let bytes = protect_bytes(
        None,
        "owner-pw",
        PermissionsRequest {
            print: false,
            extract_text: false,
            modify: false,
            ..PermissionsRequest::default()
        },
    );
    let doc = reopen(bytes, None).expect("opens");
    let d = doc.doc_id.clone();
    let err = with_state(move |st| export::print_prepare(st, &d, None)).unwrap_err();
    assert_eq!(err.code, ErrorCode::PermissionDenied);
    assert_eq!(err.detail.as_deref(), Some("print"));
    let d = doc.doc_id.clone();
    let out = v3_dir().join("never.txt").display().to_string();
    let err = with_state(move |st| export::export_text(st, &d, &[0], &out)).unwrap_err();
    assert_eq!(err.code, ErrorCode::PermissionDenied);
    assert_eq!(err.detail.as_deref(), Some("extractText"));
    // modify = false: the text-edit probe names the reason, lopdf rewrites are refused.
    let probe = with_doc(&doc.doc_id, |d| {
        seepdf_lib::engine::objects::probe(d, 0, 0, "x")
    })
    .unwrap();
    assert_eq!(
        probe.reason,
        Some(seepdf_lib::ipc::types::NotEditableReason::Permissions)
    );
    let err = set_metadata(&doc.doc_id, korean_meta()).unwrap_err();
    assert_eq!(err.code, ErrorCode::PermissionDenied);

    // Changing the security of a restricted open would hand out an unrestricted copy.
    let d = doc.doc_id.clone();
    let out = v3_dir().join("never-unlocked.pdf").display().to_string();
    let err = with_state(move |st| {
        security::set_password(
            st,
            &d,
            &out,
            None,
            "new-owner",
            PermissionsRequest::default(),
        )
    })
    .unwrap_err();
    assert_eq!(err.code, ErrorCode::PermissionDenied);
    assert_eq!(err.detail.as_deref(), Some("security"));

    unlock(&doc.doc_id, "owner-pw").expect("unlock");
    let d = doc.doc_id.clone();
    with_state(move |st| export::print_prepare(st, &d, None)).expect("print after unlocking");
    let d = doc.doc_id.clone();
    let out = v3_dir()
        .join("reprotected-after-unlock.pdf")
        .display()
        .to_string();
    with_state(move |st| {
        security::set_password(
            st,
            &d,
            &out,
            None,
            "new-owner",
            PermissionsRequest::default(),
        )
    })
    .expect("set_password after unlocking");
}

// ---------------------------------------------------------------------------------------
// S2
// ---------------------------------------------------------------------------------------

/// `plain` encrypted with AES-128 (V4 / R4, `/StdCF` AESV2) by lopdf.
fn encrypt_aes128(plain: &[u8], user: &str, owner: &str, p: PermissionsRequest) -> Vec<u8> {
    use lopdf::encryption::crypt_filters::CryptFilter;
    use std::collections::BTreeMap;
    use std::sync::Arc;
    let mut doc = lopdf::Document::load_mem(plain).unwrap();
    let filter: Arc<dyn CryptFilter> = Arc::new(Aes128CryptFilter);
    let state = lopdf::EncryptionState::try_from(lopdf::EncryptionVersion::V4 {
        document: &doc,
        encrypt_metadata: true,
        crypt_filters: BTreeMap::from([(b"StdCF".to_vec(), filter)]),
        stream_filter: b"StdCF".to_vec(),
        string_filter: b"StdCF".to_vec(),
        owner_password: owner,
        user_password: user,
        permissions: security::lopdf_permissions(p),
    })
    .unwrap();
    doc.encrypt(&state).unwrap();
    let mut out = Vec::new();
    doc.save_to(&mut out).unwrap();
    out
}

fn two_page_pdf() -> Vec<u8> {
    // tracemonkey through PDFium: a real-world file with fonts, images and an /ID.
    std::fs::read(fixture("tracemonkey.pdf")).unwrap()
}

/// Every structure edit, then Save As and a fresh open with `password`.
fn edit_structure_and_reopen(doc_id: &str, name: &str, password: Option<&str>) -> TestDoc {
    let d = doc_id.to_string();
    let nodes = vec![OutlineNode {
        title: "1장 개요".into(),
        page: Some(0),
        dest: None,
        url: None,
        open: None,
        children: vec![],
    }];
    with_state(move |st| structure::outline::set_outline(st, &d, &nodes)).expect("set_outline");
    set_metadata(doc_id, korean_meta()).expect("set_metadata");
    let d = doc_id.to_string();
    let ranges = vec![PageLabelRange {
        start: 0,
        style: PageLabelStyle::RomanUpper,
        prefix: None,
        first: None,
    }];
    with_state(move |st| structure::labels::set_page_labels(st, &d, &ranges))
        .expect("set_page_labels");
    let parent = highlight(doc_id).expect("highlight");
    let d = doc_id.to_string();
    with_state(move |st| annot::reply::reply(st, &d, 0, &parent, "답글입니다", Some("검토자")))
        .expect("reply");
    let d = doc_id.to_string();
    let labels = with_state(move |st| structure::labels::get_page_labels(st, &d))
        .expect("page labels read back on an encrypted document");
    assert_eq!(labels.len(), 1);

    let target = v3_dir().join(name);
    let _ = std::fs::remove_file(&target);
    let (d, p) = (doc_id.to_string(), target.display().to_string());
    with_state(move |st| save::save(st, &d, Some(&p), false)).expect("save as");
    let reopened =
        reopen(std::fs::read(&target).unwrap(), password).expect("reopen with the password");
    assert_korean_meta(&reopened.info.meta);
    assert!(reopened.info.has_outline);
    let outline = with_doc(&reopened.doc_id, |d| Ok(d.outline())).unwrap();
    assert_eq!(outline[0].title, "1장 개요");
    assert_eq!(reopened.info.pages[0].label.as_deref(), Some("I"));
    let annots = with_doc(&reopened.doc_id, |d| annot::list(d, 0)).unwrap();
    assert!(
        annots.iter().any(|a| a.contents == "답글입니다"),
        "the reply survives: {annots:?}"
    );
    reopened
}

#[test]
fn structure_edits_on_a_permission_only_aes128_file() {
    let restricted = PermissionsRequest {
        print: false,
        ..PermissionsRequest::default()
    };
    let bytes = encrypt_aes128(&two_page_pdf(), "", "owner-128", restricted);
    let doc = reopen(bytes, None).expect("a permission-only file opens without a password");
    assert!(doc.info.encrypted);
    assert_eq!(doc.info.permissions.revision, SecurityRevision::R4);
    let before = doc.info.permissions;
    assert!(!before.print && before.modify);

    let reopened = edit_structure_and_reopen(&doc.doc_id, "s2-aes128.pdf", None);
    assert!(reopened.info.encrypted, "still encrypted");
    assert_eq!(reopened.info.permissions, before, "permissions unchanged");
    let bytes = std::fs::read(v3_dir().join("s2-aes128.pdf")).unwrap();
    let parsed = lopdf::Document::load_mem_with_options(
        &bytes,
        lopdf::LoadOptions::with_password("owner-128"),
    )
    .unwrap();
    assert!(
        parsed.encryption_state.is_some(),
        "the owner password still works"
    );
}

#[test]
fn structure_edits_on_a_user_password_aes256_file() {
    let bytes = protect_bytes(Some("user-256"), "owner-256", PermissionsRequest::default());
    let doc = reopen(bytes, Some("user-256")).expect("opens with the user password");
    assert_eq!(doc.info.permissions.revision, SecurityRevision::R6);
    let before = doc.info.permissions;
    let reopened = edit_structure_and_reopen(&doc.doc_id, "s2-aes256.pdf", Some("user-256"));
    assert_eq!(reopened.info.permissions, before);
    let bytes = std::fs::read(v3_dir().join("s2-aes256.pdf")).unwrap();
    assert!(
        reopen(bytes, None).is_err(),
        "still needs the open password"
    );
}

#[test]
fn structure_edits_on_rc4_and_after_unlock() {
    // R2 / RC4-40 with an open password.
    let doc = try_open("gen/encrypted-rc4-40.pdf", Some("user")).expect("open");
    let info = set_metadata(&doc.doc_id, korean_meta()).expect("metadata on RC4");
    assert!(info.encrypted);
    assert_korean_meta(&info.meta);

    // Permission-only with modify = false: refused until unlocked, then written with the
    // original restrictions intact.
    let restricted = PermissionsRequest {
        modify: false,
        ..PermissionsRequest::default()
    };
    let bytes = encrypt_aes128(&two_page_pdf(), "", "owner-m", restricted);
    let doc = reopen(bytes, None).expect("opens");
    let err = set_metadata(&doc.doc_id, korean_meta()).unwrap_err();
    assert_eq!(err.code, ErrorCode::PermissionDenied);
    assert!(!with_doc(&doc.doc_id, |d| Ok(d.history.can_undo())).unwrap());
    unlock(&doc.doc_id, "owner-m").expect("unlock");
    set_metadata(&doc.doc_id, korean_meta()).expect("allowed after unlocking");
    let target = v3_dir().join("s2-unlocked.pdf");
    let (d, p) = (doc.doc_id.clone(), target.display().to_string());
    with_state(move |st| save::save(st, &d, Some(&p), false)).expect("save as");
    let again = reopen(std::fs::read(&target).unwrap(), None).expect("reopen");
    assert!(
        !again.info.permissions.modify,
        "the file keeps its restriction"
    );
    assert_korean_meta(&again.info.meta);
}

// ---------------------------------------------------------------------------------------
// S3
// ---------------------------------------------------------------------------------------

/// JavaScript (document-level + /OpenAction + a page /AA), an embedded file, a
/// FileAttachment annotation, XMP + /Info, and a hidden layer that draws a blue square.
fn risky_pdf() -> Vec<u8> {
    build_pdf(|doc, page, catalog| {
        let js = doc.add_object(
            dictionary! { "S" => "JavaScript", "JS" => Object::string_literal("app.alert('hi');") },
        );
        let open_js = doc.add_object(
            dictionary! { "S" => "JavaScript", "JS" => Object::string_literal("this.print();") },
        );
        let file = doc.add_object(Stream::new(
            dictionary! { "Type" => "EmbeddedFile" },
            b"secret spreadsheet".to_vec(),
        ));
        let filespec = doc.add_object(dictionary! {
            "Type" => "Filespec", "F" => Object::string_literal("secret.txt"),
            "EF" => dictionary! { "F" => file },
        });
        let annot_file = doc.add_object(Stream::new(
            dictionary! { "Type" => "EmbeddedFile" },
            b"annotation payload".to_vec(),
        ));
        let annot_spec = doc.add_object(dictionary! {
            "Type" => "Filespec", "F" => Object::string_literal("clip.txt"),
            "EF" => dictionary! { "F" => annot_file },
        });
        let clip = doc.add_object(dictionary! {
            "Type" => "Annot", "Subtype" => "FileAttachment",
            "Rect" => vec![500.into(), 700.into(), 520.into(), 720.into()],
            "FS" => annot_spec, "Name" => "PushPin", "NM" => Object::string_literal("clip-1"),
        });
        let xmp = doc.add_object(Stream::new(
            dictionary! { "Type" => "Metadata", "Subtype" => "XML" },
            b"<x:xmpmeta xmlns:x='adobe:ns:meta/'/>".to_vec(),
        ));
        let info = doc.add_object(dictionary! { "Author" => Object::string_literal("Hong") });
        doc.trailer.set("Info", info);
        // A layer that is off by default, drawing a blue square.
        let ocg = doc.add_object(
            dictionary! { "Type" => "OCG", "Name" => Object::string_literal("Hidden notes") },
        );
        let content_id = doc
            .get_dictionary(page)
            .unwrap()
            .get(b"Contents")
            .unwrap()
            .as_reference()
            .unwrap();
        let hidden = b"\n/OC /L1 BDC 0 0 1 rg 100 100 200 200 re f EMC\n";
        if let Ok(Object::Stream(s)) = doc.get_object_mut(content_id) {
            let mut content = s.content.clone();
            content.extend_from_slice(hidden);
            s.set_content(content);
        }
        let p = doc.get_dictionary_mut(page).unwrap();
        p.set("Annots", vec![clip.into()]);
        p.set("AA", dictionary! { "O" => js });
        if let Ok(Object::Dictionary(res)) = p.get_mut(b"Resources") {
            res.set("Properties", dictionary! { "L1" => ocg });
        }
        let c = doc.get_dictionary_mut(catalog).unwrap();
        c.set("OpenAction", open_js);
        c.set("Metadata", xmp);
        c.set(
            "Names",
            dictionary! {
                "JavaScript" => dictionary! { "Names" => vec![Object::string_literal("init"), js.into()] },
                "EmbeddedFiles" => dictionary! { "Names" => vec![Object::string_literal("secret.txt"), filespec.into()] },
            },
        );
        c.set(
            "OCProperties",
            dictionary! {
                "OCGs" => vec![ocg.into()],
                "D" => dictionary! { "OFF" => vec![ocg.into()], "Order" => vec![ocg.into()] },
            },
        );
    })
}

/// Pixels of `rgba` inside the hidden square (100..300 pt at 1x) that are clearly blue.
fn blue_pixels(page: &(u32, u32, Vec<u8>)) -> usize {
    let (w, h, px) = page;
    let mut n = 0;
    for y in 0..*h {
        for x in 0..*w {
            let i = ((y * w + x) * 4) as usize;
            let (r, g, b) = (px[i], px[i + 1], px[i + 2]);
            if b > 200 && r < 60 && g < 60 {
                n += 1;
            }
        }
    }
    n
}

fn dangerous_objects(bytes: &[u8]) -> (usize, usize, usize, bool, bool) {
    let doc = lopdf::Document::load_mem(bytes).unwrap();
    let (mut js, mut files, mut clips) = (0, 0, 0);
    for object in doc.objects.values() {
        let dict = match object {
            Object::Dictionary(d) => d,
            Object::Stream(s) => &s.dict,
            _ => continue,
        };
        if dict.get(b"S").and_then(Object::as_name).ok() == Some(b"JavaScript") || dict.has(b"JS") {
            js += 1;
        }
        if dict.get(b"Type").and_then(Object::as_name).ok() == Some(b"EmbeddedFile") {
            files += 1;
        }
        if dict.get(b"Subtype").and_then(Object::as_name).ok() == Some(b"FileAttachment") {
            clips += 1;
        }
    }
    let catalog = doc.catalog().unwrap();
    (
        js,
        files,
        clips,
        catalog.has(b"OCProperties"),
        catalog.has(b"Metadata") || doc.trailer.has(b"Info"),
    )
}

#[test]
fn sanitize_removes_scripts_files_actions_metadata_and_hidden_layers() {
    let bytes = risky_pdf();
    let (js, files, clips, layers, meta) = dangerous_objects(&bytes);
    assert!(js >= 2 && files == 2 && clips == 1 && layers && meta);
    let doc = reopen(bytes, None).expect("the fixture opens");
    assert_eq!(doc.info.attachment_count, 1);
    let before = render_page0(&doc.doc_id);
    assert_eq!(blue_pixels(&before), 0, "the layer is hidden by default");

    let d = doc.doc_id.clone();
    let result = with_state(move |st| sanitize::sanitize(st, &d, &SanitizeOptions::default()))
        .expect("sanitize");
    let r = &result.removed;
    assert_eq!(r.javascript, 2, "{r:?}"); // the name-tree script and the /OpenAction
    assert_eq!(r.attachments, 2, "{r:?}"); // the embedded file and the paperclip
    assert_eq!(r.actions, 1, "{r:?}"); // the page /AA
    assert!(r.metadata >= 2, "{r:?}"); // /Info and the XMP stream
    assert_eq!(r.hidden_layers, 1, "{r:?}");
    assert_eq!(result.info.undo_label.as_deref(), Some("undo.sanitize"));
    assert_eq!(result.info.attachment_count, 0);
    assert_eq!(result.info.meta.author, None);

    let after_bytes = with_doc(&doc.doc_id, |d| Ok(d.to_bytes()?.to_vec())).unwrap();
    assert_eq!(dangerous_objects(&after_bytes), (0, 0, 0, false, false));
    let after = render_page0(&doc.doc_id);
    assert_eq!(
        blue_pixels(&after),
        0,
        "hidden content is deleted, not revealed"
    );
    assert!(
        difference(&before, &after) < 0.005,
        "the visible page is unchanged"
    );

    // Nothing left: no second undo step.
    let d = doc.doc_id.clone();
    let again =
        with_state(move |st| sanitize::sanitize(st, &d, &SanitizeOptions::default())).unwrap();
    assert_eq!(again.removed.total(), 0);
    assert_eq!(again.info.doc_generation, result.info.doc_generation);

    // One undo step brings everything back.
    let d = doc.doc_id.clone();
    let undone = with_state(move |st| registry::undo(st, &d, false)).unwrap();
    assert_eq!(undone.attachment_count, 1);
}

#[test]
fn sanitize_options_are_independent() {
    let doc = reopen(risky_pdf(), None).unwrap();
    let d = doc.doc_id.clone();
    let only_js = SanitizeOptions {
        javascript: true,
        attachments: false,
        actions: false,
        metadata: false,
        hidden_layers: false,
    };
    let result = with_state(move |st| sanitize::sanitize(st, &d, &only_js)).unwrap();
    // The page /AA stays, minus its JavaScript entry.
    assert_eq!(result.removed.javascript, 3, "{:?}", result.removed);
    assert_eq!(result.removed.total(), 3);
    let bytes = with_doc(&doc.doc_id, |d| Ok(d.to_bytes()?.to_vec())).unwrap();
    let (js, files, clips, layers, meta) = dangerous_objects(&bytes);
    assert_eq!((js, files, clips, layers, meta), (0, 2, 1, true, true));
}

#[test]
fn sanitize_keeps_an_encrypted_file_encrypted() {
    let risky = risky_pdf();
    let bytes = encrypt_aes128(&risky, "", "owner-s", PermissionsRequest::default());
    let doc = reopen(bytes, None).unwrap();
    let before = doc.info.permissions;
    let d = doc.doc_id.clone();
    let result = with_state(move |st| sanitize::sanitize(st, &d, &SanitizeOptions::default()))
        .expect("sanitize an encrypted file");
    assert!(result.removed.javascript >= 2);
    assert!(result.info.encrypted);
    assert_eq!(result.info.permissions, before);
}

// ---------------------------------------------------------------------------------------
// S4
// ---------------------------------------------------------------------------------------

#[test]
fn attachments_add_list_save_delete() {
    let doc = open("tracemonkey.pdf");
    let source = v3_dir().join("첨부 원본.bin");
    let payload: Vec<u8> = (0..70_000u32).map(|i| (i * 31 % 251) as u8).collect();
    std::fs::write(&source, &payload).unwrap();
    let list = {
        let (d, p) = (doc.doc_id.clone(), source.display().to_string());
        with_state(move |st| attachments::add(st, &d, &p, None)).expect("add")
    };
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].name, "첨부 원본.bin");
    assert_eq!(list[0].size, payload.len() as u64);
    let info = info_of(&doc.doc_id);
    assert_eq!(info.attachment_count, 1);
    assert_eq!(info.undo_label.as_deref(), Some("undo.attachmentAdd"));
    // The same name again gets a suffix.
    let list = {
        let (d, p) = (doc.doc_id.clone(), source.display().to_string());
        with_state(move |st| attachments::add(st, &d, &p, None)).unwrap()
    };
    // PDFium lists the name tree in key order.
    let mut names: Vec<&str> = list.iter().map(|a| a.name.as_str()).collect();
    names.sort();
    assert_eq!(names, ["첨부 원본 (2).bin", "첨부 원본.bin"]);

    // Save the document, reopen, and extract: identical bytes.
    let target = v3_dir().join("attachments.pdf");
    let (d, p) = (doc.doc_id.clone(), target.display().to_string());
    with_state(move |st| save::save(st, &d, Some(&p), false)).expect("save as");
    let again = reopen(std::fs::read(&target).unwrap(), None).unwrap();
    assert_eq!(again.info.attachment_count, 2);
    let out = v3_dir().join("extracted.bin");
    let (d, p) = (again.doc_id.clone(), out.display().to_string());
    with_doc(&d, move |doc| attachments::save(doc, 0, &p)).expect("save attachment");
    assert_eq!(std::fs::read(&out).unwrap(), payload);

    // Delete both: count 0; undo brings one back.
    for _ in 0..2 {
        let d = again.doc_id.clone();
        with_state(move |st| attachments::delete(st, &d, 0)).expect("delete");
    }
    assert_eq!(info_of(&again.doc_id).attachment_count, 0);
    let d = again.doc_id.clone();
    let undone = with_state(move |st| registry::undo(st, &d, false)).unwrap();
    assert_eq!(undone.attachment_count, 1);
    let d = again.doc_id.clone();
    let err = with_state(move |st| attachments::delete(st, &d, 7)).unwrap_err();
    assert_eq!(err.code, ErrorCode::NotFound);
}
