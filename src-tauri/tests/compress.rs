//! Stage 4 — P1-5 compress (`compress_estimate` / `compress_apply` / `compress_discard`,
//! `IPC_CONTRACT.md` §7.6a).
//!
//! The fixtures' own images are either shared across pages (TAMReview's logo) or small images
//! inside Form XObjects (tracemonkey's 90, all below every preset), so the tests that need
//! something to downsample place their own high-DPI images first: a PNG (Flate path) and a
//! real JPEG (`/DCTDecode` path) — and, since Stage 8, a JPEG inside a Form XObject.

mod common;
use common::*;

use pdfium_render::prelude::*;
use seepdf_lib::engine::compress;
use seepdf_lib::engine::export::job::{JobReporter, JobSink};
use seepdf_lib::engine::objects;
use seepdf_lib::engine::registry::{self, MutateOpts};
use seepdf_lib::engine::Lane;
use seepdf_lib::ipc::types::{
    ChangeReason, CompressOptions, CompressReport, DocGeneration, DocInfo, JobEvent, Rect,
    TextAlign,
};
use seepdf_lib::ipc::{EngineError, ErrorCode};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

fn out_dir() -> PathBuf {
    let dir = fixture("out").join("stage4");
    std::fs::create_dir_all(&dir).expect("create fixtures/out/stage4");
    dir
}

fn noisy_rgb(w: u32, h: u32) -> image::RgbImage {
    let mut img = image::RgbImage::new(w, h);
    let mut seed: u32 = 0x9e37_79b9;
    for (x, y, px) in img.enumerate_pixels_mut() {
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        let n = (seed >> 27) as u8;
        *px = image::Rgb([
            (x % 256) as u8 ^ n,
            (y % 256) as u8,
            ((x + y) % 256) as u8 ^ n,
        ]);
    }
    img
}

/// Places a 1200×900 PNG at 144×108 pt (600 DPI) on page 0 through `add_image_object`.
fn add_png(doc_id: &str) {
    // One file per document: the tests run in parallel.
    let path = out_dir().join(format!("compress-600dpi-{doc_id}.png"));
    noisy_rgb(1200, 900).save(&path).unwrap();
    let (doc_id, path) = (doc_id.to_string(), path.display().to_string());
    with_state(move |st| {
        objects::add_image(
            st,
            &doc_id,
            0,
            Rect::new(72.0, 72.0, 216.0, 180.0),
            &path,
            true,
        )
    })
    .expect("add png");
}

/// Places a 1600×1200 JPEG at 192×144 pt (600 DPI) on page 1 as a real `/DCTDecode` stream.
fn add_jpeg(doc_id: &str) {
    let mut jpeg = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 92)
        .encode_image(&noisy_rgb(1600, 1200))
        .unwrap();
    let doc_id = doc_id.to_string();
    with_state(move |st| {
        registry::mutate(
            st,
            &doc_id,
            MutateOpts::new("undo.objectAdd", ChangeReason::Edit).page(1),
            move |doc| {
                let page_index = 1u16;
                doc.invalidate_page_handle(page_index);
                let document = doc.pdf();
                let mut page = document.pages().get(page_index as PdfPageIndex).unwrap();
                let mut object =
                    PdfPageImageObject::new_from_jpeg_reader(document, std::io::Cursor::new(jpeg))
                        .unwrap();
                object.scale(192.0, 144.0).unwrap();
                object
                    .translate(PdfPoints::new(72.0), PdfPoints::new(72.0))
                    .unwrap();
                page.objects_mut().add_image_object(object).unwrap();
                page.regenerate_content().unwrap();
                Ok(())
            },
        )
    })
    .expect("add jpeg");
}

/// `(pixel width, filters)` of every top-level image on `page`.
fn images(doc_id: &str, page: u16) -> Vec<(i32, Vec<String>)> {
    let doc_id = doc_id.to_string();
    with_doc(&doc_id, move |d| {
        let p = d.page(page)?;
        Ok(p.objects()
            .iter()
            .filter_map(|o| {
                o.as_image_object().map(|i| {
                    (
                        i.width().unwrap_or(0),
                        i.filters().iter().map(|f| f.name().to_string()).collect(),
                    )
                })
            })
            .collect())
    })
    .unwrap()
}

