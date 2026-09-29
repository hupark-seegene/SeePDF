//! Redaction, passwords and metadata — `IPC_CONTRACT.md` §7.5. Owner: **Stage 1 (a)** for
//! redaction, **(b)** / P1 for the file rewrites.
//!
//! `apply_redactions` re-extracts the page text after `regenerate_content()` and fails with
//! `verifyFailed` (restoring the snapshot) if any marked string survives — a fake redaction
//! is never shipped. `remove_password` is `FPDF_SaveAsCopy` with `FPDF_REMOVE_SECURITY = 4`
//! (**not** 3, the deprecated value).
//!
//! P1-1 / P1-3: `set_password`, `remove_metadata` and `set_metadata` are thin wrappers over
//! `engine::security`, which rewrites PDFium's serialised bytes with `lopdf` (AES-256 V5/R6
//! encryption; `/Info` + XMP) and verifies the result by reopening it with PDFium.

use crate::engine::redact;
use crate::engine::{EngineHandle, Lane};
use crate::ipc::types::{
    AttachmentInfo, BytesWritten, DocInfo, DocMeta, PageIndex, PermissionsRequest, Rect,
    RedactBatchMark, RedactBatchResult, RedactOptions, RedactPreview, RedactResult,
    SanitizeOptions, SanitizeResult,
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
            // v0.3 (pkg1): the static plan plus a dry run of the apply on a throw-away copy.
            redact::preview_checked(st, &doc_id, page, &rects)
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

/// Stage 8: every marked page in ONE undo step (`undo.redact`); a `verifyFailed` on any page
/// rolls all of them back. See `engine::redact::apply_batch`.
#[tauri::command]
pub async fn apply_redactions_batch(
    engine: State<'_, EngineHandle>,
    doc_id: String,
    marks: Vec<RedactBatchMark>,
    options: RedactOptions,
) -> Result<RedactBatchResult, EngineError> {
    engine
        .call(Lane::Edit, "apply_redactions_batch", move |st| {
            redact::apply_batch(st, &doc_id, &marks, &options)
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
            // v0.3 S5: an unlocked copy of a restricted document needs the owner password.
            crate::engine::security::ensure_security_change(st.doc(&doc_id)?)?;
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

/// 암호 설정 — an AES-256 (V5 / R6) protected copy at `out_path`; see
/// `engine::security::set_password`. The open document is not modified.
#[tauri::command]
pub async fn set_password(
    engine: State<'_, EngineHandle>,
    doc_id: String,
    out_path: String,
    user_password: Option<String>,
    owner_password: String,
    permissions: Option<PermissionsRequest>,
) -> Result<BytesWritten, EngineError> {
    engine
        .call(Lane::Edit, "set_password", move |st| {
            crate::engine::security::set_password(
                st,
                &doc_id,
                &out_path,
                user_password.as_deref(),
                &owner_password,
                permissions.unwrap_or_default(),
            )
        })
        .await
}

/// 메타데이터 제거 — deletes `/Info` and the XMP packet; one undo step.
#[tauri::command]
pub async fn remove_metadata(
    engine: State<'_, EngineHandle>,
    doc_id: String,
) -> Result<DocInfo, EngineError> {
    engine
        .call(Lane::Edit, "remove_metadata", move |st| {
            crate::engine::security::remove_metadata(st, &doc_id)
        })
        .await
}

/// 문서 속성 편집 — writes the given `/Info` fields; one undo step.
#[tauri::command]
pub async fn set_metadata(
    engine: State<'_, EngineHandle>,
    doc_id: String,
    meta: DocMeta,
) -> Result<DocInfo, EngineError> {
    engine
        .call(Lane::Edit, "set_metadata", move |st| {
            crate::engine::security::set_metadata(st, &doc_id, &meta)
        })
        .await
}

// ---------------------------------------------------------------------------------------
// v0.3 pkg3-security-save-integrity
// ---------------------------------------------------------------------------------------

/// S5 제한됨 › 권한 암호로 잠금 해제: reopens the document with its permissions (owner)
/// password, keeping the docId and the undo history; see `engine::security::unlock`.
#[tauri::command]
pub async fn unlock_document(
    engine: State<'_, EngineHandle>,
    doc_id: String,
    password: String,
) -> Result<DocInfo, EngineError> {
    engine
        .call(Lane::Edit, "unlock_document", move |st| {
            crate::engine::security::unlock(st, &doc_id, &password)
        })
        .await
}

/// S3 문서 정리 — one undo step; see `engine::sanitize`. Missing options are all `true`.
#[tauri::command]
pub async fn sanitize_document(
    engine: State<'_, EngineHandle>,
    doc_id: String,
    options: Option<SanitizeOptions>,
) -> Result<SanitizeResult, EngineError> {
    engine
        .call(Lane::Edit, "sanitize_document", move |st| {
            crate::engine::sanitize::sanitize(st, &doc_id, &options.unwrap_or_default())
        })
        .await
}

/// S4 첨부 파일 panel.
#[tauri::command]
pub async fn list_attachments(
    engine: State<'_, EngineHandle>,
    doc_id: String,
) -> Result<Vec<AttachmentInfo>, EngineError> {
    engine
        .call(Lane::Interactive, "list_attachments", move |st| {
            Ok(crate::engine::attachments::list(st.doc(&doc_id)?))
        })
        .await
}

#[tauri::command]
pub async fn save_attachment(
    engine: State<'_, EngineHandle>,
    doc_id: String,
    index: u32,
    path: String,
) -> Result<BytesWritten, EngineError> {
    engine
        .call(Lane::Edit, "save_attachment", move |st| {
            crate::engine::attachments::save(st.doc(&doc_id)?, index, &path)
        })
        .await
}

#[tauri::command]
pub async fn add_attachment(
    engine: State<'_, EngineHandle>,
    doc_id: String,
    path: String,
    name: Option<String>,
) -> Result<Vec<AttachmentInfo>, EngineError> {
    engine
        .call(Lane::Edit, "add_attachment", move |st| {
            crate::engine::attachments::add(st, &doc_id, &path, name.as_deref())
        })
        .await
}

#[tauri::command]
pub async fn delete_attachment(
    engine: State<'_, EngineHandle>,
    doc_id: String,
    index: u32,
) -> Result<Vec<AttachmentInfo>, EngineError> {
    engine
        .call(Lane::Edit, "delete_attachment", move |st| {
            crate::engine::attachments::delete(st, &doc_id, index)
        })
        .await
}
