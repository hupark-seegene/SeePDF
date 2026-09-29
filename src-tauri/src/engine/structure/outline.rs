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
//! the file.
//!
//! v0.3 P5 — **untouched nodes keep what the contract cannot carry**: before the rewrite the old
//! items are walked in document order next to PDFium's own reading of them; a new node whose
//! title, page, view and url equal an old item's gets that item's original target object back
//! (a **named** `/Dest`, a GoTo `/A` with a named `/D`, or any other action — Launch,
//! JavaScript, GoToR) and its colour and style (`/C`, `/F`). A node whose title or target the
//! user changed is written fresh (explicit `/Dest`, no colour), as before.
//!
//! [`append_in`] adds nodes after an existing outline without touching it — the merge / insert
//! carry-over (`pages::carry`, v0.3 P1).

use super::{
    dest_array, encode_uri, normalize_view, page_id, page_ids, refuse_encrypted, same_view,
    verify_failed,
};
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
        |bytes, doc| write_outline(bytes, &normalized, &doc.outline()),
        move |bindings, reopened| {
            let got = raw::outline::read(bindings, reopened);
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
            let dest = if page.is_some() {
                normalize_view(n.dest)?
            } else {
                None
            };
            let children = normalize(&n.children, page_count, depth + 1)?;
            let open = if children.is_empty() {
                None
            } else {
                Some(n.open.unwrap_or(true))
            };
            Ok(OutlineNode {
                title: n.title.clone(),
                page,
                dest,
                url,
                open,
                children,
            })
        })
        .collect()
}

