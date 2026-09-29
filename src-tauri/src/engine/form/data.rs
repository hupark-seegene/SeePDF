//! 양식 데이터 내보내기 / 가져오기 (v0.3 F1): field values as CSV or XFDF.
//!
//! * **CSV** — UTF-8 with a BOM (Excel opens Hangul correctly), a `name,value` header, one row
//!   per field (a multi-select list box: one row per selected option), RFC 4180 quoting.
//! * **XFDF** — ISO 19444-1: `<xfdf><fields><field name="…"><value>…</value></field>…`, dotted
//!   names written as nested `<field>` elements; import accepts both nested and dotted names.
//!
//! Values are what `list_form_fields` reports (a checkbox / radio group: its state name —
//! `Off` or the export value). Push buttons and signature fields have no value and are skipped.
//! Import writes every matching field through [`super::write_fields`] as **one** undo step
//! `undo.formImport` (a radio group set to `Off` is switched off, like 값 지우기 — except on an
//! encrypted document, where the group is left as it is); names that match no field are
//! returned in `unknown`. Checkbox / radio values are the export values read with lopdf, so a
//! Hangul export value is written and matched exactly as typed.

use super::{list, write_fields, FieldWrite};
use crate::engine::pages::write_atomic;
use crate::engine::raw;
use crate::engine::types::EngineState;
use crate::ipc::types::{
    FieldType, FieldValue, FormDataFormat, FormDataResult, FormField, PageIndex,
};
use crate::ipc::EngineError;
use std::collections::BTreeMap;
use std::path::Path;

/// `(name, values)` in first-appearance order, one entry per field name.
pub fn values_of(fields: &[FormField]) -> Vec<(String, Vec<String>)> {
    let mut out: Vec<(String, Vec<String>)> = Vec::new();
    for f in fields {
        if matches!(
            f.field_type,
            FieldType::Button | FieldType::Signature | FieldType::Unknown
        ) || f.name.is_empty()
            || out.iter().any(|(n, _)| n == &f.name)
        {
            continue;
        }
        let values = match f.field_type {
            FieldType::List => f
                .options
                .iter()
                .flatten()
                .filter(|o| o.selected)
                .map(|o| o.label.clone())
                .collect(),
            FieldType::Checkbox | FieldType::Radio => {
                vec![f
                    .value
                    .clone()
                    .filter(|v| !v.is_empty())
                    .unwrap_or_else(|| "Off".into())]
            }
            _ => vec![f.value.clone().unwrap_or_default()],
        };
        out.push((f.name.clone(), values));
    }
    out
}

// ---------------------------------------------------------------------------------------
// CSV
// ---------------------------------------------------------------------------------------

fn csv_cell(s: &str) -> String {
    if s.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

pub fn to_csv(values: &[(String, Vec<String>)]) -> String {
    let mut out = String::from("\u{feff}name,value\r\n");
    for (name, vs) in values {
        if vs.is_empty() {
            out.push_str(&format!("{},\r\n", csv_cell(name)));
        }
        for v in vs {
            out.push_str(&format!("{},{}\r\n", csv_cell(name), csv_cell(v)));
        }
    }
    out
}

/// RFC 4180 rows; the header row `name,value` (any case) is skipped.
pub fn parse_csv(text: &str) -> Vec<(String, Vec<String>)> {
    let text = text.trim_start_matches('\u{feff}');
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut row: Vec<String> = Vec::new();
    let mut cell = String::new();
    let mut quoted = false;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if quoted {
            match c {
                '"' if chars.peek() == Some(&'"') => {
                    chars.next();
                    cell.push('"');
                }
                '"' => quoted = false,
                _ => cell.push(c),
            }
            continue;
        }
        match c {
            '"' => quoted = true,
            ',' => row.push(std::mem::take(&mut cell)),
            '\r' => {}
            '\n' => {
                row.push(std::mem::take(&mut cell));
                rows.push(std::mem::take(&mut row));
            }
            _ => cell.push(c),
        }
    }
    if !cell.is_empty() || !row.is_empty() {
        row.push(cell);
        rows.push(row);
    }
    let mut out: Vec<(String, Vec<String>)> = Vec::new();
    for (i, r) in rows.into_iter().enumerate() {
        let Some(name) = r.first().map(|n| n.trim().to_string()) else {
            continue;
        };
        if name.is_empty() || (i == 0 && name.eq_ignore_ascii_case("name")) {
            continue;
        }
        let value = r.get(1).cloned().unwrap_or_default();
        match out.iter_mut().find(|(n, _)| *n == name) {
            Some((_, vs)) => vs.push(value),
            None => out.push((name, vec![value])),
        }
    }
    out
}

