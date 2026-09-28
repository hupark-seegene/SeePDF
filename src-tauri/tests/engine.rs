//! Scheduler behaviour: lane ordering, Edit FIFO, viewport stale drop, cancellation and the
//! password error mapping (`ARCHITECTURE.md` §1.2, §14).

mod common;

use common::*;
use seepdf_lib::engine::types::{CmdStatus, Viewport};
use seepdf_lib::engine::{Lane, Submit};
use seepdf_lib::ipc::ErrorCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::Duration;

/// Commands are popped by (lane, priority, seq): every `Interactive` before every `Edit`,
/// and `Edit` stays in submission order because its priority is always 0.
#[test]
fn engine_lanes() {
    let engine = engine();
    let (tx, rx) = mpsc::channel::<&'static str>();

    // Hold the engine busy so the whole batch is queued before anything is popped; without
    // this the scheduler would run each command as it arrives and order nothing.
    let gate = Arc::new((parking_lot::Mutex::new(false), parking_lot::Condvar::new()));
    let held = gate.clone();
    engine
        .send(Lane::Interactive, "test/gate", move |_st| {
            let (lock, cv) = &*held;
            let mut open = lock.lock();
            if !*open {
                cv.wait_for(&mut open, Duration::from_secs(5));
            }
        })
        .expect("gate");

    let submit = |lane: Lane, priority: u32, tag: &'static str| {
        let tx = tx.clone();
        engine
            .dispatch(
                Submit::new(lane, "test/lane").priority(priority),
                move |_st, status| {
                    assert_eq!(status, CmdStatus::Run);
                    let _ = tx.send(tag);
                },
            )
            .expect("dispatch");
    };

    submit(Lane::Background, 0, "background");
    submit(Lane::Edit, 0, "edit-1");
    submit(Lane::Thumb, 0, "thumb");
    submit(Lane::Edit, 0, "edit-2");
    submit(Lane::Prefetch, 0, "prefetch");
    submit(Lane::Interactive, 7, "interactive-far");
    submit(Lane::Interactive, 1, "interactive-near");
    submit(Lane::Edit, 0, "edit-3");

    // Let the batch through.
    {
        let (lock, cv) = &*gate;
        *lock.lock() = true;
        cv.notify_all();
    }

    let mut order = Vec::new();
    for _ in 0..8 {
        order.push(
            rx.recv_timeout(Duration::from_secs(10))
                .expect("command ran"),
        );
    }
    assert_eq!(
        order,
        vec![
            "interactive-near",
            "interactive-far",
            "edit-1",
            "edit-2",
            "edit-3",
            "prefetch",
            "thumb",
            "background",
        ],
        "lanes run in order, Interactive sorts by priority, Edit stays FIFO"
    );
}

/// A render whose viewport generation is two or more behind, for a page outside visible ± 1,
/// is dropped without touching pdfium — and still answers its caller, so a
/// `UriSchemeResponder` is never left hanging.
#[test]
fn engine_stale_drop() {
    let engine = engine();
    let dropped_before = engine.shared.stats.dropped_stale.load(Ordering::Relaxed);

    engine.set_viewport(Viewport {
        doc_id: "d-stale".into(),
        scale_key: 100,
        rotation: 0,
        centre_page: 40,
        first_page: 39,
        last_page: 41,
        velocity_px_per_ms: 0.0,
    });
    let old_gen = engine.shared.viewport_gen();

    let gate = Arc::new((parking_lot::Mutex::new(false), parking_lot::Condvar::new()));
    let held = gate.clone();
    engine
        .send(Lane::Interactive, "test/gate", move |_st| {
            let (lock, cv) = &*held;
            let mut open = lock.lock();
            if !*open {
                cv.wait_for(&mut open, Duration::from_secs(5));
            }
        })
        .expect("gate");

    let (tx, rx) = mpsc::channel::<(&'static str, CmdStatus)>();
    for (tag, page) in [("far", 0u16), ("visible", 40)] {
        let tx = tx.clone();
        engine
            .dispatch(
                Submit::new(Lane::Interactive, "test/stale")
                    .page(page)
                    .viewport(old_gen),
                move |_st, status| {
                    let _ = tx.send((tag, status));
                },
            )
            .expect("dispatch");
    }

    // Two more scroll settles: old_gen + 1 < current.
    engine.shared.bump_viewport_gen();
    engine.shared.bump_viewport_gen();
    {
        let (lock, cv) = &*gate;
        *lock.lock() = true;
        cv.notify_all();
    }

    let mut seen = std::collections::HashMap::new();
    for _ in 0..2 {
        let (tag, status) = rx.recv_timeout(Duration::from_secs(10)).expect("answered");
        seen.insert(tag, status);
    }
    assert_eq!(
        seen["far"],
        CmdStatus::Stale,
        "page 0 is far outside the viewport"
    );
    assert_eq!(
        seen["visible"],
        CmdStatus::Run,
        "a page inside visible ± 1 is never dropped"
    );
    assert!(
        engine.shared.stats.dropped_stale.load(Ordering::Relaxed) > dropped_before,
        "the drop is counted for engine_stats"
    );
}

/// A cancelled job's remaining commands are dispatched with `Cancelled` rather than running.
#[test]
fn engine_cancel_between_chunks() {
    let engine = engine();
    let token = engine.jobs.create();
    let ran = Arc::new(AtomicBool::new(false));
    let (tx, rx) = mpsc::channel::<CmdStatus>();

    assert!(
        engine.jobs.cancel(token.id),
        "cancel returns true for a live job"
    );
    assert!(!engine.jobs.cancel(999_999), "unknown job ids return false");

    let ran_in_cmd = ran.clone();
    engine
        .dispatch(
            Submit::new(Lane::Background, "test/cancel").cancel(token.cancel.clone()),
            move |_st, status| {
                if status == CmdStatus::Run {
                    ran_in_cmd.store(true, Ordering::Relaxed);
                }
                let _ = tx.send(status);
            },
        )
        .expect("dispatch");

    assert_eq!(
        rx.recv_timeout(Duration::from_secs(10)).expect("answered"),
        CmdStatus::Cancelled
    );
    assert!(!ran.load(Ordering::Relaxed));
    engine.jobs.finish(token.id);
}

/// `passwordRequired` when none was supplied, `passwordWrong` when one was, and the right
/// password opens the document (`ARCHITECTURE.md` §2).
#[test]
fn engine_password_error_mapping() {
    let name = "gen/encrypted-rc4-40.pdf";
    if !fixture(name).exists() {
        eprintln!("skipping: run `cargo run --release --example gen_fixtures` first");
        return;
    }
    assert_eq!(
        try_open(name, None).err().map(|e| e.code),
        Some(ErrorCode::PasswordRequired)
    );
    assert_eq!(
        try_open(name, Some("nope")).err().map(|e| e.code),
        Some(ErrorCode::PasswordWrong)
    );
    let doc = try_open(name, Some("user")).expect("opens with the user password");
    assert_eq!(doc.info.page_count, 1);
    assert!(
        doc.info.encrypted,
        "the document reports itself as encrypted"
    );
}
