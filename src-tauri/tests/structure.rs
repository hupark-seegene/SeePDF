//! P2 document structure — outline editing, links with targets, page labels
//! (`IPC_CONTRACT.md` §7.10). Everything is written with lopdf (or PDFium where it can), saved,
//! reopened with **PDFium** and read back through the same calls the app uses; the file
//! structure itself (`/Count`, `/First` … `/Parent`, `/Border`) is inspected with lopdf.

mod common;
use common::*;

use seepdf_lib::engine::annot;
use seepdf_lib::engine::registry;
use seepdf_lib::engine::save;
use seepdf_lib::engine::structure::{labels, links, outline};
use seepdf_lib::ipc::types::{
    Annot, AnnotKind, AnnotResult, DocInfo, LinkDest, LinkTarget, LinkUrl, OutlineDest,
    OutlineNode, PageLabelRange, PageLabelStyle, Rect,
};
use seepdf_lib::ipc::{EngineError, ErrorCode};
use std::collections::HashSet;
use std::path::PathBuf;

fn out_dir() -> PathBuf {
    let dir = fixture("out").join("p2");
    std::fs::create_dir_all(&dir).expect("create fixtures/out/p2");
    dir
}

fn set_outline(doc_id: &str, nodes: Vec<OutlineNode>) -> Result<DocInfo, EngineError> {
    let doc_id = doc_id.to_string();
    with_state(move |st| outline::set_outline(st, &doc_id, &nodes))
}

fn get_outline(doc_id: &str) -> Vec<OutlineNode> {
    with_doc(doc_id, |d| Ok(d.outline())).expect("outline")
}

fn set_labels(doc_id: &str, ranges: Vec<PageLabelRange>) -> Result<DocInfo, EngineError> {
    let doc_id = doc_id.to_string();
    with_state(move |st| labels::set_page_labels(st, &doc_id, &ranges))
}

fn get_labels(doc_id: &str) -> Result<Vec<PageLabelRange>, EngineError> {
    let doc_id = doc_id.to_string();
    with_state(move |st| labels::get_page_labels(st, &doc_id))
}

fn create_link(
    doc_id: &str,
    page: u16,
    rect: Rect,
    target: LinkTarget,
) -> Result<AnnotResult, EngineError> {
    let doc_id = doc_id.to_string();
    with_state(move |st| links::create_link(st, &doc_id, page, rect, &target))
}

fn update_link(
    doc_id: &str,
    page: u16,
    id: &str,
    rect: Option<Rect>,
    target: Option<LinkTarget>,
) -> Result<AnnotResult, EngineError> {
    let (doc_id, id) = (doc_id.to_string(), id.to_string());
    with_state(move |st| links::update_link(st, &doc_id, page, &id, rect, target.as_ref()))
}

fn delete_link(doc_id: &str, page: u16, id: &str) -> Result<AnnotResult, EngineError> {
    let (doc_id, id) = (doc_id.to_string(), id.to_string());
    with_state(move |st| links::delete_link(st, &doc_id, page, &id))
}

fn info(doc_id: &str) -> DocInfo {
    with_doc(doc_id, |d| Ok(d.info())).expect("info")
}

fn undo(doc_id: &str) -> DocInfo {
    let doc_id = doc_id.to_string();
    with_state(move |st| registry::undo(st, &doc_id, false)).expect("undo")
}

fn list(doc_id: &str, page: u16) -> Vec<Annot> {
    with_doc(doc_id, move |d| annot::list(d, page)).expect("list annotations")
}

/// Save As into `fixtures/out/p2/<name>` and return the bytes on disk.
fn save_as(doc_id: &str, name: &str) -> Vec<u8> {
    let target = out_dir().join(name);
    let _ = std::fs::remove_file(&target);
    let (doc_id, path) = (doc_id.to_string(), target.display().to_string());
    with_state(move |st| save::save(st, &doc_id, Some(&path), false)).expect("save as");
    std::fs::read(&target).expect("read the saved file")
}

fn reopen(bytes: Vec<u8>) -> TestDoc {
    let info = with_state(move |st| registry::open(st, None, bytes, None)).expect("reopen");
    let doc_id = info.doc_id.clone();
    TestDoc { info, doc_id }
}

fn node(title: &str, page: Option<u16>) -> OutlineNode {
    OutlineNode {
        title: title.into(),
        page,
        dest: None,
        url: None,
        open: None,
        children: vec![],
    }
}

fn page_target(page: u16, y: Option<f32>) -> LinkTarget {
    LinkTarget::Page(LinkDest {
        page,
        x: None,
        y,
        zoom: None,
    })
}

fn url_target(url: &str) -> LinkTarget {
    LinkTarget::Url(LinkUrl { url: url.into() })
}

// ---------------------------------------------------------------------------------------
// Outline
// ---------------------------------------------------------------------------------------

