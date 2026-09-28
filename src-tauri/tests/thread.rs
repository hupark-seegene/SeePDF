//! P2 — annotation replies / threads (`reply_annotation`, `IPC_CONTRACT.md` §7.1).
//!
//! A reply is a `Text` annotation whose `/IRT` is an indirect reference to its parent and whose
//! `/RT` is `/R`. PDFium cannot write the reference, so the engine writes it with `lopdf` through
//! `registry::mutate_bytes`; these tests check the result the way another viewer would see it —
//! by reopening the saved bytes, with PDFium **and** with `lopdf` — plus the thread-wide delete,
//! undo, the invisible appearance and the edits that must not break the link.

mod common;
use common::*;

use seepdf_lib::engine::annot;
use seepdf_lib::engine::registry::{self, MutateOpts};
use seepdf_lib::engine::render::tiles;
use seepdf_lib::ipc::types::{
    Annot, AnnotKind, AnnotPatch, AnnotSpec, ChangeReason, MarkupSpec, NoteSpec, Rect, TextAlign,
    TextBoxSpec,
};
use seepdf_lib::ipc::ErrorCode;

fn create(doc_id: &str, page: u16, spec: AnnotSpec) -> String {
    let doc_id = doc_id.to_string();
    with_state(move |st| {
        registry::mutate(
            st,
            &doc_id,
            MutateOpts::new("undo.annotCreate", ChangeReason::Edit).page(page),
            |doc| annot::create::create(doc, page, &spec, None),
        )
    })
    .expect("create annotation")
}

fn reply(doc_id: &str, page: u16, parent: &str, text: &str, author: Option<&str>) -> String {
    let (doc_id, parent, text) = (doc_id.to_string(), parent.to_string(), text.to_string());
    let author = author.map(str::to_string);
    with_state(move |st| annot::reply::reply(st, &doc_id, page, &parent, &text, author.as_deref()))
        .expect("reply")
}

fn list(doc_id: &str, page: u16) -> Vec<Annot> {
    with_doc(doc_id, move |doc| annot::list(doc, page)).expect("list annotations")
}

fn find<'a>(annots: &'a [Annot], id: &str) -> &'a Annot {
    annots
        .iter()
        .find(|a| a.id == id)
        .unwrap_or_else(|| panic!("annotation {id} not listed"))
}

fn update(doc_id: &str, page: u16, id: &str, patch: AnnotPatch) {
    let (doc_id, id) = (doc_id.to_string(), id.to_string());
    with_state(move |st| {
        registry::mutate(
            st,
            &doc_id,
            MutateOpts::new("undo.annotEdit", ChangeReason::Edit).page(page),
            |doc| annot::update::update(doc, page, &id, &patch),
        )
    })
    .expect("update annotation");
}

fn delete(doc_id: &str, page: u16, ids: Vec<String>) -> usize {
    let doc_id = doc_id.to_string();
    with_state(move |st| {
        registry::mutate(
            st,
            &doc_id,
            MutateOpts::new("undo.annotDelete", ChangeReason::Edit).page(page),
            |doc| annot::delete(doc, page, &ids),
        )
    })
    .expect("delete annotations")
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

fn note(at: [f32; 2]) -> AnnotSpec {
    AnnotSpec::Note(NoteSpec {
        at,
        color: [255, 216, 77],
        contents: "원래 메모".into(),
    })
}

/// RGBA of `rect` at 2×, through the viewer's render path.
fn render_rect(doc_id: &str, page: u16, rect: Rect) -> Vec<u8> {
    let doc_id = doc_id.to_string();
    let buffer = with_state(move |st| tiles::render_raw_buffer(st, &doc_id, page, 2.0, Some(rect)))
        .expect("render");
    buffer[32..].to_vec()
}

/// The annotation dictionary whose `/NM` is `nm`, anywhere in the file (lopdf).
fn lopdf_annot<'d>(
    doc: &'d lopdf::Document,
    nm: &str,
) -> Option<(lopdf::ObjectId, &'d lopdf::Dictionary)> {
    doc.objects.iter().find_map(|(id, obj)| {
        let dict = obj.as_dict().ok()?;
        let name = dict
            .get(b"NM")
            .ok()
            .and_then(|o| lopdf::decode_text_string(o).ok())?;
        (name == nm).then_some((*id, dict))
    })
}

