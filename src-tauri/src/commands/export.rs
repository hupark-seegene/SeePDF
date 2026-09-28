//! Export and print — `IPC_CONTRACT.md` §7.7. Owner: **Stage 1 (b)**.
//!
//! Flattening uses raw `FPDFPage_Flatten(page, FLAT_NORMALDISPLAY)` + `FPDFPage_GenerateContent`
//! plus a page reload on a scratch copy — never `PdfPage::flatten()`, which is `FLAT_PRINT` and
//! silently deletes annotations without the Print flag.
//!
//! `export_images` and `export_flattened` are jobs: one `Lane::Background` command per page,
//! all sharing one `JobToken`, so a 500-page export never blocks a tile and Cancel is observed
//! within one page.

use crate::engine::export::{self, job::JobReporter};
use crate::engine::types::CmdStatus;
use crate::engine::{EngineHandle, Lane, Submit};
use crate::ipc::types::{
    ExportEstimate, ExportImagesArgs, ExportTextResult, ImageFormat, JobEvent, JobId, PageIndex,
    PrintPrepareResult,
};
use crate::ipc::EngineError;
use std::path::PathBuf;
use tauri::ipc::Channel;
use tauri::State;

#[tauri::command]
pub async fn export_images(
    engine: State<'_, EngineHandle>,
    args: ExportImagesArgs,
    on_progress: Channel<JobEvent>,
) -> Result<JobId, EngineError> {
    let doc_id = args.doc_id.clone();
    let pages = {
        let doc_id = doc_id.clone();
        let requested = args.pages.clone();
        engine
            .call(Lane::Edit, "export_pages", move |st| {
                export::check_pages(st, &doc_id, &requested)
            })
            .await?
    };

    let token = engine.jobs.create();
    let reporter = JobReporter::start(
        on_progress,
        engine.jobs.clone(),
        token.id,
        pages.len() as u32,
    );
    if pages.is_empty() {
        reporter.finish_now();
        return Ok(token.id);
    }

    let out_dir = PathBuf::from(&args.out_dir);
    let transparent = args.transparent_background.unwrap_or(false);
    for (position, page) in pages.into_iter().enumerate() {
        let submit = Submit::new(Lane::Background, "export_image")
            .priority(position as u32)
            .page(page)
            .cancel(token.cancel.clone());
        let doc_id = doc_id.clone();
        let out_dir = out_dir.clone();
        let base_name = args.base_name.clone();
        let format = args.format;
        let dpi = args.dpi;
        let quality = args.quality;
        let reporter_for_job = reporter.clone();
        let dispatched = engine.dispatch(submit, move |st, status| {
            if reporter_for_job.is_finished() {
                return;
            }
            if status != CmdStatus::Run {
                reporter_for_job.cancel();
                return;
            }
            match export::export_page_image(
                st,
                &doc_id,
                page,
                format,
                dpi,
                quality,
                &out_dir,
                &base_name,
                transparent,
            ) {
                Ok(image) => {
                    reporter_for_job.step(Some(page), Some(image.path.display().to_string()));
                    reporter_for_job.finish_if_complete();
                }
                Err(e) => reporter_for_job.fail(e),
            }
        });
        if let Err(e) = dispatched {
            reporter.fail(e);
            break;
        }
    }
    Ok(token.id)
}

#[tauri::command]
pub async fn export_text(
    engine: State<'_, EngineHandle>,
    doc_id: String,
    pages: Vec<PageIndex>,
    out_path: String,
) -> Result<ExportTextResult, EngineError> {
    engine
        .call(Lane::Background, "export_text", move |st| {
            export::export_text(st, &doc_id, &pages, &out_path)
        })
        .await
}

/// One file, but still a job: flattening a 500-page document is seconds of engine time and the
/// UI needs a progress bar and a Cancel button in front of it.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn export_flattened(
    engine: State<'_, EngineHandle>,
    doc_id: String,
    out_path: String,
    annotations: bool,
    forms: bool,
    pages: Option<Vec<PageIndex>>,
    on_progress: Channel<JobEvent>,
) -> Result<JobId, EngineError> {
    let token = engine.jobs.create();
    let reporter = JobReporter::start(on_progress, engine.jobs.clone(), token.id, 1);
    let submit = Submit::new(Lane::Background, "export_flattened").cancel(token.cancel.clone());
    let reporter_for_job = reporter.clone();
    let dispatched = engine.dispatch(submit, move |st, status| {
        if status != CmdStatus::Run {
            reporter_for_job.cancel();
            return;
        }
        match export::export_flattened(st, &doc_id, &out_path, annotations, forms, pages.as_deref())
        {
            Ok(_) => {
                reporter_for_job.step(None, Some(out_path.clone()));
                reporter_for_job.finish_if_complete();
            }
            Err(e) => reporter_for_job.fail(e),
        }
    });
    if let Err(e) = dispatched {
        reporter.fail(e);
    }
    Ok(token.id)
}

#[tauri::command]
pub async fn estimate_export(
    engine: State<'_, EngineHandle>,
    doc_id: String,
    pages: Vec<PageIndex>,
    format: ImageFormat,
    dpi: u32,
) -> Result<ExportEstimate, EngineError> {
    engine
        .call(Lane::Background, "estimate_export", move |st| {
            export::estimate(st, &doc_id, &pages, format, dpi)
        })
        .await
}

#[tauri::command]
pub async fn print_prepare(
    engine: State<'_, EngineHandle>,
    doc_id: String,
    pages: Option<Vec<PageIndex>>,
) -> Result<PrintPrepareResult, EngineError> {
    let temp_path = engine
        .call(Lane::Edit, "print_prepare", move |st| {
            export::print_prepare(st, &doc_id, pages.as_deref())
        })
        .await?;
    Ok(PrintPrepareResult {
        temp_path: temp_path.display().to_string(),
    })
}
