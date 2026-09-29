//! App plumbing: where libpdfium lives, how files arrive, windows, the native menu and the
//! settings / recents store (`WORKPLAN.md` row 0.8).

// v0.3 pkg5: rolling log, panic hook, startup-failure box, 문제 보고 (H10)
pub mod diagnostics;
pub mod files;
#[cfg(target_os = "macos")]
pub mod menu;
pub mod pdfium_path;
pub mod signatures;
pub mod store;
pub mod tts;
pub mod undo_labels;
pub mod windows;

pub use files::PendingOpens;
pub use windows::WindowDocs;