// ---------------------------------------------------------------------------------------
// XFDF
// ---------------------------------------------------------------------------------------

fn xml_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            '\r' => out.push_str("&#13;"),
            _ => out.push(c),
        }
    }
    out
}

fn xml_unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        let tail = &rest[at..];
        let Some(end) = tail.find(';') else {
            out.push_str(tail);
            return out;
        };
        let entity = &tail[1..end];
        let decoded = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            e if e.starts_with("#x") || e.starts_with("#X") => u32::from_str_radix(&e[2..], 16)
                .ok()
                .and_then(char::from_u32),
            e if e.starts_with('#') => e[1..].parse::<u32>().ok().and_then(char::from_u32),
            _ => None,
        };
        match decoded {
            Some(c) => out.push(c),
            None => out.push_str(&tail[..=end]),
        }
        rest = &tail[end + 1..];
    }
    out.push_str(rest);
    out
}

#[derive(Default)]
struct Node {
    children: BTreeMap<String, Node>,
    order: Vec<String>,
    values: Option<Vec<String>>,
}

pub fn to_xfdf(values: &[(String, Vec<String>)]) -> String {
    let mut root = Node::default();
    for (name, vs) in values {
        let mut node = &mut root;
        for part in name.split('.') {
            if !node.children.contains_key(part) {
                node.order.push(part.to_string());
            }
            node = node.children.entry(part.to_string()).or_default();
        }
        node.values = Some(vs.clone());
    }
    fn write(node: &Node, depth: usize, out: &mut String) {
        for key in &node.order {
            let child = &node.children[key];
            let pad = "  ".repeat(depth);
            out.push_str(&format!("{pad}<field name=\"{}\">", xml_escape(key)));
            if let Some(vs) = &child.values {
                if vs.is_empty() {
                    out.push_str("<value></value>");
                }
                for v in vs {
                    out.push_str(&format!("<value>{}</value>", xml_escape(v)));
                }
            }
            if !child.order.is_empty() {
                out.push('\n');
                write(child, depth + 1, out);
                out.push_str(&pad);
            }
            out.push_str("</field>\n");
        }
    }
    let mut out = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<xfdf xmlns=\"http://ns.adobe.com/xfdf/\" xml:space=\"preserve\">\n<fields>\n",
    );
    write(&root, 1, &mut out);
    out.push_str("</fields>\n</xfdf>\n");
    out
}

fn attr(tag: &str, name: &str) -> Option<String> {
    let mut rest = tag;
    while let Some(at) = rest.find(name) {
        let before_ok = at == 0 || rest[..at].ends_with(|c: char| c.is_whitespace());
        let after = rest[at + name.len()..].trim_start();
        if before_ok && after.starts_with('=') {
            let v = after[1..].trim_start();
            let quote = v.chars().next()?;
            if quote == '"' || quote == '\'' {
                let end = v[1..].find(quote)?;
                return Some(xml_unescape(&v[1..1 + end]));
            }
        }
        rest = &rest[at + name.len()..];
    }
    None
}

