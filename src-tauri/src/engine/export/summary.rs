//! Annotation summary export — P2, `IPC_CONTRACT.md` §7.7a (`export_annotation_summary`).
//!
//! One row per annotation, pages ascending and, within a page, in the page's `/Annots` order
//! (the order the 주석 sidebar lists them in). Links and form widgets are not comments and are
//! left out; popups never appear on their own (`annot::read`).
//!
//! * **Quoted text** of a text markup (형광펜 / 밑줄 / 취소선 / 물결) comes from the page's
//!   text layer — the same cached `TextLayer` selection and search use: every character whose
//!   box centre falls inside one of the markup's quads, in reading order, whitespace folded.
//! * **Labels** (column headers, kind names) come from the frontend's own `src/i18n/*.json`
//!   through [`crate::app::undo_labels::text`], in the locale the caller names.
//! * **Dates** are PDF dates (`D:YYYYMMDDHHmmSS+hh'mm'`) shown in local time as
//!   `YYYY-MM-DD HH:MM:SS`; anything unparseable is written as found.
//! * **CSV** is RFC 4180 (CRLF, `"` doubled, quoted when needed) in UTF-8 **with a BOM**, so
//!   Excel opens Hangul correctly. A free-text cell that starts with `=`, `+`, `-` or `@` gets
//!   a leading `'`, so a hostile annotation cannot become a spreadsheet formula.
//! * **TXT** and **Markdown** are UTF-8 without a BOM.
//!
//! The file is written atomically (`pages::write_atomic`).

use crate::app::undo_labels;
use crate::engine::annot;
use crate::engine::export::check_pages;
use crate::engine::pages::write_atomic;
use crate::engine::text::layer::{self, TextLayer};
use crate::engine::types::EngineState;
use crate::ipc::types::{
    Annot, AnnotKind, AnnotationSummaryResult, Locale, PageIndex, Rect, SummaryFormat,
};
use crate::ipc::EngineError;
use std::path::Path;

/// Slack around a quad when deciding whether a character centre is inside it, in points.
const QUAD_SLACK: f32 = 1.0;

/// One exported annotation.
#[derive(Debug, Clone, PartialEq)]
pub struct SummaryRow {
    /// 1-based.
    pub page: u32,
    /// `/PageLabels` label, when the document has one.
    pub label: Option<String>,
    pub kind: String,
    pub author: String,
    pub created: String,
    pub modified: String,
    /// `#RRGGBB`.
    pub color: String,
    pub contents: String,
    /// The marked-up words, text markup only.
    pub quote: String,
    /// v0.3 A7: the annotation's id (`/NM`) — what a reply's `reply_to` names.
    pub id: String,
    /// v0.3 A7: the id of the annotation this one replies to (P2 threads), when it does.
    pub reply_to: Option<String>,
    /// v0.3 A7: 0 for a top-level annotation, 1 for a reply, 2 for a reply to a reply…
    pub depth: u32,
}

/// `export_annotation_summary`.
pub fn export_annotation_summary(
    st: &mut EngineState<'_>,
    doc_id: &str,
    path: &str,
    format: SummaryFormat,
    pages: Option<&[PageIndex]>,
    locale: Locale,
) -> Result<AnnotationSummaryResult, EngineError> {
    if path.trim().is_empty() {
        return Err(EngineError::invalid("the output path is empty"));
    }
    let rows = collect(st, doc_id, pages, locale)?;
    let name = st.doc(doc_id)?.name();
    let body = render(&rows, format, &name, locale);
    let bytes = write_atomic(Path::new(path), &body)?;
    Ok(AnnotationSummaryResult {
        count: rows.len() as u32,
        bytes,
    })
}

