//! Documents, history and view control — `IPC_CONTRACT.md` §4, §5, §7.8. Owner: Stage 0.

use crate::app::{PendingOpens, WindowDocs};
use crate::engine::registry;
use crate::engine::types::Viewport;
use crate::engine::{EngineHandle, Lane};
use crate::ipc::types::{
    DocInfo, EngineStats, OpenRequest, OutlineNode, PageIndex, Rect, ViewportHint,
};
use crate::ipc::EngineError;
use std::path::PathBuf;
use tauri::{AppHandle, State};

/// `displayName` (Stage 8): what `DocInfo.name` reports instead of the file name — a recovered
/// copy (`<uuid>.pdf`) opens under the original document's name.
#[tauri::command]
pub async fn open_document(
    engine: State<'_, EngineHandle>,
    path: String,
    password: Option<String>,
    display_name: Option<String>,
) -> Result<DocInfo, EngineError> {
    let bytes = super::read_file(&path).await?;
    let path_buf = PathBuf::from(&path);
    engine
        .call(Lane::Edit, "open_document", move |st| {
            registry::open_named(st, Some(path_buf), bytes, password, display_name)
        })
        .await
}

#[tauri::command]
pub async fn close_document(
    engine: State<'_, EngineHandle>,
    doc_id: String,
) -> Result<(), EngineError> {
    engine
        .call(Lane::Edit, "close_document", move |st| {
            registry::close(st, &doc_id)
        })
        .await
}

#[tauri::command]
pub async fn get_document(
    engine: State<'_, EngineHandle>,
    doc_id: String,
) -> Result<DocInfo, EngineError> {
    engine
        .call(Lane::Interactive, "get_document", move |st| {
            Ok(st.doc(&doc_id)?.info())
        })
        .await
}

#[tauri::command]
pub async fn get_outline(
    engine: State<'_, EngineHandle>,
    doc_id: String,
) -> Result<Vec<OutlineNode>, EngineError> {
    engine
        .call(Lane::Interactive, "get_outline", move |st| {
            Ok(st.doc(&doc_id)?.outline())
        })
        .await
}

#[tauri::command]
pub fn take_pending_opens(pending: State<'_, PendingOpens>) -> Vec<OpenRequest> {
    pending.take()
}

#[tauri::command]
pub fn open_in_new_window(app: AppHandle, path: Option<String>) -> Result<String, EngineError> {
    crate::app::windows::open_in_new_window(&app, path)
}

#[tauri::command]
pub fn window_bind_document(
    app: AppHandle,
    windows: State<'_, WindowDocs>,
    label: String,
    doc_id: Option<String>,
) -> Result<(), EngineError> {
    #[cfg(target_os = "macos")]
    crate::app::menu::document_bound(&app, &label, doc_id.as_deref());
    #[cfg(not(target_os = "macos"))]
    let _ = &app;
    windows.bind(&label, doc_id);
    Ok(())
}

/// Fire-and-forget; bumps the engine's viewport generation so in-flight renders for a place
/// the user has scrolled away from are dropped instead of queued behind the new ones.
#[tauri::command]
pub fn set_viewport(
    engine: State<'_, EngineHandle>,
    hint: ViewportHint,
) -> Result<(), EngineError> {
    engine.set_viewport(Viewport {
        doc_id: hint.doc_id,
        scale_key: hint.scale_key,
        rotation: hint.rotation,
        centre_page: hint.centre_page,
        first_page: hint.first_page,
        last_page: hint.last_page,
        velocity_px_per_ms: hint.velocity_px_per_ms,
    });
    Ok(())
}

/// Raw RGBA8 with a 32-byte `SPRX` header (`IPC_CONTRACT.md` §10.2) for the snapshot tool
/// and the print path. Tiles never use this — they go over `seepdf://`.
#[tauri::command]
pub async fn render_page_raw(
    engine: State<'_, EngineHandle>,
    doc_id: String,
    page: PageIndex,
    scale: f32,
    rect: Option<Rect>,
) -> Result<tauri::ipc::Response, EngineError> {
    let buffer = engine
        .call(Lane::Interactive, "render_page_raw", move |st| {
            crate::engine::render::tiles::render_raw_buffer(st, &doc_id, page, scale, rect)
        })
        .await?;
    Ok(tauri::ipc::Response::new(buffer))
}

#[tauri::command]
pub fn engine_stats(engine: State<'_, EngineHandle>) -> EngineStats {
    let docs = engine.shared.docs.read().len() as u32;
    engine
        .shared
        .stats
        .snapshot(docs, engine.shared.tiles.bytes() as u64)
}

#[tauri::command]
pub async fn undo(engine: State<'_, EngineHandle>, doc_id: String) -> Result<DocInfo, EngineError> {
    engine
        .call(Lane::Edit, "undo", move |st| {
            registry::undo(st, &doc_id, false)
        })
        .await
}

#[tauri::command]
pub async fn redo(engine: State<'_, EngineHandle>, doc_id: String) -> Result<DocInfo, EngineError> {
    engine
        .call(Lane::Edit, "redo", move |st| registry::undo(st, &doc_id, true))
        .await
}
