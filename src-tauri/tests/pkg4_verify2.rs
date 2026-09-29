//! pkg4 verification round 2: regression probes for the lopdf kinds (resize + recolour, print
//! flag through edits and reopen, callout text edits, heads / dash) and annotation_batch.
mod common;
use common::*;

use seepdf_lib::engine::annot;
use seepdf_lib::engine::annot::create::create_in;
use seepdf_lib::engine::annot::update::{batch_in, update_in};
use seepdf_lib::engine::pages::boxes;
use seepdf_lib::engine::registry;
use seepdf_lib::engine::render::tiles;
use seepdf_lib::ipc::types::*;

fn make(doc_id: &str, spec: AnnotSpec) -> String {
    let d = doc_id.to_string();
    with_state(move |st| create_in(st, &d, 0, &spec, None, Some("검증"))).expect("create_in")
}
fn patch(doc_id: &str, id: &str, p: AnnotPatch) -> Result<Annot, seepdf_lib::ipc::EngineError> {
    let (d, id) = (doc_id.to_string(), id.to_string());
    with_state(move |st| update_in(st, &d, 0, &id, &p))
}
fn list(doc_id: &str) -> Vec<Annot> {
    let d = doc_id.to_string();
    with_doc(&d.clone(), move |doc| annot::list(doc, 0)).unwrap()
}
fn get(doc_id: &str, id: &str) -> Annot {
    list(doc_id)
        .into_iter()
        .find(|a| a.id == id)
        .unwrap_or_else(|| panic!("{id} gone"))
}
fn depth(doc_id: &str) -> usize {
    with_doc(doc_id, |d| Ok(d.history.undo_depth())).unwrap()
}
fn undo(doc_id: &str) {
    let d = doc_id.to_string();
    with_state(move |st| registry::undo(st, &d, false)).unwrap();
}
fn line_spec() -> AnnotSpec {
    AnnotSpec::Line(LineSpec {
        p1: [100.0, 100.0],
        p2: [300.0, 200.0],
        color: [200, 0, 0],
        width: 2.0,
        opacity: 1.0,
        heads: None,
        measure: None,
        dashed: false,
    })
}
fn poly_spec() -> AnnotSpec {
    AnnotSpec::Polygon(PolySpec {
        vertices: vec![300.0, 400.0, 420.0, 400.0, 420.0, 500.0, 300.0, 500.0],
        color: [0, 90, 200],
        fill_color: None,
        width: 1.5,
        opacity: 1.0,
        cloudy: false,
        dashed: false,
        measure: None,
    })
}
fn callout_spec() -> AnnotSpec {
    AnnotSpec::Callout(CalloutSpec {
        rect: Rect::new(440.0, 600.0, 570.0, 640.0),
        text: "설명".into(),
        font_size: 12.0,
        color: [0, 0, 0],
        align: TextAlign::Left,
        fill_color: None,
        callout: vec![380.0, 560.0, 420.0, 620.0, 440.0, 620.0],
    })
}

