//! 양식 평면화 (v0.3 F2): every form field becomes part of its page, in place, as one undo step
//! `undo.formFlatten`.
//!
//! Not `FPDFPage_Flatten`: it flattens **every** annotation — comments, highlights, stamps —
//! and drops the hidden ones along with the page's whole `/Annots`. This is the forms-only
//! version, a lopdf rewrite through [`registry::mutate_bytes`]:
//!
//! 1. per page, each widget's normal appearance (`/AP /N`, the `/AS` state for a checkbox or
//!    radio) is drawn into the page as a Form XObject, mapped from its `/BBox` × `/Matrix` onto
//!    the widget's `/Rect` (ISO 32000 §12.5.5) — hidden (`/F` 2) and no-view (`/F` 32) widgets
//!    are not drawn;
//! 2. the existing content is wrapped in `q … Q` so its graphics state cannot leak into the
//!    appended drawing;
//! 3. the widgets leave `/Annots` (every other annotation stays) and `/AcroForm` leaves the
//!    catalog; unreferenced field objects are pruned.
//!
//! The values drawn are the appearances PDFium wrote when each field was filled
//! (`save::serialize` kills focus first, so the last typed value is included).

use crate::engine::registry::{self, MutateOpts};
use crate::engine::structure::{self, refuse_encrypted};
use crate::engine::types::EngineState;
use crate::ipc::types::{ChangeReason, DocInfo};
use crate::ipc::{EngineError, ErrorCode};
use lopdf::{Dictionary, Document, Object, ObjectId, Stream};

/// `flatten_form` — see the module docs. `invalidArgument` when the document has no field.
pub fn flatten_form(st: &mut EngineState<'_>, doc_id: &str) -> Result<DocInfo, EngineError> {
    refuse_encrypted(st, doc_id)?;
    if super::list(st.doc_mut(doc_id)?, None)?.is_empty() {
        return Err(EngineError::invalid("the document has no form field"));
    }
    let opts = MutateOpts::new("undo.formFlatten", ChangeReason::Edit).all_pages();
    registry::mutate_bytes_checked(
        st,
        doc_id,
        opts,
        |bytes, _| flatten_bytes(bytes),
        |_, reopened| {
            if reopened.form().is_some() {
                return Err(EngineError::new(
                    ErrorCode::VerifyFailed,
                    "the flattened document still has a form",
                ));
            }
            Ok(())
        },
    )
}

fn number(o: &Object) -> Option<f32> {
    o.as_float()
        .ok()
        .or_else(|| o.as_i64().ok().map(|i| i as f32))
}

fn numbers(doc: &Document, o: Option<&Object>) -> Option<Vec<f32>> {
    let (_, o) = doc.dereference(o?).ok()?;
    Some(
        o.as_array()
            .ok()?
            .iter()
            .filter_map(|v| doc.dereference(v).ok().and_then(|(_, v)| number(v)))
            .collect(),
    )
}

/// The stream a widget shows: `/AP /N`, or its `/AS` entry when `/N` is a state dictionary.
fn appearance(doc: &Document, widget: &Dictionary) -> Option<ObjectId> {
    let (_, ap) = doc.dereference(widget.get(b"AP").ok()?).ok()?;
    let normal = ap.as_dict().ok()?.get(b"N").ok()?;
    if let Ok(id) = normal.as_reference() {
        if let Ok(Object::Stream(_)) = doc.get_object(id) {
            return Some(id);
        }
        if let Ok(Object::Dictionary(states)) = doc.get_object(id) {
            let state = widget.get(b"AS").and_then(Object::as_name).ok()?;
            return states.get(state).and_then(Object::as_reference).ok();
        }
        return None;
    }
    let states = normal.as_dict().ok()?;
    let state = widget.get(b"AS").and_then(Object::as_name).ok()?;
    states.get(state).and_then(Object::as_reference).ok()
}

