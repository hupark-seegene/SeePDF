//! Enumeration: one `FPDF_ANNOTATION` → one contract `Annot`.
//!
//! The high-level API covers rect / contents / author / dates / flags, but **opacity
//! (`/CA`), border width, `/InkList`, line endpoints, the appearance-stream length and the
//! `/Subj` tag are raw-only reads** (annotations spike §0), and its colour getters take the
//! same undefined-behaviour fallback path that makes the setters segfault. So everything
//! here goes through `engine::raw::annot`.

use crate::engine::annot::{self, SUBJ_ARROW, SUBJ_LINE, SUBJ_PREFIX, SUBJ_TEXTBOX};
use crate::engine::raw::{self, annot::AnnotRef, annot::ColorKind, consts};
use crate::engine::registry::OpenDoc;
use crate::ipc::types::{Annot, AnnotKind, Editability, PageIndex, Rect, Rgb};
use crate::ipc::EngineError;
use pdfium_render::prelude::FPDF_DOCUMENT;
use std::os::raw::c_int;

/// Default markup colour when the annotation carries none (PDFium draws black).
const DEFAULT_COLOR: Rgb = [0, 0, 0];

/// Reads every annotation on `page`, assigning a `/NM` where one is missing.
///
/// Assigning the id is required by `IPC_CONTRACT.md` §7.1 — without it a third-party
/// annotation has no stable handle. It writes to the in-memory document but deliberately
/// does **not** bump the generation or dirty the document: it is an identity stamp, not a
/// user edit. `OpenDoc::ids_stamped` makes the next real edit snapshot the stamped document
/// (not the file), so undoing that edit keeps the ids.
///
/// A `/Annots` slot that holds no annotation (`null`, a dangling reference) is skipped.
pub fn list_page(doc: &mut OpenDoc<'_>, page_index: PageIndex) -> Result<Vec<Annot>, EngineError> {
    let bindings = doc.bindings();
    let document = doc.pdf().raw_handle();
    // The LRU page, not a `ScratchPage`: listing is a read path, and re-opening the page
    // would re-parse its content stream *and* drop the cached text layer on every call.
    let page = doc.page(page_index)?;
    let count = raw::annot::count(bindings, page);
    // Pass 1: every annotation gets its `/NM` first, so a reply listed *before* its parent
    // (P2 threads) still resolves `/IRT` to the parent's id in pass 2.
    let mut ids: Vec<Option<String>> = Vec::with_capacity(count);
    let mut stamped = false;
    for i in 0..count {
        let Some(mut a) = raw::annot::slot(bindings, page, i) else {
            ids.push(None);
            continue;
        };
        if a.subtype() == consts::FPDF_ANNOT_POPUP {
            // Popups are drawn by the React layer from the parent's contents; PDFium never
            // renders a standalone one and the contract has no `popup` kind.
            ids.push(None);
            continue;
        }
        let id = match a.string("NM") {
            Some(id) if !id.is_empty() => id,
            _ => {
                let id = annot::new_id();
                a.set_string("NM", &id);
                stamped = true;
                id
            }
        };
        ids.push(Some(id));
    }
    let mut out = Vec::with_capacity(count);
    for (i, id) in ids.into_iter().enumerate() {
        let Some(id) = id else { continue };
        let Some(a) = raw::annot::slot(bindings, page, i) else {
            continue;
        };
        out.push(read_one(&a, id, page_index, document));
    }
    if stamped {
        doc.ids_stamped = true;
    }
    Ok(out)
}

/// The `/NM` of the annotation `a` replies to (P2 threads), or `None`.
///
/// `/IRT` is an indirect reference to the parent's dictionary; `/RT` is `/R` (a reply, also
/// the default when absent) or `/Group` (the two annotations are one unit — not a reply).
/// `FPDFAnnot_GetLinkedAnnot` resolves the reference, and the parent is identified by its
/// `/NM`, like everywhere else in this module.
pub fn in_reply_to(a: &AnnotRef<'_>) -> Option<String> {
    if a.subtype() == consts::FPDF_ANNOT_POPUP {
        return None;
    }
    let rt = a.string("RT");
    if rt.is_some_and(|rt| rt.trim_start_matches('/') == "Group") {
        return None;
    }
    a.linked("IRT")
        .and_then(|parent| parent.string("NM"))
        .filter(|nm| !nm.is_empty())
}