/// The `<field>` / `<value>` structure of an XFDF file, names joined with `.`.
pub fn parse_xfdf(text: &str) -> Result<Vec<(String, Vec<String>)>, EngineError> {
    let mut out: Vec<(String, Vec<String>)> = Vec::new();
    let mut stack: Vec<String> = Vec::new();
    let mut rest = text.trim_start_matches('\u{feff}');
    let mut saw_xfdf = false;
    while let Some(open) = rest.find('<') {
        rest = &rest[open..];
        if rest.starts_with("<!--") {
            let end = rest.find("-->").map(|e| e + 3).unwrap_or(rest.len());
            rest = &rest[end..];
            continue;
        }
        if rest.starts_with("<?") || rest.starts_with("<!") {
            let end = rest.find('>').map(|e| e + 1).unwrap_or(rest.len());
            rest = &rest[end..];
            continue;
        }
        let Some(close) = rest.find('>') else {
            break;
        };
        let tag = &rest[1..close];
        rest = &rest[close + 1..];
        let self_closing = tag.ends_with('/');
        let tag = tag.trim_end_matches('/');
        let name = tag
            .split(|c: char| c.is_whitespace())
            .next()
            .unwrap_or("")
            .rsplit(':')
            .next()
            .unwrap_or("");
        match name {
            "xfdf" => saw_xfdf = true,
            "field" => {
                let field_name = attr(tag, "name").unwrap_or_default();
                if self_closing {
                    continue;
                }
                stack.push(field_name);
            }
            "/field" => {
                stack.pop();
            }
            "value" if !self_closing => {
                let end = rest.find("</").unwrap_or(rest.len());
                let value = xml_unescape(&rest[..end]);
                rest = &rest[end..];
                let full = stack
                    .iter()
                    .filter(|s| !s.is_empty())
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(".");
                if full.is_empty() {
                    continue;
                }
                match out.iter_mut().find(|(n, _)| *n == full) {
                    Some((_, vs)) => vs.push(value),
                    None => out.push((full, vec![value])),
                }
            }
            "value" => {
                let full = stack.join(".");
                if !full.is_empty() && !out.iter().any(|(n, _)| *n == full) {
                    out.push((full, vec![String::new()]));
                }
            }
            _ => {}
        }
    }
    if !saw_xfdf {
        return Err(EngineError::invalid("not an XFDF file (no <xfdf> element)"));
    }
    Ok(out)
}

// ---------------------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------------------

/// `export_form_data`.
pub fn export(
    st: &mut EngineState<'_>,
    doc_id: &str,
    format: FormDataFormat,
    out_path: &str,
) -> Result<FormDataResult, EngineError> {
    let fields = list(st.doc_mut(doc_id)?, None)?;
    let values = values_of(&fields);
    let text = match format {
        FormDataFormat::Csv => to_csv(&values),
        FormDataFormat::Xfdf => to_xfdf(&values),
    };
    write_atomic(Path::new(out_path), text.as_bytes())?;
    Ok(FormDataResult {
        fields: values.len() as u32,
        unknown: Vec::new(),
        doc_generation: None,
    })
}

/// `import_form_data`. `format: None` sniffs the file (`<` first → XFDF, else CSV).
pub fn import(
    st: &mut EngineState<'_>,
    doc_id: &str,
    path: &str,
    format: Option<FormDataFormat>,
) -> Result<FormDataResult, EngineError> {
    let bytes = std::fs::read(path).map_err(EngineError::from)?;
    let text =
        String::from_utf8(bytes).map_err(|_| EngineError::invalid("the file is not UTF-8 text"))?;
    let format = format.unwrap_or_else(|| {
        if text
            .trim_start_matches('\u{feff}')
            .trim_start()
            .starts_with('<')
        {
            FormDataFormat::Xfdf
        } else {
            FormDataFormat::Csv
        }
    });
    let values = match format {
        FormDataFormat::Csv => parse_csv(&text),
        FormDataFormat::Xfdf => parse_xfdf(&text)?,
    };
    let (writes, unknown) = writes_for(st, doc_id, &values)?;
    let count = write_fields(st, doc_id, "undo.formImport", writes)?;
    Ok(FormDataResult {
        fields: count as u32,
        unknown,
        doc_generation: Some(st.doc(doc_id)?.generation),
    })
}

