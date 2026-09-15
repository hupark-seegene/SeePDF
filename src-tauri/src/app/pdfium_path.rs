//! Locate the bundled libpdfium at runtime (`ARCHITECTURE.md` §12).
//!
//! * Production bundle: `app.path().resource_dir()` -> `<App>/Contents/Resources` on macOS,
//!   the exe directory on Windows. `bundle.resources = ["resources/pdfium/*"]` keeps the
//!   relative path, so the library lives in `<resource_dir>/resources/pdfium/`.
//! * `tauri dev` / `cargo run`: `resource_dir()` returns `target/debug` and tauri-build
//!   copies the same resources there (it runs `copy_resources` on every platform), so the
//!   same lookup works. As a belt-and-braces fallback we also try
//!   `CARGO_MANIFEST_DIR/resources/pdfium` in debug builds.

use std::path::PathBuf;
use tauri::{AppHandle, Manager};

/// File name of the pdfium dynamic library for the running host, as shipped in
/// `src-tauri/resources/pdfium/` (one file per target triple).
pub fn library_file_name() -> Option<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => Some("libpdfium-aarch64-apple-darwin.dylib"),
        ("macos", "x86_64") => Some("libpdfium-x86_64-apple-darwin.dylib"),
        ("windows", "x86_64") => Some("pdfium-x86_64-pc-windows-msvc.dll"),
        ("linux", "x86_64") => Some("libpdfium-x86_64-unknown-linux-gnu.so"),
        _ => None,
    }
}

/// Candidate directories that may contain the pdfium library, in priority order.
pub fn candidate_dirs(app: &AppHandle) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    match app.path().resource_dir() {
        Ok(res) => {
            tracing::debug!(resource_dir = %res.display(), "tauri resource_dir");
            dirs.push(res.join("resources").join("pdfium"));
            dirs.push(res.join("pdfium"));
        }
        Err(e) => tracing::warn!(error = %e, "resource_dir() unavailable"),
    }
    if cfg!(debug_assertions) {
        dirs.push(
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("resources")
                .join("pdfium"),
        );
    }
    dirs
}

/// Resolve `(library_path, directory)` or an error message.
pub fn resolve(app: &AppHandle) -> Result<(PathBuf, PathBuf), String> {
    let name = library_file_name().ok_or_else(|| {
        format!(
            "no bundled pdfium for {}-{}",
            std::env::consts::OS,
            std::env::consts::ARCH
        )
    })?;
    let dirs = candidate_dirs(app);
    for dir in &dirs {
        let lib = dir.join(name);
        if lib.is_file() {
            return Ok((lib, dir.clone()));
        }
    }
    Err(format!(
        "libpdfium ({name}) not found in any of: {}",
        dirs.iter()
            .map(|d| d.display().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    ))
}

/// Repository `fixtures/` directory (dev only; used for the startup smoke test).
pub fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map(|p| p.join("fixtures"))
        .unwrap_or_else(|| PathBuf::from("fixtures"))
}

/// Resolve without a Tauri app: the repository's `src-tauri/resources/pdfium`. Used by the
/// Rust test harness and by `--smoke` before any window exists.
pub fn resolve_local() -> Result<(PathBuf, PathBuf), String> {
    let name = library_file_name().ok_or_else(|| {
        format!(
            "no bundled pdfium for {}-{}",
            std::env::consts::OS,
            std::env::consts::ARCH
        )
    })?;
    let mut dirs = vec![PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("resources")
        .join("pdfium")];
    if let Ok(exe) = std::env::current_exe() {
        // `target/{debug,release}/deps/<test-binary>` -> `target/<profile>/resources/pdfium`
        for up in [1usize, 2, 3] {
            if let Some(dir) = exe.ancestors().nth(up) {
                dirs.push(dir.join("resources").join("pdfium"));
            }
        }
    }
    for dir in &dirs {
        let lib = dir.join(name);
        if lib.is_file() {
            return Ok((lib, dir.clone()));
        }
    }
    Err(format!(
        "libpdfium ({name}) not found in any of: {}",
        dirs.iter()
            .map(|d| d.display().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    ))
}
