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

fn save_to(
    doc_id: &str,
    target: Option<&str>,
) -> Result<seepdf_lib::ipc::types::SaveResult, seepdf_lib::ipc::EngineError> {
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
    // IPC_CONTRACT §8: a save changes nothing a cache is keyed on, so the generation stays
    // (and only `doc-saved` is emitted, never `doc-changed`) — the mock mirrors this.
    assert_eq!(result.doc_generation, dirty.doc_generation);
    assert_eq!(info.doc_generation, dirty.doc_generation);

    let reopened = open_path(&path, None);
    assert_eq!(
        reopened.info.page_count, 13,
        "the file on disk has 13 pages"
    );
}

/// Bug hunt: Save As into a directory that does not have the file yet must not be refused by
/// a pre-check on the directory's attributes (on Windows `readonly()` of Documents / Desktop /
/// Pictures is the shell-folder marker, not a permission). The check now probes by creating a
/// file; a directory that really refuses writes still answers `readOnly` before any bytes are
/// serialised, and a missing directory is `notFound`.
#[test]
fn save_as_probes_the_directory() {
    let doc = open("rotation.pdf");
    let dir = out_dir().join("save-probe");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let target = dir.join("새 파일.pdf");
    save_to(&doc.doc_id, Some(&target.display().to_string())).expect("save as into a new file");
    assert!(target.is_file());
    let leftovers: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| n != "새 파일.pdf")
        .collect();
    assert!(leftovers.is_empty(), "the probe cleans up: {leftovers:?}");

    let missing = dir.join("nope").join("x.pdf");
    let err = save_to(&doc.doc_id, Some(&missing.display().to_string())).unwrap_err();
    assert_eq!(err.code, ErrorCode::NotFound);

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let locked = dir.join("locked");
        std::fs::create_dir_all(&locked).unwrap();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o555)).unwrap();
        let err = save_to(
            &doc.doc_id,
            Some(&locked.join("x.pdf").display().to_string()),
        )
        .unwrap_err();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(err.code, ErrorCode::ReadOnly, "{err:?}");
    }
    let _ = std::fs::remove_dir_all(&dir);
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
    assert_eq!(
        info.path.as_deref(),
        Some(target.display().to_string().as_str())
    );
    assert!(!info.dirty);
}

/// An abort at any point leaves the original byte-identical and removes the temp file.
///
/// Two failure modes, because they abort at different points:
/// 1. a read-only directory (macOS) / a read-only file (Windows) — the save stops before the
///    temp exists;
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
    // Windows ignores the read-only attribute on a directory (Explorer uses it to mark a
    // customised folder; files can still be created inside), so there the read-only thing is
    // the file itself — which is also the case a Windows user actually meets.
    let locked = if cfg!(windows) { &target } else { &dir };
    let original = std::fs::metadata(locked).expect("metadata").permissions();
    let mut permissions = original.clone();
    permissions.set_readonly(true);
    std::fs::set_permissions(locked, permissions).expect("make it read-only");

    let failed = save_to(&doc.doc_id, None);
    std::fs::set_permissions(locked, original).expect("restore permissions");

    let err = failed.expect_err("a read-only target cannot be saved over");
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
    assert!(
        matches!(err.code, ErrorCode::Io | ErrorCode::ReadOnly),
        "got {err:?}"
    );
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

// ---------------------------------------------------------------------------------------
// v0.3 pkg3-security-save-integrity: H8 (file changed on disk) and U2 (backups)
// ---------------------------------------------------------------------------------------

fn save_with(
    doc_id: &str,
    target: Option<&str>,
    options: save::SaveOptions,
) -> Result<seepdf_lib::ipc::types::SaveResult, seepdf_lib::ipc::EngineError> {
    let doc_id = doc_id.to_string();
    let target = target.map(str::to_owned);
    with_state(move |st| save::save_with(st, &doc_id, target.as_deref(), &options))
}

fn rotate_first_page(doc_id: &str) {
    let doc_id = doc_id.to_string();
    with_state(move |st| {
        seepdf_lib::engine::pages::apply_ops(
            st,
            &doc_id,
            vec![seepdf_lib::ipc::types::PageOp::Rotate {
                pages: vec![0],
                delta: 90,
            }],
        )
    })
    .expect("rotate");
}

