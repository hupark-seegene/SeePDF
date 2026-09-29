//! What PDFium's form API does not expose, read with lopdf (v0.3 F2): `/MaxLen` and the
//! default value `/DV` of every widget's field — both inheritable through `/Parent`.
//!
//! Read-only, from the bytes the document was last loaded from (`OpenDoc::bytes`), parsed at
//! most once per load (a one-entry cache on the engine thread, keyed by the bytes' address and
//! length). Filling a field does not change either value and page operations do not move a
//! widget's `/Rect`, so the widgets are matched by **field name + rectangle**, not by page and
//! annotation index — which is what keeps the cache valid across fills and page moves. An
//! encrypted document, or bytes lopdf cannot parse, simply has no extras (`maxLen` absent, reset
//! to empty — the pre-v0.3 behaviour).

use crate::engine::registry::OpenDoc;
use crate::ipc::types::Rect;
use lopdf::{Dictionary, Document, Object, ObjectId};
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;

/// A field's `/DV`.
#[derive(Debug, Clone, PartialEq)]
pub enum DefaultValue {
    /// A text or choice field's string.
    Text(String),
    /// A checkbox / radio state name (`Off` or an export value).
    State(String),
    /// A multi-select list's strings.
    Many(Vec<String>),
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct WidgetExtra {
    pub max_len: Option<u32>,
    pub default: Option<DefaultValue>,
    /// A checkbox / radio widget's on-state (its `/AP /N` key other than `Off`), decoded the
    /// same way as `default` — so the two compare even for a non-ASCII export value, which
    /// PDFium's `FPDFAnnot_GetFormFieldExportValue` decodes differently.
    pub on_state: Option<String>,
    /// A checkbox / radio widget's export value — what `list_form_fields` reports as its value
    /// when it is on and what data export / import use: the field's `/Opt` entry for this
    /// widget when the field has `/Opt`, else [`Self::on_state`]. Decoded as UTF-8 (names) or
    /// a PDF text string (`/Opt`), unlike PDFium, which reads name bytes as PDFDocEncoding and
    /// so turns every Hangul export value (`여`, `선택2`) into mojibake.
    pub export: Option<String>,
}

/// Keyed by `(fully qualified name, rect rounded to 0.1 pt)`.
#[derive(Debug, Default)]
pub struct Extras {
    by_widget: HashMap<(String, [i32; 4]), WidgetExtra>,
}

fn rect_key(r: Rect) -> [i32; 4] {
    let n = |v: f32| (v * 10.0).round() as i32;
    [
        n(r.l.min(r.r)),
        n(r.b.min(r.t)),
        n(r.l.max(r.r)),
        n(r.b.max(r.t)),
    ]
}

impl Extras {
    pub fn get(&self, name: &str, rect: Rect) -> Option<&WidgetExtra> {
        self.by_widget.get(&(name.to_string(), rect_key(rect)))
    }

