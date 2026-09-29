//! What a page import leaves behind, put back (v0.3 P1 / P3).
//!
//! `FPDF_ImportPages*` copies page dictionaries and everything they reference, but
//!
//! * not the catalog: the source's **outline**, **page labels** and `/AcroForm` stay behind;
//! * and not the keys `Parent`, `Prev` and `First` of the objects it copies — it leaves them
//!   pointing at the *source's* object numbers, which in the destination are some unrelated
//!   object (a font stream, a content stream). A form widget's `/Parent` (its field) and a
//!   popup's `/Parent` (its markup annotation) come out dangling. `tests/pages.rs` shows it on
//!   `160F-2019.pdf`: 76 widgets whose `/Parent` names XObjects and fonts.
//!
//! [`apply`] runs on the bytes PDFium wrote after the import (lopdf, like `engine::structure`)
//! with a [`Source`] per imported block, read *before* the import from the source document:
//!
//! * **fields** — the imported widgets are paired with the source page's `/Annots` (same order;
//!   `/Rect` + `/Subtype` when the counts differ); each widget's field ancestors are copied from
//!   the source (their values, flags, `/DA`, `/Opt`, `/AA` — everything but `/Kids`, which is
//!   rebuilt from the imported widgets only), the widget is re-parented, the new root fields are
//!   appended to the destination's `/AcroForm /Fields` — a root whose `/T` is already taken is
//!   renamed `name_2`, `name_3`, … so the two never share a value — and the source's `/DR` fonts
//!   and `/DA` join the destination's. Popups get their `/Parent` back the same way.
//! * **outline** — each source's tree (read with PDFium) under a new top-level node named after
//!   the file, pages mapped to their new indices (a node whose page was not imported keeps its
//!   title only if a child survives), appended after the destination's own items.
//! * **page labels** — every page's label is recomputed from its own document's ranges and the
//!   result compressed back into ranges, so the destination's pages after the insertion keep
//!   their numbers and the imported pages keep theirs.
//!
//! Each part is independent and best-effort in the caller's sense: [`apply`] returns an error
//! only for bytes lopdf cannot parse or write; the caller then keeps PDFium's plain import and
//! reports what was lost, exactly as before v0.3.

use crate::engine::raw;
use crate::engine::structure::{self, labels, outline};
use crate::ipc::types::{OutlineNode, PageLabelRange, PageLabelStyle, Rect};
use crate::ipc::EngineError;
use lopdf::{Dictionary, Document, Object, ObjectId};
use pdfium_render::prelude::{PdfDocument, PdfiumLibraryBindings};
use std::collections::{HashMap, HashSet};

/// What to carry from a source.
#[derive(Debug, Clone, Copy)]
pub struct Carry {
    pub outline: bool,
    pub labels: bool,
    pub forms: bool,
}

impl Carry {
    pub const ALL: Carry = Carry {
        outline: true,
        labels: true,
        forms: true,
    };
    /// Pages dragged from another open document: only what makes the pages themselves whole.
    pub const PAGES_ONLY: Carry = Carry {
        outline: false,
        labels: false,
        forms: true,
    };
}

/// One source document, read before the import.
pub struct Source {
    /// The top-level outline node's title (the file name without extension).
    pub name: String,
    /// The source's outline as PDFium reads it (empty when not carried).
    pub outline: Vec<OutlineNode>,
    /// The source's `/PageLabels` ranges (empty when it has none or they are not carried).
    pub labels: Vec<PageLabelRange>,
    /// An unencrypted lopdf parse of the source — `None` when it could not be made.
    pub lopdf: Option<Document>,
    /// The source has AcroForm fields to carry.
    pub has_form: bool,
    /// The imported source pages, 0-based, in import order.
    pub selected: Vec<u16>,
    pub carry: Carry,
}

impl Source {
    /// Anything for [`apply`] to do?
    pub fn carries_anything(&self) -> bool {
        (self.carry.outline && !self.outline.is_empty())
            || (self.carry.labels && !self.labels.is_empty())
            || (self.carry.forms && self.has_form)
    }
}

