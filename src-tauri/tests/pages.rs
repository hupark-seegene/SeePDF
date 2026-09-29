//! Stage 1 (b) — page operations, extract, split and merge (`IPC_CONTRACT.md` §7.3).
//!
//! The order assertions all read the page **text** back rather than trusting a page count:
//! `gen/500p.pdf` stamps every page with its own number, so "page 3 is now at index 1" is
//! something the test can actually see. A page count alone would pass for a move that shuffled
//! the wrong pages.

mod common;
use common::*;

use seepdf_lib::engine::pages;
use seepdf_lib::engine::registry;
use seepdf_lib::engine::text::layer;
use seepdf_lib::ipc::types::{
    BlankPageSize, MergeInput, MergeWarning, NamedPageSize, PageOp, PageSizePt, SplitMode,
};
use std::path::PathBuf;

fn out_dir() -> PathBuf {
    let dir = fixture("out").join("stage1b");
    std::fs::create_dir_all(&dir).expect("create fixtures/out/stage1b");
    dir
}

/// The number `gen/500p.pdf` stamps on a page, read back from its text.
///
/// Parsed out of the full text rather than taken from the first characters, because PDFium
/// orders extracted runs by the **rotated** layout: rotating a page moves its footer to the
/// front of the string.
fn page_number(doc_id: &str, page: u16) -> String {
    let text = page_text(doc_id, page);
    for (i, _) in text.char_indices() {
        if text[i..].starts_with("Page ") {
            let digits: String = text[i + 5..]
                .chars()
                .take_while(|c| c.is_ascii_digit())
                .collect();
            if !digits.is_empty() {
                return format!("Page {digits}");
            }
        }
    }
    String::new()
}

fn page_text(doc_id: &str, page: u16) -> String {
    let doc_id = doc_id.to_string();
    with_doc(&doc_id, move |doc| {
        Ok(layer::page_text(doc, page)?.text.clone())
    })
    .expect("page text")
}

/// The first characters of a page's extracted text, whitespace collapsed — enough to identify
/// which page is at an index in a document that has no page stamps.
fn page_tag(doc_id: &str, page: u16) -> String {
    let text = page_text(doc_id, page);
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(24)
        .collect()
}

fn ops(doc_id: &str, ops: Vec<PageOp>) -> seepdf_lib::ipc::types::DocInfo {
    let doc_id = doc_id.to_string();
    with_state(move |st| pages::apply_ops(st, &doc_id, ops)).expect("page_ops")
}

fn open_bytes(bytes: Vec<u8>) -> TestDoc {
    let info = with_state(move |st| registry::open(st, None, bytes, None)).expect("reopen");
    let doc_id = info.doc_id.clone();
    TestDoc { info, doc_id }
}

fn open_file(path: PathBuf) -> TestDoc {
    let bytes = std::fs::read(&path).expect("read output");
    open_bytes(bytes)
}

// ---------------------------------------------------------------------------------------

/// `FPDF_MovePages([3, 2] -> 1)` has ordered-list semantics: the named pages are lifted out in
/// the order given and re-inserted as one block, so `A B C D E` becomes `A D C B E`.
#[test]
fn pages_move_order() {
    let doc = open("gen/500p.pdf");
    let before: Vec<String> = (0..5).map(|p| page_number(&doc.doc_id, p)).collect();
    assert_eq!(
        before[0], "Page 1",
        "the fixture stamps each page with its number"
    );

    let info = ops(
        &doc.doc_id,
        vec![PageOp::Move {
            pages: vec![3, 2],
            to: 1,
        }],
    );
    assert_eq!(info.page_count, 500, "a move changes no page count");
    assert!(info.dirty, "a page op dirties the document");

    let after: Vec<String> = (0..5).map(|p| page_number(&doc.doc_id, p)).collect();
    assert_eq!(
        after,
        vec![
            before[0].clone(),
            before[3].clone(),
            before[2].clone(),
            before[1].clone(),
            before[4].clone(),
        ],
        "A B C D E -> A D C B E"
    );

    // Reverse puts them all back the other way round.
    let doc2 = open("gen/500p.pdf");
    ops(&doc2.doc_id, vec![PageOp::Reverse]);
    assert_eq!(page_number(&doc2.doc_id, 0), "Page 500");
    assert_eq!(page_number(&doc2.doc_id, 499), "Page 1");
}

