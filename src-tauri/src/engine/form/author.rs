//! 필드 만들기 (v0.3 F1): create, change and delete AcroForm fields.
//!
//! PDFium's public API can fill a field but not make one (`FPDFPage_CreateAnnot` refuses
//! `WIDGET`, and nothing writes `/FT`, `/Ff`, `/Opt` or `/AcroForm /Fields`), so every edit
//! here is a lopdf rewrite through [`registry::mutate_bytes`] — one undo step, checked by
//! reopening with PDFium, the document replaced (a document that had no form gets its form
//! handle on the reload).
//!
//! What a new field is:
//!
//! | type | dictionary |
//! |---|---|
//! | text | one widget + field: `/FT /Tx`, `/Ff` (required 2, multiline 4096), `/MaxLen`, `/DA (/Helv 0 Tf 0 g)` (auto size) |
//! | checkbox | `/FT /Btn`, `/V /Off`, `/AS /Off`, `/MK /CA (4)`, `/AP /N << /Yes /Off >>` — a ZapfDingbats check |
//! | radio | a group field `/FT /Btn /Ff 49152` (radio + no-toggle-to-off) with one widget kid per button whose on-state is its export value (`options[0]`, else `선택N`, renumbered if the group already has it); a button named after an existing group joins it, and so does a group renamed after one (`update_form_field`) |
//! | combo | `/FT /Ch /Ff 131072`, `/Opt [...]` |
//! | signature | `/FT /Sig` |
//!
//! Every widget gets `/F 4` (print), `/P`, a thin grey border (`/MK /BC`, `/BS`) and a basic
//! `/AP` so it shows in viewers that do not build appearances; PDFium rebuilds the appearance
//! of a text or choice field the first time it is filled. `/AcroForm` gets `/DR` with Helvetica
//! (`/Helv`) and ZapfDingbats (`/ZaDb`) when it lacks them. Korean typed into a `/Helv` field is
//! drawn by PDFium's font substitution when the value is committed.

use super::extras;
use crate::engine::registry::{self, MutateOpts};
use crate::engine::structure::{self, refuse_encrypted};
use crate::engine::types::EngineState;
use crate::ipc::types::{
    ChangeReason, FormEditResult, FormField, FormFieldPatch, FormFieldSpec, NewFieldType,
    PageIndex, Rect,
};
use crate::ipc::{EngineError, ErrorCode};
use lopdf::{Dictionary, Document, Object, ObjectId, Stream};

const FF_REQUIRED: i64 = 1 << 1;
const FF_MULTILINE: i64 = 1 << 12;
const FF_NO_TOGGLE_TO_OFF: i64 = 1 << 14;
const FF_RADIO: i64 = 1 << 15;
const FF_COMBO: i64 = 1 << 17;
/// Smallest side of a new field, in points.
const MIN_SIDE: f32 = 4.0;

fn lopdf_err(what: &str) -> impl Fn(lopdf::Error) -> EngineError + '_ {
    move |e| crate::engine::save::lopdf_error(what, e)
}

fn edit_opts(label: &'static str, page: PageIndex) -> MutateOpts {
    MutateOpts::new(label, ChangeReason::Edit)
        .page(page)
        .keeps_text()
}

/// The object id of annotation `index` in page `page`'s `/Annots`.
pub(crate) fn widget_id(
    doc: &Document,
    pages: &[ObjectId],
    page: PageIndex,
    index: u32,
) -> Result<ObjectId, EngineError> {
    let page_obj = structure::page_id(pages, page)?;
    let annots = doc
        .get_dictionary(page_obj)
        .map_err(lopdf_err("page"))?
        .get(b"Annots")
        .ok()
        .and_then(|a| doc.dereference(a).ok())
        .and_then(|(_, a)| a.as_array().ok().cloned())
        .unwrap_or_default();
    annots
        .get(index as usize)
        .and_then(|o| o.as_reference().ok())
        .ok_or_else(|| {
            EngineError::not_found(format!("no form field at index {index}")).with_page(page)
        })
}

/// The field a widget belongs to: the nearest dictionary, starting with the widget itself,
/// that has `/FT` (or `/T` for a malformed field without a type).
pub(crate) fn field_of(doc: &Document, widget: ObjectId) -> ObjectId {
    let mut current = widget;
    for _ in 0..32 {
        let Ok(dict) = doc.get_dictionary(current) else {
            return widget;
        };
        if dict.has(b"FT") {
            return current;
        }
        match dict.get(b"Parent").and_then(Object::as_reference) {
            Ok(parent) => current = parent,
            Err(_) => return if dict.has(b"T") { current } else { widget },
        }
    }
    widget
}

