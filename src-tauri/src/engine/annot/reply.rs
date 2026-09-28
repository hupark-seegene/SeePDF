//! `reply_annotation` — annotation threads (P2).
//!
//! A reply is a `Text` annotation whose `/IRT` ("in reply to") is an **indirect reference** to
//! the parent's dictionary and whose `/RT` is `/R` (ISO 32000-1 §12.5.6.2, table 170). That is
//! what Acrobat, Preview and pdf.js group into a thread.
//!
//! PDFium can *read* the link (`FPDFAnnot_GetLinkedAnnot`) but nothing in its public API can
//! write a reference into an annotation dictionary, so the reply is written with `lopdf` on the
//! serialised file, through [`registry::mutate_bytes`] — the byte-level mutation that
//! `set_metadata` uses (`STAGE3_SECURITY_NOTES.md` §2): one undo step, PDFium reopens the result
//! before it replaces the document, and a failure leaves the document as it was.
//!
//! The reply dictionary:
//!
//! * `/Rect` — a 20 pt note square at the parent's top-left corner (where Acrobat puts it);
//! * an **empty** normal appearance (`/AP << /N <empty Form> >>`): a conforming reader shows a
//!   reply inside its parent's thread, never as a second icon on the page, and PDFium — which
//!   does not know about threads — would otherwise generate a note icon over the parent. Other
//!   viewers see exactly the same thing. `update::in_place` writes the empty stream back after
//!   an edit, which clears the AP first;
//! * `/F 28` (Print | NoZoom | NoRotate, like Acrobat's replies), `/Name /Comment`;
//! * `/NM`, `/T`, `/Contents`, `/CreationDate`, `/M`, the parent's `/C` and SeePDF's colour
//!   mirror (`annot::KEY_COLOR`), `/P`.
//!
//! Encrypted documents are refused (`unsupported`), like `set_metadata`: `lopdf` would have to
//! re-encrypt the file, which needs the owner password.

use crate::engine::annot::{self, KEY_COLOR};
use crate::engine::registry::{self, MutateOpts};
use crate::engine::save;
use crate::engine::types::EngineState;
use crate::ipc::types::{AnnotId, AnnotKind, ChangeReason, PageIndex};
use crate::ipc::{EngineError, ErrorCode};
use lopdf::{Dictionary, Object, ObjectId, Stream};

/// Side of the reply's (invisible) note square, in points — `create::NOTE_SIZE`.
const REPLY_SIZE: f32 = 20.0;
/// Print | NoZoom | NoRotate.
const REPLY_FLAGS: i64 = 4 | 8 | 16;

/// What one reply says.
#[derive(Debug, Clone)]
pub struct ReplyRequest {
    pub page: PageIndex,
    pub parent_id: AnnotId,
    pub reply_id: AnnotId,
    pub contents: String,
    pub author: Option<String>,
    /// `D:YYYYMMDDHHmmSS+00'00'` — `/CreationDate` and `/M`.
    pub date: String,
}

/// `reply_annotation` — writes a reply to `parent_id` on `page` as one undoable edit and
/// returns the reply's id (`/NM`).
pub fn reply(
    st: &mut EngineState<'_>,
    doc_id: &str,
    page: PageIndex,
    parent_id: &str,
    contents: &str,
    author: Option<&str>,
) -> Result<AnnotId, EngineError> {
    {
        let doc = st.doc_mut(doc_id)?;
        if doc.encrypted || doc.password.is_some() {
            return Err(EngineError::new(
                ErrorCode::Unsupported,
                "replies cannot be written into an encrypted document",
            ));
        }
        // Cheap checks first: the heavy path serialises and reloads the whole document.
        let parent = annot::list(doc, page)?
            .into_iter()
            .find(|a| a.id == parent_id)
            .ok_or_else(|| {
                EngineError::not_found(format!("annotation '{parent_id}' is not on page {page}"))
                    .with_page(page)
            })?;
        if matches!(parent.kind, AnnotKind::Widget | AnnotKind::Link) {
            return Err(EngineError::invalid(
                "only a markup annotation can have replies (not a form field or a link)",
            ));
        }
    }
    let request = ReplyRequest {
        page,
        parent_id: parent_id.to_string(),
        reply_id: annot::new_id(),
        contents: contents.to_string(),
        author: author
            .map(str::trim)
            .filter(|a| !a.is_empty())
            .map(str::to_string),
        date: annot::pdf_date_now(),
    };
    let opts = MutateOpts::new("undo.annotReply", ChangeReason::Edit)
        .page(page)
        .keeps_text();
    let id = request.reply_id.clone();
    registry::mutate_bytes(st, doc_id, opts, |bytes, _| write_reply(bytes, &request))?;
    Ok(id)
}

