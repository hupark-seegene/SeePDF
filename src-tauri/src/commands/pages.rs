//! Page operations — `IPC_CONTRACT.md` §7.3. Owner: **Stage 1 (b)**.
//!
//! One `page_ops` call is one `registry::mutate` with `MutateOpts::structural()` = one
//! generation = one undo step. Extract and split **copy the original and delete the other
//! pages**; `FPDF_ImportPages*` keeps the widgets but drops `/AcroForm`, the outline and the
//! metadata (pages spike §2). Merge of unrelated files is the one place import is used.

use crate::engine::export::job::JobReporter;
use crate::engine::pages;
use crate::engine::types::CmdStatus;
use crate::engine::{EngineHandle, Lane, Submit};
use crate::ipc::types::{
    DocInfo, ExtractPagesResult, JobEvent, JobId, MergeInput, MergeResult, PageIndex, PageOp,
    SplitMode,
};
use crate::ipc::EngineError;
use std::path::PathBuf;
use tauri::ipc::Channel;
use tauri::State;

#[tauri::command]
pub async fn page_ops(
    engine: State<'_, EngineHandle>,
    doc_id: String,
    ops: Vec<PageOp>,
) -> Result<DocInfo, EngineError> {
    engine
        .call(Lane::Edit, "page_ops", move |st| {
            pages::apply_ops(st, &doc_id, ops)
        })
        .await
}

#[tauri::command]
pub async fn extract_pages(
    engine: State<'_, EngineHandle>,
    doc_id: String,
    pages: Vec<PageIndex>,
    out_path: String,
    remove_after: bool,
) -> Result<ExtractPagesResult, EngineError> {
    engine
        .call(Lane::Edit, "extract_pages", move |st| {
            crate::engine::pages::extract(st, &doc_id, &pages, &out_path, remove_after)
        })
        .await
}

/// One `Lane::Background` command per output file, all sharing one `JobToken`, so the tile
/// renders of the organizer keep interleaving and Cancel is observed within one file.
#[tauri::command]
pub async fn split_document(
    engine: State<'_, EngineHandle>,
    doc_id: String,
    mode: SplitMode,
    out_dir: String,
    on_progress: Channel<JobEvent>,
) -> Result<JobId, EngineError> {
    // The plan is computed on the engine thread because it needs the page count, but it does
    // no pdfium work of its own and is cheap.
    let plan = {
        let doc_id = doc_id.clone();
        let mode = mode.clone();
        engine
            .call(Lane::Edit, "split_plan", move |st| {
                let doc = st.doc(&doc_id)?;
                pages::split_plan(&mode, doc.page_count(), &pages::output_stem(doc))
            })
            .await?
    };

    let token = engine.jobs.create();
    let reporter = JobReporter::start(
        on_progress,
        engine.jobs.clone(),
        token.id,
        plan.len() as u32,
    );
    let out_dir = PathBuf::from(out_dir);

    for (position, part) in plan.into_iter().enumerate() {
        let submit = Submit::new(Lane::Background, "split_part")
            .priority(position as u32)
            .cancel(token.cancel.clone());
        let doc_id = doc_id.clone();
        let out_dir = out_dir.clone();
        let reporter_for_job = reporter.clone();
        let dispatched = engine.dispatch(submit, move |st, status| {
            if reporter_for_job.is_finished() {
                return;
            }
            if status != CmdStatus::Run {
                reporter_for_job.cancel();
                return;
            }
            match pages::write_split_part(st, &doc_id, &part, &out_dir) {
                Ok(path) => {
                    reporter_for_job.step(None, Some(path.display().to_string()));
                    reporter_for_job.finish_if_complete();
                }
                Err(e) => reporter_for_job.fail(e),
            }
        });
        if let Err(e) = dispatched {
            reporter.fail(e);
            break;
        }
    }
    Ok(token.id)
}

#[tauri::command]
pub async fn merge_documents(
    engine: State<'_, EngineHandle>,
    inputs: Vec<MergeInput>,
) -> Result<MergeResult, EngineError> {
    engine
        .call(Lane::Edit, "merge_documents", move |st| {
            pages::merge(st, &inputs)
        })
        .await
}
