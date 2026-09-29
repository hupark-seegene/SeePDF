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
///
/// v0.3 (V4): with `sentences` the text is read one sentence per utterance from `startIndex`
/// (default 0) on, and every sentence start is announced as `tts-progress`; `text` is then
/// ignored and may be omitted.
#[tauri::command]
pub async fn tts_speak(
    tts: State<'_, Arc<Tts>>,
    text: Option<String>,
    lang: Option<String>,
    rate: Option<f32>,
    sentences: Option<Vec<String>>,
    start_index: Option<u32>,
) -> Result<TtsStatus, EngineError> {
    let tts = tts.inner().clone();
    blocking(move || match sentences {
        Some(sentences) => tts.speak_sentences(
            sentences,
            start_index.unwrap_or(0) as usize,
            lang.as_deref(),
            rate,
        ),
        None => tts.speak(text.as_deref().unwrap_or(""), lang.as_deref(), rate),
    })
    .await
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
