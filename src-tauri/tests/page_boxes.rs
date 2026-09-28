//! P2 — crop and resize pages (`set_page_boxes`, `resize_pages`; `IPC_CONTRACT.md` §7.3a).
//!
//! Claims about what is *written* are checked on the saved-and-reopened file; claims about
//! where the content ends up are checked on the page objects and the text layer, which PDFium
//! reports in page space after the content transform.

mod common;
use common::*;

use pdfium_render::prelude::*;
use seepdf_lib::engine::annot;
use seepdf_lib::engine::objects;
use seepdf_lib::engine::pages::boxes;
use seepdf_lib::engine::registry::{self, MutateOpts};
use seepdf_lib::engine::render::tiles;
use seepdf_lib::engine::stamp;
use seepdf_lib::engine::text::layer;
use seepdf_lib::ipc::types::{
    AllPages, AnnotKind, AnnotSpec, ChangeReason, CropSpec, DocInfo, InkSpec, Margins, MarkupSpec,
    PageObjectType, PageSelection, PageStampSource, PageStampSpec, PaperName, Rect, ResizeMode,
    ResizeTarget, StampAnchor, StampRole,
};
use seepdf_lib::ipc::{EngineError, ErrorCode};

const A4: (f32, f32) = (595.28, 841.89);
const TOL: f32 = 0.6;

fn near(a: f32, b: f32, tol: f32) -> bool {
    (a - b).abs() <= tol
}

fn assert_rect(actual: Rect, expected: Rect, tol: f32, what: &str) {
    assert!(
        near(actual.l, expected.l, tol)
            && near(actual.b, expected.b, tol)
            && near(actual.r, expected.r, tol)
            && near(actual.t, expected.t, tol),
        "{what}: {actual:?} != {expected:?}"
    );
}

fn list(pages: &[u16]) -> PageSelection {
    PageSelection::List(pages.to_vec())
}

fn all() -> PageSelection {
    PageSelection::All(AllPages::All)
}

fn crop(
    doc_id: &str,
    pages: PageSelection,
    crop: Option<Option<CropSpec>>,
    media: Option<Option<Rect>>,
) -> Result<DocInfo, EngineError> {
    let doc_id = doc_id.to_string();
    with_state(move |st| boxes::set_page_boxes(st, &doc_id, &pages, crop, media))
}

fn resize(
    doc_id: &str,
    pages: PageSelection,
    size: ResizeTarget,
    mode: ResizeMode,
) -> Result<DocInfo, EngineError> {
    let doc_id = doc_id.to_string();
    with_state(move |st| boxes::resize_pages(st, &doc_id, &pages, size, mode))
}

fn undo(doc_id: &str) -> DocInfo {
    let doc_id = doc_id.to_string();
    with_state(move |st| registry::undo(st, &doc_id, false)).expect("undo")
}

/// The pre-save appearance pass plus `save_to_bytes()` — what `save_document` writes.
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

/// `(crop box, media box)` as the page dictionary stores them.
fn boxes_of(doc_id: &str, page: u16) -> (Rect, Rect) {
    with_doc(doc_id, move |d| {
        let p = d.page(page)?;
        let read = |b: Result<PdfPageBoundaryBox, PdfiumError>| {
            let r = b.expect("box").bounds;
            Rect::new(
                r.left().value,
                r.bottom().value,
                r.right().value,
                r.top().value,
            )
        };
        Ok((read(p.boundaries().crop()), read(p.boundaries().media())))
    })
    .unwrap()
}

/// Width, height and number of non-white pixels of a render at `scale`.
fn render_ink(doc_id: &str, page: u16, scale: f32) -> (u32, u32, usize) {
    let doc_id = doc_id.to_string();
    let buffer =
        with_state(move |st| tiles::render_raw_buffer(st, &doc_id, page, scale, None)).unwrap();
    let w = u32::from_le_bytes(buffer[8..12].try_into().unwrap());
    let h = u32::from_le_bytes(buffer[12..16].try_into().unwrap());
    let dark = buffer[32..]
        .chunks_exact(4)
        .filter(|px| px[3] > 0 && (px[0] < 160 || px[1] < 160 || px[2] < 160))
        .count();
    (w, h, dark)
}

