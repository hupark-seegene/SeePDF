//! v0.3 pkg1 verification round 2 — regression tests for defects found by the verifier.
//!
//! 1. A word split in a run drawn with non-zero `Tc` / `Tw` (justified text) passes the
//!    preview's trial but fails after regeneration, so the whole run goes although the preview
//!    promised `split` with no collateral.
//! 2. `fixtures/160F-2019.pdf` has a `/Contents` array whose streams are cut in the middle of
//!    text objects. Redacting one word regenerates one of those streams and silently drops
//!    unrelated text (39 lines for "rémunérations"), while the apply reports success and no
//!    collateral. (Also on v0.2 / 6ea8c21 — whole-run removal regenerates the same stream.)
//!
//! Fixed by `engine::redact::survivors` (runs the regeneration moved are put back; a page-wide
//! post-condition), `contents` (split streams are joined first), `spacing` (`Tc` / `Tw`
//! written back) and `preview_checked` (the preview is a dry run of the apply).

mod common;
use common::*;

use seepdf_lib::engine::redact;
use seepdf_lib::engine::registry;
use seepdf_lib::engine::text;
use seepdf_lib::ipc::types::{PageIndex, Rect, RedactBatchMark, RedactOptions};

type Boxes = Vec<(char, Rect)>;

fn reopen(bytes: Vec<u8>) -> TestDoc {
    let info = with_state(move |st| registry::open(st, None, bytes, None)).expect("reopen");
    let doc_id = info.doc_id.clone();
    TestDoc { info, doc_id }
}

fn page_text(doc_id: &str, page: PageIndex) -> String {
    let doc_id = doc_id.to_string();
    with_doc(&doc_id, move |doc| {
        Ok(text::layer::page_text(doc, page)?.text.clone())
    })
    .expect("page text")
}

fn save_and_reopen(doc_id: &str) -> TestDoc {
    let doc_id = doc_id.to_string();
    let bytes =
        with_state(move |st| seepdf_lib::engine::save::serialize(st, &doc_id)).expect("serialize");
    reopen(bytes)
}

/// The marks 검색해서 표시 makes for the first word equal to `word`: its line rects (loose
/// boxes), 0.25 pt narrower on each side (`redact.ts` `inset`).
fn word_marks(doc_id: &str, page: PageIndex, word: &str) -> Vec<Rect> {
    let doc_id = doc_id.to_string();
    let word = word.to_string();
    with_doc(&doc_id.clone(), move |d| {
        let layer = text::layer::layer(d, page)?;
        let spelled = |s: u32, n: u32| -> String {
            layer.chars[s as usize..(s + n) as usize]
                .iter()
                .map(|c| char::from_u32(c.codepoint).unwrap_or('?'))
                .collect()
        };
        let w = layer
            .words
            .iter()
            .find(|w| spelled(w.first_char, w.char_count) == word)
            .unwrap_or_else(|| panic!("{word:?} is not a word on page {page}"));
        Ok(layer
            .range_rects(w.first_char, w.char_count)
            .into_iter()
            .map(|r| {
                let dx = 0.25f32.min((r.r - r.l) * 0.1);
                Rect::new(r.l + dx, r.b, r.r - dx, r.t)
            })
            .collect())
    })
    .expect("word marks")
}

fn preview(
    doc_id: &str,
    page: PageIndex,
    rects: Vec<Rect>,
) -> seepdf_lib::ipc::types::RedactPreview {
    let doc_id = doc_id.to_string();
    with_doc(&doc_id.clone(), move |d| redact::preview(d, page, &rects)).expect("preview")
}

/// `redact_preview` as the command runs it: the static plan plus the dry run of the apply.
fn preview_checked(
    doc_id: &str,
    page: PageIndex,
    rects: Vec<Rect>,
) -> seepdf_lib::ipc::types::RedactPreview {
    let doc_id = doc_id.to_string();
    with_state(move |st| redact::preview_checked(st, &doc_id, page, &rects))
        .expect("preview_checked")
}

