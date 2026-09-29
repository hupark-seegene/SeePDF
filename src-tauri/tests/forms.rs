//! v0.3 F1 / F2 — form authoring, data exchange, reset to `/DV`, radio clearing, `/MaxLen`,
//! and 양식 평면화, against real PDFium (every rewrite reopened and read back by PDFium).

mod common;
use common::*;

use seepdf_lib::engine::form::{self, author, data, flatten};
use seepdf_lib::engine::registry::{self, MutateOpts};
use seepdf_lib::ipc::types::{
    ChangeReason, FieldType, FieldValue, FormDataFormat, FormField, FormFieldPatch, FormFieldSpec,
    NewFieldType, Rect,
};
use std::path::PathBuf;

fn out_dir() -> PathBuf {
    let dir = fixture("out").join("v03-forms");
    std::fs::create_dir_all(&dir).expect("create fixtures/out/v03-forms");
    dir
}

fn fields(doc_id: &str) -> Vec<FormField> {
    with_doc(doc_id, |d| form::list(d, None)).expect("list")
}

fn spec(kind: NewFieldType, name: &str, rect: Rect, options: &[&str]) -> FormFieldSpec {
    FormFieldSpec {
        page: 0,
        rect,
        field_type: kind,
        name: name.into(),
        options: options.iter().map(|s| s.to_string()).collect(),
        max_len: None,
        required: false,
        multiline: false,
    }
}

fn create(doc_id: &str, spec: FormFieldSpec) -> seepdf_lib::ipc::types::FormEditResult {
    let doc_id = doc_id.to_string();
    with_state(move |st| author::create_field(st, &doc_id, &spec)).expect("create_form_field")
}

fn set(doc_id: &str, f: &FormField, value: FieldValue) -> FormField {
    let doc_id = doc_id.to_string();
    let (page, index) = (f.page, f.index);
    with_state(move |st| {
        registry::mutate(
            st,
            &doc_id,
            MutateOpts::new("undo.formFill", ChangeReason::Edit).page(page),
            |d| form::set_value(d, page, index, &value),
        )
    })
    .expect("set_form_field_value")
    .field
}

fn by_name<'a>(fields: &'a [FormField], name: &str) -> &'a FormField {
    fields.iter().find(|f| f.name == name).unwrap_or_else(|| {
        panic!(
            "no field '{name}' in {:?}",
            fields.iter().map(|f| &f.name).collect::<Vec<_>>()
        )
    })
}

fn save_and_reopen(doc_id: &str) -> TestDoc {
    let doc_id = doc_id.to_string();
    let bytes =
        with_state(move |st| seepdf_lib::engine::save::serialize(st, &doc_id)).expect("serialize");
    let info = with_state(move |st| registry::open(st, None, bytes, None)).expect("reopen");
    let doc_id = info.doc_id.clone();
    TestDoc { info, doc_id }
}

fn undo(doc_id: &str) {
    let doc_id = doc_id.to_string();
    with_state(move |st| registry::undo(st, &doc_id, false)).expect("undo");
}

/// Four fields, one of each kind, on a document that had no form: reopened, PDFium lists them
/// with their types and `set_form_field_value` fills every one.
fn authored() -> TestDoc {
    let doc = open("tracemonkey.pdf");
    assert!(!doc.info.has_form);
    let r = |x: f32, y: f32, w: f32, h: f32| Rect::new(x, y, x + w, y + h);
    let mut text = spec(NewFieldType::Text, "성명", r(72.0, 700.0, 180.0, 20.0), &[]);
    text.max_len = Some(10);
    text.required = true;
    let made = create(&doc.doc_id, text);
    assert!(made.info.has_form, "the document has a form now");
    assert_eq!(
        made.field.as_ref().map(|f| f.field_type),
        Some(FieldType::Text)
    );
    create(
        &doc.doc_id,
        spec(
            NewFieldType::Checkbox,
            "동의",
            r(72.0, 660.0, 14.0, 14.0),
            &[],
        ),
    );
    create(
        &doc.doc_id,
        spec(
            NewFieldType::Radio,
            "성별",
            r(72.0, 620.0, 14.0, 14.0),
            &["남"],
        ),
    );
    let second = create(
        &doc.doc_id,
        spec(
            NewFieldType::Radio,
            "성별",
            r(100.0, 620.0, 14.0, 14.0),
            &["여"],
        ),
    );
    assert_eq!(
        second.fields.iter().filter(|f| f.name == "성별").count(),
        2,
        "the second button joined the group"
    );
    create(
        &doc.doc_id,
        spec(
            NewFieldType::Combo,
            "지역",
            r(72.0, 580.0, 120.0, 20.0),
            &["서울", "부산", "대구"],
        ),
    );
    create(
        &doc.doc_id,
        spec(
            NewFieldType::Signature,
            "서명",
            r(300.0, 580.0, 150.0, 40.0),
            &[],
        ),
    );
    doc
}