/// A one-page A4 document: a filled rectangle at (100, 200)–(300, 500) and a text line.
fn a4_doc() -> TestDoc {
    let bytes = with_state(|st| {
        let mut doc = st.pdfium.create_new_pdf().expect("new pdf");
        let font = doc.fonts_mut().helvetica();
        {
            let mut page = doc
                .pages_mut()
                .create_page_at_end(PdfPagePaperSize::a4())
                .expect("A4 page");
            page.objects_mut()
                .create_path_object_rect(
                    PdfRect::new_from_values(200.0, 100.0, 500.0, 300.0),
                    None,
                    None,
                    Some(PdfColor::new(20, 20, 120, 255)),
                )
                .expect("rect");
            page.objects_mut()
                .create_text_object(
                    PdfPoints::new(100.0),
                    PdfPoints::new(700.0),
                    "Resize me",
                    font,
                    PdfPoints::new(24.0),
                )
                .expect("text");
            page.regenerate_content().expect("regenerate");
        }
        Ok(doc.save_to_bytes().expect("save"))
    })
    .unwrap();
    reopen(bytes)
}

fn object_rects(doc_id: &str, page: u16) -> Vec<(PageObjectType, Rect)> {
    with_doc(doc_id, move |d| {
        Ok(objects::list(d, page)?
            .objects
            .into_iter()
            .map(|o| (o.object_type, o.rect))
            .collect())
    })
    .unwrap()
}

fn first_char_box(doc_id: &str, page: u16) -> Rect {
    with_doc(doc_id, move |d| {
        let l = layer::layer(d, page)?;
        Ok(l.chars
            .iter()
            .find(|c| !c.is_generated() && c.loose.width() > 0.0)
            .expect("a character")
            .loose)
    })
    .unwrap()
}

// ---------------------------------------------------------------------------------------
// crop
// ---------------------------------------------------------------------------------------

#[test]
fn crop_survives_save_and_reopen_and_content_renders_inside() {
    let doc = open("tracemonkey.pdf");
    let (_, dark_before) = {
        let (w, h, dark) = render_ink(&doc.doc_id, 0, 1.0);
        assert_eq!((w, h), (612, 792));
        ((w, h), dark)
    };
    let area = Rect::new(72.0, 100.0, 540.0, 700.0);
    let info = crop(
        &doc.doc_id,
        list(&[0]),
        Some(Some(CropSpec::Rect(area))),
        None,
    )
    .unwrap();
    assert_eq!(info.undo_label.as_deref(), Some("undo.pageCrop"));
    assert!(info.dirty && info.can_undo);
    let g = &info.pages[0];
    assert_rect(g.crop, area, 0.01, "DocInfo crop");
    assert!(near(g.width_pt, 468.0, 0.01) && near(g.height_pt, 600.0, 0.01));
    // page 2 untouched
    assert_rect(
        info.pages[1].crop,
        Rect::new(0.0, 0.0, 612.0, 792.0),
        0.01,
        "page 2",
    );

    // content still renders, inside the new box
    let (w, h, dark) = render_ink(&doc.doc_id, 0, 1.0);
    assert_eq!((w, h), (468, 600));
    assert!(
        dark > 1000 && dark < dark_before,
        "cropped render has ink: {dark} of {dark_before}"
    );

    let reopened = reopen(save_bytes(&doc.doc_id));
    let (crop_box, media) = boxes_of(&reopened.doc_id, 0);
    assert_rect(crop_box, area, 0.01, "reopened crop box");
    assert_rect(
        media,
        Rect::new(0.0, 0.0, 612.0, 792.0),
        0.01,
        "media untouched",
    );
    assert_rect(reopened.info.pages[0].crop, area, 0.01, "reopened DocInfo");
    // the text is all still there (a crop hides, it never deletes)
    let chars = |id: &str| {
        let id = id.to_string();
        with_doc(&id, |d| Ok(layer::page_text(d, 0)?.chars.len())).unwrap()
    };
    assert_eq!(chars(&reopened.doc_id), chars(&doc.doc_id));

    // one undo step back to the full page
    let back = undo(&doc.doc_id);
    assert_rect(
        back.pages[0].crop,
        Rect::new(0.0, 0.0, 612.0, 792.0),
        0.01,
        "undone",
    );
}

