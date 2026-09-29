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
        // The Hangul export value exactly as typed (PDFium's own reading garbles it).
        let text = std::fs::read_to_string(&path).unwrap();
        let expected = match format {
            FormDataFormat::Csv => "성별,여",
            FormDataFormat::Xfdf => "<field name=\"성별\"><value>여</value>",
        };
        assert!(text.contains(expected), "{ext}: {text}");

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
        assert_eq!(
            cleared.3.as_deref().unwrap_or(""),
            "",
            "{ext}: no /DV: the combo box is emptied (so the import below really sets it)"
        );

        let imported = with_state({
            let (id, path) = (id.clone(), path.display().to_string());
            move |st| data::import(st, &id, &path, None)
        })
        .expect("import_form_data");
        assert!(imported.unknown.is_empty(), "{:?}", imported.unknown);
        assert_eq!(snapshot(&id), filled, "{ext}: import restores every value");
        assert_eq!(
            imported.fields, 4,
            "{ext}: text, checkbox, one radio button, combo — each write changed something"
        );
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

// ---------------------------------------------------------------------------------------
// Verification round 1
// ---------------------------------------------------------------------------------------

fn radios(doc_id: &str, name: &str) -> Vec<FormField> {
    fields(doc_id)
        .into_iter()
        .filter(|f| f.name == name)
        .collect()
}

fn states(doc_id: &str, name: &str) -> Vec<Option<bool>> {
    radios(doc_id, name).iter().map(|f| f.checked).collect()
}

fn import_csv(doc_id: &str, file: &str, csv: &str) -> seepdf_lib::ipc::types::FormDataResult {
    let path = out_dir().join(file);
    std::fs::write(&path, csv).unwrap();
    let (id, path) = (doc_id.to_string(), path.display().to_string());
    with_state(move |st| data::import(st, &id, &path, None)).expect("import_form_data")
}

/// 모든 필드 지우기 and 양식 데이터 가져오기 on an **encrypted** form whose radio group is on: a
/// group cannot be switched off there (a lopdf rewrite), so it is left as it is — and every
/// other field is still reset / imported instead of the whole batch failing.
#[test]
fn forms_reset_and_import_on_encrypted_form_keep_radios() {
    let doc = open("tracemonkey.pdf");
    let r = |x: f32| Rect::new(x, 620.0, x + 14.0, 634.0);
    create(
        &doc.doc_id,
        spec(
            NewFieldType::Text,
            "name",
            Rect::new(72.0, 700.0, 250.0, 720.0),
            &[],
        ),
    );
    create(
        &doc.doc_id,
        spec(NewFieldType::Radio, "grp", r(72.0), &["a"]),
    );
    create(
        &doc.doc_id,
        spec(NewFieldType::Radio, "grp", r(100.0), &["b"]),
    );
    let plain = with_state({
        let id = doc.doc_id.clone();
        move |st| seepdf_lib::engine::save::serialize(st, &id)
    })
    .unwrap();
    let encrypted =
        seepdf_lib::engine::security::encrypt_bytes(&plain, "", "owner", Default::default())
            .expect("encrypt");
    let info = with_state(move |st| registry::open(st, None, encrypted, None)).unwrap();
    assert!(info.encrypted);
    let id = info.doc_id.clone();
    let list = fields(&id);
    set(
        &id,
        by_name(&list, "name"),
        FieldValue::Text {
            text: "hello".into(),
        },
    );
    set(
        &id,
        &radios(&id, "grp")[0],
        FieldValue::Checked { checked: true },
    );

    let changed = with_state({
        let id = id.clone();
        move |st| form::reset_form(st, &id)
    })
    .expect("reset_form works on an encrypted form");
    assert_eq!(changed, 1, "the text field only");
    assert_eq!(
        by_name(&fields(&id), "name").value.as_deref().unwrap_or(""),
        ""
    );
    assert_eq!(
        states(&id, "grp"),
        [Some(true), Some(false)],
        "the radio is kept"
    );

    // Import: the radio set to Off is skipped, the text still lands.
    let result = import_csv(&id, "encrypted.csv", "name,value\nname,again\ngrp,Off\n");
    assert_eq!(result.fields, 1);
    assert_eq!(
        by_name(&fields(&id), "name").value.as_deref(),
        Some("again")
    );
    assert_eq!(states(&id, "grp"), [Some(true), Some(false)]);
    // …and picking the other button by its export value works (PDFium's export values).
    import_csv(&id, "encrypted-b.csv", "name,value\ngrp,b\n");
    assert_eq!(states(&id, "grp"), [Some(false), Some(true)]);

    // 값 지우기 itself still says why it cannot switch the group off.
    let on = radios(&id, "grp")[1].clone();
    let err = with_state({
        let id = id.clone();
        move |st| form::clear_radio(st, &id, on.page, on.index)
    })
    .expect_err("clear_radio on an encrypted document");
    assert_eq!(err.code, seepdf_lib::ipc::ErrorCode::Unsupported);
}

