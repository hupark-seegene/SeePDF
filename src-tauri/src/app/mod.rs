//! App plumbing: where libpdfium lives, how files arrive, windows, the native menu and the
//! settings / recents store (`WORKPLAN.md` row 0.8).

pub mod files;
#[cfg(target_os = "macos")]
pub mod menu;
pub mod pdfium_path;
pub mod store;
pub mod windows;

pub use files::PendingOpens;
pub use windows::WindowDocs;
