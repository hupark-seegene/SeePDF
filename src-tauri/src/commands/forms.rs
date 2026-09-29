//! AcroForm fields — `IPC_CONTRACT.md` §7.2. Owner: **Stage 1 (a)**; v0.3 authoring, data
//! exchange and flattening (pkg2-pages-structure-forms).
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
    ChangeReason, DocInfo, FieldType, FieldValue, FormDataFormat, FormDataResult, FormEditResult,
    FormField, FormFieldPatch, FormFieldSpec, PageIndex, SetFormFieldResult,
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

/// v0.3: `{ checked: false }` on a radio button that is on switches its whole group off (a
/// lopdf rewrite — `form::clear_radio`); everything else goes through the form-fill
/// environment as before.
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
            let clear_radio = matches!(value, FieldValue::Checked { checked: false })
                && form::field_at(st, &doc_id, page, index)
                    .is_ok_and(|f| f.field_type == FieldType::Radio && f.checked.unwrap_or(false));
            let outcome = if clear_radio {
                form::clear_radio(st, &doc_id, page, index)?
            } else {
                registry::mutate(
                    st,
                    &doc_id,
                    MutateOpts::new("undo.formFill", ChangeReason::Edit).page(page),
                    |doc| form::set_value(doc, page, index, &value),
                )?
            };
            tracing::debug!(?outcome.method, "form field written");
            Ok(SetFormFieldResult {
                field: outcome.field,
                previous: outcome.previous,
                doc_generation: st.doc(&doc_id)?.generation,
            })
        })
        .await
}

/// P1, v0.3: every writable field back to its `/DV` (radio groups without one switched off),
/// one undo step.
#[tauri::command]
pub async fn reset_form(
    engine: State<'_, EngineHandle>,
    doc_id: String,
) -> Result<DocInfo, EngineError> {
    engine
        .call(Lane::Edit, "reset_form", move |st| {
            form::reset_form_info(st, &doc_id)
        })
        .await
}

// ---------------------------------------------------------------------------------------
// v0.3 pkg2-pages-structure-forms — 필드 만들기, 양식 데이터, 양식 평면화
// ---------------------------------------------------------------------------------------

/// F1: a new text / checkbox / radio / combo / signature field; `undo.formFieldCreate`.
#[tauri::command]
pub async fn create_form_field(
    engine: State<'_, EngineHandle>,
    doc_id: String,
    spec: FormFieldSpec,
) -> Result<FormEditResult, EngineError> {
    engine
        .call(Lane::Edit, "create_form_field", move |st| {
            form::author::create_field(st, &doc_id, &spec)
        })
        .await
}

/// F1: name, choices, required, max length; `undo.formFieldEdit`.
#[tauri::command]
pub async fn update_form_field(
    engine: State<'_, EngineHandle>,
    doc_id: String,
    page: PageIndex,
    index: u32,
    patch: FormFieldPatch,
) -> Result<FormEditResult, EngineError> {
    engine
        .call(Lane::Edit, "update_form_field", move |st| {
            form::author::update_field(st, &doc_id, page, index, &patch)
        })
        .await
}

/// F1: the widget (and its field when it was the last widget); `undo.formFieldDelete`.
#[tauri::command]
pub async fn delete_form_field(
    engine: State<'_, EngineHandle>,
    doc_id: String,
    page: PageIndex,
    index: u32,
) -> Result<FormEditResult, EngineError> {
    engine
        .call(Lane::Edit, "delete_form_field", move |st| {
            form::author::delete_field(st, &doc_id, page, index)
        })
        .await
}

/// F1: field values to a CSV or XFDF file (no document change).
#[tauri::command]
pub async fn export_form_data(
    engine: State<'_, EngineHandle>,
    doc_id: String,
    format: FormDataFormat,
    out_path: String,
) -> Result<FormDataResult, EngineError> {
    engine
        .call(Lane::Edit, "export_form_data", move |st| {
            form::data::export(st, &doc_id, format, &out_path)
        })
        .await
}

/// F1: field values from a CSV or XFDF file; one undo step `undo.formImport`.
#[tauri::command]
pub async fn import_form_data(
    engine: State<'_, EngineHandle>,
    doc_id: String,
    path: String,
    format: Option<FormDataFormat>,
) -> Result<FormDataResult, EngineError> {
    engine
        .call(Lane::Edit, "import_form_data", move |st| {
            form::data::import(st, &doc_id, &path, format)
        })
        .await
}

/// F2: 양식 평면화 — the fields drawn into their pages, the form removed; `undo.formFlatten`.
#[tauri::command]
pub async fn flatten_form(
    engine: State<'_, EngineHandle>,
    doc_id: String,
) -> Result<DocInfo, EngineError> {
    engine
        .call(Lane::Edit, "flatten_form", move |st| {
            form::flatten::flatten_form(st, &doc_id)
        })
        .await
}