/// Runs a whole estimate job and returns every event it emitted.
fn estimate(doc_id: &str, options: CompressOptions) -> Result<Vec<JobEvent>, EngineError> {
    let events: Arc<Mutex<Vec<JobEvent>>> = Arc::default();
    let token = engine().jobs.create();
    let (d, id) = (doc_id.to_string(), token.id);
    let pages = match with_state(move |st| compress::begin(st, &d, &options, id)) {
        Ok(pages) => pages,
        Err(e) => {
            engine().jobs.finish(token.id);
            return Err(e);
        }
    };
    let sink_events = events.clone();
    let sink: JobSink = Arc::new(move |e| sink_events.lock().unwrap().push(e));
    let reporter =
        JobReporter::start_with(sink, engine().jobs.clone(), token.id, pages.len() as u32);
    compress::dispatch(engine(), doc_id, pages, &token, reporter);
    wait_terminal(&events);
    let out = events.lock().unwrap().clone();
    Ok(out)
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

fn report_of(events: &[JobEvent]) -> CompressReport {
    match events.last() {
        Some(JobEvent::Done {
            report: Some(r), ..
        }) => r.clone(),
        other => panic!("expected done with a report, got {other:?}"),
    }
}

fn apply(doc_id: &str, token: u64) -> Result<DocInfo, EngineError> {
    let d = doc_id.to_string();
    with_state(move |st| compress::apply(st, &d, token))
}

fn opts(dpi: u32) -> CompressOptions {
    CompressOptions {
        target_dpi: dpi,
        optimize: None,
        pages: None,
    }
}

#[test]
fn compress_estimate_downsamples_and_apply_is_undoable() {
    let doc = open("tracemonkey.pdf");
    add_png(&doc.doc_id);
    add_jpeg(&doc.doc_id);
    assert_eq!(images(&doc.doc_id, 0)[0].0, 1200);
    assert_eq!(
        images(&doc.doc_id, 1)[0],
        (1600, vec!["DCTDecode".to_string()])
    );
    let generation = with_doc(&doc.doc_id, |d| Ok(d.generation)).unwrap();

    let events = estimate(&doc.doc_id, opts(96)).expect("estimate");
    // started{total = pages}, one progress per page, done{report}.
    assert!(
        matches!(events[0], JobEvent::Started { total: 14, .. }),
        "{:?}",
        events[0]
    );
    let progress = events
        .iter()
        .filter(|e| matches!(e, JobEvent::Progress { .. }))
        .count();
    assert_eq!(progress, 14);
    let report = report_of(&events);
    // 2 placed + tracemonkey's 90 inside Form XObjects (Stage 8 counts them; none is above
    // 96 DPI, so only the two placed ones go).
    assert_eq!(report.images_total, 2 + TRACEMONKEY_FORM_IMAGES);
    assert_eq!(report.images_downsampled, 2);
    assert!(report.before_bytes > 0 && report.after_bytes > 0);
    assert!(
        report.after_bytes < report.before_bytes,
        "{} → {}",
        report.before_bytes,
        report.after_bytes
    );
    // The estimate did not touch the open document.
    assert_eq!(
        with_doc(&doc.doc_id, |d| Ok(d.generation)).unwrap(),
        generation
    );
    assert_eq!(images(&doc.doc_id, 0)[0].0, 1200);

    let info = apply(&doc.doc_id, report.token).expect("apply");
    assert_eq!(info.undo_label.as_deref(), Some("undo.compress"));
    assert_eq!(info.bytes, report.after_bytes);
    assert!(info.dirty);
    // 600 DPI → 96 DPI: 1200 px over 2 in → 192 px; 1600 px over 2.67 in → 256 px.
    let png = &images(&doc.doc_id, 0)[0];
    assert!((png.0 - 192).abs() <= 1, "png is {} px wide", png.0);
    assert_eq!(png.1, vec!["FlateDecode".to_string()]);
    let jpeg = &images(&doc.doc_id, 1)[0];
    assert!((jpeg.0 - 256).abs() <= 1, "jpeg is {} px wide", jpeg.0);
    assert_eq!(jpeg.1, vec!["DCTDecode".to_string()], "a JPEG stays a JPEG");
    let bytes = with_doc(&doc.doc_id, |d| Ok(d.bytes.to_vec())).unwrap();
    std::fs::write(out_dir().join("compress-96dpi.pdf"), bytes).unwrap();

    // The token is spent; one undo brings the originals back.
    assert_eq!(
        apply(&doc.doc_id, report.token).unwrap_err().code,
        ErrorCode::NotFound
    );
    let d = doc.doc_id.clone();
    with_state(move |st| registry::undo(st, &d, false)).expect("undo");
    assert_eq!(images(&doc.doc_id, 0)[0].0, 1200);
    assert_eq!(images(&doc.doc_id, 1)[0].0, 1600);
}

#[test]
fn compress_presets_and_page_subset() {
    let doc = open("tracemonkey.pdf");
    add_png(&doc.doc_id);
    // 300 DPI halves the 600-DPI image; a page subset without it downsamples nothing.
    let report = report_of(&estimate(&doc.doc_id, opts(300)).unwrap());
    assert_eq!(
        (report.images_total, report.images_downsampled),
        (1 + TRACEMONKEY_FORM_IMAGES, 1)
    );
    let subset = CompressOptions {
        target_dpi: 150,
        pages: Some(vec![3, 2]),
        optimize: None,
    };
    let events = estimate(&doc.doc_id, subset).unwrap();
    assert!(matches!(events[0], JobEvent::Started { total: 2, .. }));
    let report2 = report_of(&events);
    assert_eq!(report2.images_downsampled, 0);
    assert!(
        report2.images_total < TRACEMONKEY_FORM_IMAGES,
        "only pages 2 and 3 were scanned"
    );
    // A newer estimate replaced the older pending entry.
    assert_eq!(
        apply(&doc.doc_id, report.token).unwrap_err().code,
        ErrorCode::NotFound
    );

    // Bad options are refused before the job starts.
    assert_eq!(
        estimate(&doc.doc_id, opts(200)).unwrap_err().code,
        ErrorCode::InvalidArgument
    );
    let bad_page = CompressOptions {
        target_dpi: 96,
        pages: Some(vec![99]),
        optimize: None,
    };
    assert_eq!(
        estimate(&doc.doc_id, bad_page).unwrap_err().code,
        ErrorCode::InvalidArgument
    );
}

#[test]
fn compress_leaves_shared_images_alone() {
    // TAMReview's two ~105-DPI images (80×15 and 47×161 px) are the same XObjects on all 23
    // pages. Downsampling them per object would store one copy per page, so they are counted
    // and skipped; the top-level image downsampled is page 0's unshared 161×47 at 109 DPI.
    // Stage 8: one image inside a Form XObject is above 96 DPI too, and is replaced in place.
    let doc = open("TAMReview.pdf");
    let report = report_of(&estimate(&doc.doc_id, opts(96)).unwrap());
    assert!(
        report.images_total >= 46 + 8,
        "{} images",
        report.images_total
    );
    assert_eq!(report.images_downsampled, 2);
    assert!(report.after_bytes < report.before_bytes);

    // What the reader sees is the same page, only softer: every page renders close to before.
    let pages = doc.info.page_count;
    let before: Vec<Vec<u8>> = (0..pages).map(|p| render_page(&doc.doc_id, p)).collect();
    apply(&doc.doc_id, report.token).expect("apply");
    for p in 0..pages {
        let after = render_page(&doc.doc_id, p);
        let diff = mean_abs_diff(&before[p as usize], &after);
        assert!(diff < 6.0, "page {p} changed by {diff:.2} per channel");
    }
}

/// tracemonkey's images inside Form XObjects (small; below every preset).
const TRACEMONKEY_FORM_IMAGES: u32 = 90;

fn render_page(doc_id: &str, page: u16) -> Vec<u8> {
    let doc_id = doc_id.to_string();
    let buffer = with_state(move |st| {
        seepdf_lib::engine::render::tiles::render_raw_buffer(st, &doc_id, page, 0.5, None)
    })
    .expect("render");
    buffer[32..].to_vec()
}

fn mean_abs_diff(a: &[u8], b: &[u8]) -> f64 {
    assert_eq!(a.len(), b.len());
    let total: u64 = a
        .iter()
        .zip(b)
        .map(|(x, y)| (*x as i32 - *y as i32).unsigned_abs() as u64)
        .sum();
    total as f64 / a.len().max(1) as f64
}

#[test]
fn compress_discard_and_stale_apply() {
    let doc = open("tracemonkey.pdf");
    add_png(&doc.doc_id);
    let report = report_of(&estimate(&doc.doc_id, opts(150)).unwrap());
    let d = doc.doc_id.clone();
    let token = report.token;
    with_state(move |st| compress::discard(st, &d, token)).expect("discard");
    let pending = with_doc(&doc.doc_id, |d| Ok(d.compress_pending.is_some())).unwrap();
    assert!(!pending, "discard drops the pending bytes");
    assert_eq!(
        apply(&doc.doc_id, token).unwrap_err().code,
        ErrorCode::NotFound
    );

    // An edit after the estimate makes the result stale.
    let report = report_of(&estimate(&doc.doc_id, opts(150)).unwrap());
    let d = doc.doc_id.clone();
    with_state(move |st| {
        objects::add_text(
            st,
            &d,
            2,
            Rect::new(72.0, 600.0, 300.0, 640.0),
            "edited after the estimate",
            12.0,
            [0, 0, 0],
            TextAlign::Left,
        )
    })
    .unwrap();
    assert_eq!(
        apply(&doc.doc_id, report.token).unwrap_err().code,
        ErrorCode::Stale
    );
}

/// `(generation, dirty, undo label, bytes)` of the open document.
fn state_of(doc_id: &str) -> (DocGeneration, bool, Option<String>, Vec<u8>) {
    with_doc(doc_id, |d| {
        let info = d.info();
        Ok((
            info.doc_generation,
            info.dirty,
            info.undo_label,
            d.bytes.to_vec(),
        ))
    })
    .unwrap()
}

fn has_pending(doc_id: &str) -> bool {
    with_doc(doc_id, |d| Ok(d.compress_pending.is_some())).unwrap()
}

/// A result that saves nothing — no image downsampled, or no smaller file — is spent without
/// touching the document: no undo step, no dirty flag, no new generation. The dialog disables
/// 적용 for the same reports (`applyBlock` in `src/dialogs/compress.ts`).
#[test]
fn compress_apply_of_a_result_that_saves_nothing_is_a_no_op() {
    // Nothing to downsample: tracemonkey's own images are all below every preset.
    let doc = open("tracemonkey.pdf");
    let before = state_of(&doc.doc_id);
    let report = report_of(&estimate(&doc.doc_id, opts(96)).unwrap());
    assert_eq!(report.images_downsampled, 0);
    let info = apply(&doc.doc_id, report.token).expect("apply");
    assert_eq!(
        (info.doc_generation, info.dirty, info.undo_label.clone()),
        (before.0, before.1, before.2.clone()),
        "the DocInfo comes back unchanged"
    );
    assert_eq!(
        state_of(&doc.doc_id),
        before,
        "the document was not replaced"
    );
    assert!(!has_pending(&doc.doc_id), "the pending entry is dropped");
    assert_eq!(
        apply(&doc.doc_id, report.token).unwrap_err().code,
        ErrorCode::NotFound
    );

    // An image was downsampled but the file did not get smaller (PDFium's Flate can come out
    // larger). A fixture that grows is hard to come by, so the pending entry's `beforeBytes` is
    // brought down to the result's size.
    add_png(&doc.doc_id);
    let before = state_of(&doc.doc_id);
    let report = report_of(&estimate(&doc.doc_id, opts(96)).unwrap());
    assert_eq!(report.images_downsampled, 1);
    with_doc(&doc.doc_id, |d| {
        let pending = d.compress_pending.as_mut().expect("a pending result");
        pending.before_bytes = pending.bytes.len() as u64;
        Ok(())
    })
    .unwrap();
    let info = apply(&doc.doc_id, report.token).expect("apply");
    assert_eq!(info.doc_generation, before.0);
    assert_eq!(info.undo_label, before.2, "no `undo.compress` step");
    assert_eq!(
        state_of(&doc.doc_id),
        before,
        "the document was not replaced"
    );
    assert_eq!(
        images(&doc.doc_id, 0)[0].0,
        1200,
        "the original image stays"
    );
    assert!(!has_pending(&doc.doc_id));

    // The control: the same estimate, untouched, applies.
    let report = report_of(&estimate(&doc.doc_id, opts(96)).unwrap());
    let info = apply(&doc.doc_id, report.token).expect("apply");
    assert_eq!(info.doc_generation, before.0 + 1);
    assert_eq!(info.undo_label.as_deref(), Some("undo.compress"));
    assert!((images(&doc.doc_id, 0)[0].0 - 192).abs() <= 1);
}

#[test]
fn compress_cancel_mid_job_frees_the_scratch_copy() {
    let doc = open("gen/500p.pdf");
    // Hold the engine so the whole job is queued before any of it runs.
    let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
    engine()
        .send(Lane::Edit, "test/block", move |_| {
            let _ = release_rx.recv_timeout(Duration::from_secs(10));
        })
        .unwrap();

    let events: Arc<Mutex<Vec<JobEvent>>> = Arc::default();
    let token = engine().jobs.create();
    // `begin` must run before the job is queued; it waits behind the blocker, so run it on
    // a helper thread and release the blocker once it is queued.
    let (d, id) = (doc.doc_id.clone(), token.id);
    let begin =
        std::thread::spawn(move || with_state(move |st| compress::begin(st, &d, &opts(96), id)));
    std::thread::sleep(Duration::from_millis(20));
    release_tx.send(()).unwrap();
    let pages = begin.join().unwrap().expect("begin");
    assert!(with_doc(&doc.doc_id, |d| Ok(d.compress_work.is_some())).unwrap());

    // Block again, queue the job, cancel it, release.
    let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
    engine()
        .send(Lane::Edit, "test/block", move |_| {
            let _ = release_rx.recv_timeout(Duration::from_secs(10));
        })
        .unwrap();
    let sink_events = events.clone();
    let sink: JobSink = Arc::new(move |e| sink_events.lock().unwrap().push(e));
    let reporter =
        JobReporter::start_with(sink, engine().jobs.clone(), token.id, pages.len() as u32);
    compress::dispatch(engine(), &doc.doc_id, pages, &token, reporter);
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
    // Wait for the remaining (dropped) commands to drain, then check the scratch is gone.
    let (work, pending) = with_doc(&doc.doc_id, |d| {
        Ok((d.compress_work.is_some(), d.compress_pending.is_some()))
    })
    .unwrap();
    assert!(!work && !pending);
    assert!(!engine().jobs.cancel(token.id), "the job id is released");
}

// ---------------------------------------------------------------------------------------
// Stage 8 — images inside Form XObjects; encrypted documents
// ---------------------------------------------------------------------------------------

/// Puts one Form XObject holding a 1600×1200 JPEG (a real `/DCTDecode` stream) at 192×144 pt
/// — 600 DPI — on every page of `pages`: one stream, drawn by the same form on each page.
fn add_form_jpeg(doc_id: &str, pages: Vec<u16>) {
    use seepdf_lib::engine::raw::object::XObject;
    let mut jpeg = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 92)
        .encode_image(&noisy_rgb(1600, 1200))
        .unwrap();
    let doc_id = doc_id.to_string();
    with_state(move |st| {
        let pdfium = st.pdfium;
        let mut scratch = pdfium.create_new_pdf().unwrap();
        {
            let mut page = scratch
                .pages_mut()
                .create_page_at_end(PdfPagePaperSize::Custom(
                    PdfPoints::new(192.0),
                    PdfPoints::new(144.0),
                ))
                .unwrap();
            let mut object =
                PdfPageImageObject::new_from_jpeg_reader(&scratch, std::io::Cursor::new(jpeg))
                    .unwrap();
            object.scale(192.0, 144.0).unwrap();
            page.objects_mut().add_image_object(object).unwrap();
            page.regenerate_content().unwrap();
        }
        registry::mutate(
            st,
            &doc_id,
            MutateOpts::new("undo.objectAdd", ChangeReason::Edit).pages(pages.clone()),
            move |doc| {
                let bindings = doc.bindings();
                for &p in &pages {
                    doc.invalidate_page_handle(p);
                }
                let document = doc.pdf();
                let xobject = XObject::from_page(bindings, document, &scratch, 0)?;
                for &p in &pages {
                    let mut page = document.pages().get(p as PdfPageIndex).unwrap();
                    page.set_content_regeneration_strategy(
                        PdfPageContentRegenerationStrategy::Manual,
                    );
                    xobject.place(&page, [1.0, 0.0, 0.0, 1.0, 300.0, 72.0], None)?;
                    page.regenerate_content().unwrap();
                }
                Ok(())
            },
        )
    })
    .expect("add form jpeg");
}

