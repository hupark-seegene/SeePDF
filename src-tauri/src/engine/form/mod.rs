//! AcroForm fields — `IPC_CONTRACT.md` §7.2, `ARCHITECTURE.md` §6.4.
//!
//! **`PdfFormTextField::set_value` is not used anywhere in SeePDF.** It writes `/V` and
//! nothing else, so the field keeps its old (usually empty) appearance stream and the value
//! is invisible in PDFium, in macOS Preview and in every other viewer that trusts `/AP`
//! (annotations spike §3.4 measured 0 changed pixels). Filling goes through the form-fill
//! environment, which regenerates the appearance:
//!
//! ```text
//! text:      FORM_SetFocusedAnnot -> FORM_SelectAllText -> FORM_ReplaceSelection
//!            fallback:  click the widget centre -> FORM_SelectAllText -> backspace -> FORM_OnChar*
//! toggle:    click the widget centre, verify with FPDFAnnot_IsChecked, click again if needed
//! choice:    FORM_SetFocusedAnnot -> FORM_SetIndexSelected per index
//! always:    FORM_ForceToKillFocus   // commits /V *and* writes the regenerated /AP
//! ```
//!
//! The first line was the `WORKPLAN.md` §6 unverified item; [`TextEntryMethod`] records which
//! of the two actually ran, the probe result is asserted by `form_text_value_persists` and
//! written up in `STAGE1A_NOTES.md`.
//!
//! v0.3 (pkg2): `/MaxLen` and `/DV` are read with lopdf ([`extras`]); [`reset_form`] applies
//! `/DV`; a radio group is cleared (`/V /Off`, every kid `/AS /Off`) by a lopdf rewrite, since no
//! click can switch the last button of a group off ([`clear_radio`]); a combo box that takes no
//! typing is set by selecting the option whose label is the value, and emptied by a rewrite too,
//! since PDFium cannot deselect one ([`clear_choice`]); authoring ([`author`]),
//! data exchange ([`data`]) and flattening ([`flatten`]) live in their own modules.

pub mod author;
pub mod data;
pub mod extras;
pub mod flatten;

use crate::engine::annot::ScratchPage;
use crate::engine::raw::{self, annot::AnnotRef, consts};
use crate::engine::registry::{self, MutateOpts, OpenDoc};
use crate::engine::types::EngineState;
use crate::ipc::types::{
    ChangeReason, DocInfo, FieldType, FieldValue, FormField, FormOption, PageIndex, Rect,
};
use crate::ipc::{EngineError, ErrorCode};
use extras::{DefaultValue, Extras};
use pdfium_render::prelude::{PdfPage, PdfPageIndex, PdfiumLibraryBindings, FPDF_FORMHANDLE};
use std::collections::{BTreeMap, BTreeSet};
use std::os::raw::c_int;

/// `/Ff` bit 19 of a choice field: a combo box that also takes typed text.
const FF_CHOICE_EDIT: c_int = 1 << 18;

/// Which of the two text-entry paths was used for the last write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextEntryMethod {
    /// `FORM_SetFocusedAnnot` + `FORM_SelectAllText` + `FORM_ReplaceSelection`.
    ReplaceSelection,
    /// The spike-verified fallback: click the widget, then one `FORM_OnChar` per UTF-16 unit.
    CharLoop,
    /// Not a text field.
    NotText,
}

/// What [`set_value`] did, so the command can build `SetFormFieldResult` and the test can
/// assert which `FORM_*` path ran.
#[derive(Debug)]
pub struct WriteOutcome {
    pub field: FormField,
    pub previous: Option<String>,
    pub method: TextEntryMethod,
}

fn field_type_of(raw_type: c_int) -> FieldType {
    match raw_type {
        consts::FPDF_FORMFIELD_PUSHBUTTON => FieldType::Button,
        consts::FPDF_FORMFIELD_CHECKBOX => FieldType::Checkbox,
        consts::FPDF_FORMFIELD_RADIOBUTTON => FieldType::Radio,
        consts::FPDF_FORMFIELD_COMBOBOX => FieldType::Combo,
        consts::FPDF_FORMFIELD_LISTBOX => FieldType::List,
        consts::FPDF_FORMFIELD_TEXTFIELD => FieldType::Text,
        consts::FPDF_FORMFIELD_SIGNATURE => FieldType::Signature,
        _ => FieldType::Unknown,
    }
}

/// `list_form_fields` — every widget on `page`, or on the whole document when `page` is
/// `None`. Documents without an AcroForm return an empty list rather than an error.
pub fn list(doc: &mut OpenDoc<'_>, page: Option<PageIndex>) -> Result<Vec<FormField>, EngineError> {
    let Some(form) = doc.form_handle() else {
        return Ok(Vec::new());
    };
    let pages: Vec<PageIndex> = match page {
        Some(p) => vec![p],
        None => (0..doc.page_count()).collect(),
    };
    let bindings = doc.bindings();
    let extras = extras::of(doc);
    let mut out = Vec::new();
    for page_index in pages {
        // The LRU page already ran `FORM_OnAfterLoadPage` (`page_lru.rs`), and listing is a
        // read path, so no `ScratchPage` here.
        let page = doc.page(page_index)?;
        out.extend(list_on_page(
            bindings,
            form,
            page,
            page_index,
            Some(&extras),
        ));
    }
    fix_button_values(&mut out, &extras);
    Ok(out)
}