#[test]
fn forms_author_each_type_reopen_and_fill() {
    let doc = authored();
    let reopened = save_and_reopen(&doc.doc_id);
    let list = fields(&reopened.doc_id);
    assert_eq!(list.len(), 6);
    let text = by_name(&list, "성명");
    assert_eq!(text.field_type, FieldType::Text);
    assert!(text.required);
    assert_eq!(text.max_len, Some(10), "/MaxLen is reported (v0.3 F2)");
    assert_eq!(by_name(&list, "동의").field_type, FieldType::Checkbox);
    assert_eq!(by_name(&list, "성별").field_type, FieldType::Radio);
    let combo = by_name(&list, "지역");
    assert_eq!(combo.field_type, FieldType::Combo);
    let labels: Vec<&str> = combo
        .options
        .iter()
        .flatten()
        .map(|o| o.label.as_str())
        .collect();
    assert_eq!(labels, ["서울", "부산", "대구"]);
    assert_eq!(by_name(&list, "서명").field_type, FieldType::Signature);

    let id = reopened.doc_id.clone();
    let filled = set(
        &id,
        text,
        FieldValue::Text {
            text: "Hong Gildong".into(),
        },
    );
    assert_eq!(
        filled.value.as_deref(),
        Some("Hong Gildo"),
        "MaxLen 10 truncates"
    );
    let cb = set(
        &id,
        by_name(&list, "동의"),
        FieldValue::Checked { checked: true },
    );
    assert_eq!(cb.checked, Some(true));
    let radios: Vec<&FormField> = list.iter().filter(|f| f.name == "성별").collect();
    set(&id, radios[1], FieldValue::Checked { checked: true });
    let after = fields(&id);
    let states: Vec<bool> = after
        .iter()
        .filter(|f| f.name == "성별")
        .map(|f| f.checked == Some(true))
        .collect();
    assert_eq!(states, [false, true]);
    let combo = set(
        &id,
        by_name(&list, "지역"),
        FieldValue::Selected { selected: vec![1] },
    );
    assert_eq!(combo.value.as_deref(), Some("부산"));
}

#[test]
fn forms_update_and_delete_are_one_undo_step_each() {
    let doc = authored();
    let id = doc.doc_id.clone();
    let text = by_name(&fields(&id), "성명").clone();
    let patch = FormFieldPatch {
        name: Some("이름".into()),
        max_len: Some(0),
        required: Some(false),
        options: None,
    };
    let edited = with_state({
        let id = id.clone();
        move |st| author::update_field(st, &id, text.page, text.index, &patch)
    })
    .expect("update_form_field");
    let field = edited.field.expect("the edited field");
    assert_eq!(field.name, "이름");
    assert_eq!(field.max_len, None);
    assert!(!field.required);

    let taken = with_state({
        let id = id.clone();
        let f = field.clone();
        move |st| {
            author::update_field(
                st,
                &id,
                f.page,
                f.index,
                &FormFieldPatch {
                    name: Some("지역".into()),
                    ..Default::default()
                },
            )
        }
    });
    assert!(taken.is_err(), "a name another field has is refused");

    let before = fields(&id).len();
    let radios: Vec<FormField> = fields(&id)
        .into_iter()
        .filter(|f| f.name == "성별")
        .collect();
    let after = with_state({
        let id = id.clone();
        let r = radios[0].clone();
        move |st| author::delete_field(st, &id, r.page, r.index)
    })
    .expect("delete_form_field");
    assert_eq!(after.fields.len(), before - 1);
    assert_eq!(
        after.fields.iter().filter(|f| f.name == "성별").count(),
        1,
        "the group keeps its other button"
    );
    undo(&id);
    assert_eq!(fields(&id).len(), before, "undo brings the button back");

    let dup = with_state({
        let id = id.clone();
        move |st| {
            author::create_field(
                st,
                &id,
                &spec(
                    NewFieldType::Text,
                    "지역",
                    Rect::new(10.0, 10.0, 60.0, 30.0),
                    &[],
                ),
            )
        }
    });
    assert!(dup.is_err(), "a text field cannot take a used name");
}

