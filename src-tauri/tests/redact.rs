//! Stage 1 (a) — redaction.
//!
//! The only claim that matters: after `apply_redactions`, the text is **gone from the saved
//! file**, not merely covered. Every test here re-extracts the text from a reopened document
//! rather than trusting the in-memory page.

mod common;
use common::*;

use seepdf_lib::engine::redact;
use seepdf_lib::engine::registry;
use seepdf_lib::engine::render::tiles;
use seepdf_lib::engine::text;
use seepdf_lib::ipc::types::{PageIndex, Rect, RedactOptions};
use seepdf_lib::ipc::ErrorCode;

/// The word redacted in most of these tests, and the run it belongs to.
const TARGET: &str = "Gal";

fn page_text(doc_id: &str, page: PageIndex) -> String {
    let doc_id = doc_id.to_string();
    with_doc(&doc_id.clone(), move |doc| {
        Ok(text::layer::page_text(doc, page)?.text.clone())
    })
    .expect("page text")
}

/// The tight box of the first occurrence of `needle`, inflated by a point.
fn word_rect(doc_id: &str, page: PageIndex, needle: &str) -> Rect {
    let doc_id = doc_id.to_string();
    let needle = needle.to_string();
    with_doc(&doc_id.clone(), move |doc| {
        let layer = text::layer::layer(doc, page)?;
        let chars: Vec<char> = layer
            .chars
            .iter()
            .map(|c| char::from_u32(c.codepoint).unwrap_or('\u{fffd}'))
            .collect();
        let text: String = chars.iter().collect();
        let byte_pos = text
            .find(&needle)
            .unwrap_or_else(|| panic!("{needle:?} is not on page {page}"));
        let start = text[..byte_pos].chars().count();
        let mut rect: Option<Rect> = None;
        for c in layer.chars.iter().skip(start).take(needle.chars().count()) {
            rect = Some(match rect {
                Some(r) => r.union(&c.tight),
                None => c.tight,
            });
        }
        let r = rect.expect("the word has character boxes");
        Ok(Rect::new(r.l - 1.0, r.b - 1.0, r.r + 1.0, r.t + 1.0))
    })
    .expect("word rect")
}

fn preview(
    doc_id: &str,
    page: PageIndex,
    rects: Vec<Rect>,
) -> seepdf_lib::ipc::types::RedactPreview {
    let doc_id = doc_id.to_string();
    with_doc(&doc_id.clone(), move |doc| {
        redact::preview(doc, page, &rects)
    })
    .expect("redact preview")
}