/// The tree the outline tests write: 3 levels, Hangul, an `/XYZ` with zoom, a closed node, a
/// URI node, a node with no target at all.
fn sample_outline() -> Vec<OutlineNode> {
    vec![
        OutlineNode {
            dest: Some(OutlineDest {
                x: None,
                y: Some(700.0),
                zoom: None,
            }),
            open: Some(true),
            children: vec![
                OutlineNode {
                    dest: Some(OutlineDest {
                        x: Some(72.0),
                        y: Some(650.5),
                        zoom: Some(1.5),
                    }),
                    ..node("1.1 배경", Some(1))
                },
                OutlineNode {
                    open: Some(false),
                    children: vec![node("1.2.a 세부 (closed parent)", Some(3))],
                    ..node("1.2 방법", Some(2))
                },
            ],
            ..node("1장 서론", Some(0))
        },
        OutlineNode {
            url: Some("https://example.com/seepdf?q=1".into()),
            ..node("웹 링크", None)
        },
        node("No target", None),
        node("Last page", Some(13)),
    ]
}

/// `/First`, `/Last`, `/Prev`, `/Next`, `/Parent` and `/Count` agree with each other all the
/// way down; returns the number of items.
fn check_outline_structure(bytes: &[u8]) -> usize {
    let doc = lopdf::Document::load_mem(bytes).expect("lopdf parses the file");
    let root_id = doc
        .catalog()
        .unwrap()
        .get(b"Outlines")
        .unwrap()
        .as_reference()
        .unwrap();
    let root = doc.get_dictionary(root_id).unwrap();
    assert_eq!(root.get(b"Type").unwrap().as_name().unwrap(), b"Outlines");
    let mut seen = HashSet::new();
    let (items, visible) = check_level(&doc, root_id, &mut seen);
    assert_eq!(
        root.get(b"Count").unwrap().as_i64().unwrap(),
        visible,
        "outline /Count"
    );
    items
}

/// Walks the children of `parent`; returns (items at all levels, visible items when open).
fn check_level(
    doc: &lopdf::Document,
    parent: lopdf::ObjectId,
    seen: &mut HashSet<lopdf::ObjectId>,
) -> (usize, i64) {
    let dict = doc.get_dictionary(parent).unwrap();
    let Ok(first) = dict.get(b"First") else {
        assert!(!dict.has(b"Last"), "/Last without /First");
        return (0, 0);
    };
    let mut current = Some(first.as_reference().unwrap());
    let mut previous: Option<lopdf::ObjectId> = None;
    let (mut items, mut visible) = (0usize, 0i64);
    while let Some(id) = current {
        assert!(seen.insert(id), "cycle at {id:?}");
        let item = doc.get_dictionary(id).unwrap();
        assert_eq!(
            item.get(b"Parent").unwrap().as_reference().unwrap(),
            parent,
            "/Parent"
        );
        assert_eq!(
            item.get(b"Prev").ok().map(|p| p.as_reference().unwrap()),
            previous,
            "/Prev"
        );
        let (sub_items, sub_visible) = check_level(doc, id, seen);
        items += 1 + sub_items;
        visible += 1;
        if sub_items > 0 {
            let count = item.get(b"Count").unwrap().as_i64().unwrap();
            assert_eq!(
                count.abs(),
                sub_visible,
                "|/Count| of an item = its visible descendants"
            );
            if count > 0 {
                visible += count;
            }
        } else {
            assert!(!item.has(b"Count"), "a leaf has no /Count");
        }
        previous = Some(id);
        current = item.get(b"Next").ok().map(|n| n.as_reference().unwrap());
    }
    assert_eq!(
        dict.get(b"Last").unwrap().as_reference().unwrap(),
        previous.unwrap(),
        "/Last"
    );
    (items, visible)
}

fn catalog_has(bytes: &[u8], key: &[u8]) -> bool {
    let doc = lopdf::Document::load_mem(bytes).expect("lopdf parses the file");
    doc.catalog().expect("catalog").has(key)
}

#[test]
fn structure_outline_roundtrip() {
    let doc = open("tracemonkey.pdf");
    let wanted = sample_outline();
    let info = set_outline(&doc.doc_id, wanted.clone()).expect("set_outline");
    assert!(info.has_outline && info.dirty && info.can_undo);
    assert_eq!(info.undo_label.as_deref(), Some("undo.outlineEdit"));
    assert!(info.doc_generation > doc.info.doc_generation);
    assert_eq!(info.page_count, 14);

    // get_outline reads back exactly what was written.
    assert_eq!(get_outline(&doc.doc_id), wanted);

    // …and so does a fresh PDFium open of the saved file.
    let bytes = save_as(&doc.doc_id, "outline.pdf");
    let saved = reopen(bytes.clone());
    assert!(saved.info.has_outline);
    assert_eq!(get_outline(&saved.doc_id), wanted);

    // File structure: 7 items, root /Count = 1장 + 1.1 + 1.2 + 웹 + No target + Last = 6
    // (1.2.a is hidden by the closed 1.2).
    assert_eq!(check_outline_structure(&bytes), 7);
    let parsed = lopdf::Document::load_mem(&bytes).unwrap();
    let root_id = parsed
        .catalog()
        .unwrap()
        .get(b"Outlines")
        .unwrap()
        .as_reference()
        .unwrap();
    assert_eq!(
        parsed
            .get_dictionary(root_id)
            .unwrap()
            .get(b"Count")
            .unwrap()
            .as_i64()
            .unwrap(),
        6
    );
}

