//! Page operations — `IPC_CONTRACT.md` §7.3. Owner: **Stage 1 (b)**.
//!
//! One `page_ops` call is one `registry::mutate` with `MutateOpts::structural()` = one
//! generation = one undo step. Extract and split **copy the original and delete the other
//! pages**; `FPDF_ImportPages*` keeps the widgets but drops `/AcroForm`, the outline and the
//! metadata (pages spike §2). Merge of unrelated files is the one place import is used.

use crate::engine::EngineHandle;
use crate::ipc::types::{
    DocInfo, ExtractPagesResult, JobEvent, JobId, MergeInput, MergeResult, PageIndex, PageOp,
    SplitMode,
};
use crate::ipc::EngineError;
use tauri::ipc::Channel;
use tauri::State;

#[tauri::command]
pub async fn page_ops(
    _engine: State<'_, EngineHandle>,
    _doc_id: String,
    _ops: Vec<PageOp>,
) -> Result<DocInfo, EngineError> {
    Err(EngineError::unsupported("page_ops"))
}

#[tauri::command]
pub async fn extract_pages(
    _engine: State<'_, EngineHandle>,
    _doc_id: String,
    _pages: Vec<PageIndex>,
    _out_path: String,
    _remove_after: bool,
) -> Result<ExtractPagesResult, EngineError> {
    Err(EngineError::unsupported("extract_pages"))
}

#[tauri::command]
pub async fn split_document(
    _engine: State<'_, EngineHandle>,
    _doc_id: String,
    _mode: SplitMode,
    _out_dir: String,
    _on_progress: Channel<JobEvent>,
) -> Result<JobId, EngineError> {
    Err(EngineError::unsupported("split_document"))
}

#[tauri::command]
pub async fn merge_documents(
    _engine: State<'_, EngineHandle>,
    _inputs: Vec<MergeInput>,
) -> Result<MergeResult, EngineError> {
    Err(EngineError::unsupported("merge_documents"))
}
