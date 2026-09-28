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
