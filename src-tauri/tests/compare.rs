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
        bates: Default::default(),
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
            matches!(
                e,
                JobEvent::Done { .. } | JobEvent::Cancelled { .. } | JobEvent::Error { .. }
            )
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
        JobReporter::start_with(sink, engine().jobs.clone(), token.id, work.initial_total());
    compare::dispatch(engine(), work, &token, reporter);
    wait_terminal(&events);
    let out = events.lock().unwrap().clone();
    Ok(out)
}

fn report(events: &[JobEvent]) -> CompareReport {
    match events.last() {
        Some(JobEvent::Done {
            compare: Some(r),
            report: None,
            outputs: None,
            ..
        }) => r.clone(),
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
        assert!(p
            .ops
            .iter()
            .all(|op| op.text_a.is_none() && op.rects_a.is_none()));
    }
    assert!(r.pages[0].words_a > 500, "tracemonkey p1 has ~750 words");
    // alignPages (the default): started{total = |A| + |B| + max(|A|, |B|)}, one page-less
    // progress per scanned page, then one per row (page = index into pages[]), done.
    let n = r.pages.len();
    assert!(matches!(events[0], JobEvent::Started { total, .. } if total as usize == 3 * n));
    let progress: Vec<u16> = events
        .iter()
        .filter_map(|e| match e {
            JobEvent::Progress { page, .. } => *page,
            _ => None,
        })
        .collect();
    assert_eq!(progress, (0..n as u16).collect::<Vec<_>>());
    let scans = events
        .iter()
        .filter(|e| matches!(e, JobEvent::Progress { page: None, .. }))
        .count();
    assert_eq!(scans, 2 * n);
    assert!(matches!(
        events.iter().rev().find(|e| matches!(e, JobEvent::Progress { .. })),
        Some(JobEvent::Progress { done, total, .. }) if *done as usize == 3 * n && *total as usize == 3 * n
    ));

    // Positional pairing keeps the Stage 5 event shape: total = rows, one progress per row.
    let positional = CompareOptions {
        align_pages: Some(false),
        ..Default::default()
    };
    let events = run(&a.doc_id, &b.doc_id, positional).unwrap();
    let r = report(&events);
    assert_eq!((r.pages.len(), r.changed_pages), (n, 0));
    assert!(matches!(events[0], JobEvent::Started { total, .. } if total as usize == n));
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
        let inserts: Vec<_> = page
            .ops
            .iter()
            .filter(|op| op.kind != DiffKind::Equal)
            .collect();
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
    let op = r.pages[2]
        .ops
        .iter()
        .find(|op| op.kind != DiffKind::Equal)
        .unwrap();
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
        align_pages: None,
        visual: None,
    };

    let r = report(&run(&a.doc_id, &b.doc_id, options.clone()).unwrap());
    assert_eq!(r.pages.len(), 1);
    let ops: Vec<_> = r.pages[0]
        .ops
        .iter()
        .filter(|op| op.kind != DiffKind::Equal)
        .collect();
    assert_eq!(ops.len(), 1, "{:?}", r.pages[0].ops);
    assert_eq!(ops[0].kind, DiffKind::Replace);
    assert_eq!(
        (ops[0].text_a.as_deref(), ops[0].text_b.as_deref()),
        (Some("Beta"), Some("Delta"))
    );
    assert!(ops[0].rects_a.as_ref().is_some_and(|r| r.len() == 1));
    assert!(ops[0].rects_b.as_ref().is_some_and(|r| r.len() == 1));
    assert_eq!((r.inserted, r.deleted), (1, 1));

    // Case differences: changed without ignoreCase, equal with it.
    let r = report(&run(&a.doc_id, &c.doc_id, options.clone()).unwrap());
    assert_eq!(r.changed_pages, 1);
    assert_eq!((r.inserted, r.deleted), (3, 3));
    let r = report(
        &run(
            &a.doc_id,
            &c.doc_id,
            CompareOptions {
                ignore_case: true,
                ..options
            },
        )
        .unwrap(),
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
                align_pages: None,
                visual: None,
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

    // Whole documents of different lengths, paired by position (`alignPages: false`): extra
    // B pages are inserts with pageA = null.
    let short = open("rotation.pdf");
    let (count_short, count_long) = (short.info.page_count, a.info.page_count);
    let positional = CompareOptions {
        align_pages: Some(false),
        ..Default::default()
    };
    let r = report(&run(&short.doc_id, &a.doc_id, positional).unwrap());
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
    let work =
        with_state(move |st| compare::begin(st, &da, &db, &CompareOptions::default())).unwrap();
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
        JobReporter::start_with(sink, engine().jobs.clone(), token.id, work.initial_total());
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
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e, JobEvent::Cancelled { .. }))
            .count(),
        1
    );
    assert!(!events.iter().any(|e| matches!(e, JobEvent::Done { .. })));
    assert!(!engine().jobs.cancel(token.id), "the job id is released");
}

