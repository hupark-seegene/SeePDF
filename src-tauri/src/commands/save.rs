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
use tauri::{AppHandle, State};

/// `force` (v0.3 H8): overwrite even when the file changed on disk since it was opened —
/// the answer to the `fileChangedOnDisk` prompt's 덮어쓰기.
#[tauri::command]
pub async fn save_document(
    app: AppHandle,
    engine: State<'_, EngineHandle>,
    doc_id: String,
    force: Option<bool>,
    on_progress: Channel<JobEvent>,
) -> Result<SaveResult, EngineError> {
    run_save(
        app,
        engine,
        doc_id,
        None,
        force.unwrap_or(false),
        on_progress,
    )
    .await
}

#[tauri::command]
pub async fn save_document_as(
    app: AppHandle,
    engine: State<'_, EngineHandle>,
    doc_id: String,
    path: String,
    on_progress: Channel<JobEvent>,
) -> Result<SaveResult, EngineError> {
    run_save(app, engine, doc_id, Some(path), false, on_progress).await
}

/// v0.3 U2: the folder backups are written to (`<app data>/backups`), created if missing —
/// for Settings › 고급 › 백업 폴더 열기.
#[tauri::command]
pub fn backup_folder(app: AppHandle) -> Result<String, EngineError> {
    let dir = crate::app::store::backups_dir(&app)
        .ok_or_else(|| EngineError::io("no app data directory"))?;
    std::fs::create_dir_all(&dir).map_err(EngineError::from)?;
    Ok(dir.display().to_string())
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

/// At most this many files come back from one folder (여러 파일에서 검색 is a search over a
/// project folder, not over a disk).
pub const MAX_LISTED_PDFS: usize = 2000;
/// How deep a recursive listing goes.
const MAX_LIST_DEPTH: usize = 16;

/// P2 여러 파일에서 검색 › 폴더 추가: every `*.pdf` (any case) under `dir`, sorted by path.
///
/// File-system only, on a blocking thread — no PDFium. Hidden entries (`.name`) are skipped and
/// symbolic links are not followed (a link cycle would never end). `recursive` defaults to
/// true; at most [`MAX_LISTED_PDFS`] paths, [`MAX_LIST_DEPTH`] levels. An unreadable
/// sub-directory is skipped; an unreadable `dir` itself is an `io` error.
#[tauri::command]
pub async fn list_pdf_files(
    dir: String,
    recursive: Option<bool>,
) -> Result<Vec<String>, EngineError> {
    let recursive = recursive.unwrap_or(true);
    tauri::async_runtime::spawn_blocking(move || list_pdfs(std::path::Path::new(&dir), recursive))
        .await
        .map_err(|e| EngineError::io(format!("list folder: {e}")))?
}

pub fn list_pdfs(dir: &std::path::Path, recursive: bool) -> Result<Vec<String>, EngineError> {
    let mut out: Vec<String> = Vec::new();
    let mut stack: Vec<(std::path::PathBuf, usize)> = vec![(dir.to_path_buf(), 0)];
    let mut first = true;
    while let Some((path, depth)) = stack.pop() {
        let entries = match std::fs::read_dir(&path) {
            Ok(entries) => entries,
            Err(e) if first => return Err(EngineError::from(e)),
            Err(_) => continue,
        };
        first = false;
        for entry in entries.flatten() {
            let name = entry.file_name();
            if name.to_string_lossy().starts_with('.') {
                continue;
            }
            // `file_type` does not follow symbolic links.
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            let child = entry.path();
            if kind.is_dir() {
                if recursive && depth + 1 < MAX_LIST_DEPTH {
                    stack.push((child, depth + 1));
                }
            } else if kind.is_file()
                && child
                    .extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case("pdf"))
            {
                out.push(child.display().to_string());
                if out.len() >= MAX_LISTED_PDFS {
                    out.sort();
                    return Ok(out);
                }
            }
        }
    }
    out.sort();
    Ok(out)
}

async fn run_save(
    app: AppHandle,
    engine: State<'_, EngineHandle>,
    doc_id: String,
    path: Option<String>,
    force: bool,
    on_progress: Channel<JobEvent>,
) -> Result<SaveResult, EngineError> {
    // Progress only: a save is one atomic rewrite and cannot stop half-way, so its id is not
    // registered for `cancel_job` (which answers `false` for it).
    let job_id = engine.jobs.progress_only();
    let _ = on_progress.send(JobEvent::Started { job_id, total: 1 });
    // v0.3 U2: Settings › 저장 시 백업 decides, and backups live in the app data folder.
    let backup_root = crate::app::store::get_settings(&app)
        .backups_enabled
        .then(|| crate::app::store::backups_dir(&app))
        .flatten();
    let options = save::SaveOptions { backup_root, force };
    let result = engine
        .call(Lane::Edit, "save_document", move |st| {
            save::save_with(st, &doc_id, path.as_deref(), &options)
        })
        .await;
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
    use super::{exists, list_pdfs};

    #[test]
    fn list_pdf_files_recurses_sorts_and_skips_hidden() {
        let dir = std::env::temp_dir().join(format!("seepdf-list-pdfs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("하위/깊은")).unwrap();
        std::fs::create_dir_all(dir.join(".숨김")).unwrap();
        for (name, body) in [
            ("b.pdf", "%PDF"),
            ("A.PDF", "%PDF"),
            ("메모.txt", "x"),
            ("하위/계약서.pdf", "%PDF"),
            ("하위/깊은/c.pdf", "%PDF"),
            (".숨김/d.pdf", "%PDF"),
            (".e.pdf", "%PDF"),
        ] {
            std::fs::write(dir.join(name), body).unwrap();
        }
        let all = list_pdfs(&dir, true).unwrap();
        let rel: Vec<String> = all
            .iter()
            .map(|p| {
                p.strip_prefix(&dir.display().to_string())
                    .unwrap()
                    .trim_start_matches(['/', '\\'])
                    .replace('\\', "/")
            })
            .collect();
        assert_eq!(
            rel,
            vec!["A.PDF", "b.pdf", "하위/계약서.pdf", "하위/깊은/c.pdf"]
        );
        let flat = list_pdfs(&dir, false).unwrap();
        assert_eq!(flat.len(), 2);
        assert!(list_pdfs(&dir.join("없음"), true).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

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