/// The raw reader keeps Stage 2's destination mapping (STAGE1C_NOTES §7.1): TAMReview's first
/// node is an `/XYZ 0 806` and reads back as such, with `open` / `url` absent on leaves.
#[test]
fn structure_outline_reader_keeps_the_stage2_dest() {
    let doc = open("TAMReview.pdf");
    let outline = get_outline(&doc.doc_id);
    assert!(!outline.is_empty());
    let first = &outline[0];
    assert_eq!(first.page, Some(0));
    let dest = first.dest.expect("an /XYZ destination");
    assert_eq!((dest.x, dest.y), (Some(0.0), Some(806.0)));
    fn walk(nodes: &[OutlineNode]) {
        for n in nodes {
            assert_eq!(
                n.open.is_some(),
                !n.children.is_empty(),
                "open only on nodes with children"
            );
            assert!(n.url.is_none());
            walk(&n.children);
        }
    }
    walk(&outline);
}

#[test]
fn structure_outline_undo_remove_and_validation() {
    let name = "gen/outline-labels.pdf";
    if !fixture(name).exists() {
        eprintln!("skipping: run `cargo run --release --example gen_fixtures` first");
        return;
    }
    let doc = open(name);
    let original = get_outline(&doc.doc_id);
    assert_eq!(original.len(), 3);
    assert_eq!(
        original[0].open,
        Some(true),
        "the fixture's Chapter A has /Count 2"
    );

    // A page past the end is refused and pushes no undo step.
    let err = set_outline(&doc.doc_id, vec![node("x", Some(6))]).expect_err("page 6 of 6");
    assert_eq!(err.code, ErrorCode::InvalidArgument);
    assert!(!info(&doc.doc_id).can_undo);

    // Replace, then undo: the fixture's own 7 nodes come back.
    set_outline(&doc.doc_id, vec![node("새 목차", Some(2))]).expect("replace");
    assert_eq!(get_outline(&doc.doc_id), vec![node("새 목차", Some(2))]);
    let undone = undo(&doc.doc_id);
    assert!(undone.has_outline);
    assert_eq!(get_outline(&doc.doc_id), original);

    // An empty list removes the outline: no /Outlines, and the old items are gone from the file.
    let info = set_outline(&doc.doc_id, vec![]).expect("remove");
    assert!(!info.has_outline);
    assert!(get_outline(&doc.doc_id).is_empty());
    let bytes = save_as(&doc.doc_id, "outline-removed.pdf");
    assert!(!catalog_has(&bytes, b"Outlines"));
    let parsed = lopdf::Document::load_mem(&bytes).unwrap();
    let titles = parsed
        .objects
        .values()
        .filter_map(|o| o.as_dict().ok())
        .filter(|d| d.has(b"Title"))
        .count();
    assert_eq!(
        titles, 0,
        "the old outline items were deleted, not just unlinked"
    );
    assert!(!reopen(bytes).info.has_outline);
}

// ---------------------------------------------------------------------------------------
// Page labels
// ---------------------------------------------------------------------------------------

fn range(
    start: u16,
    style: PageLabelStyle,
    prefix: Option<&str>,
    first: Option<u32>,
) -> PageLabelRange {
    PageLabelRange {
        start,
        style,
        prefix: prefix.map(str::to_owned),
        first,
    }
}

#[test]
fn structure_labels_roundtrip() {
    let doc = open("tracemonkey.pdf");
    assert_eq!(doc.info.page_labels, None, "tracemonkey has no /PageLabels");
    assert_eq!(get_labels(&doc.doc_id).unwrap(), vec![]);

    let ranges = vec![
        range(0, PageLabelStyle::Roman, None, None),
        range(3, PageLabelStyle::Decimal, None, None),
        range(10, PageLabelStyle::AlphaUpper, Some("부록-"), None),
        range(13, PageLabelStyle::NoNumber, Some("Back cover"), None),
    ];
    let info = set_labels(&doc.doc_id, ranges.clone()).expect("set_page_labels");
    let expected: Vec<String> = [
        "i",
        "ii",
        "iii",
        "1",
        "2",
        "3",
        "4",
        "5",
        "6",
        "7",
        "부록-A",
        "부록-B",
        "부록-C",
        "Back cover",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    assert_eq!(info.page_labels.as_ref(), Some(&expected));
    assert_eq!(info.pages[10].label.as_deref(), Some("부록-A"));
    assert_eq!(info.undo_label.as_deref(), Some("undo.pageLabels"));
    assert!(info.dirty && info.can_undo);
    assert_eq!(get_labels(&doc.doc_id).unwrap(), ranges);

    // Save → reopen: FPDF_GetPageLabel gives the same labels, lopdf the same ranges.
    let bytes = save_as(&doc.doc_id, "labels.pdf");
    let saved = reopen(bytes);
    assert_eq!(saved.info.page_labels.as_ref(), Some(&expected));
    assert_eq!(get_labels(&saved.doc_id).unwrap(), ranges);

    // A first range that starts later gets plain numbers in front of it; /St is honoured.
    let info = set_labels(
        &doc.doc_id,
        vec![range(2, PageLabelStyle::Roman, Some("p."), Some(4))],
    )
    .unwrap();
    let labels = info.page_labels.unwrap();
    assert_eq!(&labels[..4], &["1", "2", "p.iv", "p.v"]);
    assert_eq!(
        get_labels(&doc.doc_id).unwrap(),
        vec![
            range(0, PageLabelStyle::Decimal, None, None),
            range(2, PageLabelStyle::Roman, Some("p."), Some(4))
        ]
    );

    // Undo twice: back to no labels at all.
    undo(&doc.doc_id);
    let back = undo(&doc.doc_id);
    assert_eq!(back.page_labels, None);
    assert!(back.pages.iter().all(|p| p.label.is_none()));

    // Validation.
    let err = set_labels(
        &doc.doc_id,
        vec![range(14, PageLabelStyle::Decimal, None, None)],
    )
    .unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidArgument);
    let err = set_labels(
        &doc.doc_id,
        vec![
            range(1, PageLabelStyle::Decimal, None, None),
            range(1, PageLabelStyle::Roman, None, None),
        ],
    )
    .unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidArgument);
}

