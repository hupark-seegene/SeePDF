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

use app::{PendingOpens, WindowDocs};
use ipc::EngineError;
use tauri::Manager;

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
    init_tracing();

    if let Some(path) = smoke_argument() {
        match run_smoke(&path) {
            Ok(()) => std::process::exit(0),
            Err(e) => {
                eprintln!("smoke failed: {e}");
                std::process::exit(1);
            }
        }
    }

    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_store::Builder::new().build())
        .plugin(tauri_plugin_window_state::Builder::new().build())
        .manage(PendingOpens::default())
        .manage(WindowDocs::default())
        .setup(|app| {
            let handle = app.handle().clone();

            // 1. locate libpdfium and start the engine thread (binding happens there).
            let (library, dir) = app::pdfium_path::resolve(&handle)?;
            tracing::info!(lib = %library.display(), "resolved bundled libpdfium");
            let settings = app::store::get_settings(&handle);
            let engine = engine::spawn(
                library,
                dir,
                Some(handle.clone()),
                app::store::history_spill_dir(Some(&handle)),
                (settings.tile_cache_mb.clamp(16, 1024) as usize) * 1024 * 1024,
            )?;
            app.manage(engine);

            // 2. native menu (macOS only; Windows uses the in-window menu bar).
            #[cfg(target_os = "macos")]
            {
                let menu = app::menu::build(&handle)?;
                app.set_menu(menu)?;
                app.on_menu_event(app::menu::on_menu_event);
            }

            // 3. window plumbing: drag-drop and the label -> document binding.
            for (_, window) in app.webview_windows() {
                app::windows::attach_handlers(&handle, &window);
            }

            // 4. files passed on the command line (Windows double-click, `cargo run -- x.pdf`).
            for path in app::files::pdf_paths_from_argv() {
                app::files::push_open(&handle, path, ipc::types::OpenSource::Argv);
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
            // --- annotations (Stage 1a) ---
            commands::annots::list_annotations,
            commands::annots::scan_annotations,
            commands::annots::create_annotation,
            commands::annots::update_annotation,
            commands::annots::delete_annotations,
            commands::annots::set_annotations_hidden,
            // --- forms (Stage 1a) ---
            commands::forms::list_form_fields,
            commands::forms::set_form_field_value,
            commands::forms::reset_form,
            // --- redaction and security (Stage 1a / 1b / P1) ---
            commands::security::redact_preview,
            commands::security::apply_redactions,
            commands::security::remove_password,
            commands::security::set_password,
            commands::security::remove_metadata,
            commands::security::set_metadata,
            // --- pages (Stage 1b) ---
            commands::pages::page_ops,
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
            // --- save (Stage 1b) ---
            commands::save::save_document,
            commands::save::save_document_as,
            // --- export and print (Stage 1b) ---
            commands::export::export_images,
            commands::export::export_text,
            commands::export::export_flattened,
            commands::export::estimate_export,
            commands::export::print_prepare,
            // --- OCR (Stage 1b engine layer, 1f workers) ---
            commands::ocr::ocr_capabilities,
            commands::ocr::ocr_page_status,
            commands::ocr::ocr_apply,
            commands::ocr::ocr_recognize_native,
        ])
        .build(tauri::generate_context!())
        .expect("error while building the tauri application")
        .run(|_app, _event| {
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
