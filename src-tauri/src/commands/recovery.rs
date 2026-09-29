//! Autosave / crash recovery (P1-8) — `IPC_CONTRACT.md` §7.6c. Owner: **Stage 5**.
//!
//! Serialisation happens on the engine thread (`recovery::snapshot`); the file I/O runs on
//! this async task against `app_data_dir()/recovery/`, created lazily.

use crate::app::store::recovery_dir;
use crate::engine::recovery;
use crate::engine::{EngineHandle, Lane};
use crate::ipc::types::RecoveryEntry;
use crate::ipc::{EngineError, ErrorCode};
use parking_lot::Mutex;
use std::collections::HashMap;
use std::path::PathBuf;
use tauri::{AppHandle, State};

/// docId → recovery id, remembered on every write so `clear_recovery` still finds the pair
/// when the frontend calls it after `close_document` (doc ids are never reused).
#[derive(Default)]
pub struct RecoveryIds(pub Mutex<HashMap<String, String>>);

fn dir(app: &AppHandle) -> Result<PathBuf, EngineError> {
    recovery_dir(app).ok_or_else(|| EngineError::io("no app data directory"))
}

#[tauri::command]
pub async fn write_recovery(
    app: AppHandle,
    engine: State<'_, EngineHandle>,
    ids: State<'_, RecoveryIds>,
    doc_id: String,
) -> Result<RecoveryEntry, EngineError> {
    let dir = dir(&app)?;
    let snap = {
        let doc_id = doc_id.clone();
        engine
            .call(Lane::Edit, "write_recovery", move |st| {
                recovery::snapshot(st, &doc_id)
            })
            .await?
    };
    ids.0.lock().insert(doc_id, snap.id.clone());
    tauri::async_runtime::spawn_blocking(move || recovery::write(&dir, snap))
        .await
        .map_err(|e| EngineError::io(format!("write_recovery: {e}")))?
}

#[tauri::command]
pub async fn clear_recovery(
    app: AppHandle,
    engine: State<'_, EngineHandle>,
    ids: State<'_, RecoveryIds>,
    doc_id: String,
) -> Result<(), EngineError> {
    let from_doc = {
        let doc_id = doc_id.clone();
        engine
            .call(Lane::Edit, "clear_recovery", move |st| {
                recovery::recovery_id(st, &doc_id)
            })
            .await
    };
    let id = match from_doc {
        Ok(id) => id,
        // Already closed: fall back to what the last write remembered.
        Err(e) if e.code == ErrorCode::NotFound => None,
        Err(e) => return Err(e),
    };
    let id = id.or_else(|| ids.0.lock().get(&doc_id).cloned());
    ids.0.lock().remove(&doc_id);
    let Some(id) = id else {
        return Ok(());
    };
    let dir = dir(&app)?;
    tauri::async_runtime::spawn_blocking(move || recovery::discard(&dir, &id))
        .await
        .map_err(|e| EngineError::io(format!("clear_recovery: {e}")))?
}

/// Skips the copies of documents that are open right now (another window, or a reloaded main
/// window): those are live autosaves, not crash leftovers. `recovery::list` also skips those of
/// another running SeePDF process (v0.3 pkg5, H2: the sidecar's owner still holds its lock).
#[tauri::command]
pub async fn list_recovery(
    app: AppHandle,
    engine: State<'_, EngineHandle>,
) -> Result<Vec<RecoveryEntry>, EngineError> {
    let dir = dir(&app)?;
    let live = engine
        .call(Lane::Edit, "list_recovery", |st| {
            Ok(recovery::open_recovery_ids(st))
        })
        .await?;
    let entries = tauri::async_runtime::spawn_blocking(move || recovery::list(&dir))
        .await
        .map_err(|e| EngineError::io(format!("list_recovery: {e}")))??;
    Ok(entries
        .into_iter()
        .filter(|e| !live.contains(&e.id))
        .collect())
}

#[tauri::command]
pub async fn discard_recovery(app: AppHandle, id: String) -> Result<(), EngineError> {
    let dir = dir(&app)?;
    tauri::async_runtime::spawn_blocking(move || recovery::discard(&dir, &id))
        .await
        .map_err(|e| EngineError::io(format!("discard_recovery: {e}")))?
}
