//! Stage 5 — P1-6 compare two documents (`compare_documents`, `IPC_CONTRACT.md` §7.6b).
//!
//! The "revised" document is the same fixture opened a second time with a Stage 4 header
//! stamp (`add_stamp`) on known pages, so the expected diff is exact: one inserted word per
//! stamped page, nothing else.

mod common;
use common::*;

use seepdf_lib::engine::compare;
use seepdf_lib::engine::export::job::{JobReporter, JobSink};
use seepdf_lib::engine::stamp;
use seepdf_lib::engine::Lane;
use seepdf_lib::ipc::types::{
    CompareOptions, CompareReport, DiffKind, JobEvent, PageSelection, PageStampSource,
    PageStampSpec, StampAnchor, StampRole,
};
use seepdf_lib::ipc::{EngineError, ErrorCode};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

fn header(text: &str, pages: Vec<u16>) -> PageStampSpec {
    PageStampSpec {
        role: StampRole::Header,
        source: PageStampSource::Text {
            text: text.into(),
            font_size_pt: 14.0,
            color: [200, 0, 0],
        },
        anchor: StampAnchor::Tc,
        margin_pt: 20.0,
        rotate_deg: 0.0,
        opacity: 1.0,
        pages: PageSelection::List(pages),
    }
}

fn stamp_doc(doc_id: &str, spec: PageStampSpec) {
    let doc_id = doc_id.to_string();
    with_state(move |st| stamp::add_stamp(st, &doc_id, &spec)).expect("add_stamp");
}

