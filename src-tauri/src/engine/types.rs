//! Engine command protocol and shared (non-pdfium) state — `ARCHITECTURE.md` §1.2.

use crate::engine::registry::OpenDoc;
use crate::engine::render::cache::TileCache;
use crate::engine::stats::Stats;
use crate::ipc::types::{DocId, PageGeom, Rotation};
use crate::ipc::EngineError;
use parking_lot::{Mutex, RwLock};
use pdfium_render::prelude::Pdfium;
use std::cmp::Ordering;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering as AtomicOrdering};
use std::sync::Arc;

/// Scheduling lanes, highest priority first. Within a lane commands are ordered by
/// `priority` then `seq` (FIFO); `Lane::Edit` is FIFO only, because edit order is
/// the user's order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u8)]
pub enum Lane {
    Interactive = 0,
    Edit = 1,
    Prefetch = 2,
    Thumb = 3,
    Background = 4,
}

impl Lane {
    pub const ALL: [Lane; 5] = [
        Lane::Interactive,
        Lane::Edit,
        Lane::Prefetch,
        Lane::Thumb,
        Lane::Background,
    ];

    pub fn index(self) -> usize {
        self as usize
    }

    /// Lanes whose commands may be dropped when the viewport has moved on.
    pub fn is_render_lane(self) -> bool {
        matches!(self, Lane::Interactive | Lane::Prefetch | Lane::Thumb)
    }
}

/// How the engine loop decided to dispatch a command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CmdStatus {
    /// Run normally.
    Run,
    /// The viewport moved on; reply `stale` without touching pdfium.
    Stale,
    /// The job token was cancelled; reply `cancelled`.
    Cancelled,
}

/// One unit of engine work. `run` is called exactly once, on the engine thread, with the
/// status the scheduler decided — so a dropped command still answers its caller (a dropped
/// `UriSchemeResponder` hangs the `<img>`).
pub struct Cmd {
    pub lane: Lane,
    /// Within a lane: tile distance from the viewport centre, or `|page - centre|`.
    pub priority: u32,
    /// Submission order; FIFO tie-break.
    pub seq: u64,
    /// `EngineShared::viewport_gen` at submit time (renders only).
    pub viewport_gen: u64,
    /// Page this command renders, used by the stale check.
    pub page: Option<u16>,
    /// For tracing.
    pub label: &'static str,
    /// Shared job cancellation flag.
    pub cancel: Option<Arc<AtomicBool>>,
    #[allow(clippy::type_complexity)]
    pub run: Box<dyn FnOnce(&mut EngineState<'_>, CmdStatus) + Send + 'static>,
}

impl Cmd {
    pub fn is_cancelled(&self) -> bool {
        self.cancel
            .as_ref()
            .map(|c| c.load(AtomicOrdering::Relaxed))
            .unwrap_or(false)
    }
}

impl PartialEq for Cmd {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for Cmd {}

impl PartialOrd for Cmd {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Cmd {
    /// Reversed so that `BinaryHeap` (a max-heap) pops the *lowest* (lane, priority, seq).
    fn cmp(&self, other: &Self) -> Ordering {
        (other.lane, other.priority, other.seq).cmp(&(self.lane, self.priority, self.seq))
    }
}

/// One-shot reply callback.
///
/// `Reply::oneshot()` gives an awaitable receiver built on `tauri::async_runtime::channel(1)`
/// — tauri re-exports tokio's mpsc but not `oneshot` (tauri spike §3). The `seepdf://`
/// handler instead moves its `UriSchemeResponder` (which is `Send`) straight into a
/// `Reply::from_fn`, so the encode pool answers the webview with no hop back.
pub struct Reply<T>(Box<dyn FnOnce(T) + Send + 'static>);

impl<T: Send + 'static> Reply<T> {
    pub fn from_fn(f: impl FnOnce(T) + Send + 'static) -> Self {
        Self(Box::new(f))
    }

    pub fn oneshot() -> (Self, tauri::async_runtime::Receiver<T>) {
        let (tx, rx) = tauri::async_runtime::channel::<T>(1);
        (
            // Receiver dropped => the caller went away (webview reload); ignore.
            Self::from_fn(move |v| {
                let _ = tx.try_send(v);
            }),
            rx,
        )
    }

    /// Blocking variant for tests and for code that is not inside the async runtime.
    pub fn blocking() -> (Self, std::sync::mpsc::Receiver<T>) {
        let (tx, rx) = std::sync::mpsc::channel::<T>();
        (
            Self::from_fn(move |v| {
                let _ = tx.send(v);
            }),
            rx,
        )
    }

    pub fn send(self, value: T) {
        (self.0)(value)
    }
}

/// What the frontend last told us it is looking at.
#[derive(Debug, Clone, Default)]
pub struct Viewport {
    pub doc_id: DocId,
    pub scale_key: u32,
    pub rotation: Rotation,
    pub centre_page: u16,
    pub first_page: u16,
    pub last_page: u16,
    pub velocity_px_per_ms: f32,
}

impl Viewport {
    /// Renders for pages inside `visible ± 1` are never dropped as stale.
    pub fn keeps(&self, page: u16) -> bool {
        page + 1 >= self.first_page && page <= self.last_page.saturating_add(1)
    }
}

/// Mirror of an open document for code that must not touch pdfium (the protocol handler
/// checks 404/410 against this on the webview thread).
#[derive(Debug, Clone)]
pub struct DocSummary {
    pub doc_id: DocId,
    pub generation: u32,
    pub page_count: u16,
    pub path: Option<PathBuf>,
    pub pages: Arc<Vec<PageGeom>>,
}

/// Everything shared between the engine thread, the command threads and the protocol
/// handler. Contains no pdfium handle of any kind.
pub struct EngineShared {
    seq: AtomicU64,
    viewport_gen: AtomicU64,
    pub viewport: Mutex<Viewport>,
    pub docs: RwLock<HashMap<DocId, DocSummary>>,
    pub tiles: Arc<TileCache>,
    pub stats: Arc<Stats>,
    /// Directory the pdfium library was loaded from (diagnostics, `app_info`).
    pub pdfium_dir: PathBuf,
    pub pdfium_version: RwLock<String>,
}

impl EngineShared {
    pub fn new(pdfium_dir: PathBuf, tile_cache_bytes: usize) -> Self {
        Self {
            seq: AtomicU64::new(0),
            viewport_gen: AtomicU64::new(0),
            viewport: Mutex::new(Viewport::default()),
            docs: RwLock::new(HashMap::new()),
            tiles: Arc::new(TileCache::new(tile_cache_bytes)),
            stats: Arc::new(Stats::default()),
            pdfium_dir,
            pdfium_version: RwLock::new("unknown".into()),
        }
    }