#[test]
fn probe_resize_then_recolor_own_shapes() {
    let doc = open("tracemonkey.pdf");
    let l = make(&doc.doc_id, line_spec());
    let p = make(&doc.doc_id, poly_spec());
    let c = make(&doc.doc_id, callout_spec());
    let before: Vec<Annot> = [&l, &p, &c].iter().map(|i| get(&doc.doc_id, i)).collect();
    {
        let d = doc.doc_id.clone();
        with_state(move |st| {
            boxes::resize_pages(
                st,
                &d,
                &PageSelection::List(vec![0]),
                ResizeTarget::Size { w: 306.0, h: 396.0 },
                ResizeMode::ScaleContent,
            )
        })
        .unwrap();
    }
    let resized: Vec<Annot> = [&l, &p, &c].iter().map(|i| get(&doc.doc_id, i)).collect();
    for (id, col) in [(&l, [0u8, 150, 0]), (&p, [0, 150, 0]), (&c, [0, 150, 0])] {
        patch(
            &doc.doc_id,
            id,
            AnnotPatch {
                color: Some(col),
                ..Default::default()
            },
        )
        .expect("recolor");
    }
    let after: Vec<Annot> = [&l, &p, &c].iter().map(|i| get(&doc.doc_id, i)).collect();
    for k in 0..3 {
        println!("kind {:?}\n before rect {:?} L {:?} V {:?} CL {:?} w {}\n resized rect {:?} L {:?} V {:?} CL {:?} w {}\n after rect {:?} L {:?} V {:?} CL {:?} w {}",
            before[k].kind, before[k].rect, before[k].line_points, before[k].vertices, before[k].callout, before[k].border_width,
            resized[k].rect, resized[k].line_points, resized[k].vertices, resized[k].callout, resized[k].border_width,
            after[k].rect, after[k].line_points, after[k].vertices, after[k].callout, after[k].border_width);
    }
    let (r, a) = (&resized[0], &after[0]);
    assert_eq!(
        r.line_points.map(|x| x.map(|v| v.round())),
        a.line_points.map(|x| x.map(|v| v.round())),
        "line snapped"
    );
    assert!(
        (r.rect.l - a.rect.l).abs() < 3.0 && (r.rect.t - a.rect.t).abs() < 3.0,
        "line rect jumped {:?} -> {:?}",
        r.rect,
        a.rect
    );
    assert!(
        (resized[1].rect.l - after[1].rect.l).abs() < 3.0
            && (resized[1].rect.t - after[1].rect.t).abs() < 3.0,
        "polygon rect jumped"
    );
    assert!(
        (resized[2].rect.l - after[2].rect.l).abs() < 3.0
            && (resized[2].rect.t - after[2].rect.t).abs() < 3.0,
        "callout rect jumped {:?} -> {:?}",
        resized[2].rect,
        after[2].rect
    );
}

