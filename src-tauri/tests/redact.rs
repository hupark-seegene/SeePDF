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
use seepdf_lib::ipc::types::{PageIndex, RedactOptions, Rect};
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

fn preview(doc_id: &str, page: PageIndex, rects: Vec<Rect>) -> seepdf_lib::ipc::types::RedactPreview {
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

/// The preview names every object that will go, and says so *before* anything is removed —
/// including the collateral, because PDFium cannot split a text object.
#[test]
fn redact_preview_reports_collateral() {
    let doc = open("tracemonkey.pdf");
    let rect = word_rect(&doc.doc_id, 0, TARGET);
    let plan = preview(&doc.doc_id, 0, vec![rect]);

    assert_eq!(plan.page, 0);
    assert!(
        !plan.text_objects.is_empty(),
        "the mark covers at least one text object"
    );
    assert!(
        plan.text_objects.iter().any(|o| o.text.contains(TARGET)),
        "the object holding {TARGET:?} is listed: {:?}",
        plan.text_objects.iter().map(|o| &o.text).collect::<Vec<_>>()
    );
    assert!(
        plan.collateral.iter().any(|c| !c.is_empty()),
        "removing one word takes its whole run: the user is told which"
    );
    assert!(
        plan.form_fields.is_empty(),
        "tracemonkey.pdf has no form fields"
    );

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
    let r = apply_batch(
        &doc.doc_id,
        vec![(1, vec![r1]), (0, vec![r0]), (1, vec![])],
    )
    .expect("batch");
    assert!(r.verified);
    assert_eq!(r.pages, vec![0, 1]);
    assert!(r.removed_objects >= 2, "{}", r.removed_objects);
    assert_eq!(r.doc_generation, before_generation + 1, "one generation for both pages");
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
    assert_eq!(page_text(&doc.doc_id, 0), text0, "page 0 was rolled back too");
    assert_eq!(page_text(&doc.doc_id, 1), text1);
    let can_undo = with_doc(&doc.doc_id, |d| Ok(d.info().can_undo)).unwrap();
    assert!(!can_undo, "no undo step was left behind");

    let form = open("160F-2019.pdf");
    let widget = {
        let doc_id = form.doc_id.clone();
        with_doc(&doc_id.clone(), move |doc| {
            let fields = seepdf_lib::engine::form::list(doc, Some(0))?;
            Ok(fields.iter().map(|f| f.rect).find(|r| r.width() > 40.0).expect("a widget"))
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
