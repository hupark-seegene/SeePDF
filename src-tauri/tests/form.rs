//! Stage 1 (a) — AcroForm filling.
//!
//! `160F-2019.pdf` is the fixture: 76 widgets on one page — 64 text fields, 10 push buttons,
//! 2 radio buttons and **no checkbox** (the spike corrected the task description here).
//!
//! `form_text_value_persists` is also the `WORKPLAN.md` §6 probe: it records which of the two
//! `FORM_*` paths actually wrote the value.

mod common;
use common::*;

use seepdf_lib::engine::form::{self, TextEntryMethod};
use seepdf_lib::engine::registry::{self, MutateOpts};
use seepdf_lib::engine::render::tiles;
use seepdf_lib::ipc::types::{ChangeReason, FieldType, FieldValue, FormField, Rect};

const RENDER_SCALE: f32 = 2.0;
const PIXEL_TOLERANCE: i32 = 24;

fn fields(doc_id: &str) -> Vec<FormField> {
    let doc_id = doc_id.to_string();
    with_doc(&doc_id.clone(), move |doc| form::list(doc, None)).expect("list form fields")
}

fn write(
    doc_id: &str,
    page: u16,
    index: u32,
    value: FieldValue,
) -> Result<form::WriteOutcome, seepdf_lib::ipc::EngineError> {
    let doc_id = doc_id.to_string();
    with_state(move |st| {
        registry::mutate(
            st,
            &doc_id,
            MutateOpts::new("undo.formFill", ChangeReason::Edit).page(page),
            |doc| form::set_value(doc, page, index, &value),
        )
    })
}

fn save_bytes(doc_id: &str) -> Vec<u8> {
    let doc_id = doc_id.to_string();
    with_state(move |st| {
        tiles::generate_appearances(st, &doc_id)?;
        Ok(st.doc(&doc_id)?.to_bytes()?.to_vec())
    })
    .expect("save to bytes")
}

fn reopen(bytes: Vec<u8>) -> TestDoc {
    let info = with_state(move |st| registry::open(st, None, bytes, None)).expect("reopen");
    let doc_id = info.doc_id.clone();
    TestDoc { info, doc_id }
}

fn render_rect(doc_id: &str, page: u16, rect: Rect) -> (u32, u32, Vec<u8>) {
    let doc_id = doc_id.to_string();
    let buffer = with_state(move |st| {
        tiles::render_raw_buffer(st, &doc_id, page, RENDER_SCALE, Some(rect))
    })
    .expect("render rect");
    let width = u32::from_le_bytes(buffer[8..12].try_into().unwrap());
    let height = u32::from_le_bytes(buffer[12..16].try_into().unwrap());
    (width, height, buffer[32..].to_vec())
}

fn changed_pixels(before: &(u32, u32, Vec<u8>), after: &(u32, u32, Vec<u8>)) -> usize {
    assert_eq!((before.0, before.1), (after.0, after.1));
    before
        .2
        .chunks_exact(4)
        .zip(after.2.chunks_exact(4))
        .filter(|(a, b)| (0..3).any(|i| (a[i] as i32 - b[i] as i32).abs() > PIXEL_TOLERANCE))
        .count()
}

fn out_dir() -> std::path::PathBuf {
    let dir = fixture("out").join("stage1a");
    std::fs::create_dir_all(&dir).expect("create fixtures/out/stage1a");
    dir
}

/// Enumeration: every widget, with its type, name, flags and rect.
#[test]
fn form_lists_every_widget() {
    let doc = open("160F-2019.pdf");
    let all = fields(&doc.doc_id);
    assert_eq!(all.len(), 76, "160F-2019.pdf has 76 widgets");
    let text = all
        .iter()
        .filter(|f| f.field_type == FieldType::Text)
        .count();
    let buttons = all
        .iter()
        .filter(|f| f.field_type == FieldType::Button)
        .count();
    let radios = all
        .iter()
        .filter(|f| f.field_type == FieldType::Radio)
        .count();
    assert_eq!((text, buttons, radios), (64, 10, 2), "field type census");
    assert!(
        all.iter().all(|f| !f.name.is_empty()),
        "every field has a fully qualified name"
    );
    assert!(
        all.iter().any(|f| f.comb == Some(true)),
        "the form has comb fields, so /Ff is being read"
    );

    // A document without a form answers with an empty list, not an error.
    let plain = open("tracemonkey.pdf");
    assert!(fields(&plain.doc_id).is_empty());
}

