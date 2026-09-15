//! Opening files from outside the webview: argv, Finder / `open -a`, Dock drops and
//! window drag-and-drop (tauri spike §4).
//!
//! A path can arrive **before** the webview has registered its listeners, so every open
//! request is both queued (drained by `take_pending_opens` on mount) and broadcast as
//! `open-file`. Neither route alone is reliable.

use crate::ipc::types::{OpenFilePayload, OpenRequest, OpenSource};
use parking_lot::Mutex;
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Emitter, Manager};

#[derive(Default)]
pub struct PendingOpens(pub Mutex<Vec<OpenRequest>>);

impl PendingOpens {
    pub fn take(&self) -> Vec<OpenRequest> {
        std::mem::take(&mut *self.0.lock())
    }
}

pub fn is_pdf(path: &Path) -> bool {
    path.extension()
        .map(|e| e.eq_ignore_ascii_case("pdf"))
        .unwrap_or(false)
}

/// `.pdf` paths passed on the command line (Windows double-click, `cargo run -- x.pdf`).
pub fn pdf_paths_from_argv() -> Vec<PathBuf> {
    std::env::args_os()
        .skip(1)
        .map(PathBuf::from)
        .filter(|p| is_pdf(p) && p.is_file())
        .collect()
}

/// Queue **and** broadcast, so a path is never lost regardless of timing.
pub fn push_open(app: &AppHandle, path: PathBuf, source: OpenSource) {
    let path = path.display().to_string();
    tracing::info!(%path, ?source, "open request");
    app.state::<PendingOpens>().0.lock().push(OpenRequest {
        path: path.clone(),
        source,
    });
    if let Err(e) = app.emit("open-file", OpenFilePayload { path, source }) {
        tracing::warn!("emit open-file failed: {e}");
    }
}

/// macOS `RunEvent::Opened { urls }`: Finder double-click, `open -a SeePDF x.pdf`, Dock drop.
/// Only fires for a bundled `.app` whose `Info.plist` has `CFBundleDocumentTypes`, which
/// `bundle.fileAssociations` generates.
pub fn handle_opened_urls(app: &AppHandle, urls: Vec<tauri::Url>) {
    for url in urls {
        match url.to_file_path() {
            Ok(path) => push_open(app, path, OpenSource::MacosOpened),
            Err(()) => tracing::warn!(%url, "RunEvent::Opened with a non-file url"),
        }
    }
}

/// Window drag-and-drop. The webview gets the same event via `onDragDropEvent`; this side
/// exists so a drop still opens the file when the frontend has not mounted yet.
pub fn handle_drop(app: &AppHandle, paths: &[PathBuf]) {
    for path in paths.iter().filter(|p| is_pdf(p)) {
        push_open(app, path.clone(), OpenSource::Drop);
    }
}
