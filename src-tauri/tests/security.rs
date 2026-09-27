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
    let (doc_id, user, owner) = (doc_id.to_string(), user.map(str::to_owned), owner.to_string());
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
    assert_eq!((a.0, a.1), (b.0, b.1), "the two renders must be the same size");
    let differing = a
        .2
        .chunks_exact(4)
        .zip(b.2.chunks_exact(4))
        .filter(|(pa, pb)| pa.iter().zip(*pb).any(|(x, y)| (*x as i32 - *y as i32).abs() > 24))
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
        DocMeta { title: Some("임시".into()), ..DocMeta::default() },
    )
    .unwrap();
    assert_eq!(info.meta.title.as_deref(), Some("임시"));
    let info = set_metadata(&doc.doc_id, DocMeta { title: Some(String::new()), ..DocMeta::default() })
        .unwrap();
    assert_eq!(info.meta.title, None);
}

#[test]
fn metadata_remove_clears_info_and_xmp() {
    let original = std::fs::read(fixture("160F-2019.pdf")).unwrap();
    assert!(catalog_has_xmp(&original), "the fixture must carry an XMP packet");
    let doc = open("160F-2019.pdf");
    let m = &doc.info.meta;
    assert!(
        [&m.title, &m.author, &m.creator, &m.producer, &m.created].iter().any(|f| f.is_some()),
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
    assert!(!catalog_has_xmp(&bytes), "the catalog must not reference an XMP stream");
    let parsed = lopdf::Document::load_mem(&bytes).unwrap();
    if let Ok(info) = parsed.trailer.get(b"Info").and_then(|o| o.as_reference()) {
        let dict = parsed.get_dictionary(info).expect("/Info dictionary");
        for key in [&b"Title"[..], b"Author", b"Subject", b"Keywords", b"Creator", b"CreationDate"] {
            assert!(!dict.has(key), "/Info still has {}", String::from_utf8_lossy(key));
        }
    }
    let saved = reopen(bytes, None).expect("reopen");
    assert_eq!(saved.info.meta.title, None);
    assert_eq!(saved.info.meta.author, None);
    assert!(saved.info.has_form, "the form survives the rewrite");
}

#[test]
fn metadata_refused_on_encrypted() {
    let doc = try_open("gen/encrypted-rc4-40.pdf", Some("user")).expect("open with the password");
    let err = set_metadata(&doc.doc_id, korean_meta()).expect_err("must refuse");
    assert_eq!(err.code, ErrorCode::Unsupported);
    assert!(err.message.contains("remove the password first"), "{}", err.message);
    let err = remove_metadata(&doc.doc_id).expect_err("must refuse");
    assert_eq!(err.code, ErrorCode::Unsupported);
    let err = set_metadata("no-such-doc", korean_meta()).expect_err("unknown doc");
    assert_eq!(err.code, ErrorCode::NotFound);
    // Nothing was pushed onto the history.
    let can_undo = with_doc(&doc.doc_id, |d| Ok(d.history.can_undo())).unwrap();
    assert!(!can_undo);
}

// ---------------------------------------------------------------------------------------
// P1-3 password
// ---------------------------------------------------------------------------------------

fn restricted() -> PermissionsRequest {
    PermissionsRequest { print: false, extract_text: false, ..PermissionsRequest::default() }
}

#[test]
fn password_aes256_roundtrip() {
    let doc = open("tracemonkey.pdf");
    let bytes = protect(&doc.doc_id, "protected-aes256.pdf", Some("user1"), "owner1", restricted())
        .expect("set_password");

    let parsed = lopdf::Document::load_mem(&bytes).expect("lopdf parses the file");
    assert!(parsed.is_encrypted());
    let encrypt = parsed.get_encrypted().expect("/Encrypt");
    assert_eq!(encrypt.get(b"V").and_then(|o| o.as_i64()).unwrap(), 5);
    assert_eq!(encrypt.get(b"R").and_then(|o| o.as_i64()).unwrap(), 6);

    let err = reopen(bytes.clone(), None).err().expect("no password must fail");
    assert!(err.is_password(), "{err:?}");
    let err = reopen(bytes.clone(), Some("nope")).err().expect("a wrong password must fail");
    assert!(err.is_password(), "{err:?}");

    let opened = reopen(bytes.clone(), Some("user1")).expect("opens with the user password");
    assert_eq!(opened.info.page_count, 14);
    assert!(opened.info.encrypted);
    let p = opened.info.permissions;
    assert!(!p.print, "{p:?}");
    assert!(!p.extract_text, "{p:?}");
    assert!(p.annotate && p.fill_forms && p.assemble && p.modify, "{p:?}");
    // Stage 4: R6 is reported as such, not `unknown`.
    assert_eq!(p.revision, SecurityRevision::R6);

    let diff = difference(&render_page0(&doc.doc_id), &render_page0(&opened.doc_id));
    assert!(diff <= 0.005, "page 0 differs in {:.3} % of pixels", diff * 100.0);

    // The open document itself is untouched.
    let (encrypted, dirty) = with_doc(&doc.doc_id, |d| Ok((d.encrypted, d.dirty()))).unwrap();
    assert!(!encrypted && !dirty);
}

#[test]
fn password_owner_only() {
    let doc = open("tracemonkey.pdf");
    let bytes = protect(&doc.doc_id, "protected-owner-only.pdf", None, "owner1", restricted())
        .expect("set_password");
    let opened = reopen(bytes.clone(), None).expect("opens without a password");
    assert_eq!(opened.info.page_count, 14);
    assert!(opened.info.encrypted);
    assert!(!opened.info.permissions.print);
    assert!(!opened.info.permissions.extract_text);
    assert!(opened.info.permissions.annotate);
    assert_eq!(opened.info.permissions.revision, SecurityRevision::R6);
    // An empty user password is the same as none.
    let bytes = protect(&doc.doc_id, "protected-owner-only-2.pdf", Some(""), "owner1", restricted())
        .expect("set_password");
    reopen(bytes, None).expect("opens without a password");
}

#[test]
fn password_reprotect_encrypted_input() {
    let doc = try_open("gen/encrypted-rc4-40.pdf", Some("user")).expect("open with the password");
    let bytes = protect(
        &doc.doc_id,
        "reprotected.pdf",
        Some("n3w"),
        "owner-n3w",
        PermissionsRequest::default(),
    )
    .expect("set_password on an encrypted input");
    let opened = reopen(bytes.clone(), Some("n3w")).expect("opens with the new password");
    assert_eq!(opened.info.page_count, 1);
    let err = reopen(bytes.clone(), Some("user")).err().expect("the old password must fail");
    assert!(err.is_password(), "{err:?}");
    let err = reopen(bytes, None).err().expect("no password must fail");
    assert!(err.is_password(), "{err:?}");
}

#[test]
fn password_keeps_acroform() {
    let doc = open("160F-2019.pdf");
    assert!(doc.info.has_form);
    let fields_before =
        with_doc(&doc.doc_id, |d| Ok(seepdf_lib::engine::form::list(d, None)?.len())).unwrap();
    assert!(fields_before > 0);

    let bytes = protect(&doc.doc_id, "protected-form.pdf", Some("form"), "owner", PermissionsRequest::default())
        .expect("set_password");
    let opened = reopen(bytes, Some("form")).expect("opens");
    assert!(opened.info.has_form);
    assert_eq!(opened.info.page_count, doc.info.page_count);
    let fields_after =
        with_doc(&opened.doc_id, |d| Ok(seepdf_lib::engine::form::list(d, None)?.len())).unwrap();
    assert_eq!(fields_after, fields_before);
}

#[test]
fn password_rejects_empty_owner() {
    let doc = open("tracemonkey.pdf");
    let err = protect(&doc.doc_id, "never-written.pdf", Some("user"), "", PermissionsRequest::default())
        .expect_err("an empty owner password is invalid");
    assert_eq!(err.code, ErrorCode::InvalidArgument);
    let long = "x".repeat(128);
    let err = protect(&doc.doc_id, "never-written.pdf", Some(&long), "owner", PermissionsRequest::default())
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
    assert_eq!(p, PermissionsRequest { print: false, extract_text: false, ..PermissionsRequest::default() });
    let p: PermissionsRequest = serde_json::from_str("{}").unwrap();
    assert_eq!(p, PermissionsRequest::default());
    let bits = security::lopdf_permissions(p);
    assert!(bits.contains(lopdf::Permissions::COPYABLE_FOR_ACCESSIBILITY | lopdf::Permissions::PRINTABLE));
    let bits = security::lopdf_permissions(restricted());
    assert!(!bits.contains(lopdf::Permissions::PRINTABLE));
    assert!(!bits.contains(lopdf::Permissions::COPYABLE));
    assert!(bits.contains(lopdf::Permissions::COPYABLE_FOR_ACCESSIBILITY));
}

/// PDF text strings and dates as PDFium reads them back.
#[test]
fn pdf_text_string_and_date_format() {
    match save::pdf_text_string("Plain ASCII") {
        lopdf::Object::String(bytes, lopdf::StringFormat::Literal) => assert_eq!(bytes, b"Plain ASCII"),
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
