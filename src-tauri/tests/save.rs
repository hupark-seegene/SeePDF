//! Stage 1 (b) — save and Save As (`IPC_CONTRACT.md` §7.6, `ARCHITECTURE.md` §8).
//!
//! The three things a save has to promise, and the three tests that hold it to them:
//!
//! * **atomic** — the original is either the old file or the new file, never a half-written
//!   one, and no `.tmp` is left behind when anything goes wrong (`save_atomic_abort`);
//! * **verified** — bytes that do not reopen with the right page count never reach the disk
//!   (`save_verify_rejects_truncated`);
//! * **lossless** — a document opened with a password is still encrypted afterwards
//!   (`save_keeps_encryption`).

mod common;
use common::*;

use seepdf_lib::engine::registry;
use seepdf_lib::engine::save;
use seepdf_lib::ipc::ErrorCode;
use std::path::{Path, PathBuf};

fn out_dir() -> PathBuf {
    let dir = fixture("out").join("stage1b");
    std::fs::create_dir_all(&dir).expect("create fixtures/out/stage1b");
    dir
}

/// A fresh copy of a fixture in `fixtures/out/stage1b/`, so a test can write to it.
fn working_copy(fixture_name: &str, as_name: &str) -> PathBuf {
    let path = out_dir().join(as_name);
    let _ = std::fs::remove_file(&path);
    std::fs::copy(fixture(fixture_name), &path).expect("copy fixture");
    path
}

fn open_path(path: &Path, password: Option<&str>) -> TestDoc {
    let bytes = std::fs::read(path).expect("read");
    let owned = path.to_path_buf();
    let password = password.map(str::to_owned);
    let info = with_state(move |st| registry::open(st, Some(owned), bytes, password))
        .expect("open working copy");
    let doc_id = info.doc_id.clone();
    TestDoc { info, doc_id }
}

fn save_to(doc_id: &str, target: Option<&str>) -> Result<seepdf_lib::ipc::types::SaveResult, seepdf_lib::ipc::EngineError> {
    let doc_id = doc_id.to_string();
    let target = target.map(str::to_owned);
    with_state(move |st| save::save(st, &doc_id, target.as_deref(), false))
}

/// Any `.seepdf-*.tmp` file left in `dir`.
fn temp_files(dir: &Path) -> Vec<String> {
    std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().to_string())
                .filter(|n| n.contains(".seepdf-") && n.ends_with(".tmp"))
                .collect()
        })
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------------------

/// A plain save writes through a temp file, fsyncs, renames, and leaves the document clean and
/// pointing at the same path.
#[test]
fn save_document_roundtrip() {
    let path = working_copy("tracemonkey.pdf", "save-roundtrip.pdf");
    let doc = open_path(&path, None);
    assert!(!doc.info.dirty);

    // Make a real change so the save has something to write.
    let dirty = with_state({
        let doc_id = doc.doc_id.clone();
        move |st| {
            seepdf_lib::engine::pages::apply_ops(
                st,
                &doc_id,
                vec![seepdf_lib::ipc::types::PageOp::Delete { pages: vec![13] }],
            )
        }
    })
    .expect("page_ops");
    assert!(dirty.dirty, "an edit dirties the document");

    let result = save_to(&doc.doc_id, None).expect("save_document");
    assert_eq!(result.path, path.display().to_string());
    assert!(result.bytes > 100_000);
    assert!(temp_files(&out_dir()).is_empty(), "no temp file survives");

    let info = with_doc(&doc.doc_id, |d| Ok(d.info())).expect("info");
    assert!(!info.dirty, "savedGeneration caught up");
    assert_eq!(info.page_count, 13);

    let reopened = open_path(&path, None);
    assert_eq!(reopened.info.page_count, 13, "the file on disk has 13 pages");
}

/// Save As writes elsewhere and re-points the document at the new file.
#[test]
fn save_document_as_repoints_the_document() {
    let doc = open("tracemonkey.pdf");
    let target = out_dir().join("save-as.pdf");
    let _ = std::fs::remove_file(&target);

    let result = save_to(&doc.doc_id, Some(&target.display().to_string())).expect("save as");
    assert_eq!(result.path, target.display().to_string());
    assert!(target.is_file());

    let info = with_doc(&doc.doc_id, |d| Ok(d.info())).expect("info");
    assert_eq!(info.path.as_deref(), Some(target.display().to_string().as_str()));
    assert!(!info.dirty);
}

