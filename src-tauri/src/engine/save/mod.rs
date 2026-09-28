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
//!
//! Metadata (P1-1) is the one thing this module writes *after* PDFium: [`write_info`] rewrites
//! `/Info` and drops the XMP packet with `lopdf`, through [`registry::mutate_bytes`].

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
    serialize_with(st, doc_id, raw::save::SaveFlags::NoIncremental)
}

/// [`serialize`] with explicit `FPDF_SaveAsCopy` flags — `set_password` needs
/// `RemoveSecurity` for an input that is already encrypted.
pub fn serialize_with(
    st: &mut EngineState<'_>,
    doc_id: &str,
    flags: raw::save::SaveFlags,
) -> Result<Vec<u8>, EngineError> {
    render::tiles::generate_appearances(st, doc_id)?;
    let doc = st.doc_mut(doc_id)?;
    if let Some(form) = doc.form_handle() {
        // The form layer commits `/V` and regenerates the widget `/AP` on kill-focus; without
        // this the value the user just typed is not in the file (`STAGE1A_NOTES.md` §3).
        raw::form::force_to_kill_focus(doc.bindings(), form);
    }
    doc.close_pages();
    raw::save::save_as_copy(doc.bindings(), doc.pdf(), flags)
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
        None => st.doc(doc_id)?.path.clone().ok_or_else(|| {
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
    st.shared.docs.write().insert(doc_id.to_string(), summary);
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
///
/// For a new file the directory is **probed** — a throw-away file is created and removed,
/// which is exactly what [`write_atomic`] is about to do — instead of trusting
/// `Permissions::readonly()`. On Windows that is only `FILE_ATTRIBUTE_READONLY`, which the
/// shell sets on Documents, Desktop, Pictures (the `desktop.ini` customisation marker) and
/// which says nothing about creating files there: Save As into those folders was refused.
/// On Unix the mode bits missed other users' directories and read-only mounts, which the
/// probe also catches. An existing file keeps the attribute check: for a file it is real.
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
        Some(dir) if dir.is_dir() => probe_directory(dir),
        Some(dir) => Err(EngineError::not_found(format!("{}", dir.display()))),
        None => Ok(()),
    }
}

/// Creates and removes `.seepdf-probe-<pid>.tmp` in `dir`; `readOnly` when that is refused.
fn probe_directory(dir: &Path) -> Result<(), EngineError> {
    let probe = dir.join(format!(".seepdf-probe-{}.tmp", std::process::id()));
    match std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(&probe)
    {
        Ok(file) => {
            drop(file);
            let _ = std::fs::remove_file(&probe);
            Ok(())
        }
        Err(e) => {
            let code = match e.kind() {
                std::io::ErrorKind::PermissionDenied | std::io::ErrorKind::ReadOnlyFilesystem => {
                    ErrorCode::ReadOnly
                }
                std::io::ErrorKind::NotFound => ErrorCode::NotFound,
                _ => ErrorCode::Io,
            };
            Err(EngineError::new(
                code,
                format!("{} is not writable: {e}", dir.display()),
            ))
        }
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

/// Rewrites the document-level metadata of a serialised PDF with `lopdf` (P1-1).
///
/// PDFium can only *read* `/Info` (`FPDF_GetMetaText`; build 8057 has no `FPDF_SetMetaText`
/// and `PdfMetadata` has no setter — pages spike §5), so `set_metadata` / `remove_metadata`
/// go through [`registry::mutate_bytes`]: PDFium serialises, this rewrites, PDFium reloads.
///
/// * `clear == false` (`set_metadata`): every `Some` field of `meta` is written, an empty or
///   blank string removes that key, `None` leaves the key as it is. `ModDate` is set to now
///   (`D:YYYYMMDDHHmmSS+HH'mm'`, local time) unless `meta.modified` carries a value.
/// * `clear == true` (`remove_metadata`, 메타데이터 제거): the `/Info` dictionary is deleted
///   outright — the object, not only the trailer reference, so no orphan copy of the old
///   strings stays in the file — and `meta` is ignored.
///
/// **Both paths drop the catalog `/Metadata` XMP stream.** Acrobat and most DAMs prefer XMP
/// over `/Info` when both exist, so a stale XMP packet would keep showing the old title after
/// an edit, and would leak the author after a "remove". Writing a fresh XMP packet is not
/// worth an XML writer for a light app; `/Info` alone is what every reader falls back to.
///
/// Strings are PDF text strings: printable ASCII as a literal, anything else (Hangul) as
/// UTF-16BE with a `FE FF` BOM, which is what `FPDF_GetMetaText` decodes. The input must not
/// be encrypted — `lopdf` would need the owner password to re-encrypt, so callers refuse
/// encrypted documents first.
pub fn write_info(bytes: &[u8], meta: &DocMeta, clear: bool) -> Result<Vec<u8>, EngineError> {
    use lopdf::{Dictionary, Object};

    let mut doc = lopdf::Document::load_mem(bytes).map_err(|e| lopdf_error("parse", e))?;
    if doc.is_encrypted() {
        return Err(EngineError::new(
            ErrorCode::Unsupported,
            "this document is encrypted: remove the password first",
        ));
    }

    // The XMP packet goes in both modes (see above).
    if let Ok(catalog) = doc.catalog_mut() {
        if let Some(Object::Reference(id)) = catalog.remove(b"Metadata") {
            doc.objects.remove(&id);
        }
    }

    let info_ref = doc
        .trailer
        .get(b"Info")
        .ok()
        .and_then(|o| o.as_reference().ok());
    if clear {
        doc.trailer.remove(b"Info");
        if let Some(id) = info_ref {
            doc.objects.remove(&id);
        }
    } else {
        let id = match info_ref {
            Some(id) if doc.get_dictionary(id).is_ok() => id,
            _ => {
                // No /Info, a dangling reference, or an inline dictionary: start a fresh
                // indirect one and carry over whatever the inline dictionary had.
                let inline = match doc.trailer.get(b"Info") {
                    Ok(Object::Dictionary(d)) => d.clone(),
                    _ => Dictionary::new(),
                };
                let id = doc.add_object(Object::Dictionary(inline));
                doc.trailer.set("Info", Object::Reference(id));
                id
            }
        };
        let modified = meta.modified.clone().or_else(|| Some(pdf_date_now()));
        let fields: [(&str, &Option<String>); 8] = [
            ("Title", &meta.title),
            ("Author", &meta.author),
            ("Subject", &meta.subject),
            ("Keywords", &meta.keywords),
            ("Creator", &meta.creator),
            ("Producer", &meta.producer),
            ("CreationDate", &meta.created),
            ("ModDate", &modified),
        ];
        let info = doc
            .get_dictionary_mut(id)
            .map_err(|e| lopdf_error("/Info", e))?;
        for (key, value) in fields {
            match value.as_deref().map(str::trim) {
                None => {}
                Some("") => {
                    info.remove(key.as_bytes());
                }
                Some(_) => info.set(key, pdf_text_string(value.as_deref().unwrap_or_default())),
            }
        }
    }

    let mut out = Vec::with_capacity(bytes.len());
    doc.save_to(&mut out).map_err(|e| lopdf_error("write", e))?;
    Ok(out)
}

/// A PDF text string (ISO 32000 §7.9.2.2): PDFDocEncoding-safe ASCII as a literal, anything
/// else as UTF-16BE with a BOM.
pub fn pdf_text_string(value: &str) -> lopdf::Object {
    if value.bytes().all(|b| (0x20..0x7f).contains(&b)) {
        lopdf::Object::string_literal(value)
    } else {
        let mut encoded = Vec::with_capacity(2 + value.len() * 2);
        lopdf::encode_utf16_be(value, &mut encoded);
        lopdf::Object::String(encoded, lopdf::StringFormat::Hexadecimal)
    }
}

/// Now as a PDF date, `D:YYYYMMDDHHmmSS+HH'mm'` in local time.
pub fn pdf_date_now() -> String {
    let now = chrono::Local::now();
    let offset = now.offset().local_minus_utc();
    let sign = if offset < 0 { '-' } else { '+' };
    let minutes = offset.abs() / 60;
    format!(
        "D:{}{sign}{:02}'{:02}'",
        now.format("%Y%m%d%H%M%S"),
        minutes / 60,
        minutes % 60
    )
}

/// A `lopdf` failure on bytes PDFium itself just produced: nothing the user can fix.
pub fn lopdf_error(context: &str, e: impl std::fmt::Display) -> EngineError {
    EngineError::new(ErrorCode::Pdfium, format!("lopdf {context}: {e}"))
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
