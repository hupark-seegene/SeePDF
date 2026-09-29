//! Stage 5 — P1-8 autosave / crash recovery (`write_recovery`, `clear_recovery`,
//! `list_recovery`, `discard_recovery`, `IPC_CONTRACT.md` §7.6c).
//!
//! The commands resolve `app_data_dir()/recovery`; the engine functions take the directory,
//! so every test aims them at its own temp dir.

mod common;
use common::*;

use seepdf_lib::engine::recovery::{self, Snapshot};
use seepdf_lib::engine::stamp;
use seepdf_lib::ipc::types::{
    AllPages, PageSelection, PageStampSource, PageStampSpec, RecoveryEntry, StampAnchor, StampRole,
};
use seepdf_lib::ipc::ErrorCode;
use std::path::{Path, PathBuf};

fn temp_root(name: &str) -> PathBuf {
    let dir = std::env::temp_dir()
        .join(format!("seepdf-recovery-test-{}", std::process::id()))
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn snapshot(doc_id: &str) -> Snapshot {
    let doc_id = doc_id.to_string();
    with_state(move |st| recovery::snapshot(st, &doc_id)).expect("snapshot")
}

fn write(root: &Path, doc_id: &str) -> RecoveryEntry {
    recovery::write(root, snapshot(doc_id)).expect("write")
}

fn reopen_page_count(path: &str) -> u16 {
    let bytes = std::fs::read(path).unwrap();
    with_state(move |st| {
        let doc = st
            .pdfium
            .load_pdf_from_byte_vec(bytes, None)
            .map_err(|e| seepdf_lib::ipc::EngineError::pdfium("reopen", e))?;
        Ok(doc.pages().len() as u16)
    })
    .unwrap()
}

#[test]
fn write_creates_pdf_and_sidecar_that_reopen() {
    let root = temp_root("write");
    let doc = open("tracemonkey.pdf");
    // An edit, so the copy is the current state rather than the file on disk.
    let doc_id = doc.doc_id.clone();
    with_state(move |st| {
        stamp::add_stamp(
            st,
            &doc_id,
            &PageStampSpec {
                role: StampRole::Watermark,
                source: PageStampSource::Text {
                    text: "RECOVER".into(),
                    font_size_pt: 40.0,
                    color: [0, 0, 200],
                },
                anchor: StampAnchor::Mc,
                margin_pt: 0.0,
                rotate_deg: 0.0,
                opacity: 0.5,
                pages: PageSelection::All(AllPages::All),
                bates: Default::default(),
            },
        )
    })
    .unwrap();
    let dirty_before = with_doc(&doc.doc_id, |d| Ok((d.dirty(), d.generation))).unwrap();

    let entry = write(&root, &doc.doc_id);
    assert!(uuid::Uuid::parse_str(&entry.id).is_ok());
    let pdf = root.join(format!("{}.pdf", entry.id));
    let sidecar = root.join(format!("{}.json", entry.id));
    assert!(pdf.is_file() && sidecar.is_file());
    // The same file as the one on disk. Compared through `canonicalize` on both sides:
    // on Windows the engine deliberately strips the `\\?\` verbatim prefix that
    // `canonicalize` adds (the path is shown in the UI and passed to Explorer).
    assert_eq!(
        std::fs::canonicalize(&entry.recovery_path).unwrap(),
        std::fs::canonicalize(&pdf).unwrap()
    );
    assert!(Path::new(&entry.recovery_path).is_absolute());
    assert!(
        !entry.recovery_path.starts_with(r"\\?\"),
        "no verbatim prefix leaks out: {}",
        entry.recovery_path
    );
    assert_eq!(entry.bytes, std::fs::metadata(&pdf).unwrap().len());
    assert_eq!(entry.pages, doc.info.page_count);
    assert_eq!(entry.name, "tracemonkey.pdf");
    assert_eq!(
        entry.original_path.as_deref(),
        Some(fixture("tracemonkey.pdf").display().to_string().as_str())
    );
    assert!(
        chrono::DateTime::parse_from_rfc3339(&entry.saved_at).is_ok(),
        "{}",
        entry.saved_at
    );
    // Sidecar == RecoveryEntry.
    let on_disk: RecoveryEntry = serde_json::from_slice(&std::fs::read(&sidecar).unwrap()).unwrap();
    assert_eq!(on_disk, entry);
    // The copy reopens with PDFium, same page count, and carries the edit.
    assert_eq!(reopen_page_count(&entry.recovery_path), doc.info.page_count);
    // No temp files left behind; the user's file and the dirty state are untouched. (The
    // process's owner lock, v0.3 H2, is the one other file.)
    let names: Vec<String> = std::fs::read_dir(&root)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
        .filter(|n| !n.starts_with(".owner-"))
        .collect();
    assert_eq!(names.len(), 2, "{names:?}");
    assert!(recovery::owner_lock_path(&root, recovery::Owner::current()).is_file());
    assert_eq!(
        with_doc(&doc.doc_id, |d| Ok((d.dirty(), d.generation))).unwrap(),
        dirty_before
    );
    assert!(dirty_before.0);
}

#[test]
fn rewrite_overwrites_the_same_id() {
    let root = temp_root("rewrite");
    let doc = open("rotation.pdf");
    let first = write(&root, &doc.doc_id);
    std::thread::sleep(std::time::Duration::from_millis(5));
    let second = write(&root, &doc.doc_id);
    assert_eq!(first.id, second.id);
    assert!(second.saved_at >= first.saved_at);
    assert_eq!(recovery::list(&root).unwrap(), vec![second.clone()]);
    let pair = std::fs::read_dir(&root)
        .unwrap()
        .filter(|e| {
            !e.as_ref()
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".owner-")
        })
        .count();
    assert_eq!(
        pair, 2,
        "the pair (plus this process's owner lock, v0.3 H2)"
    );
    // Another document gets its own id.
    let other = open("rotation.pdf");
    let third = write(&root, &other.doc_id);
    assert_ne!(third.id, first.id);
}

#[test]
fn clear_removes_the_pair() {
    let root = temp_root("clear");
    let doc = open("rotation.pdf");
    let entry = write(&root, &doc.doc_id);
    let doc_id = doc.doc_id.clone();
    let id = with_state(move |st| recovery::recovery_id(st, &doc_id)).unwrap();
    assert_eq!(id.as_deref(), Some(entry.id.as_str()));
    recovery::discard(&root, &entry.id).unwrap();
    assert!(!root.join(format!("{}.pdf", entry.id)).exists());
    assert!(!root.join(format!("{}.json", entry.id)).exists());
    assert!(recovery::list(&root).unwrap().is_empty());
    // Clearing again is a no-op.
    recovery::discard(&root, &entry.id).unwrap();
    // A document that never wrote one has no id.
    let fresh = open("rotation.pdf");
    let fresh_id = fresh.doc_id.clone();
    assert_eq!(
        with_state(move |st| recovery::recovery_id(st, &fresh_id)).unwrap(),
        None
    );
}

#[test]
fn list_is_newest_first_and_prunes_orphans() {
    let root = temp_root("list");
    assert!(
        recovery::list(&root).unwrap().is_empty(),
        "missing dir = empty"
    );
    let a = open("rotation.pdf");
    let b = open("tracemonkey.pdf");
    let c = open("alphatrans.pdf");
    let ea = write(&root, &a.doc_id);
    std::thread::sleep(std::time::Duration::from_millis(10));
    let eb = write(&root, &b.doc_id);
    std::thread::sleep(std::time::Duration::from_millis(10));
    let ec = write(&root, &c.doc_id);

    // Orphan: delete c's pdf, keep its sidecar.
    std::fs::remove_file(&ec.recovery_path).unwrap();
    // Junk that must be ignored (and not deleted).
    std::fs::write(root.join("notes.json"), b"{}").unwrap();
    std::fs::write(root.join(".x.pdf.seepdf-1.tmp"), b"tmp").unwrap();

    let list = recovery::list(&root).unwrap();
    assert_eq!(
        list.iter().map(|e| e.id.clone()).collect::<Vec<_>>(),
        vec![eb.id.clone(), ea.id.clone()]
    );
    assert!(
        !root.join(format!("{}.json", ec.id)).exists(),
        "orphan sidecar pruned"
    );
    assert!(root.join("notes.json").exists());

    // Rewriting `a` makes it the newest.
    std::thread::sleep(std::time::Duration::from_millis(10));
    let ea2 = write(&root, &a.doc_id);
    let list = recovery::list(&root).unwrap();
    assert_eq!(list[0], ea2);
    assert_eq!(list[1].id, eb.id);
}

#[test]
fn discard_unknown_is_a_noop_and_bad_ids_are_rejected() {
    let root = temp_root("discard");
    recovery::discard(&root, &uuid::Uuid::new_v4().to_string()).unwrap();
    let doc = open("rotation.pdf");
    let entry = write(&root, &doc.doc_id);
    recovery::discard(&root, &uuid::Uuid::new_v4().to_string()).unwrap();
    assert_eq!(recovery::list(&root).unwrap(), vec![entry]);
    for bad in ["../../etc/passwd", "", "x"] {
        assert_eq!(
            recovery::discard(&root, bad).unwrap_err().code,
            ErrorCode::InvalidArgument
        );
    }
}

#[test]
fn encrypted_document_stays_encrypted() {
    let root = temp_root("encrypted");
    let name = "gen/encrypted-rc4-40.pdf";
    if !fixture(name).exists() {
        eprintln!("skipping: run `cargo run --release --example gen_fixtures` first");
        return;
    }
    let doc = try_open(name, Some("user")).expect("opens with the user password");
    let entry = write(&root, &doc.doc_id);
    let bytes = std::fs::read(&entry.recovery_path).unwrap();
    let (without, with) = with_state(move |st| {
        let without = st
            .pdfium
            .load_pdf_from_byte_vec(bytes.clone(), None)
            .is_err();
        let with = st
            .pdfium
            .load_pdf_from_byte_vec(bytes, Some("user"))
            .is_ok();
        Ok((without, with))
    })
    .unwrap();
    assert!(without && with, "the recovery copy keeps its encryption");
}

#[test]
fn open_documents_report_their_recovery_ids() {
    let root = temp_root("open-ids");
    let doc = open("rotation.pdf");
    let entry = write(&root, &doc.doc_id);
    let fresh = open("rotation.pdf");
    let live = with_state(|st| Ok(recovery::open_recovery_ids(st))).unwrap();
    // `list_recovery` filters these out: they are live autosaves, not crash leftovers.
    assert!(live.contains(&entry.id));
    let fresh_id = fresh.doc_id.clone();
    assert_eq!(
        with_state(move |st| recovery::recovery_id(st, &fresh_id)).unwrap(),
        None
    );
}

// ---------------------------------------------------------------------------------------
// v0.3 pkg5 (H2): another running process's copies are not offered
// ---------------------------------------------------------------------------------------

/// Rewrites the sidecar of `entry` as if `owner` had written it.
fn set_owner(root: &Path, entry: &RecoveryEntry, owner: &recovery::Owner) {
    let path = root.join(format!("{}.json", entry.id));
    let mut json: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(
        json["owner"]["pid"],
        std::process::id(),
        "the sidecar names its writer"
    );
    json["owner"] = serde_json::to_value(owner).unwrap();
    std::fs::write(&path, serde_json::to_vec(&json).unwrap()).unwrap();
}

fn sorted_ids(list: Vec<RecoveryEntry>) -> Vec<String> {
    let mut ids: Vec<String> = list.into_iter().map(|e| e.id).collect();
    ids.sort();
    ids
}

#[test]
fn copies_of_a_live_foreign_process_are_skipped_and_a_dead_ones_are_listed() {
    let root = temp_root("owners");
    let a = open("rotation.pdf");
    let b = open("tracemonkey.pdf");
    let c = open("alphatrans.pdf");
    let live = write(&root, &a.doc_id);
    let dead = write(&root, &b.doc_id);
    let mine = write(&root, &c.doc_id);

    // `live`: a foreign pid whose process still holds its owner lock (simulated with a lock
    // taken on a second handle — a lock is per open file, so this process sees it as foreign).
    let live_owner = recovery::Owner {
        pid: 4_000_001,
        started: 1_700_000_000_000,
    };
    set_owner(&root, &live, &live_owner);
    let lock_path = recovery::owner_lock_path(&root, &live_owner);
    let held = std::fs::File::create(&lock_path).unwrap();
    held.try_lock().expect("the simulated owner takes its lock");

    // `dead`: a foreign pid that crashed — its lock file is still there but nobody holds it.
    let dead_owner = recovery::Owner {
        pid: 4_000_002,
        started: 1_700_000_000_001,
    };
    set_owner(&root, &dead, &dead_owner);
    std::fs::write(recovery::owner_lock_path(&root, &dead_owner), b"").unwrap();

    let mut expected = vec![dead.id.clone(), mine.id.clone()];
    expected.sort();
    assert_eq!(sorted_ids(recovery::list(&root).unwrap()), expected);
    assert!(
        root.join(format!("{}.json", live.id)).is_file(),
        "a live process's copy is left alone"
    );
    assert!(
        !recovery::owner_lock_path(&root, &dead_owner).exists(),
        "the dead owner's stale lock file is cleaned up"
    );

    // The live process exits: its lock goes with it and its copy is offered.
    drop(held);
    let mut all = vec![live.id.clone(), dead.id.clone(), mine.id.clone()];
    all.sort();
    assert_eq!(sorted_ids(recovery::list(&root).unwrap()), all);
}

#[test]
fn sidecars_without_an_owner_are_still_listed() {
    let root = temp_root("no-owner");
    let doc = open("rotation.pdf");
    let entry = write(&root, &doc.doc_id);
    // A pre-v0.3 sidecar: the plain RecoveryEntry.
    std::fs::write(
        root.join(format!("{}.json", entry.id)),
        serde_json::to_vec(&entry).unwrap(),
    )
    .unwrap();
    assert_eq!(recovery::list(&root).unwrap(), vec![entry]);
}