/// One annotation, fully populated.
pub fn read_one(a: &AnnotRef<'_>, id: String, page: PageIndex, document: FPDF_DOCUMENT) -> Annot {
    let subtype = a.subtype();
    let subj = a.string("Subj");
    let kind = annot::kind_of(subtype, subj.as_deref());
    let rect = a.rect().unwrap_or(Rect::ZERO);
    let flags = a.flags();
    let ap_len = a.ap_len();

    // `FPDFAnnot_GetColor` refuses to answer once an `/AP` exists, which is true of every
    // annotation that has been rendered or reopened, so SeePDF's own mirror comes first
    // (`annot::KEY_COLOR`). Third-party annotations have neither and report the default.
    let direct = a.color(ColorKind::Stroke);
    let alpha = direct.map(|(_, alpha)| alpha).unwrap_or(255);
    let color = annot::read_color_key(a, annot::KEY_COLOR)
        .or(direct.map(|(c, _)| c))
        .unwrap_or(DEFAULT_COLOR);
    let fill_color = annot::read_color_key(a, annot::KEY_FILL)
        .or_else(|| a.color(ColorKind::Interior).map(|(c, _)| c));
    // `/CA` is authoritative when present; otherwise the alpha of `/C`.
    let opacity = a.opacity().unwrap_or(alpha as f32 / 255.0).clamp(0.0, 1.0);

    let quads = match kind {
        AnnotKind::Highlight
        | AnnotKind::Underline
        | AnnotKind::Strikeout
        | AnnotKind::Squiggly
        | AnnotKind::Link => {
            let q = a.quads();
            if q.is_empty() {
                None
            } else {
                Some(q)
            }
        }
        _ => None,
    };

    let ink_paths = match subtype {
        consts::FPDF_ANNOT_INK => {
            let p = a.ink_paths();
            if p.is_empty() {
                None
            } else {
                Some(p)
            }
        }
        _ => None,
    };

    // A real `/Line` annotation reports its endpoints; ours is an Ink whose first stroke is
    // the segment itself (`ARCHITECTURE.md` §6.1).
    let line_points = match kind {
        AnnotKind::Line | AnnotKind::Arrow => a.line().or_else(|| {
            ink_paths
                .as_ref()
                .and_then(|p| p.first())
                .filter(|s| s.len() >= 4)
                .map(|s| [s[0], s[1], s[s.len() - 2], s[s.len() - 1]])
        }),
        _ => None,
    };

    let da = a.string("DA");
    let font_size = match kind {
        AnnotKind::Textbox => da.as_deref().and_then(parse_da_font_size),
        _ => None,
    };
    let contents = a.string("Contents").unwrap_or_default();
    let text = match kind {
        AnnotKind::Textbox => Some(contents.clone()),
        _ => None,
    };

    Annot {
        id,
        page,
        kind,
        subtype: annot::subtype_name(subtype).to_string(),
        rect,
        quads,
        ink_paths,
        line_points,
        color,
        fill_color,
        opacity,
        border_width: a.border_width().unwrap_or(1.0),
        contents,
        author: a.string("T"),
        created: a.string("CreationDate"),
        modified: a.string("M"),
        text,
        font_size,
        stamp_kind: match kind {
            AnnotKind::Stamp | AnnotKind::Signature => subj.clone(),
            _ => None,
        },
        image_id: None,
        uri: match kind {
            AnnotKind::Link => a.uri(document),
            _ => None,
        },
        dest: match kind {
            AnnotKind::Link => a.link_dest(document),
            _ => None,
        },
        in_reply_to: in_reply_to(a),
        hidden: flags & consts::FPDF_ANNOT_FLAG_HIDDEN != 0,
        printed: flags & consts::FPDF_ANNOT_FLAG_PRINT != 0,
        locked: flags & consts::FPDF_ANNOT_FLAG_LOCKED != 0,
        editable: editability(subtype, flags, ap_len, subj.as_deref()),
    }
}

