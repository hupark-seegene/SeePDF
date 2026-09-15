//! Stub engine thread: the single thread that owns pdfium.
//!
//! Requests arrive over a crossbeam channel; every request carries a [`Reply`] callback,
//! which is either an async oneshot (for `#[tauri::command] async fn`) or an arbitrary
//! `FnOnce` (the `seepdf://` protocol handler passes its `UriSchemeResponder` straight
//! through so the engine thread answers the webview directly, no hop back to the main
//! thread).

use crossbeam_channel::{unbounded, Receiver, Sender};
use parking_lot::RwLock;
use pdfium_render::prelude::*;
use std::collections::HashMap;
use std::io::Cursor;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

/// One-shot reply callback. `Reply::oneshot()` gives an awaitable receiver
/// (tokio mpsc with capacity 1, re-exported by `tauri::async_runtime`; tauri does not
/// re-export `tokio::sync::oneshot`, and `tokio` is not a direct dependency).
pub struct Reply<T>(Box<dyn FnOnce(T) + Send + 'static>);

impl<T: Send + 'static> Reply<T> {
    pub fn from_fn(f: impl FnOnce(T) + Send + 'static) -> Self {
        Self(Box::new(f))
    }

    pub fn oneshot() -> (Self, tauri::async_runtime::Receiver<T>) {
        let (tx, rx) = tauri::async_runtime::channel::<T>(1);
        (
            Self::from_fn(move |v| {
                // Receiver dropped => caller went away (e.g. webview reloaded); ignore.
                let _ = tx.try_send(v);
            }),
            rx,
        )
    }

    pub fn send(self, value: T) {
        (self.0)(value)
    }
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DocInfo {
    pub id: String,
    pub path: String,
    pub page_count: u16,
    pub open_ms: f64,
}

#[derive(Debug, Clone, Copy, serde::Deserialize)]
pub struct Tile {
    pub x: u32,
    pub y: u32,
    pub size: u32,
}

pub struct RenderedPng {
    pub png: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub render_ms: f64,
    pub encode_ms: f64,
}

pub enum EngineRequest {
    Ping {
        reply: Reply<String>,
    },
    OpenDocument {
        path: PathBuf,
        id: Option<String>,
        reply: Reply<Result<DocInfo, String>>,
    },
    RenderPagePng {
        doc: String,
        page: u16,
        scale: f32,
        tile: Option<Tile>,
        reply: Reply<Result<RenderedPng, String>>,
    },
    /// Blocks the engine thread; used to prove the UI/main thread are not blocked.
    Sleep {
        ms: u64,
        reply: Reply<()>,
    },
}

/// Cloneable handle stored in `tauri::State`.
#[derive(Clone)]
pub struct EngineHandle {
    tx: Sender<EngineRequest>,
    /// Directory the pdfium library was loaded from (for diagnostics).
    pub pdfium_dir: PathBuf,
    /// Open documents, mirrored from the engine thread so non-engine code (e.g. the
    /// `/raw` protocol route) can look up paths without a round trip.
    pub docs: Arc<RwLock<HashMap<String, DocInfo>>>,
}

impl EngineHandle {
    pub fn send(&self, req: EngineRequest) -> Result<(), String> {
        self.tx
            .send(req)
            .map_err(|_| "engine thread is gone".to_string())
    }

    pub async fn ping(&self) -> Result<String, String> {
        let (reply, mut rx) = Reply::oneshot();
        self.send(EngineRequest::Ping { reply })?;
        rx.recv().await.ok_or_else(|| "engine dropped reply".into())
    }

    pub async fn open_document(&self, path: PathBuf, id: Option<String>) -> Result<DocInfo, String> {
        let (reply, mut rx) = Reply::oneshot();
        self.send(EngineRequest::OpenDocument { path, id, reply })?;
        rx.recv().await.ok_or_else(|| "engine dropped reply".to_string())?
    }

    pub async fn render_page_png(
        &self,
        doc: String,
        page: u16,
        scale: f32,
        tile: Option<Tile>,
    ) -> Result<RenderedPng, String> {
        let (reply, mut rx) = Reply::oneshot();
        self.send(EngineRequest::RenderPagePng { doc, page, scale, tile, reply })?;
        rx.recv().await.ok_or_else(|| "engine dropped reply".to_string())?
    }

    pub async fn sleep(&self, ms: u64) -> Result<(), String> {
        let (reply, mut rx) = Reply::oneshot();
        self.send(EngineRequest::Sleep { ms, reply })?;
        rx.recv().await.ok_or_else(|| "engine dropped reply".to_string())
    }
}

/// Spawn the engine thread. Binding pdfium happens on that thread.
pub fn spawn_engine(pdfium_lib: PathBuf, pdfium_dir: PathBuf) -> EngineHandle {
    let (tx, rx) = unbounded::<EngineRequest>();
    let docs: Arc<RwLock<HashMap<String, DocInfo>>> = Arc::default();
    let docs_for_thread = docs.clone();
    std::thread::Builder::new()
        .name("seepdf-engine".into())
        .spawn(move || engine_main(pdfium_lib, rx, docs_for_thread))
        .expect("spawn engine thread");
    EngineHandle { tx, pdfium_dir, docs }
}

fn engine_main(lib: PathBuf, rx: Receiver<EngineRequest>, docs_index: Arc<RwLock<HashMap<String, DocInfo>>>) {
    let t0 = Instant::now();
    let bindings = match Pdfium::bind_to_library(&lib) {
        Ok(b) => b,
        Err(e) => {
            let msg = format!("failed to bind libpdfium at {}: {e:?}", lib.display());
            tracing::error!("{msg}");
            for req in rx {
                fail(req, &msg);
            }
            return;
        }
    };
    // Pdfium must outlive every PdfDocument<'a>; it lives for the whole process anyway
    // (pdfium-render 0.9 keeps the bindings in a global OnceCell), so leak it to get 'static.
    let pdfium: &'static Pdfium = Box::leak(Box::new(Pdfium::new(bindings)));
    // VERSION ships next to the library: MAJOR=155 / MINOR=0 / BUILD=8057 / PATCH=0 -> "155.0.8057.0"
    let version = std::fs::read_to_string(lib.with_file_name("VERSION"))
        .map(|s| s.lines().filter_map(|l| l.split_once('=')).map(|(_, v)| v.trim().to_string()).collect::<Vec<_>>().join("."))
        .unwrap_or_else(|_| "unknown".into());
    tracing::info!(
        lib = %lib.display(),
        pdfium_version = %version,
        bind_ms = t0.elapsed().as_secs_f64() * 1000.0,
        thread = ?std::thread::current().name(),
        "pdfium bound on engine thread"
    );

    let mut docs: HashMap<String, PdfDocument<'static>> = HashMap::new();
    let mut next_id: u32 = 1;

    for req in rx {
        match req {
            EngineRequest::Ping { reply } => reply.send(format!(
                "pong from {:?} (pdfium {version}, {} doc(s) open)",
                std::thread::current().name(),
                docs.len()
            )),
            EngineRequest::Sleep { ms, reply } => {
                std::thread::sleep(std::time::Duration::from_millis(ms));
                reply.send(());
            }
            EngineRequest::OpenDocument { path, id, reply } => {
                let t = Instant::now();
                let result = pdfium
                    .load_pdf_from_file(&path, None)
                    .map_err(|e| format!("open {}: {e:?}", path.display()))
                    .map(|doc| {
                        let id = id.unwrap_or_else(|| {
                            next_id += 1;
                            format!("doc{}", next_id - 1)
                        });
                        let info = DocInfo {
                            id: id.clone(),
                            path: path.display().to_string(),
                            page_count: doc.pages().len() as u16,
                            open_ms: t.elapsed().as_secs_f64() * 1000.0,
                        };
                        docs.insert(id.clone(), doc);
                        docs_index.write().insert(id, info.clone());
                        info
                    });
                match &result {
                    Ok(info) => tracing::info!(id = %info.id, pages = info.page_count, ms = info.open_ms, path = %info.path, "document opened"),
                    Err(e) => tracing::warn!("{e}"),
                }
                reply.send(result);
            }
            EngineRequest::RenderPagePng { doc, page, scale, tile, reply } => {
                let result = match docs.get(&doc) {
                    None => Err(format!("unknown document id '{doc}'")),
                    Some(d) => render_page_png(d, page, scale, tile),
                };
                if let Ok(r) = &result {
                    tracing::debug!(doc, page, scale, ?tile, w = r.width, h = r.height, bytes = r.png.len(), render_ms = r.render_ms, encode_ms = r.encode_ms, "rendered page png");
                }
                reply.send(result);
            }
        }
    }
    tracing::info!("engine thread exiting");
}

fn fail(req: EngineRequest, msg: &str) {
    match req {
        EngineRequest::Ping { reply } => reply.send(format!("ENGINE DOWN: {msg}")),
        EngineRequest::Sleep { reply, .. } => reply.send(()),
        EngineRequest::OpenDocument { reply, .. } => reply.send(Err(msg.to_string())),
        EngineRequest::RenderPagePng { reply, .. } => reply.send(Err(msg.to_string())),
    }
}

fn render_page_png(doc: &PdfDocument<'_>, page_index: u16, scale: f32, tile: Option<Tile>) -> Result<RenderedPng, String> {
    let t0 = Instant::now();
    let page = doc
        .pages()
        .get(page_index as PdfPageIndex)
        .map_err(|e| format!("page {page_index}: {e:?}"))?;
    let config = PdfRenderConfig::new()
        .scale_page_by_factor(scale.clamp(0.05, 16.0))
        .render_form_data(true)
        .set_clear_color(PdfColor::WHITE);
    let bitmap = page
        .render_with_config(&config)
        .map_err(|e| format!("render: {e:?}"))?;
    let mut img = bitmap.as_image().map_err(|e| format!("as_image: {e:?}"))?;
    if let Some(t) = tile {
        let (w, h) = (img.width(), img.height());
        let (x0, y0) = (t.x.saturating_mul(t.size), t.y.saturating_mul(t.size));
        if t.size == 0 || x0 >= w || y0 >= h {
            return Err(format!("tile ({},{}) size {} outside {w}x{h}", t.x, t.y, t.size));
        }
        img = img.crop_imm(x0, y0, t.size.min(w - x0), t.size.min(h - y0));
    }
    let render_ms = t0.elapsed().as_secs_f64() * 1000.0;

    let t1 = Instant::now();
    let mut buf = Cursor::new(Vec::with_capacity((img.width() * img.height()) as usize));
    let encoder = image::codecs::png::PngEncoder::new_with_quality(
        &mut buf,
        image::codecs::png::CompressionType::Fast,
        image::codecs::png::FilterType::Adaptive,
    );
    img.write_with_encoder(encoder)
        .map_err(|e| format!("png encode: {e}"))?;
    Ok(RenderedPng {
        width: img.width(),
        height: img.height(),
        png: buf.into_inner(),
        render_ms,
        encode_ms: t1.elapsed().as_secs_f64() * 1000.0,
    })
}