// ---------------------------------------------------------------------------------------
// Stage 8 — alignPages, and a document closed mid-job
// ---------------------------------------------------------------------------------------

fn page_ops(doc_id: &str, ops: Vec<seepdf_lib::ipc::types::PageOp>) {
    let doc_id = doc_id.to_string();
    with_state(move |st| seepdf_lib::engine::pages::apply_ops(st, &doc_id, ops)).expect("page_ops");
}

/// B = A with one page inserted (a blank page, then a duplicated page): aligned, every page of
/// A pairs with its copy and the inserted page is the only row — a null-sided one, counted as
/// changed even though a blank page has no words. Positional pairing, by contrast, shifts
/// every later pair.
#[test]
fn aligned_pages_absorb_an_inserted_page() {
    use seepdf_lib::ipc::types::{BlankPageSize, NamedPageSize, PageOp};
    let a = open("tracemonkey.pdf");
    let count = a.info.page_count;

    for (label, op, at) in [
        (
            "blank",
            PageOp::InsertBlank {
                at: 3,
                size: BlankPageSize::Named(NamedPageSize::SameAs),
            },
            3u16,
        ),
        ("duplicate", PageOp::Duplicate { pages: vec![5] }, 6u16),
    ] {
        let b = open("tracemonkey.pdf");
        page_ops(&b.doc_id, vec![op]);
        let r = report(&run(&a.doc_id, &b.doc_id, CompareOptions::default()).unwrap());
        assert_eq!(r.pages.len(), count as usize + 1, "{label}");
        let one_sided: Vec<_> = r
            .pages
            .iter()
            .filter(|p| p.page_a.is_none() || p.page_b.is_none())
            .collect();
        assert_eq!(one_sided.len(), 1, "{label}: {:?}", one_sided);
        assert_eq!(
            (one_sided[0].page_a, one_sided[0].page_b),
            (None, Some(at)),
            "{label}"
        );
        assert!(
            one_sided[0].changed,
            "{label}: an inserted page is a change"
        );
        let other_changed = r
            .pages
            .iter()
            .filter(|p| p.page_a.is_some() && p.page_b.is_some() && p.changed)
            .count();
        assert_eq!(other_changed, 0, "{label}");
        assert_eq!(r.changed_pages, 1, "{label}");
        for p in r
            .pages
            .iter()
            .filter(|p| p.page_a.is_some() && p.page_b.is_some())
        {
            let (pa, pb) = (p.page_a.unwrap(), p.page_b.unwrap());
            assert_eq!(
                pb,
                if pa < at { pa } else { pa + 1 },
                "{label}: {pa} ↔ {pb}"
            );
        }

        // The Stage 5 pairing shifts every pair after the insertion.
        let positional = CompareOptions {
            align_pages: Some(false),
            ..Default::default()
        };
        let r = report(&run(&a.doc_id, &b.doc_id, positional).unwrap());
        assert!(r.changed_pages as usize >= (count - at) as usize, "{label}");
    }
}

/// A document closed while the job is queued ends it with `cancelled`, never `error`.
#[test]
fn closing_a_document_mid_job_cancels() {
    for align in [true, false] {
        let a = open("tracemonkey.pdf");
        let b = open("tracemonkey.pdf");
        let options = CompareOptions {
            align_pages: Some(align),
            ..Default::default()
        };
        let (da, db) = (a.doc_id.clone(), b.doc_id.clone());
        let work = with_state(move |st| compare::begin(st, &da, &db, &options)).unwrap();
        // Hold the engine so the job and the close are both queued; the close (Edit lane)
        // then runs before the job's Background commands.
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
            JobReporter::start_with(sink, engine().jobs.clone(), token.id, work.initial_total());
        compare::dispatch(engine(), work, &token, reporter);
        let close_id = b.doc_id.clone();
        engine()
            .send(Lane::Edit, "test/close", move |st| {
                let _ = seepdf_lib::engine::registry::close(st, &close_id);
            })
            .unwrap();
        release_tx.send(()).unwrap();
        wait_terminal(&events);
        let events = events.lock().unwrap().clone();
        assert!(
            matches!(events.last(), Some(JobEvent::Cancelled { .. })),
            "align {align}: {:?}",
            events.last()
        );
        assert!(!events
            .iter()
            .any(|e| matches!(e, JobEvent::Error { .. } | JobEvent::Done { .. })));
        assert!(!engine().jobs.cancel(token.id), "the job id is released");
        drop(b); // already closed: TestDoc's close is a no-op error
    }
}