/// `(pixel width, filters)` of every image inside a Form XObject on `page` (one level).
fn form_images(doc_id: &str, page: u16) -> Vec<(i32, Vec<String>)> {
    let doc_id = doc_id.to_string();
    with_doc(&doc_id, move |d| {
        let p = d.page(page)?;
        let mut out = Vec::new();
        for o in p.objects().iter() {
            if let Some(form) = o.as_x_object_form_object() {
                for i in 0..form.len() {
                    let child = form.get(i).unwrap();
                    if let Some(img) = child.as_image_object() {
                        out.push((
                            img.width().unwrap_or(0),
                            img.filters().iter().map(|f| f.name().to_string()).collect(),
                        ));
                    }
                }
            }
        }
        Ok(out)
    })
    .unwrap()
}

#[test]
fn compress_reaches_images_inside_form_xobjects() {
    let doc = open("tracemonkey.pdf");
    add_form_jpeg(&doc.doc_id, vec![0, 1]);
    let big = |p| {
        form_images(&doc.doc_id, p)
            .into_iter()
            .filter(|(w, _)| *w == 1600)
            .count()
    };
    assert_eq!((big(0), big(1)), (1, 1));

    let report = report_of(&estimate(&doc.doc_id, opts(96)).unwrap());
    assert_eq!(report.images_total, 2 + TRACEMONKEY_FORM_IMAGES);
    // One stream, drawn on two pages: both occurrences count.
    assert_eq!(report.images_downsampled, 2);
    assert!(
        report.after_bytes + 200_000 < report.before_bytes,
        "{} → {}",
        report.before_bytes,
        report.after_bytes
    );
    let before_render = render_page(&doc.doc_id, 0);
    apply(&doc.doc_id, report.token).expect("apply");
    for p in [0u16, 1] {
        let images = form_images(&doc.doc_id, p);
        // 1600 px over 2⅔ in → 256 px at 96 DPI; still a JPEG.
        assert!(
            !images.iter().any(|(w, _)| *w == 1600),
            "page {p}: {images:?}"
        );
        assert!(
            images
                .iter()
                .any(|(w, f)| (*w - 256).abs() <= 1 && f == &vec!["DCTDecode".to_string()]),
            "page {p}: {images:?}"
        );
    }
    let diff = mean_abs_diff(&before_render, &render_page(&doc.doc_id, 0));
    assert!(diff < 6.0, "the page still looks the same: {diff:.2}");
    // The replaced stream survives a save → reopen.
    let bytes = with_doc(&doc.doc_id, |d| Ok(d.bytes.to_vec())).unwrap();
    let reopened = with_state(move |st| registry::open(st, None, bytes, None)).expect("reopen");
    let copy = TestDoc {
        doc_id: reopened.doc_id.clone(),
        info: reopened,
    };
    assert!(form_images(&copy.doc_id, 1)
        .iter()
        .any(|(w, _)| (*w - 256).abs() <= 1));
}

