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

/// v0.3 pkg5 (H2): the existing `.pdf` files among `args` (argv without the program), a
/// relative one resolved against `cwd` — the second process's working directory, not ours.
pub fn pdf_paths_from_args(args: impl IntoIterator<Item = String>, cwd: &Path) -> Vec<PathBuf> {
    args.into_iter()
        .map(PathBuf::from)
        .map(|p| if p.is_absolute() { p } else { cwd.join(p) })
        .filter(|p| is_pdf(p) && p.is_file())
        .collect()
}

/// v0.3 pkg5 (H2): `tauri-plugin-single-instance` callback (Windows). A second SeePDF was
/// started — an Explorer double-click while SeePDF runs — and has already exited; its PDFs
/// open here exactly as a macOS `RunEvent::Opened` does, and the app comes to the front.
pub fn open_from_second_instance(app: &AppHandle, argv: Vec<String>, cwd: String) {
    let paths = pdf_paths_from_args(argv.into_iter().skip(1), Path::new(&cwd));
    tracing::info!(count = paths.len(), "second instance handed over its files");
    for path in paths {
        push_open(app, path, OpenSource::Argv);
    }
    let window = app
        .get_webview_window("main")
        .or_else(|| app.webview_windows().into_values().next());
    if let Some(window) = window {
        if window.is_minimized().unwrap_or(false) {
            let _ = window.unminimize();
        }
        let _ = window.show();
        let _ = window.set_focus();
    }
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
