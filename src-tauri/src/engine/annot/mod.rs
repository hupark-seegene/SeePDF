//! Annotations — `IPC_CONTRACT.md` §7.1, `ARCHITECTURE.md` §6.1.
//!
//! Everything here runs on the engine thread, inside `registry::mutate` when it writes.
//! The pipeline is the raw `FPDFAnnot_*` one from `docs/spikes/annotations.md` §6:
//!
//! ```text
//! CreateAnnot(subtype) -> SetRect -> SetColor(/C) [+ SetColor(/IC), same alpha]
//!   -> SetBorder(0,0,w) -> geometry (quads TL,TR,BL,BR | AddInkStroke | AppendObject)
//!   -> SetFlags(PRINT) -> SetStringValue(NM, T, Contents, Subj, CreationDate, M)
//! ```
//!
//! Three invariants the rest of the engine depends on:
//!
//! * **`/NM` is the annotation id** (uuid v4). It is the only handle that survives a
//!   generation bump, an undo or a save, because page-annotation indices do not.
//! * **Appearance streams are generated lazily, at render time.** Every write adds its page
//!   to `OpenDoc::touched`; `render::tiles::generate_appearances` renders those pages once
//!   before a save, which is what makes the annotation visible in Preview and Acrobat.
//! * **Never touch colour through pdfium-render's high-level setters.** They segfault on any
//!   annotation that already has an `/AP` (annotations spike §5.1); the safe order is
//!   `AnnotRef::clear_ap()` → `set_color()` → next render.
//!
//! Page handling: all of these open their **own** `PdfPage` from `OpenDoc::pdf()` rather than
//! going through the LRU, because building a Stamp needs `&PdfDocument` (for the object
//! constructors) at the same time as the page, and `OpenDoc::page()` borrows the whole
//! `OpenDoc` mutably. `OpenDoc::invalidate_page_handle` is called first so the LRU never holds a
//! second handle to the same page across the edit.

pub mod create;
pub mod font;
pub mod read;
pub mod update;

use crate::engine::raw;
use crate::engine::registry::OpenDoc;
use crate::ipc::error::PdfiumResultExt;
use crate::ipc::types::{Annot, AnnotId, AnnotKind, PageIndex, Rgb};
use crate::ipc::EngineError;
use pdfium_render::prelude::{
    FPDF_FORMHANDLE, PdfPage, PdfPageContentRegenerationStrategy, PdfPageIndex,
};
use std::os::raw::c_int;

/// `/Subj` marker for the kinds PDFium cannot store natively. Written by [`create`], read
/// back by [`read`] so SeePDF round-trips a line/arrow/text box while other viewers still
/// see a perfectly valid Ink or Stamp annotation.
pub const SUBJ_PREFIX: &str = "SeePDF:";
pub const SUBJ_LINE: &str = "SeePDF:Line";
pub const SUBJ_ARROW: &str = "SeePDF:Arrow";
pub const SUBJ_TEXTBOX: &str = "SeePDF:TextBox";
pub const SUBJ_SIGNATURE: &str = "SeePDF:Signature";

/// Where SeePDF mirrors `/C` and `/IC`, because **`FPDFAnnot_GetColor` refuses to read a
/// colour once the annotation has an `/AP`** — the same guard that makes `SetColor` fail
/// (PDFium: "do not attempt to get the colour if the annotation contains an AP stream").
///
/// Since appearance streams are generated on the first render, every annotation SeePDF has
/// ever drawn would otherwise report black after a save and reopen. The mirror is two extra
/// private keys in the annotation dictionary — legal PDF, ignored by every other viewer, and
/// the only way to round-trip a colour through PDFium's public API. Annotations from other
/// producers still have no readable colour; they are reported as `moveOnly`.
pub const KEY_COLOR: &str = "SeePDFC";
pub const KEY_FILL: &str = "SeePDFIC";

/// Writes the `/C` + `/IC` mirror (see [`KEY_COLOR`]). `fill: None` clears the fill mirror.
pub fn record_colors(a: &mut raw::annot::AnnotRef<'_>, stroke: Rgb, fill: Option<Rgb>) {
    a.set_string(KEY_COLOR, &format_rgb(stroke));
    let fill = fill.map(format_rgb).unwrap_or_default();
    a.set_string(KEY_FILL, &fill);
}

