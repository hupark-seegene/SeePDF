//! Stage 1 (b) — page objects (`IPC_CONTRACT.md` §7.4).
//!
//! Every text assertion goes through **save → reopen → extract**, never through the in-memory
//! object: `set_text` on a subset font returns `Ok`, changes the in-memory text and renders
//! `SeePDFeditedtitle`, and an edit inside a Form XObject returns `Ok` and is then thrown away
//! by `FPDFPage_GenerateContent`. Only the reopened file tells the truth.

mod common;
use common::*;

use seepdf_lib::engine::objects;
use seepdf_lib::engine::registry;
use seepdf_lib::engine::save;
use seepdf_lib::engine::text::search;
use seepdf_lib::engine::text::layer;
use seepdf_lib::ipc::types::{
    Editability, NotEditableReason, PageObject, PageObjectList, PageObjectType, Rect, TextAlign,
    TextEditStrategy, TextObjectPatch,
};
use seepdf_lib::ipc::ErrorCode;
use std::path::PathBuf;

fn out_dir() -> PathBuf {
    let dir = fixture("out").join("stage1b");
    std::fs::create_dir_all(&dir).expect("create fixtures/out/stage1b");
    dir
}

fn list(doc_id: &str, page: u16) -> PageObjectList {
    let doc_id = doc_id.to_string();
    with_doc(&doc_id, move |doc| objects::list(doc, page)).expect("list_page_objects")
}

fn page_text(doc_id: &str, page: u16) -> String {
    let doc_id = doc_id.to_string();
    with_doc(&doc_id, move |doc| {
        Ok(layer::page_text(doc, page)?.text.clone())
    })
    .expect("page text")
}

/// The full save pipeline's bytes (appearance pass + `FPDF_SaveAsCopy(NO_INCREMENTAL)`).
fn save_bytes(doc_id: &str) -> Vec<u8> {
    let doc_id = doc_id.to_string();
    with_state(move |st| save::serialize(st, &doc_id)).expect("serialize")
}

fn reopen(bytes: Vec<u8>) -> TestDoc {
    let info = with_state(move |st| registry::open(st, None, bytes, None)).expect("reopen");
    let doc_id = info.doc_id.clone();
    TestDoc { info, doc_id }
}

fn text_objects(objects: &[PageObject]) -> Vec<&PageObject> {
    objects
        .iter()
        .filter(|o| o.object_type == PageObjectType::Text)
        .collect()
}

// ---------------------------------------------------------------------------------------

/// A base-14 font covers everything WinAnsi can draw, so the characters change **in place**:
/// the object keeps its index, matrix and colour, and no font is embedded.
#[test]
fn objects_text_inplace() {
    let doc = open("tracemonkey.pdf");
    let rect = Rect::new(72.0, 60.0, 400.0, 90.0);
    let added = with_state({
        let doc_id = doc.doc_id.clone();
        move |st| {
            objects::add_text(
                st,
                &doc_id,
                0,
                rect,
                "SeePDF in place",
                14.0,
                [10, 20, 200],
                TextAlign::Left,
            )
        }
    })
    .expect("add_text_object");
    let object_id = added.objects.last().expect("the new object").object_id;
    assert_eq!(
        added.objects.last().unwrap().font_name.as_deref(),
        Some("Helvetica")
    );

    let probe = with_doc(&doc.doc_id, move |d| {
        objects::probe(d, 0, object_id, "SeePDF edited in place")
    })
    .expect("probe_text_edit");
    assert_eq!(probe.strategy, TextEditStrategy::InPlace);
    assert!(probe.substitute_font.is_none());

    let generation = added.doc_generation;
    let after = with_state({
        let doc_id = doc.doc_id.clone();
        move |st| {
            objects::edit_text(
                st,
                &doc_id,
                0,
                object_id,
                generation,
                TextObjectPatch {
                    text: Some("SeePDF edited in place".into()),
                    font_size_pt: Some(18.0),
                    color: Some([200, 0, 0]),
                },
                false,
            )
        }
    })
    .expect("edit_text_object");
    assert_eq!(
        after.objects.len(),
        added.objects.len(),
        "an in-place edit adds no object"
    );
    let edited = &after.objects[object_id as usize];
    assert_eq!(edited.text.as_deref(), Some("SeePDF edited in place"));
    assert_eq!(edited.font_size_pt, Some(18.0));
    assert_eq!(edited.color, Some([200, 0, 0]));

    let saved = reopen(save_bytes(&doc.doc_id));
    let text = page_text(&saved.doc_id, 0);
    assert!(
        text.contains("SeePDF edited in place"),
        "the edit survives save + reopen, spaces and all: {text:?}"
    );

    // `expectGeneration` is what stops an edit aimed at a stale object index.
    let stale = with_state({
        let doc_id = doc.doc_id.clone();
        move |st| {
            objects::edit_text(
                st,
                &doc_id,
                0,
                object_id,
                1,
                TextObjectPatch {
                    text: Some("too late".into()),
                    ..Default::default()
                },
                true,
            )
        }
    });
    assert_eq!(stale.unwrap_err().code, ErrorCode::Stale);
}