fn apply(
    doc_id: &str,
    page: PageIndex,
    rects: Vec<Rect>,
    fill: [u8; 3],
) -> Result<u32, seepdf_lib::ipc::EngineError> {
    let doc_id = doc_id.to_string();
    with_state(move |st| {
        redact::apply_verified(
            st,
            &doc_id,
            page,
            &rects,
            &RedactOptions {
                fill,
                overlay_text: None,
                ungroup: false,
            },
        )
    })
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

fn generation(doc_id: &str) -> u32 {
    let doc_id = doc_id.to_string();
    with_state(move |st| Ok(st.doc(&doc_id)?.generation)).expect("generation")
}

/// RGBA pixels of `rect`, for comparing a rolled-back page against a pristine one.
fn render(doc_id: &str, page: PageIndex, rect: Rect) -> Vec<u8> {
    let doc_id = doc_id.to_string();
    with_state(move |st| tiles::render_raw_buffer(st, &doc_id, page, 2.0, Some(rect)))
        .expect("render rect")
}

fn out_dir() -> std::path::PathBuf {
    let dir = fixture("out").join("stage1a");
    std::fs::create_dir_all(&dir).expect("create fixtures/out/stage1a");
    dir
}

/// The preview names every object the marks reach, and says so *before* anything is removed.
/// v0.3 (R2): a run the mark covers only partly is **split**, so there is no collateral.
#[test]
fn redact_preview_reports_split_not_collateral() {
    let doc = open("tracemonkey.pdf");
    let rect = word_rect(&doc.doc_id, 0, TARGET);
    let plan = preview(&doc.doc_id, 0, vec![rect]);

    assert_eq!(plan.page, 0);
    let run = plan
        .text_objects
        .iter()
        .find(|o| o.text.contains(TARGET))
        .unwrap_or_else(|| {
            panic!(
                "the object holding {TARGET:?} is listed: {:?}",
                plan.text_objects
                    .iter()
                    .map(|o| &o.text)
                    .collect::<Vec<_>>()
            )
        });
    assert!(run.split, "\"Andreas Gal\" is split, not removed whole");
    assert!(!run.fully_inside);
    assert!(
        plan.collateral.is_empty(),
        "nothing outside the mark is lost: {:?}",
        plan.collateral
    );
    assert!(
        plan.form_fields.is_empty(),
        "tracemonkey.pdf has no form fields"
    );
    assert!(plan.groups.is_empty());

    // Nothing was removed by the preview.
    assert!(page_text(&doc.doc_id, 0).contains(TARGET));
}

/// The DoD test: the word is not in the **saved file**.
#[test]
fn redact_removes_text_from_saved_file() {
    let doc = open("tracemonkey.pdf");
    let rect = word_rect(&doc.doc_id, 0, TARGET);
    let before = page_text(&doc.doc_id, 0);
    let occurrences_before = before.matches(TARGET).count();
    assert!(occurrences_before > 0, "the fixture contains {TARGET:?}");

    let removed = apply(&doc.doc_id, 0, vec![rect], [0, 0, 0]).expect("apply redactions");
    assert!(removed >= 1, "at least one page object was removed");

    let after = page_text(&doc.doc_id, 0);
    assert!(
        after.matches(TARGET).count() < occurrences_before,
        "the text is gone from the in-memory page"
    );

    let bytes = save_bytes(&doc.doc_id);
    std::fs::write(out_dir().join("redacted.pdf"), &bytes).expect("write pdf");
    let saved = reopen(bytes);
    let from_disk = page_text(&saved.doc_id, 0);
    assert!(
        from_disk.matches(TARGET).count() < occurrences_before,
        "the text is gone from the saved file — this is the whole point of the feature"
    );
    assert!(
        from_disk.len() < before.len(),
        "the page lost characters ({} -> {})",
        before.len(),
        from_disk.len()
    );

    // And a black box is actually drawn over the area.
    let doc_id = saved.doc_id.clone();
    let buffer = with_state(move |st| tiles::render_raw_buffer(st, &doc_id, 0, 2.0, Some(rect)))
        .expect("render the redacted rect");
    let dark = buffer[32..]
        .chunks_exact(4)
        .filter(|p| p[0] < 40 && p[1] < 40 && p[2] < 40)
        .count();
    let total = buffer[32..].len() / 4;
    assert!(
        dark > total / 2,
        "the fill box covers the mark: {dark} of {total} pixels are black"
    );
}

/// Several marks in one call, and the black box takes the requested colour.
#[test]
fn redact_multiple_rects_and_fill_colour() {
    let doc = open("tracemonkey.pdf");
    let first = word_rect(&doc.doc_id, 0, TARGET);
    let second = word_rect(&doc.doc_id, 0, "Abstract");
    let removed =
        apply(&doc.doc_id, 0, vec![first, second], [255, 0, 0]).expect("apply redactions");
    assert!(removed >= 2, "both runs went: {removed} objects removed");

    let text = page_text(&doc.doc_id, 0);
    assert!(!text.contains("Abstract"), "the second mark applied too");

    let doc_id = doc.doc_id.clone();
    let buffer = with_state(move |st| tiles::render_raw_buffer(st, &doc_id, 0, 2.0, Some(second)))
        .expect("render");
    let red = buffer[32..]
        .chunks_exact(4)
        .filter(|p| p[0] > 200 && p[1] < 60 && p[2] < 60)
        .count();
    assert!(red > 0, "the fill colour from the options is used");
}

/// The post-condition is real. **`TAMReview.pdf` page 1 keeps 98.5 % of its characters inside
/// Form XObjects** (2738 of 2779; text spike §1), which PDFium can read but not remove:
/// `remove_object_at_index` only reaches top-level page objects. A naive implementation would
/// delete nothing, draw the black box and report success — a fake redaction.
///
/// SeePDF verifies against the strings the *text layer* says are under the mark, not against
/// the objects it managed to delete, so this case fails with `verifyFailed` and
/// `registry::mutate` restores the snapshot: same text, same generation, no box on the page.
#[test]
fn redact_verify_rollback() {
    const PAGE: PageIndex = 1;
    let doc = open("TAMReview.pdf");
    let needle = unremovable_word(&doc.doc_id, PAGE);
    let before_text = page_text(&doc.doc_id, PAGE);
    let before_generation = generation(&doc.doc_id);
    let before_pixels = render(&doc.doc_id, PAGE, word_rect(&doc.doc_id, PAGE, &needle));

    let rect = word_rect(&doc.doc_id, PAGE, &needle);
    let err = apply(&doc.doc_id, PAGE, vec![rect], [0, 0, 0]).expect_err(
        "text inside a Form XObject cannot be removed, so the apply must refuse to claim success",
    );
    assert_eq!(err.code, ErrorCode::VerifyFailed, "{err}");
    assert_eq!(err.page, Some(PAGE));
    assert!(
        err.message.contains(&needle),
        "the error names the text that survived: {}",
        err.message
    );

    // …and nothing happened.
    assert_eq!(
        generation(&doc.doc_id),
        before_generation,
        "a rolled-back redaction does not bump the generation"
    );
    assert_eq!(
        page_text(&doc.doc_id, PAGE),
        before_text,
        "the page text was restored"
    );
    assert_eq!(
        render(&doc.doc_id, PAGE, rect),
        before_pixels,
        "no black box was left behind by the rolled-back command"
    );
}

/// A word on `page` whose characters all belong to a Form XObject, i.e. one SeePDF cannot
/// remove. `TextLayer::object_id` is `u32::MAX` for exactly those.
fn unremovable_word(doc_id: &str, page: PageIndex) -> String {
    let doc_id = doc_id.to_string();
    with_doc(&doc_id.clone(), move |doc| {
        let layer = text::layer::layer(doc, page)?;
        let mut word = String::new();
        let mut unremovable = true;
        let mut best: Option<String> = None;
        for c in &layer.chars {
            let ch = char::from_u32(c.codepoint).unwrap_or('\u{fffd}');
            if ch.is_alphanumeric() {
                word.push(ch);
                unremovable &= c.object_id == u32::MAX;
            } else {
                if unremovable && word.chars().count() >= 6 {
                    best = Some(word.clone());
                    break;
                }
                word.clear();
                unremovable = true;
            }
        }
        best.ok_or_else(|| {
            seepdf_lib::ipc::EngineError::not_found("no XObject-only word on this page")
        })
    })
    .expect("a word inside a Form XObject")
}

/// An empty mark list and a mark over a form field are refused before anything is changed.
#[test]
fn redact_refuses_before_changing_anything() {
    let doc = open("tracemonkey.pdf");
    let before_generation = generation(&doc.doc_id);
    let err = apply(&doc.doc_id, 0, vec![], [0, 0, 0]).expect_err("no rects");
    assert_eq!(err.code, ErrorCode::InvalidArgument);
    assert_eq!(
        generation(&doc.doc_id),
        before_generation,
        "a rejected command does not bump the generation"
    );

    let form = open("160F-2019.pdf");
    let widget_rect = {
        let doc_id = form.doc_id.clone();
        with_doc(&doc_id.clone(), move |doc| {
            let fields = seepdf_lib::engine::form::list(doc, Some(0))?;
            Ok(fields
                .iter()
                .map(|f| f.rect)
                .find(|r| r.width() > 40.0)
                .expect("a widget"))
        })
        .expect("widget rect")
    };
    let form_generation = generation(&form.doc_id);
    let err = apply(&form.doc_id, 0, vec![widget_rect], [0, 0, 0])
        .expect_err("a form field under the mark refuses");
    assert_eq!(err.code, ErrorCode::Unsupported);
    assert_eq!(
        generation(&form.doc_id),
        form_generation,
        "the refused redaction left the document untouched"
    );
}

/// `first_survivor`'s counting rule, exercised through the real engine: redacting one
/// occurrence of a word that appears several times must succeed, not trip the verifier.
#[test]
fn redact_handles_a_repeated_word() {
    let doc = open("tracemonkey.pdf");
    let text = page_text(&doc.doc_id, 0);
    // "the" occurs many times on the first page of tracemonkey.
    let needle = "the";
    let before = text.matches(needle).count();
    assert!(before > 3, "the fixture repeats {needle:?}");

    let rect = word_rect(&doc.doc_id, 0, needle);
    apply(&doc.doc_id, 0, vec![rect], [0, 0, 0])
        .expect("redacting one occurrence of a repeated word must not trip the verifier");

    let after = page_text(&doc.doc_id, 0).matches(needle).count();
    assert!(
        after < before,
        "at least one occurrence went ({before} -> {after})"
    );
}

// ---------------------------------------------------------------------------------------
// Stage 8 — apply_redactions_batch
// ---------------------------------------------------------------------------------------

fn apply_batch(
    doc_id: &str,
    marks: Vec<(PageIndex, Vec<Rect>)>,
) -> Result<seepdf_lib::ipc::types::RedactBatchResult, seepdf_lib::ipc::EngineError> {
    let doc_id = doc_id.to_string();
    let marks: Vec<seepdf_lib::ipc::types::RedactBatchMark> = marks
        .into_iter()
        .map(|(page, rects)| seepdf_lib::ipc::types::RedactBatchMark { page, rects })
        .collect();
    with_state(move |st| {
        redact::apply_batch(
            st,
            &doc_id,
            &marks,
            &RedactOptions {
                fill: [0, 0, 0],
                overlay_text: None,
                ungroup: false,
            },
        )
    })
}

/// A word of at least `min` letters that occurs exactly once on `page`.
fn unique_word(doc_id: &str, page: PageIndex, min: usize) -> String {
    let text = page_text(doc_id, page);
    text.split(|c: char| !c.is_alphabetic())
        .filter(|w| w.chars().count() >= min)
        .find(|w| text.matches(w).count() == 1)
        .expect("a unique word")
        .to_string()
}

/// Two pages in one call: one generation, one undo step, both words gone from the saved file.
#[test]
fn redact_batch_two_pages_is_one_undo_step() {
    let doc = open("tracemonkey.pdf");
    let word1 = unique_word(&doc.doc_id, 1, 7);
    let r0 = word_rect(&doc.doc_id, 0, TARGET);
    let r1 = word_rect(&doc.doc_id, 1, &word1);
    let before_generation = generation(&doc.doc_id);
    let (text0, text1) = (page_text(&doc.doc_id, 0), page_text(&doc.doc_id, 1));

    // Out of order, and page 1 split over two entries: merged and sorted.
    let r =
        apply_batch(&doc.doc_id, vec![(1, vec![r1]), (0, vec![r0]), (1, vec![])]).expect("batch");
    assert!(r.verified);
    assert_eq!(r.pages, vec![0, 1]);
    assert!(r.removed_objects >= 2, "{}", r.removed_objects);
    assert_eq!(
        r.doc_generation,
        before_generation + 1,
        "one generation for both pages"
    );
    let info = with_doc(&doc.doc_id, |d| Ok(d.info())).unwrap();
    assert_eq!(info.undo_label.as_deref(), Some("undo.redact"));
    assert!(!page_text(&doc.doc_id, 0).contains(TARGET));
    assert!(!page_text(&doc.doc_id, 1).contains(&word1));

    let saved = reopen(save_bytes(&doc.doc_id));
    assert!(!page_text(&saved.doc_id, 0).contains(TARGET));
    assert!(!page_text(&saved.doc_id, 1).contains(&word1));

    // One undo restores both pages.
    let d = doc.doc_id.clone();
    let undone = with_state(move |st| registry::undo(st, &d, false)).expect("undo");
    assert!(!undone.can_undo, "the batch was a single step");
    assert_eq!(page_text(&doc.doc_id, 0), text0);
    assert_eq!(page_text(&doc.doc_id, 1), text1);
}

/// A page that fails verification rolls back the pages already redacted in the same call; a
/// form field under a mark refuses before anything changes.
#[test]
fn redact_batch_rolls_back_every_page() {
    let doc = open("TAMReview.pdf");
    let needle = unremovable_word(&doc.doc_id, 1);
    let word0 = unique_word(&doc.doc_id, 0, 6);
    let (text0, text1) = (page_text(&doc.doc_id, 0), page_text(&doc.doc_id, 1));
    let before_generation = generation(&doc.doc_id);
    let err = apply_batch(
        &doc.doc_id,
        vec![
            (0, vec![word_rect(&doc.doc_id, 0, &word0)]),
            (1, vec![word_rect(&doc.doc_id, 1, &needle)]),
        ],
    )
    .expect_err("page 1's text is inside a Form XObject");
    assert_eq!(err.code, ErrorCode::VerifyFailed, "{err}");
    assert_eq!(err.page, Some(1));
    assert_eq!(generation(&doc.doc_id), before_generation);
    assert_eq!(
        page_text(&doc.doc_id, 0),
        text0,
        "page 0 was rolled back too"
    );
    assert_eq!(page_text(&doc.doc_id, 1), text1);
    let can_undo = with_doc(&doc.doc_id, |d| Ok(d.info().can_undo)).unwrap();
    assert!(!can_undo, "no undo step was left behind");

    let form = open("160F-2019.pdf");
    let widget = {
        let doc_id = form.doc_id.clone();
        with_doc(&doc_id.clone(), move |doc| {
            let fields = seepdf_lib::engine::form::list(doc, Some(0))?;
            Ok(fields
                .iter()
                .map(|f| f.rect)
                .find(|r| r.width() > 40.0)
                .expect("a widget"))
        })
        .unwrap()
    };
    let g = generation(&form.doc_id);
    let err = apply_batch(&form.doc_id, vec![(0, vec![widget])]).expect_err("form field");
    assert_eq!(err.code, ErrorCode::Unsupported);
    assert_eq!(generation(&form.doc_id), g);
    let err = apply_batch(&form.doc_id, vec![(0, vec![])]).expect_err("no rects");
    assert_eq!(err.code, ErrorCode::InvalidArgument);
    let err = apply_batch(&form.doc_id, vec![(999, vec![widget])]).expect_err("bad page");
    assert_eq!(err.code, ErrorCode::InvalidArgument);
}

// ---------------------------------------------------------------------------------------
// v0.3 pkg1 — R1 image blanking, R2 word-level split, R4 groups
// ---------------------------------------------------------------------------------------

use pdfium_render::prelude::*;
use seepdf_lib::engine::redact::{image as redact_image, raw as redact_raw};

fn apply_opts(
    doc_id: &str,
    page: PageIndex,
    rects: Vec<Rect>,
    ungroup: bool,
) -> Result<u32, seepdf_lib::ipc::EngineError> {
    let doc_id = doc_id.to_string();
    with_state(move |st| {
        redact::apply_verified(
            st,
            &doc_id,
            page,
            &rects,
            &RedactOptions {
                fill: [0, 0, 0],
                overlay_text: None,
                ungroup,
            },
        )
    })
}

/// A deterministic, busy RGB image: no two neighbours alike, never black.
fn busy_rgb(w: u32, h: u32) -> image::RgbImage {
    let mut img = image::RgbImage::new(w, h);
    for (x, y, px) in img.enumerate_pixels_mut() {
        *px = image::Rgb([
            40 + ((x * 7 + y * 3) % 200) as u8,
            40 + ((x * 3 + y * 11) % 200) as u8,
            40 + ((x * 13 + y * 5) % 200) as u8,
        ]);
    }
    img
}

/// A smooth image (JPEG-friendly), never black.
fn smooth_rgb(w: u32, h: u32) -> image::RgbImage {
    let mut img = image::RgbImage::new(w, h);
    for (x, y, px) in img.enumerate_pixels_mut() {
        *px = image::Rgb([80 + (x * 150 / w) as u8, 80 + (y * 150 / h) as u8, 200]);
    }
    img
}

/// A new one-page document (600 × 800 pt) built with PDFium, opened in the engine.
fn built_doc(
    build: impl for<'a> FnOnce(&PdfDocument<'a>, &mut PdfPage<'a>) + Send + 'static,
) -> TestDoc {
    let bytes = with_state(move |st| {
        let pdfium = st.pdfium;
        let mut document = pdfium.create_new_pdf().expect("new pdf");
        {
            let mut page = document
                .pages_mut()
                .create_page_at_end(PdfPagePaperSize::Custom(
                    PdfPoints::new(600.0),
                    PdfPoints::new(800.0),
                ))
                .expect("page");
            build(&document, &mut page);
            page.regenerate_content().expect("regenerate");
        }
        Ok(document.save_to_bytes().expect("save"))
    })
    .expect("build document");
    reopen(bytes)
}

fn add_image<'a>(
    document: &PdfDocument<'a>,
    page: &mut PdfPage<'a>,
    img: image::DynamicImage,
    m: [f32; 6],
) {
    let mut object = PdfPageImageObject::new(document, &img).expect("image object");
    object
        .apply_matrix(PdfMatrix::new(m[0], m[1], m[2], m[3], m[4], m[5]))
        .expect("matrix");
    page.objects_mut()
        .add_image_object(object)
        .expect("add image");
}

