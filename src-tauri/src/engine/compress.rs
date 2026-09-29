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
//!    store N copies. Since v0.3 (X5) a shared stream goes through the Stage 8 splice below
//!    instead: re-encoded once, written into its one stream object.
//!
//! ## v0.3 (X5): soft masks and 구조 최적화
//!
//! PDFium's `FPDFPageObj_HasTransparency` does not see an image's own `/SMask` or `/Mask`, and
//! `FPDFImageObj_SetBitmap` silently drops them — so [`begin`] reads the image dictionaries
//! with lopdf ([`image_masks`]). A soft-masked image is re-encoded and spliced, and its
//! `/SMask` (8-bit gray) is resampled by the same factor (left at its resolution when lopdf
//! cannot decode it — the mask need not match the image); an all-255 mask counts as none. A
//! `/Mask` image is left alone. The optional structural pass ([`optimize_structure`]) drops
//! page resources the content never uses, empty content streams and unreferenced objects, and
//! writes object streams; it is kept only when smaller and PDFium still opens the result.
//!
//! ## Images inside Form XObjects (Stage 8)
//!
//! [`scan_page`] also walks every Form XObject on the page (nested ones too, 8 levels deep)
//! and measures each image's effective DPI through the **combined** matrix (image ∘ every
//! enclosing form's matrix), i.e. its size on the page. PDFium cannot rewrite a Form
//! XObject's content stream to point at a new image, so these are not replaced through
//! PDFium: [`process_page`] decodes and re-encodes them, and [`finish`] splices the new data
//! into the **same image stream object** of the serialised file with `lopdf` (matched by the
//! hash and length of the encoded stream). Every form that draws that image — on every page —
//! gets the smaller one, so sharing is not a problem here; the only requirement is that every
//! occurrence of the image the scan saw is above the target DPI (the lowest one sets the
//! scale). Skipped on an encrypted document (the serialised streams are encrypted), and a
//! re-encode that is not smaller than the original is dropped.
//!
//! ## What is skipped
//!
//! Images with a `/Mask` or drawn with transparent graphics state
//! (`FPDFPageObj_HasTransparency`), 1-bit images and stencil masks (CCITT / JBIG2 are already
//! small, and resampling would turn them into 8-bit), shared, soft-masked or nested images of
//! an encrypted document (no splice there), shared ones with an occurrence at or below the
//! target, and bitmaps PDFium cannot hand back in a format we can re-encode. They count in
//! `imagesTotal` but not in `imagesDownsampled`.
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
use std::collections::{HashMap, HashSet};
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

/// An image inside a Form XObject worth downsampling (Stage 8): its path of object indices
/// from the page (form, [form, …], image).
#[derive(Debug, Clone)]
struct NestedCandidate {
    path: Vec<usize>,
    hash: u64,
}

/// Every candidate occurrence of one nested image: the lowest effective DPI sets the scale.
#[derive(Debug, Clone, Copy)]
struct NestedPlan {
    dpi_x: f32,
    dpi_y: f32,
    count: u32,
}

/// A re-encoded nested image, spliced into the serialised file by [`finish`].
#[derive(Debug, Clone)]
struct NestedReplacement {
    hash: u64,
    old_len: usize,
    /// The final stream data, already encoded (and smaller than `old_len`).
    data: Vec<u8>,
    /// `/DCTDecode` data; otherwise `/FlateDecode`.
    jpeg: bool,
    width: u32,
    height: u32,
    gray: bool,
    occurrences: u32,
    /// v0.3 (X5): the image has an `/SMask`, which is downsampled by the same factor.
    smask: bool,
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
    /// Encoded-stream hash → how many image objects (top-level or inside a form, candidate or
    /// not) use it.
    seen: HashMap<u64, u32>,
    candidates: HashMap<PageIndex, Vec<Candidate>>,
    /// Stage 8: images inside Form XObjects.
    nested: HashMap<PageIndex, Vec<NestedCandidate>>,
    nested_plan: HashMap<u64, NestedPlan>,
    nested_done: HashSet<u64>,
    replacements: Vec<NestedReplacement>,
    /// The serialised streams are encrypted, so nested images cannot be spliced.
    encrypted: bool,
    images_total: u32,
    images_downsampled: u32,
    /// v0.3 (X5): top-level images with transparency above the target — spliced when the
    /// lopdf pass finds their transparency is an `/SMask` (and nothing else).
    masked: HashMap<PageIndex, Vec<Candidate>>,
    masked_seen: HashMap<u64, u32>,
    masked_plan: HashMap<u64, NestedPlan>,
    /// v0.3 (X5) 구조 최적화.
    optimize: bool,
    /// v0.3 (X5): which image streams carry a mask ([`image_masks`]).
    masks: ImageMasks,
}