#[test]
fn forms_xfdf_and_csv_round_trip() {
    let doc = authored();
    let id = doc.doc_id.clone();
    let list = fields(&id);
    set(
        &id,
        by_name(&list, "성명"),
        FieldValue::Text {
            text: "홍길동".into(),
        },
    );
    set(
        &id,
        by_name(&list, "동의"),
        FieldValue::Checked { checked: true },
    );
    let radios: Vec<FormField> = list.iter().filter(|f| f.name == "성별").cloned().collect();
    set(&id, &radios[1], FieldValue::Checked { checked: true });
    set(
        &id,
        by_name(&list, "지역"),
        FieldValue::Selected { selected: vec![2] },
    );
    let snapshot = |id: &str| {
        let fs = fields(id);
        (
            by_name(&fs, "성명").value.clone(),
            by_name(&fs, "동의").checked,
            fs.iter()
                .filter(|f| f.name == "성별")
                .map(|f| f.checked)
                .collect::<Vec<_>>(),
            by_name(&fs, "지역").value.clone(),
        )
    };
    let filled = snapshot(&id);
    assert_eq!(filled.0.as_deref(), Some("홍길동"));

    for (format, ext) in [(FormDataFormat::Xfdf, "xfdf"), (FormDataFormat::Csv, "csv")] {
        let path = out_dir().join(format!("values.{ext}"));
        let exported = with_state({
            let (id, path) = (id.clone(), path.display().to_string());
            move |st| data::export(st, &id, format, &path)
        })
        .expect("export_form_data");
        assert_eq!(
            exported.fields, 4,
            "text, checkbox, radio group, combo (the signature has no value)"
        );

        with_state({
            let id = id.clone();
            move |st| form::reset_form(st, &id)
        })
        .expect("reset_form");
        let cleared = snapshot(&id);
        assert_eq!(cleared.0.as_deref().unwrap_or(""), "");
        assert_eq!(cleared.1, Some(false));
        assert_eq!(
            cleared.2,
            [Some(false), Some(false)],
            "no /DV: the radio group is switched off"
        );

        let imported = with_state({
            let (id, path) = (id.clone(), path.display().to_string());
            move |st| data::import(st, &id, &path, None)
        })
        .expect("import_form_data");
        assert!(imported.unknown.is_empty(), "{:?}", imported.unknown);
        assert_eq!(snapshot(&id), filled, "{ext}: import restores every value");
    }

    // An unknown name is reported, not fatal.
    let path = out_dir().join("unknown.csv");
    std::fs::write(&path, "name,value\n없는필드,x\n성명,김철수\n").unwrap();
    let result = with_state({
        let (id, path) = (id.clone(), path.display().to_string());
        move |st| data::import(st, &id, &path, Some(FormDataFormat::Csv))
    })
    .unwrap();
    assert_eq!(result.unknown, ["없는필드"]);
    assert_eq!(
        by_name(&fields(&id), "성명").value.as_deref(),
        Some("김철수")
    );
}