/// Where two trees first differ (`"2.1"` = the first child of the second top-level node), or
/// `None` when they match.
fn first_difference(got: &[OutlineNode], want: &[OutlineNode], prefix: &str) -> Option<String> {
    if got.len() != want.len() {
        return Some(if prefix.is_empty() {
            "(top level: count)".into()
        } else {
            format!("{prefix} (children)")
        });
    }
    for (i, (g, w)) in got.iter().zip(want).enumerate() {
        let here = if prefix.is_empty() {
            format!("{}", i + 1)
        } else {
            format!("{prefix}.{}", i + 1)
        };
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

/// The lopdf rewrite: drop the old tree, write the new one (or none). `old` is PDFium's reading
/// of the tree being replaced (`OpenDoc::outline`), used to recognise untouched nodes (P5).
fn write_outline(
    bytes: &[u8],
    nodes: &[OutlineNode],
    old: &[OutlineNode],
) -> Result<Vec<u8>, EngineError> {
    let mut doc = super::load(bytes)?;
    let pages = page_ids(&doc);

    let old_root = doc
        .catalog_mut()
        .map_err(|e| crate::engine::save::lopdf_error("catalog", e))?
        .remove(b"Outlines");
    let first = match &old_root {
        Some(Object::Reference(root)) => doc
            .get_dictionary(*root)
            .ok()
            .and_then(|d| d.get(b"First").ok())
            .cloned(),
        Some(Object::Dictionary(root)) => root.get(b"First").ok().cloned(),
        _ => None,
    };
    let mut pool = keepers(&doc, first.clone(), old);
    delete_items(&mut doc, first);
    if let Some(Object::Reference(root)) = old_root {
        doc.objects.remove(&root);
    }

    if !nodes.is_empty() {
        let root = doc.new_object_id();
        let level = write_level(&mut doc, &pages, nodes, root, &mut pool)?;
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

/// v0.3 P1: appends `nodes` (already [`normalize`]d against `doc`'s page count) after the
/// document's existing top-level outline items, which are left exactly as they are; creates the
/// outline when there is none. Nothing to do for an empty list.
pub(crate) fn append_in(doc: &mut Document, nodes: &[OutlineNode]) -> Result<(), EngineError> {
    if nodes.is_empty() {
        return Ok(());
    }
    let pages = page_ids(doc);
    let catalog_err = |e| crate::engine::save::lopdf_error("catalog", e);
    // The outline dictionary as an indirect object: items name it as their /Parent.
    let root = match doc
        .catalog()
        .map_err(catalog_err)?
        .get(b"Outlines")
        .ok()
        .cloned()
    {
        Some(Object::Reference(id)) if doc.get_dictionary(id).is_ok() => id,
        Some(Object::Dictionary(inline)) => {
            let id = doc.add_object(Object::Dictionary(inline));
            doc.catalog_mut()
                .map_err(catalog_err)?
                .set("Outlines", Object::Reference(id));
            id
        }
        _ => {
            let mut dict = Dictionary::new();
            dict.set("Type", Object::Name(b"Outlines".to_vec()));
            let id = doc.add_object(Object::Dictionary(dict));
            doc.catalog_mut()
                .map_err(catalog_err)?
                .set("Outlines", Object::Reference(id));
            id
        }
    };
    let mut pool = Vec::new();
    let level = write_level(doc, &pages, nodes, root, &mut pool)?;
    let old_last = doc
        .get_dictionary(root)
        .ok()
        .and_then(|d| d.get(b"Last").ok())
        .and_then(|o| o.as_reference().ok())
        .filter(|id| doc.get_dictionary(*id).is_ok());
    if let Some(last) = old_last {
        if let Ok(item) = doc.get_dictionary_mut(last) {
            item.set("Next", Object::Reference(level.first));
        }
        if let Ok(item) = doc.get_dictionary_mut(level.first) {
            item.set("Prev", Object::Reference(last));
        }
    }
    let dict = doc
        .get_dictionary_mut(root)
        .map_err(|e| crate::engine::save::lopdf_error("outline root", e))?;
    if old_last.is_none() {
        dict.set("First", Object::Reference(level.first));
    }
    dict.set("Last", Object::Reference(level.last));
    let count = dict
        .get(b"Count")
        .ok()
        .and_then(|c| c.as_i64().ok())
        .unwrap_or(0)
        .max(0);
    dict.set("Count", Object::Integer(count + level.visible));
    Ok(())
}

/// Deletes every item reachable from `first` through `/First` and `/Next` (a visited set stops
/// a cyclic tree).
fn delete_items(doc: &mut Document, first: Option<Object>) {
    let mut stack: Vec<ObjectId> = first
        .and_then(|o| o.as_reference().ok())
        .into_iter()
        .collect();
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

/// What an untouched node gets back from the item it was read from (P5).
#[derive(Debug, Clone)]
struct Keep {
    title: String,
    page: Option<u16>,
    url: Option<String>,
    dest: Option<crate::ipc::types::OutlineDest>,
    /// The original target: `(b"Dest", …)` or `(b"A", …)`, as written in the file.
    target: Option<(&'static [u8], Object)>,
    color: Option<Object>,
    style: Option<Object>,
}

impl Keep {
    fn matches(&self, node: &OutlineNode) -> bool {
        self.title == node.title
            && self.page == node.page
            && self.url == node.url
            && same_view(self.dest, node.dest)
    }
}

/// The old items in document order (`/First` before `/Next`, like PDFium's walk), paired with
/// PDFium's reading of them. `[]` when the two walks disagree on the shape or on a title — then
/// nothing is kept, which is the pre-v0.3 behaviour, never a wrong pairing.
fn keepers(doc: &Document, first: Option<Object>, old: &[OutlineNode]) -> Vec<Option<Keep>> {
    fn flatten<'a>(nodes: &'a [OutlineNode], out: &mut Vec<&'a OutlineNode>) {
        for n in nodes {
            out.push(n);
            flatten(&n.children, out);
        }
    }
    let mut read: Vec<&OutlineNode> = Vec::new();
    flatten(old, &mut read);

    let mut items: Vec<ObjectId> = Vec::new();
    let mut seen: HashSet<ObjectId> = HashSet::new();
    fn walk(
        doc: &Document,
        first: Option<ObjectId>,
        depth: usize,
        seen: &mut HashSet<ObjectId>,
        out: &mut Vec<ObjectId>,
    ) {
        let mut current = first;
        while let Some(id) = current {
            if depth > raw::outline::MAX_DEPTH || out.len() >= raw::outline::MAX_NODES {
                return;
            }
            if !seen.insert(id) {
                return;
            }
            let Ok(item) = doc.get_dictionary(id) else {
                return;
            };
            out.push(id);
            let child = item.get(b"First").and_then(Object::as_reference).ok();
            walk(doc, child, depth + 1, seen, out);
            current = item.get(b"Next").and_then(Object::as_reference).ok();
        }
    }
    walk(
        doc,
        first.and_then(|o| o.as_reference().ok()),
        0,
        &mut seen,
        &mut items,
    );
    if items.len() != read.len() {
        return Vec::new();
    }
    let mut out = Vec::with_capacity(items.len());
    for (id, node) in items.iter().zip(read) {
        let Ok(item) = doc.get_dictionary(*id) else {
            return Vec::new();
        };
        let title = item
            .get(b"Title")
            .ok()
            .and_then(|t| doc.dereference(t).ok())
            .and_then(|(_, t)| lopdf::decode_text_string(t).ok())
            .unwrap_or_default();
        if title != node.title {
            return Vec::new();
        }
        let target = match (item.get(b"Dest"), item.get(b"A")) {
            (Ok(dest), _) => Some((&b"Dest"[..], dest.clone())),
            (_, Ok(action)) => Some((&b"A"[..], action.clone())),
            _ => None,
        };
        out.push(Some(Keep {
            title: node.title.clone(),
            page: node.page,
            url: node.url.clone(),
            dest: node.dest,
            target,
            color: item.get(b"C").ok().cloned(),
            style: item.get(b"F").ok().cloned(),
        }));
    }
    out
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
    pool: &mut Vec<Option<Keep>>,
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
        // Taken before the children, so a parent and a child with the same title and target
        // pair in document order.
        let keep = pool
            .iter_mut()
            .find(|k| k.as_ref().is_some_and(|k| k.matches(node)))
            .and_then(Option::take);
        if !node.children.is_empty() {
            let children = write_level(doc, pages, &node.children, ids[i], pool)?;
            item.set("First", Object::Reference(children.first));
            item.set("Last", Object::Reference(children.last));
            let open = node.open.unwrap_or(true);
            item.set(
                "Count",
                Object::Integer(if open {
                    children.visible
                } else {
                    -children.visible
                }),
            );
            if open {
                visible += children.visible;
            }
        }
        match keep {
            Some(Keep {
                target: Some((key, target)),
                color,
                style,
                ..
            }) => {
                item.set(key, target);
                keep_style(&mut item, color, style);
            }
            other => {
                if let Some(url) = &node.url {
                    item.set("A", super::uri_action(url));
                } else if let Some(page) = node.page {
                    item.set("Dest", dest_array(page_id(pages, page)?, node.dest));
                }
                if let Some(k) = other {
                    keep_style(&mut item, k.color, k.style);
                }
            }
        }
        doc.objects.insert(ids[i], Object::Dictionary(item));
    }
    Ok(Level {
        first: ids[0],
        last: ids[ids.len() - 1],
        visible,
    })
}

fn keep_style(item: &mut Dictionary, color: Option<Object>, style: Option<Object>) {
    if let Some(c) = color {
        item.set("C", c);
    }
    if let Some(f) = style {
        item.set("F", f);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::types::OutlineDest;

    fn leaf(title: &str, page: u16) -> OutlineNode {
        OutlineNode {
            title: title.into(),
            page: Some(page),
            dest: None,
            url: None,
            open: None,
            children: vec![],
        }
    }

    #[test]
    fn normalize_fills_open_and_drops_what_cannot_be_read_back() {
        let nodes = vec![OutlineNode {
            title: "A".into(),
            page: Some(0),
            dest: Some(OutlineDest {
                x: None,
                y: None,
                zoom: Some(0.0),
            }),
            url: None,
            open: Some(false),
            children: vec![
                leaf("A.1", 1),
                OutlineNode {
                    url: Some("https://예.kr".into()),
                    page: Some(1),
                    ..leaf("web", 0)
                },
            ],
        }];
        let out = normalize(&nodes, 3, 0).unwrap();
        assert_eq!(out[0].dest, None);
        assert_eq!(out[0].open, Some(false));
        assert_eq!(out[0].children[0].open, None);
        assert_eq!(out[0].children[1].page, None);
        assert_eq!(
            out[0].children[1].url.as_deref(),
            Some("https://%EC%98%88.kr")
        );
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
                    OutlineNode {
                        open: Some(false),
                        children: vec![leaf("A.2.a", 0)],
                        ..leaf("A.2", 0)
                    },
                ],
                ..leaf("A", 0)
            },
            leaf("B", 0),
        ];
        let mut doc = Document::with_version("1.7");
        let pages = vec![(1, 0)];
        let root = doc.new_object_id();
        let level = write_level(&mut doc, &pages, &tree, root, &mut Vec::new()).unwrap();
        // visible at the top: A, A.1, A.2, B (A.2.a is hidden by the closed A.2)
        assert_eq!(level.visible, 4);
        let a = doc.get_dictionary(level.first).unwrap();
        assert_eq!(a.get(b"Count").unwrap().as_i64().unwrap(), 2);
        let a2 = a.get(b"Last").unwrap().as_reference().unwrap();
        assert_eq!(
            doc.get_dictionary(a2)
                .unwrap()
                .get(b"Count")
                .unwrap()
                .as_i64()
                .unwrap(),
            -1
        );
        let b = doc.get_dictionary(level.last).unwrap();
        assert!(!b.has(b"Count"), "a leaf has no /Count");
        assert_eq!(b.get(b"Prev").unwrap().as_reference().unwrap(), level.first);
    }
}
