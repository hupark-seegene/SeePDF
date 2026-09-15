//! OCR text layer — `IPC_CONTRACT.md` §7.9. Owner: **Stage 1 (b)** for the engine layer,
//! **(f)** for the worker pipeline.
//!
//! The page image the tesseract.js workers recognise comes from the `/ocr` protocol route,
//! never from a command. `ocr_apply` is **one** `registry::mutate` for the whole batch = one
//! undo step, with per-page work on `Lane::Background` so tiles interleave.

use crate::engine::EngineHandle;
use crate::ipc::types::{
    DocInfo, OcrCapabilities, OcrPage, OcrPageStatus, PageIndex,
};
use crate::ipc::EngineError;
use tauri::ipc::Channel;
use tauri::State;

#[tauri::command]
pub async fn ocr_capabilities(
    _engine: State<'_, EngineHandle>,
) -> Result<OcrCapabilities, EngineError> {
    Err(EngineError::unsupported("ocr_capabilities"))
}

#[tauri::command]
pub async fn ocr_page_status(
    _engine: State<'_, EngineHandle>,
    _doc_id: String,
    _pages: Vec<PageIndex>,
) -> Result<Vec<OcrPageStatus>, EngineError> {
    Err(EngineError::unsupported("ocr_page_status"))
}

#[tauri::command]
pub async fn ocr_apply(
    _engine: State<'_, EngineHandle>,
    _doc_id: String,
    _pages: Vec<OcrPage>,
    _replace_existing: bool,
    _on_progress: Channel<crate::ipc::types::JobEvent>,
) -> Result<DocInfo, EngineError> {
    Err(EngineError::unsupported("ocr_apply"))
}

/// P1, macOS only (Vision).
#[tauri::command]
pub async fn ocr_recognize_native(
    _engine: State<'_, EngineHandle>,
    _doc_id: String,
    _page: PageIndex,
    _dpi: u32,
    _languages: Vec<String>,
) -> Result<OcrPage, EngineError> {
    Err(EngineError::unsupported("ocr_recognize_native"))
}