fn text(doc: &Document, dict: &Dictionary, key: &[u8]) -> Option<String> {
    let (_, v) = doc.dereference(dict.get(key).ok()?).ok()?;
    lopdf::decode_text_string(v).ok()
}

/// A usable field name: trimmed, not empty, no `.` (the hierarchy separator).
fn check_name(name: &str) -> Result<String, EngineError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(EngineError::invalid("a field needs a name"));
    }
    if name.contains('.') {
        return Err(EngineError::invalid(
            "a field name cannot contain '.' (it separates nested names)",
        ));
    }
    if name.chars().count() > 200 {
        return Err(EngineError::invalid("the field name is too long"));
    }
    Ok(name.to_string())
}

fn normalize_rect(r: Rect) -> Result<Rect, EngineError> {
    for v in [r.l, r.b, r.r, r.t] {
        if !v.is_finite() {
            return Err(EngineError::invalid("the field rect must be finite"));
        }
    }
    let out = Rect::new(r.l.min(r.r), r.b.min(r.t), r.l.max(r.r), r.b.max(r.t));
    if out.width() < MIN_SIDE || out.height() < MIN_SIDE {
        return Err(EngineError::invalid(format!(
            "a field needs at least {MIN_SIDE} pt × {MIN_SIDE} pt"
        )));
    }
    Ok(out)
}

fn rect_object(r: Rect) -> Object {
    Object::Array(vec![
        Object::Real(r.l),
        Object::Real(r.b),
        Object::Real(r.r),
        Object::Real(r.t),
    ])
}

/// The form's `/AcroForm` as an indirect object, with `/Fields`, `/DA` and `/DR` fonts.
fn acroform(doc: &mut Document) -> Result<ObjectId, EngineError> {
    let current = doc
        .catalog()
        .map_err(lopdf_err("catalog"))?
        .get(b"AcroForm")
        .ok()
        .cloned();
    let id = match current {
        Some(Object::Reference(id)) if doc.get_dictionary(id).is_ok() => id,
        Some(Object::Dictionary(inline)) => doc.add_object(Object::Dictionary(inline)),
        _ => doc.add_object(Object::Dictionary(Dictionary::new())),
    };
    doc.catalog_mut()
        .map_err(lopdf_err("catalog"))?
        .set("AcroForm", Object::Reference(id));

    // /DR /Font with /Helv and /ZaDb, wherever the dictionaries live.
    let dr = doc
        .get_dictionary(id)
        .ok()
        .and_then(|af| af.get(b"DR").ok().cloned());
    let dr_id = match dr {
        Some(Object::Reference(r)) if doc.get_dictionary(r).is_ok() => r,
        Some(Object::Dictionary(inline)) => doc.add_object(Object::Dictionary(inline)),
        _ => doc.add_object(Object::Dictionary(Dictionary::new())),
    };
    let fonts = doc
        .get_dictionary(dr_id)
        .ok()
        .and_then(|d| d.get(b"Font").ok().cloned());
    let fonts_id = match fonts {
        Some(Object::Reference(r)) if doc.get_dictionary(r).is_ok() => r,
        Some(Object::Dictionary(inline)) => doc.add_object(Object::Dictionary(inline)),
        _ => doc.add_object(Object::Dictionary(Dictionary::new())),
    };
    let helv = standard_font(doc, b"Helvetica", true);
    let zadb = standard_font(doc, b"ZapfDingbats", false);
    let font_dict = doc.get_dictionary_mut(fonts_id).map_err(lopdf_err("/DR"))?;
    if !font_dict.has(b"Helv") {
        font_dict.set("Helv", Object::Reference(helv));
    }
    if !font_dict.has(b"ZaDb") {
        font_dict.set("ZaDb", Object::Reference(zadb));
    }
    doc.get_dictionary_mut(dr_id)
        .map_err(lopdf_err("/DR"))?
        .set("Font", Object::Reference(fonts_id));
    let af = doc.get_dictionary_mut(id).map_err(lopdf_err("/AcroForm"))?;
    af.set("DR", Object::Reference(dr_id));
    if !af.has(b"DA") {
        af.set("DA", Object::string_literal("/Helv 0 Tf 0 g"));
    }
    if !matches!(
        af.get(b"Fields"),
        Ok(Object::Array(_)) | Ok(Object::Reference(_))
    ) {
        af.set("Fields", Object::Array(Vec::new()));
    }
    Ok(id)
}

fn standard_font(doc: &mut Document, base: &[u8], win_ansi: bool) -> ObjectId {
    let mut font = Dictionary::new();
    font.set("Type", Object::Name(b"Font".to_vec()));
    font.set("Subtype", Object::Name(b"Type1".to_vec()));
    font.set("BaseFont", Object::Name(base.to_vec()));
    if win_ansi {
        font.set("Encoding", Object::Name(b"WinAnsiEncoding".to_vec()));
    }
    doc.add_object(Object::Dictionary(font))
}

