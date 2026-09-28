//! Cancellable long jobs (`search_start`, exports, OCR apply, annotation scans).
//!
//! A job is a `JobId` plus an `Arc<AtomicBool>` that the engine polls between chunks. Long
//! work is submitted as **one command per page** on `Lane::Background` sharing one token, so
//! visible tiles interleave and cancellation is observed within one page (≤ 30 ms).

use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

#[derive(Debug, Clone)]
pub struct JobToken {
    pub id: u64,
    pub cancel: Arc<AtomicBool>,
}

impl JobToken {
    pub fn is_cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
}

#[derive(Default)]
pub struct Jobs {
    next_id: AtomicU64,
    flags: Mutex<HashMap<u64, Arc<AtomicBool>>>,
}

impl Jobs {
    pub fn create(&self) -> JobToken {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed) + 1;
        let cancel = Arc::new(AtomicBool::new(false));
        self.flags.lock().insert(id, cancel.clone());
        JobToken { id, cancel }
    }

    /// A job id for progress events only — for work that cannot stop half-way (a save is one
    /// atomic file rewrite). No cancel flag is registered, so `cancel_job` answers `false`
    /// instead of `true` for a job that then runs to completion anyway.
    pub fn progress_only(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::Relaxed) + 1
    }

    /// `cancel_job` (`IPC_CONTRACT.md` §6). Returns false for an unknown or finished job.
    pub fn cancel(&self, id: u64) -> bool {
        match self.flags.lock().get(&id) {
            Some(flag) => {
                flag.store(true, Ordering::Relaxed);
                true
            }
            None => false,
        }
    }

    pub fn finish(&self, id: u64) {
        self.flags.lock().remove(&id);
    }

    pub fn is_cancelled(&self, id: u64) -> bool {
        self.flags
            .lock()
            .get(&id)
            .map(|f| f.load(Ordering::Relaxed))
            .unwrap_or(false)
    }

    pub fn len(&self) -> usize {
        self.flags.lock().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::Jobs;

    #[test]
    fn progress_only_ids_are_not_cancellable() {
        let jobs = Jobs::default();
        let token = jobs.create();
        let save = jobs.progress_only();
        assert_ne!(token.id, save, "one id space");
        assert!(jobs.cancel(token.id));
        assert!(token.is_cancelled());
        assert!(!jobs.cancel(save), "cancel_job answers false for a save");
    }
}
