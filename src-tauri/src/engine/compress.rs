//! Compress — raster downsampling to a target DPI with a measured estimate. P1-5,
//! `IPC_CONTRACT.md` §7.6a (`compress_estimate` / `compress_apply` / `compress_discard`).
//!
//! The estimate never touches the open document. [`begin`] serialises it (the same bytes a
//! save would write) and loads them into a **scratch** `PdfDocument` held in
//! [`OpenDoc::compress_work`]; the job then runs as one `Lane::Background` command per page
//! (tiles interleave, Cancel is observed within a page), and [`finish`] serialises the scratch
//! copy, verifies it and parks the bytes as a [`Pending`] result behind a token. `apply` is a
//! [`registry::mutate_bytes`] replace — one undo step, `undo.compress`.
//!
//! ## Two passes
//!
//! 1. [`scan_page`] lists every top-level image whose effective DPI (PDFium's
//!    `FPDFImageObj_GetImageMetadata`, i.e. pixels ÷ on-page size × 72) exceeds the target by
//!    more than [`DPI_TOLERANCE`], and hashes its encoded stream.
//! 2. [`process_page`] downsamples the candidates whose hash occurs **once**. An image
//!    XObject shared by many pages (a logo) is one stream in the file, but PDFium's
//!    `FPDFImageObj_SetBitmap` gives every object a stream of its own — "compressing" it would
//!    store N copies. Shared images are counted and left alone.
//!
//! ## What is skipped
//!
//! Images with transparency (`/SMask`, `/Mask`, soft-mask graphics state —
//! `FPDFPageObj_HasTransparency`), 1-bit images and stencil masks (CCITT / JBIG2 are already
//! small, and resampling would turn them into 8-bit), images inside Form XObjects (not
//! top-level page objects) and bitmaps PDFium cannot hand back in a format we can re-encode.
//! They count in `imagesTotal` but not in `imagesDownsampled`.
//!
//! ## Encoding
//!
//! A `/DCTDecode` (or `/JPXDecode`) original is re-encoded as JPEG (quality [`JPEG_QUALITY`])
//! and swapped in with `FPDFImageObj_LoadJpegFileInline` on the existing object — matrix,
//! clip path and graphics state are kept — **only if** the new JPEG is smaller than the old
//! stream. Anything else goes through `FPDFImageObj_SetBitmap` with a 24-bit BGR or 8-bit gray
//! bitmap, which PDFium always writes as `/FlateDecode`; that can come out larger, and the
//! report's `afterBytes` is how the user finds out.

use crate::engine::export::job::JobReporter;
use crate::engine::jobs::JobToken;
use crate::engine::raw;
use crate::engine::registry::{self, MutateOpts};
use crate::engine::save;
use crate::engine::types::{CmdStatus, EngineState};
use crate::engine::{EngineHandle, Lane, Submit};
use crate::ipc::error::PdfiumResultExt;
use crate::ipc::types::{
    ChangeReason, CompressOptions, CompressReport, DocGeneration, DocInfo, JobId, PageIndex,
};
use crate::ipc::{EngineError, ErrorCode};
use pdfium_render::prelude::*;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

/// The contract's `CompressPreset`.
pub const PRESETS: [u32; 3] = [300, 150, 96];
/// An image is downsampled only when its effective DPI is above `target × (1 + this)`:
/// re-encoding a 310-DPI scan for a 300-DPI preset costs quality and gains nothing.
pub const DPI_TOLERANCE: f32 = 0.1;
/// JPEG quality for re-encoded `/DCTDecode` images.
pub const JPEG_QUALITY: u8 = 80;

static NEXT_TOKEN: AtomicU64 = AtomicU64::new(0);

/// One image worth downsampling, found by [`scan_page`].
#[derive(Debug, Clone, Copy)]
struct Candidate {
    index: usize,
    hash: u64,
    dpi_x: f32,
    dpi_y: f32,
}

