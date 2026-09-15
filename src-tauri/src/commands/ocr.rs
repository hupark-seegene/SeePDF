//! OCR text layer — `IPC_CONTRACT.md` §7.9. Owner: **Stage 1 (b)** for the engine layer,
//! **(f)** for the worker pipeline.
//!
//! The page image the tesseract.js workers recognise comes from the `/ocr` protocol route,
//! never from a command. `ocr_apply` is **one** `registry::mutate` for the whole batch = one
//! undo step, so cancelling a 14-page run and undoing it are both a single step.

use crate::engine::ocr;
use crate::engine::{EngineHandle, Lane};
use crate::ipc::types::{
    DocInfo, JobEvent, OcrCapabilities, OcrPage, OcrPageStatus, PageIndex,
};
use crate::ipc::EngineError;
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
    pages: Vec<OcrPage>,
    replace_existing: bool,
    on_progress: Channel<JobEvent>,
) -> Result<DocInfo, EngineError> {
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
            ocr::apply(st, &doc_id, &pages, replace_existing, &mut report)
        })
        .await;
    engine.jobs.finish(job_id);
    match &result {
        Ok(_) => {
            let _ = on_progress.send(JobEvent::Done {
                job_id,
                elapsed_ms: started.elapsed().as_secs_f64() * 1000.0,
                outputs: None,
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

/// P1, macOS only (Vision). `ocr_capabilities` does not advertise `vision` until this has a
/// body, so the frontend never routes to it by accident.
#[tauri::command]
pub async fn ocr_recognize_native(
    _engine: State<'_, EngineHandle>,
    _doc_id: String,
    _page: PageIndex,
    _dpi: u32,
    _languages: Vec<String>,
) -> Result<crate::ipc::types::OcrPage, EngineError> {
    Err(EngineError::unsupported("ocr_recognize_native"))
}
