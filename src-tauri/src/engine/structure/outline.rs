//! `set_outline` — the whole bookmark tree replaced in one undo step (P2, `undo.outlineEdit`).
//!
//! The tree is written as ISO 32000 §12.3.3 asks: an outline dictionary
//! `<< /Type /Outlines /First /Last /Count n >>` and one item per node with `/Title /Parent
//! /Prev /Next /First /Last /Count` and either a `/Dest` or a URI `/A`.
//!
//! `/Count` carries the open state. For an item with children it is `+k` when open and `−k`
//! when closed, `k` being the number of descendants that are visible while it is open (a child
//! counts 1, plus its own `k` when it is open too); the outline dictionary's `/Count` is the
//! number of items visible at the top. A leaf has no `/Count`.
//!
//! The old tree's item objects are deleted, not just unlinked, so no orphaned title survives in
//! the file. What a rewrite cannot keep: named destinations (a node read back as page + view is
//! written as an explicit `/Dest`), actions other than GoTo / URI (a Launch or JavaScript node
//! comes back from `get_outline` with no target and is written as a title only), and item
//! colour / style (`/C`, `/F`).

use super::{dest_array, encode_uri, normalize_view, page_id, page_ids, refuse_encrypted, same_view, verify_failed};
use crate::engine::raw;
use crate::engine::registry::{self, MutateOpts};
use crate::engine::types::EngineState;
use crate::ipc::types::{ChangeReason, DocInfo, OutlineNode};
use crate::ipc::EngineError;
use lopdf::{Dictionary, Document, Object, ObjectId};
use std::collections::HashSet;

/// `set_outline` — replaces the document's outline with `nodes` (an empty list removes it).
///
/// Refused with `unsupported` on an encrypted document, `invalidArgument` for a page out of
/// range or a tree deeper than 32 levels. Returns the reloaded `DocInfo` (`hasOutline` follows);
/// `get_outline` afterwards reads back exactly the normalised `nodes` — see [`normalize`].
pub fn set_outline(
    st: &mut EngineState<'_>,
    doc_id: &str,
    nodes: &[OutlineNode],
) -> Result<DocInfo, EngineError> {
    refuse_encrypted(st, doc_id)?;
    let page_count = st.doc(doc_id)?.page_count();
    let normalized = normalize(nodes, page_count, 0)?;
    let expected = normalized.clone();
    let opts = MutateOpts::new("undo.outlineEdit", ChangeReason::Edit)
        .all_pages()
        .keeps_text();
    registry::mutate_bytes_checked(
        st,
        doc_id,
        opts,
        |bytes, _| write_outline(bytes, &normalized),
        move |bindings, reopened| {
            let got = raw::outline::read(bindings, reopened.raw_handle());
            match first_difference(&got, &expected, "") {
                None => Ok(()),
                Some(at) => Err(verify_failed(format!(
                    "PDFium reads the rewritten outline differently at node {at}"
                ))),
            }
        },
    )
}

/// The nodes exactly as they will read back, or `invalidArgument`:
///
/// * a non-blank `url` wins (percent-encoded to 7-bit, `page` / `dest` dropped);
/// * otherwise `page` must be `< page_count`; `dest` loses a zoom ≤ 0 and becomes `None` when
///   nothing is left of it (a whole-page `/Fit`); no page → no `dest`;
/// * `open` is kept only on a node with children, `None` there meaning open.
pub fn normalize(
    nodes: &[OutlineNode],
    page_count: u16,
    depth: usize,
) -> Result<Vec<OutlineNode>, EngineError> {
    if !nodes.is_empty() && depth >= raw::outline::MAX_DEPTH {
        return Err(EngineError::invalid(format!(
            "the outline is deeper than {} levels",
            raw::outline::MAX_DEPTH
        )));
    }
    nodes
        .iter()
        .map(|n| {
            let url = n.url.as_deref().map(encode_uri).filter(|u| !u.is_empty());
            let page = if url.is_some() { None } else { n.page };
            if let Some(p) = page {
                if p >= page_count {
                    return Err(EngineError::invalid(format!(
                        "outline node '{}' points at page {p} of {page_count}",
                        n.title
                    ))
                    .with_page(p));
                }
            }
            let dest = if page.is_some() { normalize_view(n.dest)? } else { None };
            let children = normalize(&n.children, page_count, depth + 1)?;
            let open = if children.is_empty() { None } else { Some(n.open.unwrap_or(true)) };
            Ok(OutlineNode { title: n.title.clone(), page, dest, url, open, children })
        })
        .collect()
}