/// tracemonkey's title is set in an embedded **subset** of NimbusRomNo9L, which has no space
/// glyph: `set_text` would render `SeePDFeditedtitle`. The trial check catches that, the probe
/// reports `replaceFont`, and an edit without permission to substitute is refused.
#[test]
fn objects_text_probe_subset_font() {
    let doc = open("tracemonkey.pdf");
    let listing = list(&doc.doc_id, 0);
    let title = text_objects(&listing.objects)[0];
    let object_id = title.object_id;
    let original = title.text.clone().unwrap_or_default();
    assert!(
        title.font_name.as_deref().unwrap_or("").contains("Nimbus"),
        "the fixture's title uses an embedded Type1 subset, not a base-14 font"
    );
    assert_eq!(title.editable, Editability::Full, "it can still be edited");

    let probe = with_doc(&doc.doc_id, move |d| {
        objects::probe(d, 0, object_id, "SeePDF edited title")
    })
    .expect("probe_text_edit");
    assert_eq!(
        probe.strategy,
        TextEditStrategy::ReplaceFont,
        "the subset has no space glyph"
    );
    assert_eq!(probe.substitute_font.as_deref(), Some("Helvetica"));
    assert_eq!(probe.reason, Some(NotEditableReason::GlyphsMissing));

    // The probe must not have changed anything.
    let after_probe = list(&doc.doc_id, 0);
    assert_eq!(
        after_probe.objects[object_id as usize].text.as_deref(),
        Some(original.as_str()),
        "the trial edit is reverted"
    );
    assert_eq!(after_probe.doc_generation, listing.doc_generation);

    let refused = with_state({
        let doc_id = doc.doc_id.clone();
        let generation = listing.doc_generation;
        move |st| {
            objects::edit_text(
                st,
                &doc_id,
                0,
                object_id,
                generation,
                TextObjectPatch {
                    text: Some("SeePDF edited title".into()),
                    ..Default::default()
                },
                false,
            )
        }
    });
    assert_eq!(
        refused.unwrap_err().code,
        ErrorCode::FontCoverage,
        "allowFontSubstitution: false must not silently substitute"
    );

    let committed = with_state({
        let doc_id = doc.doc_id.clone();
        let generation = listing.doc_generation;
        move |st| {
            objects::edit_text(
                st,
                &doc_id,
                0,
                object_id,
                generation,
                TextObjectPatch {
                    text: Some("SeePDF edited title".into()),
                    ..Default::default()
                },
                true,
            )
        }
    })
    .expect("edit with substitution");
    assert_eq!(
        committed.objects.len(),
        listing.objects.len(),
        "replace-object appends one and removes one"
    );

    let saved = reopen(save_bytes(&doc.doc_id));
    let text = page_text(&saved.doc_id, 0);
    assert!(
        text.contains("SeePDF edited title"),
        "the substituted font draws the spaces too: {:?}",
        &text[..text.len().min(200)]
    );
    assert!(!text.contains(&original), "the old run is gone");
    std::fs::write(out_dir().join("text-replaced.pdf"), save_bytes(&doc.doc_id))
        .expect("write sample");
}

