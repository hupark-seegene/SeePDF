//! 문서 정리 (v0.3 S3, `sanitize_document`): removes what should not leave the organisation —
//! JavaScript, embedded files, automatic actions, hidden metadata and hidden layers.
//!
//! PDFium can write none of it, so this is a lopdf rewrite through
//! [`registry::mutate_bytes`]: one undo step, PDFium reopens the result before it replaces the
//! document, and an encrypted document stays encrypted with the same passwords and
//! permissions (S2). A dry run on the current serialisation comes first, so a document with
//! nothing to remove gets no undo step.
//!
//! What each option removes ([`sanitize_bytes`]):
//!
//! * **javascript** — `/Names /JavaScript`, and every JavaScript action (`/S /JavaScript`, or
//!   any action with a `/JS` entry) wherever it hangs: `/OpenAction`, `/A`, `/AA` entries,
//!   `/Next` chains are dropped with the action that owns them.
//! * **attachments** — `/Names /EmbeddedFiles`, the catalog's `/AF`, and every
//!   `FileAttachment` annotation (with its popup).
//! * **actions** — `/OpenAction` when it is an action (a plain destination only sets the first
//!   view and is kept) and every `/AA` (catalog, page, annotation, form field).
//! * **metadata** — the trailer's `/Info`, every `/Metadata` stream (the XMP packet and any
//!   per-object one) and every `/PieceInfo` (private application data).
//! * **hiddenLayers** — optional-content groups that are off in the default configuration:
//!   the marked-content sections that draw them (`/OC /name BDC … EMC`) and the XObjects
//!   tagged with them are deleted from the page content, then `/OCProperties` goes, so no
//!   layer is hidden any more. A page whose content lopdf cannot parse is left as it was
//!   (and `/OCProperties` is then kept, so its hidden content stays hidden rather than
//!   appearing); content inside Form XObjects is not rewritten.
//!
//! Objects nothing refers to any more (the removed scripts, file streams, XMP) are pruned, so
//! they are gone from the file, not merely unlinked.

use crate::engine::registry::{self, MutateOpts};
use crate::engine::save;
use crate::engine::types::EngineState;
use crate::ipc::types::{ChangeReason, SanitizeCounts, SanitizeOptions, SanitizeResult};
use crate::ipc::EngineError;
use lopdf::content::{Content, Operation};
use lopdf::{Dictionary, Document, Object, ObjectId};
use std::collections::{BTreeSet, HashSet};

/// `sanitize_document` — one undo step (`undo.sanitize`), or none when nothing was found.
pub fn sanitize(
    st: &mut EngineState<'_>,
    doc_id: &str,
    options: &SanitizeOptions,
) -> Result<SanitizeResult, EngineError> {
    crate::engine::security::refuse_encrypted(st, doc_id)?;
    // Dry run on exactly what the rewrite would see.
    let current = save::serialize(st, doc_id)?;
    let plain = {
        let doc = st.doc(doc_id)?;
        if doc.encrypted {
            crate::engine::security::decrypt_for_rewrite(doc, &current)?.0
        } else {
            current
        }
    };
    let (_, found) = sanitize_bytes(&plain, options)?;
    if found.total() == 0 {
        return Ok(SanitizeResult {
            removed: found,
            info: st.doc(doc_id)?.info(),
        });
    }
    let opts = MutateOpts::new("undo.sanitize", ChangeReason::Edit).all_pages();
    let mut removed = SanitizeCounts::default();
    let info = registry::mutate_bytes(st, doc_id, opts, |bytes, _| {
        let (out, counts) = sanitize_bytes(bytes, options)?;
        removed = counts;
        Ok(out)
    })?;
    Ok(SanitizeResult { removed, info })
}

