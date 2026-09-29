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
//!
//! v0.3 (pkg3-security-save-integrity) adds:
//!
//! * **S5 permissions** — [`ensure_permitted`] is the one guard every mutation passes
//!   (`registry::mutate` / `mutate_bytes` call it with [`perm_for`]); [`unlock`] reopens the
//!   document with its permissions (owner) password, keeping the docId and the undo history.
//! * **S1 signatures** — [`read_signatures`] (detection only, no cryptographic validation).
//! * **S2 encrypted rewrites** — [`Crypt`]: every lopdf rewrite of an encrypted document works
//!   on PDFium's decrypted serialisation and is re-encrypted with the document's own security
//!   handler state (same `/Encrypt` values, `/ID` and file key), so no owner password is
//!   needed and the permissions stay exactly as they were.

use crate::engine::raw::save::SaveFlags;
use crate::engine::registry::{self, MutateOpts, OpenDoc};
use crate::engine::save;
use crate::engine::types::EngineState;
use crate::ipc::types::{
    BytesWritten, ChangeReason, DocInfo, DocMeta, Permissions, PermissionsRequest, SignatureInfo,
};
use crate::ipc::{EngineError, ErrorCode};
use lopdf::encryption::crypt_filters::{Aes256CryptFilter, CryptFilter};
use std::collections::BTreeMap;
use std::sync::Arc;

/// PDF 2.0 §7.6.4.3.3: an R6 password is at most 127 UTF-8 bytes after SASLprep.
const MAX_PASSWORD_BYTES: usize = 127;

/// The pre-flight check of every lopdf rewrite (metadata, and the P2 outline / page labels /
/// go-to-page links in `engine::structure`). Checked before any undo entry is pushed.
///
/// Until v0.3 this refused every encrypted document. Since S2 an encrypted document is
/// rewritten through [`Crypt`] (decrypt with PDFium, re-encrypt with the file's own key), so
/// the only refusal left is the document's own permission bits: a rewrite is a modification.
/// An encryption lopdf cannot reproduce is refused later, inside the rewrite, with a clear
/// `unsupported` message.
pub(crate) fn refuse_encrypted(st: &EngineState<'_>, doc_id: &str) -> Result<(), EngineError> {
    ensure_permitted(st.doc(doc_id)?, Perm::Modify)
}

/// `set_metadata` — writes the `Some` fields of `meta` into `/Info` (see
/// [`save::write_info`] for the exact rules) as one undoable edit. On an encrypted document
/// the file stays encrypted with the same passwords and permissions (S2).
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
        ensure_security_change(doc)?;
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

// =======================================================================================
// v0.3 pkg3 — S5: permission flags
// =======================================================================================

/// One PDF permission bit (ISO 32000-2 table 22), as `DocInfo.permissions` decodes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Perm {
    Print,
    Modify,
    ExtractText,
    Annotate,
    FillForms,
    Assemble,
}

impl Perm {
    /// The `Permissions` field name, which is also the `detail` of a `permissionDenied`.
    pub fn name(self) -> &'static str {
        match self {
            Perm::Print => "print",
            Perm::Modify => "modify",
            Perm::ExtractText => "extractText",
            Perm::Annotate => "annotate",
            Perm::FillForms => "fillForms",
            Perm::Assemble => "assemble",
        }
    }

    pub fn allowed(self, p: &Permissions) -> bool {
        match self {
            Perm::Print => p.print,
            Perm::Modify => p.modify,
            Perm::ExtractText => p.extract_text,
            Perm::Annotate => p.annotate,
            Perm::FillForms => p.fill_forms,
            Perm::Assemble => p.assemble,
        }
    }
}

/// `permissionDenied` (detail = the permission's name) when the document's permission bits,
/// as the **current** open grants them, forbid `perm`. An unencrypted document, or one opened
/// with its owner password, grants everything.
pub fn ensure_permitted(doc: &OpenDoc<'_>, perm: Perm) -> Result<(), EngineError> {
    if perm.allowed(&doc.permissions) {
        return Ok(());
    }
    Err(EngineError::new(
        ErrorCode::PermissionDenied,
        format!(
            "the document's permissions forbid '{}'; unlock it with the permissions password",
            perm.name()
        ),
    )
    .with_detail(perm.name()))
}