#[test]
fn crop_margins_follow_the_rotation_and_reset_restores_the_media_box() {
    // rotation.pdf: page 2 carries /Rotate 90
    let doc = open("rotation.pdf");
    let g = doc.info.pages[1].clone();
    assert_eq!(g.rotation, 90);
    let (cw, ch) = (g.crop.width(), g.crop.height());
    let margins = Margins {
        top: 10.0,
        right: 20.0,
        bottom: 30.0,
        left: 40.0,
    };
    let info = crop(
        &doc.doc_id,
        list(&[1]),
        Some(Some(CropSpec::Margins { margins })),
        None,
    )
    .unwrap();
    let g2 = &info.pages[1];
    // as seen: 20 + 40 off the width, 10 + 30 off the height
    assert!(
        near(g2.width_pt, ch - 60.0, 0.01),
        "{} vs {}",
        g2.width_pt,
        ch - 60.0
    );
    assert!(near(g2.height_pt, cw - 40.0, 0.01));
    // /Rotate 90: the displayed top is user x = l, the displayed left is user y = b
    assert_rect(
        g2.crop,
        Rect::new(
            g.crop.l + 10.0,
            g.crop.b + 40.0,
            g.crop.r - 30.0,
            g.crop.t - 20.0,
        ),
        0.01,
        "rotated margins",
    );

    // 원래대로: crop = media, one more undo step
    let info = crop(&doc.doc_id, list(&[1]), Some(None), None).unwrap();
    let (crop_box, media) = boxes_of(&doc.doc_id, 1);
    assert_rect(crop_box, media, 0.01, "reset crop = media");
    assert!(near(info.pages[1].width_pt, g.width_pt, 0.01));
    assert!(near(info.pages[1].height_pt, g.height_pt, 0.01));
}

#[test]
fn a_new_media_box_clips_an_explicit_crop_box() {
    let doc = open("tracemonkey.pdf");
    crop(
        &doc.doc_id,
        list(&[0]),
        Some(Some(CropSpec::Rect(Rect::new(50.0, 50.0, 500.0, 700.0)))),
        None,
    )
    .unwrap();
    let info = crop(
        &doc.doc_id,
        list(&[0]),
        None,
        Some(Some(Rect::new(0.0, 0.0, 400.0, 600.0))),
    )
    .unwrap();
    let (crop_box, media) = boxes_of(&doc.doc_id, 0);
    assert_rect(media, Rect::new(0.0, 0.0, 400.0, 600.0), 0.01, "media");
    assert_rect(
        crop_box,
        Rect::new(50.0, 50.0, 400.0, 600.0),
        0.01,
        "clipped crop",
    );
    assert!(near(info.pages[0].width_pt, 350.0, 0.01));
}