/// `TAMReview.pdf` keeps its entire page content inside Form XObjects. `set_text` on a child
/// returns `Ok` and `regenerate_content()` throws it away, so the only honest answer is to
/// refuse (text spike §8.7).
#[test]
fn objects_text_xobject_refused() {
    let doc = open("TAMReview.pdf");
    // The fixture's Form XObjects are not on every page, so find one rather than hard-coding
    // an index that a different PDFium build might renumber.
    let (page_index, form) = (0u16..8)
        .find_map(|p| {
            let listing = list(&doc.doc_id, p);
            listing
                .objects
                .into_iter()
                .find(|o| o.object_type == PageObjectType::Form && o.text.is_some())
                .map(|o| (p, o))
        })
        .expect("TAMReview has pages built from Form XObjects with text inside");
    assert_eq!(form.editable, Editability::ReadOnly);
    assert_eq!(form.reason, Some(NotEditableReason::InsideXObject));
    assert!(
        !form.text.as_deref().unwrap_or("").is_empty(),
        "the inner text is still reported so the UI can show it"
    );

    let object_id = form.object_id;
    let probe = with_doc(&doc.doc_id, move |d| {
        objects::probe(d, page_index, object_id, "replacement")
    })
    .expect("probe_text_edit");
    assert_eq!(probe.strategy, TextEditStrategy::Refused);
    assert_eq!(probe.reason, Some(NotEditableReason::InsideXObject));

    let generation = with_doc(&doc.doc_id, |d| Ok(d.generation)).expect("generation");
    let refused = with_state({
        let doc_id = doc.doc_id.clone();
        move |st| {
            objects::edit_text(
                st,
                &doc_id,
                page_index,
                object_id,
                generation,
                TextObjectPatch {
                    text: Some("replacement".into()),
                    ..Default::default()
                },
                true,
            )
        }
    });
    let err = refused.unwrap_err();
    assert_eq!(err.code, ErrorCode::Unsupported);
    assert_eq!(err.detail.as_deref(), Some("insideXObject"));

    let unchanged = with_doc(&doc.doc_id, |d| Ok(d.info())).expect("info");
    assert_eq!(
        unchanged.doc_generation, generation,
        "a refused edit does not bump the generation"
    );
}

/// 한글 through the bundled subset: it renders, it extracts **with real spaces** (the
/// AppleGothic bug would give TABs), it survives save + reopen, and the font is embedded once
/// no matter how many lines were added.
#[test]
fn objects_add_text_korean() {
    let doc = open("tracemonkey.pdf");
    let before = with_doc(&doc.doc_id, |d| Ok(d.to_bytes()?.len())).expect("size");

    let rect = Rect::new(72.0, 500.0, 520.0, 620.0);
    let added = with_state({
        let doc_id = doc.doc_id.clone();
        move |st| {
            objects::add_text(
                st,
                &doc_id,
                0,
                rect,
                "한글 텍스트 상자입니다\n두 번째 줄 SeePDF 2026",
                20.0,
                [0, 0, 0],
                TextAlign::Left,
            )
        }
    })
    .expect("add_text_object");
    let korean: Vec<&PageObject> = added
        .objects
        .iter()
        .filter(|o| o.font_name.as_deref() == Some("SeePDF-Hangul"))
        .collect();
    assert_eq!(korean.len(), 2, "one object per line");
    assert!(korean[0].ours, "objects drawn with the bundled font are ours");

    let bytes = save_bytes(&doc.doc_id);
    let growth = bytes.len() as i64 - before as i64;
    assert!(
        growth < 1_600_000,
        "the bundled font is embedded once: +{growth} bytes"
    );

    let saved = reopen(bytes.clone());
    let text = page_text(&saved.doc_id, 0);
    assert!(
        text.contains("한글 텍스트 상자입니다"),
        "Hangul extracts with ASCII spaces, not TABs: {text:?}"
    );
    assert!(text.contains("두 번째 줄 SeePDF 2026"));
    assert!(
        !text.contains("한글\t"),
        "the space glyph must not reverse-map to U+0009 (the AppleGothic bug)"
    );

    // And the whole phrase is findable, which is what the TAB bug breaks.
    let query = search::Query::new("한글 텍스트", false, false).expect("query");
    let hits = with_doc(&saved.doc_id, move |d| search::search_page(d, 0, &query))
        .expect("search")
        .len();
    assert!(hits >= 1, "search finds the phrase across the space");

    std::fs::write(out_dir().join("korean-text-box.pdf"), &bytes).expect("write sample");

    // A second block must not embed the font again.
    let second = with_state({
        let doc_id = doc.doc_id.clone();
        move |st| {
            objects::add_text(
                st,
                &doc_id,
                0,
                Rect::new(72.0, 400.0, 520.0, 440.0),
                "세 번째 블록",
                18.0,
                [0, 0, 0],
                TextAlign::Center,
            )
        }
    })
    .expect("add a second block");
    assert!(second.objects.len() > added.objects.len());
    let grown = save_bytes(&doc.doc_id).len();
    assert!(
        grown < bytes.len() + 200_000,
        "the second block reuses the embedded font"
    );
}