/// A running estimate. Lives in [`OpenDoc::compress_work`](registry::OpenDoc).
pub struct Work<'p> {
    job_id: JobId,
    scratch: PdfDocument<'p>,
    target_dpi: u32,
    base_generation: DocGeneration,
    before_bytes: u64,
    page_count: u16,
    password: Option<String>,
    started: Instant,
    /// Encoded-stream hash → how many candidate objects use it.
    seen: HashMap<u64, u32>,
    candidates: HashMap<PageIndex, Vec<Candidate>>,
    images_total: u32,
    images_downsampled: u32,
}

/// A finished estimate, waiting for `compress_apply` / `compress_discard`.
#[derive(Debug, Clone)]
pub struct Pending {
    pub token: u64,
    pub bytes: Arc<[u8]>,
    /// The generation the estimate was made from; `apply` refuses (`stale`) once the document
    /// has moved on.
    pub base_generation: DocGeneration,
}

/// Validates the options, snapshots the document into a scratch copy and returns the pages
/// the job will visit. A previous pending result (or running estimate) is dropped.
pub fn begin(
    st: &mut EngineState<'_>,
    doc_id: &str,
    options: &CompressOptions,
    job_id: JobId,
) -> Result<Vec<PageIndex>, EngineError> {
    if !PRESETS.contains(&options.target_dpi) {
        return Err(EngineError::invalid(format!(
            "targetDpi must be one of 300, 150, 96 (got {})",
            options.target_dpi
        )));
    }
    let count = st.doc(doc_id)?.page_count();
    let pages: Vec<PageIndex> = match &options.pages {
        None => (0..count).collect(),
        Some(list) => {
            let mut pages = list.clone();
            pages.sort_unstable();
            pages.dedup();
            if let Some(&max) = pages.last() {
                if max >= count {
                    return Err(EngineError::invalid(format!(
                        "page {max} is out of range (page count {count})"
                    ))
                    .with_page(max));
                }
            }
            pages
        }
    };
    {
        let doc = st.doc_mut(doc_id)?;
        doc.compress_work = None;
        doc.compress_pending = None;
    }
    let bytes = save::serialize(st, doc_id)?;
    let before_bytes = bytes.len() as u64;
    let pdfium = st.pdfium;
    let doc = st.doc_mut(doc_id)?;
    let scratch = pdfium
        .load_pdf_from_byte_vec(bytes, doc.password.as_deref())
        .map_err(|e| EngineError::pdfium("load scratch copy", e))?;
    doc.compress_work = Some(Work {
        job_id,
        scratch,
        target_dpi: options.target_dpi,
        base_generation: doc.generation,
        before_bytes,
        page_count: count,
        password: doc.password.clone(),
        started: Instant::now(),
        seen: HashMap::new(),
        candidates: HashMap::new(),
        images_total: 0,
        images_downsampled: 0,
    });
    Ok(pages)
}

fn work<'a, 'p>(
    st: &'a mut EngineState<'p>,
    doc_id: &str,
    job_id: JobId,
) -> Result<&'a mut Work<'p>, EngineError> {
    match st.doc_mut(doc_id)?.compress_work.as_mut() {
        Some(work) if work.job_id == job_id => Ok(work),
        // A newer estimate replaced this one, or it was aborted.
        _ => Err(EngineError::cancelled("the compress estimate was superseded")),
    }
}

fn open_page<'p>(scratch: &PdfDocument<'p>, page: PageIndex) -> Result<PdfPage<'p>, EngineError> {
    let mut handle = scratch
        .pages()
        .get(page as PdfPageIndex)
        .ctx(&format!("load page {page}"))?;
    handle.set_content_regeneration_strategy(PdfPageContentRegenerationStrategy::Manual);
    Ok(handle)
}

