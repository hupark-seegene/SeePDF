//! The one error type that crosses the IPC seam (`IPC_CONTRACT.md` §2).
//!
//! Serialises as `{ code, message }` (plus `page` / `detail` only when present), so the
//! frontend can map `code` to an `error.*` i18n key and put `message` behind 자세히.

use pdfium_render::prelude::{PdfiumError, PdfiumInternalError};
use serde::{Deserialize, Serialize};

/// Every error the frontend can receive. Keep in sync with `ErrorCode` in `src/ipc/types.ts`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ErrorCode {
    /// Document is encrypted and no password was supplied.
    PasswordRequired,
    /// Supplied password rejected.
    PasswordWrong,
    /// Unknown docId / page / annotId / objectId / path.
    NotFound,
    /// Range string, page list or geometry validation failed.
    InvalidArgument,
    /// Impossible on this document (XFA, XObject text, Type3 font…), or not implemented yet.
    Unsupported,
    /// The PDF's own permission bits forbid it.
    PermissionDenied,
    /// Target file or volume is not writable; the caller should offer Save As.
    ReadOnly,
    /// `expectGeneration` mismatch, or a render whose viewport moved on.
    Stale,
    /// Job cancelled by the user.
    Cancelled,
    /// Engine queue bound exceeded; retry with backoff.
    Busy,
    /// The requested text cannot be rendered by the target font.
    FontCoverage,
    /// Save or redaction post-condition failed; the document was rolled back.
    VerifyFailed,
    /// A `PdfiumError` that maps to nothing more specific.
    Pdfium,
    /// Filesystem error.
    Io,
    /// v0.3 pkg5 (H4): the command panicked on the engine thread. The documents it touched
    /// were closed (`engine-crashed` event); the engine keeps serving the others.
    EngineCrashed,
}

/// The error object delivered to JS on rejection.
#[derive(Debug, Clone, thiserror::Error, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[error("{code:?}: {message}")]
pub struct EngineError {
    pub code: ErrorCode,
    /// English developer string. The UI never shows it directly.
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub page: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub detail: Option<String>,
}

impl EngineError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            page: None,
            detail: None,
        }
    }

    pub fn with_page(mut self, page: u16) -> Self {
        self.page = Some(page);
        self
    }

    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    /// The body every command that its owning Stage-1 module has not landed yet returns.
    pub fn unsupported(what: &str) -> Self {
        Self::new(ErrorCode::Unsupported, format!("{what} is not implemented"))
    }

    pub fn not_found(what: impl Into<String>) -> Self {
        Self::new(ErrorCode::NotFound, what)
    }

    pub fn invalid(what: impl Into<String>) -> Self {
        Self::new(ErrorCode::InvalidArgument, what)
    }

    pub fn stale(what: impl Into<String>) -> Self {
        Self::new(ErrorCode::Stale, what)
    }

    pub fn cancelled(what: impl Into<String>) -> Self {
        Self::new(ErrorCode::Cancelled, what)
    }

    pub fn busy(what: impl Into<String>) -> Self {
        Self::new(ErrorCode::Busy, what)
    }

    pub fn io(what: impl Into<String>) -> Self {
        Self::new(ErrorCode::Io, what)
    }

    pub fn pdfium(context: &str, e: PdfiumError) -> Self {
        // Both "no password given" and "wrong password" surface as this one internal error;
        // the registry decides which of the two codes applies (ARCHITECTURE §2).
        if matches!(
            e,
            PdfiumError::PdfiumLibraryInternalError(PdfiumInternalError::PasswordError)
        ) {
            return Self::new(
                ErrorCode::PasswordRequired,
                format!("{context}: password required"),
            );
        }
        Self::new(ErrorCode::Pdfium, format!("{context}: {e:?}"))
    }

    pub fn is_password(&self) -> bool {
        matches!(
            self.code,
            ErrorCode::PasswordRequired | ErrorCode::PasswordWrong
        )
    }
}

impl From<std::io::Error> for EngineError {
    fn from(e: std::io::Error) -> Self {
        let code = match e.kind() {
            std::io::ErrorKind::NotFound => ErrorCode::NotFound,
            std::io::ErrorKind::PermissionDenied | std::io::ErrorKind::ReadOnlyFilesystem => {
                ErrorCode::ReadOnly
            }
            _ => ErrorCode::Io,
        };
        let err = Self::new(code, e.to_string());
        // v0.3 pkg5 (H3): 메모리가 부족합니다 instead of a generic "cannot open".
        if e.kind() == std::io::ErrorKind::OutOfMemory {
            return err.with_detail("outOfMemory");
        }
        err
    }
}

/// Convenience for `?` on pdfium calls inside engine code.
pub trait PdfiumResultExt<T> {
    fn ctx(self, context: &str) -> Result<T, EngineError>;
}

impl<T> PdfiumResultExt<T> for Result<T, PdfiumError> {
    fn ctx(self, context: &str) -> Result<T, EngineError> {
        self.map_err(|e| EngineError::pdfium(context, e))
    }
}
