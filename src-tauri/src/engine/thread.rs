//! The `seepdf-engine` thread and the handle every other thread talks to.
//!
//! `ARCHITECTURE.md` §1.1–1.2. One thread owns `Pdfium`, every `PdfDocument` and every
//! `PdfPage`. `Pdfium` lives on that thread's **stack**, so documents borrow it with a local
//! lifetime — no `Box::leak`, no `'static`. Because the `thread_safe` feature is off, those
//! types are `!Send` and the compiler enforces the rule.
//!
//! Loop:
//! ```text
//! recv() one Cmd -> drain try_recv() into per-lane heaps -> pop by (lane, priority, seq)
//!   drop renders whose viewport_gen + 1 < shared.viewport_gen and whose page is outside visible±1
//!   drop cancelled jobs
//!   run one command
//! starvation guard: after 8 consecutive Interactive/Prefetch pops, take one Edit/Background
//! ```
//!
//! **Panics** (v0.3 pkg5, H4). A Rust panic inside one command no longer takes the process
//! down (`panic = "unwind"` in the release profile). The command's caller gets
//! `engineCrashed`; every document the command touched (`EngineState::doc` / `doc_mut`
//! record it through [`touch`]) is closed, because its pdfium state may be half-edited; the
//! `engine-crashed` event names them so their windows reopen the file (or its autosave copy);
//! and the thread carries on serving the other documents. The panic itself is logged by the
//! hook `app::diagnostics` installs. A crash **inside PDFium's C++** is not a Rust panic and
//! still ends the process — out of scope.

use crate::engine::jobs::Jobs;
use crate::engine::render::encode::EncodePool;
use crate::engine::types::{Cmd, CmdStatus, EngineShared, EngineState, Lane, Reply};
use crate::ipc::types::DocId;
use crate::ipc::{EngineError, ErrorCode};
use crossbeam_channel::{unbounded, Receiver, Sender};
use pdfium_render::prelude::Pdfium;
use std::cell::RefCell;
use std::collections::{BinaryHeap, HashMap};
use std::panic::{catch_unwind, resume_unwind, AssertUnwindSafe};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Instant;

thread_local! {
    /// The documents the running command looked up, in order, without repeats.
    static TOUCHED: RefCell<Vec<DocId>> = const { RefCell::new(Vec::new()) };
}

/// Records that the running engine command works on `doc_id` (called by
/// `EngineState::doc` / `doc_mut`; cheap — one comparison when it is the same document).
pub fn touch(doc_id: &str) {
    TOUCHED.with(|t| {
        let mut t = t.borrow_mut();
        if !t.iter().any(|d| d == doc_id) {
            t.push(doc_id.to_string());
        }
    });
}

fn take_touched() -> Vec<DocId> {
    TOUCHED.with(|t| std::mem::take(&mut *t.borrow_mut()))
}

/// The error a caller receives when its command panicked.
fn crashed(label: &str, payload: &(dyn std::any::Any + Send)) -> EngineError {
    EngineError::new(
        ErrorCode::EngineCrashed,
        format!(
            "{label} panicked: {}",
            crate::app::diagnostics::panic_message(payload)
        ),
    )
}

/// `engine-crashed`: the documents a panicking command left closed.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EngineCrashedEvent {
    pub doc_ids: Vec<DocId>,
    pub label: String,
}

/// After a panic: close every document the command touched and tell the frontend.
fn poison(st: &mut EngineState<'_>, label: &str, touched: Vec<DocId>) {
    let mut closed = Vec::new();
    for doc_id in touched {
        if !st.docs.contains_key(&doc_id) {
            continue;
        }
        // Closing runs pdfium on a document that may be half-edited; a second panic here must
        // not escape the loop either.
        let result = catch_unwind(AssertUnwindSafe(|| {
            crate::engine::registry::close(st, &doc_id)
        }));
        if !matches!(result, Ok(Ok(()))) {
            // Forget it without pdfium: leaking a handle beats crashing.
            if let Some(doc) = st.docs.remove(&doc_id) {
                std::mem::forget(doc);
            }
            st.shared.docs.write().remove(&doc_id);
            st.shared.tiles.drop_document(&doc_id);
        }
        closed.push(doc_id);
    }
    tracing::error!(label, docs = ?closed, "engine command panicked; documents closed");
    if let Some(app) = &st.app {
        use tauri::Emitter;
        let event = EngineCrashedEvent {
            doc_ids: closed,
            label: label.to_string(),
        };
        if let Err(e) = app.emit("engine-crashed", event) {
            tracing::warn!("emit engine-crashed failed: {e}");
        }
    }
}

/// Renders are refused with `busy` past this many queued commands.
const MAX_QUEUED: u32 = 1024;
/// After this many consecutive fast-lane pops, one Edit/Background command gets a turn.
const STARVATION_GUARD: u32 = 8;

