//! A page-by-page export job with state carried from page to page — v0.3 pkg8 (X3, X6).
//!
//! `export_images` needs nothing between pages; a stitched image, a multi-page TIFF or a text
//! flow document do: a canvas, an open encoder, the paragraphs so far. [`run`] is the
//! `export_images` shape (one `Lane::Background` command per page, one `JobToken`, Cancel
//! observed within a page, exactly one terminal event through [`JobReporter`]) plus that state:
//! each page's `step` gets `&mut S`, the last page's command hands `S` to `finish`, and a
//! cancel or an error hands it to `abandon` (delete a partial file) instead.
//!
//! Pages run in the order given: they share the lane and their priority is their position.

use crate::engine::export::job::{JobReporter, JobSink};
use crate::engine::types::{CmdStatus, EngineState};
use crate::engine::{EngineHandle, Lane, Submit};
use crate::ipc::types::{JobId, PageIndex};
use crate::ipc::EngineError;
use parking_lot::Mutex;
use std::sync::Arc;

/// Starts the job and returns its id. `pages` must be validated (empty finishes at once).
#[allow(clippy::too_many_arguments)]
pub fn run<S, F, G, A>(
    engine: &EngineHandle,
    sink: JobSink,
    label: &'static str,
    pages: Vec<PageIndex>,
    state: S,
    step: F,
    finish: G,
    abandon: A,
) -> JobId
where
    S: Send + 'static,
    F: Fn(&mut EngineState<'_>, &mut S, PageIndex) -> Result<Vec<String>, EngineError>
        + Send
        + Sync
        + 'static,
    G: FnOnce(&mut EngineState<'_>, S) -> Result<Vec<String>, EngineError> + Send + 'static,
    A: FnOnce(S) + Send + 'static,
{
    let token = engine.jobs.create();
    let total = pages.len();
    let reporter = JobReporter::start_with(sink, engine.jobs.clone(), token.id, total as u32);
    if total == 0 {
        abandon(state);
        reporter.finish_now();
        return token.id;
    }
    let state = Arc::new(Mutex::new(Some(state)));
    let step = Arc::new(step);
    let finish = Arc::new(Mutex::new(Some(finish)));
    let abandon = Arc::new(Mutex::new(Some(abandon)));
    let give_up = {
        let state = state.clone();
        let abandon = abandon.clone();
        move || {
            let s = state.lock().take();
            let a = abandon.lock().take();
            if let (Some(s), Some(a)) = (s, a) {
                a(s);
            }
        }
    };
    let give_up = Arc::new(give_up);

    for (position, page) in pages.into_iter().enumerate() {
        let submit = Submit::new(Lane::Background, label)
            .priority(position as u32)
            .page(page)
            .cancel(token.cancel.clone());
        let reporter_for_job = reporter.clone();
        let state = state.clone();
        let step = step.clone();
        let finish = finish.clone();
        let give_up_for_job = give_up.clone();
        let dispatched = engine.dispatch(submit, move |st, status| {
            if reporter_for_job.is_finished() {
                return;
            }
            if status != CmdStatus::Run {
                give_up_for_job();
                reporter_for_job.cancel();
                return;
            }
            let mut guard = state.lock();
            let Some(s) = guard.as_mut() else {
                return;
            };
            let outputs = match step(st, s, page) {
                Ok(outputs) => outputs,
                Err(e) => {
                    drop(guard);
                    give_up_for_job();
                    reporter_for_job.fail(e.with_page(page));
                    return;
                }
            };
            let mut outputs = outputs;
            if position + 1 == total {
                let s = guard.take().expect("state is present until the last page");
                drop(guard);
                let f = finish.lock().take().expect("finish runs once");
                match f(st, s) {
                    Ok(more) => outputs.extend(more),
                    Err(e) => {
                        reporter_for_job.fail(e);
                        return;
                    }
                }
            } else {
                drop(guard);
            }
            reporter_for_job.add_outputs(outputs);
            reporter_for_job.step(Some(page), None);
            reporter_for_job.finish_if_complete();
        });
        if let Err(e) = dispatched {
            give_up();
            reporter.fail(e);
            break;
        }
    }
    token.id
}

// ---------------------------------------------------------------------------------------
// The X3 jobs
// ---------------------------------------------------------------------------------------

use crate::engine::export;
use crate::ipc::types::ImageFormat;
use std::path::PathBuf;

/// `export_embedded_images`: every image object of `pages` as `<base>-p<page>-<n>.png`.
pub fn embedded_images(
    engine: &EngineHandle,
    sink: JobSink,
    doc_id: String,
    pages: Vec<PageIndex>,
    out_dir: PathBuf,
    base_name: String,
) -> JobId {
    run(
        engine,
        sink,
        "export_embedded_images",
        pages,
        (),
        move |st, _, page| {
            Ok(
                export::export_embedded_page(st, &doc_id, page, &out_dir, &base_name)?
                    .into_iter()
                    .map(|p| p.display().to_string())
                    .collect(),
            )
        },
        |_, _| Ok(Vec::new()),
        |_| {},
    )
}

/// `export_stitched_image`: `pages` one under the other at `plan.dpi`, one PNG / JPEG.
pub fn stitched_image(
    engine: &EngineHandle,
    sink: JobSink,
    doc_id: String,
    pages: Vec<PageIndex>,
    plan: export::StitchPlan,
    format: ImageFormat,
    out_path: PathBuf,
) -> JobId {
    let canvas = image::RgbImage::from_pixel(plan.width, plan.height, image::Rgb([255, 255, 255]));
    run(
        engine,
        sink,
        "export_stitched_image",
        pages,
        (canvas, 0u32),
        move |st, (canvas, y), page| {
            let rgb = export::render_rgb(st, &doc_id, page, plan.dpi)?;
            *y = export::stitch_page(canvas, &rgb, *y);
            Ok(Vec::new())
        },
        move |_, (canvas, _)| {
            export::write_image_file(canvas, format, &out_path)?;
            Ok(vec![out_path.display().to_string()])
        },
        |_| {},
    )
}

/// `export_tiff`: `pages` as the frames of one multi-page TIFF at `dpi`.
pub fn tiff(
    engine: &EngineHandle,
    sink: JobSink,
    doc_id: String,
    pages: Vec<PageIndex>,
    dpi: u32,
    out_path: PathBuf,
) -> Result<JobId, EngineError> {
    let writer = export::TiffPages::create(&out_path)?;
    Ok(run(
        engine,
        sink,
        "export_tiff",
        pages,
        writer,
        move |st, writer, page| {
            let rgb = export::render_rgb(st, &doc_id, page, dpi)?;
            writer.add(&rgb, dpi)?;
            Ok(Vec::new())
        },
        |_, writer| Ok(vec![writer.finish()?.display().to_string()]),
        export::TiffPages::abandon,
    ))
}

/// `export_stitched_image`'s answer: the job, and the geometry actually used.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StitchStart {
    pub job_id: JobId,
    pub dpi: u32,
    pub width: u32,
    pub height: u32,
    /// The requested DPI was lowered to stay under the pixel cap.
    pub lowered: bool,
}

/// `export_text_flow` (X6): `pages` regrouped into paragraphs, headings and images, written
/// as DOCX / HWPX / HTML / Markdown after the last page.
pub fn text_flow(
    engine: &EngineHandle,
    sink: JobSink,
    doc_id: String,
    pages: Vec<PageIndex>,
    format: export::textflow::FlowFormat,
    out_path: PathBuf,
    title: String,
) -> JobId {
    let flow = export::textflow::FlowDoc {
        blocks: Vec::new(),
        title,
    };
    run(
        engine,
        sink,
        "export_text_flow",
        pages,
        flow,
        move |st, flow, page| {
            // The command checked it before the job; checked again per page like every
            // other page job (the document could have been swapped by then).
            crate::engine::security::ensure_doc_permitted(
                st,
                &doc_id,
                crate::engine::security::Perm::ExtractText,
            )?;
            export::textflow::collect_page(st.doc_mut(&doc_id)?, page, flow)?;
            Ok(Vec::new())
        },
        move |_, flow| {
            Ok(flow
                .write(format, &out_path)?
                .into_iter()
                .map(|p| p.display().to_string())
                .collect())
        },
        |_| {},
    )
}
