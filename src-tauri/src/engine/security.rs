//! Password protection and metadata rewrites — `IPC_CONTRACT.md` §7.5, P1-1 / P1-3.
//!
//! PDFium reads encryption and `/Info` but writes neither, so both go through `lopdf` on the
//! bytes PDFium serialised, and both are verified by reopening the result **with PDFium**
//! before anything reaches the user:
//!
//! * [`set_metadata`] / [`remove_metadata`] edit the open document (one undo step, via
//!   [`registry::mutate_bytes`] + [`save::write_info`]). Refused on an encrypted document:
//!   re-encrypting needs the owner password, which a user-password open does not give us.
//! * [`set_password`] writes an AES-256 (V5 / R6) copy to `out_path` and leaves the open
//!   document alone, like `remove_password`.

use crate::engine::raw::save::SaveFlags;
use crate::engine::registry::{self, MutateOpts};
use crate::engine::save;
use crate::engine::types::EngineState;
use crate::ipc::types::{BytesWritten, ChangeReason, DocInfo, DocMeta, PermissionsRequest};
use crate::ipc::{EngineError, ErrorCode};
use lopdf::encryption::crypt_filters::{Aes256CryptFilter, CryptFilter};
use std::collections::BTreeMap;
use std::sync::Arc;

/// PDF 2.0 §7.6.4.3.3: an R6 password is at most 127 UTF-8 bytes after SASLprep.
const MAX_PASSWORD_BYTES: usize = 127;

fn refuse_encrypted(st: &EngineState<'_>, doc_id: &str) -> Result<(), EngineError> {
    let doc = st.doc(doc_id)?;
    if doc.encrypted || doc.password.is_some() {
        return Err(EngineError::new(
            ErrorCode::Unsupported,
            "this document is encrypted: remove the password first",
        ));
    }
    Ok(())
}

/// `set_metadata` — writes the `Some` fields of `meta` into `/Info` (see
/// [`save::write_info`] for the exact rules) as one undoable edit.
pub fn set_metadata(
    st: &mut EngineState<'_>,
    doc_id: &str,
    meta: &DocMeta,
) -> Result<DocInfo, EngineError> {
    refuse_encrypted(st, doc_id)?;
    let opts = MutateOpts::new("undo.metadataEdit", ChangeReason::Edit)
        .all_pages()
        .keeps_text();
    registry::mutate_bytes(st, doc_id, opts, |bytes, _| {
        save::write_info(bytes, meta, false)
    })
}

/// `remove_metadata` (메타데이터 제거) — deletes `/Info` and the XMP packet as one undoable
/// edit. Every `DocMeta` field reads back `None` afterwards.
pub fn remove_metadata(st: &mut EngineState<'_>, doc_id: &str) -> Result<DocInfo, EngineError> {
    refuse_encrypted(st, doc_id)?;
    let opts = MutateOpts::new("undo.metadataRemove", ChangeReason::Edit)
        .all_pages()
        .keeps_text();
    registry::mutate_bytes(st, doc_id, opts, |bytes, _| {
        save::write_info(bytes, &DocMeta::default(), true)
    })
}