/// A password-protected document compresses and stays protected by the same password — RC4
/// (the fixture) and AES-256 (our own `set_password` output). Images inside Form XObjects are
/// left alone there (their serialised streams are encrypted).
#[test]
fn compress_encrypted_document_keeps_its_password() {
    let aes_path = out_dir().join("compress-aes256.pdf");
    {
        let plain = open("tracemonkey.pdf");
        let (d, path) = (plain.doc_id.clone(), aes_path.display().to_string());
        with_state(move |st| {
            seepdf_lib::engine::security::set_password(
                st,
                &d,
                &path,
                Some("user"),
                "owner",
                Default::default(),
            )
        })
        .expect("set_password");
    }
    for (label, path) in [
        ("rc4", fixture("gen/encrypted-rc4-40.pdf")),
        ("aes256", aes_path.clone()),
    ] {
        let bytes = std::fs::read(&path).unwrap();
        let info = with_state(move |st| registry::open(st, Some(path), bytes, Some("user".into())))
            .expect("open with the user password");
        let doc = TestDoc {
            doc_id: info.doc_id.clone(),
            info,
        };
        assert!(doc.info.encrypted, "{label}");
        add_png(&doc.doc_id);
        if doc.info.page_count > 1 {
            add_form_jpeg(&doc.doc_id, vec![1]);
        }
        let report = report_of(&estimate(&doc.doc_id, opts(96)).expect("estimate"));
        assert_eq!(report.images_downsampled, 1, "{label}: the placed PNG only");
        let info = apply(&doc.doc_id, report.token).expect("apply");
        assert!(info.encrypted, "{label}");
        assert!(
            (images(&doc.doc_id, 0).last().unwrap().0 - 192).abs() <= 1,
            "{label}"
        );

        let saved = with_state({
            let d = doc.doc_id.clone();
            move |st| seepdf_lib::engine::save::serialize(st, &d)
        })
        .expect("serialize");
        let without = saved.clone();
        let err = with_state(move |st| registry::open(st, None, without, None).map(|i| i.doc_id))
            .unwrap_err();
        assert_eq!(
            err.code,
            ErrorCode::PasswordRequired,
            "{label}: still protected"
        );
        let reopened = with_state(move |st| registry::open(st, None, saved, Some("user".into())))
            .expect("reopen with the same password");
        let copy = TestDoc {
            doc_id: reopened.doc_id.clone(),
            info: reopened,
        };
        assert!(copy.info.encrypted, "{label}");
        assert!(
            (images(&copy.doc_id, 0).last().unwrap().0 - 192).abs() <= 1,
            "{label}"
        );
    }
}

