//! v0.3 (pkg1, verification round 2): pages whose `/Contents` streams break mid-object.
//!
//! A page's `/Contents` may be an array of streams, and the PDF spec only asks that they
//! break between *tokens*: `fixtures/160F-2019.pdf` p.0 splits text objects across streams
//! (`…(que)]` ends one, `TJ ET EMC` starts the next). PDFium parses the concatenation fine,
//! but when it regenerates only the streams whose objects changed, the neighbour keeps its
//! half of the split object — a dangling `[…] TJ ET EMC Q` that breaks the parse of
//! everything after it (36 lines of 160F lost for one redacted word, silently).
//!
//! Before the apply regenerates such a page, its streams are joined into **one** stream with
//! `lopdf` ([`join_split_streams`]), so PDFium regenerates the page as a whole. A page whose
//! streams all end between objects is left alone: PDFium regenerates only what changed there,
//! and that is safe. An old stream nothing else refers to is dropped from the file — it holds
//! the text being redacted.

use crate::ipc::types::PageIndex;
use crate::ipc::{EngineError, ErrorCode};
use lopdf::content::Content;
use lopdf::{Document, Object, ObjectId, Stream};
use std::collections::HashSet;

fn lopdf_error(what: &str, e: lopdf::Error) -> EngineError {
    EngineError::new(ErrorCode::Pdfium, format!("content streams: {what}: {e}"))
}

/// Does this content stream (decoded) end — or start — inside an object? Anything `lopdf`
/// cannot parse on its own counts as a break (joining is harmless; a stream that parses is
/// never joined for no reason).
pub fn breaks_mid_object(data: &[u8]) -> bool {
    let Ok(content) = Content::decode_strict(data) else {
        return true;
    };
    let mut in_text = false;
    let mut marked: usize = 0;
    for op in &content.operations {
        match op.operator.as_str() {
            "BT" if in_text => return true,
            "BT" => in_text = true,
            "ET" if !in_text => return true,
            "ET" => in_text = false,
            "Tj" | "TJ" | "'" | "\"" | "Td" | "TD" | "Tm" | "T*" if !in_text => return true,
            "BMC" | "BDC" => marked += 1,
            "EMC" if marked == 0 => return true,
            "EMC" => marked -= 1,
            _ => {}
        }
        let wants = match op.operator.as_str() {
            "Tj" | "TJ" | "'" => 1,
            "\"" => 3,
            _ => 0,
        };
        if op.operands.len() < wants {
            return true;
        }
    }
    in_text || marked > 0
}

/// The stream ids of a page's `/Contents` and, when it is an indirect array, that array's id.
fn content_ids(doc: &Document, page_id: ObjectId) -> Option<(Vec<ObjectId>, Option<ObjectId>)> {
    let contents = doc.get_dictionary(page_id).ok()?.get(b"Contents").ok()?;
    match contents {
        Object::Reference(id) => match doc.get_object(*id).ok()? {
            Object::Array(items) => Some((
                items.iter().filter_map(|o| o.as_reference().ok()).collect(),
                Some(*id),
            )),
            _ => None,
        },
        Object::Array(items) => Some((
            items.iter().filter_map(|o| o.as_reference().ok()).collect(),
            None,
        )),
        _ => None,
    }
}

/// Every object id referred to anywhere in `doc` (trailer included).
fn referenced(doc: &Document) -> HashSet<ObjectId> {
    fn walk(o: &Object, out: &mut HashSet<ObjectId>) {
        match o {
            Object::Reference(id) => {
                out.insert(*id);
            }
            Object::Array(items) => items.iter().for_each(|i| walk(i, out)),
            Object::Dictionary(d) => d.iter().for_each(|(_, v)| walk(v, out)),
            Object::Stream(s) => s.dict.iter().for_each(|(_, v)| walk(v, out)),
            _ => {}
        }
    }
    let mut out = HashSet::new();
    for o in doc.objects.values() {
        walk(o, &mut out);
    }
    doc.trailer.iter().for_each(|(_, v)| walk(v, &mut out));
    out
}