/// v0.3 S5: changing a document's security (`set_password` on a copy, `remove_password`) would
/// hand out a copy without the restrictions, so a restricted open needs the permissions
/// password first — like Acrobat's "Change security settings". `permissionDenied`, detail
/// `security`, when any permission of this open is missing.
pub fn ensure_security_change(doc: &OpenDoc<'_>) -> Result<(), EngineError> {
    let p = &doc.permissions;
    if !doc.encrypted
        || (p.print && p.modify && p.extract_text && p.annotate && p.fill_forms && p.assemble)
    {
        return Ok(());
    }
    Err(EngineError::new(
        ErrorCode::PermissionDenied,
        "this document is restricted: unlock it with the permissions password to change its security",
    )
    .with_detail("security"))
}

/// v0.3 S5: a file used as a *source* of pages (병합, 다른 파일에서 페이지 삽입) lands in a
/// document that has none of its restrictions, so its open must grant every permission — an
/// unencrypted file, or one opened with its permissions (owner) password. `permissionDenied`,
/// detail `security`, otherwise (the same rule as changing a document's security).
pub fn ensure_source_unrestricted(
    bindings: &dyn pdfium_render::prelude::PdfiumLibraryBindings,
    source: &pdfium_render::prelude::PdfDocument<'_>,
    path: &std::path::Path,
) -> Result<(), EngineError> {
    ensure_unrestricted_permissions(
        &registry::permissions_of(bindings, source),
        &path.display().to_string(),
    )
}

/// [`ensure_source_unrestricted`] for a source that is already open (v0.3 integration, P3 × S5:
/// `import_pages_from_doc`, pages dragged in from another window): `name` names it in the error.
pub fn ensure_unrestricted_permissions(
    p: &crate::ipc::types::Permissions,
    name: &str,
) -> Result<(), EngineError> {
    if p.print && p.modify && p.extract_text && p.annotate && p.fill_forms && p.assemble {
        return Ok(());
    }
    Err(EngineError::new(
        ErrorCode::PermissionDenied,
        format!(
            "{name} is restricted: its pages cannot be copied into another document without its \
             permissions password"
        ),
    )
    .with_detail("security"))
}

/// [`ensure_permitted`] by document id — the one-line guard for `print_prepare` and
/// `export_text`, which do not go through `registry::mutate`.
pub fn ensure_doc_permitted(
    st: &EngineState<'_>,
    doc_id: &str,
    perm: Perm,
) -> Result<(), EngineError> {
    ensure_permitted(st.doc(doc_id)?, perm)
}

/// Which permission a mutation needs, from what `registry::mutate` already knows about it —
/// so no page / annotation / object module has to pass anything:
///
/// * a page-list change (`structural`, or `ChangeReason::Pages`: move, delete, rotate, insert,
///   crop, resize) is **assemble**;
/// * annotation create / edit / delete / reply (`undo.annot*`) is **annotate**;
/// * form filling (`undo.form*`) is **fill forms** (the form module also checks it);
/// * everything else — text and image objects, redaction, OCR, stamps, links, outline,
///   metadata, page labels, compression, sanitize, attachments — is **modify**.
pub fn perm_for(label: &str, reason: ChangeReason, structural: bool) -> Perm {
    if label.starts_with("undo.annot") {
        Perm::Annotate
    } else if label.starts_with("undo.form") {
        Perm::FillForms
    } else if structural || reason == ChangeReason::Pages {
        Perm::Assemble
    } else {
        Perm::Modify
    }
}

/// `unlock_document` (제한됨 › 권한 암호로 잠금 해제): reopens the document with `password`,
/// which must be its permissions (owner) password — one that opens it with every permission.
///
/// The docId, path, generation, dirty state and the undo / redo history are kept: the current
/// state is reloaded in place (`registry::replace`) with the new password, which every undo
/// snapshot also opens with. `passwordWrong` when the password does not open the file, or
/// opens it without full rights (that is the open password). `invalidArgument` on a document
/// that is not encrypted.
pub fn unlock(
    st: &mut EngineState<'_>,
    doc_id: &str,
    password: &str,
) -> Result<DocInfo, EngineError> {
    let (encrypted, current) = {
        let doc = st.doc(doc_id)?;
        let current = if doc.dirty() || doc.ids_stamped || doc.history.undo_depth() > 0 {
            doc.snapshot_bytes()?
        } else {
            doc.bytes.clone()
        };
        (doc.encrypted, current)
    };
    if !encrypted {
        return Err(EngineError::invalid("this document is not encrypted"));
    }
    // Probe first: the open document is only touched once the password is known to be right.
    let full = {
        let probe = st
            .pdfium
            .load_pdf_from_byte_slice(&current, Some(password))
            .map_err(|_| EngineError::new(ErrorCode::PasswordWrong, "the password was rejected"))?;
        let bindings = crate::engine::raw::bindings(st.pdfium);
        let p = registry::permissions_of(bindings, &probe);
        p.print && p.modify && p.extract_text && p.annotate && p.fill_forms && p.assemble
    };
    if !full {
        return Err(EngineError::new(
            ErrorCode::PasswordWrong,
            "this is the open password, not the permissions password",
        ));
    }
    let previous = {
        let doc = st.doc_mut(doc_id)?;
        doc.password.replace(password.to_string())
    };
    if let Err(e) = registry::replace(st, doc_id, current) {
        st.doc_mut(doc_id)?.password = previous;
        return Err(e);
    }
    Ok(st.doc(doc_id)?.info())
}