/// Every exported annotation of `pages` (default: every page), in page order.
pub fn collect(
    st: &mut EngineState<'_>,
    doc_id: &str,
    pages: Option<&[PageIndex]>,
    locale: Locale,
) -> Result<Vec<SummaryRow>, EngineError> {
    let pages = check_pages(st, doc_id, pages.unwrap_or(&[]))?;
    let mut rows = Vec::new();
    for page in pages {
        let doc = st.doc_mut(doc_id)?;
        let annots: Vec<Annot> = annot::list(doc, page)?
            .into_iter()
            .filter(|a| !matches!(a.kind, AnnotKind::Link | AnnotKind::Widget))
            .collect();
        if annots.is_empty() {
            continue;
        }
        let label = doc.geom(page)?.label.clone();
        let needs_text = annots.iter().any(|a| is_text_markup(a.kind));
        let text = if needs_text {
            Some(layer::layer(doc, page)?)
        } else {
            None
        };
        for (a, depth) in threaded(annots) {
            let quote = match (&text, is_text_markup(a.kind)) {
                (Some(text), true) => {
                    let quads = match &a.quads {
                        Some(q) if !q.is_empty() => q.clone(),
                        _ => vec![a.rect],
                    };
                    quoted_text(text, &quads)
                }
                _ => String::new(),
            };
            let contents = match (&a.text, a.contents.trim().is_empty()) {
                // a text box keeps what it says in `text`; `/Contents` may be empty
                (Some(t), true) => t.clone(),
                _ => a.contents.clone(),
            };
            rows.push(SummaryRow {
                page: page as u32 + 1,
                label: label.clone().filter(|l| !l.trim().is_empty()),
                kind: kind_label(&a, locale),
                author: a.author.clone().unwrap_or_default(),
                created: a.created.as_deref().map(format_date).unwrap_or_default(),
                modified: a.modified.as_deref().map(format_date).unwrap_or_default(),
                color: format!("#{:02X}{:02X}{:02X}", a.color[0], a.color[1], a.color[2]),
                contents,
                quote,
                id: a.id.clone(),
                reply_to: if depth > 0 {
                    a.in_reply_to.clone()
                } else {
                    None
                },
                depth,
            });
        }
    }
    Ok(rows)
}

/// v0.3 A7: the page's annotations in thread order — each top-level annotation (in `/Annots`
/// order) followed by its replies, depth-first, each level oldest first — with their depth. A
/// reply whose parent is not in the list (another page, a link) is listed as top-level.
pub fn threaded(annots: Vec<Annot>) -> Vec<(Annot, u32)> {
    let ids: std::collections::HashSet<String> = annots.iter().map(|a| a.id.clone()).collect();
    let parent_of = |a: &Annot| {
        a.in_reply_to
            .clone()
            .filter(|p| ids.contains(p) && *p != a.id)
    };
    let created = |a: &Annot| a.created.as_deref().map(format_date).unwrap_or_default();
    let mut out: Vec<(Annot, u32)> = Vec::with_capacity(annots.len());
    let mut placed: std::collections::HashSet<String> = std::collections::HashSet::new();
    fn visit(
        node: &Annot,
        depth: u32,
        all: &[Annot],
        parent_of: &dyn Fn(&Annot) -> Option<String>,
        created: &dyn Fn(&Annot) -> String,
        placed: &mut std::collections::HashSet<String>,
        out: &mut Vec<(Annot, u32)>,
    ) {
        if !placed.insert(node.id.clone()) {
            return; // a cycle (`/IRT` loops exist in the wild) is listed once
        }
        out.push((node.clone(), depth));
        let mut children: Vec<&Annot> = all
            .iter()
            .filter(|c| parent_of(c).as_deref() == Some(node.id.as_str()))
            .collect();
        children.sort_by_key(|c| created(c));
        for child in children {
            visit(child, depth + 1, all, parent_of, created, placed, out);
        }
    }
    for a in annots.iter().filter(|a| parent_of(a).is_none()) {
        visit(a, 0, &annots, &parent_of, &created, &mut placed, &mut out);
    }
    // Anything only reachable through a cycle.
    for a in &annots {
        if !placed.contains(&a.id) {
            visit(a, 0, &annots, &parent_of, &created, &mut placed, &mut out);
        }
    }
    out
}