/// The `WORKPLAN.md` §6 probe **and** the DoD test: a value written through the form-fill
/// environment is visible in the render and survives save + reopen. `set_value` on the
/// high-level field would pass the first half of this test and fail the render.
#[test]
fn form_text_value_persists() {
    let doc = open("160F-2019.pdf");
    let target = fields(&doc.doc_id)
        .into_iter()
        .find(|f| {
            f.field_type == FieldType::Text
                && !f.read_only
                && f.comb != Some(true) // a comb field has a /MaxLen and truncates
                && f.rect.width() > 120.0
                && f.rect.height() > 10.0
        })
        .expect("a writable, non-comb text field");
    let before = render_rect(&doc.doc_id, target.page, target.rect);

    let text = "SeePDF 한글 입력 12345";
    let outcome = write(
        &doc.doc_id,
        target.page,
        target.index,
        FieldValue::Text {
            text: text.to_string(),
        },
    )
    .expect("write the text field");

    println!("PROBE form text entry method = {:?}", outcome.method);
    assert!(
        matches!(
            outcome.method,
            TextEntryMethod::ReplaceSelection | TextEntryMethod::CharLoop
        ),
        "one of the two documented paths must have run"
    );
    assert_eq!(outcome.field.value.as_deref(), Some(text));
    assert_eq!(outcome.previous.as_deref().unwrap_or(""), "");

    // Visible in the same session…
    let after = render_rect(&doc.doc_id, target.page, target.rect);
    assert!(
        changed_pixels(&before, &after) > 50,
        "the value must be drawn: the appearance stream was not regenerated"
    );

    // …and after save + reopen, which is where `set_value` fails.
    let bytes = save_bytes(&doc.doc_id);
    std::fs::write(out_dir().join("form-filled.pdf"), &bytes).expect("write pdf");
    let saved = reopen(bytes);
    let reread = fields(&saved.doc_id)
        .into_iter()
        .find(|f| f.name == target.name)
        .expect("the field is still there");
    assert_eq!(reread.value.as_deref(), Some(text), "value persisted");

    let reopened = render_rect(&saved.doc_id, target.page, target.rect);
    assert!(
        changed_pixels(&before, &reopened) > 50,
        "the reopened file must show the value (a saved /AP, not just /V)"
    );
}

/// Radio buttons: PDFium has no "set state" call, so the engine clicks the widget and reads
/// `FPDFAnnot_IsChecked` back. Clearing a radio is impossible by design and must be reported
/// rather than silently ignored.
#[test]
fn form_radio_toggle() {
    let doc = open("160F-2019.pdf");
    let radio = fields(&doc.doc_id)
        .into_iter()
        .find(|f| f.field_type == FieldType::Radio && !f.read_only)
        .expect("160F-2019.pdf has radio buttons");
    let before = render_rect(&doc.doc_id, radio.page, radio.rect);

    let outcome = write(
        &doc.doc_id,
        radio.page,
        radio.index,
        FieldValue::Checked { checked: true },
    )
    .expect("select the radio button");
    assert_eq!(outcome.field.checked, Some(true), "the radio is now on");

    let after = render_rect(&doc.doc_id, radio.page, radio.rect);
    let changed = changed_pixels(&before, &after);
    println!("radio toggle changed {changed} pixels");

    // It survives save + reopen.
    let bytes = save_bytes(&doc.doc_id);
    std::fs::write(out_dir().join("form-radio.pdf"), &bytes).expect("write pdf");
    let saved = reopen(bytes);
    let reread = fields(&saved.doc_id)
        .into_iter()
        .find(|f| f.name == radio.name && f.index == radio.index)
        .expect("the radio is still there");
    assert_eq!(reread.checked, Some(true), "the selection persisted");

    // Turning it back off is refused with a message the UI can explain.
    let err = write(
        &doc.doc_id,
        radio.page,
        radio.index,
        FieldValue::Checked { checked: false },
    )
    .expect_err("a radio button cannot be cleared by clicking it");
    assert_eq!(err.code, seepdf_lib::ipc::ErrorCode::Unsupported);
}

/// Wrong value shape, unknown index, push button and read-only field all produce a specific
/// error code rather than a silent no-op.
#[test]
fn form_rejects_bad_writes() {
    let doc = open("160F-2019.pdf");
    let all = fields(&doc.doc_id);
    let text = all
        .iter()
        .find(|f| f.field_type == FieldType::Text && !f.read_only)
        .expect("a text field");

    let err = write(
        &doc.doc_id,
        text.page,
        text.index,
        FieldValue::Checked { checked: true },
    )
    .expect_err("a checkbox value on a text field is invalidArgument");
    assert_eq!(err.code, seepdf_lib::ipc::ErrorCode::InvalidArgument);

    let err = write(
        &doc.doc_id,
        text.page,
        9_999,
        FieldValue::Text {
            text: "x".to_string(),
        },
    )
    .expect_err("an unknown index is notFound");
    assert_eq!(err.code, seepdf_lib::ipc::ErrorCode::NotFound);

    if let Some(button) = all.iter().find(|f| f.field_type == FieldType::Button) {
        let err = write(
            &doc.doc_id,
            button.page,
            button.index,
            FieldValue::Text {
                text: "x".to_string(),
            },
        )
        .expect_err("push buttons have no value");
        assert_eq!(err.code, seepdf_lib::ipc::ErrorCode::Unsupported);
    }

    // A document without a form.
    let plain = open("tracemonkey.pdf");
    let err = write(
        &plain.doc_id,
        0,
        0,
        FieldValue::Text {
            text: "x".to_string(),
        },
    )
    .expect_err("no AcroForm");
    assert_eq!(err.code, seepdf_lib::ipc::ErrorCode::Unsupported);
}

