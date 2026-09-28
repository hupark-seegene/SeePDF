//! Document structure PDFium can read but not write (P2): the outline (`/Outlines`), page
//! labels (`/PageLabels`) and go-to-page link destinations (`/Dest`).
//!
//! PDFium's public API has no bookmark writer, no page-label writer and no way to give a Link
//! annotation a destination (`FPDFAnnot_SetURI` is the only link setter), so all three are
//! written the Stage 3 way (`STAGE3_SECURITY_NOTES.md` §2): [`registry::mutate_bytes_checked`]
//! serialises the document with PDFium, [`lopdf`] rewrites the bytes, PDFium reopens them and a
//! per-feature check compares what PDFium reads back with what was meant, then the open
//! document is replaced — one undo step, rolled back on any failure.
//!
//! What PDFium *can* write goes through PDFium: a URI link (`FPDFAnnot_SetURI`), moving a link
//! (`FPDFAnnot_SetRect`) and deleting one (`FPDFPage_RemoveAnnot`), all inside
//! [`registry::mutate`] — which is also why those three keep working on an encrypted document
//! while every lopdf path refuses it (`security::refuse_encrypted`).
//!
//! [`registry::mutate_bytes_checked`]: crate::engine::registry::mutate_bytes_checked
//! [`registry::mutate`]: crate::engine::registry::mutate

pub mod labels;
pub mod links;
pub mod outline;

use crate::engine::save;
use crate::ipc::types::{OutlineDest, PageIndex};
use crate::ipc::{EngineError, ErrorCode};
use lopdf::{Document, Object, ObjectId};

pub(crate) use crate::engine::security::refuse_encrypted;

/// Parses bytes PDFium just produced. An encrypted file is refused — lopdf would need the
/// owner password to write it back (callers check the open document first; this is the
/// belt-and-braces for the bytes themselves).
pub(crate) fn load(bytes: &[u8]) -> Result<Document, EngineError> {
    let doc = Document::load_mem(bytes).map_err(|e| save::lopdf_error("parse", e))?;
    if doc.is_encrypted() {
        return Err(EngineError::new(
            ErrorCode::Unsupported,
            "this document is encrypted: remove the password first",
        ));
    }
    Ok(doc)
}

/// Writes the rewritten document back to bytes.
pub(crate) fn write(mut doc: Document) -> Result<Vec<u8>, EngineError> {
    let mut out = Vec::new();
    doc.save_to(&mut out).map_err(|e| save::lopdf_error("write", e))?;
    Ok(out)
}

/// The page objects in page order: index `i` is page `i` (0-based).
pub(crate) fn page_ids(doc: &Document) -> Vec<ObjectId> {
    doc.get_pages().into_values().collect()
}

/// The object id of page `index`, or `invalidArgument`.
pub(crate) fn page_id(pages: &[ObjectId], index: PageIndex) -> Result<ObjectId, EngineError> {
    pages
        .get(index as usize)
        .copied()
        .ok_or_else(|| EngineError::invalid(format!("page {index} of {}", pages.len())))
}

/// An explicit destination: `[page /XYZ left top zoom]` (null for an absent value) when any of
/// x / y / zoom is given, `[page /Fit]` — the whole page — when none is.
///
/// `/Fit` rather than `/XYZ null null null` because "retain the current left / top" means
/// nothing useful on another page, and it reads back as `dest: None` — exactly what was sent.
pub(crate) fn dest_array(page: ObjectId, view: Option<OutlineDest>) -> Object {
    let view = view.unwrap_or_default();
    if view.x.is_none() && view.y.is_none() && view.zoom.is_none() {
        return Object::Array(vec![Object::Reference(page), Object::Name(b"Fit".to_vec())]);
    }
    let num = |v: Option<f32>| v.map(Object::Real).unwrap_or(Object::Null);
    Object::Array(vec![
        Object::Reference(page),
        Object::Name(b"XYZ".to_vec()),
        num(view.x),
        num(view.y),
        num(view.zoom),
    ])
}