fn add_jpeg<'a>(
    document: &PdfDocument<'a>,
    page: &mut PdfPage<'a>,
    img: &image::RgbImage,
    m: [f32; 6],
) {
    let mut jpeg = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 92)
        .encode_image(img)
        .expect("encode jpeg");
    let mut object = PdfPageImageObject::new_from_jpeg_reader(document, std::io::Cursor::new(jpeg))
        .expect("jpeg object");
    object
        .apply_matrix(PdfMatrix::new(m[0], m[1], m[2], m[3], m[4], m[5]))
        .expect("matrix");
    page.objects_mut()
        .add_image_object(object)
        .expect("add jpeg");
}

/// The first image object's own pixels (no matrix, no mask) and its matrix.
fn first_image(doc_id: &str) -> (redact_raw::RawBitmap, [f32; 6], Vec<String>) {
    let doc_id = doc_id.to_string();
    with_doc(&doc_id, |d| {
        let bindings = d.bindings();
        let page = d.page(0)?;
        for i in 0..page.objects().len() {
            let object = page.objects().get(i).expect("object");
            if let Some(img) = object.as_image_object() {
                let filters = img.filters().iter().map(|f| f.name().to_string()).collect();
                let h = redact_raw::object_at(bindings, page, i)?;
                let bitmap = redact_raw::image_bitmap(bindings, h).expect("bitmap");
                return Ok((bitmap, redact_raw::matrix(bindings, h), filters));
            }
        }
        Err(seepdf_lib::ipc::EngineError::not_found("no image"))
    })
    .expect("first image")
}