fn format_rgb(c: Rgb) -> String {
    format!("{} {} {}", c[0], c[1], c[2])
}

/// Reads one mirrored colour key back.
pub fn read_color_key(a: &raw::annot::AnnotRef<'_>, key: &str) -> Option<Rgb> {
    let value = a.string(key)?;
    let parts: Vec<u8> = value
        .split_whitespace()
        .filter_map(|t| t.parse::<u8>().ok())
        .collect();
    if parts.len() == 3 {
        Some([parts[0], parts[1], parts[2]])
    } else {
        None
    }
}

/// A page opened for annotation work, outside the LRU (see the module docs).
///
/// Dropping it closes the `FPDF_PAGE`; the caller is expected to be inside
/// `registry::mutate`, which re-opens the page through the LRU afterwards.
pub struct ScratchPage<'p> {
    pub page: PdfPage<'p>,
    /// Set by [`ScratchPage::open_with_form`]: `FORM_OnBeforeClosePage` must run before the
    /// page handle is dropped, or the form layer keeps a dangling page.
    form: Option<(&'static dyn pdfium_render::prelude::PdfiumLibraryBindings, FPDF_FORMHANDLE)>,
}

impl<'p> ScratchPage<'p> {
    /// Opens `index` directly from the document with the `Manual` regeneration strategy
    /// (annotation work must never rewrite the page content stream).
    pub fn open(doc: &mut OpenDoc<'p>, index: PageIndex) -> Result<Self, EngineError> {
        Self::open_inner(doc, index, false)
    }

    /// The same, plus `FORM_OnAfterLoadPage` / `FORM_OnBeforeClosePage` — required before any
    /// `FORM_*` event reaches a widget on this page (annotations spike §3.4).
    pub fn open_with_form(doc: &mut OpenDoc<'p>, index: PageIndex) -> Result<Self, EngineError> {
        Self::open_inner(doc, index, true)
    }

    fn open_inner(
        doc: &mut OpenDoc<'p>,
        index: PageIndex,
        with_form: bool,
    ) -> Result<Self, EngineError> {
        if index >= doc.page_count() {
            return Err(
                EngineError::not_found(format!("page {index} of {}", doc.page_count()))
                    .with_page(index),
            );
        }
        // The LRU must not keep a second handle to this page across the edit. The handle
        // only — `mutate` decides whether the extracted text survives (STAGE1A §5.1).
        doc.invalidate_page_handle(index);
        let bindings = doc.bindings();
        let mut page = doc
            .pdf()
            .pages()
            .get(index as PdfPageIndex)
            .ctx(&format!("load page {index}"))?;
        page.set_content_regeneration_strategy(PdfPageContentRegenerationStrategy::Manual);
        let form = match (with_form, doc.form_handle()) {
            (true, Some(form)) => {
                raw::form::on_after_load_page(bindings, &page, form);
                Some((bindings, form))
            }
            _ => None,
        };
        Ok(Self { page, form })
    }
}

impl Drop for ScratchPage<'_> {
    fn drop(&mut self) {
        if let Some((bindings, form)) = self.form {
            raw::form::on_before_close_page(bindings, &self.page, form);
        }
    }
}

/// A fresh annotation id.
pub fn new_id() -> AnnotId {
    uuid::Uuid::new_v4().to_string()
}