fn wait_terminal(events: &Arc<Mutex<Vec<JobEvent>>>) {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let done = events.lock().unwrap().iter().any(|e| {
            matches!(e, JobEvent::Done { .. } | JobEvent::Cancelled { .. } | JobEvent::Error { .. })
        });
        if done {
            return;
        }
        assert!(Instant::now() < deadline, "job did not finish");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Runs a whole compare job (as the command does) and returns every event.
fn run(a: &str, b: &str, options: CompareOptions) -> Result<Vec<JobEvent>, EngineError> {
    let (da, db) = (a.to_string(), b.to_string());
    let work = with_state(move |st| compare::begin(st, &da, &db, &options))?;
    let token = engine().jobs.create();
    let events: Arc<Mutex<Vec<JobEvent>>> = Arc::default();
    let sink_events = events.clone();
    let sink: JobSink = Arc::new(move |e| sink_events.lock().unwrap().push(e));
    let reporter =
        JobReporter::start_with(sink, engine().jobs.clone(), token.id, work.pairs.len() as u32);
    compare::dispatch(engine(), work, &token, reporter);
    wait_terminal(&events);
    let out = events.lock().unwrap().clone();
    Ok(out)
}

fn report(events: &[JobEvent]) -> CompareReport {
    match events.last() {
        Some(JobEvent::Done { compare: Some(r), report: None, outputs: None, .. }) => r.clone(),
        other => panic!("expected done with a compare report, got {other:?}"),
    }
}

#[test]
fn same_document_twice_has_no_changes() {
    let a = open("tracemonkey.pdf");
    let b = open("tracemonkey.pdf");
    let events = run(&a.doc_id, &b.doc_id, CompareOptions::default()).unwrap();
    let r = report(&events);
    assert_eq!(r.pages.len(), a.info.page_count as usize);
    assert_eq!((r.changed_pages, r.inserted, r.deleted), (0, 0, 0));
    for (i, p) in r.pages.iter().enumerate() {
        assert_eq!((p.page_a, p.page_b), (Some(i as u16), Some(i as u16)));
        assert!(!p.changed);
        assert_eq!(p.words_a, p.words_b);
        assert!(p.ops.iter().all(|op| op.kind == DiffKind::Equal));
        assert!(p.ops.iter().all(|op| op.text_a.is_none() && op.rects_a.is_none()));
    }
    assert!(r.pages[0].words_a > 500, "tracemonkey p1 has ~750 words");
    // started, one progress per pair (page = index into pages[]), done.
    assert!(matches!(events[0], JobEvent::Started { total, .. } if total as usize == r.pages.len()));
    let progress: Vec<u16> = events
        .iter()
        .filter_map(|e| match e {
            JobEvent::Progress { page, .. } => *page,
            _ => None,
        })
        .collect();
    assert_eq!(progress, (0..r.pages.len() as u16).collect::<Vec<_>>());
}

#[test]
fn stamped_copy_reports_the_inserted_words() {
    let a = open("tracemonkey.pdf");
    let b = open("tracemonkey.pdf");
    stamp_doc(&b.doc_id, header("DRAFT-{{page}}", vec![0, 2]));
    let r = report(&run(&a.doc_id, &b.doc_id, CompareOptions::default()).unwrap());
    let changed: Vec<usize> = r
        .pages
        .iter()
        .enumerate()
        .filter(|(_, p)| p.changed)
        .map(|(i, _)| i)
        .collect();
    assert_eq!(changed, vec![0, 2]);
    assert_eq!((r.changed_pages, r.inserted, r.deleted), (2, 2, 0));
    let geom = b.info.pages.clone();
    for (i, expected) in [(0usize, "DRAFT-1"), (2, "DRAFT-3")] {
        let page = &r.pages[i];
        assert_eq!(page.words_b, page.words_a + 1);
        let inserts: Vec<_> = page.ops.iter().filter(|op| op.kind != DiffKind::Equal).collect();
        assert_eq!(inserts.len(), 1, "{:?}", page.ops);
        let op = inserts[0];
        assert_eq!(op.kind, DiffKind::Insert);
        assert_eq!(op.words, 1);
        assert_eq!(op.text_b.as_deref(), Some(expected));
        assert!(op.text_a.is_none() && op.rects_a.is_none());
        let rects = op.rects_b.as_ref().expect("rectsB");
        assert_eq!(rects.len(), 1, "one line");
        let crop = geom[i].crop;
        for rect in rects {
            assert!(rect.width() > 0.0 && rect.height() > 0.0, "{rect:?}");
            assert!(
                rect.l >= crop.l && rect.r <= crop.r && rect.b >= crop.b && rect.t <= crop.t,
                "{rect:?} outside {crop:?}"
            );
            // A top-centre header: upper part of the page.
            assert!(rect.b > crop.b + crop.height() * 0.8, "{rect:?}");
        }
    }

    // Reversed: the same words are deletions on A's side.
    let r = report(&run(&b.doc_id, &a.doc_id, CompareOptions::default()).unwrap());
    assert_eq!((r.changed_pages, r.inserted, r.deleted), (2, 0, 2));
    let op = r.pages[2].ops.iter().find(|op| op.kind != DiffKind::Equal).unwrap();
    assert_eq!(op.kind, DiffKind::Delete);
    assert_eq!(op.text_a.as_deref(), Some("DRAFT-3"));
    assert!(op.rects_a.as_ref().is_some_and(|r| !r.is_empty()) && op.rects_b.is_none());
}

#[test]
fn replaced_words_collapse_and_ignore_case_folds() {
    let a = open("tracemonkey.pdf");
    let b = open("tracemonkey.pdf");
    let c = open("tracemonkey.pdf");
    stamp_doc(&a.doc_id, header("Alpha Beta Gamma", vec![1]));
    stamp_doc(&b.doc_id, header("Alpha Delta Gamma", vec![1]));
    stamp_doc(&c.doc_id, header("ALPHA beta GAMMA", vec![1]));
    let options = CompareOptions {
        pages_a: Some(vec![1]),
        pages_b: Some(vec![1]),
        ignore_case: false,
    };

    let r = report(&run(&a.doc_id, &b.doc_id, options.clone()).unwrap());
    assert_eq!(r.pages.len(), 1);
    let ops: Vec<_> = r.pages[0].ops.iter().filter(|op| op.kind != DiffKind::Equal).collect();
    assert_eq!(ops.len(), 1, "{:?}", r.pages[0].ops);
    assert_eq!(ops[0].kind, DiffKind::Replace);
    assert_eq!((ops[0].text_a.as_deref(), ops[0].text_b.as_deref()), (Some("Beta"), Some("Delta")));
    assert!(ops[0].rects_a.as_ref().is_some_and(|r| r.len() == 1));
    assert!(ops[0].rects_b.as_ref().is_some_and(|r| r.len() == 1));
    assert_eq!((r.inserted, r.deleted), (1, 1));

    // Case differences: changed without ignoreCase, equal with it.
    let r = report(&run(&a.doc_id, &c.doc_id, options.clone()).unwrap());
    assert_eq!(r.changed_pages, 1);
    assert_eq!((r.inserted, r.deleted), (3, 3));
    let r = report(
        &run(&a.doc_id, &c.doc_id, CompareOptions { ignore_case: true, ..options }).unwrap(),
    );
    assert_eq!((r.changed_pages, r.inserted, r.deleted), (0, 0, 0));
}

#[test]
fn unequal_page_lists_give_null_sided_rows() {
    let a = open("tracemonkey.pdf");
    let b = open("tracemonkey.pdf");
    let r = report(
        &run(
            &a.doc_id,
            &b.doc_id,
            CompareOptions {
                pages_a: Some(vec![0, 1, 2]),
                pages_b: Some(vec![0]),
                ignore_case: false,
            },
        )
        .unwrap(),
    );
    assert_eq!(r.pages.len(), 3);
    assert_eq!((r.pages[0].page_a, r.pages[0].page_b), (Some(0), Some(0)));
    assert!(!r.pages[0].changed);
    for (i, page) in r.pages.iter().enumerate().skip(1) {
        assert_eq!((page.page_a, page.page_b), (Some(i as u16), None));
        assert!(page.changed);
        assert_eq!(page.words_b, 0);
        assert_eq!(page.ops.len(), 1);
        assert_eq!(page.ops[0].kind, DiffKind::Delete);
        assert_eq!(page.ops[0].words, page.words_a);
        assert!(page.ops[0].rects_a.as_ref().is_some_and(|r| !r.is_empty()));
    }
    assert_eq!(r.changed_pages, 2);
    assert_eq!(r.deleted, r.pages[1].words_a + r.pages[2].words_a);

    // Whole documents of different lengths: extra B pages are inserts with pageA = null.
    let short = open("rotation.pdf");
    let (count_short, count_long) = (short.info.page_count, a.info.page_count);
    let r = report(&run(&short.doc_id, &a.doc_id, CompareOptions::default()).unwrap());
    assert_eq!(r.pages.len(), count_short.max(count_long) as usize);
    if count_long > count_short {
        let last = r.pages.last().unwrap();
        assert_eq!((last.page_a, last.page_b), (None, Some(count_long - 1)));
        assert!(last.ops.iter().all(|op| op.kind == DiffKind::Insert));
    }
}

#[test]
fn invalid_requests_reject_before_the_job() {
    let a = open("tracemonkey.pdf");
    let b = open("rotation.pdf");
    let err = run(&a.doc_id, &a.doc_id, CompareOptions::default()).unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidArgument);
    let err = run(&a.doc_id, "d999999", CompareOptions::default()).unwrap_err();
    assert_eq!(err.code, ErrorCode::NotFound);
    let err = run(
        &a.doc_id,
        &b.doc_id,
        CompareOptions {
            pages_a: Some(vec![999]),
            ..Default::default()
        },
    )
    .unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidArgument);
}

