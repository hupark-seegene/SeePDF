//! SeePDF — a lightweight, high-performance PDF editor.
//!
//! Layout:
//! * [`ipc`] — the wire contract (`IPC_CONTRACT.md`): types and the one error shape;
//! * [`engine`] — the single `seepdf-engine` thread that owns pdfium, every document and
//!   every page, plus the render, text and history machinery;
//! * [`protocol`] — the `seepdf://` custom scheme that carries every pixel;
//! * [`app`] — pdfium discovery, pending opens, windows, the native menu, settings/recents;
//! * [`commands`] — every `#[tauri::command]`, registered once in [`run`].

pub mod app;
pub mod commands;
pub mod engine;
pub mod ipc;
pub mod protocol;

use app::diagnostics::{self, StartupFailure};
use app::{PendingOpens, WindowDocs};
use ipc::types::Settings;
use ipc::{EngineError, ErrorCode};
use tauri::{Emitter, Manager};

/// Everything `setup` does, with a failure classified for the startup message box (v0.3
/// pkg5, H10): a missing / quarantined / unbindable libpdfium is `Library`, the rest `Other`.
fn start(app: &mut tauri::App, settings: &Settings) -> Result<(), StartupFailure> {
    let handle = app.handle().clone();

    // 1. locate libpdfium and start the engine thread (binding happens there).
    let (library, dir) = app::pdfium_path::resolve(&handle).map_err(StartupFailure::Library)?;
    tracing::info!(lib = %library.display(), "resolved bundled libpdfium");
    let engine = engine::spawn(
        library,
        dir,
        Some(handle.clone()),
        app::store::history_spill_dir(Some(&handle)),
        commands::app::tile_cache_bytes(settings.tile_cache_mb),
    )
    .map_err(|e| match e.code {
        ErrorCode::Pdfium => StartupFailure::Library(e.message),
        _ => StartupFailure::Other(e.message),
    })?;
    // v0.3 pkg6 (H5): the tile cache reports memory pressure; the viewer halves its tile
    // budgets while it is `high` (IPC_CONTRACT §8).
    {
        let handle = handle.clone();
        engine.shared.tiles.start_pressure_monitor(move |level| {
            let payload = ipc::types::EnginePressurePayload { level };
            if let Err(e) = handle.emit("engine-pressure", payload) {
                tracing::warn!("emit engine-pressure failed: {e}");
            }
        });
    }
    app.manage(engine);
    // v0.3 pkg6 (V4): read aloud announces each sentence it starts.
    {
        let handle = handle.clone();
        app.state::<std::sync::Arc<app::tts::Tts>>()
            .set_progress_listener(move |sentence_index| {
                let payload = ipc::types::TtsProgressPayload { sentence_index };
                if let Err(e) = handle.emit("tts-progress", payload) {
                    tracing::warn!("emit tts-progress failed: {e}");
                }
            });
    }

    // 2. native menu (macOS only). Windows has no menu bar at all: its 도움말 items (단축키,
    //    문제 보고, 로그 폴더 열기, SeePDF 정보) live in the title bar's ⋯ overflow menu.
    #[cfg(target_os = "macos")]
    {
        let menu = app::menu::build(&handle, settings.locale)
            .map_err(|e| StartupFailure::Other(format!("menu: {e}")))?;
        app.set_menu(menu)
            .map_err(|e| StartupFailure::Other(format!("set_menu: {e}")))?;
        app.on_menu_event(app::menu::on_menu_event);
    }

    // 3. window plumbing: drag-drop and the label -> document binding.
    for (_, window) in app.webview_windows() {
        app::windows::attach_handlers(&handle, &window);
    }

    // 4. v0.3 pkg5 (H14): a first-run (or restored) main window larger than the screen's work
    //    area — 1400×900 on a 1366×768 laptop at 125 % — is shrunk to 90 % of it and centred.
    if let Some(window) = app.get_webview_window("main") {
        fit_main_window(&window);
    }

    // 5. files passed on the command line (Windows double-click, `cargo run -- x.pdf`).
    for path in app::files::pdf_paths_from_argv() {
        app::files::push_open(&handle, path, ipc::types::OpenSource::Argv);
    }
    Ok(())
}

/// How much of the monitor's work area a window may take before it is shrunk (H14).
const WORK_AREA_FRACTION: f64 = 0.9;