/// Delete is descending, rotate is relative, insert-blank respects the size, and the whole
/// batch is a single undo step.
#[test]
fn pages_batch_is_one_undo_step() {
    let doc = open("gen/500p.pdf");
    let generation = doc.info.doc_generation;
    let third = page_number(&doc.doc_id, 2);

    let info = ops(
        &doc.doc_id,
        vec![
            PageOp::Delete { pages: vec![0, 1] },
            PageOp::Rotate {
                pages: vec![0],
                delta: 90,
            },
            PageOp::InsertBlank {
                at: 0,
                size: BlankPageSize::Named(NamedPageSize::A4),
            },
        ],
    );
    assert_eq!(info.page_count, 499);
    assert_eq!(
        info.doc_generation,
        generation + 1,
        "one page_ops call is one generation"
    );
    assert_eq!(
        page_number(&doc.doc_id, 1),
        third,
        "the blank page took index 0, so the old page 3 is at index 1"
    );
    assert_eq!(info.pages[1].rotation, 90);
    assert!((info.pages[0].width_pt - 595.28).abs() < 1.0, "A4 width");

    let undone = with_state({
        let doc_id = doc.doc_id.clone();
        move |st| registry::undo(st, &doc_id, false)
    })
    .expect("undo");
    assert_eq!(undone.page_count, 500, "the whole batch undoes at once");
}

/// A duplicate lands immediately after its source and carries the same content.
#[test]
fn pages_duplicate() {
    let doc = open("tracemonkey.pdf");
    let first = page_tag(&doc.doc_id, 0);
    let second = page_tag(&doc.doc_id, 1);

    let info = ops(&doc.doc_id, vec![PageOp::Duplicate { pages: vec![0] }]);
    assert_eq!(info.page_count, 15, "14 -> 15 pages");
    assert_eq!(page_tag(&doc.doc_id, 0), first);
    assert_eq!(
        page_tag(&doc.doc_id, 1),
        first,
        "the copy sits after its source"
    );
    assert_eq!(
        page_tag(&doc.doc_id, 2),
        second,
        "everything else shifted by one"
    );
}

/// `insertFrom`'s range string is **1-based** (`copy_pages_from_document`), while every index
/// inside the engine is 0-based. "1-3,5" must bring pages 1, 2, 3 and 5 — not 2, 3, 4 and 6.
#[test]
fn pages_insert_from_range_1based() {
    let doc = open("tracemonkey.pdf");
    let source = fixture("gen/500p.pdf");

    let info = ops(
        &doc.doc_id,
        vec![PageOp::InsertFrom {
            at: 0,
            path: source.display().to_string(),
            range: Some("1-3,5".to_string()),
            password: None,
        }],
    );
    assert_eq!(info.page_count, 18, "14 + 4 imported pages");
    assert_eq!(page_number(&doc.doc_id, 0), "Page 1");
    assert_eq!(page_number(&doc.doc_id, 1), "Page 2");
    assert_eq!(page_number(&doc.doc_id, 2), "Page 3");
    assert_eq!(
        page_number(&doc.doc_id, 3),
        "Page 5",
        "page 4 was not selected"
    );

    // The same range read as 0-based indices would have been 2,3,4,6.
    assert_ne!(page_number(&doc.doc_id, 0), "Page 2");
}

/// Extract copies the original and deletes the other pages, because `FPDF_ImportPages*` drops
/// the catalog's `/AcroForm` — a form document extracted by import renders but cannot be
/// filled (pages spike §2).
#[test]
fn pages_extract_keeps_acroform() {
    let doc = open("160F-2019.pdf");
    assert!(doc.info.has_form, "the fixture is an AcroForm document");
    let widgets = with_doc(&doc.doc_id, |d| Ok(d.page(0)?.annotations().len())).expect("widgets");
    assert!(widgets > 50, "76 widgets on page 1");

    let out = out_dir().join("extract-form.pdf");
    let result = with_state({
        let doc_id = doc.doc_id.clone();
        let out = out.display().to_string();
        move |st| pages::extract(st, &doc_id, &[0], &out, false)
    })
    .expect("extract_pages");
    assert!(result.bytes > 1000);

    let extracted = open_file(out);
    assert_eq!(extracted.info.page_count, 1);
    assert!(
        extracted.info.has_form,
        "the extracted file must keep /AcroForm"
    );
    let kept =
        with_doc(&extracted.doc_id, |d| Ok(d.page(0)?.annotations().len())).expect("widgets");
    assert_eq!(kept, widgets, "every widget survives");
}