#[test]
fn structure_labels_fixture_undo_and_remove() {
    let name = "gen/outline-labels.pdf";
    if !fixture(name).exists() {
        eprintln!("skipping: run `cargo run --release --example gen_fixtures` first");
        return;
    }
    let doc = open(name);
    let original: Vec<String> = ["i", "ii", "1", "2", "App-A", "App-B"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    assert_eq!(doc.info.page_labels.as_ref(), Some(&original));
    assert_eq!(
        get_labels(&doc.doc_id).unwrap(),
        vec![
            range(0, PageLabelStyle::Roman, None, None),
            range(2, PageLabelStyle::Decimal, None, None),
            range(4, PageLabelStyle::AlphaUpper, Some("App-"), None),
        ],
        "/St 1 reads back as no `first`"
    );

    let info = set_labels(&doc.doc_id, vec![]).expect("remove");
    assert_eq!(info.page_labels, None);
    let bytes = save_as(&doc.doc_id, "labels-removed.pdf");
    assert!(!catalog_has(&bytes, b"PageLabels"));
    assert_eq!(get_labels(&doc.doc_id).unwrap(), vec![]);

    let back = undo(&doc.doc_id);
    assert_eq!(back.page_labels.as_ref(), Some(&original));
    assert!(
        get_outline(&doc.doc_id).len() == 3,
        "the outline was not touched"
    );
}

// ---------------------------------------------------------------------------------------
// Links
// ---------------------------------------------------------------------------------------

fn find_link<'a>(annots: &'a [Annot], id: &str) -> &'a Annot {
    annots
        .iter()
        .find(|a| a.id == id)
        .unwrap_or_else(|| panic!("link {id} not listed"))
}

fn border_of(bytes: &[u8], id: &str) -> Vec<i64> {
    let doc = lopdf::Document::load_mem(bytes).unwrap();
    for object in doc.objects.values() {
        let Ok(dict) = object.as_dict() else { continue };
        let name = dict
            .get(b"NM")
            .ok()
            .and_then(|n| lopdf::decode_text_string(n).ok());
        if name.as_deref() == Some(id) {
            assert!(
                !(dict.has(b"Dest") && dict.has(b"A")),
                "a link has /Dest or /A, never both"
            );
            return dict
                .get(b"Border")
                .unwrap()
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_float().unwrap() as i64)
                .collect();
        }
    }
    panic!("no annotation /NM {id} in the file");
}

#[test]
fn structure_link_to_a_web_address() {
    let doc = open("tracemonkey.pdf");
    let rect = Rect::new(300.0, 556.0, 120.0, 540.0); // corners in any order
    let result = create_link(
        &doc.doc_id,
        0,
        rect,
        url_target("https://example.com/seepdf"),
    )
    .expect("create");
    let link = result.annot.clone().expect("the new link");
    assert_eq!(link.kind, AnnotKind::Link);
    assert_eq!(link.uri.as_deref(), Some("https://example.com/seepdf"));
    assert_eq!(link.dest, None);
    assert_eq!(link.rect, Rect::new(120.0, 540.0, 300.0, 556.0));
    assert!(result.previous.is_none());
    assert_eq!(
        info(&doc.doc_id).undo_label.as_deref(),
        Some("undo.linkCreate")
    );

    // Non-ASCII is percent-encoded (a URI is 7-bit).
    let result = update_link(
        &doc.doc_id,
        0,
        &link.id,
        None,
        Some(url_target("https://예.kr/a b")),
    )
    .unwrap();
    assert_eq!(
        result.annot.as_ref().unwrap().uri.as_deref(),
        Some("https://%EC%98%88.kr/a%20b")
    );
    assert_eq!(
        result.previous.as_ref().unwrap().uri.as_deref(),
        Some("https://example.com/seepdf")
    );

    let bytes = save_as(&doc.doc_id, "link-url.pdf");
    assert_eq!(
        border_of(&bytes, &link.id),
        vec![0, 0, 0],
        "a link draws no box"
    );
    let saved = reopen(bytes);
    let read = find_link(&list(&saved.doc_id, 0), &link.id).clone();
    assert_eq!(read.uri.as_deref(), Some("https://%EC%98%88.kr/a%20b"));

    // Validation.
    let err = create_link(
        &doc.doc_id,
        0,
        Rect::new(10.0, 10.0, 10.5, 40.0),
        url_target("https://x"),
    )
    .unwrap_err();
    assert_eq!(
        err.code,
        ErrorCode::InvalidArgument,
        "a sliver is not a click area"
    );
    let err = create_link(&doc.doc_id, 0, rect, url_target("   ")).unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidArgument);
    let err = update_link(&doc.doc_id, 0, "no-such-id", Some(rect), None).unwrap_err();
    assert_eq!(err.code, ErrorCode::NotFound);
}

