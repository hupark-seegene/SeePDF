//! Every command of `IPC_CONTRACT.md`, one module per area.
//!
//! Stage 0 registers **all** of them in `lib.rs`'s `generate_handler!` with their real serde
//! signatures, so `lib.rs` never has to change again. A command whose owning Stage-1 module
//! has not landed yet returns `EngineError::unsupported("<command>")`; filling it in means
//! editing only that one function body.

pub mod annots;
pub mod app;
pub mod compare;
pub mod documents;
pub mod export;
pub mod forms;
pub mod objects;
pub mod ocr;
pub mod pages;
pub mod recovery;
pub mod save;
pub mod security;
pub mod stamp;
pub mod text;

use crate::ipc::EngineError;

/// Reads a file on a blocking pool thread (the `seepdf-io` role), never on the engine thread.
pub async fn read_file(path: &str) -> Result<Vec<u8>, EngineError> {
    let path = path.to_string();
    tauri::async_runtime::spawn_blocking(move || {
        std::fs::read(&path).map_err(|e| {
            let mut err = EngineError::from(e);
            err.message = format!("{path}: {}", err.message);
            err
        })
    })
    .await
    .map_err(|e| EngineError::io(format!("io task failed: {e}")))?
}