/// The `/Fields` array of the form, as an owned list (it may be indirect).
fn fields_array(doc: &Document, af: ObjectId) -> Vec<Object> {
    doc.get_dictionary(af)
        .ok()
        .and_then(|d| d.get(b"Fields").ok())
        .and_then(|f| doc.dereference(f).ok())
        .and_then(|(_, f)| f.as_array().ok().cloned())
        .unwrap_or_default()
}

fn set_fields_array(
    doc: &mut Document,
    af: ObjectId,
    fields: Vec<Object>,
) -> Result<(), EngineError> {
    let indirect = doc
        .get_dictionary(af)
        .ok()
        .and_then(|d| d.get(b"Fields").ok())
        .and_then(|f| f.as_reference().ok());
    match indirect {
        Some(r) => {
            doc.objects.insert(r, Object::Array(fields));
        }
        None => doc
            .get_dictionary_mut(af)
            .map_err(lopdf_err("/AcroForm"))?
            .set("Fields", Object::Array(fields)),
    }
    Ok(())
}

/// The root field named `name`, if any.
fn root_named(doc: &Document, af: ObjectId, name: &str) -> Option<ObjectId> {
    fields_array(doc, af)
        .iter()
        .filter_map(|o| o.as_reference().ok())
        .find(|id| {
            doc.get_dictionary(*id)
                .ok()
                .and_then(|d| text(doc, d, b"T"))
                .as_deref()
                == Some(name)
        })
}

fn appearance(doc: &mut Document, w: f32, h: f32, content: String, fonts: bool) -> ObjectId {
    let mut dict = Dictionary::new();
    dict.set("Type", Object::Name(b"XObject".to_vec()));
    dict.set("Subtype", Object::Name(b"Form".to_vec()));
    dict.set(
        "BBox",
        Object::Array(vec![
            Object::Real(0.0),
            Object::Real(0.0),
            Object::Real(w),
            Object::Real(h),
        ]),
    );
    if fonts {
        let mut font = Dictionary::new();
        font.set(
            "ZaDb",
            Object::Reference(standard_font(doc, b"ZapfDingbats", false)),
        );
        let mut res = Dictionary::new();
        res.set("Font", Object::Dictionary(font));
        dict.set("Resources", Object::Dictionary(res));
    }
    doc.add_object(Object::Stream(Stream::new(dict, content.into_bytes())))
}

fn border(w: f32, h: f32) -> String {
    format!(
        "q 0.6 0.6 0.6 RG 1 w 0.5 0.5 {:.2} {:.2} re S Q\n",
        w - 1.0,
        h - 1.0
    )
}

/// A check / dot glyph centred in the box, ZapfDingbats `4` (✔) or `l` (●).
fn glyph(w: f32, h: f32, ch: char) -> String {
    let size = (w.min(h) * 0.8).max(4.0);
    // ZapfDingbats widths: `4` = 0.846 em, `l` = 0.791 em; the ascent is ~0.7 em.
    let em = if ch == '4' { 0.846 } else { 0.791 };
    let x = (w - size * em) / 2.0;
    let y = (h - size * 0.7) / 2.0;
    format!(
        "{}q 0 g BT /ZaDb {size:.2} Tf {x:.2} {y:.2} Td ({ch}) Tj ET Q\n",
        border(w, h)
    )
}