#[test]
fn structure_link_to_a_page() {
    let doc = open("tracemonkey.pdf");
    let rect = Rect::new(100.0, 100.0, 220.0, 130.0);
    let result =
        create_link(&doc.doc_id, 0, rect, page_target(5, Some(500.0))).expect("create page link");
    let link = result.annot.clone().expect("the new link");
    assert_eq!(link.kind, AnnotKind::Link);
    assert_eq!(
        link.dest,
        Some(LinkDest {
            page: 5,
            x: None,
            y: Some(500.0),
            zoom: None
        })
    );
    assert_eq!(link.uri, None);
    let after_create = info(&doc.doc_id);
    assert_eq!(after_create.undo_label.as_deref(), Some("undo.linkCreate"));

    // page → web address (lopdf: /A in, /Dest out)
    let r = update_link(
        &doc.doc_id,
        0,
        &link.id,
        None,
        Some(url_target("https://example.com")),
    )
    .unwrap();
    let now = r.annot.unwrap();
    assert_eq!(
        (now.uri.as_deref(), now.dest),
        (Some("https://example.com"), None)
    );
    assert_eq!(r.previous.unwrap().dest.map(|d| d.page), Some(5));

    // web address → page, whole page (/Fit reads back with no position)
    let r = update_link(&doc.doc_id, 0, &link.id, None, Some(page_target(2, None))).unwrap();
    let now = r.annot.unwrap();
    assert_eq!(
        now.dest,
        Some(LinkDest {
            page: 2,
            x: None,
            y: None,
            zoom: None
        })
    );
    assert_eq!(now.uri, None);

    // move only (PDFium)
    let moved = Rect::new(150.0, 150.0, 260.0, 190.0);
    let r = update_link(&doc.doc_id, 0, &link.id, Some(moved), None).unwrap();
    let now = r.annot.unwrap();
    assert_eq!(now.rect, moved);
    assert_eq!(now.dest.map(|d| d.page), Some(2), "moving keeps the target");
    assert_eq!(
        info(&doc.doc_id).undo_label.as_deref(),
        Some("undo.linkEdit")
    );

    // Save → reopen with PDFium.
    let bytes = save_as(&doc.doc_id, "link-page.pdf");
    assert_eq!(border_of(&bytes, &link.id), vec![0, 0, 0]);
    let saved = reopen(bytes);
    let read = find_link(&list(&saved.doc_id, 0), &link.id).clone();
    assert_eq!(
        read.dest,
        Some(LinkDest {
            page: 2,
            x: None,
            y: None,
            zoom: None
        })
    );
    assert_eq!(read.rect, moved);

    // Delete, then undo brings it back.
    let r = delete_link(&doc.doc_id, 0, &link.id).unwrap();
    assert!(r.list.annots.iter().all(|a| a.id != link.id));
    assert_eq!(r.previous.unwrap().id, link.id);
    assert_eq!(
        info(&doc.doc_id).undo_label.as_deref(),
        Some("undo.linkDelete")
    );
    undo(&doc.doc_id);
    assert_eq!(find_link(&list(&doc.doc_id, 0), &link.id).rect, moved);

    // Validation.
    let err = create_link(&doc.doc_id, 0, rect, page_target(14, None)).unwrap_err();
    assert_eq!(
        err.code,
        ErrorCode::InvalidArgument,
        "target page out of range"
    );
    let err = create_link(&doc.doc_id, 14, rect, page_target(0, None)).unwrap_err();
    assert_eq!(err.code, ErrorCode::NotFound, "link page out of range");
}

#[test]
fn structure_link_update_refuses_other_annotations() {
    let doc = open("annotation-highlight.pdf");
    let annots = list(&doc.doc_id, 0);
    let other = annots
        .iter()
        .find(|a| a.kind != AnnotKind::Link)
        .expect("a non-link annotation");
    let err = update_link(
        &doc.doc_id,
        0,
        &other.id,
        Some(Rect::new(0.0, 0.0, 50.0, 50.0)),
        None,
    )
    .unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidArgument);
    let err = delete_link(&doc.doc_id, 0, &other.id).unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidArgument);
}

