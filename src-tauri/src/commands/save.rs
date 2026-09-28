//! Save and Save As — `IPC_CONTRACT.md` §7.6. Owner: **Stage 1 (b)**.
//!
//! The sequence is fixed (`ARCHITECTURE.md` §8): render every page in `OpenDoc::touched` at
//! `set_target_width(32)` so pdfium writes the `/AP` streams
//! (`engine::render::tiles::generate_appearances`) → `FORM_ForceToKillFocus` → drop the page
//! LRU → `FPDF_SaveAsCopy(FPDF_NO_INCREMENTAL)` → temp file in the same directory + fsync →
//! verify by reopening → backup → `rename` → `registry::replace`.
//!
//! Save is one engine command, not a job: a 100 MB scan takes ~0.5 s and the channel exists so
//! the status bar can show a spinner, not so the work can be cancelled half-way through a
//! file rewrite.

use crate::engine::{save, EngineHandle, Lane};
use crate::ipc::types::{JobEvent, SaveResult};
use crate::ipc::EngineError;
use tauri::ipc::Channel;
use tauri::State;

#[tauri::command]
pub async fn save_document(
    engine: State<'_, EngineHandle>,
    doc_id: String,
    on_progress: Channel<JobEvent>,
) -> Result<SaveResult, EngineError> {
    run_save(engine, doc_id, None, on_progress).await
}

#[tauri::command]
pub async fn save_document_as(
    engine: State<'_, EngineHandle>,
    doc_id: String,
    path: String,
    on_progress: Channel<JobEvent>,
) -> Result<SaveResult, EngineError> {
    run_save(engine, doc_id, Some(path), on_progress).await
}

/// `true` when something already exists at `path` (P1-7 여러 파일 OCR).
///
/// Batch OCR writes `<name>-ocr.pdf` and must never overwrite anything, so the frontend asks
/// before it picks a name and moves on to `<name>-ocr (2).pdf` when the answer is yes. A path
/// whose existence cannot be determined (permission denied on the directory) counts as taken.
#[tauri::command]
pub async fn path_exists(path: String) -> Result<bool, EngineError> {
    Ok(exists(&path))
}

fn exists(path: &str) -> bool {
    std::path::Path::new(path).try_exists().unwrap_or(true)
}

async fn run_save(
    engine: State<'_, EngineHandle>,
    doc_id: String,
    path: Option<String>,
    on_progress: Channel<JobEvent>,
) -> Result<SaveResult, EngineError> {
    let token = engine.jobs.create();
    let job_id = token.id;
    let _ = on_progress.send(JobEvent::Started { job_id, total: 1 });
    let result = engine
        .call(Lane::Edit, "save_document", move |st| {
            save::save(st, &doc_id, path.as_deref(), true)
        })
        .await;
    engine.jobs.finish(job_id);
    match &result {
        Ok(saved) => {
            let _ = on_progress.send(JobEvent::Done {
                job_id,
                elapsed_ms: saved.elapsed_ms,
                outputs: Some(vec![saved.path.clone()]),
                report: None,
                compare: None,
            });
        }
        Err(error) => {
            let _ = on_progress.send(JobEvent::Error {
                job_id,
                error: error.clone(),
            });
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::exists;

    #[test]
    fn path_exists_reports_files_and_free_names() {
        let dir = std::env::temp_dir().join(format!("seepdf-path-exists-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let taken = dir.join("스캔-ocr.pdf");
        std::fs::write(&taken, b"%PDF-1.7").unwrap();
        assert!(exists(&taken.display().to_string()));
        assert!(
            exists(&dir.display().to_string()),
            "a directory is taken too"
        );
        assert!(!exists(&dir.join("스캔-ocr (2).pdf").display().to_string()));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
