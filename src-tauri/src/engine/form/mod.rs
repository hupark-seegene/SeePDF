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

use crate::engine::annot::ScratchPage;
use crate::engine::raw::{self, annot::AnnotRef, consts};
use crate::engine::registry::OpenDoc;
use crate::ipc::types::{FieldType, FieldValue, FormField, FormOption, PageIndex, Rect};
use crate::ipc::{EngineError, ErrorCode};
use pdfium_render::prelude::FPDF_FORMHANDLE;
use std::os::raw::c_int;

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
    let mut out = Vec::new();
    for page_index in pages {
        // The LRU page already ran `FORM_OnAfterLoadPage` (`page_lru.rs`), and listing is a
        // read path, so no `ScratchPage` here.
        let page = doc.page(page_index)?;
        for i in 0..raw::annot::count(bindings, page) {
            // A bad `/Annots` slot (null, dangling) is skipped, not fatal for the page.
            let Some(a) = raw::annot::slot(bindings, page, i) else {
                continue;
            };
            if a.subtype() != consts::FPDF_ANNOT_WIDGET {
                continue;
            }
            if let Some(field) = read_field(&a, form, page_index, i as u32) {
                out.push(field);
            }
        }
    }
    Ok(out)
}

fn read_field(
    a: &AnnotRef<'_>,
    form: FPDF_FORMHANDLE,
    page: PageIndex,
    index: u32,
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
    Some(FormField {
        page,
        index,
        name: a.form_field_name(form).unwrap_or_default(),
        field_type,
        rect: a.rect().unwrap_or(Rect::ZERO),
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
        // PDFium exposes no `/MaxLen` reader; the comb cell count is not available either.
        max_len: None,
        font_size_pt: a.form_font_size(form),
    })
}

/// `set_form_field_value`. One call = one `registry::mutate` = one undo step.
pub fn set_value(
    doc: &mut OpenDoc<'_>,
    page_index: PageIndex,
    index: u32,
    value: &FieldValue,
) -> Result<WriteOutcome, EngineError> {
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
    let bindings = doc.bindings();
    let scratch = ScratchPage::open_with_form(doc, page_index)?;
    let page = &scratch.page;
    let a = raw::annot::get(bindings, page, index as usize)?;
    if a.subtype() != consts::FPDF_ANNOT_WIDGET {
        return Err(
            EngineError::not_found(format!("no form field at index {index}")).with_page(page_index),
        );
    }
    let raw_type = a.form_field_type(form);
    let field_type = field_type_of(raw_type);
    if a.form_field_flags(form) & consts::FPDF_FORMFLAG_READONLY != 0 {
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
    let field = read_field(&a, form, page_index, index)
        .ok_or_else(|| EngineError::not_found(format!("form field {index} disappeared")))?;
    drop(a);
    drop(scratch);
    doc.annots.remove(&page_index);
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
    // A radio button in a group cannot be switched off by clicking it: another button of the
    // group has to be switched on instead.
    Err(EngineError::new(
        ErrorCode::Unsupported,
        if want {
            "the widget did not accept the click"
        } else {
            "a radio button cannot be cleared; select another button of its group"
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

/// `reset_form` (P1) — clears every writable field.
///
/// PDFium has no `FORM_Reset`, so each field is cleared the same way the user would: text and
/// choice fields are emptied, checkboxes are unticked. **Radio groups keep their selection**
/// (a radio cannot be cleared by clicking it), and this resets to *empty*, not to the `/DV`
/// default value, which PDFium does not expose.
pub fn reset(doc: &mut OpenDoc<'_>) -> Result<usize, EngineError> {
    let fields = list(doc, None)?;
    let mut cleared = 0usize;
    for field in fields {
        if field.read_only {
            continue;
        }
        let value = match field.field_type {
            FieldType::Text | FieldType::Combo => {
                if field.value.as_deref().unwrap_or("").is_empty() {
                    continue;
                }
                FieldValue::Text {
                    text: String::new(),
                }
            }
            FieldType::Checkbox => {
                if !field.checked.unwrap_or(false) {
                    continue;
                }
                FieldValue::Checked { checked: false }
            }
            FieldType::List => FieldValue::Selected { selected: vec![] },
            _ => continue,
        };
        if set_value(doc, field.page, field.index, &value).is_ok() {
            cleared += 1;
        }
    }
    Ok(cleared)
}