/// Another program touching the file after it was opened makes a plain save refuse with
/// `fileChangedOnDisk`; 덮어쓰기 (`force`) then succeeds and re-records the file, so the next
/// plain save goes through again.
#[test]
fn save_detects_a_file_changed_on_disk() {
    let path = working_copy("tracemonkey.pdf", "changed-on-disk.pdf");
    let doc = open_path(&path, None);
    rotate_first_page(&doc.doc_id);

    // Only the modification time changes (a colleague's editor re-saved it, same size).
    let later = std::time::SystemTime::now() + std::time::Duration::from_secs(90);
    std::fs::File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_modified(later)
        .unwrap();
    let before = std::fs::read(&path).unwrap();
    let err = save_to(&doc.doc_id, None).expect_err("must refuse");
    assert_eq!(err.code, ErrorCode::FileChangedOnDisk);
    assert_eq!(std::fs::read(&path).unwrap(), before, "nothing was written");
    assert!(temp_files(path.parent().unwrap()).is_empty());

    // A size change is caught too, and 덮어쓰기 (force) overwrites.
    std::fs::write(&path, b"%PDF-1.7 replaced by someone else").unwrap();
    let err = save_to(&doc.doc_id, None).expect_err("must refuse");
    assert_eq!(err.code, ErrorCode::FileChangedOnDisk);
    save_with(
        &doc.doc_id,
        None,
        save::SaveOptions {
            backup_root: None,
            force: true,
        },
    )
    .expect("덮어쓰기");
    assert!(std::fs::read(&path).unwrap().len() > 1000);
    rotate_first_page(&doc.doc_id);
    save_to(&doc.doc_id, None).expect("the forced save re-recorded the file");

    // Save As to another file is never a conflict, and re-points the document.
    let other = out_dir().join("changed-on-disk-copy.pdf");
    let _ = std::fs::remove_file(&other);
    let other_path = other.display().to_string();
    std::fs::File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_modified(later + std::time::Duration::from_secs(60))
        .unwrap();
    save_to(&doc.doc_id, Some(&other_path)).expect("save as elsewhere");
    rotate_first_page(&doc.doc_id);
    save_to(&doc.doc_id, None).expect("the new file is this document's file now");
}

/// Verification round 2: Save As onto the document's own file after an outside change is the
/// user's explicit choice (the save panel asked "Replace?"), so it writes — and re-records the
/// file, so the next plain save goes through too.
#[test]
fn save_as_onto_the_own_file_after_an_outside_change() {
    let path = working_copy("tracemonkey.pdf", "save-as-own-file.pdf");
    let doc = open_path(&path, None);
    rotate_first_page(&doc.doc_id);
    let later = std::time::SystemTime::now() + std::time::Duration::from_secs(90);
    std::fs::File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_modified(later)
        .unwrap();
    let own = path.display().to_string();
    assert_eq!(
        save_to(&doc.doc_id, None)
            .expect_err("a plain save still refuses")
            .code,
        ErrorCode::FileChangedOnDisk
    );
    let before = std::fs::read(&path).unwrap();
    save_to(&doc.doc_id, Some(&own)).expect("Save As onto the own file");
    assert_ne!(
        std::fs::read(&path).unwrap(),
        before,
        "the file was written"
    );
    rotate_first_page(&doc.doc_id);
    save_to(&doc.doc_id, None).expect("the Save As re-recorded the file");
}

/// U2: `backup_root: None` writes no backup at all; `Some(root)` writes exactly one per save
/// under `<root>/<stem>/`, holding the file as it was before the save.
#[test]
fn save_backup_follows_the_setting() {
    let path = working_copy("tracemonkey.pdf", "backup-setting.pdf");
    let root = out_dir().join(format!("backups-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let doc = open_path(&path, None);

    rotate_first_page(&doc.doc_id);
    save_with(&doc.doc_id, None, save::SaveOptions::default()).expect("save without backup");
    assert!(!root.exists(), "no backup when the setting is off");

    rotate_first_page(&doc.doc_id);
    let before = std::fs::read(&path).unwrap();
    save_with(
        &doc.doc_id,
        None,
        save::SaveOptions {
            backup_root: Some(root.clone()),
            force: false,
        },
    )
    .expect("save with backup");
    let dir = root.join("backup-setting");
    let backups: Vec<PathBuf> = std::fs::read_dir(&dir)
        .expect("the backup folder")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .collect();
    assert_eq!(backups.len(), 1, "{backups:?}");
    assert_eq!(
        std::fs::read(&backups[0]).unwrap(),
        before,
        "the backup is the file as it was before the save"
    );
    assert!(backups[0]
        .file_name()
        .unwrap()
        .to_string_lossy()
        .ends_with("-backup-setting.pdf"));
    std::fs::remove_dir_all(&root).unwrap();
}