// =======================================================================================
// v0.3 pkg3 — S1: digital signatures (detection only)
// =======================================================================================

/// The document's signatures: PDFium's `FPDFSignatureObj_*` values for every signature field
/// that carries a signature (`/Contents`), plus each field's `/T` read with lopdf from
/// `bytes` (best effort: absent when the field order cannot be matched).
///
/// Nothing is validated cryptographically: a signature listed here may be broken, expired or
/// untrusted. The UI says so.
pub fn read_signatures(
    bindings: &dyn pdfium_render::prelude::PdfiumLibraryBindings,
    doc: &pdfium_render::prelude::PdfDocument<'_>,
    bytes: &[u8],
    password: Option<&str>,
) -> Vec<SignatureInfo> {
    let raw = crate::engine::raw::sig::read_all(bindings, doc);
    if raw.is_empty() {
        return Vec::new();
    }
    let names = signature_field_names(bytes, password);
    let names_fit = names.len() == raw.len();
    raw.into_iter()
        .enumerate()
        .filter(|(_, s)| s.signed)
        .map(|(i, s)| SignatureInfo {
            field_name: if names_fit { names[i].clone() } else { None },
            reason: s.reason,
            time: s.time,
            sub_filter: s.sub_filter,
        })
        .collect()
}

/// `/T` of every top-level `/FT /Sig` field, in `/AcroForm /Fields` order — the same walk
/// PDFium's `FPDF_GetSignatureObject` does, so index `i` here is PDFium's signature `i`.
fn signature_field_names(bytes: &[u8], password: Option<&str>) -> Vec<Option<String>> {
    use lopdf::Object;
    let options = lopdf::LoadOptions {
        password: password.map(str::to_owned),
        ..Default::default()
    };
    let Ok(doc) = lopdf::Document::load_mem_with_options(bytes, options) else {
        return Vec::new();
    };
    let resolve = |o: &Object| -> Option<lopdf::Dictionary> {
        match o {
            Object::Reference(id) => doc.get_dictionary(*id).ok().cloned(),
            Object::Dictionary(d) => Some(d.clone()),
            _ => None,
        }
    };
    let Some(acro) = doc
        .catalog()
        .ok()
        .and_then(|c| c.get(b"AcroForm").ok())
        .and_then(resolve)
    else {
        return Vec::new();
    };
    let Some(fields) = acro.get(b"Fields").ok().and_then(|f| match f {
        Object::Array(a) => Some(a.clone()),
        Object::Reference(id) => doc.get_object(*id).and_then(Object::as_array).ok().cloned(),
        _ => None,
    }) else {
        return Vec::new();
    };
    fields
        .iter()
        .filter_map(resolve)
        .filter(|f| f.get(b"FT").and_then(Object::as_name).ok() == Some(b"Sig".as_slice()))
        .map(|f| {
            f.get(b"T")
                .ok()
                .and_then(|t| lopdf::decode_text_string(t).ok())
                .filter(|t| !t.trim().is_empty())
        })
        .collect()
}

// =======================================================================================
// v0.3 pkg3 — S2: lopdf rewrites of encrypted documents
// =======================================================================================

/// The security handler state of an encrypted document, for re-encrypting a lopdf rewrite
/// with the **same** `/Encrypt` values (`/O`, `/U`, `/OE`, `/UE`, `/Perms`, `/P`, `/V`, `/R`,
/// crypt filters), the same `/ID` and the same file key — so every password and permission
/// of the file is exactly what it was, and no owner password is needed.
pub struct Crypt {
    state: lopdf::EncryptionState,
    id: Option<lopdf::Object>,
}