#[test]
fn crop_refuses_impossible_boxes_and_changes_nothing() {
    let doc = open("tracemonkey.pdf");
    let generation = doc.info.doc_generation;
    let expect_invalid = |r: Result<DocInfo, EngineError>, what: &str| {
        let e = r.expect_err(what);
        assert_eq!(e.code, ErrorCode::InvalidArgument, "{what}: {e:?}");
    };
    expect_invalid(crop(&doc.doc_id, all(), None, Some(None)), "media null");
    expect_invalid(
        crop(
            &doc.doc_id,
            list(&[0]),
            Some(Some(CropSpec::Rect(Rect::new(700.0, 800.0, 900.0, 1000.0)))),
            None,
        ),
        "outside the media box",
    );
    expect_invalid(
        crop(
            &doc.doc_id,
            list(&[0]),
            Some(Some(CropSpec::Margins {
                margins: Margins {
                    top: 400.0,
                    right: 0.0,
                    bottom: 400.0,
                    left: 0.0,
                },
            })),
            None,
        ),
        "margins eat the page",
    );
    expect_invalid(
        crop(
            &doc.doc_id,
            list(&[99]),
            Some(Some(CropSpec::Rect(Rect::new(0.0, 0.0, 100.0, 100.0)))),
            None,
        ),
        "page out of range",
    );
    // a failure on the 3rd page of a batch rolls the first two back as well
    expect_invalid(
        crop(
            &doc.doc_id,
            list(&[0, 1, 2]),
            Some(Some(CropSpec::Margins {
                margins: Margins {
                    top: 0.0,
                    right: 700.0,
                    bottom: 0.0,
                    left: 0.0,
                },
            })),
            None,
        ),
        "margins wider than the page",
    );
    let now = with_state({
        let id = doc.doc_id.clone();
        move |st| Ok(st.doc(&id)?.info())
    })
    .unwrap();
    assert_eq!(
        now.doc_generation, generation,
        "no failed call bumped the generation"
    );
    assert!(!now.can_undo);
}

// ---------------------------------------------------------------------------------------
// resize
// ---------------------------------------------------------------------------------------

#[test]
fn resize_a4_to_letter_keeps_the_aspect_and_centres() {
    let doc = a4_doc();
    assert!(near(doc.info.pages[0].width_pt, A4.0, 0.01));
    let before = object_rects(&doc.doc_id, 0);
    let rect_before = before
        .iter()
        .find(|(t, _)| *t == PageObjectType::Path)
        .expect("the rectangle")
        .1;
    assert_rect(
        rect_before,
        Rect::new(100.0, 200.0, 300.0, 500.0),
        TOL,
        "fixture rect",
    );

    let info = resize(
        &doc.doc_id,
        all(),
        ResizeTarget::Named(PaperName::Letter),
        ResizeMode::ScaleContent,
    )
    .unwrap();
    assert_eq!(info.undo_label.as_deref(), Some("undo.pageResize"));
    let g = &info.pages[0];
    assert!(
        near(g.width_pt, 612.0, 0.01) && near(g.height_pt, 792.0, 0.01),
        "{g:?}"
    );
    assert_rect(g.crop, Rect::new(0.0, 0.0, 612.0, 792.0), 0.01, "new crop");

    let s = (612.0 / A4.0).min(792.0 / A4.1);
    let tx = (612.0 - A4.0 * s) / 2.0;
    let after = object_rects(&doc.doc_id, 0);
    let rect_after = after
        .iter()
        .find(|(t, _)| *t == PageObjectType::Path)
        .expect("the rectangle")
        .1;
    let expected = Rect::new(100.0 * s + tx, 200.0 * s, 300.0 * s + tx, 500.0 * s);
    assert_rect(rect_after, expected, TOL, "scaled and centred rect");
    // the aspect survives
    assert!(near(
        rect_after.width() / rect_after.height(),
        rect_before.width() / rect_before.height(),
        0.005
    ));
    // centred: equal white bands left and right of the scaled page
    assert!(near(tx, 612.0 - (A4.0 * s + tx), 0.01));

    // the text is still text, and it moved with the page
    let text = with_doc(&doc.doc_id, |d| Ok(layer::page_text(d, 0)?.text.clone())).unwrap();
    assert!(text.contains("Resize me"), "{text:?}");
    let reopened = reopen(save_bytes(&doc.doc_id));
    let (crop_box, media) = boxes_of(&reopened.doc_id, 0);
    assert_rect(
        media,
        Rect::new(0.0, 0.0, 612.0, 792.0),
        0.01,
        "reopened media",
    );
    assert_rect(crop_box, media, 0.01, "reopened crop");
    let reopened_rect = object_rects(&reopened.doc_id, 0)
        .into_iter()
        .find(|(t, _)| *t == PageObjectType::Path)
        .unwrap()
        .1;
    assert_rect(reopened_rect, expected, TOL, "reopened rect");

    // one undo step
    let back = undo(&doc.doc_id);
    assert!(near(back.pages[0].width_pt, A4.0, 0.01) && near(back.pages[0].height_pt, A4.1, 0.01));
    let restored = object_rects(&doc.doc_id, 0)
        .into_iter()
        .find(|(t, _)| *t == PageObjectType::Path)
        .unwrap()
        .1;
    assert_rect(restored, rect_before, TOL, "undone rect");
}

