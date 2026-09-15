//! Redaction, passwords and metadata — `IPC_CONTRACT.md` §7.5. Owner: **Stage 1 (a)** for
//! redaction, **(b)** / P1 for the file rewrites.
//!
//! `apply_redactions` re-extracts the page text after `regenerate_content()` and fails with
//! `verifyFailed` (restoring the snapshot) if any marked string survives — a fake redaction
//! is never shipped. `remove_password` is `FPDF_SaveAsCopy` with `FPDF_REMOVE_SECURITY = 4`
//! (**not** 3, the deprecated value); `set_password`, `remove_metadata` and `set_metadata`
//! need `lopdf`, which is a P1 dependency.

use crate::engine::redact;
use crate::engine::{EngineHandle, Lane};
use crate::ipc::types::{
    BytesWritten, DocInfo, DocMeta, PageIndex, Permissions, RedactOptions, RedactPreview,
    RedactResult, Rect,
};
use crate::ipc::EngineError;
use tauri::State;

#[tauri::command]
pub async fn redact_preview(
    engine: State<'_, EngineHandle>,
    doc_id: String,
    page: PageIndex,
    rects: Vec<Rect>,
) -> Result<RedactPreview, EngineError> {
    engine
        .call(Lane::Interactive, "redact_preview", move |st| {
            let doc = st.doc_mut(&doc_id)?;
            redact::preview(doc, page, &rects)
        })
        .await
}

#[tauri::command]
pub async fn apply_redactions(
    engine: State<'_, EngineHandle>,
    doc_id: String,
    page: PageIndex,
    rects: Vec<Rect>,
    options: RedactOptions,
) -> Result<RedactResult, EngineError> {
    engine
        .call(Lane::Edit, "apply_redactions", move |st| {
            // `apply_verified` owns the whole contract: pre-flight refusal, one
            // `registry::mutate` (= one undo step), post-condition, and the byte-snapshot
            // rollback if either verification point fires.
            let removed = redact::apply_verified(st, &doc_id, page, &rects, &options)?;
            Ok(RedactResult {
                removed_objects: removed,
                // `apply` only returns `Ok` when the post-condition held.
                verified: true,
                doc_generation: st.doc(&doc_id)?.generation,
            })
        })
        .await
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
