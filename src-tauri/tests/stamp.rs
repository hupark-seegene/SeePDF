//! Stage 4 — P1-4 watermark / header / footer (`add_stamp`, `IPC_CONTRACT.md` §7.4a).
//!
//! Assertions go through the reopened file where the claim is about what is written (text,
//! alpha, marks, the shared image XObject), and through a render where the claim is about
//! what the user sees (a header on a `/Rotate 90` page is top-left *on screen*).

mod common;
use common::*;

use pdfium_render::prelude::*;
use seepdf_lib::engine::objects;
use seepdf_lib::engine::raw;
use seepdf_lib::engine::registry;
use seepdf_lib::engine::render::tiles;
use seepdf_lib::engine::save;
use seepdf_lib::engine::stamp::{self, STAMP_MARK};
use seepdf_lib::engine::text::layer;
use seepdf_lib::ipc::types::{
    AllPages, PageObjectType, PageSelection, PageStampSource, PageStampSpec, StampAnchor,
    StampResult, StampRole,
};
use seepdf_lib::ipc::{EngineError, ErrorCode};
use std::path::PathBuf;

fn out_dir() -> PathBuf {
    let dir = fixture("out").join("stage4");
    std::fs::create_dir_all(&dir).expect("create fixtures/out/stage4");
    dir
}

fn text_spec(role: StampRole, text: &str, anchor: StampAnchor) -> PageStampSpec {
    PageStampSpec {
        role,
        source: PageStampSource::Text {
            text: text.into(),
            font_size_pt: 12.0,
            color: [200, 0, 0],
        },
        anchor,
        margin_pt: 24.0,
        rotate_deg: 0.0,
        opacity: 1.0,
        pages: PageSelection::All(AllPages::All),
        bates: Default::default(),
    }
}

fn add(doc_id: &str, spec: PageStampSpec) -> Result<StampResult, EngineError> {
    let doc_id = doc_id.to_string();
    with_state(move |st| stamp::add_stamp(st, &doc_id, &spec))
}

fn object_count(doc_id: &str, page: u16) -> usize {
    let doc_id = doc_id.to_string();
    with_doc(&doc_id, move |d| Ok(objects::list(d, page)?.objects.len())).unwrap()
}

/// (type, rect, matrix, fill alpha, marked) of the last object on the page.
fn last_object(doc_id: &str, page: u16) -> (PageObjectType, [f32; 4], [f32; 6], u8, bool) {
    let doc_id = doc_id.to_string();
    with_doc(&doc_id, move |d| {
        let list = objects::list(d, page)?;
        let last = list.objects.last().expect("an object").clone();
        let bindings = d.bindings();
        let p = d.page(page)?;
        let index = last.object_id as usize;
        let alpha = p
            .objects()
            .get(index)
            .ok()
            .and_then(|o| o.fill_color().ok())
            .map(|c| c.alpha())
            .unwrap_or(0);
        let marked = raw::object::has_mark(bindings, p, index, STAMP_MARK);
        Ok((
            last.object_type,
            [last.rect.l, last.rect.b, last.rect.r, last.rect.t],
            last.matrix,
            alpha,
            marked,
        ))
    })
    .unwrap()
}

fn page_text(doc_id: &str, page: u16) -> String {
    let doc_id = doc_id.to_string();
    with_doc(&doc_id, move |d| Ok(layer::page_text(d, page)?.text.clone())).unwrap()
}

fn save_bytes(doc_id: &str) -> Vec<u8> {
    let doc_id = doc_id.to_string();
    with_state(move |st| save::serialize(st, &doc_id)).expect("serialize")
}

fn reopen(bytes: Vec<u8>, password: Option<&str>) -> TestDoc {
    let password = password.map(str::to_owned);
    let info =
        with_state(move |st| registry::open(st, None, bytes, password)).expect("reopen");
    let doc_id = info.doc_id.clone();
    TestDoc { info, doc_id }
}

fn undo(doc_id: &str) {
    let doc_id = doc_id.to_string();
    with_state(move |st| registry::undo(st, &doc_id, false)).expect("undo");
}