fn apply(
    doc_id: &str,
    page: PageIndex,
    rects: Vec<Rect>,
) -> seepdf_lib::ipc::types::RedactBatchResult {
    let doc_id = doc_id.to_string();
    with_state(move |st| {
        redact::apply_batch(
            st,
            &doc_id,
            &[RedactBatchMark { page, rects }],
            &RedactOptions {
                fill: [0, 0, 0],
                overlay_text: None,
                ungroup: false,
            },
        )
    })
    .expect("apply")
}

fn generation(doc_id: &str) -> u32 {
    let doc_id = doc_id.to_string();
    with_state(move |st| Ok(st.doc(&doc_id)?.generation)).expect("generation")
}

/// Every visible character of `page` with its tight box.
fn boxes(doc_id: &str, page: PageIndex) -> Boxes {
    let doc_id = doc_id.to_string();
    with_doc(&doc_id, move |d| {
        Ok(text::layer::layer(d, page)?
            .chars
            .iter()
            .filter(|c| !c.is_generated())
            .map(|c| (char::from_u32(c.codepoint).unwrap_or('?'), c.tight))
            .filter(|(c, _)| !c.is_whitespace())
            .collect())
    })
    .expect("boxes")
}

fn close(a: &Rect, b: &Rect) -> bool {
    (a.l - b.l).abs() <= 0.5
        && (a.b - b.b).abs() <= 0.5
        && (a.r - b.r).abs() <= 0.5
        && (a.t - b.t).abs() <= 0.5
}

/// The characters of `want` that `got` no longer has at the same box (±0.5 pt).
fn moved(want: &Boxes, got: &Boxes) -> Vec<(char, Rect)> {
    want.iter()
        .filter(|(c, r)| !got.iter().any(|(g, b)| g == c && close(r, b)))
        .copied()
        .collect()
}

/// The unmarked lines of `before` missing from `after`.
fn lost_lines<'a>(before: &'a str, after: &str, words: &[&str]) -> Vec<&'a str> {
    before
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !words.iter().any(|w| l.contains(w)))
        .filter(|l| !after.lines().any(|a| a.trim() == *l))
        .collect()
}

/// A one-page Helvetica document with `content`.
fn helvetica(content: &str) -> TestDoc {
    use lopdf::{dictionary, Object, Stream};
    let mut doc = lopdf::Document::with_version("1.7");
    let pages_id = doc.new_object_id();
    let font = doc.add_object(dictionary! {
        "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica",
        "Encoding" => "WinAnsiEncoding",
    });
    let contents = doc.add_object(Stream::new(dictionary! {}, content.as_bytes().to_vec()));
    let page = doc.add_object(dictionary! {
        "Type" => "Page", "Parent" => pages_id,
        "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
        "Contents" => contents,
        "Resources" => dictionary! { "Font" => dictionary! { "F1" => font } },
    });
    doc.objects.insert(
        pages_id,
        Object::Dictionary(
            dictionary! { "Type" => "Pages", "Kids" => vec![page.into()], "Count" => 1 },
        ),
    );
    let catalog = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
    doc.trailer.set("Root", catalog);
    let mut out = Vec::new();
    doc.save_to(&mut out).expect("save");
    reopen(out)
}

