//! Redaction, passwords and metadata — `IPC_CONTRACT.md` §7.5. Owner: **Stage 1 (a)** for
//! redaction, **(b)** / P1 for the file rewrites.
//!
//! `apply_redactions` re-extracts the page text after `regenerate_content()` and fails with
//! `verifyFailed` (restoring the snapshot) if any marked string survives — a fake redaction
//! is never shipped. `remove_password` is `FPDF_SaveAsCopy` with `FPDF_REMOVE_SECURITY = 4`
//! (**not** 3, the deprecated value); `set_password`, `remove_metadata` and `set_metadata`
//! need `lopdf`, which is a P1 dependency.

use crate::engine::EngineHandle;
use crate::ipc::types::{
    BytesWritten, DocInfo, DocMeta, PageIndex, Permissions, RedactOptions, RedactPreview,
    RedactResult, Rect,
};
use crate::ipc::EngineError;
use tauri::State;

#[tauri::command]
pub async fn redact_preview(
    _engine: State<'_, EngineHandle>,
    _doc_id: String,
    _page: PageIndex,
    _rects: Vec<Rect>,
) -> Result<RedactPreview, EngineError> {
    Err(EngineError::unsupported("redact_preview"))
}

#[tauri::command]
pub async fn apply_redactions(
    _engine: State<'_, EngineHandle>,
    _doc_id: String,
    _page: PageIndex,
    _rects: Vec<Rect>,
    _options: RedactOptions,
) -> Result<RedactResult, EngineError> {
    Err(EngineError::unsupported("apply_redactions"))
}

/// P1.
#[tauri::command]
pub async fn remove_password(
    _engine: State<'_, EngineHandle>,
    _doc_id: String,
    _out_path: String,
) -> Result<BytesWritten, EngineError> {
    Err(EngineError::unsupported("remove_password"))
}

/// P1; needs `lopdf`.
#[tauri::command]
pub async fn set_password(
    _engine: State<'_, EngineHandle>,
    _doc_id: String,
    _out_path: String,
    _user_password: Option<String>,
    _owner_password: String,
    _permissions: Permissions,
) -> Result<BytesWritten, EngineError> {
    Err(EngineError::unsupported("set_password"))
}

/// P1; needs `lopdf`.
#[tauri::command]
pub async fn remove_metadata(
    _engine: State<'_, EngineHandle>,
    _doc_id: String,
) -> Result<DocInfo, EngineError> {
    Err(EngineError::unsupported("remove_metadata"))
}

/// P1; needs `lopdf`.
#[tauri::command]
pub async fn set_metadata(
    _engine: State<'_, EngineHandle>,
    _doc_id: String,
    _meta: DocMeta,
) -> Result<DocInfo, EngineError> {
    Err(EngineError::unsupported("set_metadata"))
}