/// `create_form_field` — the new field's dictionaries (see the module table).
fn write_new_field(
    bytes: &[u8],
    spec: &FormFieldSpec,
    rect: Rect,
    name: &str,
) -> Result<Vec<u8>, EngineError> {
    let mut doc = structure::load(bytes)?;
    let pages = structure::page_ids(&doc);
    let page_obj = structure::page_id(&pages, spec.page)?;
    let af = acroform(&mut doc)?;
    let (w, h) = (rect.width(), rect.height());

    let existing = root_named(&doc, af, name);
    if let Some(found) = existing {
        let is_radio_group = doc
            .get_dictionary(found)
            .ok()
            .and_then(|d| d.get(b"Ff").ok().and_then(|f| f.as_i64().ok()))
            .is_some_and(|ff| ff & FF_RADIO != 0);
        if !(spec.field_type == NewFieldType::Radio && is_radio_group) {
            return Err(EngineError::invalid(format!(
                "a field named '{name}' already exists"
            )));
        }
    }

    let mut widget = Dictionary::new();
    widget.set("Type", Object::Name(b"Annot".to_vec()));
    widget.set("Subtype", Object::Name(b"Widget".to_vec()));
    widget.set("Rect", rect_object(rect));
    widget.set("F", Object::Integer(4));
    widget.set("P", Object::Reference(page_obj));
    let mut mk = Dictionary::new();
    mk.set(
        "BC",
        Object::Array(vec![
            Object::Real(0.6),
            Object::Real(0.6),
            Object::Real(0.6),
        ]),
    );
    let mut bs = Dictionary::new();
    bs.set("W", Object::Integer(1));
    bs.set("S", Object::Name(b"S".to_vec()));
    widget.set("BS", Object::Dictionary(bs));

    let mut ff: i64 = if spec.required { FF_REQUIRED } else { 0 };
    let widget_id = doc.new_object_id();
    let mut root_to_add: Option<ObjectId> = Some(widget_id);

    match spec.field_type {
        NewFieldType::Text => {
            widget.set("FT", Object::Name(b"Tx".to_vec()));
            widget.set("T", crate::engine::save::pdf_text_string(name));
            widget.set("DA", Object::string_literal("/Helv 0 Tf 0 g"));
            if spec.multiline {
                ff |= FF_MULTILINE;
            }
            if let Some(max) = spec.max_len.filter(|m| *m > 0) {
                widget.set("MaxLen", Object::Integer(max as i64));
            }
            let ap = appearance(&mut doc, w, h, border(w, h), false);
            widget.set("AP", ap_dict(Object::Reference(ap)));
        }
        NewFieldType::Signature => {
            widget.set("FT", Object::Name(b"Sig".to_vec()));
            widget.set("T", crate::engine::save::pdf_text_string(name));
            let ap = appearance(&mut doc, w, h, border(w, h), false);
            widget.set("AP", ap_dict(Object::Reference(ap)));
        }
        NewFieldType::Combo => {
            if spec.options.iter().all(|o| o.trim().is_empty()) {
                return Err(EngineError::invalid(
                    "a combo box needs at least one choice",
                ));
            }
            widget.set("FT", Object::Name(b"Ch".to_vec()));
            widget.set("T", crate::engine::save::pdf_text_string(name));
            widget.set("DA", Object::string_literal("/Helv 0 Tf 0 g"));
            ff |= FF_COMBO;
            widget.set("Opt", opt_array(&spec.options));
            let ap = appearance(&mut doc, w, h, border(w, h), false);
            widget.set("AP", ap_dict(Object::Reference(ap)));
        }
        NewFieldType::Checkbox => {
            widget.set("FT", Object::Name(b"Btn".to_vec()));
            widget.set("T", crate::engine::save::pdf_text_string(name));
            widget.set("V", Object::Name(b"Off".to_vec()));
            widget.set("AS", Object::Name(b"Off".to_vec()));
            widget.set("DA", Object::string_literal("/ZaDb 0 Tf 0 g"));
            mk.set("CA", Object::string_literal("4"));
            let on = appearance(&mut doc, w, h, glyph(w, h, '4'), true);
            let off = appearance(&mut doc, w, h, border(w, h), false);
            widget.set("AP", state_ap(b"Yes", on, off));
        }
        NewFieldType::Radio => {
            let export = spec
                .options
                .first()
                .map(|o| o.trim().to_string())
                .filter(|o| !o.is_empty());
            widget.set("AS", Object::Name(b"Off".to_vec()));
            widget.set("DA", Object::string_literal("/ZaDb 0 Tf 0 g"));
            mk.set("CA", Object::string_literal("l"));
            let group = match existing {
                Some(group) => {
                    root_to_add = None;
                    group
                }
                None => {
                    let mut field = Dictionary::new();
                    field.set("FT", Object::Name(b"Btn".to_vec()));
                    field.set("T", crate::engine::save::pdf_text_string(name));
                    field.set("Ff", Object::Integer(ff | FF_RADIO | FF_NO_TOGGLE_TO_OFF));
                    field.set("V", Object::Name(b"Off".to_vec()));
                    field.set("Kids", Object::Array(Vec::new()));
                    let id = doc.add_object(Object::Dictionary(field));
                    root_to_add = Some(id);
                    id
                }
            };
            let kids = radio_kid_count(&doc, group);
            let export = export.unwrap_or_else(|| format!("선택{}", kids + 1));
            // Two buttons with one export value would switch on and off together: a button
            // joining a group gets a state no other button of the group has.
            let export = unique_state(&group_states(&doc, group), &export);
            let on = appearance(&mut doc, w, h, glyph(w, h, 'l'), true);
            let off = appearance(&mut doc, w, h, border(w, h), false);
            widget.set("AP", state_ap(export.as_bytes(), on, off));
            widget.set("Parent", Object::Reference(group));
            if let Ok(g) = doc.get_dictionary_mut(group) {
                match g.get_mut(b"Kids") {
                    Ok(Object::Array(k)) => k.push(Object::Reference(widget_id)),
                    _ => g.set("Kids", Object::Array(vec![Object::Reference(widget_id)])),
                }
            }
            ff = -1; // flags live on the group
        }
    }
    if ff > 0 {
        widget.set("Ff", Object::Integer(ff));
    }
    widget.set("MK", Object::Dictionary(mk));
    doc.objects.insert(widget_id, Object::Dictionary(widget));
    append_annot(&mut doc, page_obj, widget_id)?;
    if let Some(root) = root_to_add {
        let mut fields = fields_array(&doc, af);
        fields.push(Object::Reference(root));
        set_fields_array(&mut doc, af, fields)?;
    }
    structure::write(doc)
}

