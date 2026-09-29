//! Recents, settings and app info — `IPC_CONTRACT.md` §11. Owner: Stage 0.

use crate::app::store;
use crate::engine::render::cache::{Night, RenderKind, TileKey};
use crate::engine::render::tiles::RenderRequest;
use crate::engine::{EngineHandle, Lane};
use crate::ipc::types::{AppInfo, RecentEntry, Settings, ThumbId};
use crate::ipc::EngineError;
use tauri::{AppHandle, State};

/// Width of the thumbnail stored for the welcome screen's recent cards.
const RECENT_THUMB_WIDTH: u32 = 240;

#[tauri::command]
pub fn get_recent(app: AppHandle) -> Vec<RecentEntry> {
    store::get_recent(&app)
}

#[tauri::command]
pub fn update_recent(app: AppHandle, entry: RecentEntry) -> Result<(), EngineError> {
    store::update_recent(&app, entry)
}

#[tauri::command]
pub fn remove_recent(app: AppHandle, path: String) -> Result<(), EngineError> {
    store::remove_recent(&app, &path)
}

#[tauri::command]
pub fn set_recent_pinned(app: AppHandle, path: String, pinned: bool) -> Result<(), EngineError> {
    store::set_recent_pinned(&app, &path, pinned)
}

#[tauri::command]
pub fn clear_recent(app: AppHandle) -> Result<(), EngineError> {
    store::clear_recent(&app)
}

/// Renders page 0 at [`RECENT_THUMB_WIDTH`] and stores it under `$APPDATA/SeePDF/thumbs/`,
/// where the `/recent-thumb` protocol route serves it from.
#[tauri::command]
pub async fn write_recent_thumbnail(
    app: AppHandle,
    engine: State<'_, EngineHandle>,
    doc_id: String,
) -> Result<ThumbId, EngineError> {
    let generation = engine
        .shared
        .doc(&doc_id)
        .ok_or_else(|| EngineError::not_found(format!("unknown document '{doc_id}'")))?
        .generation;
    let key = TileKey {
        doc: doc_id,
        generation,
        page: 0,
        kind: RenderKind::Thumb,
        scale_key: RECENT_THUMB_WIDTH,
        rotation: 0,
        tx: 0,
        ty: 0,
        night: Night::Off,
        hl: false,
        forms: true,
    };
    let png = engine
        .call(Lane::Thumb, "write_recent_thumbnail", move |st| {
            let raw = crate::engine::render::tiles::render(st, &RenderRequest::new(key))?;
            crate::engine::render::encode::encode_png(&raw)
        })
        .await?;
    let thumb_id = store::write_thumbnail(&app, &png)?;
    Ok(ThumbId { thumb_id })
}

/// P1-9: a typed signature, rendered to PNG by the webview, written under
/// `$APPDATA/SeePDF/signatures/` so the 서명 tool can place it as a `StampImage::Path`.
/// Returns the absolute path. File I/O only — no pdfium, so it never touches the engine thread.
#[tauri::command]
pub async fn write_signature_image(app: AppHandle, bytes: Vec<u8>) -> Result<String, EngineError> {
    let dir =
        store::signatures_dir(&app).ok_or_else(|| EngineError::io("no app data directory"))?;
    tauri::async_runtime::spawn_blocking(move || crate::app::signatures::write_png(&dir, &bytes))
        .await
        .map_err(|e| EngineError::io(format!("write signature image: {e}")))?
        .map(|path| path.display().to_string())
}

#[tauri::command]
pub fn reveal_in_file_manager(path: String) -> Result<(), EngineError> {
    store::reveal(&path)
}

#[tauri::command]
pub fn get_settings(app: AppHandle) -> Settings {
    store::get_settings(&app)
}

/// The native menu is built before the webview exists, so a language change has to rebuild
/// it (STAGE0 §4.6). Only when the locale actually moved: `set_settings` is also how the
/// theme, the zoom default and every tool default are written, and rebuilding the macOS menu
/// on each of those would flash the menu bar.
///
/// v0.3 (U3): a new 캐시 크기 applies at once — the tile cache evicts down to it (no pdfium,
/// so it runs right here, not on the engine thread).
#[tauri::command]
pub fn set_settings(
    app: AppHandle,
    engine: State<'_, EngineHandle>,
    patch: serde_json::Value,
) -> Result<Settings, EngineError> {
    let previous = store::get_settings(&app);
    let before = previous.locale;
    let settings = store::set_settings(&app, patch)?;
    if settings.tile_cache_mb != previous.tile_cache_mb {
        engine
            .shared
            .tiles
            .set_budget(tile_cache_bytes(settings.tile_cache_mb));
    }
    #[cfg(target_os = "macos")]
    if settings.locale != before {
        crate::app::menu::rebuild(&app, settings.locale);
    }
    #[cfg(not(target_os = "macos"))]
    let _ = before;
    Ok(settings)
}

// --- v0.3 pkg6 (U3 설정 › 고급, V1 스냅샷) -------------------------------------------------

/// `Settings.tileCacheMb` → the tile cache budget in bytes (16 MB … 1 GB, as at startup).
pub fn tile_cache_bytes(mb: u32) -> usize {
    (mb.clamp(16, 1024) as usize) * 1024 * 1024
}

/// 기본값으로 되돌리기: the built-in settings, so the frontend can reset to them without a
/// second copy of the defaults (it keeps the author, the saved signatures and anything it
/// does not know about).
#[tauri::command]
pub fn get_default_settings() -> Settings {
    Settings::default()
}

