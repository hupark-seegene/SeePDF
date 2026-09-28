//! 읽어 주기 (P2) — `IPC_CONTRACT.md` §11a. The logic is `app::tts`; these wrappers only hop
//! onto a blocking thread (the first call lists the voices, ≈ 100 ms) so no async worker waits.

use crate::app::tts::Tts;
use crate::ipc::types::TtsStatus;
use crate::ipc::EngineError;
use std::sync::Arc;
use tauri::State;

async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, EngineError> + Send + 'static,
) -> Result<T, EngineError> {
    tauri::async_runtime::spawn_blocking(f)
        .await
        .map_err(|e| EngineError::io(format!("tts task failed: {e}")))?
}

/// Speaks `text` with the system voice, stopping whatever was speaking. `lang` (`ko`, `en-US`)
/// picks the voice, default: from the text; `rate` 0.5 … 2.0 (default 1).
#[tauri::command]
pub async fn tts_speak(
    tts: State<'_, Arc<Tts>>,
    text: String,
    lang: Option<String>,
    rate: Option<f32>,
) -> Result<TtsStatus, EngineError> {
    let tts = tts.inner().clone();
    blocking(move || tts.speak(&text, lang.as_deref(), rate)).await
}

#[tauri::command]
pub async fn tts_stop(tts: State<'_, Arc<Tts>>) -> Result<TtsStatus, EngineError> {
    let tts = tts.inner().clone();
    blocking(move || Ok(tts.stop())).await
}

#[tauri::command]
pub async fn tts_status(tts: State<'_, Arc<Tts>>) -> Result<TtsStatus, EngineError> {
    let tts = tts.inner().clone();
    blocking(move || Ok(tts.status())).await
}