/// Hangul export values — including the `선택N` every UI-made radio gets — are listed,
/// exported and matched on import exactly as typed.
#[test]
fn forms_hangul_export_values_are_exact() {
    let doc = open("tracemonkey.pdf");
    let id = doc.doc_id.clone();
    let r = |x: f32| Rect::new(x, 620.0, x + 14.0, 634.0);
    create(&id, spec(NewFieldType::Radio, "성별", r(72.0), &["남"]));
    create(&id, spec(NewFieldType::Radio, "성별", r(100.0), &["여"]));
    create(&id, spec(NewFieldType::Radio, "선택", r(140.0), &[]));
    create(&id, spec(NewFieldType::Radio, "선택", r(170.0), &[]));

    // A hand-written CSV picks 여.
    let result = import_csv(&id, "hangul-radio.csv", "name,value\n성별,여\n선택,선택2\n");
    assert_eq!(result.fields, 2, "{result:?}");
    assert!(result.unknown.is_empty());
    assert_eq!(states(&id, "성별"), [Some(false), Some(true)]);
    assert_eq!(states(&id, "선택"), [Some(false), Some(true)]);
    let listed = fields(&id);
    assert!(listed
        .iter()
        .filter(|f| f.name == "성별")
        .all(|f| f.value.as_deref() == Some("여")));
    assert_eq!(by_name(&listed, "선택").value.as_deref(), Some("선택2"));

    // set_form_field_value answers with the export value too.
    let picked = set(
        &id,
        &radios(&id, "성별")[0],
        FieldValue::Checked { checked: true },
    );
    assert_eq!(picked.value.as_deref(), Some("남"));

    let path = out_dir().join("hangul-radio-export.csv");
    with_state({
        let (id, path) = (id.clone(), path.display().to_string());
        move |st| data::export(st, &id, FormDataFormat::Csv, &path)
    })
    .unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert_eq!(text, "\u{feff}name,value\r\n성별,남\r\n선택,선택2\r\n");

    // …and survive save + reopen.
    let re = save_and_reopen(&id);
    assert_eq!(
        by_name(&fields(&re.doc_id), "선택").value.as_deref(),
        Some("선택2")
    );
}