/// Changing a Latin run to Korean is the other half of the substitution path: no font on the
/// page can draw 한글, so the object is replaced with one drawn in the bundled subset.
#[test]
fn objects_text_replace_with_korean() {
    let doc = open("tracemonkey.pdf");
    let added = with_state({
        let doc_id = doc.doc_id.clone();
        move |st| {
            objects::add_text(
                st,
                &doc_id,
                0,
                Rect::new(72.0, 200.0, 460.0, 240.0),
                "placeholder",
                16.0,
                [0, 0, 0],
                TextAlign::Left,
            )
        }
    })
    .expect("add_text_object");
    let object_id = added.objects.last().unwrap().object_id;

    let probe = with_doc(&doc.doc_id, move |d| {
        objects::probe(d, 0, object_id, "한글로 바뀐 텍스트")
    })
    .expect("probe_text_edit");
    assert_eq!(probe.strategy, TextEditStrategy::ReplaceFont);
    assert_eq!(probe.substitute_font.as_deref(), Some("SeePDF Hangul"));

    let after = with_state({
        let doc_id = doc.doc_id.clone();
        let generation = added.doc_generation;
        move |st| {
            objects::edit_text(
                st,
                &doc_id,
                0,
                object_id,
                generation,
                TextObjectPatch {
                    text: Some("한글로 바뀐 텍스트".into()),
                    color: Some([0, 90, 0]),
                    ..Default::default()
                },
                true,
            )
        }
    })
    .expect("edit_text_object");
    let replacement = after
        .objects
        .iter()
        .find(|o| o.text.as_deref() == Some("한글로 바뀐 텍스트"))
        .expect("the replacement object");
    assert_eq!(replacement.font_name.as_deref(), Some("SeePDF-Hangul"));
    assert!(replacement.ours);
    assert_eq!(replacement.color, Some([0, 90, 0]));

    let saved = reopen(save_bytes(&doc.doc_id));
    let text = page_text(&saved.doc_id, 0);
    assert!(text.contains("한글로 바뀐 텍스트"), "{text:?}");
    assert!(!text.contains("placeholder"), "the old run is gone");
}