/// How much of an annotation SeePDF is willing to change (`IPC_CONTRACT.md` §7.1).
///
/// `moveOnly` is the honest answer for a third-party appearance stream: we can move its
/// `/Rect` (PDFium re-maps the AP through the Rect/BBox matrix) but regenerating it would
/// throw away whatever the other application drew.
fn editability(subtype: c_int, flags: c_int, ap_len: usize, subj: Option<&str>) -> Editability {
    if subtype == consts::FPDF_ANNOT_WIDGET
        || flags & consts::FPDF_ANNOT_FLAG_READONLY != 0
        || matches!(
            subtype,
            consts::FPDF_ANNOT_FILEATTACHMENT | consts::FPDF_ANNOT_POPUP
        )
    {
        return Editability::ReadOnly;
    }
    let ours = subj.map(|s| s.starts_with(SUBJ_PREFIX)).unwrap_or(false);
    let regenerable = matches!(
        subtype,
        consts::FPDF_ANNOT_HIGHLIGHT
            | consts::FPDF_ANNOT_UNDERLINE
            | consts::FPDF_ANNOT_SQUIGGLY
            | consts::FPDF_ANNOT_STRIKEOUT
            | consts::FPDF_ANNOT_SQUARE
            | consts::FPDF_ANNOT_CIRCLE
            | consts::FPDF_ANNOT_INK
            | consts::FPDF_ANNOT_TEXT
            | consts::FPDF_ANNOT_FREETEXT
            | consts::FPDF_ANNOT_LINK
    );
    if ours || ap_len == 0 || regenerable {
        Editability::Full
    } else {
        Editability::MoveOnly
    }
}

/// The font size out of a `/DA` string such as `"0 0 1 rg /Helv 12 Tf"`.
pub fn parse_da_font_size(da: &str) -> Option<f32> {
    let tokens: Vec<&str> = da.split_whitespace().collect();
    let tf = tokens.iter().position(|t| *t == "Tf")?;
    tokens.get(tf.wrapping_sub(1))?.parse::<f32>().ok()
}

/// The fill colour out of a `/DA` string (`"r g b rg"`), 0..1 floats → 0..255.
pub fn parse_da_color(da: &str) -> Option<Rgb> {
    let tokens: Vec<&str> = da.split_whitespace().collect();
    let rg = tokens.iter().position(|t| *t == "rg")?;
    if rg < 3 {
        return None;
    }
    let c: Vec<f32> = tokens[rg - 3..rg]
        .iter()
        .filter_map(|t| t.parse::<f32>().ok())
        .collect();
    if c.len() != 3 {
        return None;
    }
    Some([
        (c[0].clamp(0.0, 1.0) * 255.0).round() as u8,
        (c[1].clamp(0.0, 1.0) * 255.0).round() as u8,
        (c[2].clamp(0.0, 1.0) * 255.0).round() as u8,
    ])
}

/// `"<r> <g> <b> rg /Helv <size> Tf"` — what PDFium's FreeText appearance generator reads,
/// and where a text box keeps its font size and colour so they survive a round trip.
pub fn build_da(color: Rgb, font_size: f32) -> String {
    format!(
        "{:.4} {:.4} {:.4} rg /Helv {:.2} Tf",
        color[0] as f32 / 255.0,
        color[1] as f32 / 255.0,
        color[2] as f32 / 255.0,
        font_size
    )
}

/// Kinds whose geometry lives in `/QuadPoints`.
pub fn is_markup(kind: AnnotKind) -> bool {
    matches!(
        kind,
        AnnotKind::Highlight | AnnotKind::Underline | AnnotKind::Strikeout | AnnotKind::Squiggly
    )
}

/// The `/Subj` tag a kind is stored with, if any.
pub fn subj_for(kind: AnnotKind) -> Option<&'static str> {
    match kind {
        AnnotKind::Line => Some(SUBJ_LINE),
        AnnotKind::Arrow => Some(SUBJ_ARROW),
        AnnotKind::Textbox => Some(SUBJ_TEXTBOX),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn da_roundtrip() {
        let da = build_da([0, 0, 255], 14.0);
        assert_eq!(parse_da_font_size(&da), Some(14.0));
        assert_eq!(parse_da_color(&da), Some([0, 0, 255]));
    }

    #[test]
    fn da_from_another_producer() {
        assert_eq!(parse_da_font_size(" /Helvetica 9 Tf"), Some(9.0));
        assert_eq!(parse_da_color("1 0 0 rg /Helv 8 Tf"), Some([255, 0, 0]));
        assert_eq!(parse_da_font_size("nothing here"), None);
    }
}
