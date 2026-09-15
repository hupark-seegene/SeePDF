//! AcroForm fields — `IPC_CONTRACT.md` §7.2. Owner: **Stage 1 (a)**.
//!
//! Filling goes through the form-fill environment on the raw handles
//! (`FORM_SetFocusedAnnot` → `FORM_SelectAllText` → `FORM_ReplaceSelection` →
//! `FORM_ForceToKillFocus`, with the verified click + per-character `FORM_OnChar` fallback);
//! `PdfFormTextField::set_value` writes `/V` only and is invisible in every viewer.
//! The raw form handle is `OpenDoc::form_handle()`. See `engine::form` for the details.

use crate::engine::form;
use crate::engine::registry::{self, MutateOpts};
use crate::engine::{EngineHandle, Lane};
use crate::ipc::types::{
    ChangeReason, DocInfo, FieldValue, FormField, PageIndex, SetFormFieldResult,
};
use crate::ipc::EngineError;
use tauri::State;

#[tauri::command]
pub async fn list_form_fields(
    engine: State<'_, EngineHandle>,
    doc_id: String,
    page: Option<PageIndex>,
) -> Result<Vec<FormField>, EngineError> {
    engine
        .call(Lane::Interactive, "list_form_fields", move |st| {
            let doc = st.doc_mut(&doc_id)?;
            form::list(doc, page)
        })
        .await
}

#[tauri::command]
pub async fn set_form_field_value(
    engine: State<'_, EngineHandle>,
    doc_id: String,
    page: PageIndex,
    index: u32,
    value: FieldValue,
) -> Result<SetFormFieldResult, EngineError> {
    engine
        .call(Lane::Edit, "set_form_field_value", move |st| {
            let outcome = registry::mutate(
                st,
                &doc_id,
                MutateOpts::new("undo.formFill", ChangeReason::Edit).page(page),
                |doc| form::set_value(doc, page, index, &value),
            )?;
            tracing::debug!(?outcome.method, "form field written");
            Ok(SetFormFieldResult {
                field: outcome.field,
                previous: outcome.previous,
                doc_generation: st.doc(&doc_id)?.generation,
            })
        })
        .await
}

/// P1. Clears every writable field; radio groups keep their selection (see `engine::form`).
#[tauri::command]
pub async fn reset_form(
    engine: State<'_, EngineHandle>,
    doc_id: String,
) -> Result<DocInfo, EngineError> {
    engine
        .call(Lane::Edit, "reset_form", move |st| {
            let pages: Vec<PageIndex> = (0..st.doc(&doc_id)?.page_count()).collect();
            registry::mutate(
                st,
                &doc_id,
                MutateOpts::new("undo.formReset", ChangeReason::Edit).pages(pages),
                |doc| form::reset(doc),
            )?;
            Ok(st.doc(&doc_id)?.info())
        })
        .await
}