/// The lopdf half: `bytes` (an unencrypted file) with everything `options` names removed,
/// and how many entries of each kind went. Pure, so it is unit-testable on any PDF.
pub fn sanitize_bytes(
    bytes: &[u8],
    options: &SanitizeOptions,
) -> Result<(Vec<u8>, SanitizeCounts), EngineError> {
    let mut doc = Document::load_mem(bytes).map_err(|e| save::lopdf_error("parse", e))?;
    let mut counts = SanitizeCounts::default();
    let root = doc
        .trailer
        .get(b"Root")
        .and_then(Object::as_reference)
        .map_err(|e| save::lopdf_error("/Root", e))?;

    // ---- catalog-level entries ------------------------------------------------------
    let names_id = catalog_entry_id(&doc, root, b"Names");
    if options.javascript {
        counts.javascript += remove_name_tree(&mut doc, root, names_id, b"JavaScript");
    }
    if options.attachments {
        counts.attachments += remove_name_tree(&mut doc, root, names_id, b"EmbeddedFiles");
        if let Ok(catalog) = doc.get_dictionary_mut(root) {
            if catalog.remove(b"AF").is_some() {
                counts.attachments += 1;
            }
        }
    }
    if options.metadata && doc.trailer.remove(b"Info").is_some() {
        counts.metadata += 1;
    }
    if options.hidden_layers {
        counts.hidden_layers += remove_hidden_layers(&mut doc, root);
    }

    // ---- every dictionary in the file -----------------------------------------------
    // Decide with shared borrows first, then apply.
    let mut drop_keys: Vec<(ObjectId, &'static [u8])> = Vec::new();
    let mut aa_js: Vec<(ObjectId, Vec<Vec<u8>>)> = Vec::new();
    let mut annot_owners: Vec<ObjectId> = Vec::new();
    let file_attachments: HashSet<ObjectId> = if options.attachments {
        doc.objects
            .iter()
            .filter(|(_, o)| {
                o.as_dict().is_ok_and(|d| {
                    d.get(b"Subtype").and_then(Object::as_name).ok() == Some(b"FileAttachment")
                })
            })
            .map(|(id, _)| *id)
            .collect()
    } else {
        HashSet::new()
    };
    for (&id, object) in &doc.objects {
        let Some(dict) = dict_of(object) else {
            continue;
        };
        if options.javascript {
            for key in [b"A".as_slice(), b"OpenAction", b"Next"] {
                if dict.get(key).is_ok_and(|a| is_js_action(&doc, a)) {
                    drop_keys.push((id, static_key(key)));
                    counts.javascript += 1;
                }
            }
        }
        if options.actions {
            if dict.has(b"AA") {
                drop_keys.push((id, b"AA"));
                counts.actions += 1;
            }
            if id == root && dict.get(b"OpenAction").is_ok_and(|a| is_action(&doc, a)) {
                // A JavaScript /OpenAction was already counted as JavaScript above.
                if !(options.javascript
                    && dict.get(b"OpenAction").is_ok_and(|a| is_js_action(&doc, a)))
                {
                    drop_keys.push((id, b"OpenAction"));
                    counts.actions += 1;
                }
            }
        } else if options.javascript {
            // Keep the /AA dictionary but drop its JavaScript entries.
            if let Some(aa) = dict.get(b"AA").ok().and_then(|o| resolve_dict(&doc, o)) {
                let keys: Vec<Vec<u8>> = aa
                    .iter()
                    .filter(|(_, v)| is_js_action(&doc, v))
                    .map(|(k, _)| k.clone())
                    .collect();
                if !keys.is_empty() {
                    counts.javascript += keys.len() as u32;
                    aa_js.push((id, keys));
                }
            }
        }
        if options.metadata {
            for key in [b"Metadata".as_slice(), b"PieceInfo"] {
                if dict.has(key) {
                    drop_keys.push((id, static_key(key)));
                    counts.metadata += 1;
                }
            }
        }
        if !file_attachments.is_empty() && dict.has(b"Annots") {
            annot_owners.push(id);
        }
    }
    for (id, key) in drop_keys {
        if let Some(dict) = doc.objects.get_mut(&id).and_then(dict_of_mut) {
            dict.remove(key);
        }
    }
    for (id, keys) in aa_js {
        let aa_ref = doc
            .objects
            .get(&id)
            .and_then(dict_of)
            .and_then(|d| d.get(b"AA").ok().cloned());
        let target = match aa_ref {
            Some(Object::Reference(aa_id)) => doc.objects.get_mut(&aa_id).and_then(dict_of_mut),
            Some(Object::Dictionary(_)) => doc
                .objects
                .get_mut(&id)
                .and_then(dict_of_mut)
                .and_then(|d| d.get_mut(b"AA").ok())
                .and_then(|o| o.as_dict_mut().ok()),
            _ => None,
        };
        if let Some(aa) = target {
            for k in keys {
                aa.remove(&k);
            }
        }
    }
    // FileAttachment annotations (and their popups) out of every /Annots.
    if !file_attachments.is_empty() {
        let popups: HashSet<ObjectId> = file_attachments
            .iter()
            .filter_map(|id| doc.get_dictionary(*id).ok())
            .filter_map(|d| d.get(b"Popup").and_then(Object::as_reference).ok())
            .collect();
        for owner in annot_owners {
            let annots = doc
                .objects
                .get(&owner)
                .and_then(dict_of)
                .and_then(|d| d.get(b"Annots").ok().cloned());
            let (array_id, items) = match annots {
                Some(Object::Array(items)) => (None, items),
                Some(Object::Reference(r)) => match doc.get_object(r).and_then(Object::as_array) {
                    Ok(items) => (Some(r), items.clone()),
                    Err(_) => continue,
                },
                _ => continue,
            };
            let kept: Vec<Object> = items
                .iter()
                .filter(|o| match o {
                    Object::Reference(r) => !file_attachments.contains(r) && !popups.contains(r),
                    _ => true,
                })
                .cloned()
                .collect();
            let gone = (items.len() - kept.len()) as u32;
            if gone == 0 {
                continue;
            }
            counts.attachments += items
                .iter()
                .filter(|o| matches!(o, Object::Reference(r) if file_attachments.contains(r)))
                .count() as u32;
            match array_id {
                Some(r) => {
                    if let Some(slot) = doc.objects.get_mut(&r) {
                        *slot = Object::Array(kept);
                    }
                }
                None => {
                    if let Some(d) = doc.objects.get_mut(&owner).and_then(dict_of_mut) {
                        d.set("Annots", Object::Array(kept));
                    }
                }
            }
        }
    }

    // Whatever nothing points at any more (scripts, file streams, XMP) leaves the file.
    doc.prune_objects();
    let mut out = Vec::with_capacity(bytes.len());
    doc.save_to(&mut out)
        .map_err(|e| save::lopdf_error("write", e))?;
    Ok((out, counts))
}

fn static_key(key: &[u8]) -> &'static [u8] {
    match key {
        b"A" => b"A",
        b"OpenAction" => b"OpenAction",
        b"Next" => b"Next",
        b"Metadata" => b"Metadata",
        _ => b"PieceInfo",
    }
}