/// Pass 1 for one page: count its images, list the candidates and hash their streams.
pub fn scan_page(
    st: &mut EngineState<'_>,
    doc_id: &str,
    job_id: JobId,
    page: PageIndex,
) -> Result<(), EngineError> {
    let work = work(st, doc_id, job_id)?;
    let target = work.target_dpi as f32 * (1.0 + DPI_TOLERANCE);
    let handle = open_page(&work.scratch, page)?;
    let mut found = Vec::new();
    for (index, object) in handle.objects().iter().enumerate() {
        let Some(image) = object.as_image_object() else {
            continue;
        };
        work.images_total += 1;
        if object.has_transparency() || image.bits_per_pixel().unwrap_or(1) <= 1 {
            continue;
        }
        let (Ok(dpi_x), Ok(dpi_y)) = (image.horizontal_dpi(), image.vertical_dpi()) else {
            continue;
        };
        if !dpi_x.is_finite() || !dpi_y.is_finite() || dpi_x.min(dpi_y) <= target {
            continue;
        }
        let Ok(data) = image.get_raw_image_data() else {
            continue;
        };
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        data.hash(&mut hasher);
        let hash = hasher.finish();
        *work.seen.entry(hash).or_insert(0) += 1;
        found.push(Candidate {
            index,
            hash,
            dpi_x,
            dpi_y,
        });
    }
    drop(handle);
    if !found.is_empty() {
        work.candidates.insert(page, found);
    }
    Ok(())
}

/// Pass 2 for one page: downsample its unshared candidates. Returns how many were replaced.
pub fn process_page(
    st: &mut EngineState<'_>,
    doc_id: &str,
    job_id: JobId,
    page: PageIndex,
) -> Result<u32, EngineError> {
    let bindings = st.doc(doc_id)?.bindings();
    let work = work(st, doc_id, job_id)?;
    let Some(candidates) = work.candidates.remove(&page) else {
        return Ok(0);
    };
    let target = work.target_dpi as f32;
    let handle = open_page(&work.scratch, page)?;
    let mut replaced = 0u32;
    for c in candidates {
        if work.seen.get(&c.hash).copied().unwrap_or(0) != 1 {
            continue;
        }
        if downsample(bindings, &handle, c, target)? {
            replaced += 1;
        }
    }
    if replaced > 0 {
        let mut handle = handle;
        handle
            .regenerate_content()
            .ctx("regenerate page content")?;
    }
    work.images_downsampled += replaced;
    Ok(replaced)
}

/// Downsamples one image in place. `Ok(false)` when it was left alone (nothing to gain, or a
/// bitmap we cannot re-encode faithfully).
fn downsample(
    bindings: &dyn PdfiumLibraryBindings,
    page: &PdfPage<'_>,
    c: Candidate,
    target_dpi: f32,
) -> Result<bool, EngineError> {
    let (decoded, old_len, jpeg_source) = {
        let object = page
            .objects()
            .get(c.index)
            .ctx(&format!("load object {}", c.index))?;
        let Some(image) = object.as_image_object() else {
            return Ok(false);
        };
        let jpeg_source = image
            .filters()
            .iter()
            .any(|f| matches!(f.name(), "DCTDecode" | "JPXDecode"));
        let old_len = image.get_raw_image_data().map(|d| d.len()).unwrap_or(0);
        let Ok(decoded) = image.get_raw_image() else {
            return Ok(false);
        };
        (decoded, old_len, jpeg_source)
    };
    let (w, h) = (decoded.width(), decoded.height());
    if w == 0 || h == 0 {
        return Ok(false);
    }
    let nw = ((w as f32 * target_dpi / c.dpi_x).round() as u32).clamp(1, w);
    let nh = ((h as f32 * target_dpi / c.dpi_y).round() as u32).clamp(1, h);
    if nw >= w && nh >= h {
        return Ok(false);
    }
    let gray = matches!(decoded, image::DynamicImage::ImageLuma8(_));
    let resized = decoded.resize_exact(nw, nh, image::imageops::FilterType::Triangle);

    if jpeg_source {
        let mut jpeg = Vec::new();
        let mut encoder =
            image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, JPEG_QUALITY);
        let encoded = if gray {
            encoder.encode_image(&resized.to_luma8())
        } else {
            encoder.encode_image(&resized.to_rgb8())
        };
        if encoded.is_err() || (old_len > 0 && jpeg.len() >= old_len) {
            return Ok(false);
        }
        raw::object::replace_with_jpeg(bindings, page, c.index, &jpeg)?;
        return Ok(true);
    }

    // Flate: 24-bit BGR or 8-bit gray, never BGRA — an alpha-format bitmap makes PDFium write
    // an `/SMask` stream next to every image.
    let (format, bpp) = if gray {
        (PdfBitmapFormat::Gray, 1usize)
    } else {
        (PdfBitmapFormat::BGR, 3usize)
    };
    let stride = (nw as usize * bpp).div_ceil(4) * 4;
    let mut buffer = vec![0u8; stride * nh as usize];
    if gray {
        let luma = resized.to_luma8();
        for (y, row) in luma.rows().enumerate() {
            for (x, px) in row.enumerate() {
                buffer[y * stride + x] = px.0[0];
            }
        }
    } else {
        let rgb = resized.to_rgb8();
        for (y, row) in rgb.rows().enumerate() {
            for (x, px) in row.enumerate() {
                let o = y * stride + x * 3;
                buffer[o] = px.0[2];
                buffer[o + 1] = px.0[1];
                buffer[o + 2] = px.0[0];
            }
        }
    }
    let bitmap = PdfBitmap::from_bytes(nw as Pixels, nh as Pixels, format, &mut buffer)
        .ctx("create bitmap")?;
    let mut object = page
        .objects()
        .get(c.index)
        .ctx(&format!("load object {}", c.index))?;
    let Some(image) = object.as_image_object_mut() else {
        return Ok(false);
    };
    image.set_bitmap(&bitmap).ctx("set_bitmap")?;
    Ok(true)
}

