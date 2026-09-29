//! The engine: one thread owns pdfium, every document and every page (`ARCHITECTURE.md` §1).
//!
//! Stage 1 entry points, in the order they are usually needed:
//! * [`thread::EngineHandle::call`] — run a closure on the engine thread from a command;
//! * [`registry::mutate`] — the only `&mut` path into a document (snapshot, generation bump,
//!   cache invalidation and `doc-changed` are handled for you);
//! * [`registry::OpenDoc::page`] — open a page through the LRU (form hooks + `Manual`
//!   regeneration strategy applied);
//! * [`raw`] — the only `unsafe` in the tree.

pub mod annot;
pub mod compare;
pub mod compress;
pub mod export;
pub mod fonts;
pub mod form;
pub mod history;
pub mod jobs;
pub mod objects;
pub mod ocr;
pub mod page_lru;
pub mod pages;
pub mod raw;
pub mod recovery;
pub mod redact;
pub mod registry;
pub mod render;
pub mod save;
pub mod security;
pub mod stamp;
pub mod stats;
pub mod structure;
pub mod text;
pub mod thread;
pub mod types;
// v0.3 pkg3-security-save-integrity: 첨부 파일 (S4) and 문서 정리 (S3).
pub mod attachments;
pub mod sanitize;

pub use thread::{spawn, EngineHandle, Submit};
pub use types::{Cmd, CmdStatus, EngineShared, EngineState, Lane, Reply, Viewport};