#[test]
fn reply_written_reopened_irt_points_to_the_parent() {
    let doc = open("tracemonkey.pdf");
    let parent = create(&doc.doc_id, 0, note([100.0, 700.0]));
    let first = reply(
        &doc.doc_id,
        0,
        &parent,
        "검토했습니다 — 수정 필요",
        Some("홍길동"),
    );
    // a reply to the reply: the thread nests
    let second = reply(&doc.doc_id, 0, &first, "반영했습니다", Some("Kim"));

    // In memory, straight after the rewrite.
    let annots = list(&doc.doc_id, 0);
    let r1 = find(&annots, &first);
    assert_eq!(r1.kind, AnnotKind::Note);
    assert_eq!(r1.subtype, "Text");
    assert_eq!(r1.in_reply_to.as_deref(), Some(parent.as_str()));
    assert_eq!(r1.contents, "검토했습니다 — 수정 필요");
    assert_eq!(r1.author.as_deref(), Some("홍길동"));
    assert!(r1.created.is_some());
    assert_eq!(
        r1.color,
        [255, 216, 77],
        "the reply shares its parent's colour"
    );
    assert_eq!(
        find(&annots, &second).in_reply_to.as_deref(),
        Some(first.as_str())
    );
    assert_eq!(find(&annots, &parent).in_reply_to, None);
    let (dirty, undo) = with_doc(&doc.doc_id, |d| Ok((d.dirty(), d.history.undo_label()))).unwrap();
    assert!(dirty);
    assert_eq!(undo.as_deref(), Some("undo.annotReply"));

    // Saved and reopened with PDFium: the link survives the file.
    let bytes = save_bytes(&doc.doc_id);
    let again = reopen(bytes.clone());
    let reread = list(&again.doc_id, 0);
    assert_eq!(
        find(&reread, &first).in_reply_to.as_deref(),
        Some(parent.as_str())
    );
    assert_eq!(
        find(&reread, &second).in_reply_to.as_deref(),
        Some(first.as_str())
    );
    assert_eq!(find(&reread, &first).author.as_deref(), Some("홍길동"));

    // …and with lopdf, the way another viewer reads it: `/IRT` is an indirect reference to the
    // parent's dictionary, `/RT /R`, and the reply sits in the page's /Annots.
    let file = lopdf::Document::load_mem(&bytes).expect("lopdf reads the saved file");
    let (parent_id, _) = lopdf_annot(&file, &parent).expect("parent in the file");
    let (reply_id, reply_dict) = lopdf_annot(&file, &first).expect("reply in the file");
    let irt = reply_dict
        .get(b"IRT")
        .expect("/IRT")
        .as_reference()
        .expect("/IRT is a reference");
    assert_eq!(irt, parent_id);
    assert_eq!(reply_dict.get(b"RT").unwrap().as_name().unwrap(), b"R");
    assert_eq!(
        reply_dict.get(b"Subtype").unwrap().as_name().unwrap(),
        b"Text"
    );
    let page_id = file.get_pages()[&1];
    let annots_obj = file
        .get_dictionary(page_id)
        .unwrap()
        .get(b"Annots")
        .unwrap();
    let annots_arr = match annots_obj {
        lopdf::Object::Reference(id) => file.get_object(*id).unwrap().as_array().unwrap().clone(),
        other => other.as_array().unwrap().clone(),
    };
    assert!(annots_arr
        .iter()
        .any(|o| o.as_reference().ok() == Some(reply_id)));
}