/// Where two trees first differ (`"2.1"` = the first child of the second top-level node), or
/// `None` when they match.
fn first_difference(got: &[OutlineNode], want: &[OutlineNode], prefix: &str) -> Option<String> {
    if got.len() != want.len() {
        return Some(if prefix.is_empty() { "(top level: count)".into() } else { format!("{prefix} (children)") });
    }
    for (i, (g, w)) in got.iter().zip(want).enumerate() {
        let here = if prefix.is_empty() { format!("{}", i + 1) } else { format!("{prefix}.{}", i + 1) };
        let same = g.title == w.title
            && g.page == w.page
            && g.url == w.url
            && g.open == w.open
            && same_view(g.dest, w.dest);
        if !same {
            return Some(here);
        }
        if let Some(at) = first_difference(&g.children, &w.children, &here) {
            return Some(at);
        }
    }
    None
}

/// The lopdf rewrite: drop the old tree, write the new one (or none).
fn write_outline(bytes: &[u8], nodes: &[OutlineNode]) -> Result<Vec<u8>, EngineError> {
    let mut doc = super::load(bytes)?;
    let pages = page_ids(&doc);

    let old = doc
        .catalog_mut()
        .map_err(|e| crate::engine::save::lopdf_error("catalog", e))?
        .remove(b"Outlines");
    match old {
        Some(Object::Reference(root)) => {
            let first = doc.get_dictionary(root).ok().and_then(|d| d.get(b"First").ok()).cloned();
            delete_items(&mut doc, first);
            doc.objects.remove(&root);
        }
        Some(Object::Dictionary(root)) => delete_items(&mut doc, root.get(b"First").ok().cloned()),
        _ => {}
    }

    if !nodes.is_empty() {
        let root = doc.new_object_id();
        let level = write_level(&mut doc, &pages, nodes, root)?;
        let mut dict = Dictionary::new();
        dict.set("Type", Object::Name(b"Outlines".to_vec()));
        dict.set("First", Object::Reference(level.first));
        dict.set("Last", Object::Reference(level.last));
        dict.set("Count", Object::Integer(level.visible));
        doc.objects.insert(root, Object::Dictionary(dict));
        doc.catalog_mut()
            .map_err(|e| crate::engine::save::lopdf_error("catalog", e))?
            .set("Outlines", Object::Reference(root));
    }
    super::write(doc)
}

/// Deletes every item reachable from `first` through `/First` and `/Next` (a visited set stops
/// a cyclic tree).
fn delete_items(doc: &mut Document, first: Option<Object>) {
    let mut stack: Vec<ObjectId> = first.and_then(|o| o.as_reference().ok()).into_iter().collect();
    let mut seen: HashSet<ObjectId> = HashSet::new();
    while let Some(id) = stack.pop() {
        if !seen.insert(id) {
            continue;
        }
        if let Some(Object::Dictionary(item)) = doc.objects.remove(&id) {
            for key in [&b"First"[..], b"Next"] {
                if let Ok(next) = item.get(key).and_then(Object::as_reference) {
                    stack.push(next);
                }
            }
        }
    }
}

struct Level {
    first: ObjectId,
    last: ObjectId,
    /// Items of this level visible when its parent is open: each node, plus its visible
    /// descendants when it is open itself.
    visible: i64,
}