fn px(b: &redact_raw::RawBitmap, x: usize, y: usize) -> [u8; 3] {
    let o = y * b.stride + x * b.bpp();
    if b.bpp() == 1 {
        [b.data[o]; 3]
    } else {
        [b.data[o], b.data[o + 1], b.data[o + 2]]
    }
}

/// Every image stream of a saved file, decoded where lopdf can (Flate).
fn image_streams(bytes: &[u8]) -> Vec<Vec<u8>> {
    let doc = lopdf::Document::load_mem(bytes).expect("lopdf load");
    doc.objects
        .values()
        .filter_map(|o| o.as_stream().ok())
        .filter(|s| {
            s.dict
                .get(b"Subtype")
                .and_then(|o| o.as_name())
                .is_ok_and(|n| n == b"Image")
        })
        .map(|s| {
            s.decompressed_content()
                .unwrap_or_else(|_| s.content.clone())
        })
        .collect()
}

fn object_count(doc_id: &str) -> usize {
    with_doc(doc_id, |d| Ok(d.page(0)?.objects().len())).expect("count")
}

/// R1 (1): a full-page scan-like image with a partial mark. The pixels under the mark are
/// black **in the image data of the saved file**, everything else is untouched, and the old
/// stream is not left behind in the file.
#[test]
fn redact_blanks_partially_covered_image_pixels() {
    // 300 × 400 px over 600 × 800 pt: one image pixel = 2 pt.
    let doc = built_doc(|document, page| {
        add_image(
            document,
            page,
            image::DynamicImage::ImageRgb8(busy_rgb(300, 400)),
            [600.0, 0.0, 0.0, 800.0, 0.0, 0.0],
        )
    });
    let (original, _, _) = first_image(&doc.doc_id);
    let original_bytes = save_bytes(&doc.doc_id);
    let original_streams = image_streams(&original_bytes);
    let mark = Rect::new(100.0, 300.0, 300.0, 400.0);

    let plan = preview(&doc.doc_id, 0, vec![mark]);
    assert_eq!(plan.image_objects.len(), 1);
    assert!(
        plan.image_objects[0].blank,
        "the scan is blanked, not removed"
    );
    assert!(!plan.image_objects[0].fully_inside);

    let before_outside = render(&doc.doc_id, 0, Rect::new(320.0, 420.0, 580.0, 780.0));
    let changed = apply_opts(&doc.doc_id, 0, vec![mark], false).expect("apply");
    assert!(changed >= 1);
    assert_eq!(
        object_count(&doc.doc_id),
        2,
        "the image stays, plus the box"
    );

    let bytes = save_bytes(&doc.doc_id);
    std::fs::write(out_dir().join("v03-image-blank.pdf"), &bytes).expect("write");
    let saved = reopen(bytes.clone());
    let (after, _, _) = first_image(&saved.doc_id);
    assert_eq!((after.width, after.height), (300, 400));
    // x 100..300 pt → px 50..150; y 300..400 pt → rows (800-400)/2 = 200 .. 250.
    for y in 0..400 {
        for x in 0..300 {
            let inside = (50..150).contains(&x) && (200..250).contains(&y);
            if inside {
                assert_eq!(
                    px(&after, x, y),
                    [0, 0, 0],
                    "pixel ({x}, {y}) under the mark is black"
                );
            } else {
                assert_eq!(
                    px(&after, x, y),
                    px(&original, x, y),
                    "pixel ({x}, {y}) is unchanged"
                );
            }
        }
    }
    // The region renders uniformly black; far from it nothing changed.
    let inside = render(&saved.doc_id, 0, Rect::new(101.0, 301.0, 299.0, 399.0));
    assert!(inside[32..]
        .chunks_exact(4)
        .all(|p| p[0] < 8 && p[1] < 8 && p[2] < 8));
    assert_eq!(
        render(&saved.doc_id, 0, Rect::new(320.0, 420.0, 580.0, 780.0)),
        before_outside,
        "pixels outside the mark are unchanged"
    );
    // The old pixels are nowhere in the file.
    let after_streams = image_streams(&bytes);
    for old in original_streams.iter().filter(|s| s.len() > 1000) {
        assert!(
            !after_streams.contains(old),
            "the unredacted image stream must not survive in the saved file"
        );
    }
}