/// A checkbox / radio field's value is the export value of its widget that is on, or `Off`
/// (what `FPDFAnnot_GetFormFieldValue` reports). PDFium decodes that export value — a name —
/// as PDFDocEncoding, so a Hangul one (`여`, every UI-made `선택 N`) comes back as mojibake;
/// the export value read with lopdf ([`extras::WidgetExtra::export`]) replaces it. A field
/// with a widget lopdf could not see (an encrypted file, a widget on a page not listed) keeps
/// PDFium's value.
fn fix_button_values(fields: &mut [FormField], extras: &Extras) {
    let mut groups: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (i, f) in fields.iter().enumerate() {
        if matches!(f.field_type, FieldType::Checkbox | FieldType::Radio) {
            groups.entry(f.name.clone()).or_default().push(i);
        }
    }
    for members in groups.into_values() {
        let exports: Option<Vec<String>> = members
            .iter()
            .map(|&i| {
                let f = &fields[i];
                extras.get(&f.name, f.rect)?.export.clone()
            })
            .collect();
        let Some(exports) = exports else {
            continue;
        };
        let on = members
            .iter()
            .zip(&exports)
            .find(|(&i, _)| fields[i].checked == Some(true))
            .map(|(_, e)| e.clone());
        let value = match on {
            Some(export) => export,
            // None on here; PDFium saying otherwise means the button that is on is on a page
            // this listing does not cover — its (possibly garbled) value is the best there is.
            None if members
                .iter()
                .all(|&i| matches!(fields[i].value.as_deref(), None | Some("") | Some("Off"))) =>
            {
                "Off".to_string()
            }
            None => continue,
        };
        for &i in &members {
            fields[i].value = Some(value.clone());
        }
    }
}

/// The widgets of one loaded page (a page of the open document or of a scratch copy).
fn list_on_page(
    bindings: &'static dyn PdfiumLibraryBindings,
    form: FPDF_FORMHANDLE,
    page: &PdfPage<'_>,
    page_index: PageIndex,
    extras: Option<&Extras>,
) -> Vec<FormField> {
    let mut out = Vec::new();
    for i in 0..raw::annot::count(bindings, page) {
        // A bad `/Annots` slot (null, dangling) is skipped, not fatal for the page.
        let Some(a) = raw::annot::slot(bindings, page, i) else {
            continue;
        };
        if a.subtype() != consts::FPDF_ANNOT_WIDGET {
            continue;
        }
        if let Some(field) = read_field(&a, form, page_index, i as u32, extras) {
            out.push(field);
        }
    }
    out
}

fn read_field(
    a: &AnnotRef<'_>,
    form: FPDF_FORMHANDLE,
    page: PageIndex,
    index: u32,
    extras: Option<&Extras>,
) -> Option<FormField> {
    let raw_type = a.form_field_type(form);
    if raw_type < 0 {
        return None;
    }
    let field_type = field_type_of(raw_type);
    let flags = a.form_field_flags(form);
    let options = match field_type {
        FieldType::Combo | FieldType::List => {
            let n = a.option_count(form);
            Some(
                (0..n)
                    .map(|i| FormOption {
                        label: a.option_label(form, i).unwrap_or_default(),
                        selected: a.is_option_selected(form, i),
                    })
                    .collect(),
            )
        }
        _ => None,
    };
    let checked = match field_type {
        FieldType::Checkbox | FieldType::Radio => Some(a.is_checked(form)),
        _ => None,
    };
    let name = a.form_field_name(form).unwrap_or_default();
    let rect = a.rect().unwrap_or(Rect::ZERO);
    // v0.3: PDFium has no `/MaxLen` reader; lopdf has (`extras`).
    let max_len = match field_type {
        FieldType::Text => extras
            .and_then(|e| e.get(&name, rect))
            .and_then(|x| x.max_len),
        _ => None,
    };
    Some(FormField {
        page,
        index,
        name,
        field_type,
        rect,
        value: a.form_field_value(form),
        checked,
        options,
        read_only: flags & consts::FPDF_FORMFLAG_READONLY != 0,
        required: flags & consts::FPDF_FORMFLAG_REQUIRED != 0,
        multiline: match field_type {
            FieldType::Text => Some(flags & consts::FPDF_FORMFLAG_TEXT_MULTILINE != 0),
            _ => None,
        },
        comb: match field_type {
            FieldType::Text => Some(flags & consts::FPDF_FORMFLAG_TEXT_COMB != 0),
            _ => None,
        },
        max_len,
        font_size_pt: a.form_font_size(form),
    })
}

