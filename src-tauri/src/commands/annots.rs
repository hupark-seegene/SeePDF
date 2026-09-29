//! Annotations — `IPC_CONTRACT.md` §7.1. Owner: **Stage 1 (a)**.
//!
//! Thin bodies: every one of them forwards to `engine::annot` on `Lane::Edit`, and every
//! write is wrapped in `registry::mutate` so the undo snapshot, the generation bump, the
//! cache invalidation and `doc-changed` happen exactly once per user action.
//!
//! `set_annotations_hidden` is the single exception — it is a *view* change by contract
//! (§7.1), so it bumps `viewNonce` instead of `docGeneration` and never dirties the document.

use crate::engine::annot;
use crate::engine::registry::{self, MutateOpts};
use crate::engine::types::CmdStatus;
use crate::engine::{EngineHandle, Lane, Submit};
use crate::ipc::types::{
    Annot, AnnotList, AnnotPatch, AnnotResult, AnnotScanEvent, AnnotSpec, ChangeReason, JobId,
    PageIndex, ViewNonce,
};
use crate::ipc::EngineError;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use tauri::ipc::Channel;
use tauri::State;

/// Bumped by `set_annotations_hidden`; the viewer puts it in its tile URLs so the change is
/// visible without a generation bump.
static VIEW_NONCE: AtomicU64 = AtomicU64::new(1);

#[tauri::command]
pub async fn list_annotations(
    engine: State<'_, EngineHandle>,
    doc_id: String,
    page: PageIndex,
) -> Result<AnnotList, EngineError> {
    engine
        .call(Lane::Interactive, "list_annotations", move |st| {
            let doc = st.doc_mut(&doc_id)?;
            let annots = annot::list(doc, page)?;
            Ok(AnnotList {
                doc_id: doc.doc_id.clone(),
                page,
                doc_generation: doc.generation,
                annots,
            })
        })
        .await
}

/// One `Lane::Background` command per page, sharing one `JobToken` so a cancel is observed
/// within one page. Page events stream as they arrive; `done` carries the total.
#[tauri::command]
pub async fn scan_annotations(
    engine: State<'_, EngineHandle>,
    doc_id: String,
    on_event: Channel<AnnotScanEvent>,
) -> Result<JobId, EngineError> {
    let page_count = engine
        .shared
        .doc(&doc_id)
        .ok_or_else(|| EngineError::not_found(format!("unknown document '{doc_id}'")))?
        .page_count;
    let token = engine.jobs.create();
    let total = Arc::new(AtomicU32::new(0));
    let done = Arc::new(AtomicU32::new(0));

    if page_count == 0 {
        let _ = on_event.send(AnnotScanEvent::Done { total: 0 });
        engine.jobs.finish(token.id);
        return Ok(token.id);
    }

    for page in 0..page_count {
        let submit = Submit::new(Lane::Background, "scan_annotations")
            .priority(page as u32)
            .page(page)
            .cancel(token.cancel.clone());
        let doc_id = doc_id.clone();
        let channel = on_event.clone();
        let total = total.clone();
        let done = done.clone();
        let jobs = engine.jobs.clone();
        let job_id = token.id;
        let _ = engine.dispatch(submit, move |st, status| {
            let finished = done.fetch_add(1, Ordering::Relaxed) + 1;
            let last = finished == page_count as u32;
            if status != CmdStatus::Run {
                if last {
                    let _ = channel.send(AnnotScanEvent::Cancelled);
                    jobs.finish(job_id);
                }
                return;
            }
            if let Ok(doc) = st.doc_mut(&doc_id) {
                match annot::list(doc, page) {
                    Ok(annots) if !annots.is_empty() => {
                        total.fetch_add(annots.len() as u32, Ordering::Relaxed);
                        let _ = channel.send(AnnotScanEvent::Page { page, annots });
                    }
                    Ok(_) => {}
                    Err(e) => tracing::debug!(page, error = %e, "annotation scan skipped a page"),
                }
            }
            if last {
                let _ = channel.send(AnnotScanEvent::Done {
                    total: total.load(Ordering::Relaxed),
                });
                jobs.finish(job_id);
            }
        });
    }
    Ok(token.id)
}