/// Serialises and verifies the scratch copy, parks it as the document's [`Pending`] result.
pub fn finish(
    st: &mut EngineState<'_>,
    doc_id: &str,
    job_id: JobId,
) -> Result<CompressReport, EngineError> {
    work(st, doc_id, job_id)?;
    let doc = st.doc_mut(doc_id)?;
    let work = doc.compress_work.take().expect("checked above");
    let bytes = raw::save::save_as_copy(
        doc.bindings(),
        &work.scratch,
        raw::save::SaveFlags::NoIncremental,
    )?;
    let Work {
        scratch,
        base_generation,
        before_bytes,
        page_count,
        password,
        started,
        images_total,
        images_downsampled,
        ..
    } = work;
    drop(scratch);
    save::verify_bytes(st, &bytes, page_count, password)?;
    let token = NEXT_TOKEN.fetch_add(1, Ordering::Relaxed) + 1;
    let report = CompressReport {
        token,
        before_bytes,
        after_bytes: bytes.len() as u64,
        images_total,
        images_downsampled,
        elapsed_ms: started.elapsed().as_secs_f64() * 1000.0,
    };
    st.doc_mut(doc_id)?.compress_pending = Some(Pending {
        token,
        bytes: Arc::from(bytes.into_boxed_slice()),
        base_generation,
    });
    Ok(report)
}

/// Drops the running estimate of `job_id` (cancel / error). No-op for any other job.
pub fn abort(st: &mut EngineState<'_>, doc_id: &str, job_id: JobId) {
    if let Ok(doc) = st.doc_mut(doc_id) {
        if doc.compress_work.as_ref().map(|w| w.job_id) == Some(job_id) {
            doc.compress_work = None;
        }
    }
}

fn take_pending(
    st: &mut EngineState<'_>,
    doc_id: &str,
    token: u64,
) -> Result<Pending, EngineError> {
    let doc = st.doc_mut(doc_id)?;
    match &doc.compress_pending {
        Some(p) if p.token == token => Ok(doc.compress_pending.take().expect("matched")),
        _ => Err(EngineError::not_found(format!(
            "no pending compress result with token {token}"
        ))),
    }
}