/// The writes that make the document's fields hold `values`.
fn writes_for(
    st: &mut EngineState<'_>,
    doc_id: &str,
    values: &[(String, Vec<String>)],
) -> Result<(Vec<FieldWrite>, Vec<String>), EngineError> {
    let can_clear = super::can_clear_radios(st, doc_id);
    let doc = st.doc_mut(doc_id)?;
    let fields = list(doc, None)?;
    let bindings = doc.bindings();
    let form = doc.form_handle();
    let extras = super::extras::of(doc);
    // Each button's export value: read with lopdf (UTF-8, like the values `list` reports and
    // export writes); PDFium's — which garbles a Hangul one — only where lopdf has none (an
    // encrypted file).
    let mut exports: BTreeMap<(PageIndex, u32), String> = BTreeMap::new();
    for f in fields
        .iter()
        .filter(|f| matches!(f.field_type, FieldType::Radio | FieldType::Checkbox))
    {
        let export = match extras.get(&f.name, f.rect).and_then(|x| x.export.clone()) {
            Some(e) => Some(e),
            None => match form {
                Some(form) => raw::annot::get(bindings, doc.page(f.page)?, f.index as usize)
                    .ok()
                    .and_then(|a| a.form_field_export_value(form)),
                None => None,
            },
        };
        if let Some(e) = export {
            exports.insert((f.page, f.index), e);
        }
    }
    let mut writes = Vec::new();
    let mut unknown = Vec::new();
    for (name, vs) in values {
        let matching: Vec<&FormField> = fields
            .iter()
            .filter(|f| &f.name == name && !f.read_only)
            .collect();
        if matching.is_empty() {
            if !fields.iter().any(|f| &f.name == name) {
                unknown.push(name.clone());
            }
            continue;
        }
        let first = vs.first().cloned().unwrap_or_default();
        let off = first.is_empty() || first == "Off";
        match matching[0].field_type {
            FieldType::Text | FieldType::Combo => {
                // one widget is enough: the others share the field's value
                let f = matching[0];
                if f.value.as_deref().unwrap_or("") != first {
                    writes.push(FieldWrite::Set {
                        page: f.page,
                        index: f.index,
                        value: FieldValue::Text {
                            text: first.clone(),
                        },
                    });
                }
            }
            FieldType::List => {
                let f = matching[0];
                let selected: Vec<u32> = f
                    .options
                    .iter()
                    .flatten()
                    .enumerate()
                    .filter(|(_, o)| vs.contains(&o.label))
                    .map(|(i, _)| i as u32)
                    .collect();
                writes.push(FieldWrite::Set {
                    page: f.page,
                    index: f.index,
                    value: FieldValue::Selected { selected },
                });
            }
            FieldType::Checkbox => {
                for f in &matching {
                    let export = exports.get(&(f.page, f.index));
                    let want = !off && export.is_none_or(|e| *e == first || matching.len() == 1);
                    if f.checked.unwrap_or(false) != want {
                        writes.push(FieldWrite::Set {
                            page: f.page,
                            index: f.index,
                            value: FieldValue::Checked { checked: want },
                        });
                    }
                }
            }
            FieldType::Radio => {
                if off {
                    // An encrypted document cannot have a group switched off: left as it is.
                    if !can_clear {
                        continue;
                    }
                    if let Some(on) = matching.iter().find(|f| f.checked.unwrap_or(false)) {
                        writes.push(FieldWrite::ClearRadio {
                            page: on.page,
                            index: on.index,
                        });
                    }
                } else if let Some(target) = matching
                    .iter()
                    .find(|f| exports.get(&(f.page, f.index)) == Some(&first))
                {
                    if !target.checked.unwrap_or(false) {
                        writes.push(FieldWrite::Set {
                            page: target.page,
                            index: target.index,
                            value: FieldValue::Checked { checked: true },
                        });
                    }
                }
            }
            _ => {}
        }
    }
    Ok((writes, unknown))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(pairs: &[(&str, &[&str])]) -> Vec<(String, Vec<String>)> {
        pairs
            .iter()
            .map(|(n, vs)| (n.to_string(), vs.iter().map(|s| s.to_string()).collect()))
            .collect()
    }

    #[test]
    fn csv_round_trips_quotes_commas_and_hangul() {
        let values = v(&[
            ("성명", &["박현우"]),
            ("주소", &["서울, \"강남\"\n2층"]),
            ("동의", &["Yes"]),
            ("취미", &["독서", "등산"]),
        ]);
        let csv = to_csv(&values);
        assert!(csv.starts_with('\u{feff}'));
        assert_eq!(parse_csv(&csv), values);
    }

    #[test]
    fn xfdf_round_trips_nested_names() {
        let values = v(&[
            ("a.b", &["1 < 2 & 3"]),
            ("a.c", &["x"]),
            ("d", &["한글"]),
            ("list", &["p", "q"]),
        ]);
        let xml = to_xfdf(&values);
        assert!(xml.contains("<field name=\"a\">"));
        assert_eq!(parse_xfdf(&xml).unwrap(), values);
        // dotted names in a flat file are accepted too
        let flat = "<xfdf><fields><field name=\"a.b\"><value>z</value></field></fields></xfdf>";
        assert_eq!(parse_xfdf(flat).unwrap(), v(&[("a.b", &["z"])]));
        assert!(parse_xfdf("name,value\nx,1").is_err());
    }
}