/// v0.3 pkg5 (H14): `Some((position, size))` — the window shrunk to [`WORK_AREA_FRACTION`] of
/// the work area and centred in it — when `size` does not fit in that fraction; `None` when
/// it already fits. Physical pixels throughout; `work_pos` / `work_size` are the monitor's
/// work area (the screen minus the taskbar / Dock / menu bar).
pub fn fit_to_work_area(
    size: (u32, u32),
    work_pos: (i32, i32),
    work_size: (u32, u32),
) -> Option<((i32, i32), (u32, u32))> {
    let max_w = (work_size.0 as f64 * WORK_AREA_FRACTION).floor() as u32;
    let max_h = (work_size.1 as f64 * WORK_AREA_FRACTION).floor() as u32;
    if size.0 <= max_w && size.1 <= max_h {
        return None;
    }
    let w = size.0.min(max_w);
    let h = size.1.min(max_h);
    let x = work_pos.0 + ((work_size.0 - w) / 2) as i32;
    let y = work_pos.1 + ((work_size.1 - h) / 2) as i32;
    Some(((x, y), (w, h)))
}

/// Applies [`fit_to_work_area`] to the main window (after `tauri-plugin-window-state` restored
/// it). A maximised or full-screen window is left alone.
fn fit_main_window(window: &tauri::WebviewWindow) {
    if window.is_maximized().unwrap_or(false) || window.is_fullscreen().unwrap_or(false) {
        return;
    }
    let monitor = match window.current_monitor() {
        Ok(Some(m)) => Some(m),
        _ => window.primary_monitor().ok().flatten(),
    };
    let (Some(monitor), Ok(outer), Ok(inner)) = (monitor, window.outer_size(), window.inner_size())
    else {
        return;
    };
    let area = monitor.work_area();
    let Some(((x, y), (w, h))) = fit_to_work_area(
        (outer.width, outer.height),
        (area.position.x, area.position.y),
        (area.size.width, area.size.height),
    ) else {
        return;
    };
    // `set_size` is the inner size; keep the frame (Windows caption) out of the budget.
    let frame_w = outer.width.saturating_sub(inner.width);
    let frame_h = outer.height.saturating_sub(inner.height);
    let inner_size = tauri::PhysicalSize::new(w.saturating_sub(frame_w), h.saturating_sub(frame_h));
    tracing::info!(
        ?outer,
        w,
        h,
        "main window larger than the work area; fitted"
    );
    if let Err(e) = window.set_size(inner_size) {
        tracing::warn!("fit window size: {e}");
    }
    if let Err(e) = window.set_position(tauri::PhysicalPosition::new(x, y)) {
        tracing::warn!("fit window position: {e}");
    }
}