/// `q a 0 0 d e f cm /Name Do Q` for one widget, or `None` when nothing can be drawn.
fn placement(doc: &Document, widget: &Dictionary, ap: ObjectId) -> Option<[f32; 4]> {
    let rect = numbers(doc, widget.get(b"Rect").ok())?;
    if rect.len() != 4 {
        return None;
    }
    let (rl, rb) = (rect[0].min(rect[2]), rect[1].min(rect[3]));
    let (rr, rt) = (rect[0].max(rect[2]), rect[1].max(rect[3]));
    let Ok(Object::Stream(stream)) = doc.get_object(ap) else {
        return None;
    };
    let bbox = numbers(doc, stream.dict.get(b"BBox").ok())?;
    if bbox.len() != 4 {
        return None;
    }
    let m = numbers(doc, stream.dict.get(b"Matrix").ok())
        .filter(|m| m.len() == 6)
        .unwrap_or_else(|| vec![1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
    let corners = [
        (bbox[0], bbox[1]),
        (bbox[2], bbox[1]),
        (bbox[0], bbox[3]),
        (bbox[2], bbox[3]),
    ];
    let mapped: Vec<(f32, f32)> = corners
        .iter()
        .map(|&(x, y)| (m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5]))
        .collect();
    let (bl, br) = mapped.iter().fold((f32::MAX, f32::MIN), |(lo, hi), p| {
        (lo.min(p.0), hi.max(p.0))
    });
    let (bb, bt) = mapped.iter().fold((f32::MAX, f32::MIN), |(lo, hi), p| {
        (lo.min(p.1), hi.max(p.1))
    });
    if br - bl < 1e-3 || bt - bb < 1e-3 || rr - rl < 1e-3 || rt - rb < 1e-3 {
        return None;
    }
    let sx = (rr - rl) / (br - bl);
    let sy = (rt - rb) / (bt - bb);
    Some([sx, sy, rl - sx * bl, rb - sy * bb])
}

fn annot_refs(doc: &Document, page: ObjectId) -> Vec<Object> {
    doc.get_dictionary(page)
        .ok()
        .and_then(|d| d.get(b"Annots").ok())
        .and_then(|a| doc.dereference(a).ok())
        .and_then(|(_, a)| a.as_array().ok().cloned())
        .unwrap_or_default()
}

/// The page's resources as an owned dictionary (inherited ones included).
fn resources(doc: &Document, page: ObjectId) -> Dictionary {
    let mut current = Some(page);
    for _ in 0..32 {
        let Some(id) = current else { break };
        let Ok(dict) = doc.get_dictionary(id) else {
            break;
        };
        if let Ok(res) = dict.get(b"Resources") {
            if let Ok((_, Object::Dictionary(d))) = doc.dereference(res) {
                return d.clone();
            }
        }
        current = dict.get(b"Parent").and_then(Object::as_reference).ok();
    }
    Dictionary::new()
}

pub(crate) fn flatten_bytes(bytes: &[u8]) -> Result<Vec<u8>, EngineError> {
    let mut doc = structure::load(bytes)?;
    let pages = structure::page_ids(&doc);
    let mut serial = 0usize;
    for page in pages {
        let annots = annot_refs(&doc, page);
        let mut keep: Vec<Object> = Vec::with_capacity(annots.len());
        let mut draws: Vec<(String, ObjectId, [f32; 4])> = Vec::new();
        for item in annots {
            let widget = match doc.dereference(&item) {
                Ok((_, Object::Dictionary(d)))
                    if d.get(b"Subtype").and_then(Object::as_name).ok() == Some(b"Widget") =>
                {
                    d.clone()
                }
                _ => {
                    keep.push(item);
                    continue;
                }
            };
            let flags = widget.get(b"F").and_then(Object::as_i64).unwrap_or(0);
            if flags & (2 | 32) != 0 {
                continue;
            }
            let Some(ap) = appearance(&doc, &widget) else {
                continue;
            };
            let Some(at) = placement(&doc, &widget, ap) else {
                continue;
            };
            serial += 1;
            draws.push((format!("SeeFlat{serial}"), ap, at));
        }

        let page_dict = doc
            .get_dictionary(page)
            .map_err(|e| crate::engine::save::lopdf_error("page", e))?;
        let had_widgets = page_dict.has(b"Annots") && {
            let before = annot_refs(&doc, page).len();
            before != keep.len()
        };
        if !had_widgets {
            continue;
        }

        if !draws.is_empty() {
            // Make sure each appearance is a proper Form XObject.
            for (_, ap, _) in &draws {
                if let Ok(Object::Stream(s)) = doc.get_object_mut(*ap) {
                    s.dict.set("Type", Object::Name(b"XObject".to_vec()));
                    s.dict.set("Subtype", Object::Name(b"Form".to_vec()));
                }
            }
            let mut res = resources(&doc, page);
            let mut xobjects = match res.get(b"XObject").ok().map(|x| doc.dereference(x)) {
                Some(Ok((_, Object::Dictionary(d)))) => d.clone(),
                _ => Dictionary::new(),
            };
            let mut ops = String::from("Q\n");
            for (name, ap, [sx, sy, tx, ty]) in &draws {
                xobjects.set(name.as_bytes().to_vec(), Object::Reference(*ap));
                ops.push_str(&format!(
                    "q {sx:.6} 0 0 {sy:.6} {tx:.4} {ty:.4} cm /{name} Do Q\n"
                ));
            }
            res.set("XObject", Object::Dictionary(xobjects));
            let open = doc.add_object(Stream::new(Dictionary::new(), b"q\n".to_vec()));
            let close = doc.add_object(Stream::new(Dictionary::new(), ops.into_bytes()));
            let existing: Vec<Object> = match doc
                .get_dictionary(page)
                .ok()
                .and_then(|d| d.get(b"Contents").ok().cloned())
            {
                Some(Object::Reference(r)) => match doc.get_object(r) {
                    Ok(Object::Array(items)) => items.clone(),
                    _ => vec![Object::Reference(r)],
                },
                Some(Object::Array(items)) => items,
                _ => Vec::new(),
            };
            let mut contents = vec![Object::Reference(open)];
            contents.extend(existing);
            contents.push(Object::Reference(close));
            let dict = doc
                .get_dictionary_mut(page)
                .map_err(|e| crate::engine::save::lopdf_error("page", e))?;
            dict.set("Resources", Object::Dictionary(res));
            dict.set("Contents", Object::Array(contents));
        }
        let dict = doc
            .get_dictionary_mut(page)
            .map_err(|e| crate::engine::save::lopdf_error("page", e))?;
        if keep.is_empty() {
            dict.remove(b"Annots");
        } else {
            dict.set("Annots", Object::Array(keep));
        }
    }
    doc.catalog_mut()
        .map_err(|e| crate::engine::save::lopdf_error("catalog", e))?
        .remove(b"AcroForm");
    doc.prune_objects();
    structure::write(doc)
}
