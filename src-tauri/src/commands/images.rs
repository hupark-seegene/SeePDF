//! v0.3 pkg4-annotations-stamps-objects — image helpers for the 도장 / 서명 / 워터마크 UI.
//!
//! None of these touch PDFium: they decode or copy files with the `image` crate on a
//! blocking pool thread, never on the engine thread (CLAUDE.md hard rules).
//!
//! * `image_preview` (T1) — a picked PNG / JPEG's real size plus a small PNG the webview turns
//!   into a `blob:` URL (already allowed by the CSP's `img-src`), so the placement ghost and the
//!   워터마크 preview show the image and a stamp keeps its aspect. No asset protocol, no fs scope.
//! * `copy_library_image` / `remove_library_image` (T2 / T3) — 내 도장 and 저장된 서명 keep
//!   their own copy of a picked image under `$APPDATA/SeePDF/{stamps,signatures}/`.

use crate::app::{signatures, store};
use crate::ipc::EngineError;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tauri::AppHandle;

/// Longest side of an `image_preview` thumbnail, px.
pub const MAX_PREVIEW_PX: u32 = 1024;

/// Decodes `path` and returns `(width, height, png)`: the image's own size and a PNG no
/// larger than `max_px` on its longest side (aspect kept). Pure — the tests call it directly.
pub fn preview_png(path: &Path, max_px: u32) -> Result<(u32, u32, Vec<u8>), EngineError> {
    let bytes = std::fs::read(path)
        .map_err(|e| EngineError::io(format!("read {}: {e}", path.display())))?;
    let image = image::load_from_memory(&bytes).map_err(|e| {
        EngineError::invalid(format!(
            "{} is not a PNG or JPEG image: {e}",
            path.display()
        ))
    })?;
    let (w, h) = (image.width(), image.height());
    if w == 0 || h == 0 {
        return Err(EngineError::invalid("image has no pixels"));
    }
    let max_px = max_px.clamp(16, MAX_PREVIEW_PX);
    let thumb = if w.max(h) > max_px {
        image.thumbnail(max_px, max_px)
    } else {
        image
    };
    let mut png = Vec::new();
    thumb
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .map_err(|e| EngineError::io(format!("encode preview: {e}")))?;
    Ok((w, h, png))
}

/// `image_preview` wire format (`IPC_CONTRACT.md` §11): `u32 LE width`, `u32 LE height` (the
/// image's own pixels), then the preview PNG.
pub fn preview_payload(width: u32, height: u32, png: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(8 + png.len());
    out.extend_from_slice(&width.to_le_bytes());
    out.extend_from_slice(&height.to_le_bytes());
    out.extend_from_slice(png);
    out
}

#[tauri::command]
pub async fn image_preview(
    path: String,
    max_px: Option<u32>,
) -> Result<tauri::ipc::Response, EngineError> {
    let max_px = max_px.unwrap_or(512);
    let payload = tauri::async_runtime::spawn_blocking(move || {
        preview_png(Path::new(&path), max_px).map(|(w, h, png)| preview_payload(w, h, &png))
    })
    .await
    .map_err(|e| EngineError::io(format!("image preview: {e}")))??;
    Ok(tauri::ipc::Response::new(payload))
}

/// Which library a copied image belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ImageLibrary {
    /// 내 도장 — `$APPDATA/SeePDF/stamps/`.
    Stamp,
    /// 저장된 서명 — `$APPDATA/SeePDF/signatures/`.
    Signature,
}

/// `copy_library_image`'s answer.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryImage {
    pub path: String,
    pub width: u32,
    pub height: u32,
}

fn library_dir(app: &AppHandle, library: ImageLibrary) -> Result<PathBuf, EngineError> {
    match library {
        ImageLibrary::Stamp => store::stamps_dir(app),
        ImageLibrary::Signature => store::signatures_dir(app),
    }
    .ok_or_else(|| EngineError::io("no app data directory"))
}

#[tauri::command]
pub async fn copy_library_image(
    app: AppHandle,
    path: String,
    library: ImageLibrary,
) -> Result<LibraryImage, EngineError> {
    let dir = library_dir(&app, library)?;
    tauri::async_runtime::spawn_blocking(move || {
        signatures::copy_library_image(&dir, Path::new(&path)).map(|(p, width, height)| {
            LibraryImage {
                path: p.display().to_string(),
                width,
                height,
            }
        })
    })
    .await
    .map_err(|e| EngineError::io(format!("copy image: {e}")))?
}

/// Deletes a library copy when its 내 도장 / 저장된 서명 entry is removed. Anything that is
/// not an `img-*` file directly inside that library's directory is left alone (`false`).
#[tauri::command]
pub async fn remove_library_image(
    app: AppHandle,
    path: String,
    library: ImageLibrary,
) -> Result<bool, EngineError> {
    let dir = library_dir(&app, library)?;
    tauri::async_runtime::spawn_blocking(move || {
        signatures::remove_library_image(&dir, Path::new(&path))
    })
    .await
    .map_err(|e| EngineError::io(format!("remove image: {e}")))?
}
