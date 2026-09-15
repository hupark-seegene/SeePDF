//! Export and print — `IPC_CONTRACT.md` §7.7. Owner: **Stage 1 (b)**.
//!
//! Flattening uses raw `FPDFPage_Flatten(page, FLAT_NORMALDISPLAY)` + `FPDFPage_GenerateContent`
//! + a page reload on a scratch copy — never `PdfPage::flatten()`, which is `FLAT_PRINT` and
//! silently deletes annotations without the Print flag.

use crate::engine::EngineHandle;
use crate::ipc::types::{
    ExportEstimate, ExportImagesArgs, ExportTextResult, ImageFormat, JobEvent, JobId, PageIndex,
    PrintPrepareResult,
};
use crate::ipc::EngineError;
use tauri::ipc::Channel;
use tauri::State;

#[tauri::command]
pub async fn export_images(
    _engine: State<'_, EngineHandle>,
    _args: ExportImagesArgs,
    _on_progress: Channel<JobEvent>,
) -> Result<JobId, EngineError> {
    Err(EngineError::unsupported("export_images"))
}

#[tauri::command]
pub async fn export_text(
    _engine: State<'_, EngineHandle>,
    _doc_id: String,
    _pages: Vec<PageIndex>,
    _out_path: String,
) -> Result<ExportTextResult, EngineError> {
    Err(EngineError::unsupported("export_text"))
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn export_flattened(
    _engine: State<'_, EngineHandle>,
    _doc_id: String,
    _out_path: String,
    _annotations: bool,
    _forms: bool,
    _pages: Option<Vec<PageIndex>>,
    _on_progress: Channel<JobEvent>,
) -> Result<JobId, EngineError> {
    Err(EngineError::unsupported("export_flattened"))
}

#[tauri::command]
pub async fn estimate_export(
    _engine: State<'_, EngineHandle>,
    _doc_id: String,
    _pages: Vec<PageIndex>,
    _format: ImageFormat,
    _dpi: u32,
) -> Result<ExportEstimate, EngineError> {
    Err(EngineError::unsupported("estimate_export"))
}

#[tauri::command]
pub async fn print_prepare(
    _engine: State<'_, EngineHandle>,
    _doc_id: String,
    _pages: Option<Vec<PageIndex>>,
) -> Result<PrintPrepareResult, EngineError> {
    Err(EngineError::unsupported("print_prepare"))
}
