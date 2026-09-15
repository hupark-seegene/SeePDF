//! The backend half of `IPC_CONTRACT.md`: the wire types and the one error type.

pub mod error;
pub mod types;

pub use error::{EngineError, ErrorCode, PdfiumResultExt};
