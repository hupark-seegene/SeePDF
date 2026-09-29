//! Text layer, page text, search and job cancellation — `IPC_CONTRACT.md` §6. Owner: Stage 0.

use crate::engine::text::{layer, search, serialize, structtree, weblinks};
use crate::engine::types::CmdStatus;
use crate::engine::{EngineHandle, Lane, Submit};
use crate::ipc::types::{JobId, PageIndex, ReadingOrder, SearchEvent, WebLink};
use crate::ipc::EngineError;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Instant;
use tauri::ipc::Channel;
use tauri::State;

/// The first pages of a search go on `Lane::Interactive` so the panel fills immediately.
const INTERACTIVE_PAGES: usize = 4;

#[tauri::command]
pub async fn get_text_layer(
    engine: State<'_, EngineHandle>,
    doc_id: String,
    page: PageIndex,
) -> Result<tauri::ipc::Response, EngineError> {
    let buffer = engine
        .call(Lane::Interactive, "get_text_layer", move |st| {
            let doc = st.doc_mut(&doc_id)?;
            let layer = layer::layer(doc, page)?;
            Ok(serialize::serialize(&layer))
        })
        .await?;
    Ok(tauri::ipc::Response::new(buffer))
}

#[tauri::command]
pub async fn get_page_text(
    engine: State<'_, EngineHandle>,
    doc_id: String,
    page: PageIndex,
) -> Result<String, EngineError> {
    engine
        .call(Lane::Interactive, "get_page_text", move |st| {
            let doc = st.doc_mut(&doc_id)?;
            Ok(layer::page_text(doc, page)?.text.clone())
        })
        .await
}

/// v0.3 (V5): the URLs PDFium detects in the page text (`FPDFLink_LoadWebLinks`), for the
/// 읽기-mode link layer. Read-only; nothing is written to the document.
#[tauri::command]
pub async fn get_web_links(
    engine: State<'_, EngineHandle>,
    doc_id: String,
    page: PageIndex,
) -> Result<Vec<WebLink>, EngineError> {
    engine
        .call(Lane::Interactive, "get_web_links", move |st| {
            let doc = st.doc_mut(&doc_id)?;
            weblinks::web_links(doc, page)
        })
        .await
}

/// v0.3 (V6): the page's text-layer runs in reading order — the structure tree's MCID order
/// on a tagged page, content order otherwise. Read aloud and the screen-reader region use it.
#[tauri::command]
pub async fn get_reading_order(
    engine: State<'_, EngineHandle>,
    doc_id: String,
    page: PageIndex,
) -> Result<ReadingOrder, EngineError> {
    engine
        .call(Lane::Interactive, "get_reading_order", move |st| {
            let doc = st.doc_mut(&doc_id)?;
            structtree::reading_order(doc, page)
        })
        .await
}

/// One `Lane::Background` command per page, visiting outward from `fromPage`, all sharing one
/// `JobToken`; hits stream over the channel as they arrive (`ARCHITECTURE.md` §5).
#[tauri::command]
pub async fn search_start(
    engine: State<'_, EngineHandle>,
    doc_id: String,
    query: String,
    match_case: bool,
    whole_word: bool,
    from_page: PageIndex,
    on_event: Channel<SearchEvent>,
) -> Result<JobId, EngineError> {
    let Some(parsed) = search::Query::new(&query, match_case, whole_word) else {
        return Err(EngineError::invalid("query is empty"));
    };
    let page_count = engine
        .shared
        .doc(&doc_id)
        .ok_or_else(|| EngineError::not_found(format!("unknown document '{doc_id}'")))?
        .page_count;
    let order = search::visit_order(page_count, from_page);
    let total = order.len() as u32;
    let token = engine.jobs.create();

    let state = Arc::new(SearchProgress {
        scanned: AtomicU32::new(0),
        hits: AtomicU32::new(0),
        finished: AtomicBool::new(false),
        total,
        started: Instant::now(),
    });

    if total == 0 {
        finish(&on_event, &state, false);
        engine.jobs.finish(token.id);
        return Ok(token.id);
    }

    for (position, page) in order.into_iter().enumerate() {
        let lane = if position < INTERACTIVE_PAGES {
            Lane::Interactive
        } else {
            Lane::Background
        };
        let submit = Submit::new(lane, "search_page")
            .priority(position as u32)
            .page(page)
            .cancel(token.cancel.clone());
        let doc_id = doc_id.clone();
        let parsed = parsed.clone();
        let channel = on_event.clone();
        let state = state.clone();
        let jobs = engine.jobs.clone();
        let job_id = token.id;
        let dispatched = engine.dispatch(submit, move |st, status| {
            if status != CmdStatus::Run {
                if state.finish_once() {
                    let _ = channel.send(SearchEvent::Cancelled {
                        scanned: state.scanned.load(Ordering::Relaxed),
                    });
                    jobs.finish(job_id);
                }
                return;
            }
            let result = st
                .doc_mut(&doc_id)
                .and_then(|doc| search::search_page(doc, page, &parsed));
            let scanned = state.scanned.fetch_add(1, Ordering::Relaxed) + 1;
            match result {
                Ok(hits) => {
                    let found = hits.len() as u32;
                    state.hits.fetch_add(found, Ordering::Relaxed);
                    if found > 0 {
                        let _ = channel.send(SearchEvent::Page {
                            page,
                            hits,
                            scanned,
                            total: state.total,
                        });
                    }
                }
                Err(error) => {
                    let _ = channel.send(SearchEvent::Error { error });
                }
            }
            if scanned >= state.total && state.finish_once() {
                let _ = channel.send(SearchEvent::Done {
                    total: state.hits.load(Ordering::Relaxed),
                    pages_scanned: scanned,
                    elapsed_ms: state.started.elapsed().as_secs_f64() * 1000.0,
                });
                jobs.finish(job_id);
            }
        });
        if let Err(e) = dispatched {
            let _ = on_event.send(SearchEvent::Error { error: e });
            break;
        }
    }
    Ok(token.id)
}

#[tauri::command]
pub fn cancel_job(engine: State<'_, EngineHandle>, job_id: JobId) -> bool {
    engine.jobs.cancel(job_id)
}

struct SearchProgress {
    scanned: AtomicU32,
    hits: AtomicU32,
    finished: AtomicBool,
    total: u32,
    started: Instant,
}

impl SearchProgress {
    /// Exactly one of `done` / `cancelled` is ever sent, whichever command gets there first.
    fn finish_once(&self) -> bool {
        !self.finished.swap(true, Ordering::SeqCst)
    }
}

fn finish(channel: &Channel<SearchEvent>, state: &SearchProgress, cancelled: bool) {
    if !state.finish_once() {
        return;
    }
    let event = if cancelled {
        SearchEvent::Cancelled {
            scanned: state.scanned.load(Ordering::Relaxed),
        }
    } else {
        SearchEvent::Done {
            total: state.hits.load(Ordering::Relaxed),
            pages_scanned: state.scanned.load(Ordering::Relaxed),
            elapsed_ms: state.started.elapsed().as_secs_f64() * 1000.0,
        }
    };
    let _ = channel.send(event);
}
