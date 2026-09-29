//! `FPDF_SaveAsCopy` / `FPDF_SaveWithVersion` with explicit flags.
//!
//! `PdfDocument::save_to_bytes()` hard-codes flag `0`. Stage 1 (b)'s save path needs the flag
//! word for two reasons: `FPDF_NO_INCREMENTAL` (a full rewrite, which is what SeePDF ships —
//! incremental save is not in v1) and `FPDF_REMOVE_SECURITY` for "remove password"
//! (**value 4** on build 8057; the deprecated 3 is a no-op — pages spike §8).

use crate::ipc::{EngineError, ErrorCode};
use pdfium_render::prelude::{PdfDocument, PdfiumLibraryBindings, FPDF_FILEWRITE};
// `c_ulong` is `u64` on macOS and `u32` on Windows; the flags are 32 bits either way.
use std::os::raw::{c_int, c_ulong, c_void};

/// Flags for [`save_as_copy`]. `FPDF_DWORD` is a bit field, but PDFium treats incremental /
/// no-incremental / remove-security as mutually exclusive values, so this is an enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SaveFlags {
    /// `0` — PDFium's default (incremental when it can, full rewrite otherwise).
    Default,
    /// `FPDF_NO_INCREMENTAL = 2` — always a full rewrite. What `save_document` uses.
    NoIncremental,
    /// `FPDF_REMOVE_SECURITY = 4` — writes the document with its encryption dictionary
    /// dropped. The value is 4 on build 8057; 3 is the deprecated alias and does nothing.
    RemoveSecurity,
    /// `FPDF_INCREMENTAL = 1` — the loaded bytes verbatim plus an appended update (v0.3 S1:
    /// saving a signed document keeps the signed revisions byte-identical).
    Incremental,
}

impl SaveFlags {
    fn bits(self) -> u32 {
        match self {
            SaveFlags::Default => 0,
            SaveFlags::NoIncremental => crate::engine::raw::consts::FPDF_NO_INCREMENTAL,
            SaveFlags::RemoveSecurity => crate::engine::raw::consts::FPDF_REMOVE_SECURITY,
            SaveFlags::Incremental => crate::engine::raw::consts::FPDF_INCREMENTAL,
        }
    }
}

/// The `FPDF_FILEWRITE` PDFium writes through. `fw` **must** be the first field: PDFium is
/// handed `&mut fw` and casts it back to this struct inside [`write_block`].
#[repr(C)]
struct FileWriter {
    fw: FPDF_FILEWRITE,
    buf: Vec<u8>,
}

/// PDFium's write callback. Returns 1 on success, as `FPDF_FILEWRITE::WriteBlock` requires.
///
/// # Safety
/// `this` must point at a live [`FileWriter`] (guaranteed by [`save_as_copy`], which is the
/// only caller and keeps it on its own stack for the duration of the FFI call), and
/// `data[..size]` must be readable — that is PDFium's side of the contract.
unsafe extern "C" fn write_block(
    this: *mut FPDF_FILEWRITE,
    data: *const c_void,
    size: c_ulong,
) -> c_int {
    if this.is_null() || data.is_null() {
        return 0;
    }
    let writer = this as *mut FileWriter;
    let slice = std::slice::from_raw_parts(data as *const u8, size as usize);
    (*writer).buf.extend_from_slice(slice);
    1
}

/// Serialises `doc` to a fresh `Vec<u8>` with the given flags.
///
/// Costs ~5 ms per MB. Everything the document holds in memory is written, so annotation
/// appearance streams must already exist (`render::tiles::generate_appearances`) — saving
/// before any render leaves markup annotations without an `/AP` and they are invisible in
/// Preview and Acrobat (annotations spike §3.2).
pub fn save_as_copy(
    bindings: &dyn PdfiumLibraryBindings,
    doc: &PdfDocument<'_>,
    flags: SaveFlags,
) -> Result<Vec<u8>, EngineError> {
    let mut writer = FileWriter {
        fw: FPDF_FILEWRITE {
            version: 1,
            WriteBlock: Some(write_block),
        },
        buf: Vec::new(),
    };
    // SAFETY: `writer.fw` is the first field of a `#[repr(C)]` struct that lives on this
    // stack frame for the whole call, so the pointer PDFium hands back to `write_block` is
    // a valid `*mut FileWriter`; `doc` is live for this borrow.
    let ok = unsafe {
        bindings.FPDF_SaveAsCopy(
            doc.raw_handle(),
            &mut writer.fw as *mut FPDF_FILEWRITE,
            flags.bits() as c_ulong,
        )
    };
    if !bindings.is_true(ok) {
        return Err(EngineError::new(
            ErrorCode::Pdfium,
            format!("FPDF_SaveAsCopy({flags:?}) failed"),
        ));
    }
    if writer.buf.is_empty() {
        return Err(EngineError::new(
            ErrorCode::Pdfium,
            "FPDF_SaveAsCopy produced no bytes",
        ));
    }
    Ok(writer.buf)
}

/// `FPDF_SaveWithVersion` — same as [`save_as_copy`] but pins the header version
/// (`14` = PDF 1.4, `17` = PDF 1.7). Use it to keep a document's original version across a
/// full rewrite.
pub fn save_with_version(
    bindings: &dyn PdfiumLibraryBindings,
    doc: &PdfDocument<'_>,
    flags: SaveFlags,
    version: i32,
) -> Result<Vec<u8>, EngineError> {
    let mut writer = FileWriter {
        fw: FPDF_FILEWRITE {
            version: 1,
            WriteBlock: Some(write_block),
        },
        buf: Vec::new(),
    };
    // SAFETY: as in `save_as_copy`.
    let ok = unsafe {
        bindings.FPDF_SaveWithVersion(
            doc.raw_handle(),
            &mut writer.fw as *mut FPDF_FILEWRITE,
            flags.bits() as c_ulong,
            version as c_int,
        )
    };
    if !bindings.is_true(ok) || writer.buf.is_empty() {
        return Err(EngineError::new(
            ErrorCode::Pdfium,
            format!("FPDF_SaveWithVersion({flags:?}, {version}) failed"),
        ));
    }
    Ok(writer.buf)
}
