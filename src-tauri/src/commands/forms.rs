//! AcroForm fields — `IPC_CONTRACT.md` §7.2. Owner: **Stage 1 (a)**.
//!
//! Filling goes through the form-fill environment on the raw handles
//! (`FORM_SetFocusedAnnot` → `FORM_SelectAllText` → `FORM_ReplaceSelection` →
//! `FORM_ForceToKillFocus`, with the verified click + per-character `FORM_OnChar` fallback);
//! `PdfFormTextField::set_value` writes `/V` only and is invisible in every viewer.
//! The raw form handle is `OpenDoc::form_handle()`.

use crate::engine::EngineHandle;
use crate::ipc::types::{DocInfo, FieldValue, FormField, PageIndex, SetFormFieldResult};
use crate::ipc::EngineError;
use tauri::State;

#[tauri::command]
pub async fn list_form_fields(
    _engine: State<'_, EngineHandle>,
    _doc_id: String,
    _page: Option<PageIndex>,
) -> Result<Vec<FormField>, EngineError> {
    Err(EngineError::unsupported("list_form_fields"))
}

#[tauri::command]
pub async fn set_form_field_value(
    _engine: State<'_, EngineHandle>,
    _doc_id: String,
    _page: PageIndex,
    _index: u32,
    _value: FieldValue,
) -> Result<SetFormFieldResult, EngineError> {
    Err(EngineError::unsupported("set_form_field_value"))
}

/// P1.
#[tauri::command]
pub async fn reset_form(
    _engine: State<'_, EngineHandle>,
    _doc_id: String,
) -> Result<DocInfo, EngineError> {
    Err(EngineError::unsupported("reset_form"))
}