/// `removeAfter` turns extract into "move these pages out of the document".
#[test]
fn pages_extract_removes_after() {
    let doc = open("tracemonkey.pdf");
    let second = page_tag(&doc.doc_id, 1);
    let out = out_dir().join("extract-two.pdf");

    let result = with_state({
        let doc_id = doc.doc_id.clone();
        let out = out.display().to_string();
        move |st| pages::extract(st, &doc_id, &[1, 2], &out, true)
    })
    .expect("extract_pages");
    assert!(result.doc_generation >= 2);

    let extracted = open_file(out);
    assert_eq!(extracted.info.page_count, 2);
    assert_eq!(page_tag(&extracted.doc_id, 0), second);

    let info = with_doc(&doc.doc_id, |d| Ok(d.info())).expect("info");
    assert_eq!(info.page_count, 12, "the two pages left the original");
}

/// Split by ranges and by "every N", both through the same copy-and-delete path.
#[test]
fn pages_split_writes_every_part() {
    let doc = open("tracemonkey.pdf");
    let dir = out_dir().join("split");
    let _ = std::fs::remove_dir_all(&dir);

    let parts = with_state({
        let doc_id = doc.doc_id.clone();
        move |st| {
            let d = st.doc(&doc_id)?;
            pages::split_plan(
                &SplitMode::EveryN { every_n: 5 },
                d.page_count(),
                &pages::output_stem(d),
            )
        }
    })
    .expect("split plan");
    assert_eq!(parts.len(), 3, "14 pages in blocks of 5");

    let mut written = Vec::new();
    for part in &parts {
        let path = with_state({
            let doc_id = doc.doc_id.clone();
            let part = part.clone();
            let dir = dir.clone();
            move |st| pages::write_split_part(st, &doc_id, &part, &dir)
        })
        .expect("write split part");
        written.push(path);
    }
    assert_eq!(written.len(), 3);
    let counts: Vec<u16> = written
        .iter()
        .map(|p| open_file(p.clone()).info.page_count)
        .collect();
    assert_eq!(counts, vec![5, 5, 4]);

    // Range mode, 1-based, exactly as the user typed it.
    let ranged = with_state({
        let doc_id = doc.doc_id.clone();
        move |st| {
            let d = st.doc(&doc_id)?;
            pages::split_plan(
                &SplitMode::Ranges {
                    ranges: vec!["1-2".into(), "14".into()],
                },
                d.page_count(),
                "tracemonkey",
            )
        }
    })
    .expect("split plan");
    assert_eq!(ranged[0].pages, vec![0, 1]);
    assert_eq!(ranged[1].pages, vec![13]);
}

/// Merge is the one importer, and it says out loud what import destroyed — since v0.3 only the
/// metadata: the fields and the outline are carried over (`pages::carry`).
#[test]
fn pages_merge_reports_what_it_lost() {
    let merged = with_state(move |st| {
        pages::merge(
            st,
            &[
                MergeInput {
                    path: fixture("tracemonkey.pdf").display().to_string(),
                    range: Some("1-2".into()),
                    password: None,
                },
                MergeInput {
                    path: fixture("160F-2019.pdf").display().to_string(),
                    range: None,
                    password: None,
                },
                MergeInput {
                    path: fixture("gen/outline-labels.pdf").display().to_string(),
                    range: Some("1".into()),
                    password: None,
                },
            ],
        )
    })
    .expect("merge_documents");
    let doc = TestDoc {
        doc_id: merged.info.doc_id.clone(),
        info: merged.info.clone(),
    };
    assert_eq!(doc.info.page_count, 4, "2 + 1 + 1");
    assert!(doc.info.path.is_none(), "a merge is untitled until Save As");
    assert!(
        !merged.warnings.contains(&MergeWarning::FormsDropped),
        "v0.3: 160F-2019.pdf's AcroForm is carried over"
    );
    assert!(
        !merged.warnings.contains(&MergeWarning::OutlineDropped),
        "v0.3: outline-labels.pdf's bookmarks are carried over"
    );
    assert!(merged.warnings.contains(&MergeWarning::MetadataDropped));
    assert!(doc.info.has_form, "the merged document has the fields");

    // A human-openable sample.
    let bytes = with_doc(&doc.doc_id, |d| Ok(d.to_bytes()?.to_vec())).expect("bytes");
    std::fs::write(out_dir().join("merged.pdf"), &bytes).expect("write merged sample");
}