/// Whole page at 1x, RGBA, as displayed (`/Rotate` applied).
fn render(doc_id: &str, page: u16) -> (u32, u32, Vec<u8>) {
    let doc_id = doc_id.to_string();
    let buffer = with_state(move |st| tiles::render_raw_buffer(st, &doc_id, page, 1.0, None))
        .expect("render page");
    let width = u32::from_le_bytes(buffer[8..12].try_into().unwrap());
    let height = u32::from_le_bytes(buffer[12..16].try_into().unwrap());
    (width, height, buffer[32..].to_vec())
}

/// Reddish pixels (the stamp colour) inside `[x0, x1) × [y0, y1)` given as fractions.
fn red_pixels(img: &(u32, u32, Vec<u8>), x0: f32, x1: f32, y0: f32, y1: f32) -> usize {
    let (w, h, px) = img;
    let (xa, xb) = ((x0 * *w as f32) as u32, (x1 * *w as f32) as u32);
    let (ya, yb) = ((y0 * *h as f32) as u32, (y1 * *h as f32) as u32);
    let mut n = 0;
    for y in ya..yb {
        for x in xa..xb {
            let o = ((y * w + x) * 4) as usize;
            let (r, g, b) = (px[o] as i32, px[o + 1] as i32, px[o + 2] as i32);
            if r > 120 && r - g > 60 && r - b > 60 {
                n += 1;
            }
        }
    }
    n
}

/// A deterministic noisy RGB PNG (does not compress to nothing).
fn test_png(name: &str, w: u32, h: u32) -> PathBuf {
    let path = out_dir().join(name);
    let mut img = image::RgbImage::new(w, h);
    let mut seed: u32 = 0x1234_5678;
    for (x, y, px) in img.enumerate_pixels_mut() {
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        let noise = (seed >> 24) as u8;
        *px = image::Rgb([
            (x * 255 / w) as u8 ^ (noise & 0x3f),
            (y * 255 / h) as u8,
            noise,
        ]);
    }
    img.save(&path).expect("write test png");
    path
}

#[test]
fn stamp_text_watermark_centred_and_rotated() {
    let doc = open("tracemonkey.pdf");
    let before: Vec<usize> = (0..doc.info.page_count).map(|p| object_count(&doc.doc_id, p)).collect();
    let mut spec = text_spec(StampRole::Watermark, "DRAFT 초안", StampAnchor::Mc);
    spec.source = PageStampSource::Text {
        text: "DRAFT 초안".into(),
        font_size_pt: 72.0,
        color: [200, 0, 0],
    };
    spec.rotate_deg = 45.0;
    spec.opacity = 0.3;
    let result = add(&doc.doc_id, spec).expect("add_stamp");
    assert_eq!(result.pages_stamped, doc.info.page_count as u32);
    assert_eq!(result.info.undo_label.as_deref(), Some("undo.watermark"));

    let geom = &doc.info.pages[0];
    for p in 0..doc.info.page_count {
        assert_eq!(object_count(&doc.doc_id, p), before[p as usize] + 1, "page {p}");
    }
    let (kind, rect, m, alpha, marked) = last_object(&doc.doc_id, 0);
    assert_eq!(kind, PageObjectType::Text);
    assert!(marked, "the stamp carries the {STAMP_MARK} mark");
    assert_eq!(alpha, 77, "opacity 0.3 → fill alpha 77");
    let s = std::f32::consts::FRAC_1_SQRT_2;
    assert!((m[0] - s).abs() < 1e-3 && (m[1] - s).abs() < 1e-3, "rotated 45° CCW: {m:?}");
    let (cx, cy) = ((rect[0] + rect[2]) / 2.0, (rect[1] + rect[3]) / 2.0);
    // Glyph bounds are not the layout box (descenders, side bearings): allow a few points.
    assert!((cx - geom.width_pt / 2.0).abs() < 8.0, "centre x {cx}");
    assert!((cy - geom.height_pt / 2.0).abs() < 8.0, "centre y {cy}");

    // The file keeps the alpha, the mark and the Hangul text.
    let reopened = reopen(save_bytes(&doc.doc_id), None);
    let (_, _, _, alpha, marked) = last_object(&reopened.doc_id, 3);
    assert_eq!(alpha, 77);
    assert!(marked);
    assert!(page_text(&reopened.doc_id, 3).contains("DRAFT 초안"));
    // 30 % of (200, 0, 0) over white is ≈ (241, 178, 178): translucent red, not opaque.
    let img = render(&reopened.doc_id, 0);
    assert!(red_pixels(&img, 0.3, 0.7, 0.3, 0.7) > 200, "the watermark is visible");
    let opaque = img.2.chunks_exact(4).filter(|p| p[0] > 150 && p[1] < 60).count();
    assert_eq!(opaque, 0, "no opaque red pixels at opacity 0.3");
}