/// The widget ids under a field: its `/Kids`, or the field itself when it is also its only
/// widget (no `/Kids`).
fn kids_of(doc: &Document, field: ObjectId) -> Vec<ObjectId> {
    let kids = doc
        .get_dictionary(field)
        .ok()
        .and_then(|g| g.get(b"Kids").ok())
        .and_then(|k| doc.dereference(k).ok())
        .and_then(|(_, k)| k.as_array().ok().cloned());
    match kids {
        Some(kids) => kids.iter().filter_map(|o| o.as_reference().ok()).collect(),
        None => vec![field],
    }
}

/// A widget's on-state: its `/AP /N` key other than `Off`.
fn on_state_of(doc: &Document, widget: ObjectId) -> Option<Vec<u8>> {
    let w = doc.get_dictionary(widget).ok()?;
    let (_, ap) = doc.dereference(w.get(b"AP").ok()?).ok()?;
    let (_, normal) = doc.dereference(ap.as_dict().ok()?.get(b"N").ok()?).ok()?;
    normal
        .as_dict()
        .ok()?
        .iter()
        .map(|(k, _)| k.clone())
        .find(|k| k.as_slice() != b"Off")
}

/// The on-states of the buttons of a radio group.
fn group_states(doc: &Document, group: ObjectId) -> Vec<Vec<u8>> {
    kids_of(doc, group)
        .into_iter()
        .filter_map(|k| on_state_of(doc, k))
        .collect()
}

/// `wanted`, or — when a button of the group already has that state — the same text with the
/// next free number (`선택 2` → `선택 3`, `예` → `예 2`).
fn unique_state(taken: &[Vec<u8>], wanted: &str) -> String {
    let free = |s: &str| !taken.iter().any(|t| t.as_slice() == s.as_bytes());
    if free(wanted) {
        return wanted.to_string();
    }
    let digits = wanted
        .chars()
        .rev()
        .take_while(|c| c.is_ascii_digit())
        .count();
    let (base, start) = if digits > 0 {
        let at = wanted.len() - digits;
        (
            wanted[..at].to_string(),
            wanted[at..].parse::<u64>().unwrap_or(1) + 1,
        )
    } else {
        (format!("{wanted} "), 2)
    };
    (start..)
        .map(|n| format!("{base}{n}"))
        .find(|s| free(s))
        .expect("an unbounded range has a free number")
}

/// Renames a widget's on-state `from` to `to` in `/AP /N`, `/AP /D` and `/AS`, wherever those
/// dictionaries live (inline or indirect).
fn rename_state(doc: &mut Document, widget: ObjectId, from: &[u8], to: &[u8]) {
    let rename = |states: &mut Dictionary| {
        if let Some(stream) = states.remove(from) {
            states.set(to.to_vec(), stream);
        }
    };
    let ap = doc
        .get_dictionary(widget)
        .ok()
        .and_then(|w| w.get(b"AP").ok().cloned());
    let (ap_ref, mut ap) = match ap {
        Some(Object::Reference(r)) => match doc.get_dictionary(r) {
            Ok(d) => (Some(r), d.clone()),
            Err(_) => return,
        },
        Some(Object::Dictionary(d)) => (None, d),
        _ => return,
    };
    for key in [&b"N"[..], b"D"] {
        match ap.get(key).ok().cloned() {
            Some(Object::Reference(r)) => {
                if let Ok(states) = doc.get_dictionary_mut(r) {
                    rename(states);
                }
            }
            Some(Object::Dictionary(mut states)) => {
                rename(&mut states);
                ap.set(key.to_vec(), Object::Dictionary(states));
            }
            _ => {}
        }
    }
    match ap_ref {
        Some(r) => {
            doc.objects.insert(r, Object::Dictionary(ap));
        }
        None => {
            if let Ok(w) = doc.get_dictionary_mut(widget) {
                w.set("AP", Object::Dictionary(ap));
            }
        }
    }
    if let Ok(w) = doc.get_dictionary_mut(widget) {
        if w.get(b"AS").and_then(Object::as_name).ok() == Some(from) {
            w.set("AS", Object::Name(to.to_vec()));
        }
    }
}