#[test]
fn resize_centre_mode_keeps_the_scale() {
    let doc = open("tracemonkey.pdf");
    let before = first_char_box(&doc.doc_id, 0);
    let info = resize(
        &doc.doc_id,
        list(&[0]),
        ResizeTarget::Named(PaperName::A4),
        ResizeMode::CenterContent,
    )
    .unwrap();
    assert!(near(info.pages[0].width_pt, A4.0, 0.01));
    assert!(
        near(info.pages[1].width_pt, 612.0, 0.01),
        "only page 1 resized"
    );
    let (dx, dy) = ((A4.0 - 612.0) / 2.0, (A4.1 - 792.0) / 2.0);
    let after = first_char_box(&doc.doc_id, 0);
    assert_rect(
        after,
        Rect::new(before.l + dx, before.b + dy, before.r + dx, before.t + dy),
        TOL,
        "first character moved by the centring offset only",
    );
}

#[test]
fn resize_moves_annotations_with_the_content() {
    let doc = a4_doc();
    let id = doc.doc_id.clone();
    let quad = Rect::new(100.0, 695.0, 220.0, 722.0);
    let stroke = vec![100.0, 100.0, 200.0, 150.0, 300.0, 120.0];
    with_state(move |st| {
        registry::mutate(
            st,
            &id,
            MutateOpts::new("undo.annotCreate", ChangeReason::Edit).page(0),
            |d| {
                annot::create::create(
                    d,
                    0,
                    &AnnotSpec::Highlight(MarkupSpec {
                        rects: vec![quad],
                        color: [255, 235, 0],
                        opacity: 0.6,
                        contents: Some("형광펜".into()),
                    }),
                    None,
                )?;
                annot::create::create(
                    d,
                    0,
                    &AnnotSpec::Ink(InkSpec {
                        paths: vec![vec![100.0, 100.0, 200.0, 150.0, 300.0, 120.0]],
                        color: [200, 0, 0],
                        width: 2.0,
                        opacity: 1.0,
                    }),
                    None,
                )
            },
        )
    })
    .unwrap();
    let before = with_doc(&doc.doc_id, |d| annot::list(d, 0)).unwrap();
    assert_eq!(before.len(), 2);

    resize(
        &doc.doc_id,
        all(),
        ResizeTarget::Named(PaperName::Letter),
        ResizeMode::ScaleContent,
    )
    .unwrap();
    let s = (612.0 / A4.0).min(792.0 / A4.1);
    let tx = (612.0 - A4.0 * s) / 2.0;
    let map = |r: Rect| Rect::new(r.l * s + tx, r.b * s, r.r * s + tx, r.t * s);

    let after = with_doc(&doc.doc_id, |d| annot::list(d, 0)).unwrap();
    assert_eq!(after.len(), 2);
    let hl = after
        .iter()
        .find(|a| a.kind == AnnotKind::Highlight)
        .unwrap();
    let hl_before = before
        .iter()
        .find(|a| a.kind == AnnotKind::Highlight)
        .unwrap();
    assert_rect(
        hl.quads.as_ref().unwrap()[0],
        map(quad),
        TOL,
        "highlight quad",
    );
    assert_rect(hl.rect, map(hl_before.rect), 1.0, "highlight rect");
    let ink = after.iter().find(|a| a.kind == AnnotKind::Ink).unwrap();
    let path = &ink.ink_paths.as_ref().unwrap()[0];
    for (i, pt) in stroke.chunks_exact(2).enumerate() {
        assert!(
            near(path[2 * i], pt[0] * s + tx, TOL) && near(path[2 * i + 1], pt[1] * s, TOL),
            "ink point {i}: {path:?}"
        );
    }
    let ink_before = before.iter().find(|a| a.kind == AnnotKind::Ink).unwrap();
    assert_rect(ink.rect, map(ink_before.rect), 1.0, "ink rect");

    // the highlight still paints where its quad now is
    let (_, _, dark) = render_ink(&doc.doc_id, 0, 1.0);
    assert!(dark > 0);
}