#[test]
fn stamp_header_tokens_on_every_page() {
    let doc = open("tracemonkey.pdf");
    let total = doc.info.page_count;
    let spec = text_spec(
        StampRole::Header,
        "{{filename}} — {{page}} / {{total}} · {{date}} {{nope}}",
        StampAnchor::Tl,
    );
    let result = add(&doc.doc_id, spec).expect("add_stamp");
    assert_eq!(result.info.undo_label.as_deref(), Some("undo.headerFooter"));
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    for p in [0u16, 5, total - 1] {
        let text = page_text(&doc.doc_id, p);
        let want = format!("tracemonkey — {} / {total} · {today} {{{{nope}}}}", p + 1);
        assert!(text.contains(&want), "page {p}: {want:?} not in the page text");
        let (_, rect, _, alpha, marked) = last_object(&doc.doc_id, p);
        assert!(marked);
        assert_eq!(alpha, 255);
        let geom = &doc.info.pages[p as usize];
        assert!((rect[0] - 24.0).abs() < 4.0, "left edge {}", rect[0]);
        assert!(rect[3] <= geom.height_pt - 24.0 + 1.0 && rect[3] > geom.height_pt - 40.0);
    }
    let footer = text_spec(StampRole::Footer, "{{page}}", StampAnchor::Bc);
    add(&doc.doc_id, footer).expect("footer");
    let (_, rect, _, _, _) = last_object(&doc.doc_id, 1);
    let geom = &doc.info.pages[1];
    assert!(((rect[0] + rect[2]) / 2.0 - geom.width_pt / 2.0).abs() < 2.0);
    assert!(rect[1] >= 24.0 - 1.0 && rect[1] < 40.0, "footer bottom {}", rect[1]);
}

#[test]
fn stamp_honours_page_rotation() {
    let doc = open("rotation.pdf");
    assert_eq!(doc.info.pages[1].rotation, 90);
    let mut spec = text_spec(StampRole::Header, "TOP-LEFT HEADER", StampAnchor::Tl);
    spec.source = PageStampSource::Text {
        text: "TOP-LEFT HEADER".into(),
        font_size_pt: 18.0,
        color: [220, 0, 0],
    };
    spec.pages = PageSelection::List(vec![1]);
    let before = render(&doc.doc_id, 1);
    add(&doc.doc_id, spec).expect("add_stamp");
    let after = render(&doc.doc_id, 1);
    assert_eq!((before.0, before.1), (after.0, after.1));
    let added = |x0, x1, y0, y1| {
        red_pixels(&after, x0, x1, y0, y1) as i64 - red_pixels(&before, x0, x1, y0, y1) as i64
    };
    // Rendered as displayed: the new red ink is in the top-left quadrant and nowhere else.
    let tl = added(0.0, 0.5, 0.0, 0.25);
    assert!(tl > 50, "top-left gained {tl} red pixels");
    assert!(added(0.5, 1.0, 0.0, 1.0) < 5 && added(0.0, 0.5, 0.5, 1.0) < 5);
    // Upright as seen: the text runs along the user-space y axis on a /Rotate 90 page.
    let (_, _, m, _, _) = last_object(&doc.doc_id, 1);
    assert!(m[0].abs() < 1e-3 && (m[1] - 1.0).abs() < 1e-3, "matrix {m:?}");
    // Page 0 untouched.
    assert_eq!(result_pages(&doc.doc_id), 1);
}

