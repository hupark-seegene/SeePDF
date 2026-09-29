//! Undo / redo as whole-document snapshots — `ARCHITECTURE.md` §7.
//!
//! `registry::mutate` pushes the **pre-edit** bytes (`save_to_bytes()`, 5 ms/MB; the first
//! push after open / save / undo reuses `OpenDoc::bytes` and is free). Undo is
//! `registry::replace(doc, pop())`. One mechanism covers annotations, forms, page ops,
//! objects, OCR and redaction with no inverse-op bookkeeping.

use crate::ipc::EngineError;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

/// RAM budget for the snapshots of **every open document together** before they spill to
/// disk (`EngineState::history_budget`).
pub const DEFAULT_RAM_BUDGET: usize = 256 * 1024 * 1024;

/// The undo RAM budget one engine shares between all of its documents' [`History`]s.
///
/// Each `History` used to get its own copy of the limit, so three open 40 MB documents could
/// keep 3 × 256 MiB of snapshots resident. Now every RAM snapshot reserves its bytes here and
/// gives them back when it is dropped, popped or spilled, so the limit is global.
#[derive(Debug)]
pub struct RamBudget {
    limit: AtomicUsize,
    used: AtomicUsize,
}

impl RamBudget {
    pub fn new(limit: usize) -> Arc<Self> {
        Arc::new(Self {
            limit: AtomicUsize::new(limit),
            used: AtomicUsize::new(0),
        })
    }

    pub fn limit(&self) -> usize {
        self.limit.load(Ordering::Relaxed)
    }

    /// Changes the limit; snapshots already resident stay where they are.
    pub fn set_limit(&self, limit: usize) {
        self.limit.store(limit, Ordering::Relaxed);
    }

    /// Bytes of RAM snapshots currently resident, across all documents.
    pub fn used(&self) -> usize {
        self.used.load(Ordering::Relaxed)
    }

    fn try_reserve(&self, len: usize) -> bool {
        let limit = self.limit();
        self.used
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |used| {
                used.checked_add(len).filter(|&total| total <= limit)
            })
            .is_ok()
    }

    fn release(&self, len: usize) {
        let _ = self
            .used
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |used| {
                Some(used.saturating_sub(len))
            });
    }
}
/// Maximum undo depth for normal documents.
pub const DEFAULT_DEPTH: usize = 50;
/// Documents larger than this get depth 3 and go straight to disk.
pub const LARGE_DOC_BYTES: usize = 48 * 1024 * 1024;
pub const LARGE_DOC_DEPTH: usize = 3;
/// Two edits of the same label within this window collapse into one undo step.
pub const COALESCE_MS: u128 = 500;

#[derive(Debug)]
pub enum Snapshot {
    Ram(Arc<[u8]>),
    Disk { path: PathBuf, len: usize },
}

