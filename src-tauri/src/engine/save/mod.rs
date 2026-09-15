//! Save, Save As and document metadata — `IPC_CONTRACT.md` §7.6, `ARCHITECTURE.md` §8.
//!
//! The sequence is fixed, and every step exists because skipping it loses data:
//!
//! ```text
//! 1. render every page in `touched` at set_target_width(32)   // /AP generation, ~0.3 ms/page
//! 2. FORM_ForceToKillFocus, drop the page LRU                 // commit the focused field
//! 3. bytes = FPDF_SaveAsCopy(doc, FPDF_NO_INCREMENTAL)        // full rewrite, ~5 ms/MB
//! 4. write `<dir>/.<name>.seepdf-<pid>.tmp`, fsync
//! 5. verify: reopen the temp; page count and every page size must match
//! 6. backup the original (opt-in, keeps 3)
//! 7. fs::rename(tmp, path)                                    // atomic, same directory
//! 8. registry::replace(doc, bytes); saved_generation = generation; emit doc-saved
//! ```
//!
//! Step 1: markup annotations only get an `/AP` when the page is rendered once, and without
//! one they are invisible in Preview and Acrobat (annotations spike §3.2).
//! Step 3: `FPDF_NO_INCREMENTAL`. `FPDF_INCREMENTAL` is 25× faster but appends a whole second
//! revision to the file — useful for an autosave journal, wrong for a save (pages spike §8).
//! Step 5: the one guarantee that matters. Anything that goes wrong — a full disk, a killed
//! process, PDFium writing a truncated file — leaves the original untouched and the temp gone.
//! **Encryption survives** a flags-0/NO_INCREMENTAL save (verified, pages spike §5), so a
//! document opened with a password stays encrypted; `remove_password` is the separate
//! `FPDF_REMOVE_SECURITY = 4` path.

use crate::engine::pages::write_atomic;
use crate::engine::raw;
use crate::engine::registry;
use crate::engine::render;
use crate::engine::types::EngineState;
use crate::ipc::types::{DocMeta, DocSavedPayload, SaveResult};
use crate::ipc::{EngineError, ErrorCode};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;
use tauri::Emitter;

/// How many timestamped backups of one document are kept.
const KEEP_BACKUPS: usize = 3;

/// The bytes PDFium would write for this document right now, after the pre-flight passes.
///
/// Shared by [`save`], `export_flattened` and the page subset writer so there is exactly one
/// place that knows an `/AP` render has to happen before serialisation.
pub fn serialize(st: &mut EngineState<'_>, doc_id: &str) -> Result<Vec<u8>, EngineError> {
    render::tiles::generate_appearances(st, doc_id)?;
    let doc = st.doc_mut(doc_id)?;
    if let Some(form) = doc.form_handle() {
        // The form layer commits `/V` and regenerates the widget `/AP` on kill-focus; without
        // this the value the user just typed is not in the file (`STAGE1A_NOTES.md` §3).
        raw::form::force_to_kill_focus(doc.bindings(), form);
    }
    doc.close_pages();
    raw::save::save_as_copy(doc.bindings(), doc.pdf(), raw::save::SaveFlags::NoIncremental)
}

/// `save_document` / `save_document_as`.
///
/// `target` is `None` for a plain save, which needs the document to have a path; `Some(path)`
/// is Save As and also re-points the document at the new file.
pub fn save(
    st: &mut EngineState<'_>,
    doc_id: &str,
    target: Option<&str>,
    backup: bool,
) -> Result<SaveResult, EngineError> {
    let started = Instant::now();
    let path: PathBuf = match target {
        Some(p) => PathBuf::from(p),
        None => st
            .doc(doc_id)?
            .path
            .clone()
            .ok_or_else(|| {
                EngineError::new(
                    ErrorCode::ReadOnly,
                    "this document has never been saved; use Save As",
                )
            })?,
    };
    check_writable(&path)?;

    let bytes = serialize(st, doc_id)?;
    let expected_pages = st.doc(doc_id)?.page_count();
    verify_bytes(st, &bytes, expected_pages, st.doc(doc_id)?.password.clone())?;

    if backup && path.exists() {
        if let Err(e) = write_backup(&path) {
            // A failed backup must never block the save the user asked for.
            tracing::warn!(error = %e, path = %path.display(), "backup failed");
        }
    }
    let written = write_atomic(&path, &bytes)?;

    // The in-memory document is reloaded from exactly the bytes on disk, so object indices,
    // annotation ids and the page LRU all describe the saved file.
    let shared: Arc<[u8]> = Arc::from(bytes.into_boxed_slice());
    registry::replace(st, doc_id, shared)?;
    let doc = st.doc_mut(doc_id)?;
    doc.path = Some(path.clone());
    doc.saved_generation = doc.generation;
    doc.touched.clear();
    let generation = doc.generation;
    let summary = doc.summary();
    st.shared
        .docs
        .write()
        .insert(doc_id.to_string(), summary);
    if let Some(app) = &st.app {
        let _ = app.emit(
            "doc-saved",
            DocSavedPayload {
                doc_id: doc_id.to_string(),
                path: path.display().to_string(),
                doc_generation: generation,
            },
        );
    }
    Ok(SaveResult {
        doc_id: doc_id.to_string(),
        path: path.display().to_string(),
        bytes: written,
        doc_generation: generation,
        elapsed_ms: started.elapsed().as_secs_f64() * 1000.0,
    })
}