/// A PNG placed at a rect lands at exactly those bounds after save + reopen (F-19).
#[test]
fn objects_add_image() {
    let png_path = out_dir().join("stamp.png");
    write_test_png(&png_path, 64, 32);

    let doc = open("tracemonkey.pdf");
    let rect = Rect::new(100.0, 100.0, 260.0, 180.0);
    let added = with_state({
        let doc_id = doc.doc_id.clone();
        let png = png_path.display().to_string();
        move |st| objects::add_image(st, &doc_id, 0, rect, &png, false)
    })
    .expect("add_image_object");
    let image = added
        .objects
        .iter()
        .find(|o| o.object_type == PageObjectType::Image)
        .expect("the image object");
    assert!((image.rect.l - rect.l).abs() < 0.5);
    assert!((image.rect.b - rect.b).abs() < 0.5);
    assert!((image.rect.r - rect.r).abs() < 0.5);
    assert!((image.rect.t - rect.t).abs() < 0.5);
    let object_id = image.object_id;

    let bytes = save_bytes(&doc.doc_id);
    let saved = reopen(bytes.clone());
    let reopened = list(&saved.doc_id, 0);
    let image = reopened
        .objects
        .iter()
        .find(|o| o.object_type == PageObjectType::Image)
        .expect("the image survived the round trip");
    assert!((image.rect.l - rect.l).abs() < 0.5, "{:?}", image.rect);
    assert!((image.rect.t - rect.t).abs() < 0.5, "{:?}", image.rect);
    std::fs::write(out_dir().join("image-placed.pdf"), &bytes).expect("write sample");

    // Moving and deleting persist too.
    let generation = added.doc_generation;
    let moved = with_state({
        let doc_id = doc.doc_id.clone();
        move |st| {
            objects::transform(
                st,
                &doc_id,
                0,
                object_id,
                generation,
                Some([40.0, -20.0]),
                None,
                None,
            )
        }
    })
    .expect("transform_object");
    let image = &moved.objects[object_id as usize];
    assert!((image.rect.l - (rect.l + 40.0)).abs() < 0.5);
    assert!((image.rect.b - (rect.b - 20.0)).abs() < 0.5);

    let deleted = with_state({
        let doc_id = doc.doc_id.clone();
        let generation = moved.doc_generation;
        move |st| objects::delete(st, &doc_id, 0, &[object_id], generation)
    })
    .expect("delete_objects");
    assert!(
        !deleted
            .objects
            .iter()
            .any(|o| o.object_type == PageObjectType::Image),
        "the image is gone"
    );
    let saved = reopen(save_bytes(&doc.doc_id));
    assert!(
        !list(&saved.doc_id, 0)
            .objects
            .iter()
            .any(|o| o.object_type == PageObjectType::Image),
        "and stays gone after save + reopen"
    );
}

/// An anchored scale keeps the object's bottom-left corner where it was; a bare `scale()` would
/// scale about the page origin and move it (text spike §8.5).
#[test]
fn objects_transform_is_anchored() {
    let doc = open("tracemonkey.pdf");
    let listing = list(&doc.doc_id, 0);
    let title = text_objects(&listing.objects)[0];
    let object_id = title.object_id;
    let before = title.rect;

    let after = with_state({
        let doc_id = doc.doc_id.clone();
        let generation = listing.doc_generation;
        move |st| {
            objects::transform(
                st,
                &doc_id,
                0,
                object_id,
                generation,
                None,
                Some([1.5, 1.5]),
                None,
            )
        }
    })
    .expect("transform_object");
    let scaled = &after.objects[object_id as usize];
    assert!(
        (scaled.rect.l - before.l).abs() < 0.5 && (scaled.rect.b - before.b).abs() < 0.5,
        "the anchor corner does not move: {:?} -> {:?}",
        before,
        scaled.rect
    );
    assert!(
        (scaled.rect.width() - before.width() * 1.5).abs() < 1.0,
        "and the width is 1.5x"
    );
}

/// Every image on a page, re-encoded as PNG — the "extract images" surface.
#[test]
fn objects_extract_images() {
    let doc = open("160F-2019.pdf");
    let images = with_doc(&doc.doc_id, |d| objects::extract_images(d, 0)).expect("extract_images");
    assert_eq!(images.len(), 2, "the fixture has two images on page 1");
    for image in &images {
        assert!(image.width_px > 0 && image.height_px > 0);
        assert_eq!(&image.png[1..4], b"PNG");
    }
}