// ---------------------------------------------------------------------------------------
// v0.3 pkg8 (X5): shared images, soft masks, structural optimisation
// ---------------------------------------------------------------------------------------

/// A Flate RGB image stream (and optionally an 8-bit gray `/SMask` gradient) built with lopdf.
fn image_stream(doc: &mut lopdf::Document, w: u32, h: u32, smask: bool) -> lopdf::ObjectId {
    use lopdf::{dictionary, Object, Stream};
    let mut dict = dictionary! {
        "Type" => "XObject",
        "Subtype" => "Image",
        "Width" => w as i64,
        "Height" => h as i64,
        "ColorSpace" => "DeviceRGB",
        "BitsPerComponent" => 8,
    };
    if smask {
        // Opaque on the left, fully transparent on the right.
        let alpha: Vec<u8> = (0..h)
            .flat_map(|_| (0..w).map(move |x| 255 - (x * 255 / (w - 1)) as u8))
            .collect();
        let mut mask = Stream::new(
            dictionary! {
                "Type" => "XObject",
                "Subtype" => "Image",
                "Width" => w as i64,
                "Height" => h as i64,
                "ColorSpace" => "DeviceGray",
                "BitsPerComponent" => 8,
            },
            alpha,
        );
        mask.compress().unwrap();
        let mask_id = doc.add_object(mask);
        dict.set("SMask", Object::Reference(mask_id));
    }
    let mut stream = Stream::new(dict, noisy_rgb(w, h).into_raw());
    stream.compress().unwrap();
    doc.add_object(stream)
}