/// v0.3 (X5): the encoded-stream hashes of the document's image XObjects that have an
/// `/SMask` (`soft`) or a `/Mask` (`other`), read with lopdf — PDFium does not expose either.
/// Empty when lopdf cannot read the file (then only PDFium's transparency test applies).
#[derive(Debug, Default)]
pub struct ImageMasks {
    pub soft: HashSet<u64>,
    pub other: HashSet<u64>,
}

pub fn image_masks(bytes: &[u8], password: Option<&str>) -> ImageMasks {
    let options = lopdf::LoadOptions::with_password(password.unwrap_or(""));
    let Ok(doc) = lopdf::Document::load_mem_with_options(bytes, options) else {
        tracing::warn!("compress: lopdf could not read the document; image masks unknown");
        return ImageMasks::default();
    };
    let mut masks = ImageMasks::default();
    for object in doc.objects.values() {
        let Ok(stream) = object.as_stream() else {
            continue;
        };
        let is_image = stream
            .dict
            .get(b"Subtype")
            .and_then(|s| s.as_name())
            .map(|n| n == b"Image")
            .unwrap_or(false);
        if !is_image {
            continue;
        }
        if stream.dict.has(b"Mask") {
            masks.other.insert(hash_of(&stream.content));
        } else if stream.dict.has(b"SMask") && !opaque_mask(&doc, stream) {
            masks.soft.insert(hash_of(&stream.content));
        }
    }
    masks
}

/// An `/SMask` whose every sample is 255 changes nothing (PDFium writes one next to any image
/// placed from an alpha bitmap): such an image is compressed like an unmasked one.
fn opaque_mask(doc: &lopdf::Document, image: &lopdf::Stream) -> bool {
    let Some(mask) = image
        .dict
        .get(b"SMask")
        .and_then(|m| m.as_reference())
        .ok()
        .and_then(|id| doc.get_object(id).ok())
        .and_then(|o| o.as_stream().ok())
    else {
        return false;
    };
    if mask
        .dict
        .get(b"BitsPerComponent")
        .and_then(|b| b.as_i64())
        .ok()
        != Some(8)
        || mask.dict.has(b"Decode")
    {
        return false;
    }
    let samples = if mask.dict.has(b"Filter") {
        match mask.decompressed_content() {
            Ok(s) => s,
            Err(_) => return false,
        }
    } else {
        mask.content.clone()
    };
    !samples.is_empty() && samples.iter().all(|&s| s == 255)
}

/// A finished estimate, waiting for `compress_apply` / `compress_discard`.
#[derive(Debug, Clone)]
pub struct Pending {
    pub token: u64,
    pub bytes: Arc<[u8]>,
    /// The generation the estimate was made from; `apply` refuses (`stale`) once the document
    /// has moved on.
    pub base_generation: DocGeneration,
    /// The report's `beforeBytes` / `imagesDownsampled`, for [`Pending::saves_nothing`].
    pub before_bytes: u64,
    pub images_downsampled: u32,
    /// v0.3 (X5): the structural pass made the file smaller.
    pub optimized: bool,
}