/// Case 1: Justified text: the preview promises a split with no collateral; the apply must keep
/// "Andreas" and "wrote this" (it removes the whole run instead).
#[test]
fn split_survives_char_and_word_spacing() {
    for (label, state) in [
        ("Tc", "0.3 Tc"),
        ("Tw", "1.2 Tw"),
        ("Tc+Tw", "2 Tc 5 Tw"),
        ("negative Tc", "-0.2 Tc"),
    ] {
        let doc = helvetica(&format!(
            "BT /F1 10 Tf {state} 40 600 Td (Andreas Gal wrote this) Tj ET"
        ));
        let keep: Boxes = boxes(&doc.doc_id, 0)
            .into_iter()
            .filter(|(c, _)| !"Gal".contains(*c))
            .collect();
        let marks = word_marks(&doc.doc_id, 0, "Gal");
        let checked = preview_checked(&doc.doc_id, 0, marks.clone());
        assert!(
            checked.text_objects.iter().all(|o| o.split) && checked.collateral.is_empty(),
            "{label}: the dry run agrees: {checked:?}"
        );
        let plan = preview(&doc.doc_id, 0, marks.clone());
        assert!(
            plan.text_objects.iter().all(|o| o.split),
            "{label}: {plan:?}"
        );
        assert!(plan.collateral.is_empty(), "{label}: {:?}", plan.collateral);
        let result = apply(&doc.doc_id, 0, marks);
        let saved = save_and_reopen(&doc.doc_id);
        let text = page_text(&saved.doc_id, 0);
        assert!(
            result.collateral.is_empty() && text.contains("Andreas") && text.contains("wrote this"),
            "{label}: the preview promised a split, the apply lost {:?}; saved text {text:?}",
            result.collateral
        );
        let lost = moved(&keep, &boxes(&saved.doc_id, 0));
        assert!(lost.is_empty(), "{label}: glyphs moved: {lost:?}");
    }
}

/// Case 1b: Justified and letter-spaced text the marks do not touch is regenerated with the page
/// (PDFium writes no `Tc` / `Tw`): it stays where it was and still extracts as it did. The
/// heading inherits `Tw` from the first run (text state outlives `ET`).
#[test]
fn untouched_justified_text_keeps_its_place_and_its_words() {
    let doc = helvetica(
        "BT /F1 10 Tf 0.3 Tc 1.5 Tw 40 700 Td (Keep me in place please) Tj ET \
         BT /F1 14 Tf 3 Tc 40 650 Td (HEADING) Tj ET \
         BT /F1 10 Tf 0 Tc 0 Tw 40 600 Td (Andreas Gal wrote this) Tj ET",
    );
    let before = page_text(&doc.doc_id, 0);
    let keep: Boxes = boxes(&doc.doc_id, 0)
        .into_iter()
        .filter(|(_, r)| r.b > 640.0)
        .collect();
    let result = apply(&doc.doc_id, 0, word_marks(&doc.doc_id, 0, "Gal"));
    assert!(result.collateral.is_empty(), "{:?}", result.collateral);
    let saved = save_and_reopen(&doc.doc_id);
    let lost = moved(&keep, &boxes(&saved.doc_id, 0));
    assert!(lost.is_empty(), "untouched glyphs moved: {lost:?}");
    let text = page_text(&saved.doc_id, 0);
    for line in ["Keep me in place please", "HEADING"] {
        assert!(before.contains(line));
        assert!(text.contains(line), "{line:?} extracts as before: {text:?}");
    }
}

/// Case 2: Redacting one word must not drop text outside the marks that the preview did not name.
#[test]
fn redact_one_word_keeps_the_rest_of_a_split_content_stream_page() {
    for word in ["rémunérations", "bonifié"] {
        let doc = open("160F-2019.pdf");
        let before = page_text(&doc.doc_id, 0);
        let marks = word_marks(&doc.doc_id, 0, word);
        let plan = preview(&doc.doc_id, 0, marks.clone());
        let result = apply(&doc.doc_id, 0, marks);
        assert!(plan.collateral.is_empty() && result.collateral.is_empty());
        let saved = save_and_reopen(&doc.doc_id);
        let after = page_text(&saved.doc_id, 0);
        let lost = lost_lines(&before, &after, &[word]);
        assert!(
            lost.is_empty(),
            "{word:?}: {} unmarked line(s) vanished, e.g. {:?}",
            lost.len(),
            &lost[..lost.len().min(3)]
        );
    }
}

