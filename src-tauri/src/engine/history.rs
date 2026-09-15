//! Undo / redo as whole-document snapshots — `ARCHITECTURE.md` §7.
//!
//! `registry::mutate` pushes the **pre-edit** bytes (`save_to_bytes()`, 5 ms/MB; the first
//! push after open / save / undo reuses `OpenDoc::bytes` and is free). Undo is
//! `registry::replace(doc, pop())`. One mechanism covers annotations, forms, page ops,
//! objects, OCR and redaction with no inverse-op bookkeeping.

use crate::ipc::EngineError;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// RAM budget across the whole history stack before snapshots spill to disk.
pub const DEFAULT_RAM_BUDGET: usize = 256 * 1024 * 1024;
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
                let bytes = std::fs::read(path)
                    .map_err(|e| EngineError::io(format!("read snapshot {}: {e}", path.display())))?;
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
    ram_bytes: usize,
    budget: usize,
    depth: usize,
    spill_dir: PathBuf,
    next_spill: u64,
}

impl History {
    pub fn new(spill_dir: PathBuf, doc_bytes: usize, budget: usize) -> Self {
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
        self.redo.clear();
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
        while self.undo.len() > self.depth {
            let dropped = self.undo.remove(0);
            self.release(&dropped.snapshot);
        }
        Ok(true)
    }

    /// Undo a [`push`](Self::push) that turned out to describe an edit that never happened.
    ///
    /// Unlike [`take_undo`](Self::take_undo) this pushes **nothing** onto the redo stack: the
    /// document never left the state the popped snapshot describes, so offering to "redo" it
    /// would be offering to redo nothing. Used by `registry::mutate`'s error path.
    pub fn discard_last_undo(&mut self) {
        if let Some(entry) = self.undo.pop() {
            self.release(&entry.snapshot);
        }
    }

    /// Pops the newest undo entry; the caller pushes `current` onto the redo stack.
    pub fn take_undo(
        &mut self,
        current: Arc<[u8]>,
    ) -> Result<Option<(String, Arc<[u8]>)>, EngineError> {
        let Some(entry) = self.undo.pop() else {
            return Ok(None);
        };
        self.release(&entry.snapshot);
        let bytes = entry.snapshot.load()?;
        let snapshot = self.store(current)?;
        self.redo.push(Entry {
            label: entry.label.clone(),
            snapshot,
            at_ms: now_ms(),
        });
        Ok(Some((entry.label, bytes)))
    }

    /// Pops the newest redo entry; the caller pushes `current` onto the undo stack.
    pub fn take_redo(
        &mut self,
        current: Arc<[u8]>,
    ) -> Result<Option<(String, Arc<[u8]>)>, EngineError> {
        let Some(entry) = self.redo.pop() else {
            return Ok(None);
        };
        self.release(&entry.snapshot);
        let bytes = entry.snapshot.load()?;
        let snapshot = self.store(current)?;
        self.undo.push(Entry {
            label: entry.label.clone(),
            snapshot,
            at_ms: now_ms(),
        });
        Ok(Some((entry.label, bytes)))
    }

    /// Drops both stacks (document closed, or replaced by a Save As).
    pub fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
        self.ram_bytes = 0;
    }

    fn release(&mut self, snapshot: &Snapshot) {
        if let Snapshot::Ram(b) = snapshot {
            self.ram_bytes = self.ram_bytes.saturating_sub(b.len());
        }
    }

    fn store(&mut self, bytes: Arc<[u8]>) -> Result<Snapshot, EngineError> {
        let len = bytes.len();
        if self.ram_bytes + len <= self.budget && len <= LARGE_DOC_BYTES {
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
        self.spill_dir.join(format!("snap-{:06}.pdf", self.next_spill))
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

fn now_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}