/// The form handle of a document whose fields may be written, or why not.
fn writable_form(doc: &OpenDoc<'_>) -> Result<FPDF_FORMHANDLE, EngineError> {
    let Some(form) = doc.form_handle() else {
        return Err(EngineError::new(
            ErrorCode::Unsupported,
            "this document has no AcroForm",
        ));
    };
    if doc.xfa {
        return Err(EngineError::new(
            ErrorCode::Unsupported,
            "XFA forms are read-only",
        ));
    }
    if !doc.permissions.fill_forms {
        return Err(EngineError::new(
            ErrorCode::PermissionDenied,
            "this document's permission bits forbid filling forms",
        ));
    }
    Ok(form)
}

/// `set_form_field_value`. One call = one `registry::mutate` = one undo step.
pub fn set_value(
    doc: &mut OpenDoc<'_>,
    page_index: PageIndex,
    index: u32,
    value: &FieldValue,
) -> Result<WriteOutcome, EngineError> {
    let form = writable_form(doc)?;
    let bindings = doc.bindings();
    let scratch = ScratchPage::open_with_form(doc, page_index)?;
    let mut outcome = write_on_page(bindings, form, &scratch.page, page_index, index, value)?;
    drop(scratch);
    doc.annots.remove(&page_index);
    // A button that is now on reports its export value decoded like `list` does.
    if outcome.field.checked == Some(true) {
        let f = &outcome.field;
        if let Some(export) = extras::of(doc)
            .get(&f.name, f.rect)
            .and_then(|x| x.export.clone())
        {
            outcome.field.value = Some(export);
        }
    }
    Ok(outcome)
}

/// The write itself, on a page opened with `FORM_OnAfterLoadPage` — of the open document or of
/// a scratch copy ([`write_fields`]).
fn write_on_page(
    bindings: &'static dyn PdfiumLibraryBindings,
    form: FPDF_FORMHANDLE,
    page: &PdfPage<'_>,
    page_index: PageIndex,
    index: u32,
    value: &FieldValue,
) -> Result<WriteOutcome, EngineError> {
    let a = raw::annot::get(bindings, page, index as usize)?;
    if a.subtype() != consts::FPDF_ANNOT_WIDGET {
        return Err(
            EngineError::not_found(format!("no form field at index {index}")).with_page(page_index),
        );
    }
    let raw_type = a.form_field_type(form);
    let field_type = field_type_of(raw_type);
    let flags = a.form_field_flags(form);
    if flags & consts::FPDF_FORMFLAG_READONLY != 0 {
        return Err(EngineError::new(
            ErrorCode::PermissionDenied,
            format!(
                "field '{}' is read-only",
                a.form_field_name(form).unwrap_or_default()
            ),
        ));
    }
    let previous = a.form_field_value(form);
    let rect = a.rect().unwrap_or(Rect::ZERO);
    let centre = ((rect.l + rect.r) / 2.0, (rect.b + rect.t) / 2.0);

    let method = match (field_type, value) {
        // A combo box that takes no typing ignores typed text (PDFium keeps its value): the
        // text picks the option with that label instead.
        (FieldType::Combo, FieldValue::Text { text }) if flags & FF_CHOICE_EDIT == 0 => {
            if previous.as_deref().unwrap_or("") != text.as_str() {
                let option = (0..a.option_count(form))
                    .find(|&i| a.option_label(form, i).as_deref() == Some(text.as_str()));
                match option {
                    Some(i) => set_selected(bindings, form, page, &a, &[i as u32])?,
                    None if text.is_empty() => return Err(EngineError::new(
                        ErrorCode::Unsupported,
                        "PDFium cannot empty a combo box that takes no typing (clear_choice does)",
                    )),
                    None => {
                        return Err(EngineError::invalid(format!(
                            "'{text}' is not one of the combo box's choices"
                        )))
                    }
                }
            }
            TextEntryMethod::NotText
        }
        (FieldType::Text | FieldType::Combo, FieldValue::Text { text }) => set_text(
            bindings,
            form,
            page,
            &a,
            centre,
            text,
            previous.as_deref().unwrap_or("").chars().count(),
        ),
        (FieldType::Checkbox | FieldType::Radio, FieldValue::Checked { checked }) => {
            set_checked(bindings, form, page, &a, centre, *checked)?;
            TextEntryMethod::NotText
        }
        (FieldType::Combo | FieldType::List, FieldValue::Selected { selected }) => {
            set_selected(bindings, form, page, &a, selected)?;
            TextEntryMethod::NotText
        }
        (FieldType::Signature, _) => {
            return Err(EngineError::new(
                ErrorCode::Unsupported,
                "signature fields are read-only",
            ))
        }
        (FieldType::Button, _) => {
            return Err(EngineError::new(
                ErrorCode::Unsupported,
                "push buttons have no value; their JavaScript / URI actions are inert",
            ))
        }
        (field_type, _) => {
            return Err(EngineError::invalid(format!(
                "value shape does not match a {field_type:?} field"
            )))
        }
    };

    // Commits `/V` *and* writes the appearance stream the form layer just built.
    raw::form::force_to_kill_focus(bindings, form);
    let field = read_field(&a, form, page_index, index, None)
        .ok_or_else(|| EngineError::not_found(format!("form field {index} disappeared")))?;
    Ok(WriteOutcome {
        field,
        previous,
        method,
    })
}

