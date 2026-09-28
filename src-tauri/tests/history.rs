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
