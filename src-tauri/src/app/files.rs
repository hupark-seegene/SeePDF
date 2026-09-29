//! Opening files from outside the webview: argv, Finder / `open -a`, Dock drops and
//! window drag-and-drop (tauri spike §4).
//!
//! A path can arrive **before** the webview has registered its listeners, so every open
//! request is queued **for one window** and that window is sent `open-file`; the window drains
//! its own queue (`take_pending_opens`) on mount and on every `open-file`, so a request is
//! opened exactly once, by exactly one window, whichever comes first.
//!
//! v0.3 integration (H2 × one document per window): an open request is never broadcast — every
//! window used to act on it, each replacing its own document. A file the OS hands over
//! (argv, Finder / Explorer, the Windows single-instance hand-off) goes to a window that shows
//! nothing yet, else to a new window; a file already shown somewhere brings that window to the
//! front (H8). A drop goes to the window it was dropped on, 새 창에서 열기 to the new window.

use crate::ipc::types::{OpenFilePayload, OpenRequest, OpenSource};
use parking_lot::Mutex;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Emitter, Manager};

/// Open requests not yet taken, each with the label of the window it is for.
#[derive(Default)]
pub struct PendingOpens(pub Mutex<Vec<(String, OpenRequest)>>);

impl PendingOpens {
    /// The requests queued for the window `label`, removed from the queue.
    pub fn take(&self, label: &str) -> Vec<OpenRequest> {
        let mut queue = self.0.lock();
        let (mine, rest): (Vec<_>, Vec<_>) = std::mem::take(&mut *queue)
            .into_iter()
            .partition(|(l, _)| l == label);
        *queue = rest;
        mine.into_iter().map(|(_, r)| r).collect()
    }

    pub fn push(&self, label: &str, request: OpenRequest) {
        self.0.lock().push((label.to_string(), request));
    }

    /// Windows with a request waiting.
    pub fn labels(&self) -> HashSet<String> {
        self.0.lock().iter().map(|(l, _)| l.clone()).collect()
    }

    /// A window closed before it took its requests: they go with it.
    pub fn forget(&self, label: &str) {
        self.0.lock().retain(|(l, _)| l != label);
    }
}

/// The window an OS hand-off opens in: the first of `candidates` (in preference order) that
/// shows no document and has no open request waiting. `None`: open a new window. Pure, so it
/// is unit-testable.
pub fn pick_idle_window(
    candidates: &[String],
    bound: &HashSet<String>,
    pending: &HashSet<String>,
) -> Option<String> {
    candidates
        .iter()
        .find(|l| !bound.contains(*l) && !pending.contains(*l))
        .cloned()
}

/// v0.3 DR1: an idle window first; with 설정 › 파일 열기 = 새 탭 (`into_tab`) every window
/// takes files, so the preferred one (focused, then `main`) opens it in a new tab; `None`: a new
/// window.
pub fn pick_open_window(
    candidates: &[String],
    bound: &HashSet<String>,
    pending: &HashSet<String>,
    into_tab: bool,
) -> Option<String> {
    pick_idle_window(candidates, bound, pending).or_else(|| {
        if into_tab {
            candidates.first().cloned()
        } else {
            None
        }
    })
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
    let mut last = None;
    for path in paths {
        last = open_from_os(app, path, OpenSource::Argv).or(last);
    }
    let window = last
        .and_then(|label| app.get_webview_window(&label))
        .or_else(|| app.get_webview_window("main"))
        .or_else(|| app.webview_windows().into_values().next());
    if let Some(window) = window {
        if window.is_minimized().unwrap_or(false) {
            let _ = window.unminimize();
        }
        let _ = window.show();
        let _ = window.set_focus();
    }
}

/// A file the OS handed over (argv, Finder / Explorer, a second instance): the window that
/// already shows it is brought to the front (H8); otherwise it opens in a window that shows
/// nothing yet — the focused one, then `main`, then any — or, when every window has a document,
/// in a new window. Returns the label of the window it went to.
pub fn open_from_os(app: &AppHandle, path: PathBuf, source: OpenSource) -> Option<String> {
    let text = path.display().to_string();
    if let Some(label) = crate::app::windows::focus_window_for_path(app, &text, None) {
        tracing::info!(path = %text, %label, "already open; focused");
        return Some(label);
    }
    let mut candidates: Vec<(u8, String)> = app
        .webview_windows()
        .into_iter()
        .map(|(label, window)| {
            let rank = if window.is_focused().unwrap_or(false) {
                0
            } else if label == "main" {
                1
            } else {
                2
            };
            (rank, label)
        })
        .collect();
    candidates.sort();
    let candidates: Vec<String> = candidates.into_iter().map(|(_, l)| l).collect();
    let bound: HashSet<String> = app
        .state::<crate::app::WindowDocs>()
        .bindings()
        .into_iter()
        .map(|(label, _)| label)
        .collect();
    let pending = app.state::<PendingOpens>().labels();
    let into_tab =
        crate::app::store::get_settings(app).open_files_in == crate::ipc::types::OpenFilesIn::Tab;
    match pick_open_window(&candidates, &bound, &pending, into_tab) {
        Some(label) => {
            push_open(app, &label, path, source);
            Some(label)
        }
        None => match crate::app::windows::open_window_with(app, path, source) {
            Ok(label) => Some(label),
            Err(e) => {
                tracing::warn!(path = %text, error = %e.message, "no window for an OS open");
                None
            }
        },
    }
}