/// 필드 속성: renaming a radio group after another radio group puts its buttons into that
/// group (the way the UI joins buttons drawn apart); a clashing export value is renumbered,
/// and the merged group is one group — in SeePDF and after save + reopen. One undo step.
#[test]
fn forms_radio_rename_joins_group() {
    let doc = open("tracemonkey.pdf");
    let id = doc.doc_id.clone();
    let r = |x: f32| Rect::new(x, 620.0, x + 14.0, 634.0);
    create(
        &id,
        spec(NewFieldType::Radio, "라디오1", r(72.0), &["선택 1"]),
    );
    create(
        &id,
        spec(NewFieldType::Radio, "라디오2", r(100.0), &["선택 1"]),
    );
    // A button drawn into a group with a clashing export value is renumbered as well.
    create(
        &id,
        spec(NewFieldType::Radio, "라디오2", r(130.0), &["선택 1"]),
    );
    let second = radios(&id, "라디오2")[0].clone();
    let merged = with_state({
        let id = id.clone();
        move |st| {
            author::update_field(
                st,
                &id,
                second.page,
                second.index,
                &FormFieldPatch {
                    name: Some("라디오1".into()),
                    ..Default::default()
                },
            )
        }
    })
    .expect("a radio group renamed after another joins it");
    assert_eq!(
        merged.field.as_ref().map(|f| f.name.as_str()),
        Some("라디오1")
    );
    assert!(radios(&id, "라디오2").is_empty());
    let group = radios(&id, "라디오1");
    assert_eq!(group.len(), 3);

    // Three distinct export values: picking each turns only that one on.
    let mut values = Vec::new();
    for i in 0..3 {
        let picked = set(
            &id,
            &radios(&id, "라디오1")[i],
            FieldValue::Checked { checked: true },
        );
        values.push(picked.value.clone().unwrap());
        let on: Vec<bool> = states(&id, "라디오1")
            .iter()
            .map(|c| *c == Some(true))
            .collect();
        assert_eq!(on.iter().filter(|b| **b).count(), 1, "{on:?}");
        assert!(on[i]);
    }
    assert_eq!(values, ["선택 1", "선택 2", "선택 3"]);

    let re = save_and_reopen(&id);
    let reopened: Vec<FormField> = radios(&re.doc_id, "라디오1");
    assert_eq!(reopened.len(), 3);
    assert_eq!(
        reopened.iter().map(|f| f.checked).collect::<Vec<_>>(),
        [Some(false), Some(false), Some(true)]
    );
    set(
        &re.doc_id,
        &reopened[0],
        FieldValue::Checked { checked: true },
    );
    assert_eq!(
        states(&re.doc_id, "라디오1"),
        [Some(true), Some(false), Some(false)]
    );

    // A text field still cannot take a radio group's name, nor the reverse.
    create(
        &id,
        spec(
            NewFieldType::Text,
            "메모",
            Rect::new(72.0, 700.0, 250.0, 720.0),
            &[],
        ),
    );
    let memo = by_name(&fields(&id), "메모").clone();
    let refused = with_state({
        let id = id.clone();
        move |st| {
            author::update_field(
                st,
                &id,
                memo.page,
                memo.index,
                &FormFieldPatch {
                    name: Some("라디오1".into()),
                    ..Default::default()
                },
            )
        }
    });
    assert!(refused.is_err());

    // One undo step brings the two groups back (the picks are undone first, the refused
    // rename and the 메모 field aside).
    let label = |id: &str| with_doc(id, |d| Ok(d.info().undo_label)).unwrap();
    undo(&id); // 메모
    while label(&id).as_deref() != Some("undo.formFieldEdit") {
        undo(&id);
    }
    undo(&id); // the merge
    assert_eq!(radios(&id, "라디오2").len(), 2);
    assert_eq!(radios(&id, "라디오1").len(), 1);
}

// ---------------------------------------------------------------------------------------
// Verification round 2
// ---------------------------------------------------------------------------------------

/// The page rendered at 1 px per point: (width, height, RGBA pixels).
fn render(doc_id: &str, page: u16) -> (usize, usize, Vec<u8>) {
    let doc_id = doc_id.to_string();
    let b = with_state(move |st| {
        seepdf_lib::engine::render::tiles::render_raw_buffer(st, &doc_id, page, 1.0, None)
    })
    .expect("render_page_raw");
    let w = u32::from_le_bytes(b[8..12].try_into().unwrap()) as usize;
    let h = u32::from_le_bytes(b[12..16].try_into().unwrap()) as usize;
    (w, h, b[32..].to_vec())
}