fn set_text(
    bindings: &'static dyn pdfium_render::prelude::PdfiumLibraryBindings,
    form: FPDF_FORMHANDLE,
    page: &pdfium_render::prelude::PdfPage<'_>,
    a: &AnnotRef<'_>,
    centre: (f32, f32),
    text: &str,
    previous_len: usize,
) -> TextEntryMethod {
    // Path 1 — the `WORKPLAN.md` §6 probe. `FORM_ForceToKillFocus` is part of the probe, not
    // an afterthought: `FPDFAnnot_GetFormFieldValue` reads the committed `/V`, and the form
    // layer only commits on kill-focus, so reading before it always reports the old value.
    raw::form::set_focused_annot(bindings, form, a.handle());
    raw::form::select_all_text(bindings, form, page);
    raw::form::replace_selection(bindings, form, page, text);
    raw::form::force_to_kill_focus(bindings, form);
    if accepted(a.form_field_value(form).as_deref(), text) {
        return TextEntryMethod::ReplaceSelection;
    }

    // Path 2 — the spike-verified fallback. A click focuses the widget even when
    // `FORM_OnLButtonDown` reports false (it does for widgets at the page edge).
    raw::form::click(bindings, form, page, centre.0, centre.1);
    raw::form::select_all_text(bindings, form, page);
    // Select-all + one backspace empties the field; the rest are no-ops if it worked.
    raw::form::backspace(bindings, form, page, previous_len.max(1));
    raw::form::type_text(bindings, form, page, text);
    TextEntryMethod::CharLoop
}

/// Did the field take the value?
///
/// A `/MaxLen` (and every comb field has one) silently truncates the input, so "the field now
/// holds a non-empty prefix of what we asked for" is the only correct success test — an exact
/// comparison would report a working path as broken.
fn accepted(value: Option<&str>, wanted: &str) -> bool {
    let value = value.unwrap_or("");
    if wanted.is_empty() {
        value.is_empty()
    } else {
        !value.is_empty() && wanted.starts_with(value)
    }
}

/// Checkbox / radio: PDFium has no "set state" call, only a click, so the state is read back
/// and the click repeated once if the widget did not land where the caller asked.
fn set_checked(
    bindings: &'static dyn pdfium_render::prelude::PdfiumLibraryBindings,
    form: FPDF_FORMHANDLE,
    page: &pdfium_render::prelude::PdfPage<'_>,
    a: &AnnotRef<'_>,
    centre: (f32, f32),
    want: bool,
) -> Result<(), EngineError> {
    if a.is_checked(form) == want {
        return Ok(());
    }
    raw::form::click(bindings, form, page, centre.0, centre.1);
    if a.is_checked(form) == want {
        return Ok(());
    }
    raw::form::click(bindings, form, page, centre.0, centre.1);
    if a.is_checked(form) == want {
        return Ok(());
    }
    // A radio button in a group cannot be switched off by clicking it: `clear_radio` does it
    // with a lopdf rewrite instead.
    Err(EngineError::new(
        ErrorCode::Unsupported,
        if want {
            "the widget did not accept the click"
        } else {
            "a radio button cannot be cleared by a click"
        },
    ))
}

