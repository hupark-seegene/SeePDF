//! Save and Save As — `IPC_CONTRACT.md` §7.6. Owner: **Stage 1 (b)**.
//!
//! The sequence is fixed (`ARCHITECTURE.md` §8): render every page in `OpenDoc::touched` at
//! `set_target_width(32)` so pdfium writes the `/AP` streams
//! (`engine::render::tiles::generate_appearances`) → `FORM_ForceToKillFocus` → drop the page
//! LRU → `FPDF_SaveAsCopy(flags = 0)` → temp file in the same directory + fsync → verify by
//! reopening → backup → `rename` → `registry::replace`.

use crate::engine::EngineHandle;
use crate::ipc::types::{JobEvent, SaveResult};
use crate::ipc::EngineError;
use tauri::ipc::Channel;
use tauri::State;

#[tauri::command]
pub async fn save_document(
    _engine: State<'_, EngineHandle>,
    _doc_id: String,
    _on_progress: Channel<JobEvent>,
) -> Result<SaveResult, EngineError> {
    Err(EngineError::unsupported("save_document"))
}

#[tauri::command]
pub async fn save_document_as(
    _engine: State<'_, EngineHandle>,
    _doc_id: String,
    _path: String,
    _on_progress: Channel<JobEvent>,
) -> Result<SaveResult, EngineError> {
    Err(EngineError::unsupported("save_document_as"))
}