fn result_pages(doc_id: &str) -> usize {
    // Pages with a stamp-marked last object.
    let doc_id = doc_id.to_string();
    let count = with_doc(&doc_id, |d| Ok(d.page_count())).unwrap();
    (0..count)
        .filter(|&p| object_count(&doc_id, p) > 0 && last_object(&doc_id, p).4)
        .count()
}

#[test]
fn stamp_image_subset_shared_xobject_and_undo() {
    let doc = open("tracemonkey.pdf");
    let png = test_png("stamp-logo.png", 256, 128);
    let before0 = object_count(&doc.doc_id, 0);
    let before1 = object_count(&doc.doc_id, 1);
    let spec = PageStampSpec {
        role: StampRole::Watermark,
        source: PageStampSource::Image {
            path: png.display().to_string(),
            width_pt: 120.0,
        },
        anchor: StampAnchor::Br,
        margin_pt: 36.0,
        rotate_deg: 0.0,
        opacity: 0.5,
        pages: PageSelection::List(vec![2, 0]),
        bates: Default::default(),
    };
    let result = add(&doc.doc_id, spec.clone()).expect("image stamp");
    assert_eq!(result.pages_stamped, 2);
    assert_eq!(object_count(&doc.doc_id, 0), before0 + 1);
    assert_eq!(object_count(&doc.doc_id, 1), before1, "page 1 was not selected");
    let (kind, rect, _, _, marked) = last_object(&doc.doc_id, 2);
    assert_eq!(kind, PageObjectType::Form);
    assert!(marked);
    let geom = &doc.info.pages[2];
    assert!((rect[2] - (geom.width_pt - 36.0)).abs() < 1.0, "right edge {}", rect[2]);
    assert!((rect[1] - 36.0).abs() < 1.0 && (rect[3] - 96.0).abs() < 1.0, "{rect:?}");

    // One undo step restores the object counts.
    undo(&doc.doc_id);
    assert_eq!(object_count(&doc.doc_id, 0), before0);

    // The image is stored once however many pages carry it.
    let base = save_bytes(&doc.doc_id).len() as i64;
    let mut one = spec.clone();
    one.pages = PageSelection::List(vec![0]);
    add(&doc.doc_id, one).unwrap();
    let grew_one = save_bytes(&doc.doc_id).len() as i64 - base;
    undo(&doc.doc_id);
    let mut all = spec;
    all.pages = PageSelection::All(AllPages::All);
    add(&doc.doc_id, all).unwrap();
    let bytes = save_bytes(&doc.doc_id);
    let grew_all = bytes.len() as i64 - base;
    assert!(grew_one > 10_000, "the image is embedded ({grew_one} bytes)");
    assert!(
        grew_all < grew_one * 2,
        "14 pages grew {grew_all} bytes vs {grew_one} for one page"
    );
    std::fs::write(out_dir().join("stamp-image-all.pdf"), &bytes).unwrap();

    let reopened = reopen(bytes, None);
    let (_, _, _, _, marked) = last_object(&reopened.doc_id, 13);
    assert!(marked);
}

#[test]
fn stamp_rejects_bad_input() {
    let doc = open("tracemonkey.pdf");
    let code = |spec: PageStampSpec| add(&doc.doc_id, spec).unwrap_err().code;
    assert_eq!(
        code(text_spec(StampRole::Watermark, "  \n ", StampAnchor::Mc)),
        ErrorCode::InvalidArgument
    );
    let mut s = text_spec(StampRole::Watermark, "x", StampAnchor::Mc);
    s.pages = PageSelection::List(vec![99]);
    assert_eq!(code(s), ErrorCode::InvalidArgument);
    let mut s = text_spec(StampRole::Watermark, "x", StampAnchor::Mc);
    s.opacity = 1.5;
    assert_eq!(code(s), ErrorCode::InvalidArgument);
    let mut s = text_spec(StampRole::Watermark, "x", StampAnchor::Mc);
    s.rotate_deg = 270.0;
    assert_eq!(code(s), ErrorCode::InvalidArgument);
    let mut s = text_spec(StampRole::Watermark, "x", StampAnchor::Mc);
    s.source = PageStampSource::Image {
        path: "/nonexistent/logo.png".into(),
        width_pt: 100.0,
    };
    assert_eq!(code(s), ErrorCode::NotFound);
    // Nothing was pushed onto the history by any of the refusals.
    let can_undo = with_doc(&doc.doc_id, |d| Ok(d.history.can_undo())).unwrap();
    assert!(!can_undo);
}

