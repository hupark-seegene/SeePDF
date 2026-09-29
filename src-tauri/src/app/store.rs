//! Recents and settings, backed by `tauri-plugin-store` (`IPC_CONTRACT.md` §11).
//!
//! Both files are read and written **from Rust**, so locale, theme and window state are
//! available before the webview mounts.

use crate::ipc::types::{RecentEntry, Settings};
use crate::ipc::EngineError;
use std::path::PathBuf;
use tauri::{AppHandle, Emitter, Manager, Runtime};
use tauri_plugin_store::StoreExt;

pub const SETTINGS_FILE: &str = "settings.json";
pub const RECENTS_FILE: &str = "recents.json";
const RECENTS_KEY: &str = "entries";
const SETTINGS_KEY: &str = "settings";
/// Recents list cap (`UI_SPEC.md` welcome screen).
pub const MAX_RECENTS: usize = 40;

/// `$APPDATA/SeePDF/thumbs/`, where recent-file thumbnails live.
pub fn thumbs_dir<R: Runtime>(app: &AppHandle<R>) -> Option<PathBuf> {
    let dir = app.path().app_data_dir().ok()?.join("thumbs");
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir)
}

/// `$APPDATA/SeePDF/recovery/`, where autosave copies live. Not created here — the first
/// `write_recovery` creates it, and `list_recovery` treats a missing directory as empty.
pub fn recovery_dir<R: Runtime>(app: &AppHandle<R>) -> Option<PathBuf> {
    Some(app.path().app_data_dir().ok()?.join("recovery"))
}

/// `$APPDATA/SeePDF/signatures/`, where typed signatures are written as PNGs (P1-9). Created by
/// `app::signatures::write_png`.
pub fn signatures_dir<R: Runtime>(app: &AppHandle<R>) -> Option<PathBuf> {
    Some(app.path().app_data_dir().ok()?.join("signatures"))
}

/// v0.3 T2: `$APPDATA/SeePDF/stamps/`, where 내 도장 images are copied.
pub fn stamps_dir<R: Runtime>(app: &AppHandle<R>) -> Option<PathBuf> {
    Some(app.path().app_data_dir().ok()?.join("stamps"))
}

/// `$TEMP/seepdf-history/`, where undo snapshots spill.
pub fn history_spill_dir<R: Runtime>(app: Option<&AppHandle<R>>) -> PathBuf {
    let base = app
        .and_then(|a| a.path().temp_dir().ok())
        .unwrap_or_else(std::env::temp_dir);
    let dir = base
        .join("seepdf-history")
        .join(std::process::id().to_string());
    let _ = std::fs::create_dir_all(&dir);
    dir
}

pub fn get_settings<R: Runtime>(app: &AppHandle<R>) -> Settings {
    let Ok(store) = app.store(SETTINGS_FILE) else {
        return Settings::default();
    };
    store
        .get(SETTINGS_KEY)
        .and_then(|v| serde_json::from_value::<Settings>(v).ok())
        .unwrap_or_default()
}

/// v0.3 A9: the store key that records the one-time 작성자 prefill, so a name the user
/// cleared on purpose is never filled in again.
const AUTHOR_PREFILLED_KEY: &str = "authorPrefilled";

/// v0.3 A9: on the first run (or the first run of a version with this), an empty
/// 설정 ▸ 주석 작성자 is filled with the OS user name, so new annotations carry an author from
/// the start. Runs once per settings file.
pub fn prefill_author<R: Runtime>(app: &AppHandle<R>) {
    let Ok(store) = app.store(SETTINGS_FILE) else {
        return;
    };
    if store.get(AUTHOR_PREFILLED_KEY).is_some() {
        return;
    }
    if get_settings(app).author.trim().is_empty() {
        if let Some(name) = os_user_name() {
            let _ = set_settings(app, serde_json::json!({ "author": name }));
        }
    }
    store.set(AUTHOR_PREFILLED_KEY, serde_json::Value::Bool(true));
    let _ = store.save();
}

/// The OS account name: `USERNAME` (Windows) / `USER` (macOS, Linux), then `whoami`.
/// A `DOMAIN\user` answer keeps the user part.
pub fn os_user_name() -> Option<String> {
    let from_env = ["USERNAME", "USER"]
        .iter()
        .filter_map(|k| std::env::var(k).ok())
        .map(|v| v.trim().to_string())
        .find(|v| !v.is_empty());
    let name = from_env.or_else(|| {
        std::process::Command::new("whoami")
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .filter(|v| !v.is_empty())
    })?;
    let name = name.rsplit('\\').next().unwrap_or(&name).trim().to_string();
    (!name.is_empty()).then_some(name)
}

/// Merges `patch` (a partial `Settings` object) into the stored settings.
pub fn set_settings<R: Runtime>(
    app: &AppHandle<R>,
    patch: serde_json::Value,
) -> Result<Settings, EngineError> {
    let current = get_settings(app);
    let mut merged =
        serde_json::to_value(&current).map_err(|e| EngineError::io(format!("settings: {e}")))?;
    if let (Some(base), Some(patch)) = (merged.as_object_mut(), patch.as_object()) {
        for (k, v) in patch {
            base.insert(k.clone(), v.clone());
        }
    }
    let settings: Settings = serde_json::from_value(merged.clone())
        .map_err(|e| EngineError::invalid(format!("settings patch: {e}")))?;
    let store = app
        .store(SETTINGS_FILE)
        .map_err(|e| EngineError::io(format!("open {SETTINGS_FILE}: {e}")))?;
    store.set(SETTINGS_KEY, merged);
    store
        .save()
        .map_err(|e| EngineError::io(format!("save {SETTINGS_FILE}: {e}")))?;
    Ok(settings)
}

