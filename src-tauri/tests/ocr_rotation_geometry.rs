//! v0.3 pkg7-ocr (verification round 2): the O2 `setRotation` layer lands on the ink.
//!
//! The same upright OCR result is applied to (a) the upright scan and (b) a scan that needs a
//! turn — including one whose page already carries a `/Rotate` — with `setRotation`. Seen through
//! the page's display transform, a search hit must sit where it sits on the upright page.

mod common;
use common::*;

use seepdf_lib::engine::ocr::{self, orientation, GrayPage};
use seepdf_lib::engine::registry;
use seepdf_lib::engine::text::{layer, search};
use seepdf_lib::ipc::types::{BlankPageSize, OcrLine, OcrPage, OcrWord, PageOp, PageSizePt, Rect};
use std::path::PathBuf;

const DPI: u32 = 300;
fn out_dir() -> PathBuf {
    let dir = fixture("out").join("v03-ocr-geom");
    std::fs::create_dir_all(&dir).expect("create fixtures/out/v03-ocr-geom");
    dir
}

fn reopen(bytes: Vec<u8>) -> TestDoc {
    let info = with_state(move |st| registry::open(st, None, bytes, None)).expect("reopen");
    let doc_id = info.doc_id.clone();
    TestDoc { info, doc_id }
}

fn save_bytes(doc_id: &str) -> Vec<u8> {
    let doc_id = doc_id.to_string();
    with_state(move |st| seepdf_lib::engine::save::serialize(st, &doc_id)).expect("serialize")
}

fn page_text(doc_id: &str, page: u16) -> String {
    let doc_id = doc_id.to_string();
    with_doc(&doc_id, move |doc| {
        Ok(layer::page_text(doc, page)?.text.clone())
    })
    .expect("page text")
}

fn gray(doc_id: &str, page: u16, dpi: u32) -> GrayPage {
    let doc_id = doc_id.to_string();
    with_state(move |st| ocr::render_page_gray(st, &doc_id, page, dpi)).expect("render gray")
}

fn rotation_of(doc_id: &str, page: u16) -> u16 {
    with_doc(doc_id, move |d| Ok(d.geom(page)?.rotation)).expect("geometry")
}

/// A one-page image-only PDF of page 0 of `name`, rendered at `dpi` and turned `turn` degrees
/// clockwise — what a scanner that fed the sheet sideways produces.
fn scan_of(name: &str, dpi: u32, turn: u16) -> TestDoc {
    let source = open(name);
    let image = orientation::rotate_gray(&gray(&source.doc_id, 0, dpi), turn);
    // One file per call: tests run in parallel and several scan the same fixture.
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let png = out_dir().join(format!(
        "scan-{}-{dpi}dpi-{turn}-{n}.png",
        name.replace(['/', '.'], "-")
    ));
    image::GrayImage::from_raw(image.width, image.height, image.pixels.clone())
        .expect("gray buffer")
        .save(&png)
        .expect("write the scan PNG");
    let (w_pt, h_pt) = (
        image.width as f32 * 72.0 / dpi as f32,
        image.height as f32 * 72.0 / dpi as f32,
    );

    let host = open("tracemonkey.pdf");
    let host_id = host.doc_id.clone();
    with_state(move |st| {
        seepdf_lib::engine::pages::apply_ops(
            st,
            &host_id,
            vec![
                PageOp::InsertBlank {
                    at: 0,
                    size: BlankPageSize::Explicit(PageSizePt {
                        width_pt: w_pt,
                        height_pt: h_pt,
                    }),
                },
                PageOp::Delete {
                    pages: (1..15).collect(),
                },
            ],
        )
    })
    .expect("a blank page of the scan's size");
    let host_id = host.doc_id.clone();
    let path = png.display().to_string();
    with_state(move |st| {
        seepdf_lib::engine::objects::add_image(
            st,
            &host_id,
            0,
            Rect::new(0.0, 0.0, w_pt, h_pt),
            &path,
            false,
        )
    })
    .expect("place the scan");
    let scan = reopen(save_bytes(&host.doc_id));
    assert!(
        page_text(&scan.doc_id, 0).trim().is_empty(),
        "an image-only page"
    );
    scan
}

fn word(text: &str, bbox: [f32; 4]) -> OcrWord {
    OcrWord {
        text: text.to_string(),
        bbox,
        confidence: 92.0,
    }
}

fn line(words: Vec<OcrWord>) -> OcrLine {
    let bbox = [
        words.iter().map(|w| w.bbox[0]).fold(f32::MAX, f32::min),
        words.iter().map(|w| w.bbox[1]).fold(f32::MAX, f32::min),
        words.iter().map(|w| w.bbox[2]).fold(f32::MIN, f32::max),
        words.iter().map(|w| w.bbox[3]).fold(f32::MIN, f32::max),
    ];
    OcrLine {
        text: words
            .iter()
            .map(|w| w.text.as_str())
            .collect::<Vec<_>>()
            .join(" "),
        bbox,
        baseline: None,
        row_height_px: Some(bbox[3] - bbox[1]),
        words,
    }
}