#[test]
fn stamp_encrypted_document_keeps_its_password() {
    let doc = try_open("gen/encrypted-rc4-40.pdf", Some("user")).expect("open");
    add(&doc.doc_id, text_spec(StampRole::Footer, "Page {{page}}", StampAnchor::Br))
        .expect("stamp on an encrypted document");
    let bytes = save_bytes(&doc.doc_id);
    let reopened = reopen(bytes.clone(), Some("user"));
    assert!(page_text(&reopened.doc_id, 0).contains("Page 1"));
    let without = with_state(move |st| registry::open(st, None, bytes, None).map(|i| i.doc_id));
    assert_eq!(without.unwrap_err().code, ErrorCode::PasswordRequired);
}

// ---------------------------------------------------------------------------------------
// Stage 8 — remove_stamps, the role param, rotated stamps inside the margin
// ---------------------------------------------------------------------------------------

fn remove(
    doc_id: &str,
    pages: Option<PageSelection>,
    role: Option<StampRole>,
) -> seepdf_lib::ipc::types::RemoveStampsResult {
    let doc_id = doc_id.to_string();
    with_state(move |st| stamp::remove_stamps(st, &doc_id, pages.as_ref(), role))
        .expect("remove_stamps")
}

fn counts(doc_id: &str, pages: u16) -> Vec<usize> {
    (0..pages).map(|p| object_count(doc_id, p)).collect()
}

/// A pre-Stage 8 stamp: a text object carrying the bare `SeePDF:Stamp` mark (no `role`).
fn add_legacy_stamp(doc_id: &str, page: u16) {
    use seepdf_lib::engine::annot::ScratchPage;
    use seepdf_lib::engine::raw::object::Mark;
    use seepdf_lib::engine::registry::MutateOpts;
    use seepdf_lib::ipc::types::{ChangeReason, Rect, TextAlign};
    let id = doc_id.to_string();
    with_state(move |st| {
        objects::add_text(
            st,
            &id,
            page,
            Rect::new(72.0, 300.0, 400.0, 340.0),
            "LEGACY STAMP",
            18.0,
            [0, 0, 200],
            TextAlign::Left,
        )?;
        registry::mutate(
            st,
            &id,
            MutateOpts::new("undo.objectEdit", ChangeReason::Edit).page(page),
            |doc| {
                let bindings = doc.bindings();
                let mut scratch = ScratchPage::open(doc, page)?;
                let last = raw::object::object_count(bindings, &scratch.page) - 1;
                raw::object::add_mark(bindings, doc.pdf(), &scratch.page, last, Mark::plain(STAMP_MARK))?;
                scratch.page.regenerate_content().unwrap();
                Ok(())
            },
        )
    })
    .expect("legacy stamp");
}