impl Crypt {
    /// Re-encrypts `plain` (an unencrypted file, typically `f`'s output in
    /// `registry::mutate_bytes`).
    pub fn encrypt(&self, plain: &[u8]) -> Result<Vec<u8>, EngineError> {
        let mut doc =
            lopdf::Document::load_mem(plain).map_err(|e| save::lopdf_error("parse", e))?;
        if doc.is_encrypted() || doc.encryption_state.is_some() {
            // Already the finished, encrypted file (a rewrite that did not use its input).
            return Ok(plain.to_vec());
        }
        // An R2–R4 file key is derived from /ID[0]: the rewritten file must keep it.
        if let Some(id) = &self.id {
            doc.trailer.set("ID", id.clone());
        }
        doc.encrypt(&self.state)
            .map_err(|e| save::lopdf_error("encrypt", e))?;
        let mut out = Vec::with_capacity(plain.len() + 1024);
        doc.save_to(&mut out)
            .map_err(|e| save::lopdf_error("write", e))?;
        Ok(out)
    }
}

/// The decrypted bytes of an encrypted document (`FPDF_SaveAsCopy` with
/// `FPDF_REMOVE_SECURITY`: PDFium does the decryption, whatever password the document was
/// opened with) and the [`Crypt`] to re-encrypt a rewrite of them.
///
/// `encrypted` is the same document serialised **with** its encryption (what
/// `save::serialize` returns); the handler state is decoded from it with lopdf, and the file
/// key is checked against PDFium's own decryption on a few streams before anything is
/// written. `unsupported` when the key cannot be derived — a non-standard security handler,
/// or an R2–R4 file opened with its owner password while it also has an open password (the
/// file key needs the open password there).
pub fn decrypt_for_rewrite(
    doc: &OpenDoc<'_>,
    encrypted: &[u8],
) -> Result<(Vec<u8>, Crypt), EngineError> {
    let plain = crate::engine::raw::save::save_as_copy(
        doc.bindings(),
        doc.pdf(),
        SaveFlags::RemoveSecurity,
    )?;
    let unsupported = |why: &str| {
        EngineError::new(
            ErrorCode::Unsupported,
            format!("this document's encryption cannot be rewritten: {why}"),
        )
    };
    let options = lopdf::LoadOptions {
        password: doc.password.clone(),
        ..Default::default()
    };
    let decoded = lopdf::Document::load_mem_with_options(encrypted, options)
        .map_err(|e| unsupported(&e.to_string()))?;
    if decoded.is_encrypted() {
        return Err(unsupported("the password does not decrypt it"));
    }
    let Some(state) = decoded.encryption_state.clone() else {
        return Err(unsupported("no standard security handler"));
    };
    // Check the derived key against PDFium's decryption on up to four streams.
    let reference = lopdf::Document::load_mem(&plain).map_err(|e| save::lopdf_error("parse", e))?;
    let mut compared = 0;
    for (id, object) in &decoded.objects {
        let Ok(stream) = object.as_stream() else {
            continue;
        };
        if stream.content.is_empty() || stream.dict.has_type(b"XRef") {
            continue;
        }
        let Ok(theirs) = reference.get_object(*id).and_then(lopdf::Object::as_stream) else {
            continue;
        };
        if theirs.content != stream.content {
            return Err(unsupported(
                "the file key needs the open password; reopen the file with it",
            ));
        }
        compared += 1;
        if compared == 4 {
            break;
        }
    }
    let id = decoded.trailer.get(b"ID").ok().cloned();
    Ok((plain, Crypt { state, id }))
}

/// The document as lopdf sees it, decrypted — for the read-only lopdf paths
/// (`get_page_labels`). An unencrypted document is parsed from `bytes`; an encrypted one from
/// PDFium's decrypted serialisation of the open document.
pub fn plain_lopdf(doc: &OpenDoc<'_>) -> Result<lopdf::Document, EngineError> {
    let bytes: std::borrow::Cow<'_, [u8]> = if doc.encrypted {
        std::borrow::Cow::Owned(crate::engine::raw::save::save_as_copy(
            doc.bindings(),
            doc.pdf(),
            SaveFlags::RemoveSecurity,
        )?)
    } else {
        std::borrow::Cow::Borrowed(&doc.bytes)
    };
    lopdf::Document::load_mem(&bytes).map_err(|e| save::lopdf_error("parse", e))
}