/// What a recogniser would report for the upright Korean fixture (`gen/korean-300dpi.pdf`),
/// written by hand so the O5 comparison does not depend on a recogniser.
fn korean_ocr(page_rotation: u16, width_px: u32, height_px: u32) -> OcrPage {
    OcrPage {
        page: 0,
        dpi: DPI,
        width_px,
        height_px,
        rotation: page_rotation,
        lines: vec![
            line(vec![
                word("검색", [267.0, 240.0, 467.0, 340.0]),
                word("가능한", [500.0, 240.0, 800.0, 340.0]),
                word("한글", [833.0, 240.0, 1033.0, 340.0]),
                word("문서", [1066.0, 240.0, 1266.0, 340.0]),
            ]),
            line(vec![
                word("SeePDF", [267.0, 420.0, 520.0, 480.0]),
                word("OCR", [540.0, 420.0, 660.0, 480.0]),
                word("정확도", [680.0, 420.0, 850.0, 480.0]),
                word("측정용", [870.0, 420.0, 1040.0, 480.0]),
            ]),
            line(vec![
                word("日本語の", [267.0, 560.0, 500.0, 620.0]),
                word("中文", [520.0, 560.0, 640.0, 620.0]),
                word("2026년", [660.0, 560.0, 860.0, 620.0]),
            ]),
        ],
    }
}

fn apply(doc_id: &str, pages: Vec<OcrPage>, rotations: Vec<Option<u16>>) {
    let doc_id = doc_id.to_string();
    with_state(move |st| {
        let mut progress = |_done: usize, _page: u16| {};
        ocr::apply_rotated_cancellable(
            st,
            &doc_id,
            &pages,
            &rotations,
            false,
            &mut progress,
            &|| false,
        )
    })
    .expect("ocr_apply");
}

/// The page's mediabox size in points (unrotated).
fn media_size(doc_id: &str, page: u16) -> (f32, f32) {
    let crop = with_doc(doc_id, move |d| Ok(d.geom(page)?.crop)).expect("geometry");
    assert!(
        crop.l.abs() < 0.01 && crop.b.abs() < 0.01,
        "crop at the origin: {crop:?}"
    );
    (crop.r - crop.l, crop.t - crop.b)
}

/// A hit rect (user space) → the upright display space of a page with `/Rotate rotation`.
fn to_display(r: &Rect, rotation: u16, wu: f32, hu: f32) -> Rect {
    let pts = [(r.l, r.b), (r.r, r.t)];
    let map = |(x, y): (f32, f32)| match rotation % 360 {
        90 => (y, wu - x),
        180 => (wu - x, hu - y),
        270 => (hu - y, x),
        _ => (x, y),
    };
    let (a, b) = (map(pts[0]), map(pts[1]));
    Rect::new(a.0.min(b.0), a.1.min(b.1), a.0.max(b.0), a.1.max(b.1))
}

/// The first hit of `needle`, as the union of its rects mapped to display space. (On a page
/// turned a quarter, the search reports one rect per character: the line grouping works in
/// user space, where the characters are stacked.)
fn first_hit(doc_id: &str, needle: &str, rotation: u16, wu: f32, hu: f32) -> Rect {
    let query = search::Query::new(needle, false, false).expect("query");
    let hits = with_doc(doc_id, move |d| search::search_page(d, 0, &query)).expect("search");
    let rects: Vec<Rect> = hits
        .first()
        .expect("a hit")
        .rects
        .iter()
        .map(|r| to_display(r, rotation, wu, hu))
        .collect();
    Rect::new(
        rects.iter().map(|r| r.l).fold(f32::MAX, f32::min),
        rects.iter().map(|r| r.b).fold(f32::MAX, f32::min),
        rects.iter().map(|r| r.r).fold(f32::MIN, f32::max),
        rects.iter().map(|r| r.t).fold(f32::MIN, f32::max),
    )
}

fn rotate_page(doc_id: &str, delta: u16) {
    let id = doc_id.to_string();
    with_state(move |st| {
        seepdf_lib::engine::pages::apply_ops(
            st,
            &id,
            vec![PageOp::Rotate {
                pages: vec![0],
                delta,
            }],
        )
    })
    .expect("rotate");
}

/// `(scan turn, extra /Rotate already on the page, setRotation)`.
fn check(turn: u16, existing: u16, set: u16) {
    let upright = scan_of("gen/korean-300dpi.pdf", 150, 0);
    let img = gray(&upright.doc_id, 0, DPI);
    apply(
        &upright.doc_id,
        vec![korean_ocr(0, img.width, img.height)],
        vec![None],
    );
    let (uw, uh) = media_size(&upright.doc_id, 0);
    let expected: Vec<Rect> = ["한글", "SeePDF", "정확도"]
        .iter()
        .map(|n| first_hit(&upright.doc_id, n, 0, uw, uh))
        .collect();

    let scan = scan_of("gen/korean-300dpi.pdf", 150, turn);
    if existing != 0 {
        rotate_page(&scan.doc_id, existing);
    }
    let (wu, hu) = media_size(&scan.doc_id, 0);
    apply(
        &scan.doc_id,
        vec![korean_ocr(set, img.width, img.height)],
        vec![Some(set)],
    );
    assert_eq!(rotation_of(&scan.doc_id, 0), set);
    for (needle, want) in ["한글", "SeePDF", "정확도"].iter().zip(&expected) {
        let got = first_hit(&scan.doc_id, needle, set, wu, hu);
        let off = (got.l - want.l)
            .abs()
            .max((got.b - want.b).abs())
            .max((got.r - want.r).abs())
            .max((got.t - want.t).abs());
        assert!(off < 2.0, "{needle}: want {want:?} got {got:?}");
    }
}

#[test]
fn ocr_layer_lands_on_the_ink_set_rotation_90_from_upright_page() {
    check(270, 0, 90);
}

#[test]
fn ocr_layer_lands_on_the_ink_set_rotation_180_from_upright_page() {
    check(180, 0, 180);
}

#[test]
fn ocr_layer_lands_on_the_ink_set_rotation_270_from_upright_page() {
    check(90, 0, 270);
}

/// A page already at `/Rotate 90` that still reads upside down: the tesseract path turns 90 more.
#[test]
fn ocr_layer_lands_on_the_ink_set_rotation_on_a_page_that_already_has_rotate() {
    check(180, 90, 180);
}