impl Snapshot {
    pub fn len(&self) -> usize {
        match self {
            Snapshot::Ram(b) => b.len(),
            Snapshot::Disk { len, .. } => *len,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn load(&self) -> Result<Arc<[u8]>, EngineError> {
        match self {
            Snapshot::Ram(b) => Ok(b.clone()),
            Snapshot::Disk { path, .. } => {
                let bytes = std::fs::read(path).map_err(|e| {
                    EngineError::io(format!("read snapshot {}: {e}", path.display()))
                })?;
                Ok(Arc::from(bytes.into_boxed_slice()))
            }
        }
    }
}

impl Drop for Snapshot {
    fn drop(&mut self) {
        if let Snapshot::Disk { path, .. } = self {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// What [`History::take_undo`] / [`History::take_redo`] hand back: the entry's label and the
/// document bytes to restore.
pub type Popped = (String, Arc<[u8]>);

/// An undo / redo step that has been **prepared** but not applied: the bytes to restore are
/// loaded and the current state is already stored, yet neither stack has moved.
///
/// `registry::undo` replaces the document with [`Step::bytes`] and only then calls
/// [`History::commit`]; if the snapshot could not be loaded (a spilled file that is gone) or
/// the replace fails, the stacks are exactly as they were and [`History::abandon`] throws the
/// stored current state away.
#[derive(Debug)]
pub struct Step {
    redo: bool,
    label: String,
    bytes: Arc<[u8]>,
    current: Snapshot,
}

impl Step {
    pub fn bytes(&self) -> Arc<[u8]> {
        self.bytes.clone()
    }

    pub fn label(&self) -> &str {
        &self.label
    }
}

#[derive(Debug)]
pub struct Entry {
    /// i18n key, e.g. `"undo.annotCreate"`.
    pub label: String,
    pub snapshot: Snapshot,
    /// Monotonic milliseconds, for gesture coalescing.
    pub at_ms: u128,
}

#[derive(Debug)]
pub struct History {
    undo: Vec<Entry>,
    redo: Vec<Entry>,
    /// This document's share of `budget.used()`.
    ram_bytes: usize,
    budget: Arc<RamBudget>,
    depth: usize,
    spill_dir: PathBuf,
    next_spill: u64,
    /// v0.3 pkg4: entries ever stored by [`push`](Self::push) — the mark of
    /// [`squash_since`](Self::squash_since).
    pushes: u64,
    /// v0.3 pkg4: a batch is open ([`begin_batch`](Self::begin_batch)) — depth trimming is
    /// held until [`end_batch`](Self::end_batch), so the entry holding the pre-batch
    /// snapshot can never be trimmed away while the batch still runs.
    batching: bool,
}

impl History {
    /// A history with a budget of its own (tests, tools). Open documents use
    /// [`History::shared`] with the engine's one [`RamBudget`].
    pub fn new(spill_dir: PathBuf, doc_bytes: usize, budget: usize) -> Self {
        Self::shared(spill_dir, doc_bytes, RamBudget::new(budget))
    }

    pub fn shared(spill_dir: PathBuf, doc_bytes: usize, budget: Arc<RamBudget>) -> Self {
        let depth = if doc_bytes > LARGE_DOC_BYTES {
            LARGE_DOC_DEPTH
        } else {
            DEFAULT_DEPTH
        };
        Self {
            undo: Vec::new(),
            redo: Vec::new(),
            ram_bytes: 0,
            budget,
            depth,
            spill_dir,
            next_spill: 0,
            pushes: 0,
            batching: false,
        }
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn undo_label(&self) -> Option<String> {
        self.undo.last().map(|e| e.label.clone())
    }

    pub fn redo_label(&self) -> Option<String> {
        self.redo.last().map(|e| e.label.clone())
    }

    pub fn undo_depth(&self) -> usize {
        self.undo.len()
    }

    pub fn redo_depth(&self) -> usize {
        self.redo.len()
    }

    pub fn ram_bytes(&self) -> usize {
        self.ram_bytes
    }

    /// Pushes a pre-edit snapshot and clears the redo stack.
    ///
    /// `coalesce` is true for drag/slider gestures: a push with the same label inside
    /// [`COALESCE_MS`] keeps the older snapshot, so one gesture is one undo step.
    ///
    /// Returns whether an entry was actually stored — `false` means the push coalesced into
    /// the previous one. [`registry::mutate`](crate::engine::registry::mutate) needs that to
    /// know whether its rollback should pop anything.
    pub fn push(
        &mut self,
        label: impl Into<String>,
        bytes: Arc<[u8]>,
        coalesce: bool,
    ) -> Result<bool, EngineError> {
        let label = label.into();
        let now = now_ms();
        self.clear_redo();
        if coalesce {
            if let Some(last) = self.undo.last() {
                if last.label == label && now.saturating_sub(last.at_ms) <= COALESCE_MS {
                    return Ok(false);
                }
            }
        }
        let snapshot = self.store(bytes)?;
        self.undo.push(Entry {
            label,
            snapshot,
            at_ms: now,
        });
        self.pushes += 1;
        if !self.batching {
            self.trim();
        }
        Ok(true)
    }

    /// Drops the oldest entries beyond `depth`.
    fn trim(&mut self) {
        while self.undo.len() > self.depth {
            let dropped = self.undo.remove(0);
            self.release(&dropped.snapshot);
        }
    }

    /// Undo a [`push`](Self::push) that turned out to describe an edit that never happened.
    ///
    /// Unlike [`take_undo`](Self::take_undo) this pushes **nothing** onto the redo stack: the
    /// document never left the state the popped snapshot describes, so offering to "redo" it
    /// would be offering to redo nothing. Used by `registry::mutate`'s error path.
    pub fn discard_last_undo(&mut self) {
        if let Some(entry) = self.undo.pop() {
            self.release(&entry.snapshot);
            // v0.3 pkg4: the push it undoes no longer counts (`squash_since`).
            self.pushes = self.pushes.saturating_sub(1);
        }
    }

    /// v0.3 pkg4: a mark for [`squash_since`](Self::squash_since) — how many entries
    /// [`push`](Self::push) has stored so far.
    pub fn push_count(&self) -> u64 {
        self.pushes
    }

    /// v0.3 pkg4 (A3 / A4): folds every entry stored since `mark` (a [`push_count`]) into
    /// the oldest of them — its snapshot is the state before the first of those edits — and
    /// names it `label`: a batch of edits becomes **one** undo step. Returns whether an entry
    /// for the batch is on the stack (`false`: nothing was stored since `mark`).
    ///
    /// The folded entry then counts as the **one** push since `mark`, so calling this again
    /// with the same mark after more pushes folds only the batch's own entries. A batch
    /// calls it after every op (inside [`begin_batch`](Self::begin_batch) /
    /// [`end_batch`](Self::end_batch)) so the stack never grows by more than one op's pushes.
    ///
    /// [`push_count`]: Self::push_count
    pub fn squash_since(&mut self, mark: u64, label: &str) -> bool {
        let n = (self.pushes.saturating_sub(mark) as usize).min(self.undo.len());
        if n == 0 {
            return false;
        }
        let first = self.undo.len() - n;
        let later: Vec<Entry> = self.undo.drain(first + 1..).collect();
        for entry in &later {
            self.release(&entry.snapshot);
        }
        self.pushes = mark + 1;
        let entry = &mut self.undo[first];
        entry.label = label.to_string();
        entry.at_ms = now_ms();
        true
    }

    /// v0.3 pkg4 (A3 / A4): opens a batch — returns the mark for
    /// [`squash_since`](Self::squash_since) / [`end_batch`](Self::end_batch) and holds depth
    /// trimming until `end_batch`, so on a depth-3 (large) document, or a batch of more than
    /// [`DEFAULT_DEPTH`] ops, the pre-batch snapshot is never trimmed away mid-batch.
    pub fn begin_batch(&mut self) -> u64 {
        self.batching = true;
        self.pushes
    }

    /// v0.3 pkg4: closes a batch opened by [`begin_batch`](Self::begin_batch): folds it into
    /// one entry named `label` (see [`squash_since`](Self::squash_since)), then trims the
    /// stack to depth again. Returns whether an entry for the batch is on the stack.
    pub fn end_batch(&mut self, mark: u64, label: &str) -> bool {
        let stored = self.squash_since(mark, label);
        self.close_batch();
        stored
    }

    /// v0.3 pkg4: closes a batch without folding — after a failed batch was rolled back
    /// (its entry undone), so trimming resumes only once the pre-batch entries are the
    /// newest again and none of them was dropped for an entry that no longer exists.
    pub fn close_batch(&mut self) {
        self.batching = false;
        self.trim();
    }

    /// v0.3 pkg4: drops the newest redo entry — a batch that failed half-way is undone with
    /// [`registry::undo`](crate::engine::registry::undo) and must not be offered as a redo.
    pub fn discard_last_redo(&mut self) {
        if let Some(entry) = self.redo.pop() {
            self.release(&entry.snapshot);
        }
    }

    /// v0.3 pkg4: restarts the coalescing window of the newest undo entry, so the **second
    /// half of a two-step edit** (a PDFium step, then a lopdf step pushed with `coalesce`)
    /// joins it however long the first half took. Callers invoke it right before that step.
    pub fn refresh_last(&mut self) {
        if let Some(last) = self.undo.last_mut() {
            last.at_ms = now_ms();
        }
    }

    /// Loads the newest undo (`redo == false`) or redo entry and stores `current`, **without
    /// moving either stack** — see [`Step`]. `None` when there is nothing to undo / redo; an
    /// error (snapshot unreadable, spill failed) leaves everything as it was.
    pub fn prepare(&mut self, redo: bool, current: Arc<[u8]>) -> Result<Option<Step>, EngineError> {
        let stack = if redo { &self.redo } else { &self.undo };
        let Some(entry) = stack.last() else {
            return Ok(None);
        };
        let label = entry.label.clone();
        let bytes = entry.snapshot.load()?;
        let current = self.store(current)?;
        Ok(Some(Step {
            redo,
            label,
            bytes,
            current,
        }))
    }

    /// Applies a prepared step once the document shows its bytes: the entry leaves its stack
    /// and the stored current state goes onto the other one. Returns the step's label.
    pub fn commit(&mut self, step: Step) -> String {
        let (from, to) = if step.redo {
            (&mut self.redo, &mut self.undo)
        } else {
            (&mut self.undo, &mut self.redo)
        };
        let popped = from.pop();
        to.push(Entry {
            label: step.label.clone(),
            snapshot: step.current,
            at_ms: now_ms(),
        });
        if let Some(entry) = popped {
            self.release(&entry.snapshot);
        }
        step.label
    }

    /// Drops a prepared step that could not be applied; the stacks never moved.
    pub fn abandon(&mut self, step: Step) {
        self.release(&step.current);
    }

    /// Pops the newest undo entry; the caller pushes `current` onto the redo stack.
    pub fn take_undo(&mut self, current: Arc<[u8]>) -> Result<Option<Popped>, EngineError> {
        self.take(false, current)
    }

    /// Pops the newest redo entry; the caller pushes `current` onto the undo stack.
    pub fn take_redo(&mut self, current: Arc<[u8]>) -> Result<Option<Popped>, EngineError> {
        self.take(true, current)
    }

    fn take(&mut self, redo: bool, current: Arc<[u8]>) -> Result<Option<Popped>, EngineError> {
        let Some(step) = self.prepare(redo, current)? else {
            return Ok(None);
        };
        let bytes = step.bytes();
        Ok(Some((self.commit(step), bytes)))
    }

    /// Drops both stacks (document closed, or replaced by a Save As).
    pub fn clear(&mut self) {
        self.undo.clear();
        self.clear_redo();
        self.budget.release(self.ram_bytes);
        self.ram_bytes = 0;
    }

    fn clear_redo(&mut self) {
        for entry in std::mem::take(&mut self.redo) {
            self.release(&entry.snapshot);
        }
    }

    fn release(&mut self, snapshot: &Snapshot) {
        if let Snapshot::Ram(b) = snapshot {
            let len = b.len().min(self.ram_bytes);
            self.ram_bytes -= len;
            self.budget.release(len);
        }
    }

    fn store(&mut self, bytes: Arc<[u8]>) -> Result<Snapshot, EngineError> {
        let len = bytes.len();
        if len <= LARGE_DOC_BYTES && self.budget.try_reserve(len) {
            self.ram_bytes += len;
            return Ok(Snapshot::Ram(bytes));
        }
        let path = self.spill_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| EngineError::io(format!("create {}: {e}", parent.display())))?;
        }
        std::fs::write(&path, &bytes[..])
            .map_err(|e| EngineError::io(format!("spill snapshot {}: {e}", path.display())))?;
        Ok(Snapshot::Disk { path, len })
    }

    fn spill_path(&mut self) -> PathBuf {
        self.next_spill += 1;
        self.spill_dir
            .join(format!("snap-{:06}.pdf", self.next_spill))
    }

    /// Test/introspection helper: whether the newest undo entry lives on disk.
    pub fn newest_is_on_disk(&self) -> Option<bool> {
        self.undo
            .last()
            .map(|e| matches!(e.snapshot, Snapshot::Disk { .. }))
    }

    pub fn spill_dir(&self) -> &Path {
        &self.spill_dir
    }
}

impl Drop for History {
    fn drop(&mut self) {
        // A document dropped without `close` (engine shutdown, a replaced `History`) must not
        // keep its share of the engine-wide budget.
        self.budget.release(self.ram_bytes);
    }
}

fn now_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}