/// Cloneable handle stored in `tauri::State`. Never contains a pdfium handle.
#[derive(Clone)]
pub struct EngineHandle {
    tx: Sender<Cmd>,
    queued: Arc<AtomicU32>,
    pub shared: Arc<EngineShared>,
    pub jobs: Arc<Jobs>,
}

/// Options for one submission. Defaults are "an ordinary command, never dropped".
pub struct Submit {
    pub lane: Lane,
    pub label: &'static str,
    pub priority: u32,
    pub page: Option<u16>,
    /// Renders pass the viewport generation they were requested for.
    pub viewport_gen: Option<u64>,
    pub cancel: Option<Arc<std::sync::atomic::AtomicBool>>,
}

impl Submit {
    pub fn new(lane: Lane, label: &'static str) -> Self {
        Self {
            lane,
            label,
            priority: 0,
            page: None,
            viewport_gen: None,
            cancel: None,
        }
    }

    pub fn priority(mut self, priority: u32) -> Self {
        self.priority = priority;
        self
    }

    pub fn page(mut self, page: u16) -> Self {
        self.page = Some(page);
        self
    }

    /// Marks this command droppable when the viewport has moved on.
    pub fn viewport(mut self, gen: u64) -> Self {
        self.viewport_gen = Some(gen);
        self
    }

    pub fn cancel(mut self, flag: Arc<std::sync::atomic::AtomicBool>) -> Self {
        self.cancel = Some(flag);
        self
    }
}

impl EngineHandle {
    pub fn queue_len(&self) -> u32 {
        self.queued.load(Ordering::Relaxed)
    }

    fn push(&self, cmd: Cmd) -> Result<(), EngineError> {
        let queued = self.queued.fetch_add(1, Ordering::Relaxed) + 1;
        if queued > MAX_QUEUED && cmd.lane.is_render_lane() {
            self.queued.fetch_sub(1, Ordering::Relaxed);
            return Err(EngineError::busy("engine queue is full"));
        }
        self.tx.send(cmd).map_err(|_| {
            self.queued.fetch_sub(1, Ordering::Relaxed);
            EngineError::new(ErrorCode::Io, "engine thread is gone")
        })
    }