#[test]
fn resize_keeps_the_orientation_of_a_rotated_page() {
    let doc = open("rotation.pdf");
    let g = doc.info.pages[1].clone();
    assert!(
        g.width_pt > g.height_pt,
        "rotation.pdf page 2 is landscape as seen"
    );
    let info = resize(
        &doc.doc_id,
        list(&[1]),
        ResizeTarget::Named(PaperName::A4),
        ResizeMode::ScaleContent,
    )
    .unwrap();
    let g2 = &info.pages[1];
    assert_eq!(g2.rotation, 90);
    assert!(
        near(g2.width_pt, A4.1, 0.01) && near(g2.height_pt, A4.0, 0.01),
        "{g2:?}"
    );
    // an explicit size is the size as seen
    let info = resize(
        &doc.doc_id,
        list(&[1]),
        ResizeTarget::Size { w: 400.0, h: 300.0 },
        ResizeMode::ScaleContent,
    )
    .unwrap();
    assert!(
        near(info.pages[1].width_pt, 400.0, 0.01) && near(info.pages[1].height_pt, 300.0, 0.01)
    );
    assert_eq!(info.pages[1].rotation, 90);
    let e = resize(
        &doc.doc_id,
        list(&[0]),
        ResizeTarget::Size { w: 0.5, h: 300.0 },
        ResizeMode::ScaleContent,
    )
    .expect_err("too small");
    assert_eq!(e.code, ErrorCode::InvalidArgument);
}

/// A resized page's content is wrapped in extra content streams; editing it afterwards (a
/// stamp regenerates the page content) must neither lose nor double the transform.
#[test]
fn resize_then_stamp_survives_save_and_reopen() {
    let doc = a4_doc();
    resize(
        &doc.doc_id,
        all(),
        ResizeTarget::Named(PaperName::Letter),
        ResizeMode::ScaleContent,
    )
    .unwrap();
    let spec = PageStampSpec {
        role: StampRole::Footer,
        source: PageStampSource::Text {
            text: "Footer {{page}}".into(),
            font_size_pt: 10.0,
            color: [0, 0, 0],
        },
        anchor: StampAnchor::Bc,
        margin_pt: 24.0,
        rotate_deg: 0.0,
        opacity: 1.0,
        pages: PageSelection::All(AllPages::All),
        bates: Default::default(),
    };
    let id = doc.doc_id.clone();
    with_state(move |st| stamp::add_stamp(st, &id, &spec)).unwrap();
    let reopened = reopen(save_bytes(&doc.doc_id));
    let s = (612.0 / A4.0).min(792.0 / A4.1);
    let tx = (612.0 - A4.0 * s) / 2.0;
    let rect = object_rects(&reopened.doc_id, 0)
        .into_iter()
        .find(|(t, _)| *t == PageObjectType::Path)
        .unwrap()
        .1;
    assert_rect(
        rect,
        Rect::new(100.0 * s + tx, 200.0 * s, 300.0 * s + tx, 500.0 * s),
        TOL,
        "scaled rect after a later edit",
    );
    let text = with_doc(&reopened.doc_id, |d| {
        Ok(layer::page_text(d, 0)?.text.clone())
    })
    .unwrap();
    assert!(
        text.contains("Resize me") && text.contains("Footer 1"),
        "{text:?}"
    );
}
