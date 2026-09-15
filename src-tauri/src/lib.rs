//! SeePDF Tauri backend. The `spike` module is throw-away integration proof code
//! (see docs/spikes/tauri.md); the patterns in it are the ones to keep.

mod spike;

use spike::engine::{EngineHandle, Reply};
use spike::files::PendingOpens;
use spike::jobs::Jobs;
use tauri::{DragDropEvent, Manager, WindowEvent};

fn init_tracing() {
    use tracing_subscriber::EnvFilter;
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,seepdf_lib=debug,webview=info"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_thread_names(true)
        .with_target(true)
        .try_init();
}

// ---- spike commands that need the engine ----

#[tauri::command]
async fn engine_ping(engine: tauri::State<'_, EngineHandle>) -> Result<String, String> {
    engine.ping().await
}

#[tauri::command]
async fn open_document(
    engine: tauri::State<'_, EngineHandle>,
    path: String,
) -> Result<spike::engine::DocInfo, String> {
    engine.open_document(std::path::PathBuf::from(path), None).await
}

#[tauri::command]
fn list_documents(engine: tauri::State<'_, EngineHandle>) -> Vec<spike::engine::DocInfo> {
    engine.docs.read().values().cloned().collect()
}

/// Same render as `seepdf://localhost/page`, but returned through binary IPC so the two
/// transports can be compared for page pixels.
#[tauri::command]
async fn render_page_png_ipc(
    engine: tauri::State<'_, EngineHandle>,
    doc: String,
    page: u16,
    scale: f32,
) -> Result<tauri::ipc::Response, String> {
    let png = engine.render_page_png(doc, page, scale, None).await?;
    Ok(tauri::ipc::Response::new(png.png))
}

#[tauri::command]
async fn engine_sleep(engine: tauri::State<'_, EngineHandle>, ms: u64) -> Result<(), String> {
    engine.sleep(ms).await
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct SpikeInfo {
    pdfium_dir: String,
    resource_dir: Option<String>,
    argv: Vec<String>,
    os: &'static str,
    arch: &'static str,
    debug: bool,
}

#[tauri::command]
fn spike_info(app: tauri::AppHandle, engine: tauri::State<'_, EngineHandle>) -> SpikeInfo {
    SpikeInfo {
        pdfium_dir: engine.pdfium_dir.display().to_string(),
        resource_dir: app.path().resource_dir().ok().map(|p| p.display().to_string()),
        argv: std::env::args().collect(),
        os: std::env::consts::OS,
        arch: std::env::consts::ARCH,
        debug: cfg!(debug_assertions),
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    init_tracing();

    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_store::Builder::new().build())
        .plugin(tauri_plugin_window_state::Builder::new().build())
        .manage(Jobs::default())
        .manage(PendingOpens::default())
        .setup(|app| {
            let handle = app.handle().clone();

            // 1. locate + load libpdfium on the engine thread
            let (lib, dir) = spike::pdfium_path::resolve(&handle)?;
            tracing::info!(lib = %lib.display(), "resolved bundled libpdfium");
            let engine = spike::engine::spawn_engine(lib, dir);
            app.manage(engine.clone());

            // 2. startup smoke test: page count of fixtures/tracemonkey.pdf, logged
            let fixture = spike::pdfium_path::fixtures_dir().join("tracemonkey.pdf");
            if fixture.is_file() {
                let _ = engine.send(spike::engine::EngineRequest::OpenDocument {
                    path: fixture,
                    id: Some("tracemonkey".into()),
                    reply: Reply::from_fn(|r: Result<spike::engine::DocInfo, String>| match r {
                        Ok(info) => tracing::info!(pages = info.page_count, ms = info.open_ms, "startup smoke test: tracemonkey.pdf opened via pdfium"),
                        Err(e) => tracing::error!("startup smoke test failed: {e}"),
                    }),
                });
            } else {
                tracing::warn!(path = %fixture.display(), "fixture missing; skipping smoke test");
            }

            // 3. files passed on the command line (Windows double-click / cargo run -- x.pdf)
            for p in spike::files::pdf_paths_from_argv() {
                spike::files::push_open(&handle, p, "argv");
            }

            // 4. Rust-side view of drag-and-drop (JS gets the same via onDragDropEvent)
            if let Some(win) = app.get_webview_window("main") {
                win.on_window_event(|ev| {
                    if let WindowEvent::DragDrop(DragDropEvent::Drop { paths, position }) = ev {
                        tracing::info!(?paths, ?position, "window drag-drop (rust side)");
                    }
                });
            }
            Ok(())
        });

    let builder = spike::protocol::register(builder);

    builder
        .invoke_handler(tauri::generate_handler![
            engine_ping,
            open_document,
            list_documents,
            render_page_png_ipc,
            engine_sleep,
            spike_info,
            spike::ipc::ipc_get_bytes,
            spike::ipc::ipc_put_bytes,
            spike::ipc::ipc_get_json_array,
            spike::ipc::ipc_put_json_array,
            spike::ipc::ipc_get_base64,
            spike::ipc::ipc_put_base64,
            spike::ipc::spike_log,
            spike::jobs::start_job,
            spike::jobs::cancel_job,
            spike::files::take_pending_opens,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app, event| {
            #[cfg(any(target_os = "macos", target_os = "ios"))]
            if let tauri::RunEvent::Opened { urls } = &event {
                spike::files::handle_opened_urls(app, urls.clone());
            }
            #[cfg(not(any(target_os = "macos", target_os = "ios")))]
            let _ = (app, event);
        });
}