/// The `lopdf` half: `bytes` (a file PDFium just serialised) with the reply added to the
/// page's `/Annots`. Pure, so it is unit-testable on any PDF.
pub fn write_reply(bytes: &[u8], req: &ReplyRequest) -> Result<Vec<u8>, EngineError> {
    let mut doc = lopdf::Document::load_mem(bytes).map_err(|e| save::lopdf_error("parse", e))?;
    let page_id = *doc
        .get_pages()
        .get(&(req.page as u32 + 1))
        .ok_or_else(|| EngineError::not_found(format!("page {}", req.page)).with_page(req.page))?;

    let mut annots = annots_of(&doc, page_id)?;
    let (index, parent) = annots
        .iter()
        .enumerate()
        .find_map(|(i, entry)| {
            let dict = resolve_dict(&doc, entry)?;
            (text_of(dict, b"NM").as_deref() == Some(req.parent_id.as_str())
                && dict.get(b"Subtype").and_then(Object::as_name).ok() != Some(b"Popup".as_slice()))
            .then(|| (i, dict.clone()))
        })
        .ok_or_else(|| {
            EngineError::not_found(format!(
                "annotation '{}' is not in the saved page",
                req.parent_id
            ))
            .with_page(req.page)
        })?;

    // `/IRT` must be an indirect reference: a parent that lives directly inside `/Annots` (a
    // foreign producer, or an annotation PDFium never rendered) becomes an object of its own.
    let parent_ref: ObjectId = match &annots[index] {
        Object::Reference(id) => *id,
        _ => {
            let id = doc.add_object(Object::Dictionary(parent.clone()));
            annots[index] = Object::Reference(id);
            id
        }
    };

    let (l, t) = top_left(&parent).unwrap_or((0.0, REPLY_SIZE));
    let rect = [l, t - REPLY_SIZE, l + REPLY_SIZE, t];

    // The empty appearance: a Form XObject with no content at all.
    let mut ap_dict = Dictionary::new();
    ap_dict.set("Type", Object::Name(b"XObject".to_vec()));
    ap_dict.set("Subtype", Object::Name(b"Form".to_vec()));
    ap_dict.set("BBox", reals(&[0.0, 0.0, REPLY_SIZE, REPLY_SIZE]));
    let ap_id = doc.add_object(Object::Stream(Stream::new(ap_dict, Vec::new())));
    let mut ap = Dictionary::new();
    ap.set("N", Object::Reference(ap_id));

    let mut reply = Dictionary::new();
    reply.set("Type", Object::Name(b"Annot".to_vec()));
    reply.set("Subtype", Object::Name(b"Text".to_vec()));
    reply.set("Rect", reals(&rect));
    reply.set("Contents", save::pdf_text_string(&req.contents));
    if let Some(author) = &req.author {
        reply.set("T", save::pdf_text_string(author));
    }
    reply.set("NM", save::pdf_text_string(&req.reply_id));
    reply.set("IRT", Object::Reference(parent_ref));
    reply.set("RT", Object::Name(b"R".to_vec()));
    reply.set("Name", Object::Name(b"Comment".to_vec()));
    reply.set("F", Object::Integer(REPLY_FLAGS));
    reply.set("Open", Object::Boolean(false));
    reply.set("CreationDate", save::pdf_text_string(&req.date));
    reply.set("M", save::pdf_text_string(&req.date));
    reply.set("P", Object::Reference(page_id));
    reply.set("AP", Object::Dictionary(ap));
    // The thread shares its parent's colour (the sidebar tints the reply icon with it).
    if let Ok(color) = parent.get(b"C") {
        reply.set("C", color.clone());
    }
    if let Ok(mirror) = parent.get(KEY_COLOR.as_bytes()) {
        reply.set(KEY_COLOR, mirror.clone());
    }
    let reply_ref = doc.add_object(Object::Dictionary(reply));
    annots.push(Object::Reference(reply_ref));
    store_annots(&mut doc, page_id, annots)?;

    let mut out = Vec::with_capacity(bytes.len() + 1024);
    doc.save_to(&mut out)
        .map_err(|e| save::lopdf_error("write", e))?;
    Ok(out)
}

