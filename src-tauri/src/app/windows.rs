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

/// `open_in_new_window` (`IPC_CONTRACT.md` §4). Returns the new window's label.
pub fn open_in_new_window(app: &AppHandle, path: Option<String>) -> Result<String, EngineError> {
    let label = app.state::<WindowDocs>().next_label();
    let window = configure(WebviewWindowBuilder::new(
        app,
        label.clone(),
        WebviewUrl::default(),
    ))
    .build()
    .map_err(|e| EngineError::io(format!("create window {label}: {e}")))?;
    attach_handlers(app, &window);
    if let Some(path) = path {
        files::push_open(app, PathBuf::from(path), OpenSource::Dialog);
    }
    Ok(label)
}

/// Rust-side drag-drop and close bookkeeping. The webview gets the same drop event through
/// `onDragDropEvent`; this side keeps working before the frontend mounts.
pub fn attach_handlers(app: &AppHandle, window: &tauri::WebviewWindow) {
    let handle = app.clone();
    let label = window.label().to_string();
    window.on_window_event(move |event| match event {
        WindowEvent::DragDrop(DragDropEvent::Drop { paths, .. }) => {
            files::handle_drop(&handle, paths);
        }
        WindowEvent::Destroyed => {
            handle.state::<WindowDocs>().bind(&label, None);
        }
        // Stage 8: the native 편집 menu names the focused window's undo step.
        #[cfg(target_os = "macos")]
        WindowEvent::Focused(true) => {
            crate::app::menu::window_focused(&handle, &label);
        }
        _ => {}
    });
}