fn is_text_markup(kind: AnnotKind) -> bool {
    matches!(
        kind,
        AnnotKind::Highlight | AnnotKind::Underline | AnnotKind::Strikeout | AnnotKind::Squiggly
    )
}

// ---------------------------------------------------------------------------------------
// Quoted text
// ---------------------------------------------------------------------------------------

/// The characters of `text` whose box centre lies inside one of `quads`, in reading order.
/// A skipped whitespace or line break between two kept characters becomes one space.
pub fn quoted_text(text: &TextLayer, quads: &[Rect]) -> String {
    let inside = |c: &layer::CharEntry| -> bool {
        let (x, y) = ((c.loose.l + c.loose.r) / 2.0, (c.loose.b + c.loose.t) / 2.0);
        quads.iter().any(|q| {
            x >= q.l - QUAD_SLACK
                && x <= q.r + QUAD_SLACK
                && y >= q.b - QUAD_SLACK
                && y <= q.t + QUAD_SLACK
        })
    };
    let mut out = String::new();
    let mut gap = false;
    let mut started = false;
    for c in &text.chars {
        let ch = char::from_u32(c.codepoint).unwrap_or('\u{fffd}');
        let blank = ch.is_whitespace() || c.is_generated();
        if !blank && c.loose.width() > 0.0 && inside(c) {
            if gap && started {
                out.push(' ');
            }
            out.push(ch);
            started = true;
            gap = false;
        } else if blank {
            gap = true;
        } else if started {
            // A character outside every quad between two kept runs (another column, a word
            // the highlight skipped): the runs are separate phrases.
            gap = true;
        }
    }
    out.trim().to_string()
}

// ---------------------------------------------------------------------------------------
// Labels, dates
// ---------------------------------------------------------------------------------------

fn tr(key: &str, locale: Locale, fallback: &'static str) -> String {
    undo_labels::text(key, locale)
        .unwrap_or(fallback)
        .to_string()
}

/// The kind's name as the 주석 sidebar shows it (`annotLabel` in `AnnotationList.tsx`).
pub fn kind_label(a: &Annot, locale: Locale) -> String {
    let (key, fallback) = match a.kind {
        AnnotKind::Highlight => ("tool.highlight", "Highlight"),
        AnnotKind::Underline => ("tool.underline", "Underline"),
        AnnotKind::Strikeout => ("tool.strikeout", "Strikethrough"),
        AnnotKind::Squiggly => ("tool.squiggly", "Squiggly"),
        AnnotKind::Note => ("tool.note", "Note"),
        AnnotKind::Ink => ("tool.pen", "Pen"),
        AnnotKind::Square => ("tool.rectangle", "Rectangle"),
        AnnotKind::Circle => ("tool.ellipse", "Ellipse"),
        AnnotKind::Line => ("annot.kind.line", "Line"),
        AnnotKind::Arrow => ("annot.kind.arrow", "Arrow"),
        AnnotKind::Textbox => ("annot.kind.textbox", "Text box"),
        AnnotKind::Stamp => ("tool.stamp", "Stamp"),
        AnnotKind::Signature => ("tool.signature", "Signature"),
        AnnotKind::Polygon => ("annot.kind.polygon", "Polygon"),
        AnnotKind::Polyline => ("annot.kind.polyline", "Polyline"),
        AnnotKind::Callout => ("annot.kind.callout", "Callout"),
        AnnotKind::Link | AnnotKind::Widget | AnnotKind::Other => return a.subtype.clone(),
    };
    tr(key, locale, fallback)
}

