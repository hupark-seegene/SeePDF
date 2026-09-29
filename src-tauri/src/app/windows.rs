//! Window creation and the label ↔ document binding.
//!
//! One document per window. The main window is declared in `tauri.conf.json`; extra windows
//! are created here with the label `doc-<n>`, which is what `capabilities/default.json`
//! grants permissions to (`windows: ["main", "doc-*"]`).

use crate::app::files;
use crate::ipc::types::{DocId, OpenSource};
use crate::ipc::EngineError;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use tauri::{AppHandle, DragDropEvent, Manager, WebviewUrl, WebviewWindowBuilder, WindowEvent};

/// Which document each window is showing, so the engine can close documents whose window is
/// gone and so `doc-changed` consumers can be scoped.
#[derive(Default)]
pub struct WindowDocs {
    next: AtomicU32,
    map: Mutex<HashMap<String, DocId>>,
}

impl WindowDocs {
    pub fn bind(&self, label: &str, doc_id: Option<DocId>) {
        let mut map = self.map.lock();
        match doc_id {
            Some(id) => map.insert(label.to_string(), id),
            None => map.remove(label),
        };
    }

    pub fn doc_of(&self, label: &str) -> Option<DocId> {
        self.map.lock().get(label).cloned()
    }

    pub fn windows_of(&self, doc_id: &str) -> Vec<String> {
        self.map
            .lock()
            .iter()
            .filter(|(_, id)| id.as_str() == doc_id)
            .map(|(label, _)| label.clone())
            .collect()
    }

    fn next_label(&self) -> String {
        format!("doc-{}", self.next.fetch_add(1, Ordering::Relaxed) + 1)
    }

    /// Every (window label, document) binding.
    pub fn bindings(&self) -> Vec<(String, DocId)> {
        self.map
            .lock()
            .iter()
            .map(|(l, d)| (l.clone(), d.clone()))
            .collect()
    }
}

// ---------------------------------------------------------------------------------------
// v0.3 pkg3 (H8): one window per file
// ---------------------------------------------------------------------------------------