fn set_selected(
    bindings: &'static dyn pdfium_render::prelude::PdfiumLibraryBindings,
    form: FPDF_FORMHANDLE,
    page: &pdfium_render::prelude::PdfPage<'_>,
    a: &AnnotRef<'_>,
    selected: &[u32],
) -> Result<(), EngineError> {
    let count = a.option_count(form);
    if let Some(&bad) = selected.iter().find(|&&i| i as usize >= count) {
        return Err(EngineError::invalid(format!(
            "option {bad} is out of range (the field has {count})"
        )));
    }
    raw::form::set_focused_annot(bindings, form, a.handle());
    for i in 0..count {
        let want = selected.contains(&(i as u32));
        if raw::form::is_index_selected(bindings, form, page, i) != want {
            raw::form::set_index_selected(bindings, form, page, i, want);
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------
// v0.3: writes that may need a radio group cleared — reset to /DV, data import, 값 지우기
// ---------------------------------------------------------------------------------------

/// One field write of a batch.
#[derive(Debug, Clone)]
pub enum FieldWrite {
    Set {
        page: PageIndex,
        index: u32,
        value: FieldValue,
    },
    /// Switch every button of this widget's radio group off (`/V /Off`, `/AS /Off`).
    ClearRadio { page: PageIndex, index: u32 },
    /// Empty this widget's combo box (`/V ()`). PDFium cannot deselect a combo box
    /// (`FORM_SetIndexSelected(.., false)` is refused) nor type into one without the Edit flag,
    /// so the field is made editable for a moment, emptied by the form layer — which draws the
    /// empty appearance — and its `/Ff` restored and `/V` removed afterwards.
    ClearChoice { page: PageIndex, index: u32 },
}

/// The write that makes combo box `field` show `want` (`/DV`, an imported value), or `None`
/// when it already does or cannot: a value that is not a choice's label goes in as text only
/// when the combo box takes typing (`editable`); emptying one that does not is a
/// [`FieldWrite::ClearChoice`], possible only when the document can be rewritten
/// (`can_rewrite`, [`can_clear_radios`]).
pub(crate) fn combo_write(
    field: &FormField,
    want: &str,
    editable: bool,
    can_rewrite: bool,
) -> Option<FieldWrite> {
    let (page, index) = (field.page, field.index);
    let set = |value: FieldValue| Some(FieldWrite::Set { page, index, value });
    let current = field.value.as_deref().unwrap_or("");
    let options = field.options.as_deref().unwrap_or(&[]);
    if want.is_empty() {
        if current.is_empty() && !options.iter().any(|o| o.selected) {
            return None;
        }
        if editable {
            return set(FieldValue::Text {
                text: String::new(),
            });
        }
        return can_rewrite.then_some(FieldWrite::ClearChoice { page, index });
    }
    if current == want {
        return None;
    }
    match options.iter().position(|o| o.label == want) {
        Some(i) if options[i].selected => None,
        Some(i) => set(FieldValue::Selected {
            selected: vec![i as u32],
        }),
        None if editable => set(FieldValue::Text {
            text: want.to_string(),
        }),
        None => None,
    }
}

/// Whether the combo box at `(page, index)` takes typed text (`/Ff` Edit).
pub(crate) fn combo_editable(doc: &mut OpenDoc<'_>, page: PageIndex, index: u32) -> bool {
    let Some(form) = doc.form_handle() else {
        return false;
    };
    let bindings = doc.bindings();
    let Ok(page) = doc.page(page) else {
        return false;
    };
    raw::annot::get(bindings, page, index as usize)
        .is_ok_and(|a| a.form_field_flags(form) & FF_CHOICE_EDIT != 0)
}

/// Applies `writes` as **one** undo step `label`, returning how many were applied.
///
/// Without a [`FieldWrite::ClearRadio`] or [`FieldWrite::ClearChoice`] this is one
/// `registry::mutate` through the form-fill environment, like `set_form_field_value`. With one
/// it is a `registry::mutate_bytes`: the combo boxes to empty are made editable with lopdf, the
/// sets (and those combo boxes emptied) run through the form-fill environment of a scratch
/// copy, which is saved (appearance streams included) and then rewritten with lopdf to switch
/// the groups off and give the combo boxes back their flags — refused on an encrypted document,
/// which lopdf cannot write. A write that fails on its own (a read-only field, a value that is
/// not a choice) is skipped, not fatal, and not counted.
pub fn write_fields(
    st: &mut EngineState<'_>,
    doc_id: &str,
    label: &'static str,
    writes: Vec<FieldWrite>,
) -> Result<usize, EngineError> {
    if writes.is_empty() {
        return Ok(0);
    }
    writable_form(st.doc(doc_id)?)?;
    let pages: Vec<PageIndex> = writes
        .iter()
        .map(|w| match w {
            FieldWrite::Set { page, .. }
            | FieldWrite::ClearRadio { page, .. }
            | FieldWrite::ClearChoice { page, .. } => *page,
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let clears: Vec<(PageIndex, u32)> = writes
        .iter()
        .filter_map(|w| match w {
            FieldWrite::ClearRadio { page, index } => Some((*page, *index)),
            _ => None,
        })
        .collect();
    let choices: Vec<(PageIndex, u32)> = writes
        .iter()
        .filter_map(|w| match w {
            FieldWrite::ClearChoice { page, index } => Some((*page, *index)),
            _ => None,
        })
        .collect();
    let sets: Vec<(PageIndex, u32, FieldValue)> = writes
        .into_iter()
        .filter_map(|w| match w {
            FieldWrite::Set { page, index, value } => Some((page, index, value)),
            _ => None,
        })
        .collect();
    if clears.is_empty() && choices.is_empty() {
        let opts = MutateOpts::new(label, ChangeReason::Edit).pages(pages);
        return registry::mutate(st, doc_id, opts, |doc| {
            let mut done = 0usize;
            for (page, index, value) in &sets {
                if set_value(doc, *page, *index, value).is_ok() {
                    done += 1;
                }
            }
            Ok(done)
        });
    }
    crate::engine::structure::refuse_encrypted(st, doc_id)?;
    let pdfium = st.pdfium;
    let bindings = raw::bindings(pdfium);
    let opts = MutateOpts::new(label, ChangeReason::Edit).all_pages();
    let empty = FieldValue::Text {
        text: String::new(),
    };
    let cleared = clears.len();
    let mut done = 0usize;
    let counted = &mut done;
    registry::mutate_bytes(st, doc_id, opts, move |bytes, doc| {
        let (bytes, flags) = choices_editable(bytes, &choices)?;
        let scratch = pdfium
            .load_pdf_from_byte_vec(bytes, doc.password.as_deref())
            .map_err(|e| EngineError::pdfium("reload for form writes", e))?;
        let form = scratch.form().map(|f| f.raw_handle()).ok_or_else(|| {
            EngineError::new(ErrorCode::Unsupported, "this document has no AcroForm")
        })?;
        let mut by_page: BTreeMap<PageIndex, Vec<(u32, &FieldValue)>> = BTreeMap::new();
        for (page, index, value) in &sets {
            by_page.entry(*page).or_default().push((*index, value));
        }
        for (page, index) in &choices {
            by_page.entry(*page).or_default().push((*index, &empty));
        }
        for (page_index, list) in by_page {
            let page = scratch
                .pages()
                .get(page_index as PdfPageIndex)
                .map_err(|e| EngineError::pdfium("load page", e))?;
            raw::form::on_after_load_page(bindings, &page, form);
            for (index, value) in list {
                if write_on_page(bindings, form, &page, page_index, index, value).is_ok() {
                    *counted += 1;
                }
            }
            raw::form::force_to_kill_focus(bindings, form);
            raw::form::on_before_close_page(bindings, &page, form);
        }
        let written =
            raw::save::save_as_copy(bindings, &scratch, raw::save::SaveFlags::NoIncremental)?;
        drop(scratch);
        let written = clear_radios_in(&written, &clears)?;
        restore_choices(&written, &flags)
    })?;
    // A radio group is switched off by the rewrite itself; an emptied combo box was counted
    // by its form-layer write above.
    Ok(done + cleared)
}

/// The first half of [`FieldWrite::ClearChoice`]: each widget's field gets the Edit flag, so the
/// form layer can empty it. Returns the rewritten bytes and, per widget, the field's own `/Ff`
/// before (`None`: it had none) for [`restore_choices`].
#[allow(clippy::type_complexity)]
fn choices_editable(
    bytes: &[u8],
    widgets: &[(PageIndex, u32)],
) -> Result<(Vec<u8>, Vec<((PageIndex, u32), Option<lopdf::Object>)>), EngineError> {
    use lopdf::Object;
    if widgets.is_empty() {
        return Ok((bytes.to_vec(), Vec::new()));
    }
    let mut doc = crate::engine::structure::load(bytes)?;
    let pages = crate::engine::structure::page_ids(&doc);
    let mut before = Vec::new();
    for &(page, index) in widgets {
        let widget = author::widget_id(&doc, &pages, page, index)?;
        let field = author::field_of(&doc, widget);
        let own = doc
            .get_dictionary(field)
            .ok()
            .and_then(|d| d.get(b"Ff").ok().cloned());
        let effective = inherited_ff(&doc, field);
        if let Ok(dict) = doc.get_dictionary_mut(field) {
            dict.set("Ff", Object::Integer(effective | FF_CHOICE_EDIT as i64));
        }
        before.push(((page, index), own));
    }
    Ok((crate::engine::structure::write(doc)?, before))
}

/// A field's `/Ff`, inherited from its ancestors when it has none.
fn inherited_ff(doc: &lopdf::Document, field: lopdf::ObjectId) -> i64 {
    let mut current = field;
    for _ in 0..32 {
        let Ok(dict) = doc.get_dictionary(current) else {
            break;
        };
        if let Ok(ff) = dict.get(b"Ff").and_then(lopdf::Object::as_i64) {
            return ff;
        }
        match dict.get(b"Parent").and_then(lopdf::Object::as_reference) {
            Ok(parent) => current = parent,
            Err(_) => break,
        }
    }
    0
}

/// The second half of [`FieldWrite::ClearChoice`]: each field gets its own `/Ff` back, an empty
/// `/V` and no `/I`; the empty appearance the form layer drew stays.
fn restore_choices(
    bytes: &[u8],
    flags: &[((PageIndex, u32), Option<lopdf::Object>)],
) -> Result<Vec<u8>, EngineError> {
    if flags.is_empty() {
        return Ok(bytes.to_vec());
    }
    use lopdf::Object;
    let mut doc = crate::engine::structure::load(bytes)?;
    let pages = crate::engine::structure::page_ids(&doc);
    for ((page, index), own) in flags {
        let widget = author::widget_id(&doc, &pages, *page, *index)?;
        let field = author::field_of(&doc, widget);
        if let Ok(dict) = doc.get_dictionary_mut(field) {
            match own {
                Some(ff) => dict.set("Ff", ff.clone()),
                None => {
                    dict.remove(b"Ff");
                }
            }
            // An empty /V, not none: without /V PDFium reports (and draws) a choice field's /DV.
            dict.set("V", Object::string_literal(""));
            dict.remove(b"I");
        }
    }
    crate::engine::structure::write(doc)
}

/// The lopdf half of [`FieldWrite::ClearRadio`]: for each `(page, annotation index)` widget,
/// its field (the nearest ancestor with `/FT`) gets `/V /Off` and every widget under that field
/// `/AS /Off`.
fn clear_radios_in(bytes: &[u8], widgets: &[(PageIndex, u32)]) -> Result<Vec<u8>, EngineError> {
    use lopdf::Object;
    let mut doc = crate::engine::structure::load(bytes)?;
    let pages = crate::engine::structure::page_ids(&doc);
    for &(page, index) in widgets {
        let widget = author::widget_id(&doc, &pages, page, index)?;
        let field = author::field_of(&doc, widget);
        if let Ok(dict) = doc.get_dictionary_mut(field) {
            dict.set("V", Object::Name(b"Off".to_vec()));
        }
        let mut stack = vec![field];
        let mut seen = BTreeSet::new();
        while let Some(id) = stack.pop() {
            if !seen.insert(id) {
                continue;
            }
            let kids: Vec<lopdf::ObjectId> = doc
                .get_dictionary(id)
                .ok()
                .and_then(|d| d.get(b"Kids").ok())
                .and_then(|k| doc.dereference(k).ok())
                .and_then(|(_, k)| k.as_array().ok().cloned())
                .unwrap_or_default()
                .iter()
                .filter_map(|o| o.as_reference().ok())
                .collect();
            if let Ok(dict) = doc.get_dictionary_mut(id) {
                if dict.get(b"Subtype").and_then(Object::as_name).ok() == Some(b"Widget") {
                    dict.set("AS", Object::Name(b"Off".to_vec()));
                }
            }
            stack.extend(kids);
        }
    }
    crate::engine::structure::write(doc)
}

/// `set_form_field_value { checked: false }` on a radio button that is on (v0.3 F2): the group
/// is switched off. One undo step `undo.formFill`.
pub fn clear_radio(
    st: &mut EngineState<'_>,
    doc_id: &str,
    page: PageIndex,
    index: u32,
) -> Result<WriteOutcome, EngineError> {
    let previous = field_at(st, doc_id, page, index)?.value;
    write_fields(
        st,
        doc_id,
        "undo.formFill",
        vec![FieldWrite::ClearRadio { page, index }],
    )?;
    let field = field_at(st, doc_id, page, index)?;
    Ok(WriteOutcome {
        field,
        previous,
        method: TextEntryMethod::NotText,
    })
}

/// `set_form_field_value { text: "" }` (or `{ selected: [] }`) on a combo box that has a value
/// (v0.3 F2, 값 지우기): the combo box is emptied — through the form layer when it takes typing,
/// else by [`FieldWrite::ClearChoice`], which an encrypted document refuses. One undo step
/// `undo.formFill`.
pub fn clear_choice(
    st: &mut EngineState<'_>,
    doc_id: &str,
    page: PageIndex,
    index: u32,
) -> Result<WriteOutcome, EngineError> {
    let previous = field_at(st, doc_id, page, index)?.value;
    if combo_editable(st.doc_mut(doc_id)?, page, index) {
        let empty = FieldValue::Text {
            text: String::new(),
        };
        return registry::mutate(
            st,
            doc_id,
            MutateOpts::new("undo.formFill", ChangeReason::Edit).page(page),
            |doc| set_value(doc, page, index, &empty),
        );
    }
    write_fields(
        st,
        doc_id,
        "undo.formFill",
        vec![FieldWrite::ClearChoice { page, index }],
    )?;
    let field = field_at(st, doc_id, page, index)?;
    Ok(WriteOutcome {
        field,
        previous,
        method: TextEntryMethod::NotText,
    })
}

/// The field at `(page, index)`, or `notFound`.
pub fn field_at(
    st: &mut EngineState<'_>,
    doc_id: &str,
    page: PageIndex,
    index: u32,
) -> Result<FormField, EngineError> {
    let doc = st.doc_mut(doc_id)?;
    if page >= doc.page_count() {
        return Err(EngineError::not_found(format!("page {page}")).with_page(page));
    }
    list(doc, Some(page))?
        .into_iter()
        .find(|f| f.index == index)
        .ok_or_else(|| {
            EngineError::not_found(format!("no form field at index {index}")).with_page(page)
        })
}

/// `reset_form` — every writable field back to its default value (`/DV`), one undo step
/// `undo.formReset`:
///
/// * text fields get their `/DV` string, or become empty; a combo box selects the choice its
///   `/DV` names (typed in when it takes typing and `/DV` is no choice), or is emptied
///   ([`combo_write`]);
/// * a list box selects the options its `/DV` names, or none;
/// * a checkbox is on only when its `/DV` names its on-state;
/// * a radio group turns on the button whose export value its `/DV` names, and is switched
///   off entirely when it has no `/DV` (or `/DV /Off`) — except where switching a group off (a
///   lopdf rewrite) is refused or would invalidate signatures, where radio groups are left as
///   they are ([`can_clear_radios`]).
///
/// Returns how many fields were changed.
pub fn reset_form(st: &mut EngineState<'_>, doc_id: &str) -> Result<usize, EngineError> {
    let writes = reset_writes(st, doc_id)?;
    write_fields(st, doc_id, "undo.formReset", writes)
}

/// Whether [`FieldWrite::ClearRadio`] can be applied: it is a lopdf rewrite, which a document
/// whose permissions forbid changes refuses. Where it cannot, reset and import leave radio groups
/// as they are and still apply every other write (the pre-v0.3 behaviour) instead of failing the
/// whole batch.
///
/// v0.3 integration (F2 × S1): nor on a signed document that still saves incrementally
/// (`pristine`) — the rewrite would force a full save and invalidate its signatures, where
/// PDFium's own writes (every other reset / import write) keep them.
pub(crate) fn can_clear_radios(st: &EngineState<'_>, doc_id: &str) -> bool {
    crate::engine::structure::refuse_encrypted(st, doc_id).is_ok()
        && st.doc(doc_id).is_ok_and(|d| !d.pristine)
}

fn reset_writes(st: &mut EngineState<'_>, doc_id: &str) -> Result<Vec<FieldWrite>, EngineError> {
    let can_clear = can_clear_radios(st, doc_id);
    let doc = st.doc_mut(doc_id)?;
    let extras = extras::of(doc);
    let fields = list(doc, None)?;
    let editable: BTreeSet<(PageIndex, u32)> = fields
        .iter()
        .filter(|f| f.field_type == FieldType::Combo)
        .map(|f| (f.page, f.index))
        .filter(|&(page, index)| combo_editable(doc, page, index))
        .collect();
    // The on-state of each radio widget, decoded like `/DV` (both read with lopdf).
    let exports: BTreeMap<(PageIndex, u32), String> = fields
        .iter()
        .filter(|f| f.field_type == FieldType::Radio)
        .filter_map(|f| {
            let on = extras.get(&f.name, f.rect)?.on_state.clone()?;
            Some(((f.page, f.index), on))
        })
        .collect();
    let mut writes = Vec::new();
    let mut groups_done: BTreeSet<String> = BTreeSet::new();
    for field in &fields {
        if field.read_only {
            continue;
        }
        let default = extras
            .get(&field.name, field.rect)
            .and_then(|x| x.default.clone());
        let set = |value: FieldValue| FieldWrite::Set {
            page: field.page,
            index: field.index,
            value,
        };
        match field.field_type {
            FieldType::Text | FieldType::Combo => {
                let want = match default {
                    Some(DefaultValue::Text(t)) | Some(DefaultValue::State(t)) => t,
                    Some(DefaultValue::Many(v)) => v.into_iter().next().unwrap_or_default(),
                    None => String::new(),
                };
                if field.field_type == FieldType::Combo {
                    writes.extend(combo_write(
                        field,
                        &want,
                        editable.contains(&(field.page, field.index)),
                        can_clear,
                    ));
                } else if field.value.as_deref().unwrap_or("") != want {
                    writes.push(set(FieldValue::Text { text: want }));
                }
            }
            FieldType::List => {
                let wanted: Vec<String> = match default {
                    Some(DefaultValue::Text(t)) | Some(DefaultValue::State(t)) => vec![t],
                    Some(DefaultValue::Many(v)) => v,
                    None => Vec::new(),
                };
                let options = field.options.clone().unwrap_or_default();
                let selected: Vec<u32> = options
                    .iter()
                    .enumerate()
                    .filter(|(_, o)| wanted.contains(&o.label))
                    .map(|(i, _)| i as u32)
                    .collect();
                let current: Vec<u32> = options
                    .iter()
                    .enumerate()
                    .filter(|(_, o)| o.selected)
                    .map(|(i, _)| i as u32)
                    .collect();
                if current != selected {
                    writes.push(set(FieldValue::Selected { selected }));
                }
            }
            FieldType::Checkbox => {
                let want = matches!(&default, Some(DefaultValue::State(s)) if s != "Off");
                if field.checked.unwrap_or(false) != want {
                    writes.push(set(FieldValue::Checked { checked: want }));
                }
            }
            FieldType::Radio => {
                if !groups_done.insert(field.name.clone()) {
                    continue;
                }
                let group: Vec<&FormField> = fields
                    .iter()
                    .filter(|f| f.field_type == FieldType::Radio && f.name == field.name)
                    .collect();
                let target = match &default {
                    Some(DefaultValue::State(s)) if s != "Off" => group
                        .iter()
                        .find(|f| exports.get(&(f.page, f.index)) == Some(s))
                        .copied(),
                    _ => None,
                };
                match target {
                    Some(t) if !t.checked.unwrap_or(false) => writes.push(FieldWrite::Set {
                        page: t.page,
                        index: t.index,
                        value: FieldValue::Checked { checked: true },
                    }),
                    Some(_) => {}
                    None if !can_clear => {}
                    None => {
                        if let Some(on) = group.iter().find(|f| f.checked.unwrap_or(false)) {
                            writes.push(FieldWrite::ClearRadio {
                                page: on.page,
                                index: on.index,
                            });
                        }
                    }
                }
            }
            _ => {}
        }
    }
    Ok(writes)
}

/// `reset_form` result helper for the command: the fresh `DocInfo`.
pub fn reset_form_info(st: &mut EngineState<'_>, doc_id: &str) -> Result<DocInfo, EngineError> {
    reset_form(st, doc_id)?;
    Ok(st.doc(doc_id)?.info())
}