    /// Fire-and-forget. `run` is called with the status the scheduler decided, so the caller
    /// can still answer a `UriSchemeResponder` when the command is dropped.
    pub fn dispatch(
        &self,
        opts: Submit,
        run: impl FnOnce(&mut EngineState<'_>, CmdStatus) + Send + 'static,
    ) -> Result<(), EngineError> {
        let cmd = Cmd {
            lane: opts.lane,
            priority: opts.priority,
            seq: self.shared.next_seq(),
            viewport_gen: opts.viewport_gen.unwrap_or(u64::MAX),
            page: opts.page,
            label: opts.label,
            cancel: opts.cancel,
            run: Box::new(run),
        };
        self.push(cmd)
    }

    /// Fire-and-forget for work that is never dropped.
    pub fn send(
        &self,
        lane: Lane,
        label: &'static str,
        f: impl FnOnce(&mut EngineState<'_>) + Send + 'static,
    ) -> Result<(), EngineError> {
        self.dispatch(Submit::new(lane, label), move |st, status| {
            if status == CmdStatus::Run {
                f(st)
            }
        })
    }

    /// The call every `#[tauri::command]` makes.
    pub async fn call<T, F>(&self, lane: Lane, label: &'static str, f: F) -> Result<T, EngineError>
    where
        T: Send + 'static,
        F: FnOnce(&mut EngineState<'_>) -> Result<T, EngineError> + Send + 'static,
    {
        let (reply, mut rx) = Reply::<Result<T, EngineError>>::oneshot();
        self.dispatch_reply(Submit::new(lane, label), reply, f)?;
        rx.recv()
            .await
            .unwrap_or_else(|| Err(EngineError::new(ErrorCode::Io, "engine dropped the reply")))
    }

    /// Blocking variant, for tests and for code outside the async runtime.
    pub fn call_blocking<T, F>(
        &self,
        lane: Lane,
        label: &'static str,
        f: F,
    ) -> Result<T, EngineError>
    where
        T: Send + 'static,
        F: FnOnce(&mut EngineState<'_>) -> Result<T, EngineError> + Send + 'static,
    {
        let (reply, rx) = Reply::<Result<T, EngineError>>::blocking();
        self.dispatch_reply(Submit::new(lane, label), reply, f)?;
        rx.recv()
            .unwrap_or_else(|_| Err(EngineError::new(ErrorCode::Io, "engine dropped the reply")))
    }

    /// Like [`EngineHandle::call`] but with full scheduling control (used by renders).
    pub fn call_with<T, F>(
        &self,
        opts: Submit,
        f: F,
    ) -> tauri::async_runtime::Receiver<Result<T, EngineError>>
    where
        T: Send + 'static,
        F: FnOnce(&mut EngineState<'_>) -> Result<T, EngineError> + Send + 'static,
    {
        let (reply, rx) = Reply::<Result<T, EngineError>>::oneshot();
        if let Err(e) = self.dispatch_reply(opts, reply, f) {
            let (reply2, rx2) = Reply::<Result<T, EngineError>>::oneshot();
            reply2.send(Err(e));
            return rx2;
        }
        rx
    }

    fn dispatch_reply<T, F>(
        &self,
        opts: Submit,
        reply: Reply<Result<T, EngineError>>,
        f: F,
    ) -> Result<(), EngineError>
    where
        T: Send + 'static,
        F: FnOnce(&mut EngineState<'_>) -> Result<T, EngineError> + Send + 'static,
    {
        let label = opts.label;
        self.dispatch(opts, move |st, status| match status {
            // A panic answers the caller with `engineCrashed`, then carries on unwinding so
            // the loop closes the documents the command touched.
            CmdStatus::Run => match catch_unwind(AssertUnwindSafe(|| f(st))) {
                Ok(result) => reply.send(result),
                Err(payload) => {
                    reply.send(Err(crashed(label, payload.as_ref())));
                    resume_unwind(payload)
                }
            },
            CmdStatus::Stale => reply.send(Err(EngineError::stale("viewport moved on"))),
            CmdStatus::Cancelled => reply.send(Err(EngineError::cancelled("job cancelled"))),
        })
    }

    /// `set_viewport`: records the hint and bumps the generation renders are compared against.
    pub fn set_viewport(&self, viewport: crate::engine::types::Viewport) {
        *self.shared.viewport.lock() = viewport;
        self.shared.bump_viewport_gen();
    }
}

/// Spawns the engine thread and waits for `Pdfium::bind_to_library` to succeed, so a missing
/// or broken libpdfium fails at startup instead of on the first render.
pub fn spawn(
    library: PathBuf,
    pdfium_dir: PathBuf,
    app: Option<tauri::AppHandle>,
    spill_dir: PathBuf,
    tile_cache_bytes: usize,
) -> Result<EngineHandle, EngineError> {
    let (tx, rx) = unbounded::<Cmd>();
    let shared = Arc::new(EngineShared::new(pdfium_dir, tile_cache_bytes));
    let queued = Arc::new(AtomicU32::new(0));
    let handle = EngineHandle {
        tx,
        queued: queued.clone(),
        shared: shared.clone(),
        jobs: Arc::new(Jobs::default()),
    };

    let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<(), String>>();
    let thread_shared = shared.clone();
    std::thread::Builder::new()
        .name("seepdf-engine".into())
        .stack_size(4 * 1024 * 1024)
        .spawn(move || engine_main(library, rx, queued, thread_shared, app, spill_dir, ready_tx))
        .map_err(|e| EngineError::io(format!("spawn engine thread: {e}")))?;

    match ready_rx.recv() {
        Ok(Ok(())) => Ok(handle),
        Ok(Err(e)) => Err(EngineError::new(ErrorCode::Pdfium, e)),
        Err(_) => Err(EngineError::new(
            ErrorCode::Io,
            "engine thread died during startup",
        )),
    }
}

#[allow(clippy::too_many_arguments)]
fn engine_main(
    library: PathBuf,
    rx: Receiver<Cmd>,
    queued: Arc<AtomicU32>,
    shared: Arc<EngineShared>,
    app: Option<tauri::AppHandle>,
    spill_dir: PathBuf,
    ready: std::sync::mpsc::Sender<Result<(), String>>,
) {
    let t0 = Instant::now();
    let bindings = match Pdfium::bind_to_library(&library) {
        Ok(b) => b,
        Err(e) => {
            let msg = format!("failed to bind libpdfium at {}: {e:?}", library.display());
            tracing::error!("{msg}");
            let _ = ready.send(Err(msg));
            return;
        }
    };
    // `Pdfium` lives on this thread's stack for the whole process; documents borrow it.
    let pdfium = Pdfium::new(bindings);
    let version = read_version(&library);
    *shared.pdfium_version.write() = version.clone();
    tracing::info!(
        lib = %library.display(),
        pdfium_version = %version,
        bind_ms = t0.elapsed().as_secs_f64() * 1000.0,
        "pdfium bound on the engine thread"
    );
    let _ = ready.send(Ok(()));

    let mut st = EngineState {
        pdfium: &pdfium,
        docs: HashMap::new(),
        shared: shared.clone(),
        app,
        encode: EncodePool::spawn(encode_workers()),
        next_doc_id: 0,
        spill_dir,
        history_budget: crate::engine::history::RamBudget::new(
            crate::engine::history::DEFAULT_RAM_BUDGET,
        ),
    };

    let mut lanes: [BinaryHeap<Cmd>; 5] = Default::default();
    let mut fast_streak: u32 = 0;

    loop {
        // Block for the first command, then drain everything that is already queued so the
        // heap sees the whole picture before choosing.
        match rx.recv() {
            Ok(cmd) => lanes[cmd.lane.index()].push(cmd),
            Err(_) => break,
        }
        drain(&rx, &mut lanes);

        while let Some(cmd) = pop_next(&mut lanes, &mut fast_streak) {
            queued.fetch_sub(1, Ordering::Relaxed);
            publish_depth(&shared, &lanes);
            let status = classify(&shared, &cmd);
            if status == CmdStatus::Stale {
                shared.stats.dropped_stale.fetch_add(1, Ordering::Relaxed);
                tracing::trace!(label = cmd.label, page = ?cmd.page, "dropped stale render");
            }
            let label = cmd.label;
            let started = Instant::now();
            let run = cmd.run;
            take_touched();
            if catch_unwind(AssertUnwindSafe(|| run(&mut st, status))).is_err() {
                poison(&mut st, label, take_touched());
            }
            let elapsed = started.elapsed().as_secs_f64() * 1000.0;
            if elapsed > 50.0 {
                tracing::debug!(label, ms = elapsed, "long engine command");
            }
            let lru: u32 = st.docs.values().map(|d| d.page_lru_len() as u32).sum();
            shared.stats.page_lru_len.store(lru, Ordering::Relaxed);
            drain(&rx, &mut lanes);
        }
    }

    // Drop documents (pages first, inside OpenDoc) before `pdfium` leaves scope.
    st.docs.clear();
    tracing::info!("engine thread exiting");
}

fn drain(rx: &Receiver<Cmd>, lanes: &mut [BinaryHeap<Cmd>; 5]) {
    while let Ok(cmd) = rx.try_recv() {
        lanes[cmd.lane.index()].push(cmd);
    }
}

fn publish_depth(shared: &EngineShared, lanes: &[BinaryHeap<Cmd>; 5]) {
    for lane in Lane::ALL {
        shared
            .stats
            .set_queue_depth(lane, lanes[lane.index()].len() as u32);
    }
}

/// Lane order, with the starvation guard: after [`STARVATION_GUARD`] consecutive
/// Interactive/Prefetch pops, an Edit or Background command gets a turn.
fn pop_next(lanes: &mut [BinaryHeap<Cmd>; 5], fast_streak: &mut u32) -> Option<Cmd> {
    let fast_pending =
        !lanes[Lane::Interactive.index()].is_empty() || !lanes[Lane::Prefetch.index()].is_empty();
    if *fast_streak >= STARVATION_GUARD && fast_pending {
        for lane in [Lane::Edit, Lane::Background] {
            if let Some(cmd) = lanes[lane.index()].pop() {
                *fast_streak = 0;
                return Some(cmd);
            }
        }
    }
    for lane in Lane::ALL {
        if let Some(cmd) = lanes[lane.index()].pop() {
            if matches!(lane, Lane::Interactive | Lane::Prefetch) {
                *fast_streak += 1;
            } else {
                *fast_streak = 0;
            }
            return Some(cmd);
        }
    }
    None
}

fn classify(shared: &EngineShared, cmd: &Cmd) -> CmdStatus {
    if cmd.is_cancelled() {
        return CmdStatus::Cancelled;
    }
    if cmd.viewport_gen != u64::MAX && cmd.lane.is_render_lane() {
        let current = shared.viewport_gen();
        if cmd.viewport_gen + 1 < current {
            let keep = cmd
                .page
                .map(|p| shared.viewport.lock().keeps(p))
                .unwrap_or(false);
            if !keep {
                return CmdStatus::Stale;
            }
        }
    }
    CmdStatus::Run
}

fn encode_workers() -> usize {
    std::thread::available_parallelism()
        .map(|n| (n.get().saturating_sub(2)).clamp(1, 2))
        .unwrap_or(1)
}

/// `VERSION` ships next to the library: `MAJOR=155 / MINOR=0 / BUILD=8057 / PATCH=0`.
fn read_version(library: &std::path::Path) -> String {
    std::fs::read_to_string(library.with_file_name("VERSION"))
        .map(|s| {
            s.lines()
                .filter_map(|l| l.split_once('='))
                .map(|(_, v)| v.trim().to_string())
                .collect::<Vec<_>>()
                .join(".")
        })
        .unwrap_or_else(|_| "unknown".into())
}
