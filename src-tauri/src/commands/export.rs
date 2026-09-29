//! Export and print — `IPC_CONTRACT.md` §7.7. Owner: **Stage 1 (b)**.
//!
//! Flattening uses raw `FPDFPage_Flatten(page, FLAT_NORMALDISPLAY)` + `FPDFPage_GenerateContent`
//! plus a page reload on a scratch copy — never `PdfPage::flatten()`, which is `FLAT_PRINT` and
//! silently deletes annotations without the Print flag.
//!
//! `export_images` and `export_flattened` are jobs: one `Lane::Background` command per page,
//! all sharing one `JobToken`, so a 500-page export never blocks a tile and Cancel is observed
//! within one page.

use crate::engine::export::{self, job, job::JobReporter, pagejob};
use crate::engine::types::CmdStatus;
use crate::engine::{EngineHandle, Lane, Submit};
use crate::ipc::types::{
    AnnotationSummaryResult, ExportEstimate, ExportImagesArgs, ExportTextResult, ImageFormat,
    JobEvent, JobId, Locale, PageIndex, PrintPrepareResult, SummaryFormat,
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

/// v0.3 (X3): `preserveLayout` — 레이아웃 유지 (monospace columns from the x positions).
#[tauri::command]
pub async fn export_text(
    engine: State<'_, EngineHandle>,
    doc_id: String,
    pages: Vec<PageIndex>,
    out_path: String,
    preserve_layout: Option<bool>,
) -> Result<ExportTextResult, EngineError> {
    let preserve = preserve_layout.unwrap_or(false);
    engine
        .call(Lane::Background, "export_text", move |st| {
            export::export_text_with(st, &doc_id, &pages, &out_path, preserve)
        })
        .await
}

/// P2 주석 목록 내보내기 (§7.7a): one row per annotation of `pages` (default all) as TXT, CSV
/// (UTF-8 with BOM) or Markdown, labelled in `locale` (default: the app language).
#[tauri::command]
pub async fn export_annotation_summary(
    app: tauri::AppHandle,
    engine: State<'_, EngineHandle>,
    doc_id: String,
    path: String,
    format: SummaryFormat,
    pages: Option<Vec<PageIndex>>,
    locale: Option<Locale>,
) -> Result<AnnotationSummaryResult, EngineError> {
    let locale = locale.unwrap_or_else(|| crate::app::store::get_settings(&app).locale);
    engine
        .call(Lane::Background, "export_annotation_summary", move |st| {
            export::summary::export_annotation_summary(
                st,
                &doc_id,
                &path,
                format,
                pages.as_deref(),
                locale,
            )
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

/// v0.3 (X7): `annots` (인쇄 ▸ 주석, default `all`); X8: the temp file is deleted
/// [`export::PRINT_TEMP_MAX_AGE`] after it was written (and by the startup / exit sweeps).
#[tauri::command]
pub async fn print_prepare(
    engine: State<'_, EngineHandle>,
    doc_id: String,
    pages: Option<Vec<PageIndex>>,
    annots: Option<export::PrintAnnots>,
) -> Result<PrintPrepareResult, EngineError> {
    let temp_path = engine
        .call(Lane::Edit, "print_prepare", move |st| {
            export::print_prepare_with(st, &doc_id, pages.as_deref(), annots.unwrap_or_default())
        })
        .await?;
    export::schedule_print_temp_removal(temp_path.clone(), export::PRINT_TEMP_MAX_AGE);
    Ok(PrintPrepareResult {
        temp_path: temp_path.display().to_string(),
    })
}

// ---------------------------------------------------------------------------------------
// v0.3 pkg8 (X2): 모아찍기 / 소책자
// ---------------------------------------------------------------------------------------

/// `make_nup` — a new document with `perSheet` pages per sheet (or a saddle-stitch booklet),
/// written to `outPath` (내보내기 ▸ N-up PDF) or, without one, to a print temp file that the
/// print path opens like any document (deleted after 10 minutes and by the startup / exit
/// sweeps).
#[tauri::command]
pub async fn make_nup(
    engine: State<'_, EngineHandle>,
    doc_id: String,
    pages: Option<Vec<PageIndex>>,
    options: export::nup::NupOptions,
    out_path: Option<String>,
) -> Result<export::nup::NupResult, EngineError> {
    let temp = out_path.is_none();
    let result = engine
        .call(Lane::Background, "make_nup", move |st| {
            let (bytes, page_count) =
                export::nup::make_nup_bytes(st, &doc_id, pages.as_deref(), &options)?;
            let path = match out_path {
                Some(p) => {
                    let p = PathBuf::from(p);
                    crate::engine::pages::write_atomic(&p, &bytes)?;
                    p
                }
                None => export::write_print_temp(st, &doc_id, "-nup", &bytes)?,
            };
            Ok(export::nup::NupResult {
                path: path.display().to_string(),
                page_count,
            })
        })
        .await?;
    if temp {
        export::schedule_print_temp_removal(
            PathBuf::from(&result.path),
            export::PRINT_TEMP_MAX_AGE,
        );
    }
    Ok(result)
}

// ---------------------------------------------------------------------------------------
// v0.3 pkg8 (X3 / X6): page-by-page export jobs with state (engine/export/pagejob.rs)
// ---------------------------------------------------------------------------------------

/// Validates `pages` (empty = every page) on the engine thread.
async fn resolve_pages(
    engine: &EngineHandle,
    doc_id: &str,
    pages: Vec<PageIndex>,
) -> Result<Vec<PageIndex>, EngineError> {
    let doc_id = doc_id.to_string();
    engine
        .call(Lane::Edit, "export_pages", move |st| {
            export::check_pages(st, &doc_id, &pages)
        })
        .await
}

/// 이미지 추출: every embedded image of `pages` as `<baseName>-p<page>-<n>.png` in `outDir`.
#[tauri::command]
pub async fn export_embedded_images(
    engine: State<'_, EngineHandle>,
    doc_id: String,
    pages: Vec<PageIndex>,
    out_dir: String,
    base_name: String,
    on_progress: Channel<JobEvent>,
) -> Result<JobId, EngineError> {
    let pages = resolve_pages(&engine, &doc_id, pages).await?;
    Ok(pagejob::embedded_images(
        &engine,
        job::channel_sink(on_progress),
        doc_id,
        pages,
        PathBuf::from(out_dir),
        base_name,
    ))
}

/// 하나의 이미지로 이어 붙이기: `pages` one under the other, one PNG / JPEG. The DPI is
/// lowered when the image would exceed `STITCH_MAX_PX` / `STITCH_MAX_SIDE`; the answer says so.
#[tauri::command]
pub async fn export_stitched_image(
    engine: State<'_, EngineHandle>,
    doc_id: String,
    pages: Vec<PageIndex>,
    dpi: u32,
    format: ImageFormat,
    out_path: String,
    on_progress: Channel<JobEvent>,
) -> Result<pagejob::StitchStart, EngineError> {
    let pages = resolve_pages(&engine, &doc_id, pages).await?;
    let plan = {
        let doc_id = doc_id.clone();
        let pages = pages.clone();
        engine
            .call(Lane::Edit, "stitch_plan", move |st| {
                export::stitch_plan(st, &doc_id, &pages, dpi)
            })
            .await?
    };
    let job_id = pagejob::stitched_image(
        &engine,
        job::channel_sink(on_progress),
        doc_id,
        pages,
        plan,
        format,
        PathBuf::from(out_path),
    );
    Ok(pagejob::StitchStart {
        job_id,
        dpi: plan.dpi,
        width: plan.width,
        height: plan.height,
        lowered: plan.lowered,
    })
}

/// 여러 페이지 TIFF: `pages` as the frames of one TIFF (RGB, Deflate) at `dpi`.
#[tauri::command]
pub async fn export_tiff(
    engine: State<'_, EngineHandle>,
    doc_id: String,
    pages: Vec<PageIndex>,
    dpi: u32,
    out_path: String,
    on_progress: Channel<JobEvent>,
) -> Result<JobId, EngineError> {
    let pages = resolve_pages(&engine, &doc_id, pages).await?;
    if !(36..=1200).contains(&dpi) {
        return Err(EngineError::invalid(format!(
            "{dpi} DPI is outside 36..=1200"
        )));
    }
    pagejob::tiff(
        &engine,
        job::channel_sink(on_progress),
        doc_id,
        pages,
        dpi,
        PathBuf::from(out_path),
    )
}

/// X6 텍스트 흐름: DOCX / HWPX / HTML / Markdown, lossy (no tables, no layout).
#[tauri::command]
pub async fn export_text_flow(
    engine: State<'_, EngineHandle>,
    doc_id: String,
    pages: Vec<PageIndex>,
    format: export::textflow::FlowFormat,
    out_path: String,
    on_progress: Channel<JobEvent>,
) -> Result<JobId, EngineError> {
    let pages = resolve_pages(&engine, &doc_id, pages).await?;
    let title = {
        let doc_id = doc_id.clone();
        engine
            .call(Lane::Edit, "export_text_flow_title", move |st| {
                Ok(st.doc(&doc_id)?.name_stem())
            })
            .await?
    };
    Ok(pagejob::text_flow(
        &engine,
        job::channel_sink(on_progress),
        doc_id,
        pages,
        format,
        PathBuf::from(out_path),
        title,
    ))
}