/// A lopdf-built document: one page per entry, each drawing `draw` (content-stream text) with
/// `resources` (an `/XObject` dictionary).
fn lopdf_pdf(pages: Vec<(String, lopdf::Dictionary)>, doc: lopdf::Document) -> Vec<u8> {
    use lopdf::{dictionary, Object, Stream};
    let mut doc = doc;
    let pages_id = doc.new_object_id();
    let mut kids = Vec::new();
    for (content, xobjects) in pages {
        let content_id = doc.add_object(Stream::new(dictionary! {}, content.into_bytes()));
        let page_id = doc.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
            "Contents" => content_id,
            "Resources" => dictionary! { "XObject" => Object::Dictionary(xobjects) },
        });
        kids.push(Object::Reference(page_id));
    }
    let count = kids.len() as i64;
    doc.objects.insert(
        pages_id,
        Object::Dictionary(dictionary! { "Type" => "Pages", "Kids" => kids, "Count" => count }),
    );
    let catalog = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
    doc.trailer.set("Root", catalog);
    let mut out = Vec::new();
    doc.save_to(&mut out).unwrap();
    out
}

fn open_bytes(bytes: Vec<u8>, name: &str) -> TestDoc {
    std::fs::write(out_dir().join(name), &bytes).unwrap();
    let info = with_state(move |st| registry::open(st, None, bytes, None)).expect("open");
    TestDoc {
        doc_id: info.doc_id.clone(),
        info,
    }
}

