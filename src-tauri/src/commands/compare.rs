//! Compare two documents (P1-6) — `IPC_CONTRACT.md` §7.6b. Owner: **Stage 5**.
//!
//! `compare_documents` validates on the engine thread (unknown doc → `notFound`, same doc or a
//! page out of range → `invalidArgument`, promise rejected, no job), then runs one
//! `Lane::Background` command per page pair (with `alignPages`, the default since Stage 8:
//! one scan per page, an align step, then one per row); the `done` event carries the
//! `CompareReport`. Closing either document mid-job ends it with `cancelled`.

use crate::engine::compare;
use crate::engine::export::job::{channel_sink, JobReporter};
use crate::engine::{EngineHandle, Lane};
use crate::ipc::types::{CompareOptions, JobEvent, JobId};
use crate::ipc::EngineError;
use tauri::ipc::Channel;
use tauri::State;

#[tauri::command]
pub async fn compare_documents(
    engine: State<'_, EngineHandle>,
    doc_a: String,
    doc_b: String,
    options: CompareOptions,
    on_progress: Channel<JobEvent>,
) -> Result<JobId, EngineError> {
    let work = engine
        .call(Lane::Edit, "compare_begin", move |st| {
            compare::begin(st, &doc_a, &doc_b, &options)
        })
        .await?;
    let token = engine.jobs.create();
    let job_id = token.id;
    let reporter = JobReporter::start_with(
        channel_sink(on_progress),
        engine.jobs.clone(),
        job_id,
        work.initial_total(),
    );
    compare::dispatch(&engine, work, &token, reporter);
    Ok(job_id)
}