#[test]
fn cancel_mid_job() {
    let a = open("gen/500p.pdf");
    let b = open("gen/500p.pdf");
    let (da, db) = (a.doc_id.clone(), b.doc_id.clone());
    let work = with_state(move |st| compare::begin(st, &da, &db, &CompareOptions::default()))
        .unwrap();
    // Hold the engine so the whole job is queued before any of it runs.
    let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
    engine()
        .send(Lane::Edit, "test/block", move |_| {
            let _ = release_rx.recv_timeout(Duration::from_secs(10));
        })
        .unwrap();
    let token = engine().jobs.create();
    let events: Arc<Mutex<Vec<JobEvent>>> = Arc::default();
    let sink_events = events.clone();
    let sink: JobSink = Arc::new(move |e| sink_events.lock().unwrap().push(e));
    let reporter =
        JobReporter::start_with(sink, engine().jobs.clone(), token.id, work.pairs.len() as u32);
    compare::dispatch(engine(), work, &token, reporter);
    assert!(engine().jobs.cancel(token.id));
    release_tx.send(()).unwrap();
    wait_terminal(&events);
    let events = events.lock().unwrap().clone();
    assert!(
        matches!(events.last(), Some(JobEvent::Cancelled { done: 0, .. })),
        "{:?}",
        events.last()
    );
    assert_eq!(events.iter().filter(|e| matches!(e, JobEvent::Cancelled { .. })).count(), 1);
    assert!(!events.iter().any(|e| matches!(e, JobEvent::Done { .. })));
    assert!(!engine().jobs.cancel(token.id), "the job id is released");
}