impl Pending {
    /// `true` when applying would replace the document for no gain: no image was downsampled
    /// (and the structural pass did not apply), or the file did not get smaller. The same rule
    /// as the dialog's `applyBlock` (`src/dialogs/compress.ts`), which disables 적용 for
    /// exactly these reports.
    pub fn saves_nothing(&self) -> bool {
        (self.images_downsampled == 0 && !self.optimized)
            || self.bytes.len() as u64 >= self.before_bytes
    }
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
    let masks = image_masks(&bytes, st.doc(doc_id)?.password.as_deref());
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
        nested: HashMap::new(),
        nested_plan: HashMap::new(),
        nested_done: HashSet::new(),
        replacements: Vec::new(),
        encrypted: doc.encrypted || doc.password.is_some(),
        images_total: 0,
        images_downsampled: 0,
        masked: HashMap::new(),
        masked_seen: HashMap::new(),
        masked_plan: HashMap::new(),
        optimize: options.optimize.unwrap_or(false),
        masks,
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
        _ => Err(EngineError::cancelled(
            "the compress estimate was superseded",
        )),
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
    let mut masked = Vec::new();
    let mut nested_found: Vec<NestedFound> = Vec::new();
    for (index, object) in handle.objects().iter().enumerate() {
        if let Some(form) = object.as_x_object_form_object() {
            let matrix = object_matrix(&object);
            walk_form(form, &mut vec![index], matrix, 0, &mut nested_found);
            continue;
        }
        let Some(image) = object.as_image_object() else {
            continue;
        };
        work.images_total += 1;
        if image.bits_per_pixel().unwrap_or(1) <= 1 {
            continue;
        }
        let Ok(data) = image.get_raw_image_data() else {
            continue;
        };
        let hash = hash_of(&data);
        // v0.3 (X5): PDFium's `FPDFPageObj_HasTransparency` does not see an image's own
        // `/SMask` / `/Mask`; the dictionaries were read with lopdf in `begin`. A soft-masked
        // image goes through the lopdf splice (mask downsampled with it); a `/Mask` (colour
        // key or stencil) image and one drawn with transparent graphics state are left alone —
        // `FPDFImageObj_SetBitmap` would drop the mask.
        if work.masks.other.contains(&hash) {
            continue;
        }
        let transparent = work.masks.soft.contains(&hash);
        if !transparent && object.has_transparency() {
            continue;
        }
        if transparent && work.encrypted {
            continue;
        }
        let (Ok(dpi_x), Ok(dpi_y)) = (image.horizontal_dpi(), image.vertical_dpi()) else {
            if !transparent {
                *work.seen.entry(hash).or_insert(0) += 1;
            }
            continue;
        };
        let above = dpi_x.is_finite() && dpi_y.is_finite() && dpi_x.min(dpi_y) > target;
        if transparent {
            // An `/SMask` image (the splice checks the dictionary): every occurrence counts.
            *work.masked_seen.entry(hash).or_insert(0) += 1;
            if above {
                plan_occurrence(&mut work.masked_plan, hash, dpi_x, dpi_y);
                masked.push(Candidate {
                    index,
                    hash,
                    dpi_x,
                    dpi_y,
                });
            }
            continue;
        }
        // Every eligible occurrence counts, candidate or not: a stream drawn twice is shared.
        *work.seen.entry(hash).or_insert(0) += 1;
        if !above {
            continue;
        }
        // v0.3 (X5): a shared stream is re-encoded once and spliced, like a nested image —
        // the plan collects every candidate occurrence, top-level and nested alike.
        plan_occurrence(&mut work.nested_plan, hash, dpi_x, dpi_y);
        found.push(Candidate {
            index,
            hash,
            dpi_x,
            dpi_y,
        });
    }
    if !masked.is_empty() {
        work.masked.insert(page, masked);
    }
    drop(handle);
    let mut nested = Vec::new();
    for f in nested_found {
        work.images_total += 1;
        let Some((hash, dpi_x, dpi_y)) = f.eligible else {
            continue;
        };
        *work.seen.entry(hash).or_insert(0) += 1;
        if !dpi_x.is_finite() || !dpi_y.is_finite() || dpi_x.min(dpi_y) <= target {
            continue;
        }
        plan_occurrence(&mut work.nested_plan, hash, dpi_x, dpi_y);
        nested.push(NestedCandidate { path: f.path, hash });
    }
    if !found.is_empty() {
        work.candidates.insert(page, found);
    }
    if !nested.is_empty() {
        work.nested.insert(page, nested);
    }
    Ok(())
}

/// One more candidate occurrence of `hash`: the lowest DPI sets the scale.
fn plan_occurrence(plans: &mut HashMap<u64, NestedPlan>, hash: u64, dpi_x: f32, dpi_y: f32) {
    let plan = plans.entry(hash).or_insert(NestedPlan {
        dpi_x,
        dpi_y,
        count: 0,
    });
    plan.dpi_x = plan.dpi_x.min(dpi_x);
    plan.dpi_y = plan.dpi_y.min(dpi_y);
    plan.count += 1;
}

fn hash_of(data: &Vec<u8>) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    data.hash(&mut hasher);
    hasher.finish()
}

/// `[a, b, c, d, e, f]` of an object (a form object's is its form matrix).
fn object_matrix(object: &PdfPageObject<'_>) -> raw::object::Matrix {
    object
        .matrix()
        .map(|m| [m.a(), m.b(), m.c(), m.d(), m.e(), m.f()])
        .unwrap_or([1.0, 0.0, 0.0, 1.0, 0.0, 0.0])
}

/// `a` then `b` (PDF row-vector convention).
fn concat(a: raw::object::Matrix, b: raw::object::Matrix) -> raw::object::Matrix {
    [
        a[0] * b[0] + a[1] * b[2],
        a[0] * b[1] + a[1] * b[3],
        a[2] * b[0] + a[3] * b[2],
        a[2] * b[1] + a[3] * b[3],
        a[4] * b[0] + a[5] * b[2] + b[4],
        a[4] * b[1] + a[5] * b[3] + b[5],
    ]
}