#[test]
fn reply_is_not_drawn_on_the_page_even_after_an_edit() {
    let doc = open("tracemonkey.pdf");
    // A highlight parent: a note icon drawn for the reply (at the parent's top-left corner)
    // would land on the text, so any drawing shows up in the pixels.
    let parent = create(
        &doc.doc_id,
        0,
        AnnotSpec::Highlight(MarkupSpec {
            rects: vec![Rect::new(100.0, 640.0, 260.0, 652.0)],
            color: [77, 184, 255],
            opacity: 0.4,
            contents: None,
        }),
    );
    let area = Rect::new(90.0, 600.0, 270.0, 660.0);
    let before = render_rect(&doc.doc_id, 0, area);
    let id = reply(&doc.doc_id, 0, &parent, "보이지 않는 답글", None);
    assert_eq!(
        render_rect(&doc.doc_id, 0, area),
        before,
        "a reply adds no icon to the page"
    );
    update(
        &doc.doc_id,
        0,
        &id,
        AnnotPatch {
            contents: Some("고친 답글".into()),
            ..AnnotPatch::default()
        },
    );
    let annots = list(&doc.doc_id, 0);
    assert_eq!(find(&annots, &id).contents, "고친 답글");
    assert_eq!(
        find(&annots, &id).in_reply_to.as_deref(),
        Some(parent.as_str())
    );
    assert_eq!(
        render_rect(&doc.doc_id, 0, area),
        before,
        "an edit keeps the empty appearance"
    );

    // Other viewers get the same: the reply's normal appearance is an empty Form XObject.
    let file = lopdf::Document::load_mem(&save_bytes(&doc.doc_id)).unwrap();
    let (_, dict) = lopdf_annot(&file, &id).expect("reply in the file");
    let ap = dict.get(b"AP").unwrap().as_dict().unwrap();
    let n = file
        .get_object(ap.get(b"N").unwrap().as_reference().unwrap())
        .unwrap();
    let stream = n.as_stream().expect("a Form XObject");
    let content = stream
        .decompressed_content()
        .unwrap_or_else(|_| stream.content.clone());
    assert!(
        content.iter().all(|b| b.is_ascii_whitespace()),
        "empty appearance, got {content:?}"
    );
}

#[test]
fn deleting_the_parent_deletes_its_thread_and_undo_restores_it() {
    let doc = open("tracemonkey.pdf");
    let parent = create(&doc.doc_id, 0, note([100.0, 500.0]));
    let other = create(&doc.doc_id, 0, note([400.0, 500.0]));
    let r1 = reply(&doc.doc_id, 0, &parent, "하나", None);
    let r2 = reply(&doc.doc_id, 0, &r1, "둘", None);
    let r3 = reply(&doc.doc_id, 0, &other, "다른 스레드", None);
    assert_eq!(list(&doc.doc_id, 0).len(), 5);

    // deleting one reply takes its own replies with it, not its parent
    let removed = delete(&doc.doc_id, 0, vec![r1.clone()]);
    assert_eq!(removed, 2);
    let ids: Vec<String> = list(&doc.doc_id, 0).into_iter().map(|a| a.id).collect();
    assert_eq!(ids.len(), 3);
    assert!(ids.contains(&parent) && ids.contains(&other) && ids.contains(&r3));

    // undo brings the branch back; deleting the parent takes the whole thread
    let id = doc.doc_id.clone();
    with_state(move |st| registry::undo(st, &id, false)).expect("undo");
    assert_eq!(list(&doc.doc_id, 0).len(), 5);
    delete(&doc.doc_id, 0, vec![parent.clone()]);
    let left: Vec<String> = list(&doc.doc_id, 0).into_iter().map(|a| a.id).collect();
    assert_eq!(left, vec![other.clone(), r3.clone()]);

    // the deletion is in the file too
    let reread = list(&reopen(save_bytes(&doc.doc_id)).doc_id, 0);
    assert_eq!(reread.len(), 2);
    assert!(reread.iter().all(|a| a.id != r2));
}