/// Reads what [`apply`] needs from the source, on the engine thread.
pub fn read_source(
    bindings: &'static dyn PdfiumLibraryBindings,
    source: &PdfDocument<'_>,
    name: &str,
    selected: Vec<u16>,
    carry: Carry,
) -> Source {
    let outline = if carry.outline {
        raw::outline::read(bindings, source)
    } else {
        Vec::new()
    };
    // The lopdf copy costs a serialisation and a parse of the whole source, so it is only made
    // when there is something it is needed for: fields, or page labels (PDFium answers a label
    // for page 0 only when the file has a `/PageLabels` tree).
    let count = raw::page::page_count(bindings, source);
    let has_labels = carry.labels
        && (raw::doc::page_label(bindings, source, 0).is_some()
            || (count > 1 && raw::doc::page_label(bindings, source, count - 1).is_some()));
    let wants_lopdf = (carry.forms && source.form().is_some()) || has_labels;
    // `RemoveSecurity`: an encrypted source (opened with its password) is written in the clear
    // so lopdf can read it; nothing of it reaches the destination but what is copied below.
    let lopdf = wants_lopdf
        .then(|| {
            raw::save::save_as_copy(bindings, source, raw::save::SaveFlags::RemoveSecurity).ok()
        })
        .flatten()
        .and_then(|bytes| Document::load_mem(&bytes).ok())
        .filter(|d| !d.is_encrypted());
    let (labels, has_form) = match &lopdf {
        Some(doc) => (
            if carry.labels {
                labels::read_ranges(doc)
            } else {
                Vec::new()
            },
            has_fields(doc),
        ),
        None => (Vec::new(), false),
    };
    Source {
        name: name.to_string(),
        outline,
        labels,
        lopdf,
        has_form,
        selected,
        carry,
    }
}

fn has_fields(doc: &Document) -> bool {
    acroform(doc)
        .and_then(|af| af.get(b"Fields").ok())
        .and_then(|f| doc.dereference(f).ok())
        .and_then(|(_, f)| f.as_array().ok())
        .is_some_and(|a| !a.is_empty())
}

fn acroform(doc: &Document) -> Option<&Dictionary> {
    let catalog = doc.catalog().ok()?;
    let (_, af) = doc.dereference(catalog.get(b"AcroForm").ok()?).ok()?;
    af.as_dict().ok()
}

/// A source's pages and where the first of them landed in the destination.
pub struct Placed {
    pub source: Source,
    /// Destination index of `source.selected[0]`; the block is contiguous.
    pub at: u16,
}

/// How the outline nodes are added.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutlineMode {
    /// `merge_documents`: one top-level node per source (a leaf for a source without an
    /// outline), but only when at least one source has an outline.
    PerSource,
    /// Insert into an existing document: a node only for a source that has an outline.
    OnlyOutlined,
}

/// The rewrite. `dest` is what PDFium wrote after importing every block of `placed`.
pub fn apply(dest: &[u8], placed: &[Placed], mode: OutlineMode) -> Result<Vec<u8>, EngineError> {
    let mut doc = structure::load(dest)?;
    let total = structure::page_ids(&doc).len();
    for p in placed {
        if p.at as usize + p.source.selected.len() > total {
            return Err(EngineError::invalid(
                "an imported block lies past the end of the document",
            ));
        }
    }
    repair_annotations(&mut doc, placed)?;
    carry_outline(&mut doc, placed, mode, total as u16)?;
    carry_labels(&mut doc, placed, total as u16)?;
    structure::write(doc)
}

// ---------------------------------------------------------------------------------------
// Fields and popups
// ---------------------------------------------------------------------------------------

fn annot_ids(doc: &Document, page: ObjectId) -> Vec<ObjectId> {
    let Ok(dict) = doc.get_dictionary(page) else {
        return Vec::new();
    };
    let Ok(annots) = dict.get(b"Annots") else {
        return Vec::new();
    };
    let Ok((_, Object::Array(items))) = doc.dereference(annots) else {
        return Vec::new();
    };
    items.iter().filter_map(|o| o.as_reference().ok()).collect()
}

fn name_of(dict: &Dictionary, key: &[u8]) -> Option<Vec<u8>> {
    dict.get(key).ok()?.as_name().ok().map(<[u8]>::to_vec)
}

fn rect_of(doc: &Document, dict: &Dictionary) -> Option<Rect> {
    let (_, r) = doc.dereference(dict.get(b"Rect").ok()?).ok()?;
    let a = r.as_array().ok()?;
    let v: Vec<f32> = a
        .iter()
        .filter_map(|o| {
            o.as_float()
                .ok()
                .or_else(|| o.as_i64().ok().map(|i| i as f32))
        })
        .collect();
    (v.len() == 4).then(|| Rect::new(v[0], v[1], v[2], v[3]))
}