    pub fn next_seq(&self) -> u64 {
        self.seq.fetch_add(1, AtomicOrdering::Relaxed)
    }

    pub fn viewport_gen(&self) -> u64 {
        self.viewport_gen.load(AtomicOrdering::Relaxed)
    }

    pub fn bump_viewport_gen(&self) -> u64 {
        self.viewport_gen.fetch_add(1, AtomicOrdering::Relaxed) + 1
    }

    pub fn doc(&self, doc_id: &str) -> Option<DocSummary> {
        self.docs.read().get(doc_id).cloned()
    }
}

/// The engine thread's own state: `Pdfium` plus every open document. Never leaves the
/// engine thread (pdfium types are `!Send` because `thread_safe` is off).
pub struct EngineState<'p> {
    pub pdfium: &'p Pdfium,
    /// Field order matters only inside `OpenDoc`; documents themselves are independent.
    pub docs: HashMap<DocId, OpenDoc<'p>>,
    pub shared: Arc<EngineShared>,
    pub app: Option<tauri::AppHandle>,
    pub encode: crate::engine::render::encode::EncodePool,
    pub next_doc_id: u32,
    /// `$TEMP/seepdf-history/` — undo snapshots spill here.
    pub spill_dir: PathBuf,
    /// RAM budget for undo snapshots across all documents (256 MiB by default).
    pub history_budget: usize,
}

impl<'p> EngineState<'p> {
    pub fn doc(&self, doc_id: &str) -> Result<&OpenDoc<'p>, EngineError> {
        self.docs
            .get(doc_id)
            .ok_or_else(|| EngineError::not_found(format!("unknown document '{doc_id}'")))
    }

    pub fn doc_mut(&mut self, doc_id: &str) -> Result<&mut OpenDoc<'p>, EngineError> {
        self.docs
            .get_mut(doc_id)
            .ok_or_else(|| EngineError::not_found(format!("unknown document '{doc_id}'")))
    }

    pub fn alloc_doc_id(&mut self) -> DocId {
        self.next_doc_id += 1;
        format!("d{}", self.next_doc_id)
    }
}
