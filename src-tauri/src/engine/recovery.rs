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
//!
//! **Owners** (v0.3 pkg5, H2). Two SeePDF processes can share the directory (Windows, before
//! the single-instance hand-off, or a second copy started some other way), and the second
//! one's startup recovery used to offer — and on 버리기 delete — the first one's *live*
//! autosaves. Each sidecar now records its [`Owner`] (pid + process start time), and each
//! writing process holds an exclusive lock on `.owner-<pid>-<started>.lock` in the directory
//! for as long as it runs. [`list`] skips a copy whose owner is another process that still
//! holds its lock; the operating system drops the lock when that process exits or crashes,
//! so a dead owner's copies are offered as before, and a recycled pid cannot keep them hidden
//! (the lock names the start time too). Sidecars written before v0.3 have no owner and are
//! always listed.

use crate::engine::pages::write_atomic;
use crate::engine::save;
use crate::engine::types::EngineState;
use crate::ipc::types::RecoveryEntry;
use crate::ipc::EngineError;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, OnceLock};

/// Display name of a never-saved document (contract: "file name or 제목 없음").
pub const UNTITLED_NAME: &str = "제목 없음";

/// Serialises writes and deletes: two autosaves of one document (or an autosave racing a
/// `clear_recovery`) must not interleave their temp files and renames.
static IO_LOCK: Mutex<()> = Mutex::new(());

// ---------------------------------------------------------------------------------------
// Owners (v0.3 pkg5, H2)
// ---------------------------------------------------------------------------------------

/// The process that wrote a recovery copy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Owner {
    pub pid: u32,
    /// When that process first touched the recovery directory, Unix milliseconds.
    pub started: u64,
}

impl Owner {
    /// This process.
    pub fn current() -> &'static Owner {
        static CURRENT: OnceLock<Owner> = OnceLock::new();
        CURRENT.get_or_init(|| Owner {
            pid: std::process::id(),
            started: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0),
        })
    }
}

/// The lock file an owner holds in `dir` while it runs.
pub fn owner_lock_path(dir: &Path, owner: &Owner) -> PathBuf {
    dir.join(format!(".owner-{}-{}.lock", owner.pid, owner.started))
}

/// The locks this process holds, one per recovery directory it wrote to. Never released: the
/// operating system drops them with the process.
static HELD: LazyLock<Mutex<HashMap<PathBuf, std::fs::File>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Takes this process's owner lock in `dir` (once). A failure is logged, not fatal: the copy
/// is still written, it can just be offered to a second process while this one runs.
fn hold_owner_lock(dir: &Path) {
    let mut held = HELD.lock();
    if held.contains_key(dir) {
        return;
    }
    let path = owner_lock_path(dir, Owner::current());
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path);
    match file {
        Ok(file) => match file.try_lock() {
            Ok(()) => {
                held.insert(dir.to_path_buf(), file);
            }
            Err(e) => tracing::warn!(path = %path.display(), "recovery owner lock: {e}"),
        },
        Err(e) => tracing::warn!(path = %path.display(), "recovery owner lock: {e}"),
    }
}

/// `true` when `owner` is another process that is still running (it still holds its lock).
/// A dead owner's lock file is removed on the way.
pub fn owned_by_live_process(dir: &Path, owner: &Owner) -> bool {
    if owner == Owner::current() {
        return false;
    }
    let path = owner_lock_path(dir, owner);
    let Ok(file) = std::fs::OpenOptions::new().write(true).open(&path) else {
        return false; // no lock file: that process never took one, or it was cleaned up
    };
    match file.try_lock() {
        Ok(()) => {
            drop(file);
            let _ = std::fs::remove_file(&path);
            false
        }
        Err(std::fs::TryLockError::WouldBlock) => true,
        Err(std::fs::TryLockError::Error(e)) => {
            tracing::warn!(path = %path.display(), "recovery owner probe: {e}");
            false
        }
    }
}

/// The `<id>.json` sidecar: the [`RecoveryEntry`] plus, since v0.3, its owner.
#[derive(Serialize, Deserialize)]
struct Sidecar {
    #[serde(flatten)]
    entry: RecoveryEntry,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    owner: Option<Owner>,
}

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

/// `fs::canonicalize`, minus the Windows verbatim prefix.
///
/// On Windows `canonicalize` answers `\\?\C:\Users\…`. The recovery path is handed to
/// `open_document` and becomes `DocInfo.path`, so the prefix leaked into the UI and into
/// `explorer /select,`, which does not accept it. A plain drive (or UNC) path short enough
/// not to need the verbatim form gets it stripped — what the `dunce` crate does, without the
/// dependency.
fn canonical(path: PathBuf) -> PathBuf {
    let resolved = std::fs::canonicalize(&path).unwrap_or(path);
    match strip_verbatim(&resolved.to_string_lossy()) {
        Some(plain) => PathBuf::from(plain),
        None => resolved,
    }
}