fn estimate_with(doc_id: &str, dpi: u32, optimize: bool) -> CompressReport {
    report_of(
        &estimate(
            doc_id,
            CompressOptions {
                target_dpi: dpi,
                pages: None,
                optimize: Some(optimize),
            },
        )
        .expect("estimate"),
    )
}

/// `(width, SMask width)` of every image XObject in the document's current bytes.
fn image_widths(doc_id: &str) -> Vec<(i64, Option<i64>)> {
    let bytes = with_doc(doc_id, |d| Ok(d.bytes.to_vec())).unwrap();
    let doc = lopdf::Document::load_mem(&bytes).unwrap();
    let mut out = Vec::new();
    for object in doc.objects.values() {
        let Ok(stream) = object.as_stream() else {
            continue;
        };
        let image = stream
            .dict
            .get(b"Subtype")
            .and_then(|s| s.as_name())
            .map(|n| n == b"Image")
            .unwrap_or(false);
        let is_mask = stream
            .dict
            .get(b"ColorSpace")
            .and_then(|s| s.as_name())
            .map(|n| n == b"DeviceGray")
            .unwrap_or(false);
        if !image || is_mask {
            continue;
        }
        let smask = stream
            .dict
            .get(b"SMask")
            .and_then(|m| m.as_reference())
            .ok()
            .and_then(|id| doc.get_object(id).ok())
            .and_then(|o| o.as_stream().ok())
            .and_then(|s| s.dict.get(b"Width").and_then(|w| w.as_i64()).ok());
        out.push((stream.dict.get(b"Width").unwrap().as_i64().unwrap(), smask));
    }
    out
}

/// X5: one image XObject drawn on three pages (432 DPI) is downsampled **once** — its one
/// stream object is replaced — and every page renders as before, only softer.
#[test]
fn compress_downsamples_a_shared_image_once() {
    use lopdf::{dictionary, Object};
    let mut doc = lopdf::Document::with_version("1.5");
    let im = image_stream(&mut doc, 1200, 800, false);
    let draw = "q 200 0 0 133.33 72 500 cm /Im1 Do Q".to_string();
    let pages = (0..3)
        .map(|_| (draw.clone(), dictionary! { "Im1" => Object::Reference(im) }))
        .collect();
    let doc = open_bytes(lopdf_pdf(pages, doc), "shared-image.pdf");
    let before: Vec<Vec<u8>> = (0..3).map(|p| render_page(&doc.doc_id, p)).collect();

    let report = estimate_with(&doc.doc_id, 150, false);
    assert_eq!(report.images_total, 3);
    assert_eq!(
        report.images_downsampled, 3,
        "every occurrence of the one stream"
    );
    assert!(
        report.after_bytes * 3 < report.before_bytes,
        "{} → {}",
        report.before_bytes,
        report.after_bytes
    );
    apply(&doc.doc_id, report.token).expect("apply");
    let widths = image_widths(&doc.doc_id);
    assert_eq!(widths.len(), 1, "still one shared stream: {widths:?}");
    // 1200 px over 200 pt = 432 DPI → 150 DPI: ~417 px.
    assert!((widths[0].0 - 417).abs() <= 1, "{widths:?}");
    for p in 0..3u16 {
        let diff = mean_abs_diff(&before[p as usize], &render_page(&doc.doc_id, p));
        assert!(diff < 8.0, "page {p} changed by {diff:.2}");
    }
}

/// X5: an image with an `/SMask` is downsampled together with its mask, and the page still
/// shows the red rectangle through the transparent half.
#[test]
fn compress_keeps_soft_mask_transparency() {
    use lopdf::{dictionary, Object};
    let mut doc = lopdf::Document::with_version("1.5");
    let im = image_stream(&mut doc, 1200, 800, true);
    // A red rectangle, then the half-transparent image over it.
    let draw = "1 0 0 rg 72 500 200 133.33 re f q 200 0 0 133.33 72 500 cm /Im1 Do Q".to_string();
    let doc = open_bytes(
        lopdf_pdf(
            vec![(draw, dictionary! { "Im1" => Object::Reference(im) })],
            doc,
        ),
        "smask-image.pdf",
    );
    let before = render_page(&doc.doc_id, 0);
    let report = estimate_with(&doc.doc_id, 150, false);
    assert_eq!(report.images_downsampled, 1);
    assert!(report.after_bytes < report.before_bytes);
    apply(&doc.doc_id, report.token).expect("apply");
    let widths = image_widths(&doc.doc_id);
    assert_eq!(widths.len(), 1, "{widths:?}");
    assert!((widths[0].0 - 417).abs() <= 1, "{widths:?}");
    let dump = with_doc(&doc.doc_id, |d| Ok(d.bytes.to_vec())).unwrap();
    std::fs::write(out_dir().join("smask-after.pdf"), &dump).unwrap();
    let mask = widths[0].1.expect("the image keeps its /SMask");
    assert!(
        (mask - 417).abs() <= 1,
        "the mask is downsampled by the same factor: {mask}"
    );

    let after = render_page(&doc.doc_id, 0);
    assert!(mean_abs_diff(&before, &after) < 8.0);
    // At 0.5× the image spans x 36..136, y (792-633.33)/2..(792-500)/2; its right edge is
    // transparent, so the pixel there is still red.
    let w = (612.0f32 * 0.5).round() as usize;
    let (x, y) = (133usize, 120usize);
    let px = &after[(y * w + x) * 4..(y * w + x) * 4 + 3];
    assert!(
        px[0] > 200 && px[1] < 80 && px[2] < 80,
        "the transparent side shows the red underneath: {px:?}"
    );
}