/// `set_password` — an AES-256 (V5 / R6) protected copy of the document at `out_path`.
///
/// `user_password` empty or `None` = no open prompt (owner password + permissions only). The
/// open document is not modified. The written file is verified with PDFium: it must open
/// with the user password (or none), keep the page count, and — when a user password was set
/// — refuse to open without it.
pub fn set_password(
    st: &mut EngineState<'_>,
    doc_id: &str,
    out_path: &str,
    user_password: Option<&str>,
    owner_password: &str,
    permissions: PermissionsRequest,
) -> Result<BytesWritten, EngineError> {
    let user = user_password.filter(|p| !p.is_empty());
    if owner_password.is_empty() {
        return Err(EngineError::invalid("the owner password must not be empty"));
    }
    for p in [Some(owner_password), user].into_iter().flatten() {
        if p.len() > MAX_PASSWORD_BYTES {
            return Err(EngineError::invalid(format!(
                "a password may be at most {MAX_PASSWORD_BYTES} bytes (UTF-8)"
            )));
        }
    }

    let (encrypted, pages) = {
        let doc = st.doc(doc_id)?;
        (doc.encrypted || doc.password.is_some(), doc.page_count())
    };
    // An encrypted input is decrypted on the way out (we hold its password); `lopdf` refuses
    // to encrypt a document that already has an /Encrypt dictionary.
    let flags = if encrypted {
        SaveFlags::RemoveSecurity
    } else {
        SaveFlags::NoIncremental
    };
    let plain = save::serialize_with(st, doc_id, flags)?;
    let out = encrypt_bytes(&plain, user.unwrap_or(""), owner_password, permissions)?;

    // Verify: opens with the user password (or none) and has every page …
    save::verify_bytes(st, &out, pages, user.map(str::to_owned))?;
    // … opens with the owner password …
    save::verify_bytes(st, &out, pages, Some(owner_password.to_owned()))?;
    // … and, with a user password, does not open without it.
    if user.is_some() && st.pdfium.load_pdf_from_byte_slice(&out, None).is_ok() {
        return Err(EngineError::new(
            ErrorCode::VerifyFailed,
            "the protected copy opened without its user password",
        ));
    }

    let written = crate::engine::pages::write_atomic(std::path::Path::new(out_path), &out)?;
    Ok(BytesWritten { bytes: written })
}

/// `lopdf`'s permission bits for our flags. `COPYABLE_FOR_ACCESSIBILITY` is always granted
/// (PDF 2.0 deprecates clearing it; screen readers need the text).
pub fn lopdf_permissions(p: PermissionsRequest) -> lopdf::Permissions {
    use lopdf::Permissions as P;
    let mut bits = P::COPYABLE_FOR_ACCESSIBILITY;
    for (on, flag) in [
        (p.print, P::PRINTABLE | P::PRINTABLE_IN_HIGH_QUALITY),
        (p.extract_text, P::COPYABLE),
        (p.modify, P::MODIFIABLE),
        (p.annotate, P::ANNOTABLE),
        (p.fill_forms, P::FILLABLE),
        (p.assemble, P::ASSEMBLABLE),
    ] {
        if on {
            bits |= flag;
        }
    }
    bits
}

/// Encrypts an unencrypted PDF with AES-256 (standard security handler V5 / R6, `/StdCF`,
/// metadata encrypted too) and a fresh random 32-byte file key.
pub fn encrypt_bytes(
    plain: &[u8],
    user_password: &str,
    owner_password: &str,
    permissions: PermissionsRequest,
) -> Result<Vec<u8>, EngineError> {
    let mut doc = lopdf::Document::load_mem(plain).map_err(|e| save::lopdf_error("parse", e))?;
    let mut file_key = [0u8; 32];
    getrandom::fill(&mut file_key)
        .map_err(|e| EngineError::io(format!("no system randomness for the file key: {e}")))?;
    let filter: Arc<dyn CryptFilter> = Arc::new(Aes256CryptFilter);
    let state = lopdf::EncryptionState::try_from(lopdf::EncryptionVersion::V5 {
        encrypt_metadata: true,
        crypt_filters: BTreeMap::from([(b"StdCF".to_vec(), filter)]),
        file_encryption_key: &file_key,
        stream_filter: b"StdCF".to_vec(),
        string_filter: b"StdCF".to_vec(),
        owner_password,
        user_password,
        permissions: lopdf_permissions(permissions),
    })
    // SASLprep rejections (prohibited code points) are the user's input, not a bug.
    .map_err(|e| EngineError::invalid(format!("password rejected: {e}")))?;
    doc.encrypt(&state)
        .map_err(|e| save::lopdf_error("encrypt", e))?;
    // AES-256 is a PDF 2.0 (ISO 32000-2) / Acrobat X feature; label older files 1.7 at least.
    if doc.version.as_str() < "1.7" {
        doc.version = "1.7".to_string();
    }
    let mut out = Vec::with_capacity(plain.len() + 1024);
    doc.save_to(&mut out)
        .map_err(|e| save::lopdf_error("write", e))?;
    Ok(out)
}