fn write_level(
    doc: &mut Document,
    pages: &[ObjectId],
    nodes: &[OutlineNode],
    parent: ObjectId,
) -> Result<Level, EngineError> {
    let ids: Vec<ObjectId> = nodes.iter().map(|_| doc.new_object_id()).collect();
    let mut visible = 0i64;
    for (i, node) in nodes.iter().enumerate() {
        let mut item = Dictionary::new();
        item.set("Title", crate::engine::save::pdf_text_string(&node.title));
        item.set("Parent", Object::Reference(parent));
        if i > 0 {
            item.set("Prev", Object::Reference(ids[i - 1]));
        }
        if i + 1 < ids.len() {
            item.set("Next", Object::Reference(ids[i + 1]));
        }
        visible += 1;
        if !node.children.is_empty() {
            let children = write_level(doc, pages, &node.children, ids[i])?;
            item.set("First", Object::Reference(children.first));
            item.set("Last", Object::Reference(children.last));
            let open = node.open.unwrap_or(true);
            item.set("Count", Object::Integer(if open { children.visible } else { -children.visible }));
            if open {
                visible += children.visible;
            }
        }
        if let Some(url) = &node.url {
            item.set("A", super::uri_action(url));
        } else if let Some(page) = node.page {
            item.set("Dest", dest_array(page_id(pages, page)?, node.dest));
        }
        doc.objects.insert(ids[i], Object::Dictionary(item));
    }
    Ok(Level { first: ids[0], last: ids[ids.len() - 1], visible })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::types::OutlineDest;

    fn leaf(title: &str, page: u16) -> OutlineNode {
        OutlineNode { title: title.into(), page: Some(page), dest: None, url: None, open: None, children: vec![] }
    }

    #[test]
    fn normalize_fills_open_and_drops_what_cannot_be_read_back() {
        let nodes = vec![OutlineNode {
            title: "A".into(),
            page: Some(0),
            dest: Some(OutlineDest { x: None, y: None, zoom: Some(0.0) }),
            url: None,
            open: Some(false),
            children: vec![
                leaf("A.1", 1),
                OutlineNode { url: Some("https://예.kr".into()), page: Some(1), ..leaf("web", 0) },
            ],
        }];
        let out = normalize(&nodes, 3, 0).unwrap();
        assert_eq!(out[0].dest, None);
        assert_eq!(out[0].open, Some(false));
        assert_eq!(out[0].children[0].open, None);
        assert_eq!(out[0].children[1].page, None);
        assert_eq!(out[0].children[1].url.as_deref(), Some("https://%EC%98%88.kr"));
        let err = normalize(&[leaf("x", 3)], 3, 0).unwrap_err();
        assert_eq!(err.code, crate::ipc::ErrorCode::InvalidArgument);
    }

    #[test]
    fn counts_follow_the_open_state() {
        // A (open) { A.1, A.2 (closed) { A.2.a } }, B
        let tree = vec![
            OutlineNode {
                open: Some(true),
                children: vec![
                    leaf("A.1", 0),
                    OutlineNode { open: Some(false), children: vec![leaf("A.2.a", 0)], ..leaf("A.2", 0) },
                ],
                ..leaf("A", 0)
            },
            leaf("B", 0),
        ];
        let mut doc = Document::with_version("1.7");
        let pages = vec![(1, 0)];
        let root = doc.new_object_id();
        let level = write_level(&mut doc, &pages, &tree, root).unwrap();
        // visible at the top: A, A.1, A.2, B (A.2.a is hidden by the closed A.2)
        assert_eq!(level.visible, 4);
        let a = doc.get_dictionary(level.first).unwrap();
        assert_eq!(a.get(b"Count").unwrap().as_i64().unwrap(), 2);
        let a2 = a.get(b"Last").unwrap().as_reference().unwrap();
        assert_eq!(doc.get_dictionary(a2).unwrap().get(b"Count").unwrap().as_i64().unwrap(), -1);
        let b = doc.get_dictionary(level.last).unwrap();
        assert!(!b.has(b"Count"), "a leaf has no /Count");
        assert_eq!(b.get(b"Prev").unwrap().as_reference().unwrap(), level.first);
    }
}