/// `D:YYYYMMDDHHmmSS±hh'mm'` → local `YYYY-MM-DD HH:MM:SS`. ISO-8601 strings (what the mock
/// and some producers write) are accepted too; anything else comes back unchanged.
pub fn format_date(raw: &str) -> String {
    use chrono::{DateTime, FixedOffset, Local, NaiveDate, TimeZone};
    let s = raw.trim();
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return dt
            .with_timezone(&Local)
            .format("%Y-%m-%d %H:%M:%S")
            .to_string();
    }
    let body = s.strip_prefix("D:").unwrap_or(s);
    let digits: String = body.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.len() < 4 {
        return raw.to_string();
    }
    let num = |from: usize, len: usize, default: u32| -> u32 {
        digits
            .get(from..from + len)
            .and_then(|d| d.parse().ok())
            .unwrap_or(default)
    };
    let (year, month, day) = (num(0, 4, 0) as i32, num(4, 2, 1), num(6, 2, 1));
    let (hour, minute, second) = (num(8, 2, 0), num(10, 2, 0), num(12, 2, 0));
    let Some(naive) = NaiveDate::from_ymd_opt(year, month, day)
        .and_then(|d| d.and_hms_opt(hour, minute, second.min(59)))
    else {
        return raw.to_string();
    };
    // Timezone: `Z`, `+hh'mm'`, `-hh'mm'`; absent = unknown, shown as written (no conversion).
    let tz = &body[digits.len()..];
    let offset = match tz.chars().next() {
        Some('Z') => Some(0),
        Some(sign @ ('+' | '-')) => {
            let nums: Vec<i32> = tz[1..]
                .split(|c: char| !c.is_ascii_digit())
                .filter(|p| !p.is_empty())
                .filter_map(|p| p.parse().ok())
                .collect();
            let secs =
                nums.first().copied().unwrap_or(0) * 3600 + nums.get(1).copied().unwrap_or(0) * 60;
            Some(if sign == '-' { -secs } else { secs })
        }
        _ => None,
    };
    match offset.and_then(FixedOffset::east_opt) {
        Some(offset) => match offset.from_local_datetime(&naive).single() {
            Some(dt) => dt
                .with_timezone(&Local)
                .format("%Y-%m-%d %H:%M:%S")
                .to_string(),
            None => naive.format("%Y-%m-%d %H:%M:%S").to_string(),
        },
        None => naive.format("%Y-%m-%d %H:%M:%S").to_string(),
    }
}

// ---------------------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------------------

/// The file's bytes in `format`.
pub fn render(
    rows: &[SummaryRow],
    format: SummaryFormat,
    doc_name: &str,
    locale: Locale,
) -> Vec<u8> {
    match format {
        SummaryFormat::Csv => render_csv(rows, locale),
        SummaryFormat::Txt => render_txt(rows, doc_name, locale).into_bytes(),
        SummaryFormat::Md => render_md(rows, doc_name, locale).into_bytes(),
    }
}

struct Headers {
    page: String,
    label: String,
    kind: String,
    author: String,
    created: String,
    modified: String,
    color: String,
    contents: String,
    quote: String,
    id: String,
    reply_to: String,
}

fn headers(locale: Locale) -> Headers {
    Headers {
        page: tr("annotSummary.col.page", locale, "Page"),
        label: tr("annotSummary.col.label", locale, "Page label"),
        kind: tr("annotSummary.col.kind", locale, "Type"),
        author: tr("annotSummary.col.author", locale, "Author"),
        created: tr("annotSummary.col.created", locale, "Created"),
        modified: tr("annotSummary.col.modified", locale, "Modified"),
        color: tr("annotSummary.col.color", locale, "Colour"),
        contents: tr("annotSummary.col.contents", locale, "Contents"),
        quote: tr("annotSummary.col.quote", locale, "Quoted text"),
        id: tr("annotSummary.col.id", locale, "id"),
        reply_to: tr("annotSummary.col.replyTo", locale, "reply_to"),
    }
}