    /// Parses `bytes`; an unreadable or encrypted file gives no extras.
    pub fn parse(bytes: &[u8]) -> Extras {
        let Ok(doc) = Document::load_mem(bytes) else {
            return Extras::default();
        };
        if doc.is_encrypted() {
            return Extras::default();
        }
        let mut out = Extras::default();
        for page in doc.get_pages().into_values() {
            let Ok(dict) = doc.get_dictionary(page) else {
                continue;
            };
            let Ok(annots) = dict.get(b"Annots").and_then(|a| doc.dereference(a)) else {
                continue;
            };
            let Ok(items) = annots.1.as_array() else {
                continue;
            };
            for item in items {
                let Ok((_, Object::Dictionary(w))) = doc.dereference(item) else {
                    continue;
                };
                if w.get(b"Subtype").and_then(Object::as_name).ok() != Some(b"Widget") {
                    continue;
                }
                let Some(rect) = rect_of(&doc, w) else {
                    continue;
                };
                let id = item.as_reference().ok();
                let name = qualified_name(&doc, w, id);
                let on_state = on_state(&doc, w);
                let export = opt_export(&doc, w, id).or_else(|| on_state.clone());
                let extra = WidgetExtra {
                    max_len: inherited(&doc, w, b"MaxLen")
                        .and_then(|o| o.as_i64().ok())
                        .and_then(|v| u32::try_from(v).ok())
                        .filter(|v| *v > 0),
                    default: inherited(&doc, w, b"DV").and_then(|o| default_of(&doc, o)),
                    on_state,
                    export,
                };
                out.by_widget.insert((name, rect_key(rect)), extra);
            }
        }
        out
    }
}

pub(crate) fn rect_of(doc: &Document, dict: &Dictionary) -> Option<Rect> {
    let (_, r) = doc.dereference(dict.get(b"Rect").ok()?).ok()?;
    let v: Vec<f32> = r
        .as_array()
        .ok()?
        .iter()
        .filter_map(|o| {
            doc.dereference(o).ok().and_then(|(_, o)| {
                o.as_float()
                    .ok()
                    .or_else(|| o.as_i64().ok().map(|i| i as f32))
            })
        })
        .collect();
    (v.len() == 4).then(|| Rect::new(v[0], v[1], v[2], v[3]))
}

/// `a.b.c` from the `/T` of the widget and its ancestors (a kid without `/T` adds nothing).
pub(crate) fn qualified_name(doc: &Document, widget: &Dictionary, _id: Option<ObjectId>) -> String {
    let mut parts: Vec<String> = Vec::new();
    let mut current = Some(widget.clone());
    let mut depth = 0;
    while let Some(dict) = current {
        if depth > 32 {
            break;
        }
        depth += 1;
        if let Some(t) = dict
            .get(b"T")
            .ok()
            .and_then(|t| doc.dereference(t).ok())
            .and_then(|(_, t)| lopdf::decode_text_string(t).ok())
        {
            parts.push(t);
        }
        current = dict
            .get(b"Parent")
            .and_then(Object::as_reference)
            .ok()
            .and_then(|p| doc.get_dictionary(p).ok())
            .cloned();
    }
    parts.reverse();
    parts.join(".")
}

/// The first `key` on the widget or one of its ancestors, dereferenced.
fn inherited<'a>(doc: &'a Document, widget: &'a Dictionary, key: &[u8]) -> Option<&'a Object> {
    let mut current: &Dictionary = widget;
    for _ in 0..32 {
        if let Ok(v) = current.get(key) {
            return doc.dereference(v).ok().map(|(_, o)| o);
        }
        let parent = current.get(b"Parent").and_then(Object::as_reference).ok()?;
        current = doc.get_dictionary(parent).ok()?;
    }
    None
}

fn on_state(doc: &Document, widget: &Dictionary) -> Option<String> {
    let (_, ap) = doc.dereference(widget.get(b"AP").ok()?).ok()?;
    let (_, normal) = doc.dereference(ap.as_dict().ok()?.get(b"N").ok()?).ok()?;
    normal
        .as_dict()
        .ok()?
        .iter()
        .map(|(k, _)| k)
        .find(|k| k.as_slice() != b"Off")
        .map(|k| name_text(k))
}

/// A name's bytes as text: UTF-8 (ISO 32000-2 §7.3.5, and what SeePDF writes), or — for a
/// legacy name that is not valid UTF-8 — one char per byte (Latin-1, which is PDFDocEncoding
/// for every letter a name is likely to hold).
pub(crate) fn name_text(bytes: &[u8]) -> String {
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_string(),
        Err(_) => bytes.iter().map(|&b| b as char).collect(),
    }
}

/// `/Opt[i]` of the widget's field, `i` being the widget's position in the field's `/Kids`
/// (ISO 32000 §12.7.4.2.4) — only for a widget that is a kid of a field with `/Opt`.
fn opt_export(doc: &Document, widget: &Dictionary, id: Option<ObjectId>) -> Option<String> {
    let id = id?;
    let opt = inherited(doc, widget, b"Opt")?.as_array().ok()?;
    let parent = widget.get(b"Parent").and_then(Object::as_reference).ok()?;
    let (_, kids) = doc
        .dereference(doc.get_dictionary(parent).ok()?.get(b"Kids").ok()?)
        .ok()?;
    let at = kids
        .as_array()
        .ok()?
        .iter()
        .position(|k| k.as_reference().ok() == Some(id))?;
    let (_, entry) = doc.dereference(opt.get(at)?).ok()?;
    lopdf::decode_text_string(entry).ok()
}

fn default_of(doc: &Document, value: &Object) -> Option<DefaultValue> {
    match value {
        Object::Name(n) => Some(DefaultValue::State(name_text(n))),
        Object::String(..) => lopdf::decode_text_string(value)
            .ok()
            .map(DefaultValue::Text),
        Object::Array(items) => Some(DefaultValue::Many(
            items
                .iter()
                .filter_map(|o| doc.dereference(o).ok())
                .filter_map(|(_, o)| lopdf::decode_text_string(o).ok())
                .collect(),
        )),
        _ => None,
    }
}

/// `(doc id, bytes address, bytes length)`.
type CacheKey = (String, usize, usize);

thread_local! {
    /// One entry: `(doc id, bytes address, bytes length)` → extras. Engine thread only.
    static CACHE: RefCell<Option<(CacheKey, Arc<Extras>)>> = const { RefCell::new(None) };
}

/// The extras of `doc`'s current bytes (parsed on first use after each load).
pub fn of(doc: &OpenDoc<'_>) -> Arc<Extras> {
    let key = (
        doc.doc_id.clone(),
        doc.bytes.as_ptr() as usize,
        doc.bytes.len(),
    );
    if let Some(hit) = CACHE.with(|c| {
        c.borrow()
            .as_ref()
            .filter(|(k, _)| *k == key)
            .map(|(_, e)| e.clone())
    }) {
        return hit;
    }
    let extras = Arc::new(Extras::parse(&doc.bytes));
    CACHE.with(|c| *c.borrow_mut() = Some((key, extras.clone())));
    extras
}