#[test]
fn undo_removes_the_reply() {
    let doc = open("tracemonkey.pdf");
    let parent = create(&doc.doc_id, 1, note([80.0, 700.0]));
    let id = reply(&doc.doc_id, 1, &parent, "되돌릴 답글", None);
    assert_eq!(list(&doc.doc_id, 1).len(), 2);
    let d = doc.doc_id.clone();
    with_state(move |st| registry::undo(st, &d, false)).expect("undo");
    let annots = list(&doc.doc_id, 1);
    assert_eq!(annots.len(), 1);
    assert!(annots.iter().all(|a| a.id != id));
}

#[test]
fn a_rebuilt_parent_keeps_its_replies() {
    let doc = open("tracemonkey.pdf");
    let parent = create(
        &doc.doc_id,
        0,
        AnnotSpec::Textbox(TextBoxSpec {
            rect: Rect::new(100.0, 600.0, 300.0, 640.0),
            text: "원문".into(),
            font_size: 12.0,
            color: [0, 0, 0],
            align: TextAlign::Left,
            fill_color: None,
        }),
    );
    let id = reply(&doc.doc_id, 0, &parent, "문구를 바꿔 주세요", None);
    // a text box's text is rebuilt (delete + re-create under the same /NM)
    update(
        &doc.doc_id,
        0,
        &parent,
        AnnotPatch {
            text: Some("바꾼 문구".into()),
            ..AnnotPatch::default()
        },
    );
    let annots = list(&doc.doc_id, 0);
    assert_eq!(find(&annots, &parent).text.as_deref(), Some("바꾼 문구"));
    assert_eq!(
        find(&annots, &id).in_reply_to.as_deref(),
        Some(parent.as_str())
    );
    let reread = list(&reopen(save_bytes(&doc.doc_id)).doc_id, 0);
    assert_eq!(
        find(&reread, &id).in_reply_to.as_deref(),
        Some(parent.as_str())
    );
}

#[test]
fn a_reply_to_a_foreign_highlight() {
    let doc = open("annotation-highlight.pdf");
    let foreign = list(&doc.doc_id, 0);
    let parent = foreign
        .iter()
        .find(|a| a.kind == AnnotKind::Highlight)
        .expect("the fixture has a highlight")
        .id
        .clone();
    let id = reply(&doc.doc_id, 0, &parent, "출처 확인", Some("검토자"));
    let reread = list(&reopen(save_bytes(&doc.doc_id)).doc_id, 0);
    assert_eq!(
        find(&reread, &id).in_reply_to.as_deref(),
        Some(parent.as_str())
    );
    assert_eq!(reread.len(), foreign.len() + 1);
}

#[test]
fn reply_errors() {
    let doc = open("tracemonkey.pdf");
    let highlight = create(
        &doc.doc_id,
        0,
        AnnotSpec::Highlight(MarkupSpec {
            rects: vec![Rect::new(100.0, 700.0, 200.0, 712.0)],
            color: [255, 216, 77],
            opacity: 0.4,
            contents: None,
        }),
    );
    let d = doc.doc_id.clone();
    let err = with_state(move |st| annot::reply::reply(st, &d, 0, "no-such-annot", "x", None))
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::NotFound);
    // wrong page for the parent
    let (d, h) = (doc.doc_id.clone(), highlight.clone());
    let err = with_state(move |st| annot::reply::reply(st, &d, 1, &h, "x", None)).unwrap_err();
    assert_eq!(err.code, ErrorCode::NotFound);
    // nothing was pushed for the failed attempts
    let undo = with_doc(&doc.doc_id, |d| Ok(d.history.undo_label())).unwrap();
    assert_eq!(undo.as_deref(), Some("undo.annotCreate"));

    let locked =
        try_open("gen/encrypted-rc4-40.pdf", Some("user")).expect("open with the password");
    let d = locked.doc_id.clone();
    let err = with_state(move |st| annot::reply::reply(st, &d, 0, "any", "x", None)).unwrap_err();
    assert_eq!(err.code, ErrorCode::Unsupported);
}
