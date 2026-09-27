//! Watermark / header / footer (P1-4) and compress (P1-5) — `IPC_CONTRACT.md` §7.4a, §7.6a.
//! Owner: **Stage 4**.
//!
//! `add_stamp` is one `Lane::Edit` call (one undo step). `compress_estimate` is a job on the
//! `job` channel like the exports; its `done` event carries the `CompressReport`, whose token
//! `compress_apply` / `compress_discard` then consume.

use crate::engine::compress;
use crate::engine::export::job::{channel_sink, JobReporter};
use crate::engine::stamp;
use crate::engine::{EngineHandle, Lane};
use crate::ipc::types::{CompressOptions, DocInfo, JobEvent, JobId, PageStampSpec, StampResult};
use crate::ipc::EngineError;
use tauri::ipc::Channel;
use tauri::State;

#[tauri::command]
pub async fn add_stamp(
    engine: State<'_, EngineHandle>,
    doc_id: String,
    spec: PageStampSpec,
) -> Result<StampResult, EngineError> {
    engine
        .call(Lane::Edit, "add_stamp", move |st| {
            stamp::add_stamp(st, &doc_id, &spec)
        })
        .await
}

/// Validation and the scratch snapshot happen before the job id is returned, so a bad preset
/// or page index rejects the promise instead of arriving as an `error` event.
#[tauri::command]
pub async fn compress_estimate(
    engine: State<'_, EngineHandle>,
    doc_id: String,
    options: CompressOptions,
    on_progress: Channel<JobEvent>,
) -> Result<JobId, EngineError> {
    let token = engine.jobs.create();
    let job_id = token.id;
    let pages = {
        let doc_id = doc_id.clone();
        engine
            .call(Lane::Edit, "compress_begin", move |st| {
                compress::begin(st, &doc_id, &options, job_id)
            })
            .await
    };
    let pages = match pages {
        Ok(pages) => pages,
        Err(e) => {
            engine.jobs.finish(job_id);
            return Err(e);
        }
    };
    let reporter = JobReporter::start_with(
        channel_sink(on_progress),
        engine.jobs.clone(),
        job_id,
        pages.len() as u32,
    );
    compress::dispatch(&engine, &doc_id, pages, &token, reporter);
    Ok(job_id)
}

#[tauri::command]
pub async fn compress_apply(
    engine: State<'_, EngineHandle>,
    doc_id: String,
    token: u64,
) -> Result<DocInfo, EngineError> {
    engine
        .call(Lane::Edit, "compress_apply", move |st| {
            compress::apply(st, &doc_id, token)
        })
        .await
}

#[tauri::command]
pub async fn compress_discard(
    engine: State<'_, EngineHandle>,
    doc_id: String,
    token: u64,
) -> Result<(), EngineError> {
    engine
        .call(Lane::Edit, "compress_discard", move |st| {
            compress::discard(st, &doc_id, token)
        })
        .await
}