fn page_heading(row: &SummaryRow, locale: Locale) -> String {
    let base =
        tr("annotSummary.page", locale, "Page {{n}}").replace("{{n}}", &row.page.to_string());
    match &row.label {
        Some(label) if *label != row.page.to_string() => format!("{base} ({label})"),
        _ => base,
    }
}

fn count_line(count: usize, locale: Locale) -> String {
    tr("annotSummary.count", locale, "{{count}} annotations")
        .replace("{{count}}", &count.to_string())
}

/// RFC 4180 with a UTF-8 BOM and CRLF line ends.
fn render_csv(rows: &[SummaryRow], locale: Locale) -> Vec<u8> {
    let h = headers(locale);
    let mut out = String::from("\u{FEFF}");
    let header = [
        &h.page,
        &h.label,
        &h.kind,
        &h.author,
        &h.created,
        &h.modified,
        &h.color,
        &h.contents,
        &h.quote,
        &h.id,
        &h.reply_to,
    ];
    out.push_str(
        &header
            .iter()
            .map(|c| csv_field(c, false))
            .collect::<Vec<_>>()
            .join(","),
    );
    out.push_str("\r\n");
    for r in rows {
        let cells = [
            csv_field(&r.page.to_string(), false),
            csv_field(r.label.as_deref().unwrap_or(""), true),
            csv_field(&r.kind, false),
            csv_field(&r.author, true),
            csv_field(&r.created, false),
            csv_field(&r.modified, false),
            csv_field(&r.color, false),
            csv_field(&r.contents, true),
            csv_field(&r.quote, true),
            csv_field(&r.id, true),
            csv_field(r.reply_to.as_deref().unwrap_or(""), true),
        ];
        out.push_str(&cells.join(","));
        out.push_str("\r\n");
    }
    out.into_bytes()
}

/// One CSV cell. `free_text` cells get the spreadsheet-formula guard (module docs).
pub fn csv_field(value: &str, free_text: bool) -> String {
    let mut v = value.replace("\r\n", "\n");
    if free_text && v.starts_with(['=', '+', '-', '@']) {
        v.insert(0, '\'');
    }
    let needs_quotes = v.contains([',', '"', '\n', '\r'])
        || v.starts_with(char::is_whitespace)
        || v.ends_with(char::is_whitespace);
    if needs_quotes {
        format!("\"{}\"", v.replace('"', "\"\""))
    } else {
        v
    }
}

fn render_txt(rows: &[SummaryRow], doc_name: &str, locale: Locale) -> String {
    let h = headers(locale);
    let mut out = format!(
        "{} — {}\n{}\n",
        tr("annotSummary.title", locale, "Annotations"),
        doc_name,
        count_line(rows.len(), locale)
    );
    if rows.is_empty() {
        out.push('\n');
        out.push_str(&tr("annotSummary.none", locale, "No annotations"));
        out.push('\n');
    }
    for r in rows {
        out.push('\n');
        let mut head = vec![r.kind.clone()];
        if !r.author.is_empty() {
            head.push(r.author.clone());
        }
        let when = if r.modified.is_empty() {
            &r.created
        } else {
            &r.modified
        };
        if !when.is_empty() {
            head.push(when.clone());
        }
        // v0.3 A7: a reply sits under its parent, indented by its depth (no blank line).
        let pad = "    ".repeat(r.depth as usize);
        if r.depth > 0 {
            out.pop();
            out.push_str(&format!("{pad}\u{21B3} {}\n", head.join(" · ")));
        } else {
            out.push_str(&format!(
                "[{}] {}\n",
                page_heading(r, locale),
                head.join(" · ")
            ));
        }
        if !r.quote.is_empty() {
            out.push_str(&format!(
                "{pad}  {}: \u{201C}{}\u{201D}\n",
                h.quote, r.quote
            ));
        }
        if !r.contents.trim().is_empty() {
            out.push_str(&format!(
                "{pad}  {}: {}\n",
                h.contents,
                indent(&r.contents, &format!("{pad}    "))
            ));
        }
        if r.depth == 0 {
            out.push_str(&format!("  {}: {}\n", h.color, r.color));
        }
    }
    out
}