/// The bounding box `(x0, y0, x1, y1)` of the pixels that differ between two renders.
fn changed_box(
    a: &(usize, usize, Vec<u8>),
    b: &(usize, usize, Vec<u8>),
) -> (usize, usize, usize, usize) {
    assert_eq!((a.0, a.1), (b.0, b.1));
    let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0, 0);
    for y in 0..a.1 {
        for x in 0..a.0 {
            let at = (y * a.0 + x) * 4;
            let d = (0..3)
                .map(|c| (a.2[at + c] as i32 - b.2[at + c] as i32).abs())
                .max()
                .unwrap();
            if d > 48 {
                (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
            }
        }
    }
    assert!(x0 <= x1, "the fill changed no pixel");
    (x0, y0, x1, y1)
}

/// Round 2, F1: a field drawn on a `/Rotate 90` page to look horizontal in the view (tall in
/// user space) gets `/MK /R 90`, and its filled value reads horizontally, at a readable size —
/// not sideways and tiny across the box's short side. Same for a combo box.
#[test]
fn forms_field_on_rotated_page_reads_horizontally() {
    let doc = open("rotation.pdf");
    let page = doc
        .info
        .pages
        .iter()
        .find(|p| p.rotation == 90)
        .expect("rotation.pdf has a /Rotate 90 page");
    let (pg, c) = (page.index, page.crop);
    // What AuthorSurface sends for a 200 × 30 box drawn in the view: 30 × 200 in user space.
    let mut text = spec(
        NewFieldType::Text,
        "rot",
        Rect::new(c.l + 100.0, c.b + 100.0, c.l + 130.0, c.b + 300.0),
        &[],
    );
    text.page = pg;
    create(&doc.doc_id, text);
    let mut combo = spec(
        NewFieldType::Combo,
        "rotcombo",
        Rect::new(c.l + 200.0, c.b + 100.0, c.l + 220.0, c.b + 250.0),
        &["Seoul", "Busan"],
    );
    combo.page = pg;
    create(&doc.doc_id, combo);

    // The widget carries the page's rotation.
    let bytes = with_state({
        let id = doc.doc_id.clone();
        move |st| seepdf_lib::engine::save::serialize(st, &id)
    })
    .unwrap();
    let parsed = lopdf::Document::load_mem(&bytes).unwrap();
    let rotations: Vec<i64> = parsed
        .objects
        .values()
        .filter_map(|o| o.as_dict().ok())
        .filter(|d| d.get(b"Subtype").and_then(|s| s.as_name()).ok() == Some(b"Widget"))
        .map(|d| {
            d.get(b"MK")
                .and_then(|m| m.as_dict())
                .and_then(|m| m.get(b"R"))
                .and_then(|r| r.as_i64())
                .unwrap_or(0)
        })
        .collect();
    assert_eq!(rotations, [90, 90], "/MK /R follows the page's /Rotate");

    let all = fields(&doc.doc_id);
    let before = render(&doc.doc_id, pg);
    set(
        &doc.doc_id,
        by_name(&all, "rot"),
        FieldValue::Text {
            text: "HELLO WORLD".into(),
        },
    );
    let after = render(&doc.doc_id, pg);
    let (x0, y0, x1, y1) = changed_box(&before, &after);
    let (w, h) = (x1 - x0 + 1, y1 - y0 + 1);
    assert!(
        w > 2 * h,
        "the value runs horizontally in the view: {w} × {h} px"
    );
    assert!(h >= 8, "at a readable size: {w} × {h} px");

    let before = render(&doc.doc_id, pg);
    set(
        &doc.doc_id,
        by_name(&all, "rotcombo"),
        FieldValue::Selected { selected: vec![1] },
    );
    let after = render(&doc.doc_id, pg);
    let (x0, y0, x1, y1) = changed_box(&before, &after);
    let (w, h) = (x1 - x0 + 1, y1 - y0 + 1);
    assert!(w > h, "the combo's value runs horizontally: {w} × {h} px");
}