/// `/A << /S /URI /URI (…) >>`.
pub(crate) fn uri_action(url: &str) -> Object {
    let mut action = lopdf::Dictionary::new();
    action.set("S", Object::Name(b"URI".to_vec()));
    action.set("URI", Object::string_literal(url.as_bytes().to_vec()));
    Object::Dictionary(action)
}

/// A URI is 7-bit ASCII (ISO 32000 §12.6.4.8): every byte outside `!`..`~` — spaces, controls
/// and the UTF-8 of Hangul — is percent-encoded, so `https://예.kr/a b` is written (and read
/// back) as `https://%EC%98%88.kr/a%20b`. An already-encoded URL passes through unchanged.
pub fn encode_uri(url: &str) -> String {
    let mut out = String::with_capacity(url.len());
    for b in url.trim().bytes() {
        if (0x21..0x7f).contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Finite, or `invalidArgument` (lopdf would write `NaN` / `inf`, which is not PDF).
pub(crate) fn finite(value: Option<f32>, what: &str) -> Result<Option<f32>, EngineError> {
    match value {
        Some(v) if !v.is_finite() => Err(EngineError::invalid(format!("{what} must be a finite number"))),
        other => Ok(other),
    }
}

/// A destination view as it will read back: non-finite rejected, zoom ≤ 0 dropped ("retain"),
/// all-absent → `None`.
pub(crate) fn normalize_view(view: Option<OutlineDest>) -> Result<Option<OutlineDest>, EngineError> {
    let Some(v) = view else {
        return Ok(None);
    };
    let out = OutlineDest {
        x: finite(v.x, "x")?,
        y: finite(v.y, "y")?,
        zoom: finite(v.zoom, "zoom")?.filter(|z| *z > 0.0),
    };
    if out.x.is_none() && out.y.is_none() && out.zoom.is_none() {
        return Ok(None);
    }
    Ok(Some(out))
}

/// Two coordinates are the same destination (PDFium parses the real lopdf wrote).
pub(crate) fn same_f32(a: Option<f32>, b: Option<f32>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => (a - b).abs() <= 0.01,
        _ => false,
    }
}

pub(crate) fn same_view(a: Option<OutlineDest>, b: Option<OutlineDest>) -> bool {
    let (a, b) = (a.unwrap_or_default(), b.unwrap_or_default());
    same_f32(a.x, b.x) && same_f32(a.y, b.y) && same_f32(a.zoom, b.zoom)
}

pub(crate) fn verify_failed(what: impl Into<String>) -> EngineError {
    EngineError::new(ErrorCode::VerifyFailed, what.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uri_is_percent_encoded_to_7_bit() {
        assert_eq!(encode_uri("https://example.com/a?b=c"), "https://example.com/a?b=c");
        assert_eq!(encode_uri(" https://예.kr/a b "), "https://%EC%98%88.kr/a%20b");
        assert_eq!(encode_uri("https://x.kr/%20"), "https://x.kr/%20");
    }

    #[test]
    fn dest_arrays() {
        let page = (7, 0);
        assert_eq!(
            dest_array(page, None),
            Object::Array(vec![Object::Reference(page), Object::Name(b"Fit".to_vec())])
        );
        let xyz = dest_array(page, Some(OutlineDest { x: None, y: Some(700.0), zoom: None }));
        assert_eq!(
            xyz,
            Object::Array(vec![
                Object::Reference(page),
                Object::Name(b"XYZ".to_vec()),
                Object::Null,
                Object::Real(700.0),
                Object::Null
            ])
        );
    }

    #[test]
    fn views_normalize_like_they_read_back() {
        assert_eq!(normalize_view(None).unwrap(), None);
        assert_eq!(normalize_view(Some(OutlineDest { x: None, y: None, zoom: Some(0.0) })).unwrap(), None);
        let v = normalize_view(Some(OutlineDest { x: Some(1.0), y: None, zoom: Some(-1.0) })).unwrap();
        assert_eq!(v, Some(OutlineDest { x: Some(1.0), y: None, zoom: None }));
        assert!(normalize_view(Some(OutlineDest { x: Some(f32::NAN), y: None, zoom: None })).is_err());
    }
}