/// An abort at any point leaves the original byte-identical and removes the temp file.
///
/// Two failure modes, because they abort at different points:
/// 1. a read-only directory — `File::create` fails, so the temp never exists;
/// 2. a destination that cannot be renamed onto (here: a directory of that name) — the temp is
///    written and fsynced and the **rename** fails, which is the case that would leave litter.
#[test]
fn save_atomic_abort() {
    // --- 1. read-only directory -------------------------------------------------------
    let dir = out_dir().join("abort-readonly");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create dir");
    let target = dir.join("original.pdf");
    std::fs::copy(fixture("tracemonkey.pdf"), &target).expect("seed original");
    let before = std::fs::read(&target).expect("read original");

    let doc = open_path(&target, None);
    let mut permissions = std::fs::metadata(&dir).expect("metadata").permissions();
    permissions.set_readonly(true);
    std::fs::set_permissions(&dir, permissions).expect("make the directory read-only");

    let failed = save_to(&doc.doc_id, None);
    let restore = std::fs::metadata(&dir).expect("metadata").permissions();
    let mut restore = restore;
    restore.set_readonly(false);
    std::fs::set_permissions(&dir, restore).expect("restore permissions");

    let err = failed.expect_err("a read-only directory cannot be saved into");
    assert!(
        matches!(err.code, ErrorCode::ReadOnly | ErrorCode::Io),
        "got {err:?}"
    );
    assert_eq!(
        std::fs::read(&target).expect("read original"),
        before,
        "the original is byte-identical"
    );
    assert!(temp_files(&dir).is_empty(), "no temp file left behind");
    let info = with_doc(&doc.doc_id, |d| Ok(d.info())).expect("info");
    assert!(
        info.doc_generation >= 1,
        "the document is still usable after a failed save"
    );

    // --- 2. the rename fails after the temp is written ---------------------------------
    let dir = out_dir().join("abort-rename");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create dir");
    // A *directory* called `blocked.pdf`: writable parent, so the temp is created and fsynced,
    // but `rename(tmp, blocked.pdf)` cannot replace a directory with a file.
    let blocked = dir.join("blocked.pdf");
    std::fs::create_dir_all(&blocked).expect("create the blocking directory");
    std::fs::write(blocked.join("keep.txt"), b"not a pdf").expect("make it non-empty");

    let doc = open("tracemonkey.pdf");
    let failed = save_to(&doc.doc_id, Some(&blocked.display().to_string()));
    let err = failed.expect_err("renaming onto a non-empty directory must fail");
    assert!(matches!(err.code, ErrorCode::Io | ErrorCode::ReadOnly), "got {err:?}");
    assert!(
        temp_files(&dir).is_empty(),
        "the temp file is removed when the rename fails: {:?}",
        temp_files(&dir)
    );
    assert!(
        blocked.join("keep.txt").is_file(),
        "the target was not touched"
    );
}

/// The verify step reopens the serialised bytes and compares the page count. Truncated or
/// otherwise broken bytes never reach the file.
#[test]
fn save_verify_rejects_truncated() {
    let doc = open("tracemonkey.pdf");
    let bytes = with_state({
        let doc_id = doc.doc_id.clone();
        move |st| save::serialize(st, &doc_id)
    })
    .expect("serialize");
    assert!(bytes.len() > 100_000);

    // The real bytes pass.
    with_state({
        let bytes = bytes.clone();
        move |st| save::verify_bytes(st, &bytes, 14, None)
    })
    .expect("the full document verifies");

    // A truncated file does not reopen at all.
    let truncated = bytes[..bytes.len() * 3 / 5].to_vec();
    let err = with_state(move |st| save::verify_bytes(st, &truncated, 14, None))
        .expect_err("truncated bytes must be rejected");
    assert_eq!(err.code, ErrorCode::VerifyFailed);

    // Bytes that *do* reopen but have the wrong page count are rejected too — this is the case
    // a page-count check catches and a "does it parse" check does not.
    let err = with_state({
        let bytes = bytes.clone();
        move |st| save::verify_bytes(st, &bytes, 99, None)
    })
    .expect_err("a page-count mismatch must be rejected");
    assert_eq!(err.code, ErrorCode::VerifyFailed);
    assert!(err.message.contains("14 pages"), "{}", err.message);

    // Garbage that is not a PDF at all.
    let err = with_state(move |st| save::verify_bytes(st, b"%PDF-1.7\nnot really", 1, None))
        .expect_err("garbage must be rejected");
    assert_eq!(err.code, ErrorCode::VerifyFailed);
}

/// `FPDF_SaveAsCopy` keeps the encryption dictionary, so a document opened with a password is
/// still password-protected after a save. (Dropping it is the separate
/// `FPDF_REMOVE_SECURITY = 4` path — pages spike §5.)
#[test]
fn save_keeps_encryption() {
    let doc = try_open("gen/encrypted-rc4-40.pdf", Some("user")).expect("open with the password");
    assert!(doc.info.encrypted);
    assert_eq!(doc.info.page_count, 1);

    let target = out_dir().join("encrypted-resaved.pdf");
    let _ = std::fs::remove_file(&target);
    save_to(&doc.doc_id, Some(&target.display().to_string())).expect("save as");

    let bytes = std::fs::read(&target).expect("read the saved file");
    let no_password = with_state({
        let bytes = bytes.clone();
        move |st| registry::open(st, None, bytes, None)
    });
    let err = no_password.expect_err("the saved file must still need a password");
    assert_eq!(err.code, ErrorCode::PasswordRequired);

    let with_password = with_state(move |st| registry::open(st, None, bytes, Some("user".into())))
        .expect("the saved file opens with the original password");
    assert_eq!(with_password.page_count, 1);
    assert!(with_password.encrypted);
}

/// A document that has never been saved cannot be saved without a path; the UI is expected to
/// fall through to Save As.
#[test]
fn save_untitled_needs_a_path() {
    let merged = with_state(move |st| {
        seepdf_lib::engine::pages::merge(
            st,
            &[seepdf_lib::ipc::types::MergeInput {
                path: fixture("tracemonkey.pdf").display().to_string(),
                range: Some("1".into()),
                password: None,
            }],
        )
    })
    .expect("merge");
    let doc = TestDoc {
        doc_id: merged.info.doc_id.clone(),
        info: merged.info,
    };
    assert!(doc.info.path.is_none());

    let err = save_to(&doc.doc_id, None).expect_err("an untitled document has nowhere to go");
    assert_eq!(err.code, ErrorCode::ReadOnly);
}