/// The dark pixels (any channel under 128) inside `rect` (PDF points, page without rotation,
/// rendered at 1 px per point), left of `right_margin` points from its right edge.
fn ink_in(doc_id: &str, page: u16, crop: Rect, rect: Rect, right_margin: f32) -> usize {
    let (w, _, px) = render(doc_id, page);
    let x0 = (rect.l - crop.l + 2.0) as usize;
    let x1 = (rect.r - crop.l - right_margin) as usize;
    let y0 = (crop.t - rect.t + 2.0) as usize;
    let y1 = (crop.t - rect.b - 2.0) as usize;
    let mut n = 0;
    for y in y0..y1 {
        for x in x0..x1 {
            let at = (y * w + x) * 4;
            if px[at..at + 3].iter().any(|&c| c < 128) {
                n += 1;
            }
        }
    }
    n
}

/// Round 2, F1/F2: a combo box (every one `create_form_field` makes, most real ones) takes no
/// typing, so PDFium ignores a text write to it. Import selects the choice whose label is the
/// value; reset selects the `/DV` choice or empties it; 값 지우기 empties it — and the value is
/// gone from the page too. Only writes that change something are counted.
#[test]
fn forms_combo_import_reset_and_clear() {
    let doc = open("tracemonkey.pdf");
    let id = doc.doc_id.clone();
    let ko_rect = Rect::new(72.0, 580.0, 192.0, 600.0);
    create(
        &id,
        spec(
            NewFieldType::Combo,
            "지역",
            ko_rect,
            &["서울", "부산", "대구"],
        ),
    );
    create(
        &id,
        spec(
            NewFieldType::Combo,
            "city",
            Rect::new(72.0, 540.0, 192.0, 560.0),
            &["Seoul", "Busan", "Daegu"],
        ),
    );
    let values = |id: &str| -> Vec<(String, Option<String>)> {
        fields(id)
            .into_iter()
            .map(|f| (f.name, f.value.filter(|v| !v.is_empty())))
            .collect()
    };
    let crop = doc.info.pages[0].crop;
    // the dropdown button takes the right end of the box: look left of it
    let blank_ink = ink_in(&id, 0, crop, ko_rect, 24.0);

    let imported = import_csv(&id, "combo-in.csv", "name,value\n지역,대구\ncity,Daegu\n");
    assert_eq!(imported.fields, 2);
    assert_eq!(
        values(&id),
        [
            ("지역".to_string(), Some("대구".to_string())),
            ("city".to_string(), Some("Daegu".to_string()))
        ],
        "import selects the choices"
    );
    let again = import_csv(&id, "combo-in.csv", "name,value\n지역,대구\ncity,Daegu\n");
    assert_eq!(again.fields, 0, "nothing changed, nothing counted");
    let odd = import_csv(&id, "combo-odd.csv", "name,value\ncity,Tokyo\n");
    assert_eq!(odd.fields, 0, "a value that is no choice is not typed in");
    assert_eq!(values(&id)[1].1.as_deref(), Some("Daegu"));
    assert!(
        ink_in(&id, 0, crop, ko_rect, 24.0) > blank_ink,
        "the chosen value is drawn"
    );

    // Reset without /DV empties both — value and appearance.
    let changed = with_state({
        let id = id.clone();
        move |st| form::reset_form(st, &id)
    })
    .expect("reset_form");
    assert_eq!(changed, 2);
    assert_eq!(
        values(&id),
        [("지역".to_string(), None), ("city".to_string(), None)],
        "no /DV: emptied"
    );
    assert!(
        ink_in(&id, 0, crop, ko_rect, 24.0) <= blank_ink,
        "the emptied combo box shows no value"
    );
    let flags = |id: &str| -> Vec<i64> {
        let bytes = with_state({
            let id = id.to_string();
            move |st| seepdf_lib::engine::save::serialize(st, &id)
        })
        .unwrap();
        let parsed = lopdf::Document::load_mem(&bytes).unwrap();
        parsed
            .objects
            .values()
            .filter_map(|o| o.as_dict().ok())
            .filter(|d| d.get(b"FT").and_then(|t| t.as_name()).ok() == Some(b"Ch"))
            .map(|d| d.get(b"Ff").and_then(|f| f.as_i64()).unwrap_or(0))
            .collect()
    };
    assert_eq!(
        flags(&id),
        [1 << 17, 1 << 17],
        "the combo boxes are not left editable"
    );
    undo(&id);
    assert_eq!(
        values(&id)[0].1.as_deref(),
        Some("대구"),
        "reset is one undo step"
    );

    // Reset with /DV selects the /DV choice (the verifier's probe: /V differs from /DV).
    let bytes = with_state({
        let id = id.clone();
        move |st| seepdf_lib::engine::save::serialize(st, &id)
    })
    .unwrap();
    let mut parsed = lopdf::Document::load_mem(&bytes).unwrap();
    let ids: Vec<lopdf::ObjectId> = parsed.objects.keys().copied().collect();
    for oid in ids {
        let Ok(d) = parsed.get_dictionary_mut(oid) else {
            continue;
        };
        let name = d
            .get(b"T")
            .ok()
            .and_then(|t| lopdf::decode_text_string(t).ok());
        let (dv, v) = match name.as_deref() {
            Some("지역") => ("서울", "부산"),
            Some("city") => ("Seoul", "Busan"),
            _ => continue,
        };
        d.set("DV", seepdf_lib::engine::save::pdf_text_string(dv));
        d.set("V", seepdf_lib::engine::save::pdf_text_string(v));
    }
    let mut out = Vec::new();
    parsed.save_to(&mut out).unwrap();
    let info = with_state(move |st| registry::open(st, None, out, None)).unwrap();
    let dv = info.doc_id.clone();
    assert_eq!(
        values(&dv),
        [
            ("지역".to_string(), Some("부산".to_string())),
            ("city".to_string(), Some("Busan".to_string()))
        ]
    );
    let changed = with_state({
        let dv = dv.clone();
        move |st| form::reset_form(st, &dv)
    })
    .expect("reset_form");
    assert_eq!(changed, 2);
    assert_eq!(
        values(&dv),
        [
            ("지역".to_string(), Some("서울".to_string())),
            ("city".to_string(), Some("Seoul".to_string()))
        ],
        "reset goes to /DV"
    );
    let again = with_state({
        let dv = dv.clone();
        move |st| form::reset_form(st, &dv)
    })
    .unwrap();
    assert_eq!(again, 0, "already at /DV: nothing counted");

    // 값 지우기 (what `set_form_field_value { text: "" }` routes a combo box to).
    let combo = by_name(&fields(&dv), "지역").clone();
    let cleared = with_state({
        let dv = dv.clone();
        move |st| form::clear_choice(st, &dv, combo.page, combo.index)
    })
    .expect("clear_choice");
    assert_eq!(cleared.field.value.as_deref().unwrap_or(""), "");
    assert_eq!(cleared.previous.as_deref(), Some("서울"));
    assert!(
        ink_in(&dv, 0, crop, ko_rect, 24.0) <= blank_ink,
        "the emptied combo box shows no value (not its /DV either)"
    );
    let reopened = save_and_reopen(&dv);
    assert_eq!(
        values(&reopened.doc_id),
        [
            ("지역".to_string(), None),
            ("city".to_string(), Some("Seoul".to_string()))
        ],
        "only that combo box is emptied, and it stays empty after a save"
    );
    undo(&dv);
    assert_eq!(values(&dv)[0].1.as_deref(), Some("서울"), "one undo step");
}