// ---------------------------------------------------------------------------------------
// The PDFium check of a rewrite
// ---------------------------------------------------------------------------------------

/// A rewrite PDFium reads differently from what was meant never replaces the document: the
/// check's `verifyFailed` comes back, no undo step is pushed, the generation does not move and
/// the outline is the one from before.
#[test]
fn structure_failed_check_rolls_back() {
    use seepdf_lib::engine::registry::MutateOpts;
    use seepdf_lib::ipc::types::ChangeReason;
    let doc = open("tracemonkey.pdf");
    let doc_id = doc.doc_id.clone();
    let err = with_state(move |st| {
        registry::mutate_bytes_checked(
            st,
            &doc_id,
            MutateOpts::new("undo.outlineEdit", ChangeReason::Edit).all_pages(),
            |bytes, _| Ok(bytes.to_vec()),
            |_, _| {
                Err(EngineError::new(
                    ErrorCode::VerifyFailed,
                    "the check said no",
                ))
            },
        )
    })
    .unwrap_err();
    assert_eq!(err.code, ErrorCode::VerifyFailed);
    let after = info(&doc.doc_id);
    assert!(!after.can_undo && !after.dirty);
    assert_eq!(after.doc_generation, doc.info.doc_generation);
    assert_eq!(
        get_outline(&doc.doc_id).len(),
        get_outline_len_of("tracemonkey.pdf")
    );
}

fn get_outline_len_of(name: &str) -> usize {
    let fresh = open(name);
    get_outline(&fresh.doc_id).len()
}

// ---------------------------------------------------------------------------------------
// Encrypted documents
// ---------------------------------------------------------------------------------------

#[test]
fn structure_refused_on_encrypted() {
    let doc = try_open("gen/encrypted-rc4-40.pdf", Some("user")).expect("open with the password");
    let can_undo = |id: &str| with_doc(id, |d| Ok(d.history.can_undo())).unwrap();

    let err = set_outline(&doc.doc_id, vec![node("x", Some(0))]).unwrap_err();
    assert_eq!(err.code, ErrorCode::Unsupported);
    assert!(
        err.message.contains("remove the password first"),
        "{}",
        err.message
    );
    let err = set_labels(
        &doc.doc_id,
        vec![range(0, PageLabelStyle::Roman, None, None)],
    )
    .unwrap_err();
    assert_eq!(err.code, ErrorCode::Unsupported);
    assert_eq!(
        get_labels(&doc.doc_id).unwrap_err().code,
        ErrorCode::Unsupported
    );
    let rect = Rect::new(50.0, 50.0, 150.0, 80.0);
    let err = create_link(&doc.doc_id, 0, rect, page_target(0, Some(10.0))).unwrap_err();
    assert_eq!(err.code, ErrorCode::Unsupported);
    assert!(!can_undo(&doc.doc_id), "a refusal pushes no undo step");

    // A web link needs no rewrite: PDFium writes it, encryption and all.
    let r = create_link(&doc.doc_id, 0, rect, url_target("https://example.com")).expect("url link");
    assert_eq!(r.annot.unwrap().uri.as_deref(), Some("https://example.com"));
    assert!(can_undo(&doc.doc_id));
}

// ---------------------------------------------------------------------------------------
// v0.3 pkg2-pages-structure-forms (P5): link borders and quads, outline fidelity
// ---------------------------------------------------------------------------------------

/// The link `/NM id`'s dictionary in saved bytes.
fn link_dict(bytes: &[u8], id: &str) -> lopdf::Dictionary {
    let doc = lopdf::Document::load_mem(bytes).unwrap();
    for object in doc.objects.values() {
        let Ok(dict) = object.as_dict() else { continue };
        let name = dict
            .get(b"NM")
            .ok()
            .and_then(|n| lopdf::decode_text_string(n).ok());
        if name.as_deref() == Some(id) {
            return dict.clone();
        }
    }
    panic!("no annotation /NM {id}");
}

fn floats(o: &lopdf::Object) -> Vec<f32> {
    o.as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_float().unwrap_or_else(|_| v.as_i64().unwrap() as f32))
        .collect()
}