/// P1 `reset_form`: text fields are emptied. Radio groups keep their selection — PDFium
/// cannot clear one, and this is the documented gap.
#[test]
fn form_reset_clears_text_fields() {
    let doc = open("160F-2019.pdf");
    let target = fields(&doc.doc_id)
        .into_iter()
        .find(|f| f.field_type == FieldType::Text && !f.read_only && f.rect.width() > 80.0)
        .expect("a writable text field");
    write(
        &doc.doc_id,
        target.page,
        target.index,
        FieldValue::Text {
            text: "to be cleared".to_string(),
        },
    )
    .expect("fill");

    let cleared = {
        let doc_id = doc.doc_id.clone();
        with_state(move |st| {
            let pages: Vec<u16> = (0..st.doc(&doc_id)?.page_count()).collect();
            registry::mutate(
                st,
                &doc_id,
                MutateOpts::new("undo.formReset", ChangeReason::Edit).pages(pages),
                form::reset,
            )
        })
        .expect("reset")
    };
    assert!(cleared >= 1, "at least the field we filled was cleared");

    let after = fields(&doc.doc_id)
        .into_iter()
        .find(|f| f.name == target.name)
        .expect("the field is still there");
    assert_eq!(
        after.value.as_deref().unwrap_or(""),
        "",
        "the text field is empty again"
    );
}

/// The `WORKPLAN.md` §6 probe, step by step, so the finding in `STAGE1A_NOTES.md` names the
/// call that fails rather than just "the fast path did not work". Informational: it prints
/// what each `FORM_*` call returned and never fails, because either outcome is shippable.
#[test]
fn form_probe_replace_selection() {
    use seepdf_lib::engine::annot::ScratchPage;
    use seepdf_lib::engine::raw;

    let doc = open("160F-2019.pdf");
    let target = fields(&doc.doc_id)
        .into_iter()
        .find(|f| {
            f.field_type == FieldType::Text
                && !f.read_only
                && f.comb != Some(true)
                && f.rect.width() > 120.0
        })
        .expect("a writable, non-comb text field");

    let doc_id = doc.doc_id.clone();
    let index = target.index as usize;
    let report = with_doc(&doc_id, move |doc| {
        let bindings = doc.bindings();
        let form = doc.form_handle().expect("form handle");
        let scratch = ScratchPage::open_with_form(doc, 0)?;
        let a = raw::annot::get(bindings, &scratch.page, index)?;
        let focused = raw::form::set_focused_annot(bindings, form, a.handle());
        let after_focus = a.form_field_value(form);
        let selected = raw::form::select_all_text(bindings, form, &scratch.page);
        raw::form::replace_selection(bindings, form, &scratch.page, "PROBE");
        // The value is only committed on kill-focus; reading before it returns the old /V.
        let before_commit = a.form_field_value(form);
        raw::form::force_to_kill_focus(bindings, form);
        let after_replace = a.form_field_value(form);
        // Now the verified path, for comparison.
        let (down, up) = raw::form::click(bindings, form, &scratch.page, 
            (target.rect.l + target.rect.r) / 2.0,
            (target.rect.b + target.rect.t) / 2.0);
        let selected2 = raw::form::select_all_text(bindings, form, &scratch.page);
        raw::form::type_text(bindings, form, &scratch.page, "CHAR");
        raw::form::force_to_kill_focus(bindings, form);
        let after_chars = a.form_field_value(form);
        Ok(format!(
            "SetFocusedAnnot={focused} value_after_focus={after_focus:?} \
             SelectAllText(focus path)={selected} value_before_commit={before_commit:?} \
             value_after_ReplaceSelection={after_replace:?} \
             || click=({down},{up}) SelectAllText(click path)={selected2} \
             value_after_OnChar={after_chars:?}"
        ))
    })
    .expect("probe");
    println!("PROBE FORM_* {report}");
}