/// The validation that keeps a bad request from reaching PDFium, where a `false` return would
/// leave the document in an undefined state.
#[test]
fn pages_rejects_impossible_requests() {
    let doc = open("tracemonkey.pdf");

    let dup = with_state({
        let doc_id = doc.doc_id.clone();
        move |st| {
            pages::apply_ops(
                st,
                &doc_id,
                vec![PageOp::Move {
                    pages: vec![2, 2],
                    to: 0,
                }],
            )
        }
    });
    assert!(dup.is_err(), "duplicate indices are refused");

    let out_of_range = with_state({
        let doc_id = doc.doc_id.clone();
        move |st| pages::apply_ops(st, &doc_id, vec![PageOp::Delete { pages: vec![0, 99] }])
    });
    assert!(out_of_range.is_err());

    let all = with_state({
        let doc_id = doc.doc_id.clone();
        move |st| {
            pages::apply_ops(
                st,
                &doc_id,
                vec![PageOp::Delete {
                    pages: (0..14).collect(),
                }],
            )
        }
    });
    assert!(all.is_err(), "a document must keep at least one page");

    let info = with_doc(&doc.doc_id, |d| Ok(d.info())).expect("info");
    assert_eq!(info.page_count, 14, "a refused batch changes nothing");
    assert_eq!(info.doc_generation, 1, "and does not bump the generation");
}

/// `sameAs` copies the neighbour's size; an explicit size is used verbatim.
#[test]
fn pages_insert_blank_sizes() {
    let doc = open("tracemonkey.pdf");
    let info = ops(
        &doc.doc_id,
        vec![
            PageOp::InsertBlank {
                at: 0,
                size: BlankPageSize::Named(NamedPageSize::SameAs),
            },
            PageOp::InsertBlank {
                at: 0,
                size: BlankPageSize::Explicit(PageSizePt {
                    width_pt: 200.0,
                    height_pt: 400.0,
                }),
            },
        ],
    );
    assert_eq!(info.page_count, 16);
    assert!((info.pages[0].width_pt - 200.0).abs() < 0.5);
    assert!((info.pages[0].height_pt - 400.0).abs() < 0.5);
    assert!(
        (info.pages[1].width_pt - 612.0).abs() < 0.5,
        "sameAs -> letter"
    );
}

/// Bug hunt: a merge has never been saved, so it opens **dirty** — otherwise closing the
/// window, quitting or opening another file discards it without the 저장 / 저장 안 함 / 취소
/// prompt, and autosave (which only writes dirty documents) never protects it either.
#[test]
fn pages_merge_result_is_dirty() {
    let merged = with_state(move |st| {
        pages::merge(
            st,
            &[
                MergeInput {
                    path: fixture("rotation.pdf").display().to_string(),
                    range: None,
                    password: None,
                },
                MergeInput {
                    path: fixture("tracemonkey.pdf").display().to_string(),
                    range: Some("1".into()),
                    password: None,
                },
            ],
        )
    })
    .expect("merge_documents");
    let doc = TestDoc {
        doc_id: merged.info.doc_id.clone(),
        info: merged.info.clone(),
    };
    assert!(doc.info.path.is_none());
    assert!(doc.info.dirty, "an untitled merge is unsaved work");

    // An edit keeps it dirty; Save As is what makes it clean.
    let edited = ops(
        &doc.doc_id,
        vec![PageOp::Rotate {
            pages: vec![0],
            delta: 90,
        }],
    );
    assert!(edited.dirty);
    let target = out_dir().join("merged-dirty.pdf");
    let _ = std::fs::remove_file(&target);
    let saved = with_state({
        let doc_id = doc.doc_id.clone();
        let target = target.display().to_string();
        move |st| {
            seepdf_lib::engine::save::save(st, &doc_id, Some(&target), false)?;
            Ok(st.doc(&doc_id)?.info())
        }
    })
    .expect("save as");
    assert!(!saved.dirty, "Save As makes the merge clean");
    let _ = std::fs::remove_file(&target);
}

