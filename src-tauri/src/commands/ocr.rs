//! OCR text layer — `IPC_CONTRACT.md` §7.9. Owner: **Stage 1 (b)** for the engine layer,
//! **(f)** for the worker pipeline.
//!
//! The page image the tesseract.js workers recognise comes from the `/ocr` protocol route,
//! never from a command. `ocr_apply` is **one** `registry::mutate` for the whole batch = one
//! undo step, so cancelling a 14-page run and undoing it are both a single step.
//!
//! `ocr_recognize_native` (P1-11 macOS Vision, v0.3 O3 Windows.Media.Ocr) renders that same
//! image on the engine thread and recognises it on a blocking thread — the recogniser never
//! runs on the engine thread, PDFium never runs off it. `ocr_detect_orientation` (v0.3 O2) does
//! the same at 100 DPI, four times.

use crate::engine::ocr;
use crate::engine::ocr::orientation::{self, OrientationResult};
use crate::engine::{EngineHandle, Lane};
use crate::ipc::types::{
    DocInfo, JobEvent, OcrApplyPage, OcrCapabilities, OcrPage, OcrPageStatus, PageIndex, Rotation,
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
    // `OcrPage[]` and the Stage 8 `{ page, ocr, setRotation? }[]` form are the same batch.
    let (pages, rotations): (Vec<OcrPage>, Vec<Option<Rotation>>) = pages
        .into_iter()
        .map(OcrApplyPage::into_page_and_rotation)
        .collect::<Result<Vec<_>, EngineError>>()?
        .into_iter()
        .unzip();
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
            ocr::apply_rotated_cancellable(
                st,
                &doc_id,
                &pages,
                &rotations,
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

/// P1-11 macOS Vision (accurate level, `ko-KR` + `en-US`, language correction), v0.3 O3
/// Windows.Media.Ocr. Returns the same `OcrPage` the tesseract path builds — image pixels,
/// origin top-left — for `ocr_apply`. `unsupported` where `ocr_capabilities` lists neither
/// `vision` nor `windows`.
///
/// `rotate` (v0.3 O2, default 0) turns the image that many degrees clockwise before it is
/// read; the `OcrPage` then says `rotation = (page /Rotate + rotate) % 360`, which is what
/// `ocr_apply`'s `setRotation` expects.
#[tauri::command]
pub async fn ocr_recognize_native(
    engine: State<'_, EngineHandle>,
    doc_id: String,
    page: PageIndex,
    dpi: u32,
    languages: Vec<String>,
    rotate: Option<Rotation>,
) -> Result<OcrPage, EngineError> {
    if !ocr::native_available() {
        return Err(EngineError::unsupported("ocr_recognize_native"));
    }
    let rotate = check_turn(rotate.unwrap_or(0))?;
    // The `/ocr` image, on the engine thread (the only thread that may touch PDFium)…
    let image = engine
        .call(Lane::Background, "ocr_recognize_native", move |st| {
            ocr::render_page_gray(st, &doc_id, page, dpi)
        })
        .await?;
    // …and the recogniser on a blocking thread, so tiles keep flowing while it reads
    // (0.1–3 s a page).
    tauri::async_runtime::spawn_blocking(move || {
        let image = orientation::rotate_gray(&image, rotate);
        ocr::recognize_gray(&image, &languages)
    })
    .await
    .map_err(|e| EngineError::new(ErrorCode::Pdfium, format!("OCR thread: {e}")))?
}

/// v0.3 O2 페이지 회전 자동 감지 with Apple Vision: the page at 100 DPI, every line voting with
/// its reading direction; `rotation` is the clockwise turn that makes it upright, 0 when none
/// wins clearly. `unsupported` where `ocr_capabilities` has no `vision` (the frontend then
/// detects with tesseract).
#[tauri::command]
pub async fn ocr_detect_orientation(
    engine: State<'_, EngineHandle>,
    doc_id: String,
    page: PageIndex,
    languages: Vec<String>,
) -> Result<OrientationResult, EngineError> {
    if !ocr::vision_available() {
        return Err(EngineError::unsupported("ocr_detect_orientation"));
    }
    let image = engine
        .call(Lane::Background, "ocr_detect_orientation", move |st| {
            ocr::render_page_gray(st, &doc_id, page, orientation::DETECT_DPI)
        })
        .await?;
    tauri::async_runtime::spawn_blocking(move || ocr::detect_orientation(&image, &languages))
        .await
        .map_err(|e| EngineError::new(ErrorCode::Pdfium, format!("OCR thread: {e}")))?
}

fn check_turn(rotate: Rotation) -> Result<Rotation, EngineError> {
    if matches!(rotate, 0 | 90 | 180 | 270) {
        Ok(rotate)
    } else {
        Err(EngineError::invalid(format!(
            "rotate {rotate} is not 0, 90, 180 or 270"
        )))
    }
}