/// A tiny opaque PNG, so the test does not depend on a binary fixture.
fn write_test_png(path: &PathBuf, width: u32, height: u32) {
    let mut pixels = Vec::with_capacity((width * height * 4) as usize);
    for y in 0..height {
        for x in 0..width {
            pixels.extend_from_slice(&[
                (x * 255 / width.max(1)) as u8,
                (y * 255 / height.max(1)) as u8,
                0x40,
                0xFF,
            ]);
        }
    }
    let buffer = image::RgbaImage::from_raw(width, height, pixels).expect("buffer");
    buffer.save(path).expect("write png");
}

// ---------------------------------------------------------------------------------------
// Stage 8 — duplicate_objects
// ---------------------------------------------------------------------------------------

fn duplicate(
    doc_id: &str,
    page: u16,
    ids: Vec<u32>,
    offset: [f32; 2],
    target: Option<u16>,
) -> Result<seepdf_lib::ipc::types::DuplicateObjectsResult, seepdf_lib::ipc::EngineError> {
    let doc_id = doc_id.to_string();
    with_state(move |st| {
        let generation = st.doc(&doc_id)?.generation;
        objects::duplicate(st, &doc_id, page, &ids, generation, offset, target)
    })
}

fn moved(r: &Rect, dx: f32, dy: f32) -> Rect {
    Rect::new(r.l + dx, r.b + dy, r.r + dx, r.t + dy)
}

fn near(a: &Rect, b: &Rect) -> bool {
    (a.l - b.l).abs() < 1.0 && (a.b - b.b).abs() < 1.0 && (a.r - b.r).abs() < 1.0 && (a.t - b.t).abs() < 1.0
}

/// Same page: a text object (original glyph codes, subset font) and an image, in one call —
/// one undo step, the copies moved by the offset, the originals untouched, and the copied
/// text is really in the saved file.
#[test]
fn duplicate_objects_on_the_same_page() {
    let png_path = out_dir().join("duplicate.png");
    write_test_png(&png_path, 64, 32);
    let doc = open("tracemonkey.pdf");
    {
        let (doc_id, png) = (doc.doc_id.clone(), png_path.display().to_string());
        with_state(move |st| {
            objects::add_image(st, &doc_id, 0, Rect::new(400.0, 600.0, 480.0, 640.0), &png, false)
        })
        .expect("add image");
    }
    let before = list(&doc.doc_id, 0);
    let title = before
        .objects
        .iter()
        .find(|o| o.object_type == PageObjectType::Text && o.text.as_deref().unwrap_or("").contains("Trace-based"))
        .expect("the title run")
        .clone();
    let image = before
        .objects
        .iter()
        .rev()
        .find(|o| o.object_type == PageObjectType::Image)
        .expect("the added image")
        .clone();
    let words_before = page_text(&doc.doc_id, 0).matches("Trace-based").count();

    let (dx, dy) = (0.0, -250.0);
    let r = duplicate(&doc.doc_id, 0, vec![image.object_id, title.object_id], [dx, dy], None)
        .expect("duplicate");
    assert_eq!(r.objects.len(), before.objects.len() + 2);
    assert_eq!(r.new_object_ids.len(), 2);
    // `newObjectIds` follow the (sorted) request order: the title run first, then the image.
    let copies: Vec<&PageObject> = r.new_object_ids.iter().map(|&id| &r.objects[id as usize]).collect();
    let (text_copy, image_copy) = if copies[0].object_type == PageObjectType::Text {
        (copies[0], copies[1])
    } else {
        (copies[1], copies[0])
    };
    assert_eq!(text_copy.object_type, PageObjectType::Text);
    assert_eq!(text_copy.text, title.text);
    assert!(near(&text_copy.rect, &moved(&title.rect, dx, dy)), "{:?}", text_copy.rect);
    assert_eq!(image_copy.object_type, PageObjectType::Image);
    assert!(near(&image_copy.rect, &moved(&image.rect, dx, dy)), "{:?}", image_copy.rect);
    // The originals are where they were.
    assert!(r.objects.iter().any(|o| o.object_type == PageObjectType::Text && near(&o.rect, &title.rect)));
    let info = with_doc(&doc.doc_id, |d| Ok(d.info())).unwrap();
    assert_eq!(info.undo_label.as_deref(), Some("undo.objectDuplicate"));

    // In the file: the title's words twice, extracted from the reopened copy.
    let saved = reopen(save_bytes(&doc.doc_id));
    assert_eq!(
        page_text(&saved.doc_id, 0).matches("Trace-based").count(),
        words_before + 1
    );
    assert_eq!(list(&saved.doc_id, 0).objects.len(), before.objects.len() + 2);

    // One undo step for the whole call.
    let d = doc.doc_id.clone();
    with_state(move |st| registry::undo(st, &d, false)).expect("undo");
    assert_eq!(list(&doc.doc_id, 0).objects.len(), before.objects.len());
    assert_eq!(page_text(&doc.doc_id, 0).matches("Trace-based").count(), words_before);
}