fn dict_of(object: &Object) -> Option<&Dictionary> {
    match object {
        Object::Dictionary(d) => Some(d),
        Object::Stream(s) => Some(&s.dict),
        _ => None,
    }
}

fn dict_of_mut(object: &mut Object) -> Option<&mut Dictionary> {
    match object {
        Object::Dictionary(d) => Some(d),
        Object::Stream(s) => Some(&mut s.dict),
        _ => None,
    }
}

fn resolve_dict<'a>(doc: &'a Document, object: &'a Object) -> Option<&'a Dictionary> {
    match object {
        Object::Reference(id) => doc.get_object(*id).ok().and_then(dict_of),
        other => dict_of(other),
    }
}

/// An action dictionary (`/S` present). A destination array is not an action.
fn is_action(doc: &Document, object: &Object) -> bool {
    resolve_dict(doc, object).is_some_and(|d| d.has(b"S"))
}

/// `/S /JavaScript`, or any action carrying a `/JS` entry (a Rendition action can).
fn is_js_action(doc: &Document, object: &Object) -> bool {
    resolve_dict(doc, object).is_some_and(|d| {
        d.get(b"S").and_then(Object::as_name).ok() == Some(b"JavaScript") || d.has(b"JS")
    })
}

/// The object id of `catalog[key]` when it is an indirect dictionary.
fn catalog_entry_id(doc: &Document, root: ObjectId, key: &[u8]) -> Option<ObjectId> {
    doc.get_dictionary(root)
        .ok()?
        .get(key)
        .and_then(Object::as_reference)
        .ok()
}

/// Removes `/Names /<tree>` and returns how many leaf entries it had.
fn remove_name_tree(
    doc: &mut Document,
    root: ObjectId,
    names_id: Option<ObjectId>,
    tree: &[u8],
) -> u32 {
    let names = match names_id {
        Some(id) => doc.get_dictionary(id).ok().cloned(),
        None => doc
            .get_dictionary(root)
            .ok()
            .and_then(|c| c.get(b"Names").ok())
            .and_then(|n| n.as_dict().ok())
            .cloned(),
    };
    let Some(entry) = names.and_then(|n| n.get(tree).ok().cloned()) else {
        return 0;
    };
    let count = count_name_tree(doc, &entry, 0).max(1);
    let target = match names_id {
        Some(id) => doc.get_dictionary_mut(id).ok(),
        None => doc
            .get_dictionary_mut(root)
            .ok()
            .and_then(|c| c.get_mut(b"Names").ok())
            .and_then(|n| n.as_dict_mut().ok()),
    };
    if let Some(names) = target {
        names.remove(tree);
    }
    count
}

