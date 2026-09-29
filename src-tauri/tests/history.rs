//! Undo / redo snapshots (`ARCHITECTURE.md` §7).

mod common;

use common::*;
use pdfium_render::prelude::PdfPageRenderRotation;
use seepdf_lib::engine::history::History;
use seepdf_lib::engine::registry::{self, MutateOpts};
use seepdf_lib::ipc::types::ChangeReason;
use std::sync::Arc;

fn rotate(doc_id: &str, page: u16, degrees: PdfPageRenderRotation) {
    let doc_id = doc_id.to_string();
    with_state(move |st| {
        registry::mutate(
            st,
            &doc_id,
            MutateOpts::new("undo.pageRotate", ChangeReason::Pages).page(page),
            move |d| {
                let p = d.page(page)?;
                p.set_rotation(degrees);
                Ok(())
            },
        )
    })
    .expect("rotate");
}

fn rotations(doc_id: &str) -> Vec<u16> {
    let doc_id = doc_id.to_string();
    with_state(move |st| {
        Ok(st
            .doc(&doc_id)?
            .pages_meta
            .iter()
            .map(|p| p.rotation)
            .collect())
    })
    .expect("rotations")
}

/// `(undo depth, redo depth, generation)` of an open document.
fn stacks(doc_id: &str) -> (usize, usize, u64) {
    let doc_id = doc_id.to_string();
    with_doc(&doc_id, |d| {
        Ok((
            d.history.undo_depth(),
            d.history.redo_depth(),
            d.generation as u64,
        ))
    })
    .expect("stacks")
}

fn spilled_files(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut files: Vec<_> = std::fs::read_dir(dir)
        .map(|e| e.filter_map(|e| e.ok()).map(|e| e.path()).collect())
        .unwrap_or_default();
    files.sort();
    files
}