/// Maps a PDF subtype (plus our `/Subj` tag) onto the contract's `AnnotKind`.
pub fn kind_of(subtype: c_int, subj: Option<&str>) -> AnnotKind {
    use raw::consts as k;
    match subtype {
        k::FPDF_ANNOT_HIGHLIGHT => AnnotKind::Highlight,
        k::FPDF_ANNOT_UNDERLINE => AnnotKind::Underline,
        k::FPDF_ANNOT_STRIKEOUT => AnnotKind::Strikeout,
        k::FPDF_ANNOT_SQUIGGLY => AnnotKind::Squiggly,
        k::FPDF_ANNOT_SQUARE => AnnotKind::Square,
        k::FPDF_ANNOT_CIRCLE => AnnotKind::Circle,
        k::FPDF_ANNOT_TEXT => AnnotKind::Note,
        k::FPDF_ANNOT_FREETEXT => AnnotKind::Textbox,
        k::FPDF_ANNOT_LINK => AnnotKind::Link,
        k::FPDF_ANNOT_WIDGET => AnnotKind::Widget,
        k::FPDF_ANNOT_LINE => AnnotKind::Line,
        k::FPDF_ANNOT_INK => match subj {
            Some(SUBJ_LINE) => AnnotKind::Line,
            Some(SUBJ_ARROW) => AnnotKind::Arrow,
            Some(SUBJ_SIGNATURE) => AnnotKind::Signature,
            _ => AnnotKind::Ink,
        },
        k::FPDF_ANNOT_STAMP => match subj {
            Some(SUBJ_TEXTBOX) => AnnotKind::Textbox,
            Some(SUBJ_SIGNATURE) => AnnotKind::Signature,
            _ => AnnotKind::Stamp,
        },
        _ => AnnotKind::Other,
    }
}

/// The raw subtype name, as it appears in the PDF (`Annot::subtype`).
pub fn subtype_name(subtype: c_int) -> &'static str {
    use raw::consts as k;
    match subtype {
        k::FPDF_ANNOT_TEXT => "Text",
        k::FPDF_ANNOT_LINK => "Link",
        k::FPDF_ANNOT_FREETEXT => "FreeText",
        k::FPDF_ANNOT_LINE => "Line",
        k::FPDF_ANNOT_SQUARE => "Square",
        k::FPDF_ANNOT_CIRCLE => "Circle",
        k::FPDF_ANNOT_POLYGON => "Polygon",
        k::FPDF_ANNOT_POLYLINE => "PolyLine",
        k::FPDF_ANNOT_HIGHLIGHT => "Highlight",
        k::FPDF_ANNOT_UNDERLINE => "Underline",
        k::FPDF_ANNOT_SQUIGGLY => "Squiggly",
        k::FPDF_ANNOT_STRIKEOUT => "StrikeOut",
        k::FPDF_ANNOT_STAMP => "Stamp",
        k::FPDF_ANNOT_CARET => "Caret",
        k::FPDF_ANNOT_INK => "Ink",
        k::FPDF_ANNOT_POPUP => "Popup",
        k::FPDF_ANNOT_FILEATTACHMENT => "FileAttachment",
        k::FPDF_ANNOT_WIDGET => "Widget",
        k::FPDF_ANNOT_REDACT => "Redact",
        _ => "Unknown",
    }
}

/// `D:YYYYMMDDHHmmSS` for `/CreationDate` and `/M`. PDFium stores these as plain strings and
/// the contract passes them straight through, so a second-resolution UTC stamp (with the
/// `Z`-equivalent `+00'00'` offset) is enough and keeps the crate list free of `chrono`.
pub fn pdf_date_now() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let (y, mo, d, h, mi, s) = civil_from_unix(secs);
    format!("D:{y:04}{mo:02}{d:02}{h:02}{mi:02}{s:02}+00'00'")
}

/// Days-from-civil, Howard Hinnant's algorithm, inverted. No dependency, no time zones.
fn civil_from_unix(secs: i64) -> (i64, u32, u32, u32, u32, u32) {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = if m <= 2 { y + 1 } else { y };
    (
        y,
        m,
        d,
        (rem / 3600) as u32,
        ((rem % 3600) / 60) as u32,
        (rem % 60) as u32,
    )
}

/// Finds the index of the annotation whose `/NM` is `id`, or `notFound`.
pub fn index_of(
    bindings: &'static dyn pdfium_render::prelude::PdfiumLibraryBindings,
    page: &PdfPage<'_>,
    id: &str,
) -> Result<usize, EngineError> {
    for i in 0..raw::annot::count(bindings, page) {
        let annot = raw::annot::get(bindings, page, i)?;
        if annot.string("NM").as_deref() == Some(id) {
            return Ok(i);
        }
    }
    Err(EngineError::not_found(format!("annotation '{id}'")))
}

// ---------------------------------------------------------------------------------------
// public entry points
// ---------------------------------------------------------------------------------------

