//! Digital signature reads (v0.3 S1): `FPDF_GetSignatureCount` / `FPDFSignatureObj_*`.
//!
//! pdfium-render has `reason()` and `signing_date()` but no `/SubFilter` accessor, and it
//! `assert_eq!`s on the second call's length — a malformed file must never panic the engine
//! thread, so all four reads live here with plain length checks instead.

use pdfium_render::prelude::{PdfDocument, PdfiumLibraryBindings, FPDF_SIGNATURE};
use std::os::raw::{c_char, c_ulong, c_void};

/// One signature's raw strings; `None` where the dictionary has no such entry.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RawSignature {
    /// The field carries a signature (`/V` with non-empty `/Contents`). PDFium also counts
    /// empty signature fields, which are not signatures.
    pub signed: bool,
    pub reason: Option<String>,
    pub time: Option<String>,
    pub sub_filter: Option<String>,
}

/// `FPDF_GetSignatureCount` (0 on a document without signatures, and on error).
pub fn count(bindings: &dyn PdfiumLibraryBindings, doc: &PdfDocument<'_>) -> usize {
    // SAFETY: a live document handle on the engine thread.
    let n = unsafe { bindings.FPDF_GetSignatureCount(doc.raw_handle()) };
    n.max(0) as usize
}

/// Every signature of `doc`, in PDFium's order (the order of the signature fields).
pub fn read_all(bindings: &dyn PdfiumLibraryBindings, doc: &PdfDocument<'_>) -> Vec<RawSignature> {
    (0..count(bindings, doc))
        .filter_map(|i| {
            // SAFETY: as above; `i` is below the count PDFium just reported.
            let sig = unsafe { bindings.FPDF_GetSignatureObject(doc.raw_handle(), i as i32) };
            (!sig.is_null()).then(|| read_one(bindings, sig))
        })
        .collect()
}

fn read_one(bindings: &dyn PdfiumLibraryBindings, sig: FPDF_SIGNATURE) -> RawSignature {
    // SAFETY (all three closures): `sig` is a live signature handle of a live document, the
    // buffer is `len` bytes long and PDFium writes at most `len` bytes into it.
    let reason = read_buffer(|buf, len| unsafe {
        bindings.FPDFSignatureObj_GetReason(sig, buf as *mut c_void, len)
    })
    .and_then(utf16le);
    let time = read_buffer(|buf, len| unsafe {
        bindings.FPDFSignatureObj_GetTime(sig, buf as *mut c_char, len)
    })
    .and_then(ascii);
    let sub_filter = read_buffer(|buf, len| unsafe {
        bindings.FPDFSignatureObj_GetSubFilter(sig, buf as *mut c_char, len)
    })
    .and_then(ascii);
    // SAFETY: a null buffer of length 0 only asks for the length.
    let contents = unsafe { bindings.FPDFSignatureObj_GetContents(sig, std::ptr::null_mut(), 0) };
    RawSignature {
        signed: contents > 0,
        reason,
        time,
        sub_filter,
    }
}

/// The two-call pattern: ask for the length with a null buffer, then fill a buffer of it.
fn read_buffer(call: impl Fn(*mut u8, c_ulong) -> c_ulong) -> Option<Vec<u8>> {
    let len = call(std::ptr::null_mut(), 0);
    if len == 0 || len > 1 << 20 {
        return None;
    }
    let mut buf = vec![0u8; len as usize];
    let written = call(buf.as_mut_ptr(), len);
    if written == 0 || written > len {
        return None;
    }
    buf.truncate(written as usize);
    Some(buf)
}

/// A NUL-terminated byte string (`/M`, `/SubFilter`).
fn ascii(bytes: Vec<u8>) -> Option<String> {
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    let s = String::from_utf8_lossy(&bytes[..end]).trim().to_string();
    (!s.is_empty()).then_some(s)
}

/// A NUL-terminated UTF-16LE string (`/Reason`).
fn utf16le(bytes: Vec<u8>) -> Option<String> {
    let units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .take_while(|&u| u != 0)
        .collect();
    let s = String::from_utf16_lossy(&units).trim().to_string();
    (!s.is_empty()).then_some(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_the_two_string_encodings() {
        assert_eq!(
            ascii(b"adbe.pkcs7.detached\0".to_vec()).as_deref(),
            Some("adbe.pkcs7.detached")
        );
        assert_eq!(ascii(b"\0".to_vec()), None);
        let mut wide: Vec<u8> = "승인"
            .encode_utf16()
            .flat_map(|u| u.to_le_bytes())
            .collect();
        wide.extend([0, 0]);
        assert_eq!(utf16le(wide).as_deref(), Some("승인"));
    }
}
