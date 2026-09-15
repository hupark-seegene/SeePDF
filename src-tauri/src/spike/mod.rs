//! Tauri v2 integration spike (see docs/spikes/tauri.md).
//!
//! Everything in this module is throw-away proof code: it demonstrates the custom
//! `seepdf://` protocol, binary IPC, managed engine state, cancellable jobs with
//! progress channels, file opening (dialog / drag-drop / argv / Finder) and the
//! runtime location of the bundled libpdfium. Implementation agents should copy the
//! patterns, not the module.

pub mod engine;
pub mod files;
pub mod ipc;
pub mod jobs;
pub mod pdfium_path;
pub mod protocol;