/// R1 (2): a `/DCTDecode` image stays a JPEG; the marked pixels read back black within JPEG
/// tolerance, the rest within re-encoding noise.
#[test]
fn redact_blanks_a_jpeg_image() {
    let source = smooth_rgb(200, 100);
    let doc = built_doc({
        let source = source.clone();
        move |document, page| {
            add_jpeg(
                document,
                page,
                &source,
                [200.0, 0.0, 0.0, 100.0, 50.0, 50.0],
            )
        }
    });
    let (original, _, filters) = first_image(&doc.doc_id);
    assert_eq!(filters, vec!["DCTDecode".to_string()]);
    // Page (100..150, 70..120) → px 50..100, rows (150-120) 30 .. (150-70) 80.
    let mark = Rect::new(100.0, 70.0, 150.0, 120.0);
    apply_opts(&doc.doc_id, 0, vec![mark], false).expect("apply");

    let saved = reopen(save_bytes(&doc.doc_id));
    let (after, _, filters) = first_image(&saved.doc_id);
    assert_eq!(
        filters,
        vec!["DCTDecode".to_string()],
        "a JPEG stays a JPEG"
    );
    let (mut max, mut sum, mut n) = (0u8, 0u64, 0u64);
    let (mut diff, mut m) = (0u64, 0u64);
    for y in 0..100 {
        for x in 0..200 {
            let p = px(&after, x, y);
            if (50..100).contains(&x) && (30..80).contains(&y) {
                max = max.max(*p.iter().max().unwrap());
                sum += p.iter().map(|&v| v as u64).sum::<u64>();
                n += 3;
            } else if !(46..104).contains(&x) || !(26..84).contains(&y) {
                let o = px(&original, x, y);
                diff += p
                    .iter()
                    .zip(o)
                    .map(|(a, b)| (*a as i32 - b as i32).unsigned_abs() as u64)
                    .sum::<u64>();
                m += 3;
            }
        }
    }
    let mean = sum as f64 / n as f64;
    assert!(
        max <= redact_image::JPEG_MAX && mean <= redact_image::JPEG_MEAN_MAX,
        "blanked JPEG area: max {max}, mean {mean:.1}"
    );
    let outside = diff as f64 / m as f64;
    assert!(
        outside < 6.0,
        "outside the mark only re-encoding noise: {outside:.2}"
    );
}

/// R1 (3): a rotated, scaled image. Every image pixel whose centre lies on the mark is black;
/// every pixel whose whole footprint lies clearly off the mark is unchanged.
#[test]
fn redact_blanks_a_rotated_image() {
    let (c, s) = (30f32.to_radians().cos(), 30f32.to_radians().sin());
    let m = [240.0 * c, 240.0 * s, -160.0 * s, 160.0 * c, 300.0, 200.0];
    let doc = built_doc(move |document, page| {
        add_image(
            document,
            page,
            image::DynamicImage::ImageRgb8(busy_rgb(120, 80)),
            m,
        )
    });
    let (original, matrix, _) = first_image(&doc.doc_id);
    // The page point of the image centre, and a mark around it.
    let (cx, cy) = (
        m[0] * 0.5 + m[2] * 0.5 + m[4],
        m[1] * 0.5 + m[3] * 0.5 + m[5],
    );
    let mark = Rect::new(cx - 30.0, cy - 20.0, cx + 30.0, cy + 20.0);
    apply_opts(&doc.doc_id, 0, vec![mark], false).expect("apply");
    let saved = reopen(save_bytes(&doc.doc_id));
    let (after, after_matrix, _) = first_image(&saved.doc_id);
    for (a, b) in matrix.iter().zip(after_matrix) {
        assert!((a - b).abs() < 1e-3, "the matrix is kept");
    }
    let pixel_pt = 2.0 * (240.0f32 / 120.0).max(160.0 / 80.0);
    let (mut blanked, mut kept) = (0, 0);
    for y in 0..80usize {
        for x in 0..120usize {
            let (u, v) = ((x as f32 + 0.5) / 120.0, 1.0 - (y as f32 + 0.5) / 80.0);
            let (px_, py_) = (m[0] * u + m[2] * v + m[4], m[1] * u + m[3] * v + m[5]);
            let inside = px_ > mark.l && px_ < mark.r && py_ > mark.b && py_ < mark.t;
            let far = px_ < mark.l - pixel_pt
                || px_ > mark.r + pixel_pt
                || py_ < mark.b - pixel_pt
                || py_ > mark.t + pixel_pt;
            if inside {
                assert_eq!(px(&after, x, y), [0, 0, 0], "({x}, {y}) is on the mark");
                blanked += 1;
            } else if far {
                assert_eq!(
                    px(&after, x, y),
                    px(&original, x, y),
                    "({x}, {y}) is off the mark"
                );
                kept += 1;
            }
        }
    }
    assert!(
        blanked > 100 && kept > 5000,
        "blanked {blanked}, kept {kept}"
    );
}

/// R1: an image with transparency cannot be re-encoded faithfully (the `/SMask` would go), so
/// it is removed whole — and the preview says so.
#[test]
fn redact_removes_a_transparent_image_whole() {
    let doc = built_doc(|document, page| {
        let mut rgba = image::RgbaImage::new(100, 100);
        for (x, y, p) in rgba.enumerate_pixels_mut() {
            *p = image::Rgba([200, 50, 50, if (x + y) % 2 == 0 { 255 } else { 90 }]);
        }
        add_image(
            document,
            page,
            image::DynamicImage::ImageRgba8(rgba),
            [200.0, 0.0, 0.0, 200.0, 100.0, 100.0],
        )
    });
    // The fixture really is a masked image (which `FPDFPageObj_HasTransparency` does not see).
    let file = lopdf::Document::load_mem(&save_bytes(&doc.doc_id)).expect("lopdf");
    assert!(
        file.objects
            .values()
            .filter_map(|o| o.as_stream().ok())
            .any(|s| s.dict.has(b"SMask")),
        "PDFium wrote the alpha as an /SMask"
    );
    let mark = Rect::new(150.0, 150.0, 200.0, 200.0);
    let plan = preview(&doc.doc_id, 0, vec![mark]);
    assert_eq!(plan.image_objects.len(), 1);
    assert!(
        !plan.image_objects[0].blank,
        "a transparent image is removed whole"
    );
    apply_opts(&doc.doc_id, 0, vec![mark], false).expect("apply");
    let images = with_doc(&doc.doc_id, |d| {
        let p = d.page(0)?;
        Ok(p.objects()
            .iter()
            .filter(|o| o.as_image_object().is_some())
            .count())
    })
    .unwrap();
    assert_eq!(images, 0, "the image went whole");
}

// --- R2: word-level split -------------------------------------------------------------

/// The characters of `needle`'s first occurrence on `page`, with their tight boxes.
fn char_boxes(doc_id: &str, page: PageIndex, needle: &str) -> Vec<(char, Rect)> {
    let doc_id = doc_id.to_string();
    let needle = needle.to_string();
    with_doc(&doc_id.clone(), move |doc| {
        let layer = text::layer::layer(doc, page)?;
        let chars: Vec<char> = layer
            .chars
            .iter()
            .map(|c| char::from_u32(c.codepoint).unwrap_or('\u{fffd}'))
            .collect();
        let text: String = chars.iter().collect();
        let byte_pos = text
            .find(&needle)
            .unwrap_or_else(|| panic!("{needle:?} is not on page {page}"));
        let start = text[..byte_pos].chars().count();
        Ok(layer
            .chars
            .iter()
            .skip(start)
            .take(needle.chars().count())
            .map(|c| (char::from_u32(c.codepoint).unwrap_or('?'), c.tight))
            .collect())
    })
    .expect("char boxes")
}