/// Another page: text, image and a Form XObject copy onto page 1 (its content streams are
/// different), page 0 is untouched; a stale generation and a bad id are refused.
#[test]
fn duplicate_objects_across_pages() {
    let png_path = out_dir().join("duplicate-cross.png");
    write_test_png(&png_path, 48, 48);
    let doc = open("TAMReview.pdf");
    // A page with a Form XObject (TAMReview has them), and some plain text to copy.
    let (page, form) = (0u16..8)
        .find_map(|p| {
            list(&doc.doc_id, p)
                .objects
                .into_iter()
                .find(|o| o.object_type == PageObjectType::Form)
                .map(|o| (p, o))
        })
        .expect("a page with a Form XObject");
    {
        let (doc_id, png) = (doc.doc_id.clone(), png_path.display().to_string());
        with_state(move |st| {
            objects::add_image(st, &doc_id, page, Rect::new(50.0, 50.0, 98.0, 98.0), &png, false)
        })
        .expect("add image");
    }
    let source = list(&doc.doc_id, page);
    let image = source
        .objects
        .iter()
        .rev()
        .find(|o| o.object_type == PageObjectType::Image)
        .unwrap()
        .clone();
    let target = if page + 1 < doc.info.page_count { page + 1 } else { page - 1 };
    let target_before = list(&doc.doc_id, target);
    let (dx, dy) = (10.0, 20.0);
    let r = duplicate(
        &doc.doc_id,
        page,
        vec![form.object_id, image.object_id],
        [dx, dy],
        Some(target),
    )
    .expect("cross-page duplicate");
    assert_eq!(r.objects.len(), target_before.objects.len() + 2);
    let copies: Vec<&PageObject> = r.new_object_ids.iter().map(|&id| &r.objects[id as usize]).collect();
    assert_eq!(copies.len(), 2);
    assert!(copies.iter().any(|c| c.object_type == PageObjectType::Form && near(&c.rect, &moved(&form.rect, dx, dy))));
    assert!(copies.iter().any(|c| c.object_type == PageObjectType::Image && near(&c.rect, &moved(&image.rect, dx, dy))));
    assert_eq!(list(&doc.doc_id, page).objects.len(), source.objects.len(), "the source page is untouched");

    // The copies survive a save; the form's text is now on the target page too.
    let form_text = form.text.clone().unwrap_or_default();
    let saved = reopen(save_bytes(&doc.doc_id));
    assert_eq!(list(&saved.doc_id, target).objects.len(), target_before.objects.len() + 2);
    if let Some(word) = form_text.split_whitespace().find(|w| w.len() > 4) {
        assert!(page_text(&saved.doc_id, target).contains(word), "{word:?} copied");
    }

    // Refusals.
    let d = doc.doc_id.clone();
    let err = with_state(move |st| objects::duplicate(st, &d, 0, &[0], 1, [0.0, 0.0], None)).unwrap_err();
    assert_eq!(err.code, ErrorCode::Stale);
    let err = duplicate(&doc.doc_id, page, vec![99_999], [0.0, 0.0], None).unwrap_err();
    assert_eq!(err.code, ErrorCode::NotFound);
}