/// Pairs the imported page's annotations with the source page's.
fn pair(
    src: &Document,
    s: &[ObjectId],
    dst: &Document,
    d: &[ObjectId],
) -> Vec<(ObjectId, ObjectId)> {
    if s.len() == d.len() {
        return s.iter().copied().zip(d.iter().copied()).collect();
    }
    let key = |doc: &Document, id: ObjectId| {
        let dict = doc.get_dictionary(id).ok()?;
        let rect = rect_of(doc, dict)?;
        Some((name_of(dict, b"Subtype"), rect))
    };
    let mut used: HashSet<ObjectId> = HashSet::new();
    let mut out = Vec::new();
    for &sid in s {
        let Some((kind, rect)) = key(src, sid) else {
            continue;
        };
        let found = d.iter().copied().find(|did| {
            !used.contains(did)
                && key(dst, *did).is_some_and(|(k, r)| {
                    k == kind
                        && (r.l - rect.l).abs() < 0.01
                        && (r.b - rect.b).abs() < 0.01
                        && (r.r - rect.r).abs() < 0.01
                        && (r.t - rect.t).abs() < 0.01
                })
        });
        if let Some(did) = found {
            used.insert(did);
            out.push((sid, did));
        }
    }
    out
}

/// Deep-copies `value` from `src` into `dst`. References are followed (memoised in `map`);
/// `/P` and `/Parent` inside copied dictionaries are dropped (a page or a tree node of the
/// source must never be dragged in), and so is a reference to a source **page**.
fn copy_value(
    src: &Document,
    dst: &mut Document,
    value: &Object,
    map: &mut HashMap<ObjectId, ObjectId>,
    depth: usize,
) -> Object {
    if depth > 64 {
        return Object::Null;
    }
    match value {
        Object::Reference(id) => {
            if let Some(&mapped) = map.get(id) {
                return Object::Reference(mapped);
            }
            let Ok(target) = src.get_object(*id) else {
                return Object::Null;
            };
            if let Ok(dict) = target.as_dict() {
                if name_of(dict, b"Type").as_deref() == Some(b"Page") {
                    return Object::Null;
                }
            }
            let new_id = dst.new_object_id();
            map.insert(*id, new_id);
            let copied = copy_value(src, dst, target, map, depth + 1);
            dst.objects.insert(new_id, copied);
            Object::Reference(new_id)
        }
        Object::Array(items) => Object::Array(
            items
                .iter()
                .map(|v| copy_value(src, dst, v, map, depth + 1))
                .collect(),
        ),
        Object::Dictionary(dict) => Object::Dictionary(copy_dict(src, dst, dict, map, depth)),
        Object::Stream(stream) => {
            let mut out = stream.clone();
            out.dict = copy_dict(src, dst, &stream.dict, map, depth);
            Object::Stream(out)
        }
        other => other.clone(),
    }
}

fn copy_dict(
    src: &Document,
    dst: &mut Document,
    dict: &Dictionary,
    map: &mut HashMap<ObjectId, ObjectId>,
    depth: usize,
) -> Dictionary {
    let mut out = Dictionary::new();
    for (key, value) in dict.iter() {
        if key == b"P" || key == b"Parent" {
            continue;
        }
        out.set(key.clone(), copy_value(src, dst, value, map, depth + 1));
    }
    out
}

/// The destination copy of the source field `id` (and, recursively, of its ancestors).
fn ensure_field(
    src: &Document,
    dst: &mut Document,
    id: ObjectId,
    fields: &mut HashMap<ObjectId, ObjectId>,
    values: &mut HashMap<ObjectId, ObjectId>,
    roots: &mut Vec<ObjectId>,
    depth: usize,
) -> Option<ObjectId> {
    if let Some(&done) = fields.get(&id) {
        return Some(done);
    }
    if depth > 32 {
        return None;
    }
    let source = src.get_dictionary(id).ok()?.clone();
    let new_id = dst.new_object_id();
    fields.insert(id, new_id);
    let mut out = Dictionary::new();
    for (key, value) in source.iter() {
        if key == b"Kids" || key == b"Parent" || key == b"P" {
            continue;
        }
        out.set(key.clone(), copy_value(src, dst, value, values, 0));
    }
    out.set("Kids", Object::Array(Vec::new()));
    let parent = source
        .get(b"Parent")
        .and_then(Object::as_reference)
        .ok()
        .and_then(|p| ensure_field(src, dst, p, fields, values, roots, depth + 1));
    match parent {
        Some(p) => {
            out.set("Parent", Object::Reference(p));
            add_kid(dst, p, new_id);
        }
        None => roots.push(new_id),
    }
    dst.objects.insert(new_id, Object::Dictionary(out));
    Some(new_id)
}

