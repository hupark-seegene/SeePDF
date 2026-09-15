//! Page objects (text and image editing) — `IPC_CONTRACT.md` §7.4. Owner: **Stage 1 (b)**.
//!
//! `objectId` is the page-object index and is valid **only** for the generation it was
//! listed in, which is why every mutating command carries `expectGeneration` and fails with
//! `stale` when it no longer matches. Every command here regenerates the page content once.

use crate::engine::objects;
use crate::engine::{EngineHandle, Lane};
use crate::ipc::types::{
    DocGeneration, ObjectId, PageIndex, PageObjectList, Point, Rect, Rgb, TextAlign,
    TextEditProbe, TextObjectPatch,
};
use crate::ipc::EngineError;
use tauri::State;

#[tauri::command]
pub async fn list_page_objects(
    engine: State<'_, EngineHandle>,
    doc_id: String,
    page: PageIndex,
) -> Result<PageObjectList, EngineError> {
    engine
        .call(Lane::Interactive, "list_page_objects", move |st| {
            let doc = st.doc_mut(&doc_id)?;
            objects::list(doc, page)
        })
        .await
}

#[tauri::command]
pub async fn probe_text_edit(
    engine: State<'_, EngineHandle>,
    doc_id: String,
    page: PageIndex,
    object_id: ObjectId,
    text: String,
) -> Result<TextEditProbe, EngineError> {
    engine
        .call(Lane::Interactive, "probe_text_edit", move |st| {
            let doc = st.doc_mut(&doc_id)?;
            objects::probe(doc, page, object_id, &text)
        })
        .await
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn edit_text_object(
    engine: State<'_, EngineHandle>,
    doc_id: String,
    page: PageIndex,
    object_id: ObjectId,
    expect_generation: DocGeneration,
    patch: TextObjectPatch,
    allow_font_substitution: bool,
) -> Result<PageObjectList, EngineError> {
    engine
        .call(Lane::Edit, "edit_text_object", move |st| {
            objects::edit_text(
                st,
                &doc_id,
                page,
                object_id,
                expect_generation,
                patch,
                allow_font_substitution,
            )
        })
        .await
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn add_text_object(
    engine: State<'_, EngineHandle>,
    doc_id: String,
    page: PageIndex,
    rect: Rect,
    text: String,
    font_size_pt: f32,
    color: Rgb,
    align: TextAlign,
) -> Result<PageObjectList, EngineError> {
    engine
        .call(Lane::Edit, "add_text_object", move |st| {
            objects::add_text(st, &doc_id, page, rect, &text, font_size_pt, color, align)
        })
        .await
}

#[tauri::command]
pub async fn add_image_object(
    engine: State<'_, EngineHandle>,
    doc_id: String,
    page: PageIndex,
    rect: Rect,
    path: String,
    keep_aspect: bool,
) -> Result<PageObjectList, EngineError> {
    engine
        .call(Lane::Edit, "add_image_object", move |st| {
            objects::add_image(st, &doc_id, page, rect, &path, keep_aspect)
        })
        .await
}

/// 이미지 바꾸기 — swap the bitmap of an existing image object, keeping its matrix (so the
/// replacement lands at exactly the same place and size). `STAGE1B_NOTES.md` §5.4.
#[tauri::command]
pub async fn replace_image(
    engine: State<'_, EngineHandle>,
    doc_id: String,
    page: PageIndex,
    object_id: ObjectId,
    expect_generation: DocGeneration,
    path: String,
) -> Result<PageObjectList, EngineError> {
    engine
        .call(Lane::Edit, "replace_image", move |st| {
            objects::replace_image(st, &doc_id, page, object_id, expect_generation, &path)
        })
        .await
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn transform_object(
    engine: State<'_, EngineHandle>,
    doc_id: String,
    page: PageIndex,
    object_id: ObjectId,
    expect_generation: DocGeneration,
    translate: Option<Point>,
    scale: Option<[f32; 2]>,
    rotate_deg: Option<f32>,
) -> Result<PageObjectList, EngineError> {
    engine
        .call(Lane::Edit, "transform_object", move |st| {
            objects::transform(
                st,
                &doc_id,
                page,
                object_id,
                expect_generation,
                translate,
                scale,
                rotate_deg,
            )
        })
        .await
}

#[tauri::command]
pub async fn delete_objects(
    engine: State<'_, EngineHandle>,
    doc_id: String,
    page: PageIndex,
    object_ids: Vec<ObjectId>,
    expect_generation: DocGeneration,
) -> Result<PageObjectList, EngineError> {
    engine
        .call(Lane::Edit, "delete_objects", move |st| {
            objects::delete(st, &doc_id, page, &object_ids, expect_generation)
        })
        .await
}