fn close(a: &Rect, b: &Rect) -> bool {
    (a.l - b.l).abs() <= 0.5
        && (a.b - b.b).abs() <= 0.5
        && (a.r - b.r).abs() <= 0.5
        && (a.t - b.t).abs() <= 0.5
}

/// Every `(char, box)` of `want` is somewhere on `page` at the same box (±0.5 pt).
fn assert_still_there(doc_id: &str, page: PageIndex, want: &[(char, Rect)]) {
    let doc_id = doc_id.to_string();
    let got: Vec<(char, Rect)> = with_doc(&doc_id, move |doc| {
        let layer = text::layer::layer(doc, page)?;
        Ok(layer
            .chars
            .iter()
            .map(|c| (char::from_u32(c.codepoint).unwrap_or('?'), c.tight))
            .collect())
    })
    .unwrap();
    for (ch, rect) in want.iter().filter(|(c, _)| !c.is_whitespace()) {
        assert!(
            got.iter().any(|(c, r)| c == ch && close(r, rect)),
            "{ch:?} at {rect:?} survives, extractable, in place"
        );
    }
}

/// R2: redacting "Gal" in "Andreas Gal" keeps "Andreas" — extractable, at the same boxes, in
/// the saved file.
#[test]
fn redact_word_keeps_the_rest_of_its_run() {
    let doc = open("tracemonkey.pdf");
    let keep = char_boxes(&doc.doc_id, 0, "Andreas Gal");
    let before = page_text(&doc.doc_id, 0);
    let rect = word_rect(&doc.doc_id, 0, "Gal");
    apply(&doc.doc_id, 0, vec![rect], [0, 0, 0]).expect("apply");

    let saved = reopen(save_bytes(&doc.doc_id));
    let text = page_text(&saved.doc_id, 0);
    assert!(
        text.matches("Gal").count() < before.matches("Gal").count(),
        "\"Gal\" is gone from the saved file"
    );
    assert!(!text.contains("Andreas Gal"));
    assert!(text.contains("Andreas"), "the rest of the run survives");
    assert_still_there(&saved.doc_id, 0, &keep[..7]);
    // The rest of the page is still there too.
    assert!(text.contains("Brendan Eich"));
}

/// R2: a mark in the middle of a run leaves two runs (the second is a copy of the object).
#[test]
fn redact_middle_of_a_run_leaves_both_sides() {
    let doc = open("tracemonkey.pdf");
    let line = "Brendan Eich";
    let boxes = char_boxes(&doc.doc_id, 0, line);
    // Mark "rend" (chars 1..5) only.
    let mark = boxes[1..5]
        .iter()
        .map(|(_, r)| *r)
        .reduce(|a, b| a.union(&b))
        .unwrap();
    let mark = Rect::new(mark.l + 0.3, mark.b, mark.r - 0.3, mark.t);
    let plan = preview(&doc.doc_id, 0, vec![mark]);
    assert!(
        plan.text_objects.iter().any(|o| o.split),
        "{:?}",
        plan.text_objects
    );
    apply(&doc.doc_id, 0, vec![mark], [0, 0, 0]).expect("apply");
    let saved = reopen(save_bytes(&doc.doc_id));
    let text = page_text(&saved.doc_id, 0);
    assert!(!text.contains("Brendan"), "{text:?}");
    let mut kept = vec![boxes[0]];
    kept.extend_from_slice(&boxes[5..]);
    assert_still_there(&saved.doc_id, 0, &kept);
}

/// R2 with a CID font: 한글 in the bundled subset (Identity-H + generated `/ToUnicode`).
#[test]
fn redact_splits_a_hangul_run() {
    let doc = open("tracemonkey.pdf");
    let line = "홍길동 900101-1234567 서울";
    {
        let doc_id = doc.doc_id.clone();
        with_state(move |st| {
            seepdf_lib::engine::objects::add_text(
                st,
                &doc_id,
                0,
                Rect::new(72.0, 40.0, 560.0, 70.0),
                line,
                14.0,
                [0, 0, 0],
                seepdf_lib::ipc::types::TextAlign::Left,
            )
        })
        .expect("add Hangul text");
    }
    // Work on a parsed file, as a user would.
    let doc = reopen(save_bytes(&doc.doc_id));
    let boxes = char_boxes(&doc.doc_id, 0, line);
    let rrn = word_rect(&doc.doc_id, 0, "900101-1234567");
    let plan = preview(&doc.doc_id, 0, vec![rrn]);
    let run = plan
        .text_objects
        .iter()
        .find(|o| o.text.contains("홍길동"))
        .expect("the Hangul run is listed");
    assert!(run.split, "a CID run with /ToUnicode is split");
    assert!(plan.collateral.is_empty(), "{:?}", plan.collateral);

    apply(&doc.doc_id, 0, vec![rrn], [0, 0, 0]).expect("apply");
    let saved = reopen(save_bytes(&doc.doc_id));
    let text = page_text(&saved.doc_id, 0);
    assert!(!text.contains("1234567"), "the number is gone");
    assert!(text.contains("홍길동") && text.contains("서울"), "{text:?}");
    let keep: Vec<(char, Rect)> = boxes
        .iter()
        .filter(|(c, _)| !c.is_ascii_digit() && *c != '-')
        .copied()
        .collect();
    assert_still_there(&saved.doc_id, 0, &keep);
}

/// A minimal PDF built with lopdf: one 400 × 300 page whose content and resources are given.
fn lopdf_doc(
    content: &str,
    resources: lopdf::Dictionary,
    extra: impl FnOnce(&mut lopdf::Document) -> lopdf::Dictionary,
) -> Vec<u8> {
    use lopdf::{dictionary, Document, Object, Stream};
    let mut doc = Document::with_version("1.7");
    let pages_id = doc.new_object_id();
    let mut resources = resources;
    for (k, v) in extra(&mut doc).into_iter() {
        resources.set(k.clone(), v.clone());
    }
    let content_id = doc.add_object(Stream::new(dictionary! {}, content.as_bytes().to_vec()));
    let page_id = doc.add_object(dictionary! {
        "Type" => "Page",
        "Parent" => pages_id,
        "MediaBox" => vec![0.into(), 0.into(), 400.into(), 300.into()],
        "Contents" => content_id,
        "Resources" => resources,
    });
    doc.objects.insert(
        pages_id,
        Object::Dictionary(dictionary! {
            "Type" => "Pages",
            "Kids" => vec![page_id.into()],
            "Count" => 1,
        }),
    );
    let catalog_id = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
    doc.trailer.set("Root", catalog_id);
    let mut out = Vec::new();
    doc.save_to(&mut out).expect("lopdf save");
    out
}