/// Case 2b: The joined page: the file keeps none of the old streams (they hold the redacted word),
/// and undo brings back the document exactly as it was — its own streams included.
#[test]
fn joined_streams_leave_nothing_behind_and_undo_restores_them() {
    let original = std::fs::read(fixture("160F-2019.pdf")).expect("fixture");
    let old_streams: Vec<Vec<u8>> = {
        let file = lopdf::Document::load_mem(&original).expect("lopdf");
        let page = *file.get_pages().get(&1).expect("page 1");
        file.get_page_contents(page)
            .into_iter()
            .filter_map(|id| file.get_object(id).and_then(lopdf::Object::as_stream).ok())
            .map(|s| {
                s.decompressed_content()
                    .unwrap_or_else(|_| s.content.clone())
            })
            .collect()
    };
    assert!(old_streams.len() > 1, "the fixture splits its page content");

    let doc = open("160F-2019.pdf");
    let before = page_text(&doc.doc_id, 0);
    apply(&doc.doc_id, 0, word_marks(&doc.doc_id, 0, "rémunérations"));
    let doc_id = doc.doc_id.clone();
    let bytes =
        with_state(move |st| seepdf_lib::engine::save::serialize(st, &doc_id)).expect("save");
    let file = lopdf::Document::load_mem(&bytes).expect("lopdf");
    let streams: Vec<Vec<u8>> = file
        .objects
        .values()
        .filter_map(|o| o.as_stream().ok())
        .map(|s| {
            s.decompressed_content()
                .unwrap_or_else(|_| s.content.clone())
        })
        .collect();
    for old in &old_streams {
        assert!(
            !streams.contains(old),
            "an old content stream is still in the file"
        );
    }

    let doc_id = doc.doc_id.clone();
    let info = with_state(move |st| registry::undo(st, &doc_id, false)).expect("undo");
    assert_eq!(info.undo_label, None, "one step");
    assert_eq!(page_text(&doc.doc_id, 0), before);
    let doc_id = doc.doc_id.clone();
    let undone = with_state(move |st| {
        let d = st.doc(&doc_id)?;
        Ok(d.bytes.to_vec())
    })
    .expect("bytes");
    let file = lopdf::Document::load_mem(&undone).expect("lopdf");
    let page = *file.get_pages().get(&1).expect("page 1");
    assert_eq!(
        file.get_page_contents(page).len(),
        old_streams.len(),
        "undo brings the page's own streams back"
    );
}

/// Case 3: The preview is a dry run of the apply: what it names is what the apply does, on the
/// cases the verifier found (TAMReview p.0 — a font that cannot re-encode a space; 160F — split
/// streams), and it changes nothing.
#[test]
fn the_preview_names_what_the_apply_does() {
    for (file, words) in [
        ("TAMReview.pdf", &["Working", "Sprouts"][..]),
        (
            "160F-2019.pdf",
            &["national", "année", "page", "rue", "rémunérations"][..],
        ),
    ] {
        let doc = open(file);
        let before = page_text(&doc.doc_id, 0);
        let marks: Vec<Rect> = words
            .iter()
            .flat_map(|w| word_marks(&doc.doc_id, 0, w))
            .collect();
        let g = generation(&doc.doc_id);
        let checked = preview_checked(&doc.doc_id, 0, marks.clone());
        assert_eq!(
            generation(&doc.doc_id),
            g,
            "{file}: the dry run changed nothing"
        );
        assert_eq!(page_text(&doc.doc_id, 0), before, "{file}");
        let result = apply(&doc.doc_id, 0, marks);
        assert_eq!(
            checked.collateral, result.collateral,
            "{file}: preview and apply agree"
        );
        assert!(
            result.collateral.is_empty(),
            "{file}: {:?}",
            result.collateral
        );
        let saved = save_and_reopen(&doc.doc_id);
        let lost = lost_lines(&before, &page_text(&saved.doc_id, 0), words);
        assert!(lost.is_empty(), "{file}: {lost:?}");
    }
}