/// Queues `path` for the window `label` and tells that window (only) to take it. The queue
/// covers a window that has not mounted its listener yet.
pub fn push_open(app: &AppHandle, label: &str, path: PathBuf, source: OpenSource) {
    let path = path.display().to_string();
    tracing::info!(%path, ?source, %label, "open request");
    app.state::<PendingOpens>().push(
        label,
        OpenRequest {
            path: path.clone(),
            source,
        },
    );
    if let Err(e) = app.emit_to(label, "open-file", OpenFilePayload { path, source }) {
        tracing::warn!("emit open-file failed: {e}");
    }
}

/// macOS `RunEvent::Opened { urls }`: Finder double-click, `open -a SeePDF x.pdf`, Dock drop.
/// Only fires for a bundled `.app` whose `Info.plist` has `CFBundleDocumentTypes`, which
/// `bundle.fileAssociations` generates.
pub fn handle_opened_urls(app: &AppHandle, urls: Vec<tauri::Url>) {
    for url in urls {
        match url.to_file_path() {
            Ok(path) => {
                open_from_os(app, path, OpenSource::MacosOpened);
            }
            Err(()) => tracing::warn!(%url, "RunEvent::Opened with a non-file url"),
        }
    }
}

/// Window drag-and-drop onto the window `label`. The webview gets the same event via
/// `onDragDropEvent`; this side exists so a drop still opens the file when the frontend has
/// not mounted yet.
pub fn handle_drop(app: &AppHandle, label: &str, paths: &[PathBuf]) {
    for path in paths.iter().filter(|p| is_pdf(p)) {
        push_open(app, label, path.clone(), OpenSource::Drop);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(labels: &[&str]) -> HashSet<String> {
        labels.iter().map(|l| l.to_string()).collect()
    }

    #[test]
    fn an_os_open_goes_to_an_idle_window_or_a_new_one() {
        let candidates: Vec<String> = ["doc-2", "main", "doc-1"].map(String::from).to_vec();
        // The focused window first, when it shows nothing.
        assert_eq!(
            pick_idle_window(&candidates, &set(&[]), &set(&[])).as_deref(),
            Some("doc-2")
        );
        // Windows with a document, or with a request already waiting, are skipped.
        assert_eq!(
            pick_idle_window(&candidates, &set(&["doc-2"]), &set(&["main"])).as_deref(),
            Some("doc-1")
        );
        // Every window busy: a new window.
        assert_eq!(
            pick_idle_window(&candidates, &set(&["doc-2", "main"]), &set(&["doc-1"])),
            None
        );
    }

    /// v0.3 DR1: with 새 탭, a file the OS hands over while every window has a document goes to
    /// the preferred window's tabs instead of a new window; an idle window still comes first.
    #[test]
    fn open_files_in_a_tab_of_the_preferred_window() {
        let candidates = vec!["doc-1".to_string(), "main".to_string()];
        let bound = set(&["doc-1", "main"]);
        let none = HashSet::new();
        assert_eq!(pick_open_window(&candidates, &bound, &none, false), None);
        assert_eq!(
            pick_open_window(&candidates, &bound, &none, true).as_deref(),
            Some("doc-1")
        );
        let bound = set(&["doc-1"]);
        assert_eq!(
            pick_open_window(&candidates, &bound, &none, true).as_deref(),
            Some("main")
        );
    }

    #[test]
    fn a_window_takes_only_its_own_requests() {
        let pending = PendingOpens::default();
        let req = |p: &str| OpenRequest {
            path: p.into(),
            source: OpenSource::Argv,
        };
        pending.push("main", req("/a.pdf"));
        pending.push("doc-1", req("/b.pdf"));
        pending.push("main", req("/c.pdf"));
        assert_eq!(pending.labels(), set(&["main", "doc-1"]));
        let main: Vec<String> = pending.take("main").into_iter().map(|r| r.path).collect();
        assert_eq!(main, ["/a.pdf", "/c.pdf"]);
        assert!(pending.take("main").is_empty(), "taken once");
        pending.forget("doc-1");
        assert!(pending.take("doc-1").is_empty());
    }
}
