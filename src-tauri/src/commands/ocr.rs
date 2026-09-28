//! OCR text layer — `IPC_CONTRACT.md` §7.9. Owner: **Stage 1 (b)** for the engine layer,
//! **(f)** for the worker pipeline.
//!
//! The page image the tesseract.js workers recognise comes from the `/ocr` protocol route,
//! never from a command. `ocr_apply` is **one** `registry::mutate` for the whole batch = one
//! undo step, so cancelling a 14-page run and undoing it are both a single step.
//!
//! `ocr_recognize_native` (P1-11, macOS Vision) renders that same image on the engine thread and
//! recognises it on a blocking thread — Vision never runs on the engine thread, PDFium never
//! runs off it.

use crate::engine::ocr;
use crate::engine::{EngineHandle, Lane};
use crate::ipc::types::{
    DocInfo, JobEvent, OcrApplyPage, OcrCapabilities, OcrPage, OcrPageStatus, PageIndex,
};
use crate::ipc::{EngineError, ErrorCode};
use tauri::ipc::Channel;
use tauri::State;

#[tauri::command]
pub async fn ocr_capabilities(
    _engine: State<'_, EngineHandle>,
) -> Result<OcrCapabilities, EngineError> {
    // Nothing here touches pdfium, so it does not go to the engine thread.
    Ok(ocr::capabilities())
}

#[tauri::command]
pub async fn ocr_page_status(
    engine: State<'_, EngineHandle>,
    doc_id: String,
    pages: Vec<PageIndex>,
) -> Result<Vec<OcrPageStatus>, EngineError> {
    engine
        .call(Lane::Background, "ocr_page_status", move |st| {
            let doc = st.doc_mut(&doc_id)?;
            ocr::page_status(doc, &pages)
        })
        .await
}

#[tauri::command]
pub async fn ocr_apply(
    engine: State<'_, EngineHandle>,
    doc_id: String,
    pages: Vec<OcrApplyPage>,
    replace_existing: bool,
    on_progress: Channel<JobEvent>,
) -> Result<DocInfo, EngineError> {
    // `OcrPage[]` and the Stage 8 `{ page, ocr }[]` form are the same batch.
    let pages = pages
        .into_iter()
        .map(OcrApplyPage::into_page)
        .collect::<Result<Vec<OcrPage>, EngineError>>()?;
    let token = engine.jobs.create();
    let job_id = token.id;
    let total = pages.len() as u32;
    let started = std::time::Instant::now();
    let _ = on_progress.send(JobEvent::Started { job_id, total });

    let channel = on_progress.clone();
    let result = engine
        .call(Lane::Edit, "ocr_apply", move |st| {
            // The whole batch is one `mutate`, so progress is reported from inside it rather
            // than by one command per page: splitting it would be several undo steps.
            let mut report = |done: usize, page: PageIndex| {
                let _ = channel.send(JobEvent::Progress {
                    job_id,
                    done: done as u32,
                    total,
                    page: Some(page),
                    note: None,
                });
            };
            // `cancel_job` is honoured between pages; the batch then rolls back whole.
            let cancelled = || token.is_cancelled();
            ocr::apply_cancellable(
                st,
                &doc_id,
                &pages,
                replace_existing,
                &mut report,
                &cancelled,
            )
        })
        .await;
    engine.jobs.finish(job_id);
    match &result {
        Err(error) if error.code == ErrorCode::Cancelled => {
            // The batch is one `mutate`, so a cancel rolled every page of it back.
            let _ = on_progress.send(JobEvent::Cancelled { job_id, done: 0 });
        }
        Ok(_) => {
            let _ = on_progress.send(JobEvent::Done {
                job_id,
                elapsed_ms: started.elapsed().as_secs_f64() * 1000.0,
                outputs: None,
                report: None,
                compare: None,
            });
        }
        Err(error) => {
            let _ = on_progress.send(JobEvent::Error {
                job_id,
                error: error.clone(),
            });
        }
    }
    result
}

/// P1-11, macOS only (Vision, accurate level, `ko-KR` + `en-US`, language correction). Returns
/// the same `OcrPage` the tesseract path builds — image pixels, origin top-left — for
/// `ocr_apply`. `unsupported` where `ocr_capabilities` does not list `vision`.
#[tauri::command]
pub async fn ocr_recognize_native(
    engine: State<'_, EngineHandle>,
    doc_id: String,
    page: PageIndex,
    dpi: u32,
    languages: Vec<String>,
) -> Result<OcrPage, EngineError> {
    if !ocr::vision_available() {
        return Err(EngineError::unsupported("ocr_recognize_native"));
    }
    // The `/ocr` image, on the engine thread (the only thread that may touch PDFium)…
    let image = engine
        .call(Lane::Background, "ocr_recognize_native", move |st| {
            ocr::render_page_gray(st, &doc_id, page, dpi)
        })
        .await?;
    // …and Vision on a blocking thread, so tiles keep flowing while it reads (0.1–3 s a page).
    tauri::async_runtime::spawn_blocking(move || ocr::recognize_gray(&image, &languages))
        .await
        .map_err(|e| EngineError::new(ErrorCode::Pdfium, format!("Vision thread: {e}")))?
}
