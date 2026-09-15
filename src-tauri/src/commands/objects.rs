//! Page objects (text and image editing) — `IPC_CONTRACT.md` §7.4. Owner: **Stage 1 (b)**.
//!
//! `objectId` is the page-object index and is valid **only** for the generation it was
//! listed in, which is why every mutating command carries `expectGeneration` and fails with
//! `stale` when it no longer matches. Every command here regenerates the page content once.

use crate::engine::EngineHandle;
use crate::ipc::types::{
    DocGeneration, ObjectId, PageIndex, PageObjectList, Point, Rect, Rgb, TextAlign,
    TextEditProbe, TextObjectPatch,
};
use crate::ipc::EngineError;
use tauri::State;

#[tauri::command]
pub async fn list_page_objects(
    _engine: State<'_, EngineHandle>,
    _doc_id: String,
    _page: PageIndex,
) -> Result<PageObjectList, EngineError> {
    Err(EngineError::unsupported("list_page_objects"))
}

#[tauri::command]
pub async fn probe_text_edit(
    _engine: State<'_, EngineHandle>,
    _doc_id: String,
    _page: PageIndex,
    _object_id: ObjectId,
    _text: String,
) -> Result<TextEditProbe, EngineError> {
    Err(EngineError::unsupported("probe_text_edit"))
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn edit_text_object(
    _engine: State<'_, EngineHandle>,
    _doc_id: String,
    _page: PageIndex,
    _object_id: ObjectId,
    _expect_generation: DocGeneration,
    _patch: TextObjectPatch,
    _allow_font_substitution: bool,
) -> Result<PageObjectList, EngineError> {
    Err(EngineError::unsupported("edit_text_object"))
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn add_text_object(
    _engine: State<'_, EngineHandle>,
    _doc_id: String,
    _page: PageIndex,
    _rect: Rect,
    _text: String,
    _font_size_pt: f32,
    _color: Rgb,
    _align: TextAlign,
) -> Result<PageObjectList, EngineError> {
    Err(EngineError::unsupported("add_text_object"))
}

#[tauri::command]
pub async fn add_image_object(
    _engine: State<'_, EngineHandle>,
    _doc_id: String,
    _page: PageIndex,
    _rect: Rect,
    _path: String,
    _keep_aspect: bool,
) -> Result<PageObjectList, EngineError> {
    Err(EngineError::unsupported("add_image_object"))
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn transform_object(
    _engine: State<'_, EngineHandle>,
    _doc_id: String,
    _page: PageIndex,
    _object_id: ObjectId,
    _expect_generation: DocGeneration,
    _translate: Option<Point>,
    _scale: Option<[f32; 2]>,
    _rotate_deg: Option<f32>,
) -> Result<PageObjectList, EngineError> {
    Err(EngineError::unsupported("transform_object"))
}

#[tauri::command]
pub async fn delete_objects(
    _engine: State<'_, EngineHandle>,
    _doc_id: String,
    _page: PageIndex,
    _object_ids: Vec<ObjectId>,
    _expect_generation: DocGeneration,
) -> Result<PageObjectList, EngineError> {
    Err(EngineError::unsupported("delete_objects"))
}