/// Joins the `/Contents` streams of every page of `pages` whose streams break mid-object
/// ([`breaks_mid_object`]) into one stream. `None` when there is nothing to do — no such page,
/// or an encrypted file (`lopdf` would need the key to write it back; the apply's page-wide
/// post-condition still refuses a redaction that would lose text there).
pub fn join_split_streams(
    bytes: &[u8],
    pages: &[PageIndex],
) -> Result<Option<Vec<u8>>, EngineError> {
    let mut doc = Document::load_mem(bytes).map_err(|e| lopdf_error("parse", e))?;
    if doc.is_encrypted() {
        return Ok(None);
    }
    let page_ids = doc.get_pages();
    let mut dropped: Vec<ObjectId> = Vec::new();
    let mut changed = false;
    for &page in pages {
        let Some(&page_id) = page_ids.get(&(page as u32 + 1)) else {
            continue;
        };
        let Some((ids, array)) = content_ids(&doc, page_id) else {
            continue;
        };
        if ids.len() < 2 {
            continue;
        }
        let mut data: Vec<Vec<u8>> = Vec::with_capacity(ids.len());
        for id in &ids {
            let Ok(stream) = doc.get_object(*id).and_then(Object::as_stream) else {
                data.clear();
                break;
            };
            // A filter lopdf cannot undo: leave the page alone (the post-condition decides).
            let Ok(plain) = (if stream.dict.has(b"Filter") {
                stream.decompressed_content()
            } else {
                Ok(stream.content.clone())
            }) else {
                data.clear();
                break;
            };
            data.push(plain);
        }
        if data.len() != ids.len() || !data.iter().any(|d| breaks_mid_object(d)) {
            continue;
        }
        let mut joined = Vec::with_capacity(data.iter().map(Vec::len).sum::<usize>() + ids.len());
        for d in &data {
            joined.extend_from_slice(d);
            joined.push(b'\n');
        }
        let mut stream = Stream::new(lopdf::Dictionary::new(), joined);
        let _ = stream.compress();
        let new_id = doc.add_object(stream);
        doc.get_dictionary_mut(page_id)
            .map_err(|e| lopdf_error("page", e))?
            .set("Contents", Object::Reference(new_id));
        dropped.extend(ids);
        dropped.extend(array);
        changed = true;
    }
    if !changed {
        return Ok(None);
    }
    // The old streams hold the text being redacted: nothing may keep them in the file unless
    // another page still draws them.
    let still = referenced(&doc);
    for id in dropped {
        if !still.contains(&id) {
            doc.objects.remove(&id);
        }
    }
    let mut out = Vec::new();
    doc.save_to(&mut out).map_err(|e| {
        EngineError::new(
            ErrorCode::Pdfium,
            format!("content streams: write the document: {e}"),
        )
    })?;
    Ok(Some(out))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stream_that_ends_or_starts_inside_an_object_breaks() {
        assert!(!breaks_mid_object(b"q BT /F1 10 Tf (a) Tj ET Q"));
        assert!(!breaks_mid_object(b"/P <</MCID 0>> BDC BT (a) Tj ET EMC"));
        // 160F-2019.pdf: `…(que)]` ends a stream, `TJ ET EMC` starts the next.
        assert!(breaks_mid_object(b"BT /F1 10 Tf [(par)-3(voie)(que)]"));
        assert!(breaks_mid_object(b"TJ ET EMC Q"));
        assert!(breaks_mid_object(b"BT /F1 10 Tf"));
        assert!(breaks_mid_object(b"(a) Tj ET"));
        assert!(breaks_mid_object(b"/P <</MCID 0>> BDC"));
        // q / Q may span streams (a watermark stream wrapping the page): not a break.
        assert!(!breaks_mid_object(b"q 1 0 0 1 0 0 cm"));
    }

    fn two_stream_doc(first: &str, second: &str) -> Vec<u8> {
        use lopdf::dictionary;
        let mut doc = Document::with_version("1.7");
        let pages_id = doc.new_object_id();
        let a = doc.add_object(Stream::new(dictionary! {}, first.as_bytes().to_vec()));
        let b = doc.add_object(Stream::new(dictionary! {}, second.as_bytes().to_vec()));
        let page = doc.add_object(dictionary! {
            "Type" => "Page", "Parent" => pages_id,
            "MediaBox" => vec![0.into(), 0.into(), 200.into(), 200.into()],
            "Contents" => vec![a.into(), b.into()],
        });
        doc.objects.insert(
            pages_id,
            Object::Dictionary(
                dictionary! { "Type" => "Pages", "Kids" => vec![page.into()], "Count" => 1 },
            ),
        );
        let catalog = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
        doc.trailer.set("Root", catalog);
        let mut out = Vec::new();
        doc.save_to(&mut out).unwrap();
        out
    }

    #[test]
    fn split_streams_are_joined_and_the_old_ones_dropped() {
        let bytes = two_stream_doc("BT /F1 10 Tf [(secret)", "] TJ ET");
        let out = join_split_streams(&bytes, &[0]).unwrap().expect("joined");
        let doc = Document::load_mem(&out).unwrap();
        let page_id = *doc.get_pages().get(&1).unwrap();
        assert_eq!(doc.get_page_contents(page_id).len(), 1);
        let content = doc.get_page_content(page_id);
        assert!(!breaks_mid_object(&content));
        // One content stream left in the whole file: the old halves are gone.
        let streams: Vec<_> = doc
            .objects
            .iter()
            .filter(|(_, o)| {
                o.as_stream().is_ok_and(|s| {
                    s.dict.get(b"Type").ok() != Some(&Object::Name(b"XRef".to_vec()))
                })
            })
            .collect();
        assert_eq!(streams.len(), 1, "{streams:?}");
    }

    #[test]
    fn streams_that_end_between_objects_are_left_alone() {
        let bytes = two_stream_doc("q BT (a) Tj ET", "BT (b) Tj ET Q");
        assert!(join_split_streams(&bytes, &[0]).unwrap().is_none());
    }
}