/// `\\?\C:\a\b` → `C:\a\b`, `\\?\UNC\server\share\a` → `\\server\share\a`; `None`
/// for anything else, and for a path that needs the verbatim form (≥ 260 characters).
fn strip_verbatim(path: &str) -> Option<String> {
    const MAX_PATH: usize = 260;
    let rest = path.strip_prefix(r"\\?\")?;
    let plain = if let Some(unc) = rest.strip_prefix(r"UNC\") {
        format!(r"\\{unc}")
    } else {
        let bytes = rest.as_bytes();
        let drive = bytes.len() >= 3
            && bytes[0].is_ascii_alphabetic()
            && bytes[1] == b':'
            && bytes[2] == b'\\';
        if !drive {
            return None;
        }
        rest.to_string()
    };
    (plain.len() < MAX_PATH).then_some(plain)
}

fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

/// Writes (or overwrites) the pair for `snap` into `dir`, creating `dir` if needed.
pub fn write(dir: &Path, snap: Snapshot) -> Result<RecoveryEntry, EngineError> {
    check_id(&snap.id)?;
    let _guard = IO_LOCK.lock();
    std::fs::create_dir_all(dir).map_err(EngineError::from)?;
    hold_owner_lock(dir);
    let pdf = pdf_path(dir, &snap.id);
    let bytes = write_atomic(&pdf, &snap.bytes)?;
    let recovery_path = canonical(pdf);
    let entry = RecoveryEntry {
        id: snap.id,
        original_path: snap.original_path,
        name: snap.name,
        saved_at: now_iso(),
        bytes,
        pages: snap.pages,
        recovery_path: recovery_path.display().to_string(),
    };
    let sidecar = Sidecar {
        entry,
        owner: Some(Owner::current().clone()),
    };
    let json = serde_json::to_vec_pretty(&sidecar)
        .map_err(|e| EngineError::io(format!("recovery sidecar: {e}")))?;
    write_atomic(&sidecar_path(dir, &sidecar.entry.id), &json)?;
    Ok(sidecar.entry)
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
/// deleted and skipped. A missing directory is an empty list. A copy another running SeePDF
/// process owns is skipped (and left alone): it is that process's live autosave.
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
            .and_then(|bytes| serde_json::from_slice::<Sidecar>(&bytes).ok());
        let pdf = pdf_path(dir, stem);
        let Some(Sidecar {
            mut entry, owner, ..
        }) = parsed.filter(|s| s.entry.id == stem && pdf.is_file())
        else {
            tracing::info!(path = %path.display(), "pruning recovery sidecar without a pdf");
            let _ = std::fs::remove_file(&path);
            continue;
        };
        if let Some(owner) = owner.filter(|o| owned_by_live_process(dir, o)) {
            tracing::debug!(id = %entry.id, pid = owner.pid, "recovery copy of a running process; not offered");
            continue;
        }
        // The directory is the source of truth for where the copy is now.
        entry.recovery_path = canonical(pdf).display().to_string();
        let at = chrono::DateTime::parse_from_rfc3339(&entry.saved_at).ok();
        out.push((at, entry));
    }
    out.sort_by(|(ta, a), (tb, b)| tb.cmp(ta).then_with(|| b.saved_at.cmp(&a.saved_at)));
    Ok(out.into_iter().map(|(_, e)| e).collect())
}

#[cfg(test)]
mod tests {
    use super::strip_verbatim;

    #[test]
    fn verbatim_prefix_is_stripped_from_plain_paths() {
        assert_eq!(
            strip_verbatim(r"\\?\C:\Users\u\AppData\Roaming\SeePDF\recovery\a.pdf").as_deref(),
            Some(r"C:\Users\u\AppData\Roaming\SeePDF\recovery\a.pdf")
        );
        assert_eq!(
            strip_verbatim(r"\\?\UNC\server\share\a.pdf").as_deref(),
            Some(r"\\server\share\a.pdf")
        );
        assert_eq!(strip_verbatim(r"C:\a.pdf"), None, "already plain");
        assert_eq!(strip_verbatim("/Users/u/a.pdf"), None);
        assert_eq!(
            strip_verbatim(r"\\?\Volume{0}\a.pdf"),
            None,
            "not a drive path"
        );
        let long = format!(r"\\?\C:\{}.pdf", "a".repeat(300));
        assert_eq!(strip_verbatim(&long), None, "too long for the plain form");
    }
}