/// X5 구조 최적화: an image listed in a page's resources but never drawn, and the empty
/// content streams, are dropped, and the file is written with object streams — the estimate
/// shows the saving and 적용 applies it even with no image downsampled.
#[test]
fn compress_structural_pass_drops_unused_objects() {
    use lopdf::{dictionary, Object};
    let mut doc = lopdf::Document::with_version("1.5");
    let used = image_stream(&mut doc, 60, 40, false);
    let unused = image_stream(&mut doc, 900, 700, false);
    let draw = "q 60 0 0 40 72 500 cm /Im1 Do Q".to_string();
    let doc = open_bytes(
        lopdf_pdf(
            vec![(
                draw,
                dictionary! { "Im1" => Object::Reference(used), "Unused" => Object::Reference(unused) },
            )],
            doc,
        ),
        "orphan-image.pdf",
    );
    let before = render_page(&doc.doc_id, 0);

    // Without the option the unused image stays (PDFium keeps everything reachable).
    let plain = estimate_with(&doc.doc_id, 300, false);
    assert_eq!(plain.images_downsampled, 0);
    assert!(plain.after_bytes as f64 > plain.before_bytes as f64 * 0.9);

    let report = estimate_with(&doc.doc_id, 300, true);
    assert_eq!(report.images_downsampled, 0);
    assert!(
        report.after_bytes * 5 < report.before_bytes,
        "the unused 900×700 image is gone: {} → {}",
        report.before_bytes,
        report.after_bytes
    );
    let info = apply(&doc.doc_id, report.token).expect("apply");
    assert_eq!(info.undo_label.as_deref(), Some("undo.compress"));
    assert_eq!(info.bytes, report.after_bytes);
    assert_eq!(image_widths(&doc.doc_id), vec![(60, None)]);
    let bytes = with_doc(&doc.doc_id, |d| Ok(d.bytes.to_vec())).unwrap();
    assert!(
        bytes.windows(7).any(|w| w == b"/ObjStm"),
        "written with object streams"
    );
    assert!(mean_abs_diff(&before, &render_page(&doc.doc_id, 0)) < 1.0);
}

#[test]
fn optimize_structure_keeps_what_is_used() {
    use lopdf::{dictionary, Object};
    // Two pages share one resource dictionary: an image used only by page 2 must survive.
    let mut doc = lopdf::Document::with_version("1.5");
    let a = image_stream(&mut doc, 20, 20, false);
    let b = image_stream(&mut doc, 20, 20, false);
    let res = doc.add_object(dictionary! {
        "XObject" => dictionary! { "A" => Object::Reference(a), "B" => Object::Reference(b) },
    });
    let pages_id = doc.new_object_id();
    let mut kids = Vec::new();
    for draw in ["q 20 0 0 20 0 0 cm /A Do Q", "q 20 0 0 20 0 0 cm /B Do Q"] {
        let content = doc.add_object(lopdf::Stream::new(dictionary! {}, draw.as_bytes().to_vec()));
        let page = doc.add_object(dictionary! {
            "Type" => "Page", "Parent" => pages_id, "Contents" => content, "Resources" => res,
            "MediaBox" => vec![0.into(), 0.into(), 100.into(), 100.into()],
        });
        kids.push(Object::Reference(page));
    }
    doc.objects.insert(
        pages_id,
        Object::Dictionary(dictionary! { "Type" => "Pages", "Kids" => kids, "Count" => 2 }),
    );
    let catalog = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
    doc.trailer.set("Root", catalog);
    let mut bytes = Vec::new();
    doc.save_to(&mut bytes).unwrap();

    let out = compress::optimize_structure(&bytes).expect("optimize");
    let doc = lopdf::Document::load_mem(&out).unwrap();
    let images = doc
        .objects
        .values()
        .filter(|o| {
            o.as_stream()
                .ok()
                .and_then(|s| s.dict.get(b"Subtype").ok().and_then(|n| n.as_name().ok()))
                == Some(b"Image".as_slice())
        })
        .count();
    assert_eq!(images, 2, "both images are used by one of the two pages");
}