/// R2 fallback: a Type3 font has no font program, so the run is removed whole and the
/// preview names the collateral first.
#[test]
fn redact_type3_run_falls_back_to_whole_removal() {
    use lopdf::{dictionary, Object, Stream};
    let bytes = lopdf_doc(
        "BT /T3 12 Tf 50 150 Td (abc def) Tj ET BT /F1 12 Tf 50 100 Td (Helvetica stays) Tj ET",
        dictionary! {},
        |doc| {
            let glyph = |w: i64| {
                Stream::new(
                    dictionary! {},
                    format!("{w} 0 0 0 {w} 700 d1 0 0 {w} 700 re f").into_bytes(),
                )
            };
            let mut procs = lopdf::Dictionary::new();
            for name in ["a", "b", "c", "d", "e", "f"] {
                let id = doc.add_object(glyph(500));
                procs.set(name, id);
            }
            let space = doc.add_object(Stream::new(dictionary! {}, b"250 0 d0".to_vec()));
            procs.set("space", space);
            let widths: Vec<Object> = (32..=102)
                .map(|c| Object::Integer(if c == 32 { 250 } else { 500 }))
                .collect();
            let t3 = doc.add_object(dictionary! {
                "Type" => "Font",
                "Subtype" => "Type3",
                "FontBBox" => vec![0.into(), 0.into(), 500.into(), 700.into()],
                "FontMatrix" => vec![0.001.into(), 0.into(), 0.into(), 0.001.into(), 0.into(), 0.into()],
                "CharProcs" => procs,
                "Encoding" => dictionary! {
                    "Type" => "Encoding",
                    "Differences" => vec![
                        32.into(), Object::Name(b"space".to_vec()),
                        97.into(), Object::Name(b"a".to_vec()), Object::Name(b"b".to_vec()),
                        Object::Name(b"c".to_vec()), Object::Name(b"d".to_vec()),
                        Object::Name(b"e".to_vec()), Object::Name(b"f".to_vec()),
                    ],
                },
                "FirstChar" => 32,
                "LastChar" => 102,
                "Widths" => widths,
                "Resources" => dictionary! {},
            });
            let f1 = doc.add_object(dictionary! {
                "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica",
                "Encoding" => "WinAnsiEncoding",
            });
            dictionary! { "Font" => dictionary! { "T3" => t3, "F1" => f1 } }
        },
    );
    let doc = reopen(bytes);
    let text = page_text(&doc.doc_id, 0);
    assert!(text.contains("abc def"), "the Type3 run extracts: {text:?}");
    let mark = word_rect(&doc.doc_id, 0, "def");
    let plan = preview(&doc.doc_id, 0, vec![mark]);
    let run = plan
        .text_objects
        .iter()
        .find(|o| o.text.contains("def"))
        .expect("listed");
    assert!(!run.split, "a Type3 run cannot be split");
    assert!(
        plan.collateral.iter().any(|c| c.contains("abc")),
        "the collateral is named: {:?}",
        plan.collateral
    );
    apply(&doc.doc_id, 0, vec![mark], [0, 0, 0]).expect("apply");
    let saved = reopen(save_bytes(&doc.doc_id));
    let text = page_text(&saved.doc_id, 0);
    assert!(!text.contains("def") && !text.contains("abc"), "{text:?}");
    assert!(text.contains("Helvetica stays"));
}

// --- R4: groups (Form XObjects) ---------------------------------------------------------

/// Office-like: the page body lives in one Form XObject — text in a standard font, a box in a
/// calibrated (non-device) colour space and a coloured line.
fn group_doc() -> TestDoc {
    use lopdf::{dictionary, Stream};
    let bytes = lopdf_doc("q 1 0 0 1 20 30 cm /Fm0 Do Q", dictionary! {}, |doc| {
        let f1 = doc.add_object(dictionary! {
            "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica",
            "Encoding" => "WinAnsiEncoding",
        });
        let body = "/CS0 cs 0.9 0.2 0.2 sc 10 150 120 60 re f \
                    0 0 1 RG 2 w 150 150 m 340 220 l S \
                    BT /F1 14 Tf 0 0 0 rg 10 100 Td (Andreas Gal secret 900101) Tj ET \
                    BT /F1 12 Tf 10 60 Td (Second line stays) Tj ET";
        let form = doc.add_object(Stream::new(
            dictionary! {
                "Type" => "XObject",
                "Subtype" => "Form",
                "BBox" => vec![0.into(), 0.into(), 360.into(), 240.into()],
                "Resources" => dictionary! {
                    "Font" => dictionary! { "F1" => f1 },
                    "ColorSpace" => dictionary! {
                        "CS0" => vec![
                            lopdf::Object::Name(b"CalRGB".to_vec()),
                            dictionary! { "WhitePoint" => vec![0.9505.into(), 1.into(), 1.089.into()] }.into(),
                        ],
                    },
                },
            },
            body.as_bytes().to_vec(),
        ));
        dictionary! { "XObject" => dictionary! { "Fm0" => form } }
    });
    reopen(bytes)
}

fn render_full(doc_id: &str) -> Vec<u8> {
    let doc_id = doc_id.to_string();
    with_state(move |st| tiles::render_raw_buffer(st, &doc_id, 0, 2.0, None)).expect("render")
}

/// Mean absolute channel difference and the share of pixels differing by more than 24.
fn pixel_diff(a: &[u8], b: &[u8]) -> (f64, f64) {
    assert_eq!(a.len(), b.len());
    let (mut sum, mut off, mut n) = (0u64, 0u64, 0u64);
    for (p, q) in a[32..].chunks_exact(4).zip(b[32..].chunks_exact(4)) {
        let d: u32 = (0..3)
            .map(|k| (p[k] as i32 - q[k] as i32).unsigned_abs())
            .sum();
        sum += d as u64;
        if d > 24 {
            off += 1;
        }
        n += 1;
    }
    (sum as f64 / (3 * n) as f64, off as f64 / n as f64)
}

/// R4: 그룹 해제 moves the form's children onto the page; the page renders the same (the
/// calibrated colour is re-set as RGB, not dropped to black) and it is one undo step.
#[test]
fn ungroup_object_keeps_the_page_identical() {
    let doc = group_doc();
    let before = render_full(&doc.doc_id);
    let generation = generation(&doc.doc_id);
    let result = {
        let doc_id = doc.doc_id.clone();
        with_state(move |st| {
            seepdf_lib::engine::objects::ungroup::ungroup(st, &doc_id, 0, 0, generation)
        })
        .expect("ungroup")
    };
    assert_eq!(
        result.new_object_ids,
        vec![0, 1, 2, 3],
        "four children, in order"
    );
    assert!(result
        .objects
        .iter()
        .all(|o| o.object_type != seepdf_lib::ipc::types::PageObjectType::Form));
    assert!(result.objects[2]
        .text
        .as_deref()
        .is_some_and(|t| t.contains("Andreas Gal")));
    assert_eq!(
        result.objects[2].editable,
        seepdf_lib::ipc::types::Editability::Full
    );
    let info = with_doc(&doc.doc_id, |d| Ok(d.info())).unwrap();
    assert_eq!(info.undo_label.as_deref(), Some("undo.ungroup"));

    let (mean, share) = pixel_diff(&before, &render_full(&doc.doc_id));
    assert!(
        mean < 1.0 && share < 0.002,
        "in memory: mean {mean:.3}, share {share:.4}"
    );
    let saved = reopen(save_bytes(&doc.doc_id));
    let (mean, share) = pixel_diff(&before, &render_full(&saved.doc_id));
    assert!(
        mean < 1.0 && share < 0.002,
        "saved: mean {mean:.3}, share {share:.4}"
    );

    // Refused on something that is not a group, and on a stale generation.
    let doc_id = doc.doc_id.clone();
    let g = generation + 1;
    let err =
        with_state(move |st| seepdf_lib::engine::objects::ungroup::ungroup(st, &doc_id, 0, 0, g))
            .expect_err("object 0 is now a path");
    assert_eq!(err.code, ErrorCode::InvalidArgument);
    let doc_id = doc.doc_id.clone();
    let err =
        with_state(move |st| seepdf_lib::engine::objects::ungroup::ungroup(st, &doc_id, 0, 0, 0))
            .expect_err("stale");
    assert_eq!(err.code, ErrorCode::Stale);

    // One undo restores the group.
    let d = doc.doc_id.clone();
    with_state(move |st| registry::undo(st, &d, false)).expect("undo");
    let forms = with_doc(&doc.doc_id, |d| {
        Ok(d.page(0)?
            .objects()
            .iter()
            .filter(|o| o.object_type() == PdfPageObjectType::XObjectForm)
            .count())
    })
    .unwrap();
    assert_eq!(forms, 1);
}

