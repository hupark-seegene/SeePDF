//! Annotations — `IPC_CONTRACT.md` §7.1. Owner: **Stage 1 (a)**.
//!
//! Signatures are final; the bodies are the Stage-0 stubs. To implement one, replace the
//! `Err(EngineError::unsupported(...))` with a `engine.call(Lane::Edit, "…", move |st|
//! registry::mutate(st, &doc_id, MutateOpts::new("undo.annotCreate", ChangeReason::Edit)
//! .page(page), |doc| { … })).await` — `lib.rs` does not change.

use crate::engine::EngineHandle;
use crate::ipc::types::{
    Annot, AnnotList, AnnotPatch, AnnotResult, AnnotScanEvent, AnnotSpec, JobId, PageIndex,
    ViewNonce,
};
use crate::ipc::EngineError;
use tauri::ipc::Channel;
use tauri::State;

#[tauri::command]
pub async fn list_annotations(
    _engine: State<'_, EngineHandle>,
    _doc_id: String,
    _page: PageIndex,
) -> Result<AnnotList, EngineError> {
    Err(EngineError::unsupported("list_annotations"))
}

#[tauri::command]
pub async fn scan_annotations(
    _engine: State<'_, EngineHandle>,
    _doc_id: String,
    _on_event: Channel<AnnotScanEvent>,
) -> Result<JobId, EngineError> {
    Err(EngineError::unsupported("scan_annotations"))
}

#[tauri::command]
pub async fn create_annotation(
    _engine: State<'_, EngineHandle>,
    _doc_id: String,
    _page: PageIndex,
    _spec: AnnotSpec,
    _id: Option<String>,
) -> Result<AnnotResult, EngineError> {
    Err(EngineError::unsupported("create_annotation"))
}

#[tauri::command]
pub async fn update_annotation(
    _engine: State<'_, EngineHandle>,
    _doc_id: String,
    _page: PageIndex,
    _id: String,
    _patch: AnnotPatch,
) -> Result<AnnotResult, EngineError> {
    Err(EngineError::unsupported("update_annotation"))
}

#[tauri::command]
pub async fn delete_annotations(
    _engine: State<'_, EngineHandle>,
    _doc_id: String,
    _page: PageIndex,
    _ids: Vec<String>,
) -> Result<AnnotResult, EngineError> {
    Err(EngineError::unsupported("delete_annotations"))
}

/// P1. Transient: bumps `viewNonce`, **not** `docGeneration`, and never dirties the document.
#[tauri::command]
pub async fn set_annotations_hidden(
    _engine: State<'_, EngineHandle>,
    _doc_id: String,
    _page: PageIndex,
    _ids: Vec<String>,
    _hidden: bool,
) -> Result<ViewNonce, EngineError> {
    Err(EngineError::unsupported("set_annotations_hidden"))
}

/// Keeps `Annot` referenced from this module so Stage 1 (a) starts from a compiling file.
#[allow(dead_code)]
fn _type_anchor(a: Annot) -> Annot {
    a
}