/// One image found inside a Form XObject: its path and, when it is eligible (no transparency,
/// more than 1 bpp, raw data readable), `(hash, dpi_x, dpi_y)` measured on the page.
struct NestedFound {
    path: Vec<usize>,
    eligible: Option<(u64, f32, f32)>,
}

/// How deep [`walk_form`] follows forms inside forms.
const MAX_FORM_DEPTH: usize = 8;

fn walk_form(
    form: &PdfPageXObjectFormObject<'_>,
    path: &mut Vec<usize>,
    to_page: raw::object::Matrix,
    depth: usize,
    out: &mut Vec<NestedFound>,
) {
    for i in 0..form.len() {
        let Ok(child) = form.get(i) else {
            continue;
        };
        path.push(i);
        let matrix = concat(object_matrix(&child), to_page);
        if let Some(inner) = child.as_x_object_form_object() {
            if depth + 1 < MAX_FORM_DEPTH {
                walk_form(inner, path, matrix, depth + 1, out);
            }
        } else if let Some(image) = child.as_image_object() {
            let eligible = (|| {
                if child.has_transparency() || image.bits_per_pixel().unwrap_or(1) <= 1 {
                    return None;
                }
                let data = image.get_raw_image_data().ok()?;
                let (w, h) = (image.width().ok()? as f32, image.height().ok()? as f32);
                // The image's unit square on the page: its x and y edges' lengths in points.
                let width_pt = matrix[0].hypot(matrix[1]);
                let height_pt = matrix[2].hypot(matrix[3]);
                if width_pt <= 0.0 || height_pt <= 0.0 {
                    return None;
                }
                Some((hash_of(&data), w * 72.0 / width_pt, h * 72.0 / height_pt))
            })();
            out.push(NestedFound {
                path: path.clone(),
                eligible,
            });
        }
        path.pop();
    }
}