fn count_name_tree(doc: &Document, node: &Object, depth: usize) -> u32 {
    if depth > 32 {
        return 0;
    }
    let Some(dict) = resolve_dict(doc, node) else {
        return 0;
    };
    let leaves = dict
        .get(b"Names")
        .ok()
        .and_then(|n| match n {
            Object::Array(a) => Some(a.len() / 2),
            Object::Reference(r) => doc
                .get_object(*r)
                .and_then(Object::as_array)
                .ok()
                .map(|a| a.len() / 2),
            _ => None,
        })
        .unwrap_or(0) as u32;
    let kids = dict
        .get(b"Kids")
        .ok()
        .and_then(|k| match k {
            Object::Array(a) => Some(a.clone()),
            Object::Reference(r) => doc.get_object(*r).and_then(Object::as_array).ok().cloned(),
            _ => None,
        })
        .unwrap_or_default();
    leaves
        + kids
            .iter()
            .map(|k| count_name_tree(doc, k, depth + 1))
            .sum::<u32>()
}

/// The optional-content groups that are off in the default configuration (`/D`).
fn hidden_groups(doc: &Document, root: ObjectId) -> BTreeSet<ObjectId> {
    let refs = |o: Option<&Object>| -> Vec<ObjectId> {
        let array = match o {
            Some(Object::Array(a)) => a.clone(),
            Some(Object::Reference(r)) => doc
                .get_object(*r)
                .and_then(Object::as_array)
                .cloned()
                .unwrap_or_default(),
            _ => Vec::new(),
        };
        array.iter().filter_map(|o| o.as_reference().ok()).collect()
    };
    let Some(props) = doc
        .get_dictionary(root)
        .ok()
        .and_then(|c| c.get(b"OCProperties").ok())
        .and_then(|o| resolve_dict(doc, o))
    else {
        return BTreeSet::new();
    };
    let all = refs(props.get(b"OCGs").ok());
    let Some(config) = props.get(b"D").ok().and_then(|o| resolve_dict(doc, o)) else {
        return BTreeSet::new();
    };
    let off: BTreeSet<ObjectId> = refs(config.get(b"OFF").ok()).into_iter().collect();
    if config.get(b"BaseState").and_then(Object::as_name).ok() == Some(b"OFF") {
        let on: BTreeSet<ObjectId> = refs(config.get(b"ON").ok()).into_iter().collect();
        return all.into_iter().filter(|g| !on.contains(g)).collect();
    }
    off
}

/// Deletes the page content drawn by hidden layers, then `/OCProperties`. Returns the number
/// of hidden layers (0 when there were none, or when a page could not be rewritten).
fn remove_hidden_layers(doc: &mut Document, root: ObjectId) -> u32 {
    let hidden = hidden_groups(doc, root);
    if hidden.is_empty() {
        return 0;
    }
    // An OCG, or an OCMD whose groups are all hidden.
    let is_hidden = |doc: &Document, o: &Object| -> bool {
        let Ok(id) = o.as_reference() else {
            return false;
        };
        if hidden.contains(&id) {
            return true;
        }
        let Ok(d) = doc.get_dictionary(id) else {
            return false;
        };
        let groups: Vec<ObjectId> = match d.get(b"OCGs") {
            Ok(Object::Reference(r)) if doc.get_dictionary(*r).is_ok_and(|g| g.has(b"Name")) => {
                vec![*r]
            }
            Ok(Object::Array(a)) => a.iter().filter_map(|o| o.as_reference().ok()).collect(),
            _ => Vec::new(),
        };
        !groups.is_empty() && groups.iter().all(|g| hidden.contains(g))
    };
    let mut complete = true;
    let pages: Vec<ObjectId> = doc.get_pages().into_values().collect();
    for page in pages {
        // Resource names that point at a hidden group, and XObjects tagged with one.
        let resources = doc
            .get_page_resources(page)
            .ok()
            .and_then(|(inline, ids)| {
                inline.cloned().or_else(|| {
                    ids.first()
                        .and_then(|id| doc.get_dictionary(*id).ok().cloned())
                })
            })
            .unwrap_or_default();
        let hidden_props: HashSet<Vec<u8>> = resources
            .get(b"Properties")
            .ok()
            .and_then(|o| resolve_dict(doc, o))
            .map(|p| {
                p.iter()
                    .filter(|(_, v)| is_hidden(doc, v))
                    .map(|(k, _)| k.clone())
                    .collect()
            })
            .unwrap_or_default();
        let hidden_xobjects: HashSet<Vec<u8>> = resources
            .get(b"XObject")
            .ok()
            .and_then(|o| resolve_dict(doc, o))
            .map(|x| {
                x.iter()
                    .filter(|(_, v)| {
                        resolve_dict(doc, v)
                            .and_then(|d| d.get(b"OC").ok())
                            .is_some_and(|oc| is_hidden(doc, oc))
                    })
                    .map(|(k, _)| k.clone())
                    .collect()
            })
            .unwrap_or_default();
        if hidden_props.is_empty() && hidden_xobjects.is_empty() {
            continue;
        }
        let raw = doc.get_page_content(page);
        if raw.is_empty() {
            complete = false;
            continue;
        }
        let Ok(content) = Content::decode(&raw) else {
            complete = false;
            continue;
        };
        let (kept, dropped) = strip_hidden(content.operations, &hidden_props, &hidden_xobjects);
        if dropped == 0 {
            continue;
        }
        match (Content { operations: kept }).encode() {
            Ok(encoded) => {
                if doc.change_page_content(page, encoded).is_err() {
                    complete = false;
                }
            }
            Err(_) => complete = false,
        }
    }
    if !complete {
        // Leave the layers hidden rather than reveal what could not be deleted.
        return 0;
    }
    if let Ok(catalog) = doc.get_dictionary_mut(root) {
        catalog.remove(b"OCProperties");
    }
    hidden.len() as u32
}