/// `seepdf --smoke <file.pdf>`: bind the *bundled* pdfium, open the file, render page 0 at
/// 2×, build the text layer, exit 0/1. Runs before any window is created, so CI can drive it
/// against the built bundle on every target (`ARCHITECTURE.md` §14).
fn run_smoke(path: &str) -> Result<(), EngineError> {
    let (library, dir) = app::pdfium_path::resolve_local().map_err(EngineError::io)?;
    let engine = engine::spawn(
        library,
        dir,
        None,
        std::env::temp_dir().join("seepdf-smoke"),
        engine::render::cache::DEFAULT_CAPACITY_BYTES,
    )?;
    let bytes = std::fs::read(path).map_err(|e| EngineError::io(format!("{path}: {e}")))?;
    let path_buf = std::path::PathBuf::from(path);
    let info = engine.call_blocking(engine::Lane::Edit, "smoke/open", move |st| {
        engine::registry::open(st, Some(path_buf), bytes, None)
    })?;
    let doc_id = info.doc_id.clone();
    let key = engine::render::TileKey {
        doc: doc_id.clone(),
        generation: info.doc_generation,
        page: 0,
        kind: engine::render::RenderKind::Page,
        scale_key: 200,
        rotation: 0,
        tx: 0,
        ty: 0,
        night: engine::render::Night::Off,
        hl: false,
        forms: true,
    };
    let (width, height) = engine.call_blocking(engine::Lane::Interactive, "smoke/render", {
        let key = key.clone();
        move |st| {
            let raw = engine::render::tiles::render(st, &engine::render::RenderRequest::new(key))?;
            Ok((raw.width, raw.height))
        }
    })?;
    let chars = engine.call_blocking(engine::Lane::Interactive, "smoke/text", move |st| {
        let doc = st.doc_mut(&doc_id)?;
        Ok(engine::text::layer::layer(doc, 0)?.chars.len())
    })?;
    println!(
        "smoke ok: {} pages, page 0 rendered {width}x{height} px at 2x, {chars} chars",
        info.page_count
    );
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // v0.3 pkg5 (H10): stderr + a daily rolling file in the app log directory, and every
    // panic logged before the default hook prints it.
    diagnostics::init_tracing(diagnostics::log_dir().as_deref());
    diagnostics::install_panic_hook();
    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        os = std::env::consts::OS,
        arch = std::env::consts::ARCH,
        "SeePDF starting"
    );

    if let Some(path) = smoke_argument() {
        match run_smoke(&path) {
            Ok(()) => std::process::exit(0),
            Err(e) => {
                eprintln!("smoke failed: {e}");
                std::process::exit(1);
            }
        }
    }

    let builder = tauri::Builder::default();
    // v0.3 pkg5 (H2): registered first, as the plugin requires. A second SeePDF process (an
    // Explorer double-click while SeePDF runs) exits at once and hands its argv to this one,
    // which opens the PDFs and comes to the front. macOS is single-instance already.
    #[cfg(not(target_os = "macos"))]
    let builder = builder.plugin(tauri_plugin_single_instance::init(|app, argv, cwd| {
        app::files::open_from_second_instance(app, argv, cwd);
    }));
    let builder = builder
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_store::Builder::new().build())
        .plugin(tauri_plugin_window_state::Builder::new().build())
        // 업데이트 확인 (v0.2.0): `check` / `download` / `install` from the frontend, then
        // `relaunch` from the process plugin. Endpoint and public key: `plugins.updater` in
        // tauri.conf.json; the release workflow signs the bundles.
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .manage(PendingOpens::default())
        .manage(WindowDocs::default())
        .manage(commands::recovery::RecoveryIds::default())
        // P2 읽어 주기: one system voice for the whole app, stopped on exit (below).
        .manage(std::sync::Arc::new(app::tts::Tts::system()))
        .setup(|app| {
            let settings = app::store::get_settings(app.handle());
            // v0.3 pkg5 (H10): a failure here used to propagate out of `build` into an
            // `expect` — the process vanished (no console on Windows). Now it says why.
            if let Err(failure) = start(app, &settings) {
                diagnostics::fail_startup(failure, settings.locale);
            }
            Ok(())
        });

    protocol::register(builder)
        .invoke_handler(tauri::generate_handler![
            // --- documents, view, history (Stage 0) ---
            commands::documents::open_document,
            commands::documents::close_document,
            commands::documents::get_document,
            commands::documents::get_outline,
            commands::documents::take_pending_opens,
            commands::documents::open_in_new_window,
            commands::documents::window_bind_document,
            commands::documents::set_viewport,
            commands::documents::render_page_raw,
            commands::documents::engine_stats,
            commands::documents::undo,
            commands::documents::redo,
            // --- text and search (Stage 0) ---
            commands::text::get_text_layer,
            commands::text::get_page_text,
            commands::text::search_start,
            commands::text::cancel_job,
            // --- app, recents, settings (Stage 0) ---
            commands::app::get_recent,
            commands::app::update_recent,
            commands::app::remove_recent,
            commands::app::set_recent_pinned,
            commands::app::clear_recent,
            commands::app::write_recent_thumbnail,
            commands::app::reveal_in_file_manager,
            commands::app::get_settings,
            commands::app::set_settings,
            commands::app::app_info,
            // --- typed signature → PNG (Stage 6b, P1-9) ---
            commands::app::write_signature_image,
            // --- annotations (Stage 1a) ---
            commands::annots::list_annotations,
            commands::annots::scan_annotations,
            commands::annots::create_annotation,
            commands::annots::update_annotation,
            commands::annots::delete_annotations,
            commands::annots::set_annotations_hidden,
            // --- annotation threads (P2) ---
            commands::annots::reply_annotation,
            // --- forms (Stage 1a) ---
            commands::forms::list_form_fields,
            commands::forms::set_form_field_value,
            commands::forms::reset_form,
            // --- redaction and security (Stage 1a / 1b / P1) ---
            commands::security::redact_preview,
            commands::security::apply_redactions,
            commands::security::apply_redactions_batch,
            commands::security::remove_password,
            commands::security::set_password,
            commands::security::remove_metadata,
            commands::security::set_metadata,
            // --- document structure: outline, links, page labels (P2) ---
            commands::structure::set_outline,
            commands::structure::create_link,
            commands::structure::update_link,
            commands::structure::delete_link,
            commands::structure::set_page_labels,
            commands::structure::get_page_labels,
            // --- stamps and compression (Stage 4) ---
            commands::stamp::add_stamp,
            commands::stamp::remove_stamps,
            commands::stamp::compress_estimate,
            commands::stamp::compress_apply,
            commands::stamp::compress_discard,
            // --- compare and autosave recovery (Stage 5) ---
            commands::compare::compare_documents,
            commands::recovery::write_recovery,
            commands::recovery::clear_recovery,
            commands::recovery::list_recovery,
            commands::recovery::discard_recovery,
            // --- pages (Stage 1b) ---
            commands::pages::page_ops,
            commands::pages::set_page_boxes,
            commands::pages::resize_pages,
            commands::pages::extract_pages,
            commands::pages::split_document,
            commands::pages::merge_documents,
            // --- page objects (Stage 1b) ---
            commands::objects::list_page_objects,
            commands::objects::probe_text_edit,
            commands::objects::edit_text_object,
            commands::objects::add_text_object,
            commands::objects::add_image_object,
            commands::objects::replace_image,
            commands::objects::transform_object,
            commands::objects::delete_objects,
            commands::objects::duplicate_objects,
            commands::objects::probe_paragraph,
            commands::objects::edit_paragraph,
            // --- save (Stage 1b) ---
            commands::save::save_document,
            commands::save::save_document_as,
            commands::save::path_exists,
            // --- 여러 파일에서 검색 › 폴더 추가 (P2) ---
            commands::save::list_pdf_files,
            // --- export and print (Stage 1b) ---
            commands::export::export_images,
            commands::export::export_text,
            commands::export::export_flattened,
            commands::export::estimate_export,
            commands::export::print_prepare,
            commands::export::export_annotation_summary,
            // --- read aloud (P2) ---
            commands::tts::tts_speak,
            commands::tts::tts_stop,
            commands::tts::tts_status,
            // --- v0.3 pkg6: web links, reading order, settings reset / cache, snapshot save ---
            commands::text::get_web_links,
            commands::text::get_reading_order,
            commands::app::get_default_settings,
            commands::app::clear_render_cache,
            commands::app::save_snapshot_png,
            // --- OCR (Stage 1b engine layer, 1f workers) ---
            commands::ocr::ocr_capabilities,
            commands::ocr::ocr_page_status,
            commands::ocr::ocr_apply,
            commands::ocr::ocr_recognize_native,
            // --- v0.3 pkg5-app-shell-release-diagnostics: 문제 보고, 로그, 라이선스 ---
            commands::app::problem_report,
            commands::app::open_log_folder,
            commands::app::third_party_notices,
        ])
        .build(tauri::generate_context!())
        .unwrap_or_else(|e| {
            // WebView2 missing, no window could be created, …
            diagnostics::fail_startup(
                StartupFailure::Other(e.to_string()),
                Settings::default().locale,
            )
        })
        .run(|_app, _event| {
            // P2 읽어 주기: the voice is a child process — never let it outlive the app.
            if let tauri::RunEvent::Exit = &_event {
                if let Some(tts) = _app.try_state::<std::sync::Arc<app::tts::Tts>>() {
                    tts.stop();
                }
            }
            // Finder double-click / `open -a SeePDF x.pdf` / Dock drop. Only fires for a
            // bundled .app whose Info.plist has CFBundleDocumentTypes.
            #[cfg(any(target_os = "macos", target_os = "ios"))]
            if let tauri::RunEvent::Opened { urls } = &_event {
                app::files::handle_opened_urls(_app, urls.clone());
            }
        });
}

/// `--smoke <file.pdf>`.
fn smoke_argument() -> Option<String> {
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--smoke" {
            return args.next();
        }
        if let Some(path) = arg.strip_prefix("--smoke=") {
            return Some(path.to_string());
        }
    }
    None
}
