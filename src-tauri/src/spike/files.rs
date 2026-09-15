//! File-open plumbing: CLI argv (Windows double-click, `cargo run -- x.pdf`), macOS
//! Finder/`open` (`RunEvent::Opened { urls }`), and a queue for paths that arrive before
//! the webview has mounted its listeners.

use parking_lot::Mutex;
use std::path::PathBuf;
use tauri::{AppHandle, Emitter, Manager};

#[derive(Default)]
pub struct PendingOpens(pub Mutex<Vec<String>>);

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenFilePayload {
    pub path: String,
    pub source: &'static str,
}

/// `.pdf` paths passed on the command line (skips flags).
pub fn pdf_paths_from_argv() -> Vec<PathBuf> {
    std::env::args_os()
        .skip(1)
        .map(PathBuf::from)
        .filter(|p| {
            p.extension()
                .map(|e| e.eq_ignore_ascii_case("pdf"))
                .unwrap_or(false)
                && p.is_file()
        })
        .collect()
}

/// Queue + broadcast. The frontend calls `take_pending_opens` once mounted and listens to
/// `open-file` afterwards, so a path is never lost regardless of timing.
pub fn push_open(app: &AppHandle, path: PathBuf, source: &'static str) {
    let path = path.display().to_string();
    tracing::info!(%path, source, "open request");
    app.state::<PendingOpens>().0.lock().push(path.clone());
    if let Err(e) = app.emit("open-file", OpenFilePayload { path, source }) {
        tracing::warn!("emit open-file failed: {e}");
    }
}

/// macOS: `RunEvent::Opened { urls }` (Finder double-click / `open -a SeePDF x.pdf` / drag
/// onto the Dock icon). URLs are `file://` URLs.
pub fn handle_opened_urls(app: &AppHandle, urls: Vec<tauri::Url>) {
    for url in urls {
        match url.to_file_path() {
            Ok(path) => push_open(app, path, "macos-opened"),
            Err(()) => tracing::warn!(%url, "RunEvent::Opened with non-file url"),
        }
    }
}

#[tauri::command]
pub fn take_pending_opens(state: tauri::State<'_, PendingOpens>) -> Vec<String> {
    std::mem::take(&mut *state.0.lock())
}