// ---------------------------------------------------------------------------------------
// v0.3 pkg2-pages-structure-forms: P1 (merge / insert carry-over), P3 (import between open
// documents), P4 (split by bookmarks)
// ---------------------------------------------------------------------------------------

use seepdf_lib::engine::form;
use seepdf_lib::ipc::types::{FieldType, FieldValue, FormField, OutlineNode, OutlineSplit};

fn outline_of(doc_id: &str) -> Vec<OutlineNode> {
    with_doc(doc_id, |d| Ok(d.outline())).expect("outline")
}

fn fields_of(doc_id: &str) -> Vec<FormField> {
    with_doc(doc_id, |d| form::list(d, None)).expect("fields")
}

fn merge_docs(inputs: Vec<MergeInput>) -> (TestDoc, Vec<MergeWarning>) {
    let merged = with_state(move |st| pages::merge(st, &inputs)).expect("merge_documents");
    let doc = TestDoc {
        doc_id: merged.info.doc_id.clone(),
        info: merged.info.clone(),
    };
    (doc, merged.warnings)
}

fn input(name: &str) -> MergeInput {
    MergeInput {
        path: fixture(name).display().to_string(),
        range: None,
        password: None,
    }
}

/// `node` with every page shifted by `by` (what the carried tree must read back as).
fn shifted(nodes: &[OutlineNode], by: u16) -> Vec<(String, Option<u16>, usize)> {
    fn walk(
        nodes: &[OutlineNode],
        by: u16,
        depth: usize,
        out: &mut Vec<(String, Option<u16>, usize)>,
    ) {
        for n in nodes {
            out.push((n.title.clone(), n.page.map(|p| p + by), depth));
            walk(&n.children, by, depth + 1, out);
        }
    }
    let mut out = Vec::new();
    walk(nodes, by, 0, &mut out);
    out
}

/// P1: two outlined files merge into one document with both trees, each under a node named
/// after its file, every target on the right page; the page labels of both survive.
#[test]
fn pages_merge_keeps_both_outlines_and_labels() {
    let a = open("gen/outline-labels.pdf");
    let b = open("TAMReview.pdf");
    let (a_outline, b_outline) = (outline_of(&a.doc_id), outline_of(&b.doc_id));
    let (a_pages, b_pages) = (a.info.page_count, b.info.page_count);

    let (doc, warnings) = merge_docs(vec![
        input("gen/outline-labels.pdf"),
        input("TAMReview.pdf"),
    ]);
    assert!(
        !warnings.contains(&MergeWarning::OutlineDropped),
        "{warnings:?}"
    );
    assert_eq!(doc.info.page_count, a_pages + b_pages);
    let merged = outline_of(&doc.doc_id);
    assert_eq!(merged.len(), 2, "one top-level node per file");
    assert_eq!(merged[0].title, "outline-labels");
    assert_eq!(merged[0].page, Some(0));
    assert_eq!(merged[1].title, "TAMReview");
    assert_eq!(merged[1].page, Some(a_pages));
    assert_eq!(shifted(&merged[0].children, 0), shifted(&a_outline, 0));
    assert_eq!(
        shifted(&merged[1].children, 0),
        shifted(&b_outline, a_pages),
        "the second file's targets are offset by the first file's page count"
    );
    // outline-labels.pdf: i, ii, 1, 2, App-A, App-B; TAMReview: 1…23 (its own labels)
    let labels = doc.info.page_labels.clone().expect("labels carried");
    assert_eq!(&labels[..6], ["i", "ii", "1", "2", "App-A", "App-B"]);
    assert_eq!(labels[6], "1");
    assert_eq!(labels[6 + 22], "23");
}