#[test]
fn structure_link_quads_and_border() {
    use seepdf_lib::ipc::types::LinkBorder;
    let doc = open("tracemonkey.pdf");
    let quads = vec![
        Rect::new(100.0, 700.0, 300.0, 712.0),
        Rect::new(72.0, 686.0, 180.0, 698.0),
    ];
    let red = LinkBorder {
        width: 1.0,
        color: [255, 0, 0],
    };
    for target in [page_target(3, None), url_target("https://example.com/q")] {
        let (id, q) = (doc.doc_id.clone(), quads.clone());
        let r = with_state(move |st| {
            links::create_link_styled(st, &id, 0, Rect::ZERO, &target, &q, Some(red))
        })
        .expect("create_link with quads");
        let link = r.annot.expect("the link");
        let read = link.quads.clone().expect("quads read back");
        assert_eq!(read.len(), 2, "a link with 2 quads reads back 2 quads");
        assert!(
            (link.rect.l - 72.0).abs() < 0.1 && (link.rect.t - 712.0).abs() < 0.1,
            "the union rect: {:?}",
            link.rect
        );
        let bytes = save_as(&doc.doc_id, "link-quads.pdf");
        let dict = link_dict(&bytes, &link.id);
        assert_eq!(floats(dict.get(b"Border").unwrap()), [0.0, 0.0, 1.0]);
        assert_eq!(floats(dict.get(b"C").unwrap()), [1.0, 0.0, 0.0]);
        assert_eq!(floats(dict.get(b"QuadPoints").unwrap()).len(), 16);

        // The border off again, target untouched.
        let (id, link_id) = (doc.doc_id.clone(), link.id.clone());
        with_state(move |st| {
            links::update_link_styled(
                st,
                &id,
                0,
                &link_id,
                None,
                None,
                Some(LinkBorder {
                    width: 0.0,
                    color: [0, 0, 0],
                }),
            )
        })
        .expect("border off");
        let bytes = save_as(&doc.doc_id, "link-quads.pdf");
        assert_eq!(border_of(&bytes, &link.id), [0, 0, 0]);
    }
}

/// `set_outline` keeps a named `/Dest` and `/C` `/F` on a node whose title and target did not
/// change; a renamed node is written fresh.
#[test]
fn structure_outline_keeps_named_dests_and_styles() {
    use lopdf::{Dictionary, Object};
    // Build the fixture: /Dests << /chap1 [page 2 /Fit] >> and two items, one with a named
    // destination and a red bold style.
    let mut parsed = lopdf::Document::load(fixture("tracemonkey.pdf")).unwrap();
    let pages: Vec<lopdf::ObjectId> = parsed.get_pages().into_values().collect();
    let mut dests = Dictionary::new();
    dests.set(
        "chap1",
        Object::Array(vec![
            Object::Reference(pages[2]),
            Object::Name(b"Fit".to_vec()),
        ]),
    );
    let dests_id = parsed.add_object(Object::Dictionary(dests));
    let root = parsed.new_object_id();
    let (a, b) = (parsed.new_object_id(), parsed.new_object_id());
    let mut item_a = Dictionary::new();
    item_a.set("Title", Object::string_literal("Named"));
    item_a.set("Parent", Object::Reference(root));
    item_a.set("Next", Object::Reference(b));
    item_a.set("Dest", Object::string_literal("chap1"));
    item_a.set(
        "C",
        Object::Array(vec![
            Object::Real(1.0),
            Object::Real(0.0),
            Object::Real(0.0),
        ]),
    );
    item_a.set("F", Object::Integer(2));
    let mut item_b = Dictionary::new();
    item_b.set("Title", Object::string_literal("Plain"));
    item_b.set("Parent", Object::Reference(root));
    item_b.set("Prev", Object::Reference(a));
    item_b.set(
        "Dest",
        Object::Array(vec![
            Object::Reference(pages[5]),
            Object::Name(b"Fit".to_vec()),
        ]),
    );
    item_b.set(
        "C",
        Object::Array(vec![
            Object::Real(0.0),
            Object::Real(0.0),
            Object::Real(1.0),
        ]),
    );
    parsed.objects.insert(a, Object::Dictionary(item_a));
    parsed.objects.insert(b, Object::Dictionary(item_b));
    let mut outlines = Dictionary::new();
    outlines.set("Type", Object::Name(b"Outlines".to_vec()));
    outlines.set("First", Object::Reference(a));
    outlines.set("Last", Object::Reference(b));
    outlines.set("Count", Object::Integer(2));
    parsed.objects.insert(root, Object::Dictionary(outlines));
    let catalog = parsed.catalog_mut().unwrap();
    catalog.set("Outlines", Object::Reference(root));
    catalog.set("Dests", Object::Reference(dests_id));
    let mut bytes = Vec::new();
    parsed.save_to(&mut bytes).unwrap();
    let doc = reopen(bytes);

    let mut nodes = get_outline(&doc.doc_id);
    assert_eq!(nodes.len(), 2);
    assert_eq!(
        nodes[0].page,
        Some(2),
        "PDFium resolves the named destination"
    );
    nodes[1].title = "Plain, renamed".into();
    set_outline(&doc.doc_id, nodes.clone()).expect("set_outline");
    assert_eq!(get_outline(&doc.doc_id)[1].title, "Plain, renamed");

    let saved = save_as(&doc.doc_id, "outline-named.pdf");
    let reread = lopdf::Document::load_mem(&saved).unwrap();
    let item = |title: &str| {
        reread
            .objects
            .values()
            .filter_map(|o| o.as_dict().ok())
            .find(|d| {
                d.get(b"Title")
                    .ok()
                    .and_then(|t| lopdf::decode_text_string(t).ok())
                    .as_deref()
                    == Some(title)
            })
            .cloned()
            .unwrap_or_else(|| panic!("no item {title}"))
    };
    let named = item("Named");
    assert_eq!(
        named.get(b"Dest").unwrap().as_str().ok(),
        Some(&b"chap1"[..]),
        "the named destination is kept, not made explicit"
    );
    assert_eq!(floats(named.get(b"C").unwrap()), [1.0, 0.0, 0.0]);
    assert_eq!(named.get(b"F").unwrap().as_i64().unwrap(), 2);
    let renamed = item("Plain, renamed");
    assert!(!renamed.has(b"C") || floats(renamed.get(b"C").unwrap()) == [0.0, 0.0, 1.0]);
    assert!(
        renamed.get(b"Dest").unwrap().as_array().is_ok(),
        "an explicit destination"
    );
    // Round trip once more: still the same outline for PDFium.
    let again = reopen(saved);
    assert_eq!(get_outline(&again.doc_id)[0].page, Some(2));
}