#[test]
fn probe_printed_survives_lopdf_edit_and_reopen() {
    let doc = open("tracemonkey.pdf");
    let l = make(&doc.doc_id, line_spec());
    patch(
        &doc.doc_id,
        &l,
        AnnotPatch {
            printed: Some(false),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(!get(&doc.doc_id, &l).printed, "printed off");
    patch(
        &doc.doc_id,
        &l,
        AnnotPatch {
            color: Some([0, 0, 200]),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(!get(&doc.doc_id, &l).printed, "printed off after recolor");
    let c = make(&doc.doc_id, callout_spec());
    patch(
        &doc.doc_id,
        &c,
        AnnotPatch {
            printed: Some(false),
            ..Default::default()
        },
    )
    .unwrap();
    patch(
        &doc.doc_id,
        &c,
        AnnotPatch {
            text: Some("바뀜".into()),
            contents: Some("바뀜".into()),
            ..Default::default()
        },
    )
    .unwrap();
    let ca = get(&doc.doc_id, &c);
    assert!(!ca.printed, "callout printed off after text edit");
    assert_eq!(ca.kind, AnnotKind::Callout);
    let d = doc.doc_id.clone();
    let bytes = with_state(move |st| {
        tiles::generate_appearances(st, &d)?;
        Ok(st.doc(&d)?.to_bytes()?.to_vec())
    })
    .unwrap();
    let info = with_state(move |st| registry::open(st, None, bytes, None)).unwrap();
    let re = list(&info.doc_id);
    assert!(!re.iter().find(|a| a.id == l).unwrap().printed);
    let rc = re.iter().find(|a| a.id == c).unwrap();
    assert!(!rc.printed);
    assert_eq!(rc.text.as_deref(), Some("바뀜"));
    assert_eq!(rc.author.as_deref(), Some("검증"), "callout author");
}

#[test]
fn probe_callout_text_edit_one_step_keeps_leader() {
    let doc = open("tracemonkey.pdf");
    let c = make(&doc.doc_id, callout_spec());
    let before = get(&doc.doc_id, &c);
    let d0 = depth(&doc.doc_id);
    patch(
        &doc.doc_id,
        &c,
        AnnotPatch {
            text: Some("새 글".into()),
            align: Some(TextAlign::Center),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(depth(&doc.doc_id), d0 + 1);
    let after = get(&doc.doc_id, &c);
    println!(
        "before {:?} {:?}\nafter {:?} {:?} {:?}",
        before.rect, before.callout, after.rect, after.callout, after.align
    );
    assert_eq!(after.callout, before.callout);
    assert_eq!(after.align, Some(TextAlign::Center));
    assert_eq!(after.author.as_deref(), Some("검증"));
    undo(&doc.doc_id);
    let back = get(&doc.doc_id, &c);
    assert_eq!(back.text.as_deref(), Some("설명"));
    assert_eq!(back.kind, AnnotKind::Callout);
}

#[test]
fn probe_heads_off_turns_arrow_into_line_and_dashed_line() {
    let doc = open("tracemonkey.pdf");
    let a = make(
        &doc.doc_id,
        AnnotSpec::Arrow(LineSpec {
            p1: [100.0, 100.0],
            p2: [300.0, 200.0],
            color: [200, 0, 0],
            width: 2.0,
            opacity: 1.0,
            heads: Some([false, true]),
            measure: None,
            dashed: false,
        }),
    );
    assert_eq!(get(&doc.doc_id, &a).kind, AnnotKind::Arrow);
    patch(
        &doc.doc_id,
        &a,
        AnnotPatch {
            heads: Some([false, false]),
            ..Default::default()
        },
    )
    .unwrap();
    let x = get(&doc.doc_id, &a);
    assert_eq!(x.kind, AnnotKind::Line, "{:?}", x.heads);
    patch(
        &doc.doc_id,
        &a,
        AnnotPatch {
            dashed: Some(true),
            ..Default::default()
        },
    )
    .unwrap();
    let y = get(&doc.doc_id, &a);
    assert!(y.dashed, "dashed line");
    assert!(
        (y.border_width - 2.0).abs() < 0.01,
        "width {}",
        y.border_width
    );
    patch(
        &doc.doc_id,
        &a,
        AnnotPatch {
            border_width: Some(4.0),
            ..Default::default()
        },
    )
    .unwrap();
    let z = get(&doc.doc_id, &a);
    assert!(
        z.dashed && (z.border_width - 4.0).abs() < 0.01,
        "{} {}",
        z.dashed,
        z.border_width
    );
}

#[test]
fn probe_batch_mixed_lopdf() {
    let doc = open("tracemonkey.pdf");
    let l = make(&doc.doc_id, line_spec());
    let p = make(&doc.doc_id, poly_spec());
    let d0 = depth(&doc.doc_id);
    let n0 = list(&doc.doc_id).len();
    let ops = vec![
        AnnotOp::Update {
            id: l.clone(),
            patch: AnnotPatch {
                color: Some([0, 0, 0]),
                ..Default::default()
            },
        },
        AnnotOp::Create {
            spec: callout_spec(),
            id: None,
        },
        AnnotOp::Delete {
            ids: vec![p.clone()],
        },
    ];
    let d = doc.doc_id.clone();
    let created = with_state(move |st| batch_in(st, &d, 0, &ops, Some("B"))).unwrap();
    assert_eq!(created.len(), 1);
    assert_eq!(depth(&doc.doc_id), d0 + 1);
    let mid = list(&doc.doc_id);
    assert_eq!(mid.len(), n0);
    assert_eq!(
        mid.iter().find(|a| a.id == created[0]).unwrap().kind,
        AnnotKind::Callout
    );
    undo(&doc.doc_id);
    let back = list(&doc.doc_id);
    assert_eq!(back.len(), n0);
    assert_eq!(get(&doc.doc_id, &l).color, [200, 0, 0]);
    assert!(back.iter().any(|a| a.id == p));
}

fn save_reopen(doc_id: &str) -> String {
    let d = doc_id.to_string();
    let bytes = with_state(move |st| {
        tiles::generate_appearances(st, &d)?;
        Ok(st.doc(&d)?.to_bytes()?.to_vec())
    })
    .unwrap();
    with_state(move |st| registry::open(st, None, bytes, None))
        .unwrap()
        .doc_id
}

#[test]
fn probe_callout_text_edit_survives_save() {
    let doc = open("tracemonkey.pdf");
    let c = make(&doc.doc_id, callout_spec());
    patch(
        &doc.doc_id,
        &c,
        AnnotPatch {
            text: Some("바뀜".into()),
            contents: Some("바뀜".into()),
            ..Default::default()
        },
    )
    .unwrap();
    let a = get(&doc.doc_id, &c);
    println!(
        "in memory: text {:?} contents {:?} kind {:?}",
        a.text, a.contents, a.kind
    );
    let re = save_reopen(&doc.doc_id);
    let b = get(&re, &c);
    println!(
        "reopened: text {:?} contents {:?} kind {:?}",
        b.text, b.contents, b.kind
    );
    assert_eq!(b.text.as_deref(), Some("바뀜"));
}

#[test]
fn probe_textbox_text_edit_survives_save() {
    let doc = open("tracemonkey.pdf");
    let c = make(
        &doc.doc_id,
        AnnotSpec::Textbox(TextBoxSpec {
            rect: Rect::new(440.0, 600.0, 570.0, 640.0),
            text: "설명".into(),
            font_size: 12.0,
            color: [0, 0, 0],
            align: TextAlign::Left,
            fill_color: None,
        }),
    );
    patch(
        &doc.doc_id,
        &c,
        AnnotPatch {
            text: Some("바뀜".into()),
            contents: Some("바뀜".into()),
            ..Default::default()
        },
    )
    .unwrap();
    let re = save_reopen(&doc.doc_id);
    let b = get(&re, &c);
    println!(
        "textbox reopened: text {:?} contents {:?}",
        b.text, b.contents
    );
    assert_eq!(b.text.as_deref(), Some("바뀜"));
}

fn ink(y: f32) -> AnnotSpec {
    AnnotSpec::Ink(InkSpec {
        paths: vec![vec![100.0, y, 200.0, y, 300.0, y]],
        color: [0, 0, 255],
        width: 2.0,
        opacity: 1.0,
    })
}

/// tracemonkey.pdf padded past LARGE_DOC_BYTES (48 MB): an undo depth of 3.
fn open_large() -> String {
    let mut bytes = std::fs::read(fixture("tracemonkey.pdf")).unwrap();
    bytes.extend_from_slice(b"\n%");
    bytes.extend(std::iter::repeat_n(b'x', 49 * 1024 * 1024));
    bytes.extend_from_slice(b"\n");
    with_state(move |st| registry::open(st, None, bytes, None))
        .unwrap()
        .doc_id
}

/// A document over LARGE_DOC_BYTES (48 MB) gets an undo depth of 3: a scrub across 4 strokes.
#[test]
fn probe_batch_on_a_large_document() {
    let doc_id = open_large();
    let ids: Vec<String> = (0..4)
        .map(|k| make(&doc_id, ink(100.0 + 40.0 * k as f32)))
        .collect();
    let before: Vec<Vec<Vec<f32>>> = ids
        .iter()
        .map(|i| get(&doc_id, i).ink_paths.unwrap())
        .collect();
    println!("depth before batch = {}", depth(&doc_id));
    let ops: Vec<AnnotOp> = ids
        .iter()
        .enumerate()
        .map(|(k, i)| AnnotOp::Update {
            id: i.clone(),
            patch: AnnotPatch {
                paths: Some(vec![vec![
                    100.0,
                    100.0 + 40.0 * k as f32,
                    140.0,
                    100.0 + 40.0 * k as f32,
                ]]),
                ..Default::default()
            },
        })
        .collect();
    let d = doc_id.clone();
    with_state(move |st| batch_in(st, &d, 0, &ops, None)).unwrap();
    println!("depth after batch = {}", depth(&doc_id));
    undo(&doc_id);
    let after: Vec<Vec<Vec<f32>>> = ids
        .iter()
        .map(|i| get(&doc_id, i).ink_paths.unwrap())
        .collect();
    for k in 0..4 {
        println!(
            "stroke {k}: before {:?}\n          after undo {:?}",
            before[k], after[k]
        );
    }
    assert_eq!(after, before, "one undo of the scrub restores every stroke");
}

/// Round 2 fix (A3): on a depth-3 document, a batch of 4 updates whose 5th op fails is rolled
/// back completely — every stroke as before, the undo stack as before, nothing to redo.
#[test]
fn batch_rollback_on_a_large_document() {
    let doc_id = open_large();
    let ids: Vec<String> = (0..4)
        .map(|k| make(&doc_id, ink(100.0 + 40.0 * k as f32)))
        .collect();
    let before: Vec<Vec<Vec<f32>>> = ids
        .iter()
        .map(|i| get(&doc_id, i).ink_paths.unwrap())
        .collect();
    let depth_before = depth(&doc_id);
    let mut ops: Vec<AnnotOp> = ids
        .iter()
        .map(|i| AnnotOp::Update {
            id: i.clone(),
            patch: AnnotPatch {
                paths: Some(vec![vec![10.0, 10.0, 20.0, 10.0]]),
                ..Default::default()
            },
        })
        .collect();
    ops.push(AnnotOp::Update {
        id: "no-such-annotation".into(),
        patch: AnnotPatch {
            paths: Some(vec![vec![10.0, 10.0, 20.0, 10.0]]),
            ..Default::default()
        },
    });
    let d = doc_id.clone();
    assert!(with_state(move |st| batch_in(st, &d, 0, &ops, None)).is_err());
    let after: Vec<Vec<Vec<f32>>> = ids
        .iter()
        .map(|i| get(&doc_id, i).ink_paths.unwrap())
        .collect();
    assert_eq!(after, before, "the failed batch is all or nothing");
    assert_eq!(depth(&doc_id), depth_before);
    let d = doc_id.clone();
    let redo = with_doc(&d, |d| Ok(d.history.redo_depth())).unwrap();
    assert_eq!(redo, 0, "a rolled-back batch is not offered as a redo");
}

/// Round 2 fix (A3): a batch of more ops than DEFAULT_DEPTH (50) on a normal document is still
/// one undo step.
#[test]
fn batch_longer_than_the_undo_depth_is_one_step() {
    let doc = open("tracemonkey.pdf");
    let n = seepdf_lib::engine::history::DEFAULT_DEPTH + 5;
    let ops: Vec<AnnotOp> = (0..n)
        .map(|k| AnnotOp::Create {
            spec: ink(50.0 + 10.0 * k as f32),
            id: None,
        })
        .collect();
    let base = list(&doc.doc_id).len();
    let d = doc.doc_id.clone();
    let created = with_state(move |st| batch_in(st, &d, 0, &ops, None)).unwrap();
    assert_eq!(created.len(), n);
    assert_eq!(list(&doc.doc_id).len(), base + n);
    assert_eq!(depth(&doc.doc_id), 1);
    undo(&doc.doc_id);
    assert_eq!(
        list(&doc.doc_id).len(),
        base,
        "one undo removes every stroke"
    );
}

/// Round 2 fix (A6): a dashed square, circle, line and arrow are created dashed (what 복제 /
/// 붙여넣기 send for a dashed original), each in one undo step, and a later width change
/// keeps the dash.
#[test]
fn dashed_shapes_and_lines_are_created_dashed() {
    let doc = open("tracemonkey.pdf");
    let shape = ShapeSpec {
        rect: Rect::new(100.0, 100.0, 200.0, 180.0),
        color: [200, 0, 0],
        fill_color: None,
        width: 2.0,
        opacity: 1.0,
        dashed: true,
    };
    let line = LineSpec {
        p1: [100.0, 300.0],
        p2: [300.0, 360.0],
        color: [0, 0, 200],
        width: 2.0,
        opacity: 1.0,
        heads: None,
        measure: None,
        dashed: true,
    };
    let specs = [
        AnnotSpec::Square(shape.clone()),
        AnnotSpec::Circle(shape),
        AnnotSpec::Line(line.clone()),
        AnnotSpec::Arrow(line),
    ];
    for spec in specs {
        let depth_before = depth(&doc.doc_id);
        let id = make(&doc.doc_id, spec.clone());
        let a = get(&doc.doc_id, &id);
        assert!(a.dashed, "{spec:?} is created dashed");
        assert_eq!(depth(&doc.doc_id), depth_before + 1, "one undo step");
        patch(
            &doc.doc_id,
            &id,
            AnnotPatch {
                border_width: Some(4.0),
                ..Default::default()
            },
        )
        .unwrap();
        let a = get(&doc.doc_id, &id);
        assert!(a.dashed && (a.border_width - 4.0).abs() < 0.01, "{a:?}");
        undo(&doc.doc_id);
        undo(&doc.doc_id);
        assert!(
            list(&doc.doc_id).iter().all(|x| x.id != id),
            "one undo removes the dashed {:?}",
            a.kind
        );
    }
    // a solid spec stays solid
    let id = make(
        &doc.doc_id,
        AnnotSpec::Square(ShapeSpec {
            rect: Rect::new(100.0, 100.0, 200.0, 180.0),
            color: [200, 0, 0],
            fill_color: None,
            width: 2.0,
            opacity: 1.0,
            dashed: false,
        }),
    );
    assert!(!get(&doc.doc_id, &id).dashed);
}