/// R4: a word inside a group. Without `ungroup` the apply refuses (the pre-v0.3 contract);
/// with it the group is ungrouped in the same undo step, the word goes and its neighbours stay.
#[test]
fn redact_inside_a_group_with_ungroup() {
    let doc = group_doc();
    let keep = char_boxes(&doc.doc_id, 0, "Andreas");
    let mark = word_rect(&doc.doc_id, 0, "Gal");
    let plan = preview(&doc.doc_id, 0, vec![mark]);
    assert_eq!(plan.groups, vec![0], "the preview names the group");

    let g = generation(&doc.doc_id);
    let err = apply_opts(&doc.doc_id, 0, vec![mark], false).expect_err("no ungroup");
    assert_eq!(err.code, ErrorCode::VerifyFailed);
    assert_eq!(generation(&doc.doc_id), g, "nothing happened");

    apply_opts(&doc.doc_id, 0, vec![mark], true).expect("apply with ungroup");
    assert_eq!(generation(&doc.doc_id), g + 1, "one step");
    let info = with_doc(&doc.doc_id, |d| Ok(d.info())).unwrap();
    assert_eq!(info.undo_label.as_deref(), Some("undo.redact"));
    let saved = reopen(save_bytes(&doc.doc_id));
    let text = page_text(&saved.doc_id, 0);
    assert!(!text.contains("Gal"), "{text:?}");
    assert!(
        text.contains("Andreas") && text.contains("secret") && text.contains("Second line stays")
    );
    assert_still_there(&saved.doc_id, 0, &keep);
}

/// R4 on the real fixture: `TAMReview.pdf` p.2 keeps its text in a Form XObject (the case
/// `redact_verify_rollback` refuses). With `ungroup` the word is removed and verified.
#[test]
fn redact_tamreview_with_ungroup() {
    const PAGE: PageIndex = 1;
    let doc = open("TAMReview.pdf");
    let needle = unremovable_word(&doc.doc_id, PAGE);
    let before = page_text(&doc.doc_id, PAGE).matches(&needle).count();
    let rect = word_rect(&doc.doc_id, PAGE, &needle);
    let plan = preview(&doc.doc_id, PAGE, vec![rect]);
    assert!(!plan.groups.is_empty());
    apply_opts(&doc.doc_id, PAGE, vec![rect], true).expect("apply with ungroup");
    let saved = reopen(save_bytes(&doc.doc_id));
    assert!(page_text(&saved.doc_id, PAGE).matches(&needle).count() < before);
}

/// R1: one image XObject drawn on two pages. Blanking it on page 1 must not show page 2's
/// copy as blanked in memory (PDFium shares the decoded image) when the file keeps it — what
/// the viewer shows after the apply is what the saved file holds.
#[test]
fn redact_blank_of_a_shared_image_touches_only_its_page() {
    use lopdf::{dictionary, Document, Object, Stream};
    let mut file = Document::with_version("1.7");
    let pages_id = file.new_object_id();
    let mut samples = Vec::new();
    for y in 0..20u32 {
        for x in 0..20u32 {
            samples.extend_from_slice(&[(50 + x * 5) as u8, (50 + y * 5) as u8, 180]);
        }
    }
    let img = file.add_object(Stream::new(
        dictionary! { "Type" => "XObject", "Subtype" => "Image", "Width" => 20, "Height" => 20,
        "ColorSpace" => "DeviceRGB", "BitsPerComponent" => 8 },
        samples,
    ));
    let mut kids = vec![];
    for _ in 0..2 {
        let c = file.add_object(Stream::new(
            dictionary! {},
            b"q 200 0 0 200 100 100 cm /Im0 Do Q".to_vec(),
        ));
        let p = file.add_object(dictionary! {
            "Type" => "Page", "Parent" => pages_id,
            "MediaBox" => vec![0.into(), 0.into(), 400.into(), 400.into()],
            "Contents" => c,
            "Resources" => dictionary! { "XObject" => dictionary! { "Im0" => img } },
        });
        kids.push(Object::Reference(p));
    }
    file.objects.insert(
        pages_id,
        Object::Dictionary(dictionary! { "Type" => "Pages", "Kids" => kids, "Count" => 2 }),
    );
    let catalog = file.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
    file.trailer.set("Root", catalog);
    let mut bytes = Vec::new();
    file.save_to(&mut bytes).unwrap();
    let doc = reopen(bytes);

    // The bottom-left image pixel (x 0, row 19) of each page's image.
    let corner = |doc_id: &str, page: PageIndex| {
        let doc_id = doc_id.to_string();
        with_doc(&doc_id, move |d| {
            let bindings = d.bindings();
            let p = d.page(page)?;
            let h = redact_raw::object_at(bindings, p, 0)?;
            let b = redact_raw::image_bitmap(bindings, h).expect("bitmap");
            Ok(px(&b, 0, 19))
        })
        .unwrap()
    };
    let original = corner(&doc.doc_id, 1);
    apply_opts(
        &doc.doc_id,
        0,
        vec![Rect::new(100.0, 100.0, 130.0, 130.0)],
        false,
    )
    .expect("apply");
    assert_eq!(
        corner(&doc.doc_id, 0),
        [0, 0, 0],
        "blanked on the marked page"
    );
    assert_eq!(
        corner(&doc.doc_id, 1),
        original,
        "the other page's copy is not shown as blanked in memory"
    );
    let saved = reopen(save_bytes(&doc.doc_id));
    assert_eq!(corner(&saved.doc_id, 0), [0, 0, 0]);
    assert_eq!(
        corner(&saved.doc_id, 1),
        original,
        "and not in the file either"
    );
    // Still one undo step.
    let d = doc.doc_id.clone();
    let undone = with_state(move |st| registry::undo(st, &d, false)).expect("undo");
    assert!(!undone.can_undo);
    assert_eq!(corner(&doc.doc_id, 0), original);
}