fn is_radio_group(doc: &Document, field: ObjectId) -> bool {
    doc.get_dictionary(field)
        .ok()
        .filter(|d| !d.has(b"Opt"))
        .and_then(|d| d.get(b"Ff").ok().and_then(|f| f.as_i64().ok()))
        .is_some_and(|ff| ff & FF_RADIO != 0)
}

/// 필드 속성's 이름 naming another radio group: the buttons of radio group `src` join `dst`
/// (a clashing export value is renumbered, [`unique_state`]) and `src` goes. `dst` keeps its
/// value; when it has none, the button that was on in `src` stays on.
fn merge_radio_groups(
    doc: &mut Document,
    af: ObjectId,
    src: ObjectId,
    dst: ObjectId,
) -> Result<(), EngineError> {
    let mut taken = group_states(doc, dst);
    let dst_value = doc
        .get_dictionary(dst)
        .ok()
        .and_then(|d| d.get(b"V").ok())
        .and_then(|v| v.as_name().ok())
        .filter(|v| *v != b"Off")
        .map(<[u8]>::to_vec);
    let mut src_value = doc
        .get_dictionary(src)
        .ok()
        .and_then(|d| d.get(b"V").ok())
        .and_then(|v| v.as_name().ok())
        .filter(|v| *v != b"Off")
        .map(<[u8]>::to_vec);
    let widgets = kids_of(doc, src);
    let merged_widget = widgets == [src];
    for &w in &widgets {
        if let Some(state) = on_state_of(doc, w) {
            let wanted = extras::name_text(&state);
            let new = unique_state(&taken, &wanted).into_bytes();
            if new != state {
                rename_state(doc, w, &state, &new);
                if src_value.as_deref() == Some(state.as_slice()) {
                    src_value = Some(new.clone());
                }
            }
            taken.push(new.clone());
            let keep_on = dst_value.is_none() && src_value.as_deref() == Some(new.as_slice());
            if !keep_on {
                if let Ok(d) = doc.get_dictionary_mut(w) {
                    d.set("AS", Object::Name(b"Off".to_vec()));
                }
            }
        }
        if let Ok(d) = doc.get_dictionary_mut(w) {
            d.set("Parent", Object::Reference(dst));
            if merged_widget {
                // A field-and-widget in one dictionary becomes a plain widget kid.
                for key in [&b"FT"[..], b"T", b"Ff", b"V", b"DV", b"TU", b"TM"] {
                    d.remove(key);
                }
            }
        }
    }
    let mut kids: Vec<Object> = kids_of(doc, dst)
        .into_iter()
        .map(Object::Reference)
        .collect();
    kids.extend(widgets.iter().copied().map(Object::Reference));
    let dst_dict = doc.get_dictionary_mut(dst).map_err(lopdf_err("field"))?;
    dst_dict.set("Kids", Object::Array(kids));
    if dst_value.is_none() {
        if let Some(v) = src_value {
            dst_dict.set("V", Object::Name(v));
        }
    }
    let mut fields = fields_array(doc, af);
    fields.retain(|o| o.as_reference().ok() != Some(src));
    set_fields_array(doc, af, fields)?;
    if !merged_widget {
        doc.objects.remove(&src);
    }
    Ok(())
}

fn radio_kid_count(doc: &Document, group: ObjectId) -> usize {
    doc.get_dictionary(group)
        .ok()
        .and_then(|g| g.get(b"Kids").ok())
        .and_then(|k| doc.dereference(k).ok())
        .and_then(|(_, k)| k.as_array().ok().map(Vec::len))
        .unwrap_or(0)
}

fn ap_dict(normal: Object) -> Object {
    let mut ap = Dictionary::new();
    ap.set("N", normal);
    Object::Dictionary(ap)
}

fn state_ap(on_name: &[u8], on: ObjectId, off: ObjectId) -> Object {
    let mut states = Dictionary::new();
    states.set(on_name.to_vec(), Object::Reference(on));
    states.set("Off", Object::Reference(off));
    let mut ap = Dictionary::new();
    ap.set("N", Object::Dictionary(states));
    Object::Dictionary(ap)
}

fn opt_array(options: &[String]) -> Object {
    Object::Array(
        options
            .iter()
            .map(|o| o.trim())
            .filter(|o| !o.is_empty())
            .map(crate::engine::save::pdf_text_string)
            .collect(),
    )
}

