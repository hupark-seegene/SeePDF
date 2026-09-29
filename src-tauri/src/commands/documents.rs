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
    // v0.3 integration (X2 × S5): open with the password of this open document — the print
    // path's n-up temp file is encrypted as its source is, and the webview never holds the
    // source's password.
    password_from: Option<String>,
) -> Result<DocInfo, EngineError> {
    let bytes = super::read_file(&path).await?;
    let path_buf = PathBuf::from(&path);
    engine
        .call(Lane::Edit, "open_document", move |st| {
            let password = match (password, password_from) {
                (None, Some(from)) => st.doc(&from)?.password.clone(),
                (password, _) => password,
            };
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

/// The open requests queued for the calling window (and only those), removed from the queue.
#[tauri::command]
pub fn take_pending_opens(
    window: tauri::WebviewWindow,
    pending: State<'_, PendingOpens>,
) -> Vec<OpenRequest> {
    pending.take(window.label())
}

#[tauri::command]
pub fn open_in_new_window(app: AppHandle, path: Option<String>) -> Result<String, EngineError> {
    crate::app::windows::open_in_new_window(&app, path)
}

/// `doc_id`: the window's active tab (the native menu names its undo step). `tabs` (v0.3 DR1):
/// every document open in the window's tabs, so H8 finds a file in a background tab; omitted,
/// the list announced before is kept.
#[tauri::command]
pub fn window_bind_document(
    app: AppHandle,
    windows: State<'_, WindowDocs>,
    label: String,
    doc_id: Option<String>,
    tabs: Option<Vec<String>>,
) -> Result<(), EngineError> {
    #[cfg(target_os = "macos")]
    crate::app::menu::document_bound(&app, &label, doc_id.as_deref());
    #[cfg(not(target_os = "macos"))]
    let _ = &app;
    windows.bind(&label, doc_id);
    windows.set_tabs(&label, tabs);
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
        .call(Lane::Edit, "redo", move |st| {
            registry::undo(st, &doc_id, true)
        })
        .await
}

/// v0.3 pkg3 (H8): the window already showing the file at `path`, brought to the front; `null`
/// when no window has it. `openPath` calls this first and opens nothing when another window
/// answers.
///
/// v0.3 DR1: a file in another window's background tab counts too — that window is sent
/// `focus-document` and brings the tab to the front. The calling window is not sent it: it
/// switches tabs itself — by the returned `docId` (v0.3.0), since its tab may spell the path
/// differently (`/tmp` vs `/private/tmp`, a symlinked folder, case on Windows).
#[tauri::command]
pub fn focus_document_window(
    app: AppHandle,
    window: tauri::WebviewWindow,
    path: String,
) -> Option<crate::app::windows::FocusedWindow> {
    crate::app::windows::focus_window_for_path(&app, &path, Some(window.label()))
}

// ---------------------------------------------------------------------------------------
// v0.3 pkg2-pages-structure-forms — 이미지로 PDF 만들기 (D1)
// ---------------------------------------------------------------------------------------

/// D1: a new untitled, dirty document with one page per PNG / JPEG in `paths` (in order).
///
/// The images are read and measured on a blocking worker (no PDFium there), one `progress`
/// event each, then one engine call builds the document; `cancel_job` with the `started`
/// event's id stops it between images (`cancelled`). `margin` is in points (default 0),
/// `fit` defaults to `contain`.
#[tauri::command]
pub async fn create_from_images(
    engine: State<'_, EngineHandle>,
    paths: Vec<String>,
    page_size: crate::ipc::types::ImagePageSize,
    margin: Option<f32>,
    fit: Option<crate::ipc::types::ImageFit>,
    on_progress: tauri::ipc::Channel<crate::ipc::types::JobEvent>,
) -> Result<DocInfo, EngineError> {
    use crate::engine::export::job::JobReporter;
    use crate::engine::pages::create;

    let options =
        create::ImagesToPdfOptions::new(page_size, margin.unwrap_or(0.0), fit.unwrap_or_default())?;
    if paths.is_empty() {
        return Err(EngineError::invalid("no image was given"));
    }
    if paths.len() > create::MAX_IMAGES {
        return Err(EngineError::invalid(format!(
            "at most {} images per document",
            create::MAX_IMAGES
        )));
    }
    let token = engine.jobs.create();
    // One unit per image, plus one for building the document.
    let reporter = JobReporter::start(
        on_progress,
        engine.jobs.clone(),
        token.id,
        paths.len() as u32 + 1,
    );
    let cancel = token.cancel.clone();
    let worker = reporter.clone();
    let prepared = tauri::async_runtime::spawn_blocking(move || {
        let mut out = Vec::with_capacity(paths.len());
        for path in &paths {
            if cancel.load(std::sync::atomic::Ordering::Relaxed) {
                return Err(EngineError::cancelled("create_from_images"));
            }
            out.push(create::prepare_image(std::path::Path::new(path))?);
            worker.step(None, None);
        }
        Ok(out)
    })
    .await
    .map_err(|e| EngineError::io(format!("read images: {e}")))
    .and_then(|r| r);
    let prepared = match prepared {
        Ok(p) => p,
        Err(e) => {
            if e.code == crate::ipc::ErrorCode::Cancelled {
                reporter.cancel();
            } else {
                reporter.fail(e.clone());
            }
            return Err(e);
        }
    };
    let built = engine
        .call(Lane::Edit, "create_from_images", move |st| {
            create::build(st, &prepared, &options)
        })
        .await;
    match &built {
        Ok(_) => {
            reporter.step(None, None);
            reporter.finish_if_complete();
        }
        Err(e) => reporter.fail(e.clone()),
    }
    built
}

/// D1: writes a clipboard image (PNG or JPEG bytes, sent as the raw request body) to a temp
/// file and returns its path — for 클립보드에서 새로 만들기 and pasting an image in 편집.
/// Anything that is not a PNG or JPEG is `unsupported`.
#[tauri::command]
pub async fn write_temp_image(request: tauri::ipc::Request<'_>) -> Result<String, EngineError> {
    let tauri::ipc::InvokeBody::Raw(bytes) = request.body() else {
        return Err(EngineError::invalid(
            "expected the image bytes as the request body",
        ));
    };
    let bytes = bytes.clone();
    tauri::async_runtime::spawn_blocking(move || write_temp_image_bytes(&bytes))
        .await
        .map_err(|e| EngineError::io(format!("write image: {e}")))?
        .map(|p| p.display().to_string())
}

/// v0.3 integration (D1): `clipboard_image_to_temp` — the system clipboard's image read
/// natively (no user gesture needed, unlike the webview's `navigator.clipboard.read`, which
/// WebKit refuses when the request comes from a native menu item) and written as a temp PNG.
/// `None` when the clipboard holds no image. Synchronous, so it runs on the main thread, where
/// AppKit wants the pasteboard read.
#[tauri::command]
pub fn clipboard_image_to_temp() -> Result<Option<String>, EngineError> {
    match crate::app::clipboard::read_image_png()? {
        Some(png) => write_temp_image_bytes(&png).map(|p| Some(p.display().to_string())),
        None => Ok(None),
    }
}

/// v0.3 integration (X8): `pdf_handler_is_self` — SeePDF is the default `.pdf` application
/// (Windows), so 인쇄 must not hand its copy to "the PDF app".
#[tauri::command]
pub fn pdf_handler_is_self() -> bool {
    crate::app::pdf_handler::pdf_handler_is_self()
}

/// [`write_temp_image`]'s file half: `$TMPDIR/seepdf-clipboard/<uuid>.png|jpg`.
pub fn write_temp_image_bytes(bytes: &[u8]) -> Result<PathBuf, EngineError> {
    let ext = if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        "png"
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        "jpg"
    } else {
        return Err(EngineError::new(
            crate::ipc::ErrorCode::Unsupported,
            "the clipboard image is not a PNG or JPEG",
        ));
    };
    let dir = std::env::temp_dir().join("seepdf-clipboard");
    std::fs::create_dir_all(&dir).map_err(EngineError::from)?;
    let path = dir.join(format!("{}.{ext}", uuid::Uuid::new_v4()));
    crate::engine::pages::write_atomic(&path, bytes)?;
    Ok(path)
}