fn render_md(rows: &[SummaryRow], doc_name: &str, locale: Locale) -> String {
    let mut out = format!(
        "# {} — {}\n\n{}\n",
        tr("annotSummary.title", locale, "Annotations"),
        md_escape(doc_name),
        count_line(rows.len(), locale)
    );
    if rows.is_empty() {
        out.push('\n');
        out.push_str(&tr("annotSummary.none", locale, "No annotations"));
        out.push('\n');
    }
    let mut current: Option<u32> = None;
    for r in rows {
        if current != Some(r.page) {
            current = Some(r.page);
            out.push_str(&format!("\n## {}\n\n", md_escape(&page_heading(r, locale))));
        }
        let mut head = vec![format!("**{}**", md_escape(&r.kind))];
        if !r.author.is_empty() {
            head.push(md_escape(&r.author));
        }
        let when = if r.modified.is_empty() {
            &r.created
        } else {
            &r.modified
        };
        if !when.is_empty() {
            head.push(when.clone());
        }
        // v0.3 A7: a reply is a nested list item under its parent (two spaces per level).
        let pad = "  ".repeat(r.depth as usize);
        if r.depth == 0 {
            head.push(format!("`{}`", r.color));
        } else {
            out.push('\n');
        }
        out.push_str(&format!("{pad}- {}\n", head.join(" · ")));
        if !r.quote.is_empty() {
            out.push_str(&format!("\n{pad}  > {}\n", md_escape(&r.quote)));
        }
        if !r.contents.trim().is_empty() {
            out.push('\n');
            for line in r.contents.replace("\r\n", "\n").lines() {
                if line.trim().is_empty() {
                    out.push('\n');
                } else {
                    out.push_str(&format!("{pad}  {}\n", md_escape(line)));
                }
            }
        }
    }
    out
}

fn indent(text: &str, pad: &str) -> String {
    text.replace("\r\n", "\n")
        .replace('\n', &format!("\n{pad}"))
}