/// P1: two form files merge with every field of both, none sharing a name, each fillable on
/// its own.
#[test]
fn pages_merge_keeps_both_forms_without_collision() {
    let original = open("160F-2019.pdf");
    let per_copy = fields_of(&original.doc_id).len();
    let (doc, warnings) = merge_docs(vec![input("160F-2019.pdf"), input("160F-2019.pdf")]);
    assert!(
        !warnings.contains(&MergeWarning::FormsDropped),
        "{warnings:?}"
    );
    assert!(doc.info.has_form);
    let fields = fields_of(&doc.doc_id);
    let first: Vec<&FormField> = fields.iter().filter(|f| f.page == 0).collect();
    let second: Vec<&FormField> = fields.iter().filter(|f| f.page == 1).collect();
    assert_eq!(first.len(), per_copy, "every widget of copy 1 is a field");
    assert_eq!(second.len(), per_copy, "every widget of copy 2 is a field");
    for f in &second {
        assert!(
            !first.iter().any(|g| g.name == f.name),
            "'{}' of the second copy was renamed away from the first",
            f.name
        );
    }
    // The same text field in both copies takes two different values.
    let text = first
        .iter()
        .find(|f| f.field_type == FieldType::Text && !f.read_only && f.rect.width() > 80.0)
        .expect("a text field");
    let twin = second
        .iter()
        .find(|f| f.index == text.index)
        .expect("its twin on page 2");
    for (f, value) in [(text, "first copy"), (twin, "second copy")] {
        let doc_id = doc.doc_id.clone();
        let (page, index) = (f.page, f.index);
        let value = FieldValue::Text {
            text: value.to_string(),
        };
        with_state(move |st| {
            registry::mutate(
                st,
                &doc_id,
                registry::MutateOpts::new(
                    "undo.formFill",
                    seepdf_lib::ipc::types::ChangeReason::Edit,
                )
                .page(page),
                |d| form::set_value(d, page, index, &value),
            )
        })
        .expect("set_form_field_value");
    }
    let after = fields_of(&doc.doc_id);
    let get = |f: &FormField| {
        after
            .iter()
            .find(|g| g.page == f.page && g.index == f.index)
            .and_then(|g| g.value.clone())
    };
    assert_eq!(get(text).as_deref(), Some("first copy"));
    assert_eq!(get(twin).as_deref(), Some("second copy"));

    // Saved, the file is a valid form: every widget's /Parent is a field, not a stray object.
    let bytes = with_doc(&doc.doc_id, |d| Ok(d.to_bytes()?.to_vec())).unwrap();
    let parsed = lopdf::Document::load_mem(&bytes).unwrap();
    for (_, obj) in parsed.objects.iter() {
        let Ok(dict) = obj.as_dict() else { continue };
        if dict.get(b"Subtype").and_then(|s| s.as_name()).ok() != Some(b"Widget") {
            continue;
        }
        if let Ok(parent) = dict.get(b"Parent").and_then(|p| p.as_reference()) {
            let parent = parsed
                .get_dictionary(parent)
                .expect("parent is a dictionary");
            assert!(
                parent.has(b"Kids"),
                "a widget's /Parent is a field with /Kids, not whatever the source number was"
            );
        }
    }
    std::fs::write(out_dir().join("merged-forms.pdf"), &bytes).unwrap();
}

/// P1: 파일에서 페이지 삽입 brings the source's outline (under its file name) and page labels,
/// shifts nothing it should not, and is one undo step.
#[test]
fn pages_insert_from_keeps_outline_and_labels() {
    let doc = open("tracemonkey.pdf");
    let source = open("gen/outline-labels.pdf");
    let source_outline = outline_of(&source.doc_id);
    let info = ops(
        &doc.doc_id,
        vec![PageOp::InsertFrom {
            at: 1,
            path: fixture("gen/outline-labels.pdf").display().to_string(),
            range: None,
            password: None,
        }],
    );
    assert_eq!(info.page_count, 14 + 6);
    let outline = outline_of(&doc.doc_id);
    assert_eq!(outline.len(), 1);
    assert_eq!(outline[0].title, "outline-labels");
    assert_eq!(outline[0].page, Some(1));
    assert_eq!(
        shifted(&outline[0].children, 0),
        shifted(&source_outline, 1)
    );
    let labels = info.page_labels.expect("labels");
    assert_eq!(
        &labels[..9],
        ["1", "i", "ii", "1", "2", "App-A", "App-B", "2", "3"],
        "the inserted pages keep their labels and the document's resume after them"
    );
    let undone = with_state({
        let doc_id = doc.doc_id.clone();
        move |st| registry::undo(st, &doc_id, false)
    })
    .expect("undo");
    assert_eq!(undone.page_count, 14, "one undo step");
    assert!(outline_of(&doc.doc_id).is_empty());
}