/// One pixel of page `page` rendered at 100 % with annotations (`render_raw_buffer`).
fn pixel(doc_id: &str, page: u16, x: usize, y: usize) -> [u8; 3] {
    let doc_id = doc_id.to_string();
    let buffer = with_state(move |st| {
        seepdf_lib::engine::render::tiles::render_raw_buffer(st, &doc_id, page, 1.0, None)
    })
    .expect("render");
    let width = u32::from_le_bytes(buffer[8..12].try_into().unwrap()) as usize;
    let at = 32 + (y * width + x) * 4;
    [buffer[at], buffer[at + 1], buffer[at + 2]]
}

fn is_red(p: &[u8; 3]) -> bool {
    p[0] > 180 && p[1] < 100 && p[2] < 100
}

/// P5 (verification round 1): 테두리 표시 shows in SeePDF's own rendering — PDFium draws no
/// `/Border` of a link without `/AP`, so the border is written as an appearance stream — for
/// both writers (page target: lopdf, web address: PDFium); it follows a moved link and goes
/// when the border is switched off.
#[test]
fn structure_link_border_is_visible() {
    use seepdf_lib::ipc::types::LinkBorder;
    let doc = open("tracemonkey.pdf");
    let h = doc.info.pages[0].height_pt;
    let y = (h - 350.0) as usize;
    let red = LinkBorder {
        width: 3.0,
        color: [255, 0, 0],
    };
    let left_edge = |x: f32| -> Vec<[u8; 3]> {
        (0..3)
            .map(|dx| pixel(&doc.doc_id, 0, x as usize + dx, y))
            .collect()
    };
    assert!(
        !left_edge(8.0).iter().any(is_red),
        "the margin starts blank"
    );
    for (i, target) in [page_target(3, None), url_target("https://example.com")]
        .into_iter()
        .enumerate()
    {
        let x = 8.0 + 60.0 * i as f32;
        let rect = Rect::new(x, 300.0, x + 40.0, 400.0);
        let r = with_state({
            let id = doc.doc_id.clone();
            move |st| links::create_link_styled(st, &id, 0, rect, &target, &[], Some(red))
        })
        .expect("create a bordered link");
        let link = r.annot.expect("the link");
        assert_eq!(link.border_width, 3.0);
        assert!(
            left_edge(x).iter().any(is_red),
            "target {i}: the red border shows: {:?}",
            left_edge(x)
        );

        // Moved down the margin: the border moves with it and the old place is clear again.
        let moved = Rect::new(x, 200.0, x + 40.0, 280.0);
        with_state({
            let (id, link_id) = (doc.doc_id.clone(), link.id.clone());
            move |st| links::update_link(st, &id, 0, &link_id, Some(moved), None)
        })
        .expect("move");
        assert!(
            !left_edge(x).iter().any(is_red),
            "target {i}: old place clear"
        );
        let y2 = (h - 240.0) as usize;
        let at_new: Vec<[u8; 3]> = (0..3)
            .map(|dx| pixel(&doc.doc_id, 0, x as usize + dx, y2))
            .collect();
        assert!(
            at_new.iter().any(is_red),
            "target {i}: moved border {at_new:?}"
        );

        // Border off: nothing drawn, /AP gone.
        with_state({
            let (id, link_id) = (doc.doc_id.clone(), link.id.clone());
            move |st| {
                links::update_link_styled(
                    st,
                    &id,
                    0,
                    &link_id,
                    None,
                    None,
                    Some(LinkBorder {
                        width: 0.0,
                        color: [0, 0, 0],
                    }),
                )
            }
        })
        .expect("border off");
        let after: Vec<[u8; 3]> = (0..3)
            .map(|dx| pixel(&doc.doc_id, 0, x as usize + dx, y2))
            .collect();
        assert!(
            !after.iter().any(is_red),
            "target {i}: border off {after:?}"
        );
        let bytes = save_as(&doc.doc_id, "link-border-off.pdf");
        assert!(
            !link_dict(&bytes, &link.id).has(b"AP"),
            "target {i}: /AP removed"
        );
    }
}