/// Appends `annot` to the page's `/Annots`, wherever that array lives.
fn append_annot(doc: &mut Document, page: ObjectId, annot: ObjectId) -> Result<(), EngineError> {
    let existing = doc
        .get_dictionary(page)
        .map_err(lopdf_err("page"))?
        .get(b"Annots")
        .ok()
        .cloned();
    match existing {
        Some(Object::Reference(array)) => {
            if let Ok(Object::Array(items)) = doc.get_object_mut(array) {
                items.push(Object::Reference(annot));
                return Ok(());
            }
        }
        Some(Object::Array(mut items)) => {
            items.push(Object::Reference(annot));
            doc.get_dictionary_mut(page)
                .map_err(lopdf_err("page"))?
                .set("Annots", Object::Array(items));
            return Ok(());
        }
        _ => {}
    }
    doc.get_dictionary_mut(page)
        .map_err(lopdf_err("page"))?
        .set("Annots", Object::Array(vec![Object::Reference(annot)]));
    Ok(())
}

/// Removes `annot` from the page's `/Annots`.
fn remove_annot(doc: &mut Document, page: ObjectId, annot: ObjectId) -> Result<(), EngineError> {
    let existing = doc
        .get_dictionary(page)
        .map_err(lopdf_err("page"))?
        .get(b"Annots")
        .ok()
        .cloned();
    let keep = |items: &mut Vec<Object>| items.retain(|o| o.as_reference().ok() != Some(annot));
    match existing {
        Some(Object::Reference(array)) => {
            if let Ok(Object::Array(items)) = doc.get_object_mut(array) {
                keep(items);
            }
        }
        Some(Object::Array(mut items)) => {
            keep(&mut items);
            doc.get_dictionary_mut(page)
                .map_err(lopdf_err("page"))?
                .set("Annots", Object::Array(items));
        }
        _ => {}
    }
    Ok(())
}

/// The fields after an edit, and the one at `(page, name, rect)` when asked for.
fn result(
    st: &mut EngineState<'_>,
    doc_id: &str,
    pick: Option<(PageIndex, &str, Rect)>,
) -> Result<FormEditResult, EngineError> {
    let doc = st.doc_mut(doc_id)?;
    let fields = super::list(doc, None)?;
    let field = pick.and_then(|(page, name, rect)| {
        fields
            .iter()
            .filter(|f| f.page == page && f.name == name)
            .min_by(|a, b| {
                let d = |f: &FormField| (f.rect.l - rect.l).abs() + (f.rect.b - rect.b).abs();
                d(a).total_cmp(&d(b))
            })
            .cloned()
    });
    Ok(FormEditResult {
        info: doc.info(),
        fields,
        field,
    })
}

/// `create_form_field` (v0.3 F1). One undo step `undo.formFieldCreate`. `unsupported` on an
/// encrypted or XFA document; `invalidArgument` for a bad name (empty, with a `.`, already
/// used by a field that is not a radio group a radio button can join), a rect under 4 pt, a
/// page out of range or a combo box without choices.
pub fn create_field(
    st: &mut EngineState<'_>,
    doc_id: &str,
    spec: &FormFieldSpec,
) -> Result<FormEditResult, EngineError> {
    refuse_encrypted(st, doc_id)?;
    let doc = st.doc(doc_id)?;
    if doc.xfa {
        return Err(EngineError::new(
            ErrorCode::Unsupported,
            "XFA forms are read-only",
        ));
    }
    if spec.page >= doc.page_count() {
        return Err(
            EngineError::invalid(format!("page {} of {}", spec.page, doc.page_count()))
                .with_page(spec.page),
        );
    }
    let name = check_name(&spec.name)?;
    let rect = normalize_rect(spec.rect)?;
    let spec_owned = spec.clone();
    let name_owned = name.clone();
    registry::mutate_bytes(
        st,
        doc_id,
        edit_opts("undo.formFieldCreate", spec.page),
        move |bytes, _| write_new_field(bytes, &spec_owned, rect, &name_owned),
    )?;
    let out = result(st, doc_id, Some((spec.page, &name, rect)))?;
    if out.field.is_none() {
        return Err(EngineError::new(
            ErrorCode::VerifyFailed,
            "PDFium does not list the new field",
        ));
    }
    Ok(out)
}