/// P3: pages of one open document copied into another, in the order given, one undo step on
/// the target, the source untouched; imported form widgets stay fields.
#[test]
fn pages_import_between_open_documents() {
    let src = open("gen/500p.pdf");
    let dst = open("tracemonkey.pdf");
    let info = with_state({
        let (s, d) = (src.doc_id.clone(), dst.doc_id.clone());
        move |st| pages::import_pages_from_doc(st, &s, &[3, 1], &d, 2)
    })
    .expect("import_pages_from_doc");
    assert_eq!(info.page_count, 16);
    assert_eq!(page_number(&dst.doc_id, 2), "Page 4");
    assert_eq!(page_number(&dst.doc_id, 3), "Page 2");
    assert_eq!(
        with_doc(&src.doc_id, |d| Ok(d.page_count())).unwrap(),
        500,
        "the source is untouched"
    );
    let undone = with_state({
        let doc_id = dst.doc_id.clone();
        move |st| registry::undo(st, &doc_id, false)
    })
    .expect("undo");
    assert_eq!(undone.page_count, 14);

    // A form page dragged over keeps working fields.
    let form_src = open("160F-2019.pdf");
    let expected = fields_of(&form_src.doc_id).len();
    with_state({
        let (s, d) = (form_src.doc_id.clone(), dst.doc_id.clone());
        move |st| pages::import_pages_from_doc(st, &s, &[0], &d, 0)
    })
    .expect("import a form page");
    let fields = fields_of(&dst.doc_id);
    assert_eq!(fields.iter().filter(|f| f.page == 0).count(), expected);

    let same = with_state({
        let s = src.doc_id.clone();
        move |st| pages::import_pages_from_doc(st, &s, &[0], &s, 0)
    });
    assert!(same.is_err(), "a document cannot import from itself");
}

/// P4: 책갈피로 분할 — one file per top-level bookmark, named after it, the right page counts.
#[test]
fn pages_split_by_outline() {
    let doc = open("gen/outline-labels.pdf");
    let outline = outline_of(&doc.doc_id);
    let plan = pages::split_plan_with_outline(
        &SplitMode::ByOutline {
            by_outline: OutlineSplit { level: 1 },
        },
        doc.info.page_count,
        "outline-labels",
        &outline,
    )
    .expect("plan");
    // Chapter A / B / C, each from its page to the page before the next chapter.
    let starts: Vec<u16> = outline.iter().filter_map(|n| n.page).collect();
    let front = usize::from(starts[0] > 0);
    assert_eq!(plan.len(), starts.len() + front);
    for (i, node) in outline.iter().enumerate() {
        let part = &plan[i + front];
        let first = node.page.unwrap();
        let last = starts
            .get(i + 1)
            .map(|n| n - 1)
            .unwrap_or(doc.info.page_count - 1);
        assert_eq!(
            part.pages,
            (first..=last).collect::<Vec<_>>(),
            "{}",
            node.title
        );
        assert!(
            part.name.ends_with(&format!("{}.pdf", node.title)),
            "{}",
            part.name
        );
    }
    let level2 = pages::split_plan_with_outline(
        &SplitMode::ByOutline {
            by_outline: OutlineSplit { level: 2 },
        },
        doc.info.page_count,
        "outline-labels",
        &outline,
    )
    .expect("level 2");
    assert!(level2.iter().any(|p| p.name.contains("A.1 Introduction")));

    // Written for real: every part opens with its page count.
    let dir = out_dir().join("split-by-outline");
    let _ = std::fs::remove_dir_all(&dir);
    for part in &plan {
        let path = with_state({
            let (doc_id, part, dir) = (doc.doc_id.clone(), part.clone(), dir.clone());
            move |st| pages::write_split_part(st, &doc_id, &part, &dir)
        })
        .expect("write part");
        let written = open_file(path);
        assert_eq!(written.info.page_count as usize, part.pages.len());
    }
    let none = pages::split_plan_with_outline(
        &SplitMode::ByOutline {
            by_outline: OutlineSplit { level: 1 },
        },
        14,
        "x",
        &[],
    );
    assert!(none.is_err(), "no outline, no split");
    assert_eq!(
        pages::safe_file_title("제1장: 서론/개요?", "x"),
        "제1장_ 서론_개요_"
    );
}