/// F2: reset applies `/DV`; a radio button can be cleared; both one undo step.
#[test]
fn forms_reset_uses_dv_and_radio_clears() {
    let doc = authored();
    // Give the text field and the radio group a /DV (what a form designer would have set).
    let bytes = with_state({
        let id = doc.doc_id.clone();
        move |st| seepdf_lib::engine::save::serialize(st, &id)
    })
    .unwrap();
    let mut parsed = lopdf::Document::load_mem(&bytes).unwrap();
    let ids: Vec<lopdf::ObjectId> = parsed.objects.keys().copied().collect();
    for oid in ids {
        let Ok(dict) = parsed.get_dictionary_mut(oid) else {
            continue;
        };
        let name = dict
            .get(b"T")
            .ok()
            .and_then(|t| lopdf::decode_text_string(t).ok());
        match name.as_deref() {
            Some("성명") => dict.set("DV", seepdf_lib::engine::save::pdf_text_string("기본값")),
            Some("성별") => dict.set("DV", lopdf::Object::Name("여".as_bytes().to_vec())),
            _ => {}
        }
    }
    let mut out = Vec::new();
    parsed.save_to(&mut out).unwrap();
    std::fs::write(out_dir().join("dv-fixture.pdf"), &out).unwrap();
    let info = with_state(move |st| registry::open(st, None, out, None)).unwrap();
    let dv = TestDoc {
        doc_id: info.doc_id.clone(),
        info,
    };
    let id = dv.doc_id.clone();
    let list = fields(&id);
    set(
        &id,
        by_name(&list, "성명"),
        FieldValue::Text {
            text: "바뀐 값".into(),
        },
    );
    let radios: Vec<FormField> = list.iter().filter(|f| f.name == "성별").cloned().collect();
    set(&id, &radios[0], FieldValue::Checked { checked: true });

    with_state({
        let id = id.clone();
        move |st| form::reset_form(st, &id)
    })
    .expect("reset");
    let after = fields(&id);
    assert_eq!(
        by_name(&after, "성명").value.as_deref(),
        Some("기본값"),
        "reset applies /DV"
    );
    let states: Vec<Option<bool>> = after
        .iter()
        .filter(|f| f.name == "성별")
        .map(|f| f.checked)
        .collect();
    assert_eq!(
        states,
        [Some(false), Some(true)],
        "the /DV button of the group is on"
    );

    // 값 지우기 on the radio that is on: the whole group off, one undo step.
    let on = after
        .iter()
        .find(|f| f.name == "성별" && f.checked == Some(true))
        .unwrap()
        .clone();
    let cleared = with_state({
        let id = id.clone();
        move |st| form::clear_radio(st, &id, on.page, on.index)
    })
    .expect("clear_radio");
    assert_eq!(cleared.field.checked, Some(false));
    assert!(fields(&id)
        .iter()
        .filter(|f| f.name == "성별")
        .all(|f| f.checked == Some(false)));
    undo(&id);
    assert!(
        fields(&id)
            .iter()
            .any(|f| f.name == "성별" && f.checked == Some(true)),
        "undo turns it back on"
    );
}

/// F2: 양식 평면화 leaves no field (the values stay on the page) and undo brings them back.
#[test]
fn forms_flatten_in_place_and_undo() {
    let doc = authored();
    let id = doc.doc_id.clone();
    let list = fields(&id);
    set(
        &id,
        by_name(&list, "성명"),
        FieldValue::Text {
            text: "FLATTENED".into(),
        },
    );
    let before = list.len();
    let info = with_state({
        let id = id.clone();
        move |st| flatten::flatten_form(st, &id)
    })
    .expect("flatten_form");
    assert!(!info.has_form, "no AcroForm after flattening");
    assert!(fields(&id).is_empty(), "0 fields");
    let text = with_doc(&id, |d| {
        Ok(seepdf_lib::engine::text::layer::page_text(d, 0)?
            .text
            .clone())
    })
    .unwrap();
    assert!(
        text.contains("FLATTENED"),
        "the value is part of the page now"
    );
    undo(&id);
    assert_eq!(fields(&id).len(), before, "undo restores every field");

    let formless = open("tracemonkey.pdf");
    let refused = with_state({
        let id = formless.doc_id.clone();
        move |st| flatten::flatten_form(st, &id)
    });
    assert!(refused.is_err(), "nothing to flatten");
}