/// Bug hunt: undo / redo used to pop the history entry (and push the redo entry) *before*
/// the snapshot was loaded and the document replaced. A spilled snapshot that is gone (macOS
/// purges `$TMPDIR`) or does not reload then consumed an undo step and left a redo entry for a
/// state the document never left. Both stacks must be untouched when the step fails.
#[test]
fn history_undo_is_transactional() {
    let doc = open("rotation.pdf");
    let doc_id = doc.doc_id.clone();
    let spill = std::env::temp_dir().join(format!("seepdf-undo-tx-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&spill);
    {
        let spill = spill.clone();
        // Budget 0: every snapshot goes to disk.
        with_doc(&doc_id, move |d| {
            d.history = History::new(spill, d.bytes.len(), 0);
            Ok(())
        })
        .expect("swap history");
    }
    rotate(&doc_id, 0, PdfPageRenderRotation::Degrees90);
    rotate(&doc_id, 0, PdfPageRenderRotation::Degrees180);
    let edited = rotations(&doc_id);
    let before = stacks(&doc_id);
    assert_eq!((before.0, before.1), (2, 0));
    assert_eq!(spilled_files(&spill).len(), 2, "both snapshots spilled");

    // (a) the snapshot file is gone.
    for f in spilled_files(&spill) {
        std::fs::remove_file(f).expect("delete snapshot");
    }
    let err = with_state({
        let doc_id = doc_id.clone();
        move |st| registry::undo(st, &doc_id, false)
    });
    assert!(err.is_err(), "undo cannot load a deleted snapshot");
    assert_eq!(
        stacks(&doc_id),
        before,
        "a failed undo leaves both stacks and the generation alone"
    );
    assert_eq!(rotations(&doc_id), edited, "the document is unchanged");

    // (b) the snapshot is there but does not load as a PDF: `replace` fails.
    let doc2 = open("rotation.pdf");
    let doc2_id = doc2.doc_id.clone();
    let spill2 = spill.join("second");
    {
        let spill2 = spill2.clone();
        with_doc(&doc2_id, move |d| {
            d.history = History::new(spill2, d.bytes.len(), 0);
            Ok(())
        })
        .expect("swap history");
    }
    rotate(&doc2_id, 1, PdfPageRenderRotation::Degrees90);
    let before2 = stacks(&doc2_id);
    for f in spilled_files(&spill2) {
        std::fs::write(f, b"not a pdf").expect("corrupt snapshot");
    }
    let err = with_state({
        let doc_id = doc2_id.clone();
        move |st| registry::undo(st, &doc_id, false)
    });
    assert!(err.is_err(), "a snapshot PDFium cannot load fails the undo");
    assert_eq!(stacks(&doc2_id), before2);

    // A redo that fails is just as transactional: undo for real, break the redo snapshot.
    let doc3 = open("rotation.pdf");
    let doc3_id = doc3.doc_id.clone();
    let spill3 = spill.join("third");
    {
        let spill3 = spill3.clone();
        with_doc(&doc3_id, move |d| {
            d.history = History::new(spill3, d.bytes.len(), 0);
            Ok(())
        })
        .expect("swap history");
    }
    rotate(&doc3_id, 0, PdfPageRenderRotation::Degrees270);
    with_state({
        let doc_id = doc3_id.clone();
        move |st| registry::undo(st, &doc_id, false)
    })
    .expect("undo");
    let before3 = stacks(&doc3_id);
    assert_eq!((before3.0, before3.1), (0, 1));
    for f in spilled_files(&spill3) {
        std::fs::remove_file(f).expect("delete snapshot");
    }
    assert!(with_state({
        let doc_id = doc3_id.clone();
        move |st| registry::undo(st, &doc_id, true)
    })
    .is_err());
    assert_eq!(stacks(&doc3_id), before3);
    let _ = std::fs::remove_dir_all(&spill);
}

/// Bug hunt: the undo RAM budget is documented (and meant) as one budget **across all open
/// documents**, but every document used to get its own 256 MiB. Two documents' snapshots must
/// share it: once the first document has used it up, the second one spills to disk.
#[test]
fn history_budget_is_shared_across_documents() {
    // One engine-thread call, so no other test's document can come and go in between.
    let spilled = with_state(|st| {
        let bytes = std::fs::read(fixture("tracemonkey.pdf"))?;
        let len = bytes.len();
        let budget = st.history_budget.clone();
        let saved = budget.limit();
        // Room for one more snapshot of this file, not two — on top of whatever the other
        // tests of this process keep resident right now.
        budget.set_limit(budget.used() + len + len / 2);
        let result = (|| {
            let a = registry::open(st, Some(fixture("tracemonkey.pdf")), bytes.clone(), None)?;
            let b = registry::open(st, Some(fixture("tracemonkey.pdf")), bytes, None)?;
            for id in [&a.doc_id, &b.doc_id] {
                registry::mutate(
                    st,
                    id,
                    MutateOpts::new("undo.pageRotate", ChangeReason::Pages).page(0),
                    |d| {
                        d.page(0)?.set_rotation(PdfPageRenderRotation::Degrees90);
                        Ok(())
                    },
                )?;
            }
            let first = st.doc(&a.doc_id)?.history.newest_is_on_disk();
            let second = st.doc(&b.doc_id)?.history.newest_is_on_disk();
            let used = budget.used();
            registry::close(st, &a.doc_id)?;
            registry::close(st, &b.doc_id)?;
            assert_eq!(
                budget.used(),
                used - len,
                "closing gives the resident snapshot back"
            );
            Ok((first, second))
        })();
        budget.set_limit(saved);
        result
    })
    .expect("two documents");
    assert_eq!(spilled.0, Some(false), "the first snapshot fits in RAM");
    assert_eq!(
        spilled.1,
        Some(true),
        "the second document's snapshot spills: the budget is shared"
    );
}

/// 20 edits, then 20 undos, must return the document to its opening state; redo replays them.
#[test]
fn history_roundtrip() {
    let doc = open("tracemonkey.pdf");
    let doc_id = doc.doc_id.clone();
    let before = rotations(&doc_id);
    assert_eq!(before, vec![0u16; 14]);

    // A deterministic "random" walk: page i gets rotated by (i * 90) % 360.
    let steps: Vec<(u16, PdfPageRenderRotation)> = (0..20u16)
        .map(|i| {
            let page = i % 14;
            let degrees = match (i / 3) % 3 {
                0 => PdfPageRenderRotation::Degrees90,
                1 => PdfPageRenderRotation::Degrees180,
                _ => PdfPageRenderRotation::Degrees270,
            };
            (page, degrees)
        })
        .collect();
    for (page, degrees) in &steps {
        rotate(&doc_id, *page, *degrees);
    }
    let after_edits = rotations(&doc_id);
    assert_ne!(after_edits, before, "20 edits changed something");

    let generation = with_state({
        let doc_id = doc_id.clone();
        move |st| {
            let d = st.doc(&doc_id)?;
            Ok((d.generation, d.history.undo_depth(), d.dirty()))
        }
    })
    .expect("state");
    assert_eq!(
        generation,
        (21, 20, true),
        "one mutate = one generation = one undo step"
    );

    for i in 0..20 {
        let doc_id = doc_id.clone();
        with_state(move |st| registry::undo(st, &doc_id, false)).unwrap_or_else(|e| {
            panic!("undo {i}: {e}");
        });
    }
    assert_eq!(
        rotations(&doc_id),
        before,
        "20 undos restore the opening state"
    );

    let exhausted = with_state({
        let doc_id = doc_id.clone();
        move |st| Ok(registry::undo(st, &doc_id, false).is_err())
    })
    .expect("state");
    assert!(exhausted, "the 21st undo fails cleanly");

    for _ in 0..20 {
        let doc_id = doc_id.clone();
        with_state(move |st| registry::undo(st, &doc_id, true)).expect("redo");
    }
    assert_eq!(rotations(&doc_id), after_edits, "redo replays every step");
}

/// Snapshots spill to disk once the RAM budget is exhausted, and the spilled bytes come back
/// intact.
#[test]
fn history_spills_to_disk() {
    let dir = std::env::temp_dir().join(format!("seepdf-history-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    // A 4 MB budget, as the DoD asks for.
    let mut history = History::new(dir.clone(), 1024, 4 * 1024 * 1024);

    let small: Arc<[u8]> = Arc::from(vec![1u8; 1024 * 1024].into_boxed_slice());
    for i in 0..3 {
        history
            .push(format!("undo.step{i}"), small.clone(), false)
            .expect("push");
    }
    assert_eq!(history.newest_is_on_disk(), Some(false));
    assert!(history.ram_bytes() >= 3 * 1024 * 1024);

    let big: Arc<[u8]> = Arc::from(vec![7u8; 2 * 1024 * 1024].into_boxed_slice());
    history.push("undo.big", big.clone(), false).expect("push");
    assert_eq!(
        history.newest_is_on_disk(),
        Some(true),
        "past the budget, snapshots go to disk"
    );

    let (label, bytes) = history
        .take_undo(Arc::from(vec![0u8; 16].into_boxed_slice()))
        .expect("take")
        .expect("an entry");
    assert_eq!(label, "undo.big");
    assert_eq!(bytes.len(), big.len());
    assert!(
        bytes.iter().all(|&b| b == 7),
        "the spilled bytes round-trip"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// Gesture edits with the same label inside 500 ms collapse into one undo step.
#[test]
fn history_coalesces_a_gesture() {
    let dir = std::env::temp_dir().join(format!("seepdf-coalesce-{}", std::process::id()));
    let mut history = History::new(dir.clone(), 1024, 64 * 1024 * 1024);
    let bytes: Arc<[u8]> = Arc::from(vec![0u8; 64].into_boxed_slice());
    for _ in 0..10 {
        history
            .push("undo.annotResize", bytes.clone(), true)
            .expect("push");
    }
    assert_eq!(history.undo_depth(), 1);
    history
        .push("undo.annotCreate", bytes, false)
        .expect("push");
    assert_eq!(
        history.undo_depth(),
        2,
        "a different action is its own step"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// v0.3 pkg4 (verification round 2, A3): a batch on a depth-3 (large) document — more ops
/// than the depth — stays one entry whose snapshot is the pre-batch state; the entries
/// before the batch are trimmed only once it ends.
#[test]
fn history_batch_survives_a_depth_of_three() {
    let dir = std::env::temp_dir().join(format!("seepdf-batch-{}", std::process::id()));
    let mut history = History::new(
        dir.clone(),
        seepdf_lib::engine::history::LARGE_DOC_BYTES + 1,
        64 * 1024 * 1024,
    );
    let snap = |b: u8| -> Arc<[u8]> { Arc::from(vec![b; 16].into_boxed_slice()) };
    history.push("undo.a", snap(1), false).expect("push");
    history.push("undo.b", snap(2), false).expect("push");
    let mark = history.begin_batch();
    for k in 0..6u8 {
        // an op may store more than one entry (a PDFium step, then a lopdf step)
        history
            .push("undo.op", snap(10 + 2 * k), false)
            .expect("push");
        history
            .push("undo.op2", snap(11 + 2 * k), false)
            .expect("push");
        assert!(history.squash_since(mark, "undo.annotEdit"));
        assert_eq!(history.undo_depth(), 3, "the batch is one entry mid-way");
    }
    assert!(history.end_batch(mark, "undo.annotEdit"));
    assert_eq!(history.undo_depth(), 3);
    assert_eq!(history.undo_label().as_deref(), Some("undo.annotEdit"));
    let (label, bytes) = history
        .take_undo(snap(99))
        .expect("undo")
        .expect("an entry");
    assert_eq!(label, "undo.annotEdit");
    assert!(
        bytes.iter().all(|&b| b == 10),
        "one undo restores the state before the batch"
    );
    let (label, _) = history
        .take_undo(snap(10))
        .expect("undo")
        .expect("an entry");
    assert_eq!(label, "undo.b", "the pre-batch entries are untouched");

    // after the batch, trimming is back on
    for k in 0..5u8 {
        history.push("undo.c", snap(50 + k), false).expect("push");
    }
    assert_eq!(history.undo_depth(), 3);
    let _ = std::fs::remove_dir_all(&dir);
}