pub fn get_recent<R: Runtime>(app: &AppHandle<R>) -> Vec<RecentEntry> {
    let Ok(store) = app.store(RECENTS_FILE) else {
        return Vec::new();
    };
    let mut entries: Vec<RecentEntry> = store
        .get(RECENTS_KEY)
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default();
    // Pinned first, then most recently opened.
    entries.sort_by(|a, b| {
        b.pinned
            .cmp(&a.pinned)
            .then_with(|| b.last_opened.cmp(&a.last_opened))
    });
    entries
}

fn write_recent<R: Runtime>(
    app: &AppHandle<R>,
    entries: Vec<RecentEntry>,
) -> Result<(), EngineError> {
    let store = app
        .store(RECENTS_FILE)
        .map_err(|e| EngineError::io(format!("open {RECENTS_FILE}: {e}")))?;
    let value =
        serde_json::to_value(&entries).map_err(|e| EngineError::io(format!("recents: {e}")))?;
    store.set(RECENTS_KEY, value);
    store
        .save()
        .map_err(|e| EngineError::io(format!("save {RECENTS_FILE}: {e}")))?;
    let _ = app.emit("recents-changed", ());
    Ok(())
}

pub fn update_recent<R: Runtime>(
    app: &AppHandle<R>,
    entry: RecentEntry,
) -> Result<(), EngineError> {
    let mut entries = get_recent(app);
    entries.retain(|e| e.path != entry.path);
    entries.insert(0, entry);
    // Keep every pinned entry, then fill up to MAX_RECENTS with the newest.
    if entries.len() > MAX_RECENTS {
        let mut kept: Vec<RecentEntry> = entries.iter().filter(|e| e.pinned).cloned().collect();
        for e in entries.into_iter().filter(|e| !e.pinned) {
            if kept.len() >= MAX_RECENTS {
                break;
            }
            kept.push(e);
        }
        entries = kept;
    }
    write_recent(app, entries)
}

pub fn remove_recent<R: Runtime>(app: &AppHandle<R>, path: &str) -> Result<(), EngineError> {
    let mut entries = get_recent(app);
    let removed = entries.iter().find(|e| e.path == path).cloned();
    entries.retain(|e| e.path != path);
    if let Some(entry) = removed {
        if let (Some(id), Some(dir)) = (entry.thumb_id, thumbs_dir(app)) {
            let _ = std::fs::remove_file(dir.join(format!("{id}.png")));
        }
    }
    write_recent(app, entries)
}

pub fn set_recent_pinned<R: Runtime>(
    app: &AppHandle<R>,
    path: &str,
    pinned: bool,
) -> Result<(), EngineError> {
    let mut entries = get_recent(app);
    let mut found = false;
    for entry in entries.iter_mut() {
        if entry.path == path {
            entry.pinned = pinned;
            found = true;
        }
    }
    if !found {
        return Err(EngineError::not_found(format!(
            "no recent entry for {path}"
        )));
    }
    write_recent(app, entries)
}

pub fn clear_recent<R: Runtime>(app: &AppHandle<R>) -> Result<(), EngineError> {
    let entries = get_recent(app);
    if let Some(dir) = thumbs_dir(app) {
        for entry in &entries {
            if let Some(id) = &entry.thumb_id {
                let _ = std::fs::remove_file(dir.join(format!("{id}.png")));
            }
        }
    }
    // Pinned entries survive "메뉴 지우기", as they do in every macOS app.
    write_recent(app, entries.into_iter().filter(|e| e.pinned).collect())
}

/// Writes a thumbnail PNG for the recents grid and returns its id.
pub fn write_thumbnail<R: Runtime>(app: &AppHandle<R>, png: &[u8]) -> Result<String, EngineError> {
    let dir = thumbs_dir(app).ok_or_else(|| EngineError::io("no app data directory"))?;
    let id = uuid::Uuid::new_v4().simple().to_string();
    std::fs::write(dir.join(format!("{id}.png")), png)
        .map_err(|e| EngineError::io(format!("write thumbnail: {e}")))?;
    Ok(id)
}

/// Opens the platform file manager with `path` selected.
pub fn reveal(path: &str) -> Result<(), EngineError> {
    let path = PathBuf::from(path);
    if !path.exists() {
        return Err(EngineError::not_found(format!(
            "{} is gone",
            path.display()
        )));
    }
    #[cfg(target_os = "macos")]
    let result = std::process::Command::new("open")
        .arg("-R")
        .arg(&path)
        .spawn();
    #[cfg(target_os = "windows")]
    let result = std::process::Command::new("explorer")
        .arg(format!("/select,{}", path.display()))
        .spawn();
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let result = std::process::Command::new("xdg-open")
        .arg(path.parent().unwrap_or(&path))
        .spawn();
    result
        .map(|_| ())
        .map_err(|e| EngineError::io(format!("reveal: {e}")))
}