#[test]
fn remove_stamps_by_role_and_all_with_undo() {
    let doc = open("tracemonkey.pdf");
    let n = doc.info.page_count;
    let base = counts(&doc.doc_id, n);

    let mut watermark = text_spec(StampRole::Watermark, "DRAFT", StampAnchor::Mc);
    watermark.rotate_deg = 45.0;
    add(&doc.doc_id, watermark).expect("watermark");
    let mut header = text_spec(StampRole::Header, "p. {{page}}", StampAnchor::Tc);
    header.pages = PageSelection::List(vec![0, 2]);
    add(&doc.doc_id, header).expect("header");
    add(
        &doc.doc_id,
        text_spec(StampRole::Footer, "footer one\nfooter two", StampAnchor::Bc),
    )
    .expect("footer");
    add_legacy_stamp(&doc.doc_id, 1);
    let stamped = counts(&doc.doc_id, n);
    for p in 0..n as usize {
        let header = if p == 0 || p == 2 { 1 } else { 0 };
        let legacy = if p == 1 { 1 } else { 0 };
        assert_eq!(stamped[p], base[p] + 1 + header + 2 + legacy, "page {p}");
    }

    // The role survives serialisation: a reopened copy filters by role too.
    let copy = reopen(save_bytes(&doc.doc_id), None);
    assert_eq!(remove(&copy.doc_id, None, Some(StampRole::Header)).removed, 2);

    // One role.
    let r = remove(&doc.doc_id, None, Some(StampRole::Header));
    assert_eq!(r.removed, 2);
    assert_eq!(r.info.undo_label.as_deref(), Some("undo.removeStamps"));
    assert_eq!(object_count(&doc.doc_id, 0), stamped[0] - 1);
    assert_eq!(object_count(&doc.doc_id, 1), stamped[1]);
    // One role on a page subset.
    let r = remove(&doc.doc_id, Some(PageSelection::List(vec![1])), Some(StampRole::Watermark));
    assert_eq!(r.removed, 1);
    assert_eq!(object_count(&doc.doc_id, 1), stamped[1] - 1);
    // Nothing left of a role: not an error, no undo step, same generation.
    let generation = r.info.doc_generation;
    let none = remove(&doc.doc_id, None, Some(StampRole::Header));
    assert_eq!(none.removed, 0);
    assert_eq!(none.info.doc_generation, generation);
    assert_eq!(none.info.undo_label.as_deref(), Some("undo.removeStamps"));
    // The legacy stamp has no role: only an all-roles removal takes it.
    let r = remove(&doc.doc_id, None, Some(StampRole::Footer));
    assert_eq!(r.removed, 2 * n as u32);
    assert_eq!(object_count(&doc.doc_id, 1), stamped[1] - 1 - 2);
    let before_all = counts(&doc.doc_id, n);
    let r = remove(&doc.doc_id, None, None);
    assert_eq!(r.removed, (n - 1) as u32 + 1, "the other watermarks and the legacy stamp");
    assert_eq!(counts(&doc.doc_id, n), base);
    assert!(!page_text(&doc.doc_id, 1).contains("LEGACY STAMP"));
    assert!(!page_text(&doc.doc_id, 0).contains("DRAFT"));

    // One undo step brings exactly that removal back.
    undo(&doc.doc_id);
    assert_eq!(counts(&doc.doc_id, n), before_all);
    assert!(page_text(&doc.doc_id, 1).contains("LEGACY STAMP"));
}