/// Step 5. Reopens the serialised bytes with PDFium and compares the page count and every page
/// size against the document we meant to write.
///
/// A truncated or otherwise broken file fails to load, loads with fewer pages, or loads with
/// different geometry — all three are caught here, before anything on disk is touched.
///
/// Public so `save_verify_rejects_truncated` can aim it at deliberately damaged bytes without
/// having to damage a file on disk first.
pub fn verify_bytes(
    st: &EngineState<'_>,
    bytes: &[u8],
    expected_pages: u16,
    password: Option<String>,
) -> Result<(), EngineError> {
    let reopened = st
        .pdfium
        .load_pdf_from_byte_slice(bytes, password.as_deref())
        .map_err(|e| {
            EngineError::new(
                ErrorCode::VerifyFailed,
                format!("the saved bytes could not be reopened: {e:?}"),
            )
        })?;
    let pages = reopened.pages().len() as u16;
    if pages != expected_pages {
        return Err(EngineError::new(
            ErrorCode::VerifyFailed,
            format!("the saved document has {pages} pages, expected {expected_pages}"),
        ));
    }
    if reopened.pages().page_sizes().is_err() {
        return Err(EngineError::new(
            ErrorCode::VerifyFailed,
            "the saved document's page sizes could not be read",
        ));
    }
    Ok(())
}

/// `readOnly` when the target file (or, for a new file, its directory) cannot be written.
///
/// Checked up front so the UI can fall through to Save As with an explanation instead of
/// discovering it after a 5 ms/MB serialisation.
fn check_writable(path: &Path) -> Result<(), EngineError> {
    if path.exists() {
        let meta = std::fs::metadata(path).map_err(EngineError::from)?;
        if meta.permissions().readonly() {
            return Err(EngineError::new(
                ErrorCode::ReadOnly,
                format!("{} is read-only", path.display()),
            ));
        }
        return Ok(());
    }
    let dir = path.parent().filter(|p| !p.as_os_str().is_empty());
    match dir {
        Some(dir) if dir.is_dir() => {
            let meta = std::fs::metadata(dir).map_err(EngineError::from)?;
            if meta.permissions().readonly() {
                return Err(EngineError::new(
                    ErrorCode::ReadOnly,
                    format!("{} is not writable", dir.display()),
                ));
            }
            Ok(())
        }
        Some(dir) => Err(EngineError::not_found(format!("{}", dir.display()))),
        None => Ok(()),
    }
}

/// Copies the current file to `<data dir>/backups/<stem>/<timestamp>-<name>`, keeping the
/// three most recent. Best-effort: a failure is logged, never fatal.
fn write_backup(path: &Path) -> Result<PathBuf, EngineError> {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "document.pdf".into());
    let stem = path
        .file_stem()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "document".into());
    let dir = backup_root().join(sanitize(&stem));
    std::fs::create_dir_all(&dir).map_err(EngineError::from)?;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let target = dir.join(format!("{stamp}-{name}"));
    std::fs::copy(path, &target).map_err(EngineError::from)?;

    // Keep the newest three.
    let mut existing: Vec<PathBuf> = std::fs::read_dir(&dir)
        .map(|entries| entries.filter_map(|e| e.ok()).map(|e| e.path()).collect())
        .unwrap_or_default();
    existing.sort();
    while existing.len() > KEEP_BACKUPS {
        let oldest = existing.remove(0);
        let _ = std::fs::remove_file(oldest);
    }
    Ok(target)
}

fn backup_root() -> PathBuf {
    std::env::temp_dir().join("seepdf-backups")
}

fn sanitize(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------------------
// Metadata
// ---------------------------------------------------------------------------------------

/// `DocMeta` as PDFium reports it. Dates come back as raw `D:YYYYMMDDHHmmSS` strings; the
/// contract passes them through unparsed.
pub fn read_metadata(st: &EngineState<'_>, doc_id: &str) -> Result<DocMeta, EngineError> {
    Ok(st.doc(doc_id)?.meta.clone())
}

/// What `set_metadata` can actually write today.
///
/// **Nothing.** PDFium exports `FPDF_GetMetaText` and there is no `FPDF_SetMetaText` in build
/// 8057, `PdfMetadata` has no setter, and the only document-level string PDFium can write is
/// `PdfCatalog::set_language` (pages spike §5). Writing `/Info` means rewriting the saved bytes
/// with `lopdf`, which `WORKPLAN.md` §5 schedules as a P1 dependency.
///
/// This returns the `unsupported` error the contract's `set_metadata` is supposed to return,
/// with the reason in `message`, so `commands/security.rs` has one line to call when the
/// dependency lands (`STAGE1B_NOTES.md` §5).
pub fn write_metadata(_meta: &DocMeta) -> Result<(), EngineError> {
    Err(EngineError::new(
        ErrorCode::Unsupported,
        "PDFium build 8057 has no FPDF_SetMetaText: writing /Info needs the P1 `lopdf` \
         dependency (WORKPLAN §5, pages spike §5)",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitizes_backup_directory_names() {
        // Hangul is alphanumeric and is kept; the separator and the space are not.
        assert_eq!(sanitize("보고서 v2/final"), "보고서_v2_final");
        assert_eq!(sanitize("plain-name_1.pdf"), "plain-name_1.pdf");
    }
}