/// Runs `f` on the object at `path` (page index, then indices inside each enclosing form).
/// Recursive because a form's children borrow the form object.
fn with_object_at_path<R>(
    page: &PdfPage<'_>,
    path: &[usize],
    f: impl FnOnce(&PdfPageObject<'_>) -> Option<R>,
) -> Option<R> {
    fn descend<R>(
        object: &PdfPageObject<'_>,
        rest: &[usize],
        f: impl FnOnce(&PdfPageObject<'_>) -> Option<R>,
    ) -> Option<R> {
        match rest.split_first() {
            None => f(object),
            Some((&i, tail)) => {
                let child = object.as_x_object_form_object()?.get(i).ok()?;
                descend(&child, tail, f)
            }
        }
    }
    let (&first, rest) = path.split_first()?;
    let top = page.objects().get(first).ok()?;
    descend(&top, rest, f)
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
    let candidates = work.candidates.remove(&page).unwrap_or_default();
    let masked = work.masked.remove(&page).unwrap_or_default();
    if candidates.is_empty() && masked.is_empty() && !work.nested.contains_key(&page) {
        return Ok(0);
    }
    let target = work.target_dpi as f32;
    let handle = open_page(&work.scratch, page)?;
    let mut replaced = 0u32;
    for c in candidates {
        let seen = work.seen.get(&c.hash).copied().unwrap_or(0);
        if seen == 1 {
            if downsample(bindings, &handle, c, target)? {
                replaced += 1;
            }
            continue;
        }
        // v0.3 (X5): a shared stream — re-encoded once, spliced into its one stream object by
        // `finish`, so every page that draws it gets the smaller image. Only when every
        // occurrence is above the target (the lowest sets the scale), never encrypted.
        if work.encrypted || work.nested_done.contains(&c.hash) {
            continue;
        }
        let Some(plan) = work.nested_plan.get(&c.hash).copied() else {
            continue;
        };
        if plan.count != seen {
            continue;
        }
        work.nested_done.insert(c.hash);
        if let Some(r) = with_object_at_path(&handle, &[c.index], |object| {
            reencode_nested(object, c.hash, plan, target)
        }) {
            work.replacements.push(r);
        }
    }
    // v0.3 (X5): images with transparency, re-encoded here; the splice keeps only those
    // whose dictionary has an `/SMask` and downsamples that mask by the same factor.
    for c in masked {
        if work.nested_done.contains(&c.hash) {
            continue;
        }
        let Some(plan) = work.masked_plan.get(&c.hash).copied() else {
            continue;
        };
        if plan.count != work.masked_seen.get(&c.hash).copied().unwrap_or(0) {
            continue;
        }
        work.nested_done.insert(c.hash);
        if let Some(mut r) = with_object_at_path(&handle, &[c.index], |object| {
            reencode_nested(object, c.hash, plan, target)
        }) {
            r.smask = true;
            work.replacements.push(r);
        }
    }
    // Stage 8: nested images are re-encoded here and spliced in by `finish`.
    for n in work.nested.remove(&page).unwrap_or_default() {
        if work.encrypted || work.nested_done.contains(&n.hash) {
            continue;
        }
        let Some(plan) = work.nested_plan.get(&n.hash).copied() else {
            continue;
        };
        // Every occurrence the scan saw must be a candidate: one below the target would be
        // degraded by the shared replacement.
        if plan.count != work.seen.get(&n.hash).copied().unwrap_or(0) {
            continue;
        }
        work.nested_done.insert(n.hash);
        if let Some(r) = with_object_at_path(&handle, &n.path, |object| {
            reencode_nested(object, n.hash, plan, target)
        }) {
            work.replacements.push(r);
        }
    }
    if replaced > 0 {
        let mut handle = handle;
        handle.regenerate_content().ctx("regenerate page content")?;
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

/// Decodes a nested image, downsamples it to `target` DPI (from its lowest on-page DPI) and
/// re-encodes it — JPEG for a `/DCTDecode` / `/JPXDecode` original, Flate-compressed 8-bit
/// samples otherwise. `None` when there is nothing to gain (the result is not smaller), so a
/// page of big scans never holds raw samples until `finish`.
fn reencode_nested(
    object: &PdfPageObject<'_>,
    hash: u64,
    plan: NestedPlan,
    target_dpi: f32,
) -> Option<NestedReplacement> {
    let image = object.as_image_object()?;
    let jpeg_source = image
        .filters()
        .iter()
        .any(|f| matches!(f.name(), "DCTDecode" | "JPXDecode"));
    let old_len = image.get_raw_image_data().ok()?.len();
    let decoded = image.get_raw_image().ok()?;
    let (w, h) = (decoded.width(), decoded.height());
    if w == 0 || h == 0 {
        return None;
    }
    let nw = ((w as f32 * target_dpi / plan.dpi_x).round() as u32).clamp(1, w);
    let nh = ((h as f32 * target_dpi / plan.dpi_y).round() as u32).clamp(1, h);
    if nw >= w && nh >= h {
        return None;
    }
    let gray = matches!(decoded, image::DynamicImage::ImageLuma8(_));
    let resized = decoded.resize_exact(nw, nh, image::imageops::FilterType::Triangle);
    let data = if jpeg_source {
        let mut jpeg = Vec::new();
        let mut encoder =
            image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, JPEG_QUALITY);
        let encoded = if gray {
            encoder.encode_image(&resized.to_luma8())
        } else {
            encoder.encode_image(&resized.to_rgb8())
        };
        encoded.ok()?;
        jpeg
    } else {
        let samples = if gray {
            resized.to_luma8().into_raw()
        } else {
            resized.to_rgb8().into_raw()
        };
        let mut stream = lopdf::Stream::new(lopdf::Dictionary::new(), samples);
        stream.compress().ok()?;
        if stream.dict.get(b"Filter").is_err() {
            return None; // Flate did not help at all
        }
        stream.content
    };
    if data.len() >= old_len {
        return None;
    }
    Some(NestedReplacement {
        hash,
        old_len,
        data,
        jpeg: jpeg_source,
        width: nw,
        height: nh,
        gray,
        occurrences: plan.count,
        smask: false,
    })
}

/// Stage 8: writes the re-encoded nested images into their own stream objects of the
/// serialised file (`lopdf`), found by the hash and length of the encoded stream. Returns the
/// new file and how many image occurrences changed; a replacement that would not be smaller
/// is skipped. Any `lopdf` failure leaves the bytes as they were.
fn splice_nested(bytes: &[u8], replacements: &[NestedReplacement]) -> (Vec<u8>, u32) {
    let Ok(mut doc) = lopdf::Document::load_mem(bytes) else {
        tracing::warn!("compress: lopdf could not read the scratch copy; nested images kept");
        return (bytes.to_vec(), 0);
    };
    let mut done: HashSet<u64> = HashSet::new();
    let mut changed = 0u32;
    // v0.3 (X5): the soft masks of spliced images, with the scale their image got.
    let mut masks: Vec<(lopdf::ObjectId, f64, f64)> = Vec::new();
    for object in doc.objects.values_mut() {
        let lopdf::Object::Stream(stream) = object else {
            continue;
        };
        let is_image = stream
            .dict
            .get(b"Subtype")
            .and_then(|s| s.as_name())
            .map(|n| n == b"Image")
            .unwrap_or(false);
        let smask = stream
            .dict
            .get(b"SMask")
            .and_then(|m| m.as_reference())
            .ok();
        if !is_image
            || stream.dict.has(b"Mask")
            || stream
                .dict
                .get(b"ImageMask")
                .and_then(|m| m.as_bool())
                .unwrap_or(false)
            // an inline or broken /SMask is left alone
            || (stream.dict.has(b"SMask") && smask.is_none())
        {
            continue;
        }
        let len = stream.content.len();
        let Some(r) = replacements
            .iter()
            .find(|r| r.old_len == len && !done.contains(&r.hash) && r.smask == smask.is_some())
            .filter(|r| hash_of(&stream.content) == r.hash)
        else {
            continue;
        };
        if let Some(id) = smask {
            let dim = |key: &[u8]| stream.dict.get(key).and_then(|v| v.as_i64()).unwrap_or(0);
            let (w, h) = (dim(b"Width"), dim(b"Height"));
            if w > 0 && h > 0 {
                masks.push((id, r.width as f64 / w as f64, r.height as f64 / h as f64));
            }
        }
        let mut next = stream.clone();
        next.dict.remove(b"DecodeParms");
        next.dict.remove(b"Decode");
        next.dict.set("Width", r.width as i64);
        next.dict.set("Height", r.height as i64);
        next.dict.set("BitsPerComponent", 8);
        next.dict.set(
            "ColorSpace",
            lopdf::Object::Name(if r.gray {
                b"DeviceGray".to_vec()
            } else {
                b"DeviceRGB".to_vec()
            }),
        );
        let filter: &[u8] = if r.jpeg { b"DCTDecode" } else { b"FlateDecode" };
        next.dict
            .set("Filter", lopdf::Object::Name(filter.to_vec()));
        next.set_content(r.data.clone());
        *stream = next;
        done.insert(r.hash);
        changed += r.occurrences;
    }
    if changed == 0 {
        return (bytes.to_vec(), 0);
    }
    // A mask that cannot be resampled keeps its resolution: `/SMask` need not match the image.
    let mut resized: HashSet<lopdf::ObjectId> = HashSet::new();
    for (id, sx, sy) in masks {
        if !resized.insert(id) {
            continue;
        }
        if let Ok(lopdf::Object::Stream(mask)) = doc.get_object_mut(id) {
            if let Some(next) = downsample_mask(mask, sx, sy) {
                *mask = next;
            }
        }
    }
    let mut out = Vec::with_capacity(bytes.len());
    match doc.save_to(&mut out) {
        Ok(()) => (out, changed),
        Err(e) => {
            tracing::warn!("compress: lopdf could not write the spliced copy: {e}");
            (bytes.to_vec(), 0)
        }
    }
}

/// v0.3 (X5): an 8-bit gray `/SMask` resampled by `(sx, sy)` and Flate-compressed; `None`
/// when it is not a plain 8-bit gray mask lopdf can decode, or nothing would shrink.
fn downsample_mask(mask: &lopdf::Stream, sx: f64, sy: f64) -> Option<lopdf::Stream> {
    let dim = |key: &[u8]| mask.dict.get(key).and_then(|v| v.as_i64()).ok();
    let (w, h) = (dim(b"Width")? as u32, dim(b"Height")? as u32);
    if dim(b"BitsPerComponent") != Some(8) || w == 0 || h == 0 {
        return None;
    }
    let (nw, nh) = (
        ((w as f64 * sx).round() as u32).clamp(1, w),
        ((h as f64 * sy).round() as u32).clamp(1, h),
    );
    if nw >= w && nh >= h {
        return None;
    }
    let samples = if mask.dict.has(b"Filter") {
        mask.decompressed_content().ok()?
    } else {
        mask.content.clone()
    };
    let gray = image::GrayImage::from_raw(w, h, samples.get(..(w * h) as usize)?.to_vec())?;
    let small = image::imageops::resize(&gray, nw, nh, image::imageops::FilterType::Triangle);
    let mut next = mask.clone();
    next.dict.remove(b"DecodeParms");
    next.dict.remove(b"Filter");
    next.dict.set("Width", nw as i64);
    next.dict.set("Height", nh as i64);
    next.set_content(small.into_raw());
    next.compress().ok()?;
    Some(next)
}

/// `(XObject names, font names)` a content stream uses (`"*"` = could not tell: keep all).
type UsedNames = (HashSet<Vec<u8>>, HashSet<Vec<u8>>);

/// v0.3 (X5) 구조 최적화 — lopdf over the finished bytes (unencrypted only):
///
/// 1. resource entries a page's content never uses (`/XObject` names without a `Do`, `/Font`
///    names without a `Tf`) are removed from the page's own `/Resources` — a resource
///    dictionary shared by several pages keeps the union of their uses, one shared with a form
///    XObject or inherited from the page tree is left alone, and a form XObject drawn without
///    resources of its own counts its content as the page's;
/// 2. empty content streams are dropped from `/Contents`;
/// 3. unreferenced objects are pruned;
/// 4. the file is written with object streams and a cross-reference stream.
///
/// `None` when lopdf cannot read or write the file.
pub fn optimize_structure(bytes: &[u8]) -> Option<Vec<u8>> {
    use lopdf::{Dictionary, Object, ObjectId};
    let mut doc = lopdf::Document::load_mem(bytes).ok()?;
    if doc.is_encrypted() {
        return None;
    }
    // Names used by a content stream, following forms that inherit their resources.
    fn used_names(
        doc: &lopdf::Document,
        content: &[u8],
        xobjects: Option<&Dictionary>,
        depth: u8,
        out: &mut UsedNames,
    ) {
        let Ok(ops) = lopdf::content::Content::decode(content) else {
            // Unparseable: keep every name.
            out.0.insert(b"*".to_vec());
            out.1.insert(b"*".to_vec());
            return;
        };
        for op in ops.operations {
            let name = op.operands.first().and_then(|o| o.as_name().ok());
            match (op.operator.as_str(), name) {
                ("Do", Some(n)) => {
                    out.0.insert(n.to_vec());
                    // A form without its own /Resources draws with the page's.
                    let form = xobjects
                        .and_then(|x| x.get(n).ok())
                        .and_then(|o| doc.dereference(o).ok())
                        .and_then(|(_, o)| o.as_stream().ok());
                    if let Some(form) = form {
                        let is_form = form
                            .dict
                            .get(b"Subtype")
                            .and_then(|s| s.as_name())
                            .map(|s| s == b"Form")
                            .unwrap_or(false);
                        if is_form && !form.dict.has(b"Resources") && depth < 8 {
                            if let Ok(inner) = form.decompressed_content() {
                                used_names(doc, &inner, xobjects, depth + 1, out);
                            }
                        }
                    }
                }
                ("Tf", Some(n)) => {
                    out.1.insert(n.to_vec());
                }
                _ => {}
            }
        }
    }

    // Which resource dictionaries (by id) are also a form's resources: left alone.
    let mut form_resources: HashSet<ObjectId> = HashSet::new();
    for object in doc.objects.values() {
        if let Ok(stream) = object.as_stream() {
            if let Ok(Object::Reference(id)) = stream.dict.get(b"Resources") {
                form_resources.insert(*id);
            }
        }
    }

    enum ResourcesAt {
        Inline(ObjectId),
        Shared(ObjectId),
    }
    let mut usage: HashMap<Vec<u8>, UsedNames> = HashMap::new();
    let mut targets: Vec<(ResourcesAt, Vec<u8>)> = Vec::new();
    let pages: Vec<ObjectId> = doc.get_pages().values().copied().collect();
    for page_id in pages {
        let Ok(page) = doc.get_dictionary(page_id) else {
            continue;
        };
        let at = match page.get(b"Resources") {
            Ok(Object::Dictionary(_)) => ResourcesAt::Inline(page_id),
            Ok(Object::Reference(id)) if !form_resources.contains(id) => ResourcesAt::Shared(*id),
            // Inherited from the page tree, or shared with a form: left alone.
            _ => continue,
        };
        let key = match &at {
            ResourcesAt::Inline(id) => format!("page {} {}", id.0, id.1).into_bytes(),
            ResourcesAt::Shared(id) => format!("res {} {}", id.0, id.1).into_bytes(),
        };
        let resources = match &at {
            ResourcesAt::Inline(_) => page.get(b"Resources").and_then(Object::as_dict).ok(),
            ResourcesAt::Shared(id) => doc.get_dictionary(*id).ok(),
        };
        let xobjects = resources
            .and_then(|r| r.get(b"XObject").ok())
            .and_then(|o| doc.dereference(o).ok())
            .and_then(|(_, o)| o.as_dict().ok());
        // Every content stream decoded, or the page keeps all of its resources.
        let mut content = Vec::new();
        let mut readable = true;
        for id in doc.get_page_contents(page_id) {
            let data = doc
                .get_object(id)
                .and_then(Object::as_stream)
                .and_then(|s| {
                    if s.dict.has(b"Filter") {
                        s.decompressed_content()
                    } else {
                        Ok(s.content.clone())
                    }
                });
            match data {
                Ok(data) => {
                    content.extend_from_slice(&data);
                    content.push(b'\n');
                }
                Err(_) => readable = false,
            }
        }
        let entry = usage.entry(key.clone()).or_default();
        if readable {
            used_names(&doc, &content, xobjects, 0, entry);
        } else {
            entry.0.insert(b"*".to_vec());
            entry.1.insert(b"*".to_vec());
        }
        if !targets.iter().any(|(_, k)| *k == key) {
            targets.push((at, key));
        }
    }

    for (at, key) in targets {
        let (xo_used, font_used) = &usage[&key];
        let resources: Option<&mut Dictionary> = match at {
            ResourcesAt::Inline(page_id) => doc
                .get_dictionary_mut(page_id)
                .ok()
                .and_then(|p| p.get_mut(b"Resources").ok())
                .and_then(|r| r.as_dict_mut().ok()),
            ResourcesAt::Shared(id) => doc.get_dictionary_mut(id).ok(),
        };
        let Some(resources) = resources else { continue };
        for (kind, used) in [
            (b"XObject".as_slice(), xo_used),
            (b"Font".as_slice(), font_used),
        ] {
            if used.contains(b"*".as_slice()) {
                continue;
            }
            // Only a direct sub-dictionary is edited; a referenced one may be shared.
            if let Ok(Object::Dictionary(sub)) = resources.get_mut(kind) {
                let unused: Vec<Vec<u8>> = sub
                    .iter()
                    .map(|(k, _)| k.clone())
                    .filter(|k| !used.contains(k))
                    .collect();
                for k in unused {
                    sub.remove(&k);
                }
            }
        }
    }

    // Empty content streams: out of the page's /Contents.
    let pages: Vec<ObjectId> = doc.get_pages().values().copied().collect();
    for page_id in pages {
        let empty = |doc: &lopdf::Document, o: &Object| -> bool {
            o.as_reference()
                .ok()
                .and_then(|id| doc.get_object(id).ok())
                .and_then(|o| o.as_stream().ok())
                .map(|s| s.content.is_empty())
                .unwrap_or(false)
        };
        let contents = doc
            .get_dictionary(page_id)
            .ok()
            .and_then(|p| p.get(b"Contents").ok())
            .cloned();
        if let Some(Object::Array(items)) = contents {
            let kept: Vec<Object> = items.iter().filter(|o| !empty(&doc, o)).cloned().collect();
            if kept.len() != items.len() {
                if let Ok(page) = doc.get_dictionary_mut(page_id) {
                    page.set("Contents", Object::Array(kept));
                }
            }
        }
    }

    doc.prune_objects();
    let mut out = Vec::with_capacity(bytes.len());
    doc.save_modern(&mut out).ok()?;
    Some(out)
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
        mut images_downsampled,
        replacements,
        optimize,
        encrypted,
        ..
    } = work;
    drop(scratch);
    let bytes = if replacements.is_empty() {
        bytes
    } else {
        let (spliced, changed) = splice_nested(&bytes, &replacements);
        images_downsampled += changed;
        spliced
    };
    // v0.3 (X5) 구조 최적화: kept only when it is smaller and PDFium still reads it.
    let mut optimized = false;
    let bytes = match (optimize && !encrypted)
        .then(|| optimize_structure(&bytes))
        .flatten()
    {
        Some(smaller)
            if smaller.len() < bytes.len()
                && save::verify_bytes(st, &smaller, page_count, password.clone()).is_ok() =>
        {
            optimized = true;
            smaller
        }
        _ => bytes,
    };
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
        before_bytes,
        images_downsampled,
        optimized,
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
///
/// A result that [saves nothing](Pending::saves_nothing) is spent without touching the
/// document: no undo step, no dirty flag, no new generation — the current `DocInfo` comes back
/// as it is. That check comes first, so such a token is never `stale` (there is nothing to
/// apply to a changed document either).
pub fn apply(st: &mut EngineState<'_>, doc_id: &str, token: u64) -> Result<DocInfo, EngineError> {
    let pending = take_pending(st, doc_id, token)?;
    if pending.saves_nothing() {
        return Ok(st.doc(doc_id)?.info());
    }
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
    let submit =
        |label: &'static str| Submit::new(Lane::Background, label).cancel(token.cancel.clone());
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