/// The operations outside hidden marked-content sections, minus `Do`s of hidden XObjects,
/// and how many sections / invocations were dropped.
fn strip_hidden(
    ops: Vec<Operation>,
    hidden_props: &HashSet<Vec<u8>>,
    hidden_xobjects: &HashSet<Vec<u8>>,
) -> (Vec<Operation>, u32) {
    let mut kept = Vec::with_capacity(ops.len());
    let mut dropped = 0u32;
    // Depth of marked content inside a hidden section (0 = not skipping).
    let mut skip_depth = 0usize;
    for op in ops {
        let name = op.operator.as_str();
        if skip_depth > 0 {
            match name {
                "BMC" | "BDC" => skip_depth += 1,
                "EMC" => skip_depth -= 1,
                _ => {}
            }
            continue;
        }
        if name == "BDC"
            && op.operands.first().and_then(|o| o.as_name().ok()) == Some(b"OC")
            && op
                .operands
                .get(1)
                .and_then(|o| o.as_name().ok())
                .is_some_and(|n| hidden_props.contains(n))
        {
            skip_depth = 1;
            dropped += 1;
            continue;
        }
        if name == "Do"
            && op
                .operands
                .first()
                .and_then(|o| o.as_name().ok())
                .is_some_and(|n| hidden_xobjects.contains(n))
        {
            dropped += 1;
            continue;
        }
        kept.push(op);
    }
    (kept, dropped)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn op(name: &str, operands: Vec<Object>) -> Operation {
        Operation::new(name, operands)
    }

    #[test]
    fn strips_nested_hidden_sections_only() {
        let hidden: HashSet<Vec<u8>> = [b"L1".to_vec()].into();
        let xobj: HashSet<Vec<u8>> = [b"Im9".to_vec()].into();
        let ops = vec![
            op("q", vec![]),
            op(
                "BDC",
                vec![Object::Name(b"OC".to_vec()), Object::Name(b"L1".to_vec())],
            ),
            op("BMC", vec![Object::Name(b"Span".to_vec())]),
            op("re", vec![]),
            op("EMC", vec![]),
            op("f", vec![]),
            op("EMC", vec![]),
            op(
                "BDC",
                vec![Object::Name(b"OC".to_vec()), Object::Name(b"L2".to_vec())],
            ),
            op("Tj", vec![]),
            op("EMC", vec![]),
            op("Do", vec![Object::Name(b"Im9".to_vec())]),
            op("Q", vec![]),
        ];
        let (kept, dropped) = strip_hidden(ops, &hidden, &xobj);
        let names: Vec<&str> = kept.iter().map(|o| o.operator.as_str()).collect();
        assert_eq!(names, ["q", "BDC", "Tj", "EMC", "Q"]);
        assert_eq!(dropped, 2);
    }
}