/// 캐시 비우기: drops every encoded tile and page image. No pdfium; pages re-render on demand.
/// Returns the bytes freed.
#[tauri::command]
pub fn clear_render_cache(engine: State<'_, EngineHandle>) -> u64 {
    let freed = engine.shared.tiles.bytes() as u64;
    engine.shared.tiles.clear();
    freed
}

/// 스냅샷 › PNG로 저장… (V1): the clipboard refused the image, so it goes to the file the user
/// picked in the save panel. Only a PNG (signature checked) to a `.png` path is written.
#[tauri::command]
pub async fn save_snapshot_png(path: String, bytes: Vec<u8>) -> Result<(), EngineError> {
    const PNG: &[u8] = &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
    if !bytes.starts_with(PNG) {
        return Err(EngineError::invalid("not a PNG image"));
    }
    let target = std::path::PathBuf::from(&path);
    let is_png = target
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("png"));
    if !is_png {
        return Err(EngineError::invalid("a snapshot is saved as .png"));
    }
    tauri::async_runtime::spawn_blocking(move || std::fs::write(&target, &bytes))
        .await
        .map_err(|e| EngineError::io(format!("save snapshot: {e}")))?
        .map_err(|e| EngineError::io(format!("{path}: {e}")))
}

/// Not part of the contract's command list: a diagnostics read the status bar and bug
/// reports use. Cheap, synchronous, no pdfium call.
#[tauri::command]
pub fn app_info(app: AppHandle, engine: State<'_, EngineHandle>) -> AppInfo {
    let settings = store::get_settings(&app);
    AppInfo {
        version: app.package_info().version.to_string(),
        os: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
        debug: cfg!(debug_assertions),
        pdfium_version: engine.shared.pdfium_version.read().clone(),
        pdfium_dir: engine.shared.pdfium_dir.display().to_string(),
        locale: settings.locale,
        theme: settings.theme,
    }
}

// ---------------------------------------------------------------------------------------
// v0.3 pkg5: 도움말 › 문제 보고… / 로그 폴더 열기 (H10), 오픈 소스 라이선스 (H11)
// ---------------------------------------------------------------------------------------

use crate::app::diagnostics;
use crate::ipc::types::ProblemReport;

fn log_dir() -> Result<std::path::PathBuf, EngineError> {
    diagnostics::log_dir().ok_or_else(|| EngineError::io("no log directory"))
}

/// 문제 보고…: version, OS / arch, pdfium version and the last 200 log lines as one text
/// (the frontend copies it to the clipboard), also written to `problem-report.txt` in the log
/// folder (the frontend reveals it). File I/O only; no pdfium.
#[tauri::command]
pub async fn problem_report(
    app: AppHandle,
    engine: State<'_, EngineHandle>,
) -> Result<ProblemReport, EngineError> {
    let info = app_info(app, engine);
    let dir = log_dir()?;
    tauri::async_runtime::spawn_blocking(move || {
        let lines = diagnostics::recent_log_lines(&dir, diagnostics::REPORT_LOG_LINES);
        let text = diagnostics::problem_report(&info, &lines);
        let path = diagnostics::write_report(&dir, &text).map_err(EngineError::io)?;
        Ok(ProblemReport {
            text,
            path: path.display().to_string(),
        })
    })
    .await
    .map_err(|e| EngineError::io(format!("problem_report: {e}")))?
}

/// 로그 폴더 열기: the log directory in Finder / Explorer (created if it does not exist yet).
#[tauri::command]
pub fn open_log_folder() -> Result<String, EngineError> {
    let dir = log_dir()?;
    std::fs::create_dir_all(&dir).map_err(EngineError::from)?;
    tauri_plugin_opener::open_path(&dir, None::<&str>)
        .map_err(|e| EngineError::io(format!("open {}: {e}", dir.display())))?;
    Ok(dir.display().to_string())
}

/// SeePDF 정보 › 오픈 소스 라이선스: `THIRD_PARTY_NOTICES.txt`, which
/// `scripts/gen-notices.mjs` builds before every release bundle. A build without it (`tauri
/// dev` before the script ran) answers with the licences that always ship next to the binary.
#[tauri::command]
pub async fn third_party_notices(app: AppHandle) -> Result<String, EngineError> {
    use tauri::Manager;
    let mut roots = Vec::new();
    if let Ok(res) = app.path().resource_dir() {
        roots.push(res.join("resources"));
    }
    if cfg!(debug_assertions) {
        roots.push(std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources"));
    }
    tauri::async_runtime::spawn_blocking(move || read_notices(&roots))
        .await
        .map_err(|e| EngineError::io(format!("third_party_notices: {e}")))?
}

/// The generated notices under the first root that has them, else the bundled licence files.
pub fn read_notices(roots: &[std::path::PathBuf]) -> Result<String, EngineError> {
    for root in roots {
        if let Ok(text) = std::fs::read_to_string(root.join("notices/THIRD_PARTY_NOTICES.txt")) {
            return Ok(text);
        }
    }
    let mut out = String::new();
    for root in roots {
        for (title, rel) in [
            ("PDFium", "pdfium/LICENSE.pdfium"),
            ("SeePDF-Hangul.ttf (Noto Sans KR)", "fonts/OFL.txt"),
        ] {
            if let Ok(text) = std::fs::read_to_string(root.join(rel)) {
                out.push_str(&format!("==== {title} ====\n\n{text}\n\n"));
            }
        }
        if !out.is_empty() {
            return Ok(out);
        }
    }
    Err(EngineError::not_found(
        "THIRD_PARTY_NOTICES.txt is not bundled",
    ))
}
