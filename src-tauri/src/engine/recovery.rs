//! Autosave / crash recovery (P1-8, `IPC_CONTRACT.md` §7.6c).
//!
//! A recovery copy is a pair in the recovery directory (`app_data_dir()/recovery/`):
//! `<id>.pdf` — the document's current state exactly as a save would write it
//! ([`save::serialize`], appearance streams included, encryption kept) — and `<id>.json`, the
//! [`RecoveryEntry`] describing it. The user's own file is never touched.
//!
//! Split by thread: [`snapshot`] runs on the engine thread (PDFium serialisation, assigns the
//! document's recovery id on first use); everything else here is plain file I/O on the calling
//! (command) task and takes the directory as an argument, so tests aim it at a temp dir.
//!
//! Writes are atomic per file (temp + fsync + rename). The PDF is written before the sidecar
//! and [`list`] ignores (and deletes) a sidecar whose PDF is missing, so a crash between the
//! two leaves either the previous pair or an orphan PDF that the next write overwrites.

use crate::engine::pages::write_atomic;
use crate::engine::save;
use crate::engine::types::EngineState;
use crate::ipc::types::RecoveryEntry;
use crate::ipc::EngineError;
use parking_lot::Mutex;
use std::path::{Path, PathBuf};

/// Display name of a never-saved document (contract: "file name or 제목 없음").
pub const UNTITLED_NAME: &str = "제목 없음";

/// Serialises writes and deletes: two autosaves of one document (or an autosave racing a
/// `clear_recovery`) must not interleave their temp files and renames.
static IO_LOCK: Mutex<()> = Mutex::new(());

/// What the engine thread hands the command task.
#[derive(Debug, Clone)]
pub struct Snapshot {
    pub id: String,
    pub bytes: Vec<u8>,
    pub original_path: Option<String>,
    pub name: String,
    pub pages: u16,
}

/// Engine thread: the document's recovery id (assigned now if it has none) and its current
/// bytes. Does not change the generation or the dirty flag.
pub fn snapshot(st: &mut EngineState<'_>, doc_id: &str) -> Result<Snapshot, EngineError> {
    let bytes = save::serialize(st, doc_id)?;
    let doc = st.doc_mut(doc_id)?;
    let id = doc
        .recovery_id
        .get_or_insert_with(|| uuid::Uuid::new_v4().to_string())
        .clone();
    let original_path = doc.path.as_ref().map(|p| p.display().to_string());
    let name = doc
        .path
        .as_ref()
        .and_then(|p| p.file_name())
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| UNTITLED_NAME.to_string());
    Ok(Snapshot {
        id,
        bytes,
        original_path,
        name,
        pages: doc.page_count(),
    })
}

/// Engine thread: the document's recovery id, if it ever wrote one.
pub fn recovery_id(st: &EngineState<'_>, doc_id: &str) -> Result<Option<String>, EngineError> {
    Ok(st.doc(doc_id)?.recovery_id.clone())
}

/// Engine thread: the recovery ids of every open document (their copies are live autosaves,
/// so `list_recovery` does not offer them).
pub fn open_recovery_ids(st: &EngineState<'_>) -> std::collections::HashSet<String> {
    st.docs
        .values()
        .filter_map(|d| d.recovery_id.clone())
        .collect()
}

/// A recovery id is a uuid; anything else could name a path outside the directory.
fn check_id(id: &str) -> Result<(), EngineError> {
    uuid::Uuid::parse_str(id)
        .map(|_| ())
        .map_err(|_| EngineError::invalid(format!("'{id}' is not a recovery id")))
}

fn pdf_path(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("{id}.pdf"))
}

fn sidecar_path(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("{id}.json"))
}

fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

/// Writes (or overwrites) the pair for `snap` into `dir`, creating `dir` if needed.
pub fn write(dir: &Path, snap: Snapshot) -> Result<RecoveryEntry, EngineError> {
    check_id(&snap.id)?;
    let _guard = IO_LOCK.lock();
    std::fs::create_dir_all(dir).map_err(EngineError::from)?;
    let pdf = pdf_path(dir, &snap.id);
    let bytes = write_atomic(&pdf, &snap.bytes)?;
    let recovery_path = std::fs::canonicalize(&pdf).unwrap_or(pdf);
    let entry = RecoveryEntry {
        id: snap.id,
        original_path: snap.original_path,
        name: snap.name,
        saved_at: now_iso(),
        bytes,
        pages: snap.pages,
        recovery_path: recovery_path.display().to_string(),
    };
    let json = serde_json::to_vec_pretty(&entry)
        .map_err(|e| EngineError::io(format!("recovery sidecar: {e}")))?;
    write_atomic(&sidecar_path(dir, &entry.id), &json)?;
    Ok(entry)
}

/// Deletes one pair. Unknown id (or a missing directory) is a no-op; a malformed id is
/// `invalidArgument`.
pub fn discard(dir: &Path, id: &str) -> Result<(), EngineError> {
    check_id(id)?;
    let _guard = IO_LOCK.lock();
    remove_if_exists(&sidecar_path(dir, id))?;
    remove_if_exists(&pdf_path(dir, id))?;
    Ok(())
}

fn remove_if_exists(path: &Path) -> Result<(), EngineError> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(EngineError::from(e)),
    }
}

/// Every pair in `dir`, newest first. A sidecar whose PDF is gone, or that does not parse, is
/// deleted and skipped. A missing directory is an empty list.
pub fn list(dir: &Path) -> Result<Vec<RecoveryEntry>, EngineError> {
    let _guard = IO_LOCK.lock();
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(EngineError::from(e)),
    };
    let mut out: Vec<(Option<chrono::DateTime<chrono::FixedOffset>>, RecoveryEntry)> = Vec::new();
    for dirent in entries.filter_map(|e| e.ok()) {
        let path = dirent.path();
        let is_sidecar = path.extension().map(|e| e == "json").unwrap_or(false);
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        // Skips write_atomic's `.<name>.seepdf-<pid>.tmp` files and anything else foreign.
        if !is_sidecar || uuid::Uuid::parse_str(stem).is_err() {
            continue;
        }
        let parsed = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<RecoveryEntry>(&bytes).ok());
        let pdf = pdf_path(dir, stem);
        let Some(mut entry) = parsed.filter(|e| e.id == stem && pdf.is_file()) else {
            tracing::info!(path = %path.display(), "pruning recovery sidecar without a pdf");
            let _ = std::fs::remove_file(&path);
            continue;
        };
        // The directory is the source of truth for where the copy is now.
        entry.recovery_path = std::fs::canonicalize(&pdf)
            .unwrap_or(pdf)
            .display()
            .to_string();
        let at = chrono::DateTime::parse_from_rfc3339(&entry.saved_at).ok();
        out.push((at, entry));
    }
    out.sort_by(|(ta, a), (tb, b)| tb.cmp(ta).then_with(|| b.saved_at.cmp(&a.saved_at)));
    Ok(out.into_iter().map(|(_, e)| e).collect())
}