/// `delete_form_field` (v0.3 F1): the widget at `(page, index)` goes; so does its field when
/// it was the field's last widget (and an ancestor left with no kids). One undo step
/// `undo.formFieldDelete`.
pub fn delete_field(
    st: &mut EngineState<'_>,
    doc_id: &str,
    page: PageIndex,
    index: u32,
) -> Result<FormEditResult, EngineError> {
    refuse_encrypted(st, doc_id)?;
    super::field_at(st, doc_id, page, index)?;
    registry::mutate_bytes(
        st,
        doc_id,
        edit_opts("undo.formFieldDelete", page),
        move |bytes, _| {
            let mut doc = structure::load(bytes)?;
            let pages = structure::page_ids(&doc);
            let page_obj = structure::page_id(&pages, page)?;
            let widget = widget_id(&doc, &pages, page, index)?;
            remove_annot(&mut doc, page_obj, widget)?;
            let af = acroform(&mut doc)?;
            // Walk up: drop the node from its parent's /Kids (or /Fields), then the parent too
            // if nothing is left under it.
            let mut node = widget;
            for _ in 0..32 {
                let parent = doc
                    .get_dictionary(node)
                    .ok()
                    .and_then(|d| d.get(b"Parent").ok())
                    .and_then(|p| p.as_reference().ok());
                doc.objects.remove(&node);
                match parent {
                    Some(p) => {
                        let empty = match doc.get_dictionary_mut(p) {
                            Ok(pd) => match pd.get_mut(b"Kids") {
                                Ok(Object::Array(kids)) => {
                                    kids.retain(|o| o.as_reference().ok() != Some(node));
                                    kids.is_empty()
                                }
                                _ => true,
                            },
                            Err(_) => false,
                        };
                        if !empty {
                            break;
                        }
                        node = p;
                    }
                    None => {
                        let mut fields = fields_array(&doc, af);
                        fields.retain(|o| o.as_reference().ok() != Some(node));
                        set_fields_array(&mut doc, af, fields)?;
                        break;
                    }
                }
            }
            structure::write(doc)
        },
    )?;
    result(st, doc_id, None)
}

/// `update_form_field` (v0.3 F1): 이름, 선택 항목, 필수, 최대 글자 수 of the field the widget at
/// `(page, index)` belongs to. One undo step `undo.formFieldEdit`. A new name must not be
/// taken by another root field — unless both are radio groups: then the buttons of this group
/// join that one ([`merge_radio_groups`]).
pub fn update_field(
    st: &mut EngineState<'_>,
    doc_id: &str,
    page: PageIndex,
    index: u32,
    patch: &FormFieldPatch,
) -> Result<FormEditResult, EngineError> {
    refuse_encrypted(st, doc_id)?;
    let before = super::field_at(st, doc_id, page, index)?;
    let name = patch.name.as_deref().map(check_name).transpose()?;
    let patch = patch.clone();
    let new_name = name.clone();
    registry::mutate_bytes(
        st,
        doc_id,
        edit_opts("undo.formFieldEdit", page),
        move |bytes, _| {
            let mut doc = structure::load(bytes)?;
            let pages = structure::page_ids(&doc);
            let widget = widget_id(&doc, &pages, page, index)?;
            let mut field = field_of(&doc, widget);
            let mut renamed = false;
            if let Some(name) = &new_name {
                let af = acroform(&mut doc)?;
                let is_root = doc
                    .get_dictionary(field)
                    .map(|d| !d.has(b"Parent"))
                    .unwrap_or(true);
                if let Some(other) = root_named(&doc, af, name) {
                    if other != field && is_root {
                        // A radio group renamed after another radio group joins it — the one
                        // way 필드 속성 has to put buttons drawn apart into one group.
                        if !(is_radio_group(&doc, field) && is_radio_group(&doc, other)) {
                            return Err(EngineError::invalid(format!(
                                "a field named '{name}' already exists"
                            )));
                        }
                        merge_radio_groups(&mut doc, af, field, other)?;
                        field = other;
                        renamed = true;
                    }
                }
            }
            let dict = doc.get_dictionary_mut(field).map_err(lopdf_err("field"))?;
            if let Some(name) = new_name.as_ref().filter(|_| !renamed) {
                dict.set("T", crate::engine::save::pdf_text_string(name));
            }
            if let Some(options) = &patch.options {
                if dict.get(b"FT").and_then(Object::as_name).ok() == Some(b"Ch") {
                    dict.set("Opt", opt_array(options));
                }
            }
            if let Some(required) = patch.required {
                let ff = dict.get(b"Ff").and_then(Object::as_i64).unwrap_or(0);
                let ff = if required {
                    ff | FF_REQUIRED
                } else {
                    ff & !FF_REQUIRED
                };
                dict.set("Ff", Object::Integer(ff));
            }
            if let Some(max) = patch.max_len {
                if max == 0 {
                    dict.remove(b"MaxLen");
                } else {
                    dict.set("MaxLen", Object::Integer(max as i64));
                }
            }
            structure::write(doc)
        },
    )?;
    let name = name.unwrap_or(before.name);
    result(st, doc_id, Some((page, &name, before.rect)))
}