#[test]
fn alignment_unit_cases() {
    use compare::{align, jaccard};
    let s = |words: &[u32]| words.to_vec();
    // Identical: positional.
    let a = vec![s(&[1, 2]), s(&[3, 4]), s(&[5])];
    assert_eq!(
        align(&a, &a),
        vec![(Some(0), Some(0)), (Some(1), Some(1)), (Some(2), Some(2))]
    );
    // A page deleted from the middle, and every page slightly edited.
    let b = vec![s(&[1, 2, 9]), s(&[5, 9])];
    assert_eq!(
        align(&a, &b),
        vec![(Some(0), Some(0)), (Some(1), None), (Some(2), Some(1))]
    );
    // Completely different pages still pair by position rather than becoming 4 rows.
    let c = vec![s(&[7]), s(&[8])];
    let d = vec![s(&[10]), s(&[11])];
    assert_eq!(align(&c, &d), vec![(Some(0), Some(0)), (Some(1), Some(1))]);
    // Empty sides.
    assert_eq!(align::<u32>(&[], &[]), vec![]);
    assert_eq!(align(&c, &[]), vec![(Some(0), None), (Some(1), None)]);
    assert_eq!(jaccard::<u32>(&[], &[]), 1.0);
    assert_eq!(jaccard(&[1u32, 2], &[2, 3]), 1.0 / 3.0);
}

// ---------------------------------------------------------------------------------------
// v0.3 pkg8 (X4): pixel diff and scans
// ---------------------------------------------------------------------------------------

use pdfium_render::prelude::*;

/// A scan-like page image: `kind` picks a pattern so the pages hash differently.
fn pattern(kind: u8) -> image::DynamicImage {
    image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(300, 420, move |x, y| {
        let on = match kind % 5 {
            0 => (x / 30) % 2 == 0,
            1 => (y / 30) % 2 == 0,
            2 => ((x / 40) + (y / 40)) % 2 == 0,
            3 => x < 150,
            _ => y < 210,
        };
        if on {
            image::Rgb([30, 30, 30])
        } else {
            image::Rgb([250, 250, 250])
        }
    }))
}

/// One A4 page per entry: an optional caption line of text, and a 300 × 420 pt image at
/// (150, 250) drawn from `pattern(kind)`.
fn image_doc(pages: &[(u8, Option<&str>)]) -> TestDoc {
    let pages: Vec<(u8, Option<String>)> = pages
        .iter()
        .map(|(k, t)| (*k, t.map(str::to_owned)))
        .collect();
    let bytes = with_state(move |st| {
        let mut doc = st.pdfium.create_new_pdf().expect("new pdf");
        let font = doc.fonts_mut().helvetica();
        for (index, (kind, caption)) in pages.iter().enumerate() {
            doc.pages_mut()
                .create_page_at_end(PdfPagePaperSize::a4())
                .expect("page");
            let mut object = PdfPageImageObject::new_with_size(
                &doc,
                &pattern(*kind),
                PdfPoints::new(300.0),
                PdfPoints::new(420.0),
            )
            .expect("image");
            object
                .translate(PdfPoints::new(150.0), PdfPoints::new(250.0))
                .expect("place");
            let mut page = doc.pages().get(index as i32).expect("page");
            page.objects_mut().add_image_object(object).expect("add");
            if let Some(text) = caption {
                page.objects_mut()
                    .create_text_object(
                        PdfPoints::new(72.0),
                        PdfPoints::new(760.0),
                        text,
                        font,
                        PdfPoints::new(14.0),
                    )
                    .expect("text");
            }
            page.regenerate_content().expect("regenerate");
        }
        Ok(doc.save_to_bytes().expect("save"))
    })
    .expect("build");
    let info = with_state(move |st| seepdf_lib::engine::registry::open(st, None, bytes, None))
        .expect("open");
    let doc_id = info.doc_id.clone();
    TestDoc { info, doc_id }
}