/// The canonical form two paths are compared in (`fs::canonicalize`, or the path itself when
/// it cannot be resolved).
fn canonical(path: &std::path::Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// The label of the window whose document is the file at `path`, if any. `path_of` looks a
/// docId up in the engine's document mirror. Pure, so it is unit-testable.
pub fn window_for_path(
    bindings: &[(String, DocId)],
    path: &std::path::Path,
    path_of: impl Fn(&DocId) -> Option<PathBuf>,
) -> Option<String> {
    let want = canonical(path);
    let mut found: Vec<&(String, DocId)> = bindings
        .iter()
        .filter(|(_, doc)| path_of(doc).is_some_and(|p| canonical(&p) == want))
        .collect();
    // Deterministic when (against the rule) two windows show the same file.
    found.sort();
    found.first().map(|(label, _)| label.clone())
}

/// `focus_document_window`: when another window already shows the file at `path`, bring it to
/// the front (un-minimise, show, focus) and return its label; `None` when no window has it.
/// The caller's own window counts too — the frontend decides what to do then.
pub fn focus_window_for_path(app: &AppHandle, path: &str) -> Option<String> {
    let engine = app.try_state::<crate::engine::EngineHandle>()?;
    let bindings = app.state::<WindowDocs>().bindings();
    let label = {
        let docs = engine.shared.docs.read();
        window_for_path(&bindings, std::path::Path::new(path), |id| {
            docs.get(id).and_then(|d| d.path.clone())
        })
    }?;
    if let Some(window) = app.get_webview_window(&label) {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
    Some(label)
}

/// Window geometry shared by every window (`UI_SPEC.md` §2, tauri spike §4).
fn configure(
    builder: WebviewWindowBuilder<'_, tauri::Wry, AppHandle>,
) -> WebviewWindowBuilder<'_, tauri::Wry, AppHandle> {
    // Drag-drop is enabled by default; `disable_drag_drop_handler()` would turn it off.
    let builder = builder
        .title("SeePDF")
        .inner_size(1400.0, 900.0)
        .min_inner_size(800.0, 520.0);
    #[cfg(target_os = "macos")]
    let builder = builder
        .title_bar_style(tauri::TitleBarStyle::Overlay)
        .hidden_title(true);
    builder
}

/// `open_in_new_window` (`IPC_CONTRACT.md` §4). Returns the new window's label. The path is
/// queued for the new window only (v0.3 integration: it used to be broadcast, and the window
/// that asked replaced its own document with it).
pub fn open_in_new_window(app: &AppHandle, path: Option<String>) -> Result<String, EngineError> {
    match path {
        Some(path) => open_window_with(app, PathBuf::from(path), OpenSource::Dialog),
        None => new_window(app),
    }
}

/// A new window that opens `path` once it has mounted.
pub fn open_window_with(
    app: &AppHandle,
    path: PathBuf,
    source: OpenSource,
) -> Result<String, EngineError> {
    let label = app.state::<WindowDocs>().next_label();
    // Queued before the window exists, so its first `take_pending_opens` finds it.
    files::push_open(app, &label, path, source);
    if let Err(e) = build_window(app, &label) {
        app.state::<files::PendingOpens>().forget(&label);
        return Err(e);
    }
    Ok(label)
}

fn new_window(app: &AppHandle) -> Result<String, EngineError> {
    let label = app.state::<WindowDocs>().next_label();
    build_window(app, &label)?;
    Ok(label)
}

fn build_window(app: &AppHandle, label: &str) -> Result<(), EngineError> {
    let window = configure(WebviewWindowBuilder::new(
        app,
        label.to_string(),
        WebviewUrl::default(),
    ))
    .build()
    .map_err(|e| EngineError::io(format!("create window {label}: {e}")))?;
    attach_handlers(app, &window);
    Ok(())
}

/// Rust-side drag-drop and close bookkeeping. The webview gets the same drop event through
/// `onDragDropEvent`; this side keeps working before the frontend mounts.
pub fn attach_handlers(app: &AppHandle, window: &tauri::WebviewWindow) {
    let handle = app.clone();
    let label = window.label().to_string();
    window.on_window_event(move |event| match event {
        WindowEvent::DragDrop(DragDropEvent::Drop { paths, .. }) => {
            files::handle_drop(&handle, &label, paths);
        }
        WindowEvent::Destroyed => {
            handle.state::<WindowDocs>().bind(&label, None);
            handle.state::<files::PendingOpens>().forget(&label);
        }
        // Stage 8: the native 편집 menu names the focused window's undo step.
        #[cfg(target_os = "macos")]
        WindowEvent::Focused(true) => {
            crate::app::menu::window_focused(&handle, &label);
        }
        _ => {}
    });
}

#[cfg(test)]
mod tests {
    use super::window_for_path;
    use std::path::PathBuf;

    #[test]
    fn finds_the_window_of_a_path() {
        let dir = std::env::temp_dir().join(format!("seepdf-winpath-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let a = dir.join("계약서.pdf");
        std::fs::write(&a, b"%PDF").unwrap();
        let bindings = vec![
            ("main".to_string(), "d1".to_string()),
            ("doc-1".to_string(), "d2".to_string()),
        ];
        let path_of = |id: &String| match id.as_str() {
            "d2" => Some(a.clone()),
            _ => Some(PathBuf::from("/nowhere/other.pdf")),
        };
        // A non-canonical spelling of the same file still matches.
        let spelled = dir.join(".").join("계약서.pdf");
        assert_eq!(
            window_for_path(&bindings, &spelled, path_of).as_deref(),
            Some("doc-1")
        );
        assert_eq!(
            window_for_path(&bindings, &dir.join("없음.pdf"), path_of),
            None
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
