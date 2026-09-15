//! Redaction, passwords and metadata — `IPC_CONTRACT.md` §7.5. Owner: **Stage 1 (a)** for
//! redaction, **(b)** / P1 for the file rewrites.
//!
//! `apply_redactions` re-extracts the page text after `regenerate_content()` and fails with
//! `verifyFailed` (restoring the snapshot) if any marked string survives — a fake redaction
//! is never shipped. `remove_password` is `FPDF_SaveAsCopy` with `FPDF_REMOVE_SECURITY = 4`
//! (**not** 3, the deprecated value); `set_password`, `remove_metadata` and `set_metadata`
//! need `lopdf`, which is a P1 dependency.
//!
//! Stage 2: the three file-rewrite bodies `STAGE1B_NOTES.md` §5.1 asks for now live here —
//! `remove_password` is implemented, and the two metadata writers go through
//! `engine::save::write_metadata`, which owns the single "why not yet" message so the P1
//! `lopdf` patch has exactly one call site.

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

/// 암호 제거 — `FPDF_SaveAsCopy(FPDF_REMOVE_SECURITY)` into `out_path`.
///
/// Verified by the pages spike §5: the copy opens with no password. Pending annotation
/// appearances are generated first, exactly as `save_document` does, so a document that was
/// edited before the password was removed does not lose its `/AP`s.
#[tauri::command]
pub async fn remove_password(
    engine: State<'_, EngineHandle>,
    doc_id: String,
    out_path: String,
) -> Result<BytesWritten, EngineError> {
    engine
        .call(Lane::Edit, "remove_password", move |st| {
            crate::engine::render::tiles::generate_appearances(st, &doc_id)?;
            let doc = st.doc(&doc_id)?;
            let bytes = crate::engine::raw::save::save_as_copy(
                doc.bindings(),
                doc.pdf(),
                crate::engine::raw::save::SaveFlags::RemoveSecurity,
            )?;
            let written =
                crate::engine::pages::write_atomic(std::path::Path::new(&out_path), &bytes)?;
            Ok(BytesWritten { bytes: written })
        })
        .await
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

/// P1; needs `lopdf`. `engine::save::write_metadata` carries the reason.
#[tauri::command]
pub async fn remove_metadata(
    engine: State<'_, EngineHandle>,
    doc_id: String,
) -> Result<DocInfo, EngineError> {
    engine
        .call(Lane::Edit, "remove_metadata", move |st| {
            // Existence check first, so an unknown docId is `notFound` and not `unsupported`.
            let _ = st.doc(&doc_id)?;
            crate::engine::save::write_metadata(&DocMeta::default())?;
            Ok(st.doc(&doc_id)?.info())
        })
        .await
}

/// P1; needs `lopdf`. `engine::save::write_metadata` carries the reason.
#[tauri::command]
pub async fn set_metadata(
    engine: State<'_, EngineHandle>,
    doc_id: String,
    meta: DocMeta,
) -> Result<DocInfo, EngineError> {
    engine
        .call(Lane::Edit, "set_metadata", move |st| {
            let _ = st.doc(&doc_id)?;
            crate::engine::save::write_metadata(&meta)?;
            Ok(st.doc(&doc_id)?.info())
        })
        .await
}