fn visual_rects(page: &seepdf_lib::ipc::types::ComparePage) -> Vec<seepdf_lib::ipc::types::Rect> {
    page.ops
        .iter()
        .filter(|op| op.kind == DiffKind::Visual)
        .flat_map(|op| op.rects_a.clone().unwrap_or_default())
        .collect()
}

/// X4: scanned pages (no words) align by their thumbnail hash, so a scan inserted into B is a
/// null-sided row and every later page still pairs with its own counterpart.
#[test]
fn scans_align_around_an_inserted_page() {
    let a = image_doc(&[(0, None), (1, None), (2, None)]);
    let b = image_doc(&[(0, None), (3, None), (1, None), (2, None)]);
    let r = report(&run(&a.doc_id, &b.doc_id, CompareOptions::default()).unwrap());
    let rows: Vec<_> = r.pages.iter().map(|p| (p.page_a, p.page_b)).collect();
    assert_eq!(
        rows,
        vec![
            (Some(0), Some(0)),
            (None, Some(1)),
            (Some(1), Some(2)),
            (Some(2), Some(3))
        ]
    );
    assert_eq!(r.changed_pages, 1, "only the inserted scan is a change");
    assert!(r.pages.iter().all(|p| visual_rects(p).is_empty()));
}

/// X4: a changed figure on a page whose text is the same gives a visual region over the
/// figure (and nowhere else); identical pages give none.
#[test]
fn changed_image_gives_a_visual_rect_in_the_right_place() {
    let a = image_doc(&[
        (0, Some("Figure 1: the same caption")),
        (4, Some("Unchanged")),
    ]);
    let b = image_doc(&[
        (2, Some("Figure 1: the same caption")),
        (4, Some("Unchanged")),
    ]);
    let r = report(&run(&a.doc_id, &b.doc_id, CompareOptions::default()).unwrap());
    assert_eq!(r.pages.len(), 2);
    assert_eq!((r.inserted, r.deleted), (0, 0), "the text is the same");

    let changed = &r.pages[0];
    assert!(changed.changed);
    let rects = visual_rects(changed);
    assert!(!rects.is_empty(), "{:?}", changed.ops);
    // Every region lies on the image (150, 250)–(450, 670), give or take a tile.
    for rect in &rects {
        assert!(
            rect.l >= 150.0 - 12.0 && rect.r <= 450.0 + 12.0,
            "{rect:?} is outside the image horizontally"
        );
        assert!(
            rect.b >= 250.0 - 12.0 && rect.t <= 670.0 + 12.0,
            "{rect:?} is outside the image vertically"
        );
    }
    let covered: f32 = rects.iter().map(|r| (r.r - r.l) * (r.t - r.b)).sum();
    assert!(
        covered > 300.0 * 420.0 * 0.3,
        "most of the figure changed: {covered}"
    );
    let op = changed
        .ops
        .iter()
        .find(|op| op.kind == DiffKind::Visual)
        .unwrap();
    assert_eq!(
        op.rects_a.as_ref().unwrap().len(),
        op.rects_b.as_ref().unwrap().len()
    );

    // Identical pages: no visual op, not changed.
    assert!(!r.pages[1].changed);
    assert!(visual_rects(&r.pages[1]).is_empty());

    // `visual: false` turns the pixel diff off.
    let r = report(
        &run(
            &a.doc_id,
            &b.doc_id,
            CompareOptions {
                visual: Some(false),
                ..Default::default()
            },
        )
        .unwrap(),
    );
    assert_eq!(r.changed_pages, 0);
}

#[test]
fn changed_regions_group_tiles() {
    // Two separate blobs → two regions; one noisy pixel → none.
    let regions = compare::changed_regions(64, 64, |x, y| {
        (x < 10 && y < 10) || ((40..60).contains(&x) && (40..50).contains(&y)) || (x, y) == (30, 5)
    });
    assert_eq!(regions.len(), 2, "{regions:?}");
    assert!(regions.contains(&(0, 0, 16, 16)));
    assert!(regions.contains(&(40, 40, 64, 56)));
    assert!(compare::changed_regions(64, 64, |_, _| false).is_empty());
}