/// v0.3 A9: the author is 설정 ▸ 주석 작성자, read here on the command side (never taken
/// from the webview) and written as `/T` on every new annotation, stamp and signature.
/// v0.3 A2: `create_in` picks PDFium or lopdf per kind — still one undo step.
#[tauri::command]
pub async fn create_annotation(
    app: tauri::AppHandle,
    engine: State<'_, EngineHandle>,
    doc_id: String,
    page: PageIndex,
    spec: AnnotSpec,
    id: Option<String>,
) -> Result<AnnotResult, EngineError> {
    let author = crate::app::store::get_settings(&app).author;
    engine
        .call(Lane::Edit, "create_annotation", move |st| {
            let new_id =
                annot::create::create_in(st, &doc_id, page, &spec, id, Some(author.as_str()))?;
            result_for(st, &doc_id, page, Some(new_id), None)
        })
        .await
}

#[tauri::command]
pub async fn update_annotation(
    engine: State<'_, EngineHandle>,
    doc_id: String,
    page: PageIndex,
    id: String,
    patch: AnnotPatch,
) -> Result<AnnotResult, EngineError> {
    engine
        .call(Lane::Edit, "update_annotation", move |st| {
            let previous = annot::update::update_in(st, &doc_id, page, &id, &patch)?;
            result_for(st, &doc_id, page, Some(previous.id.clone()), Some(previous))
        })
        .await
}

#[tauri::command]
pub async fn delete_annotations(
    engine: State<'_, EngineHandle>,
    doc_id: String,
    page: PageIndex,
    ids: Vec<String>,
) -> Result<AnnotResult, EngineError> {
    engine
        .call(Lane::Edit, "delete_annotations", move |st| {
            registry::mutate(
                st,
                &doc_id,
                MutateOpts::new("undo.annotDelete", ChangeReason::Edit)
                    .page(page)
                    .keeps_text(),
                |doc| annot::delete(doc, page, &ids),
            )?;
            result_for(st, &doc_id, page, None, None)
        })
        .await
}

/// P2 threads: a reply to `parent_id` (a `Text` annotation with `/IRT` + `/RT /R`). The
/// reference can only be written on the serialised file, so this goes through
/// `registry::mutate_bytes` (`engine::annot::reply`) — still one undo step, `undo.annotReply`.
#[tauri::command]
pub async fn reply_annotation(
    engine: State<'_, EngineHandle>,
    doc_id: String,
    page: PageIndex,
    parent_id: String,
    contents: String,
    author: Option<String>,
) -> Result<AnnotResult, EngineError> {
    engine
        .call(Lane::Edit, "reply_annotation", move |st| {
            let id =
                annot::reply::reply(st, &doc_id, page, &parent_id, &contents, author.as_deref())?;
            result_for(st, &doc_id, page, Some(id), None)
        })
        .await
}

/// P1. Transient: bumps `viewNonce`, **not** `docGeneration`, and never dirties the document.
#[tauri::command]
pub async fn set_annotations_hidden(
    engine: State<'_, EngineHandle>,
    doc_id: String,
    page: PageIndex,
    ids: Vec<String>,
    hidden: bool,
) -> Result<ViewNonce, EngineError> {
    engine
        .call(Lane::Edit, "set_annotations_hidden", move |st| {
            let doc = st.doc_mut(&doc_id)?;
            annot::set_hidden(doc, page, &ids, hidden)?;
            // The pixels change, so the tiles of this page must not be served from cache.
            st.shared.tiles.drop_document(&doc_id);
            Ok(ViewNonce {
                view_nonce: VIEW_NONCE.fetch_add(1, Ordering::Relaxed) + 1,
            })
        })
        .await
}

/// The `AnnotResult` every mutating command returns: the fresh page list plus the annotation
/// that was touched.
fn result_for(
    st: &mut crate::engine::EngineState<'_>,
    doc_id: &str,
    page: PageIndex,
    id: Option<String>,
    previous: Option<Annot>,
) -> Result<AnnotResult, EngineError> {
    let doc = st.doc_mut(doc_id)?;
    let annots = annot::list(doc, page)?;
    let annot = id
        .as_ref()
        .and_then(|id| annots.iter().find(|a| &a.id == id).cloned());
    Ok(AnnotResult {
        list: AnnotList {
            doc_id: doc.doc_id.clone(),
            page,
            doc_generation: doc.generation,
            annots,
        },
        annot,
        previous,
    })
}
