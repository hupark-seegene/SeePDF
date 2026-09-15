//! Cancellable long-running job with progress reporting.
//!
//! Progress goes over a `tauri::ipc::Channel<JobEvent>` (ordered, per-call, cheap) and/or
//! `app.emit("job-progress", ..)` (broadcast to every listener, JSON event bus) so the two
//! can be compared. Cancellation is an `AtomicBool` the worker polls between steps.

use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tauri::ipc::Channel;
use tauri::{AppHandle, Emitter, Manager, State};

#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum JobEvent {
    Started { job_id: u64, total: u32 },
    Progress { job_id: u64, done: u32, total: u32 },
    Done { job_id: u64, elapsed_ms: f64 },
    Cancelled { job_id: u64, done: u32 },
}

#[derive(Default)]
pub struct Jobs {
    next_id: AtomicU64,
    cancel_flags: Mutex<HashMap<u64, Arc<AtomicBool>>>,
}

/// `mode`: "channel" | "event" | "both". Returns the job id immediately; the work runs on a
/// worker thread. (Real pdfium jobs run on the engine thread and poll the flag per page.)
#[tauri::command]
pub fn start_job(
    app: AppHandle,
    jobs: State<'_, Jobs>,
    steps: u32,
    step_ms: u64,
    mode: String,
    on_progress: Channel<JobEvent>,
) -> u64 {
    let job_id = jobs.next_id.fetch_add(1, Ordering::Relaxed) + 1;
    let flag = Arc::new(AtomicBool::new(false));
    jobs.cancel_flags.lock().insert(job_id, flag.clone());
    let use_channel = mode != "event";
    let use_event = mode != "channel";

    std::thread::Builder::new()
        .name(format!("job-{job_id}"))
        .spawn(move || {
            let t0 = Instant::now();
            let send = |ev: JobEvent| {
                if use_channel {
                    if let Err(e) = on_progress.send(ev.clone()) {
                        tracing::warn!(job_id, "channel send failed: {e}");
                    }
                }
                if use_event {
                    if let Err(e) = app.emit("job-progress", ev) {
                        tracing::warn!(job_id, "emit failed: {e}");
                    }
                }
            };
            send(JobEvent::Started { job_id, total: steps });
            let mut done = 0;
            let mut cancelled = false;
            while done < steps {
                if flag.load(Ordering::Relaxed) {
                    cancelled = true;
                    break;
                }
                if step_ms > 0 {
                    std::thread::sleep(Duration::from_millis(step_ms));
                }
                done += 1;
                send(JobEvent::Progress { job_id, done, total: steps });
            }
            let elapsed_ms = t0.elapsed().as_secs_f64() * 1000.0;
            if cancelled {
                tracing::info!(job_id, done, "job cancelled");
                send(JobEvent::Cancelled { job_id, done });
            } else {
                tracing::info!(job_id, steps, elapsed_ms, mode, "job done");
                send(JobEvent::Done { job_id, elapsed_ms });
            }
            app.state::<Jobs>().cancel_flags.lock().remove(&job_id);
        })
        .expect("spawn job thread");
    job_id
}

#[tauri::command]
pub fn cancel_job(jobs: State<'_, Jobs>, job_id: u64) -> bool {
    match jobs.cancel_flags.lock().get(&job_id) {
        Some(flag) => {
            flag.store(true, Ordering::Relaxed);
            true
        }
        None => false,
    }
}
