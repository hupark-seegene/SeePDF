//! Progress bookkeeping shared by the three channel-driven jobs Stage 1 (b) owns:
//! `export_images`, `export_flattened` and `split_document`.
//!
//! All three have the same shape — one `Lane::Background` command per unit of work, all
//! sharing one `JobToken`, streaming `JobEvent`s over a per-invocation `Channel` — and the
//! same obligation: **exactly one** terminal event (`done`, `cancelled` or `error`) reaches the
//! frontend, whichever command gets there first, and `Jobs::finish` runs with it so the token
//! is not leaked. [`JobReporter`] is that bookkeeping; the commands supply the work.

use crate::engine::jobs::Jobs;
use crate::ipc::types::{JobEvent, JobId, PageIndex};
use crate::ipc::EngineError;
use parking_lot::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Instant;
use tauri::ipc::Channel;

pub struct JobReporter {
    channel: Channel<JobEvent>,
    jobs: Arc<Jobs>,
    job_id: JobId,
    total: u32,
    done: AtomicU32,
    finished: AtomicBool,
    started: Instant,
    outputs: Mutex<Vec<String>>,
}

impl JobReporter {
    /// Emits `started` immediately, as the contract's `JobEvent` sequence requires.
    pub fn start(
        channel: Channel<JobEvent>,
        jobs: Arc<Jobs>,
        job_id: JobId,
        total: u32,
    ) -> Arc<Self> {
        let _ = channel.send(JobEvent::Started { job_id, total });
        Arc::new(Self {
            channel,
            jobs,
            job_id,
            total,
            done: AtomicU32::new(0),
            finished: AtomicBool::new(false),
            started: Instant::now(),
            outputs: Mutex::new(Vec::new()),
        })
    }

    pub fn job_id(&self) -> JobId {
        self.job_id
    }

    /// One unit finished. `output` is the file it produced, if any.
    pub fn step(&self, page: Option<PageIndex>, output: Option<String>) -> u32 {
        if let Some(path) = output {
            self.outputs.lock().push(path);
        }
        let done = self.done.fetch_add(1, Ordering::Relaxed) + 1;
        let _ = self.channel.send(JobEvent::Progress {
            job_id: self.job_id,
            done,
            total: self.total,
            page,
            note: None,
        });
        done
    }

    /// Sends `done` once every unit has reported. No-op until then.
    pub fn finish_if_complete(&self) {
        if self.done.load(Ordering::Relaxed) < self.total {
            return;
        }
        if !self.claim() {
            return;
        }
        let outputs = self.outputs.lock().clone();
        let _ = self.channel.send(JobEvent::Done {
            job_id: self.job_id,
            elapsed_ms: self.started.elapsed().as_secs_f64() * 1000.0,
            outputs: Some(outputs),
        });
        self.jobs.finish(self.job_id);
    }

    /// Sends `done` regardless of the counter (an empty job, or one that short-circuited).
    pub fn finish_now(&self) {
        if !self.claim() {
            return;
        }
        let outputs = self.outputs.lock().clone();
        let _ = self.channel.send(JobEvent::Done {
            job_id: self.job_id,
            elapsed_ms: self.started.elapsed().as_secs_f64() * 1000.0,
            outputs: Some(outputs),
        });
        self.jobs.finish(self.job_id);
    }

    pub fn cancel(&self) {
        if !self.claim() {
            return;
        }
        let _ = self.channel.send(JobEvent::Cancelled {
            job_id: self.job_id,
            done: self.done.load(Ordering::Relaxed),
        });
        self.jobs.finish(self.job_id);
    }

    /// An error ends the job: a half-written export is not something to keep grinding at.
    pub fn fail(&self, error: EngineError) {
        if !self.claim() {
            return;
        }
        let _ = self.channel.send(JobEvent::Error {
            job_id: self.job_id,
            error,
        });
        self.jobs.finish(self.job_id);
    }

    pub fn is_finished(&self) -> bool {
        self.finished.load(Ordering::SeqCst)
    }

    /// Exactly one terminal event per job.
    fn claim(&self) -> bool {
        !self.finished.swap(true, Ordering::SeqCst)
    }
}