fn add_kid(dst: &mut Document, parent: ObjectId, kid: ObjectId) {
    if let Ok(dict) = dst.get_dictionary_mut(parent) {
        match dict.get_mut(b"Kids") {
            Ok(Object::Array(kids)) => kids.push(Object::Reference(kid)),
            _ => dict.set("Kids", Object::Array(vec![Object::Reference(kid)])),
        }
    }
}

fn repair_annotations(doc: &mut Document, placed: &[Placed]) -> Result<(), EngineError> {
    let dst_pages = structure::page_ids(doc);
    let mut new_roots: Vec<ObjectId> = Vec::new();
    let mut dr_sources: Vec<(usize, Dictionary)> = Vec::new();
    for (k, p) in placed.iter().enumerate() {
        let Some(src) = p.source.lopdf.as_ref() else {
            continue;
        };
        let forms = p.source.carry.forms && p.source.has_form;
        let src_pages = structure::page_ids(src);
        let mut fields: HashMap<ObjectId, ObjectId> = HashMap::new();
        let mut values: HashMap<ObjectId, ObjectId> = HashMap::new();
        for (j, &sp) in p.source.selected.iter().enumerate() {
            let (Some(&s_page), Some(&d_page)) =
                (src_pages.get(sp as usize), dst_pages.get(p.at as usize + j))
            else {
                continue;
            };
            let s_annots = annot_ids(src, s_page);
            let d_annots = annot_ids(doc, d_page);
            let pairs = pair(src, &s_annots, doc, &d_annots);
            let by_source: HashMap<ObjectId, ObjectId> = pairs.iter().copied().collect();
            for &(sid, did) in &pairs {
                let Ok(s_dict) = src.get_dictionary(sid) else {
                    continue;
                };
                let subtype = name_of(s_dict, b"Subtype");
                let s_parent = s_dict.get(b"Parent").and_then(Object::as_reference).ok();
                match subtype.as_deref() {
                    Some(b"Widget") if forms => {
                        let parent = s_parent.and_then(|sp| {
                            ensure_field(src, doc, sp, &mut fields, &mut values, &mut new_roots, 0)
                        });
                        let is_field = s_dict.has(b"T") || s_dict.has(b"FT");
                        if let Ok(d) = doc.get_dictionary_mut(did) {
                            match parent {
                                Some(pid) => d.set("Parent", Object::Reference(pid)),
                                None => {
                                    d.remove(b"Parent");
                                }
                            }
                        }
                        match parent {
                            Some(pid) => add_kid(doc, pid, did),
                            None if is_field => new_roots.push(did),
                            None => {}
                        }
                    }
                    Some(b"Widget") => {
                        // Not carrying fields: at least do not leave a dangling /Parent.
                        if let Ok(d) = doc.get_dictionary_mut(did) {
                            d.remove(b"Parent");
                        }
                    }
                    Some(b"Popup") => {
                        let fixed = s_parent.and_then(|sp| by_source.get(&sp).copied());
                        if let Ok(d) = doc.get_dictionary_mut(did) {
                            match fixed {
                                Some(pid) => d.set("Parent", Object::Reference(pid)),
                                None => {
                                    d.remove(b"Parent");
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        if forms {
            if let Some(af) = acroform(src) {
                dr_sources.push((k, af.clone()));
            }
        }
    }
    if new_roots.is_empty() {
        return Ok(());
    }
    add_to_acroform(doc, placed, &new_roots, &dr_sources)
}

/// The destination's `/AcroForm`, as an indirect object (created, or moved out of the catalog).
fn acroform_id(doc: &mut Document) -> Result<ObjectId, EngineError> {
    let catalog_err = |e| crate::engine::save::lopdf_error("catalog", e);
    let current = doc
        .catalog()
        .map_err(catalog_err)?
        .get(b"AcroForm")
        .ok()
        .cloned();
    let id = match current {
        Some(Object::Reference(id)) if doc.get_dictionary(id).is_ok() => return Ok(id),
        Some(Object::Dictionary(inline)) => doc.add_object(Object::Dictionary(inline)),
        _ => {
            let mut dict = Dictionary::new();
            dict.set("Fields", Object::Array(Vec::new()));
            doc.add_object(Object::Dictionary(dict))
        }
    };
    doc.catalog_mut()
        .map_err(catalog_err)?
        .set("AcroForm", Object::Reference(id));
    Ok(id)
}

fn text_of(doc: &Document, dict: &Dictionary, key: &[u8]) -> Option<String> {
    let (_, v) = doc.dereference(dict.get(key).ok()?).ok()?;
    lopdf::decode_text_string(v).ok()
}

fn add_to_acroform(
    doc: &mut Document,
    placed: &[Placed],
    roots: &[ObjectId],
    dr_sources: &[(usize, Dictionary)],
) -> Result<(), EngineError> {
    let af_id = acroform_id(doc)?;
    // The existing root fields and their names.
    let existing: Vec<Object> = {
        let af = doc
            .get_dictionary(af_id)
            .map_err(|e| crate::engine::save::lopdf_error("AcroForm", e))?;
        match af.get(b"Fields").ok().map(|f| doc.dereference(f)) {
            Some(Ok((_, Object::Array(items)))) => items.clone(),
            _ => Vec::new(),
        }
    };
    let mut names: HashSet<String> = existing
        .iter()
        .filter_map(|o| o.as_reference().ok())
        .filter_map(|id| doc.get_dictionary(id).ok())
        .filter_map(|d| text_of(doc, d, b"T"))
        .collect();
    let mut fields = existing;
    for &root in roots {
        let name = doc
            .get_dictionary(root)
            .ok()
            .and_then(|d| text_of(doc, d, b"T"));
        if let Some(name) = name.filter(|n| !n.is_empty()) {
            let mut unique = name.clone();
            let mut n = 2;
            while names.contains(&unique) {
                unique = format!("{name}_{n}");
                n += 1;
            }
            if unique != name {
                if let Ok(d) = doc.get_dictionary_mut(root) {
                    d.set("T", crate::engine::save::pdf_text_string(&unique));
                }
            }
            names.insert(unique);
        }
        fields.push(Object::Reference(root));
    }

    // Default appearance and resources from the sources, where the destination has none.
    let mut da: Option<Object> = None;
    let mut fonts: Vec<(Vec<u8>, Object)> = Vec::new();
    let mut other_dr: Vec<(Vec<u8>, Object)> = Vec::new();
    for (k, af) in dr_sources {
        let Some(src) = placed[*k].source.lopdf.as_ref() else {
            continue;
        };
        let mut values: HashMap<ObjectId, ObjectId> = HashMap::new();
        if da.is_none() {
            da = af.get(b"DA").ok().cloned();
        }
        let Some(Ok((_, Object::Dictionary(dr)))) = af.get(b"DR").ok().map(|d| src.dereference(d))
        else {
            continue;
        };
        for (key, value) in dr.iter() {
            if key == b"Font" {
                if let Ok((_, Object::Dictionary(font_dict))) = src.dereference(value) {
                    for (name, font) in font_dict.iter() {
                        if !fonts.iter().any(|(n, _)| n == name) {
                            fonts.push((name.clone(), copy_value(src, doc, font, &mut values, 0)));
                        }
                    }
                }
            } else if !other_dr.iter().any(|(n, _)| n == key) {
                other_dr.push((key.clone(), copy_value(src, doc, value, &mut values, 0)));
            }
        }
    }

    // The destination's /DR, resolved to an owned dictionary we can extend.
    let dr_current = doc
        .get_dictionary(af_id)
        .ok()
        .and_then(|af| af.get(b"DR").ok().cloned());
    let (dr_ref, mut dr) = match dr_current {
        Some(Object::Reference(id)) => (
            Some(id),
            doc.get_dictionary(id).cloned().unwrap_or_default(),
        ),
        Some(Object::Dictionary(d)) => (None, d),
        _ => (None, Dictionary::new()),
    };
    let font_current = dr.get(b"Font").ok().cloned();
    let (font_ref, mut font_dict) = match font_current {
        Some(Object::Reference(id)) => (
            Some(id),
            doc.get_dictionary(id).cloned().unwrap_or_default(),
        ),
        Some(Object::Dictionary(d)) => (None, d),
        _ => (None, Dictionary::new()),
    };
    for (name, font) in fonts {
        if !font_dict.has(&name) {
            font_dict.set(name, font);
        }
    }
    match font_ref {
        Some(id) => {
            doc.objects.insert(id, Object::Dictionary(font_dict));
        }
        None => dr.set("Font", Object::Dictionary(font_dict)),
    }
    for (key, value) in other_dr {
        if !dr.has(&key) {
            dr.set(key, value);
        }
    }
    let af = doc
        .get_dictionary_mut(af_id)
        .map_err(|e| crate::engine::save::lopdf_error("AcroForm", e))?;
    af.set("Fields", Object::Array(fields));
    match dr_ref {
        Some(id) => {
            doc.objects.insert(id, Object::Dictionary(dr));
        }
        None => {
            let af = doc
                .get_dictionary_mut(af_id)
                .map_err(|e| crate::engine::save::lopdf_error("AcroForm", e))?;
            af.set("DR", Object::Dictionary(dr));
        }
    }
    let af = doc
        .get_dictionary_mut(af_id)
        .map_err(|e| crate::engine::save::lopdf_error("AcroForm", e))?;
    if !af.has(b"DA") {
        af.set(
            "DA",
            da.unwrap_or_else(|| Object::string_literal("/Helv 0 Tf 0 g")),
        );
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------
// Outline
// ---------------------------------------------------------------------------------------

/// `node` with its pages mapped through `map` (source index → destination index). A node whose
/// page was not imported is kept as a title only when one of its children survives.
fn map_node(node: &OutlineNode, map: &HashMap<u16, u16>) -> Option<OutlineNode> {
    let children: Vec<OutlineNode> = node
        .children
        .iter()
        .filter_map(|c| map_node(c, map))
        .collect();
    let page = node.page.and_then(|p| map.get(&p).copied());
    if page.is_none() && node.url.is_none() && children.is_empty() {
        return None;
    }
    Some(OutlineNode {
        title: node.title.clone(),
        page,
        dest: if page.is_some() { node.dest } else { None },
        url: node.url.clone(),
        open: if children.is_empty() { None } else { node.open },
        children,
    })
}

fn carry_outline(
    doc: &mut Document,
    placed: &[Placed],
    mode: OutlineMode,
    total: u16,
) -> Result<(), EngineError> {
    let outlined = |p: &Placed| p.source.carry.outline && !p.source.outline.is_empty();
    if !placed.iter().any(outlined) {
        return Ok(());
    }
    let mut nodes: Vec<OutlineNode> = Vec::new();
    for p in placed {
        if !p.source.carry.outline {
            continue;
        }
        if !outlined(p) && mode == OutlineMode::OnlyOutlined {
            continue;
        }
        let map: HashMap<u16, u16> = p
            .source
            .selected
            .iter()
            .enumerate()
            .map(|(j, &s)| (s, p.at + j as u16))
            .collect();
        let children: Vec<OutlineNode> = p
            .source
            .outline
            .iter()
            .filter_map(|n| map_node(n, &map))
            .collect();
        nodes.push(OutlineNode {
            title: p.source.name.clone(),
            page: Some(p.at),
            dest: None,
            url: None,
            open: if children.is_empty() {
                None
            } else {
                Some(true)
            },
            children,
        });
    }
    let nodes = outline::normalize(&nodes, total, 0)?;
    outline::append_in(doc, &nodes)
}

// ---------------------------------------------------------------------------------------
// Page labels
// ---------------------------------------------------------------------------------------

/// One page's label as `(style, prefix, number)`.
#[derive(Debug, Clone, PartialEq)]
struct Spec {
    style: PageLabelStyle,
    prefix: Option<String>,
    number: u64,
}

/// Every page's spec under `ranges` (sorted); a page before the first range is PDFium's
/// fallback — its 1-based number.
fn specs(ranges: &[PageLabelRange], count: u16) -> Vec<Spec> {
    (0..count)
        .map(|page| match ranges.iter().rev().find(|r| r.start <= page) {
            Some(r) => Spec {
                style: r.style,
                prefix: r.prefix.clone(),
                number: r.first.unwrap_or(1) as u64 + (page - r.start) as u64,
            },
            None => Spec {
                style: PageLabelStyle::Decimal,
                prefix: None,
                number: page as u64 + 1,
            },
        })
        .collect()
}

/// Consecutive specs back into ranges. Pure.
fn compress(specs: &[Spec]) -> Vec<PageLabelRange> {
    let mut out: Vec<PageLabelRange> = Vec::new();
    let mut previous: Option<&Spec> = None;
    for (i, s) in specs.iter().enumerate() {
        let continues = previous.is_some_and(|p| {
            p.style == s.style
                && p.prefix == s.prefix
                && (s.style == PageLabelStyle::NoNumber || s.number == p.number + 1)
        });
        if !continues {
            out.push(PageLabelRange {
                start: i as u16,
                style: s.style,
                prefix: s.prefix.clone(),
                first: (s.style != PageLabelStyle::NoNumber)
                    .then_some(s.number.min(labels::MAX_FIRST as u64) as u32),
            });
        }
        previous = Some(s);
    }
    out
}

fn carry_labels(doc: &mut Document, placed: &[Placed], total: u16) -> Result<(), EngineError> {
    let carried = |p: &Placed| p.source.carry.labels && !p.source.labels.is_empty();
    let dest_ranges = labels::read_ranges(doc);
    if dest_ranges.is_empty() && !placed.iter().any(carried) {
        return Ok(());
    }
    let imported: usize = placed.iter().map(|p| p.source.selected.len()).sum();
    let own_count = (total as usize).saturating_sub(imported) as u16;
    let own = specs(&dest_ranges, own_count);
    let mut own_iter = own.into_iter();
    let mut blocks: Vec<&Placed> = placed.iter().collect();
    blocks.sort_by_key(|p| p.at);
    let mut out: Vec<Spec> = Vec::with_capacity(total as usize);
    let mut page = 0u16;
    let mut next_block = blocks.into_iter().peekable();
    while page < total {
        if let Some(p) = next_block.next_if(|p| p.at == page) {
            let source_count = p.source.selected.iter().copied().max().map_or(0, |m| m + 1);
            let src = specs(&p.source.labels, source_count);
            for &s in &p.source.selected {
                out.push(src[s as usize].clone());
            }
            page += p.source.selected.len() as u16;
        } else {
            out.push(own_iter.next().unwrap_or(Spec {
                style: PageLabelStyle::Decimal,
                prefix: None,
                number: page as u64 + 1,
            }));
            page += 1;
        }
    }
    out.truncate(total as usize);
    let ranges = labels::normalize(&compress(&out), total)?;
    labels::set_in(doc, &ranges)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn range(
        start: u16,
        style: PageLabelStyle,
        prefix: Option<&str>,
        first: Option<u32>,
    ) -> PageLabelRange {
        PageLabelRange {
            start,
            style,
            prefix: prefix.map(str::to_owned),
            first,
        }
    }

    #[test]
    fn labels_survive_an_insertion() {
        // i, ii, 1, 2, 3 — insert two "A-1, A-2" pages after "1" (at 3)
        let own = specs(
            &[
                range(0, PageLabelStyle::Roman, None, None),
                range(2, PageLabelStyle::Decimal, None, None),
            ],
            5,
        );
        let src = specs(&[range(0, PageLabelStyle::Decimal, Some("A-"), None)], 2);
        let mut all = own[..3].to_vec();
        all.extend(src);
        all.extend(own[3..].to_vec());
        let ranges = compress(&all);
        let labels = labels::labels_for(&labels::normalize(&ranges, 7).unwrap(), 7);
        assert_eq!(labels, ["i", "ii", "1", "A-1", "A-2", "2", "3"]);
    }

    #[test]
    fn map_node_keeps_only_imported_targets() {
        let leaf = |t: &str, p: u16| OutlineNode {
            title: t.into(),
            page: Some(p),
            dest: None,
            url: None,
            open: None,
            children: vec![],
        };
        let tree = OutlineNode {
            children: vec![leaf("b", 5), leaf("c", 2)],
            ..leaf("a", 9)
        };
        let map: HashMap<u16, u16> = [(2u16, 10u16), (3, 11)].into_iter().collect();
        let mapped = map_node(&tree, &map).unwrap();
        assert_eq!(
            mapped.page, None,
            "the parent's page was not imported: title only"
        );
        assert_eq!(mapped.children.len(), 1);
        assert_eq!(mapped.children[0].page, Some(10));
        assert!(map_node(&leaf("gone", 7), &map).is_none());
    }
}