/// Stage 8: a rotated stamp is anchored by its rotated bounding box, so at every corner and
/// edge of every page (a `/Rotate 90` one included) its ink stays inside the page — inside
/// the margin, give or take the glyphs' side bearings.
#[test]
fn rotated_stamp_stays_inside_the_page_at_every_corner() {
    let margin = 12.0;
    for fixture_name in ["tracemonkey.pdf", "rotation.pdf"] {
        for anchor in [
            StampAnchor::Tl,
            StampAnchor::Tc,
            StampAnchor::Tr,
            StampAnchor::Ml,
            StampAnchor::Mr,
            StampAnchor::Bl,
            StampAnchor::Bc,
            StampAnchor::Br,
        ] {
            for deg in [45.0, -30.0, 90.0] {
                let doc = open(fixture_name);
                let pages: Vec<u16> = (0..doc.info.page_count.min(2)).collect();
                let mut spec = text_spec(StampRole::Watermark, "CONFIDENTIAL 대외비", anchor);
                spec.source = PageStampSource::Text {
                    text: "CONFIDENTIAL 대외비".into(),
                    font_size_pt: 48.0,
                    color: [200, 0, 0],
                };
                spec.margin_pt = margin;
                spec.rotate_deg = deg;
                spec.pages = PageSelection::List(pages.clone());
                add(&doc.doc_id, spec).expect("add_stamp");
                for &p in &pages {
                    let crop = doc.info.pages[p as usize].crop;
                    let (kind, rect, _, _, marked) = last_object(&doc.doc_id, p);
                    assert!(marked && kind == PageObjectType::Text);
                    let slack = 3.0;
                    assert!(
                        rect[0] >= crop.l + margin - slack
                            && rect[1] >= crop.b + margin - slack
                            && rect[2] <= crop.r - margin + slack
                            && rect[3] <= crop.t - margin + slack,
                        "{fixture_name} p{p} {anchor:?} {deg}°: {rect:?} outside {crop:?} − {margin}"
                    );
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------------------
// P2 — Bates numbering (`{{bates}}`)
// ---------------------------------------------------------------------------------------

/// 14 pages, `ABC` + 6 digits from 101: every page carries its own number, as extractable
/// text, in the saved file too; a range counts only the stamped pages; `remove_stamps` with
/// the footer role takes the numbers off again.
#[test]
fn stamp_bates_numbers_every_page_and_is_extractable() {
    use seepdf_lib::ipc::types::BatesOptions;
    let doc = open("tracemonkey.pdf");
    assert_eq!(doc.info.page_count, 14);
    let mut spec = text_spec(StampRole::Footer, "{{bates}}", StampAnchor::Br);
    spec.bates = BatesOptions {
        bates_start: 101,
        bates_digits: 6,
        bates_prefix: "ABC".into(),
        bates_suffix: String::new(),
    };
    let result = add(&doc.doc_id, spec).expect("add_stamp with {{bates}}");
    assert_eq!(result.pages_stamped, 14);
    assert_eq!(result.info.undo_label.as_deref(), Some("undo.headerFooter"));
    for p in 0..14u16 {
        let text = page_text(&doc.doc_id, p);
        let expected = format!("ABC{:06}", 101 + p as u32);
        assert!(text.contains(&expected), "page {}: {expected} missing", p + 1);
        // exactly one Bates number per page
        assert_eq!(text.matches("ABC000").count(), 1, "page {}", p + 1);
    }
    let reopened = reopen(save_bytes(&doc.doc_id), None);
    assert!(page_text(&reopened.doc_id, 0).contains("ABC000101"));
    assert!(page_text(&reopened.doc_id, 13).contains("ABC000114"));

    // a range counts the stamped pages: 3, 4, 5 get 000001 … 000003 (suffix kept)
    let other = open("tracemonkey.pdf");
    let mut ranged = text_spec(StampRole::Header, "Exhibit {{bates}}", StampAnchor::Tr);
    ranged.pages = PageSelection::List(vec![2, 3, 4]);
    ranged.bates = BatesOptions {
        bates_suffix: "-K".into(),
        ..BatesOptions::default()
    };
    add(&other.doc_id, ranged).expect("ranged bates");
    assert!(page_text(&other.doc_id, 2).contains("Exhibit 000001-K"));
    assert!(page_text(&other.doc_id, 4).contains("Exhibit 000003-K"));
    assert!(!page_text(&other.doc_id, 5).contains("Exhibit"));

    // remove_stamps with the role takes the numbers off (role support unchanged)
    let removed = with_state({
        let id = doc.doc_id.clone();
        move |st| stamp::remove_stamps(st, &id, None, Some(StampRole::Footer))
    })
    .expect("remove footers");
    assert_eq!(removed.removed, 14);
    assert!(!page_text(&doc.doc_id, 0).contains("ABC000101"));

    // bad options are refused before anything changes
    let mut bad = text_spec(StampRole::Footer, "{{bates}}", StampAnchor::Br);
    bad.bates.bates_digits = 0;
    assert_eq!(add(&doc.doc_id, bad).unwrap_err().code, ErrorCode::InvalidArgument);
    let mut bad = text_spec(StampRole::Footer, "{{bates}}", StampAnchor::Br);
    bad.bates.bates_prefix = "x".repeat(65);
    assert_eq!(add(&doc.doc_id, bad).unwrap_err().code, ErrorCode::InvalidArgument);
}