/// `compress_apply` — replaces the document with the pending bytes as one undo step.
pub fn apply(st: &mut EngineState<'_>, doc_id: &str, token: u64) -> Result<DocInfo, EngineError> {
    let pending = take_pending(st, doc_id, token)?;
    if st.doc(doc_id)?.generation != pending.base_generation {
        return Err(EngineError::stale(
            "the document changed after the estimate; estimate again",
        ));
    }
    let opts = MutateOpts::new("undo.compress", ChangeReason::Edit).all_pages();
    let bytes = pending.bytes;
    registry::mutate_bytes(st, doc_id, opts, move |_, _| Ok(bytes.to_vec()))
}

/// `compress_discard`. Unknown tokens are not an error: the entry may already be gone
/// (document replaced, newer estimate), which is what the caller wanted anyway.
pub fn discard(st: &mut EngineState<'_>, doc_id: &str, token: u64) -> Result<(), EngineError> {
    let doc = st.doc_mut(doc_id)?;
    if doc.compress_pending.as_ref().map(|p| p.token) == Some(token) {
        doc.compress_pending = None;
    }
    Ok(())
}

/// Queues the job: one scan command per page, one process command per page (progress), one
/// finish command (`done` with the report). All share `token`, so a cancel drops the rest
/// and the first dropped command reports `cancelled` and frees the scratch copy.
///
/// [`begin`] must have succeeded for `token.id` first.
pub fn dispatch(
    engine: &EngineHandle,
    doc_id: &str,
    pages: Vec<PageIndex>,
    token: &JobToken,
    reporter: Arc<JobReporter>,
) {
    let job_id = token.id;
    let submit = |label: &'static str| {
        Submit::new(Lane::Background, label).cancel(token.cancel.clone())
    };
    let mut queued: Result<(), EngineError> = Ok(());
    for &page in &pages {
        let (doc_id, reporter) = (doc_id.to_string(), reporter.clone());
        queued = queued.and_then(|_| {
            engine.dispatch(submit("compress_scan").page(page), move |st, status| {
                if !proceed(&reporter, st, &doc_id, job_id, status) {
                    return;
                }
                if let Err(e) = scan_page(st, &doc_id, job_id, page) {
                    abort(st, &doc_id, job_id);
                    fail(&reporter, e);
                }
            })
        });
    }
    for &page in &pages {
        let (doc_id, reporter) = (doc_id.to_string(), reporter.clone());
        queued = queued.and_then(|_| {
            engine.dispatch(submit("compress_page").page(page), move |st, status| {
                if !proceed(&reporter, st, &doc_id, job_id, status) {
                    return;
                }
                match process_page(st, &doc_id, job_id, page) {
                    Ok(_) => {
                        reporter.step(Some(page), None);
                    }
                    Err(e) => {
                        abort(st, &doc_id, job_id);
                        fail(&reporter, e);
                    }
                }
            })
        });
    }
    {
        let (doc_id, reporter) = (doc_id.to_string(), reporter.clone());
        queued = queued.and_then(|_| {
            engine.dispatch(submit("compress_finish"), move |st, status| {
                if !proceed(&reporter, st, &doc_id, job_id, status) {
                    return;
                }
                match finish(st, &doc_id, job_id) {
                    Ok(report) => reporter.finish_with_report(report),
                    Err(e) => {
                        abort(st, &doc_id, job_id);
                        fail(&reporter, e);
                    }
                }
            })
        });
    }
    if let Err(e) = queued {
        reporter.fail(e);
    }
}

/// Every job command's preamble: bail out once the job has a terminal event, and turn a
/// dropped (cancelled) command into `cancelled`, freeing the scratch copy either way.
fn proceed(
    reporter: &JobReporter,
    st: &mut EngineState<'_>,
    doc_id: &str,
    job_id: JobId,
    status: CmdStatus,
) -> bool {
    if reporter.is_finished() {
        abort(st, doc_id, job_id);
        return false;
    }
    if status != CmdStatus::Run {
        abort(st, doc_id, job_id);
        reporter.cancel();
        return false;
    }
    true
}

/// A superseded job reports `cancelled`, not an error.
fn fail(reporter: &JobReporter, e: EngineError) {
    if e.code == ErrorCode::Cancelled {
        reporter.cancel();
    } else {
        reporter.fail(e);
    }
}