/// The page's `/Annots` entries (the array may be inline or an indirect object).
fn annots_of(doc: &lopdf::Document, page_id: ObjectId) -> Result<Vec<Object>, EngineError> {
    let page = doc
        .get_dictionary(page_id)
        .map_err(|e| save::lopdf_error("page", e))?;
    match page.get(b"Annots") {
        Ok(Object::Array(items)) => Ok(items.clone()),
        Ok(Object::Reference(id)) => Ok(doc
            .get_object(*id)
            .and_then(Object::as_array)
            .cloned()
            .unwrap_or_default()),
        _ => Ok(Vec::new()),
    }
}

/// Writes `/Annots` back where it came from.
fn store_annots(
    doc: &mut lopdf::Document,
    page_id: ObjectId,
    annots: Vec<Object>,
) -> Result<(), EngineError> {
    let indirect = doc
        .get_dictionary(page_id)
        .ok()
        .and_then(|p| p.get(b"Annots").ok())
        .and_then(|o| o.as_reference().ok());
    if let Some(id) = indirect {
        if let Ok(target) = doc.get_object_mut(id) {
            if let Ok(array) = target.as_array_mut() {
                *array = annots;
                return Ok(());
            }
        }
    }
    doc.get_dictionary_mut(page_id)
        .map_err(|e| save::lopdf_error("page", e))?
        .set("Annots", Object::Array(annots));
    Ok(())
}

fn resolve_dict<'d>(doc: &'d lopdf::Document, entry: &'d Object) -> Option<&'d Dictionary> {
    match entry {
        Object::Reference(id) => doc.get_dictionary(*id).ok(),
        Object::Dictionary(d) => Some(d),
        _ => None,
    }
}

/// A text-string value (`/NM`, `/T`…), decoded from PDFDocEncoding or UTF-16BE.
fn text_of(dict: &Dictionary, key: &[u8]) -> Option<String> {
    dict.get(key)
        .ok()
        .and_then(|o| lopdf::decode_text_string(o).ok())
}

/// The top-left corner of an annotation's `/Rect` (normalised: the rectangle may be written
/// with its corners in any order).
fn top_left(dict: &Dictionary) -> Option<(f32, f32)> {
    let rect = dict.get(b"Rect").ok()?.as_array().ok()?;
    let v: Vec<f32> = rect.iter().filter_map(number).collect();
    if v.len() != 4 {
        return None;
    }
    Some((v[0].min(v[2]), v[1].max(v[3])))
}

fn number(o: &Object) -> Option<f32> {
    match o {
        Object::Integer(i) => Some(*i as f32),
        Object::Real(r) => Some(*r),
        _ => None,
    }
}

fn reals(values: &[f32]) -> Object {
    Object::Array(values.iter().map(|v| Object::Real(*v)).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn top_left_normalises_the_rect() {
        let mut d = Dictionary::new();
        d.set(
            "Rect",
            Object::Array(vec![
                Object::Integer(200),
                Object::Real(50.0),
                Object::Integer(100),
                Object::Real(80.5),
            ]),
        );
        assert_eq!(top_left(&d), Some((100.0, 80.5)));
        assert_eq!(top_left(&Dictionary::new()), None);
    }
}