/// `list_annotations` — every annotation on one page, with a `/NM` assigned where missing.
pub fn list(doc: &mut OpenDoc<'_>, page: PageIndex) -> Result<Vec<Annot>, EngineError> {
    if let Some(cached) = doc.annots.get(&page) {
        return Ok(cached.clone());
    }
    let list = read::list_page(doc, page)?;
    doc.annots.insert(page, list.clone());
    Ok(list)
}

/// `delete_annotations` — removes the named annotations **and** their linked popups.
///
/// Indices shift on every removal, so the whole set is collected first and removed in
/// descending order. Missing ids are reported rather than silently skipped: the frontend
/// treats `notFound` as "re-list and retry".
pub fn delete(
    doc: &mut OpenDoc<'_>,
    page_index: PageIndex,
    ids: &[AnnotId],
) -> Result<usize, EngineError> {
    let bindings = doc.bindings();
    let scratch = ScratchPage::open(doc, page_index)?;
    let page = &scratch.page;
    let count = raw::annot::count(bindings, page);

    let mut victims: Vec<usize> = Vec::new();
    let mut found: Vec<&str> = Vec::new();
    for i in 0..count {
        let annot = raw::annot::get(bindings, page, i)?;
        let name = annot.string("NM");
        let is_target = name
            .as_deref()
            .map(|n| ids.iter().any(|id| id == n))
            .unwrap_or(false);
        if is_target {
            victims.push(i);
            if let Some(id) = ids.iter().find(|id| Some(id.as_str()) == name.as_deref()) {
                found.push(id);
            }
            continue;
        }
        // A Popup whose /Parent is one of the targets goes with it.
        if annot.subtype() == raw::consts::FPDF_ANNOT_POPUP && parent_is_target(&annot, ids) {
            victims.push(i);
        }
    }
    if found.len() < ids.len() {
        let missing: Vec<&AnnotId> = ids
            .iter()
            .filter(|id| !found.contains(&id.as_str()))
            .collect();
        return Err(EngineError::not_found(format!(
            "annotation(s) {missing:?} are not on page {page_index}"
        ))
        .with_page(page_index));
    }
    for &i in victims.iter().rev() {
        raw::annot::remove(bindings, page, i)?;
    }
    let removed = victims.len();
    drop(scratch);
    doc.touched.insert(page_index);
    doc.annots.remove(&page_index);
    Ok(removed)
}

/// Whether this Popup's `/Parent` is one of the annotations being deleted.
///
/// `FPDFAnnot_GetLinkedAnnot` hands back a second handle to the same dictionary, so handle
/// identity means nothing: the comparison is on `/NM`.
fn parent_is_target(annot: &raw::annot::AnnotRef<'_>, ids: &[AnnotId]) -> bool {
    annot
        .linked("Parent")
        .and_then(|parent| parent.string("NM"))
        .map(|name| ids.contains(&name))
        .unwrap_or(false)
}

/// `set_annotations_hidden` (P1) — flips the `/F` HIDDEN bit.
///
/// Transient by contract: the caller bumps `viewNonce`, **not** `docGeneration`, and the
/// document is not dirtied. That makes this the one write that does not go through
/// `registry::mutate`.
pub fn set_hidden(
    doc: &mut OpenDoc<'_>,
    page_index: PageIndex,
    ids: &[AnnotId],
    hidden: bool,
) -> Result<usize, EngineError> {
    let bindings = doc.bindings();
    let scratch = ScratchPage::open(doc, page_index)?;
    let page = &scratch.page;
    let mut changed = 0usize;
    for i in 0..raw::annot::count(bindings, page) {
        let mut annot = raw::annot::get(bindings, page, i)?;
        let Some(name) = annot.string("NM") else {
            continue;
        };
        if !ids.contains(&name) {
            continue;
        }
        let flags = annot.flags();
        let next = if hidden {
            flags | raw::consts::FPDF_ANNOT_FLAG_HIDDEN
        } else {
            flags & !raw::consts::FPDF_ANNOT_FLAG_HIDDEN
        };
        if next != flags && annot.set_flags(next) {
            changed += 1;
        }
    }
    drop(scratch);
    doc.annots.remove(&page_index);
    Ok(changed)
}