/// Backslash-escapes the characters Markdown would read as markup.
pub fn md_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if matches!(
            c,
            '\\' | '`' | '*' | '_' | '[' | ']' | '<' | '>' | '#' | '|'
        ) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(page: u32, kind: &str, contents: &str, quote: &str) -> SummaryRow {
        SummaryRow {
            page,
            label: None,
            kind: kind.into(),
            author: "홍길동".into(),
            created: String::new(),
            modified: "2026-09-28 12:00:00".into(),
            color: "#FFD400".into(),
            contents: contents.into(),
            quote: quote.into(),
            id: format!("id-{page}"),
            reply_to: None,
            depth: 0,
        }
    }

    #[test]
    fn replies_nest_under_their_parent() {
        let mut parent = row(1, "형광펜", "원문", "Trace");
        parent.id = "p".into();
        let mut r1 = row(1, "메모", "첫 답글", "");
        (r1.id, r1.reply_to, r1.depth) = ("r1".into(), Some("p".into()), 1);
        let mut r2 = row(1, "메모", "두 번째", "");
        (r2.id, r2.reply_to, r2.depth) = ("r2".into(), Some("p".into()), 1);
        let rows = vec![parent, r1, r2];
        let md = String::from_utf8(render(&rows, SummaryFormat::Md, "a.pdf", Locale::Ko)).unwrap();
        assert!(md.contains("\n  - **메모** · 홍길동"), "{md}");
        assert!(md.contains("\n    첫 답글\n"), "{md}");
        let txt =
            String::from_utf8(render(&rows, SummaryFormat::Txt, "a.pdf", Locale::En)).unwrap();
        assert!(txt.contains("\n    \u{21B3} 메모 · 홍길동"), "{txt}");
        let csv =
            String::from_utf8(render(&rows, SummaryFormat::Csv, "a.pdf", Locale::En)).unwrap();
        assert!(
            csv.contains(",r1,p\r\n") && csv.contains(",r2,p\r\n"),
            "{csv}"
        );
    }

    #[test]
    fn csv_quotes_only_what_needs_it() {
        assert_eq!(csv_field("plain", true), "plain");
        assert_eq!(csv_field("a,b", true), "\"a,b\"");
        assert_eq!(csv_field("say \"hi\"", true), "\"say \"\"hi\"\"\"");
        assert_eq!(csv_field("two\r\nlines", true), "\"two\nlines\"");
        assert_eq!(csv_field("=HYPERLINK(1)", true), "'=HYPERLINK(1)");
        assert_eq!(csv_field("-5", false), "-5");
        assert_eq!(csv_field(" padded", false), "\" padded\"");
    }

    #[test]
    fn csv_has_bom_crlf_and_one_row_per_annotation() {
        let rows = vec![
            row(1, "형광펜", "메모, \"인용\"", "Trace-based"),
            row(3, "메모", "두 줄\n내용", ""),
        ];
        let bytes = render(&rows, SummaryFormat::Csv, "doc.pdf", Locale::Ko);
        assert_eq!(&bytes[..3], &[0xEF, 0xBB, 0xBF]);
        let text = String::from_utf8(bytes[3..].to_vec()).unwrap();
        assert!(text.starts_with("페이지,페이지 레이블,종류,"));
        assert!(text.contains(
            "1,,형광펜,홍길동,,2026-09-28 12:00:00,#FFD400,\"메모, \"\"인용\"\"\",Trace-based,id-1,\r\n"
        ));
        assert!(text.contains("\"두 줄\n내용\""));
        // header + 2 records; the embedded LF is inside quotes, so count CRLFs
        assert_eq!(text.matches("\r\n").count(), 3);
    }

    #[test]
    fn txt_and_markdown_group_by_page() {
        let rows = vec![
            row(1, "형광펜", "", "Trace-based"),
            row(1, "메모", "확인 *필요*", ""),
            row(2, "펜", "", ""),
        ];
        let md = String::from_utf8(render(&rows, SummaryFormat::Md, "a.pdf", Locale::Ko)).unwrap();
        assert!(md.starts_with("# 주석 목록 — a.pdf\n\n주석 3개\n"));
        assert_eq!(md.matches("\n## 1쪽\n").count(), 1);
        assert!(md.contains("\n## 2쪽\n"));
        assert!(md.contains("  > Trace-based\n"));
        assert!(md.contains("확인 \\*필요\\*"));
        let txt =
            String::from_utf8(render(&rows, SummaryFormat::Txt, "a.pdf", Locale::En)).unwrap();
        assert!(txt.starts_with("Annotations — a.pdf\n3 annotations\n"));
        assert!(txt.contains("[Page 1] 형광펜 · 홍길동 · 2026-09-28 12:00:00\n"));
        assert!(txt.contains("  Quoted text: \u{201C}Trace-based\u{201D}\n"));
    }

    #[test]
    fn pdf_dates_are_read_leniently() {
        // no timezone: shown as written
        assert_eq!(format_date("D:20260928123456"), "2026-09-28 12:34:56");
        assert_eq!(format_date("D:2026"), "2026-01-01 00:00:00");
        assert_eq!(format_date("not a date"), "not a date");
        // with a timezone: converted to local time, so only the shape is stable here
        let converted = format_date("D:20260928123456+09'00'");
        assert_eq!(converted.len(), "2026-09-28 12:34:56".len());
        let utc = format_date("D:20260928123456Z");
        assert_eq!(utc, format_date("2026-09-28T12:34:56Z"));
    }
}
