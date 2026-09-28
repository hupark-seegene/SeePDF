//! Stage 1 (b) — the invisible OCR text layer (`IPC_CONTRACT.md` §7.9).
//!
//! The two claims that matter, and nothing else is worth testing:
//!
//! * **searchable** — after apply + save + reopen, the Korean words extract *with spaces* and
//!   `search()` finds them, while the rendered page is pixel-identical (render mode 3 paints
//!   nothing). The space is the whole point: the AppleGothic candidate rendered Hangul
//!   perfectly and reverse-mapped its space glyph to U+0009, which turns `검색 가능한` into
//!   `검색\t가능한` and breaks every search (`WORKPLAN.md` §6).
//! * **word spaces** — every word but a line's last carries a real U+0020, so the text layer
//!   reads `quick brown` whatever the gap between the boxes: Vision's padded word boxes sit only
//!   6–13 px apart, too close for PDFium to infer the break, and read `Thequickbrown` before.
//! * **rotation** — OCR boxes arrive in *display* pixels and page objects live in unrotated
//!   user space. `fixtures/rotation.pdf` page 2 is `/Rotate 90`; a word placed from its image
//!   box must map back to that box. The spike reasoned this from the API and never measured
//!   it (ocr spike gotcha 16).

mod common;
use common::*;

use seepdf_lib::engine::ocr;
use seepdf_lib::engine::ocr::vision::{
    self, VisionContext, VisionObservation, VisionRect, VisionWord,
};
use seepdf_lib::engine::registry;
use seepdf_lib::engine::render::tiles;
use seepdf_lib::engine::text::{layer, search};
use seepdf_lib::ipc::types::{OcrLine, OcrPage, OcrWord, Rect};
use std::path::PathBuf;

const DPI: u32 = 300;

fn out_dir() -> PathBuf {
    let dir = fixture("out").join("stage1b");
    std::fs::create_dir_all(&dir).expect("create fixtures/out/stage1b");
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

fn apply(doc_id: &str, pages: Vec<OcrPage>, replace: bool) -> seepdf_lib::ipc::types::DocInfo {
    let doc_id = doc_id.to_string();
    with_state(move |st| {
        let mut progress = |_done: usize, _page: u16| {};
        ocr::apply(st, &doc_id, &pages, replace, &mut progress)
    })
    .expect("ocr_apply")
}

/// The image size the `/ocr` route produces for a page at `DPI`.
fn image_size(doc_id: &str, page: u16) -> (u32, u32) {
    let doc_id = doc_id.to_string();
    with_state(move |st| seepdf_lib::engine::export::pixel_size(st, &doc_id, page, DPI))
        .expect("pixel size")
}

fn word(text: &str, bbox: [f32; 4]) -> OcrWord {
    OcrWord {
        text: text.to_string(),
        bbox,
        confidence: 92.0,
    }
}

fn line(text: &str, bbox: [f32; 4], words: Vec<OcrWord>) -> OcrLine {
    OcrLine {
        text: text.to_string(),
        bbox,
        baseline: None,
        row_height_px: Some(bbox[3] - bbox[1]),
        words,
    }
}

/// The union of the loose boxes of the characters spelling `needle`.
///
/// Used instead of `TextLayer::lines` because the line grouping works in **unrotated** user
/// space: on a `/Rotate 90` page every character has its own baseline, so each one becomes its
/// own "line". The characters themselves are still exactly where they were drawn.
fn run_rect(layer: &seepdf_lib::engine::text::TextLayer, needle: &str) -> Rect {
    let chars: Vec<char> = layer
        .chars
        .iter()
        .map(|c| char::from_u32(c.codepoint).unwrap_or('\u{fffd}'))
        .collect();
    let target: Vec<char> = needle.chars().collect();
    let start = (0..chars.len().saturating_sub(target.len()) + 1)
        .find(|&i| chars[i..i + target.len()] == target[..])
        .unwrap_or_else(|| panic!("{needle:?} is not in the extracted characters"));
    let mut rect: Option<Rect> = None;
    for entry in &layer.chars[start..start + target.len()] {
        if entry.loose.width() <= 0.0 && entry.loose.height() <= 0.0 {
            continue;
        }
        rect = Some(match rect {
            None => entry.loose,
            Some(r) => r.union(&entry.loose),
        });
    }
    rect.expect("the run has at least one drawn character")
}

/// A blank Letter page standing in for a scan, so nothing else on the page can make the
/// assertions pass by accident: tracemonkey with a blank page inserted and its own 14 deleted.
fn blank_letter() -> TestDoc {
    let doc = open("tracemonkey.pdf");
    with_state({
        let doc_id = doc.doc_id.clone();
        move |st| {
            seepdf_lib::engine::pages::apply_ops(
                st,
                &doc_id,
                vec![
                    seepdf_lib::ipc::types::PageOp::InsertBlank {
                        at: 0,
                        size: seepdf_lib::ipc::types::BlankPageSize::Named(
                            seepdf_lib::ipc::types::NamedPageSize::Letter,
                        ),
                    },
                    seepdf_lib::ipc::types::PageOp::Delete {
                        pages: (1..15).collect(),
                    },
                ],
            )
        }
    })
    .expect("build a blank page");
    assert!(page_text(&doc.doc_id, 0).trim().is_empty());
    doc
}

fn render(doc_id: &str, page: u16, rect: Option<Rect>) -> (u32, u32, Vec<u8>) {
    let doc_id = doc_id.to_string();
    let buffer = with_state(move |st| tiles::render_raw_buffer(st, &doc_id, page, 2.0, rect))
        .expect("render");
    let width = u32::from_le_bytes(buffer[8..12].try_into().unwrap());
    let height = u32::from_le_bytes(buffer[12..16].try_into().unwrap());
    (width, height, buffer[32..].to_vec())
}

// ---------------------------------------------------------------------------------------

/// `ocr_page_status` separates an image-only page from one that already has text.
#[test]
fn ocr_layer_page_status() {
    let doc = open("tracemonkey.pdf");
    let status = with_doc(&doc.doc_id, |d| ocr::page_status(d, &[0, 1])).expect("ocr_page_status");
    assert_eq!(status.len(), 2);
    assert!(status[0].has_text);
    assert!(status[0].char_count > 1000);

    // A freshly inserted blank page is the image-only case.
    let info = with_state({
        let doc_id = doc.doc_id.clone();
        move |st| {
            seepdf_lib::engine::pages::apply_ops(
                st,
                &doc_id,
                vec![seepdf_lib::ipc::types::PageOp::InsertBlank {
                    at: 0,
                    size: seepdf_lib::ipc::types::BlankPageSize::Named(
                        seepdf_lib::ipc::types::NamedPageSize::Letter,
                    ),
                }],
            )
        }
    })
    .expect("insert blank");
    assert_eq!(info.page_count, 15);
    let status = with_doc(&doc.doc_id, |d| ocr::page_status(d, &[0])).expect("ocr_page_status");
    assert!(!status[0].has_text);
    assert_eq!(status[0].char_count, 0);

    let caps = ocr::capabilities();
    assert!(caps.languages.iter().any(|l| l == "kor"));
    assert!(caps.languages.iter().any(|l| l == "eng"));
}

/// The gate for the bundled font and for the whole feature: Hangul words become searchable
/// invisible text that paints nothing.
#[test]
fn ocr_layer_apply_searchable() {
    let doc = blank_letter();

    let (width_px, height_px) = image_size(&doc.doc_id, 0);
    assert_eq!((width_px, height_px), (2550, 3300), "letter at 300 DPI");
    let before = render(&doc.doc_id, 0, None);

    // One word containing a space (this is what catches the space→TAB bug) plus two separate
    // words on a second line.
    let ocr_page = OcrPage {
        page: 0,
        dpi: DPI,
        width_px,
        height_px,
        rotation: 0,
        lines: vec![
            line(
                "검색 가능한 한글 문서",
                [300.0, 900.0, 1150.0, 975.0],
                vec![word("검색 가능한 한글 문서", [300.0, 900.0, 1150.0, 975.0])],
            ),
            line(
                "Searchable OCR",
                [300.0, 1100.0, 1300.0, 1165.0],
                vec![
                    word("Searchable", [300.0, 1100.0, 900.0, 1165.0]),
                    word("OCR", [950.0, 1100.0, 1300.0, 1165.0]),
                ],
            ),
        ],
    };

    let generation = with_doc(&doc.doc_id, |d| Ok(d.generation)).expect("generation");
    let info = apply(&doc.doc_id, vec![ocr_page.clone()], false);
    assert_eq!(
        info.doc_generation,
        generation + 1,
        "the whole batch is one undo step"
    );

    // Render mode 3 paints nothing: the page is pixel-identical.
    let after = render(&doc.doc_id, 0, None);
    assert_eq!(after.2, before.2, "the OCR layer is invisible");

    let bytes = save_bytes(&doc.doc_id);
    std::fs::write(out_dir().join("ocr-korean.pdf"), &bytes).expect("write sample");
    let saved = reopen(bytes);

    let text = page_text(&saved.doc_id, 0);
    assert!(
        text.contains("검색 가능한 한글 문서"),
        "Hangul extracts with ASCII spaces after save + reopen: {text:?}"
    );
    assert!(
        !text.contains('\u{9}'),
        "no TAB anywhere — the AppleGothic /ToUnicode bug: {text:?}"
    );
    // The tesseract-shaped line (tight boxes, a wide gap) gets exactly one space too.
    assert!(text.contains("Searchable OCR"), "{text:?}");
    assert!(!text.contains("  "), "single spaces: {text:?}");

    for needle in ["검색 가능한", "한글 문서", "Searchable"] {
        let query = search::Query::new(needle, false, false).expect("query");
        let hits = with_doc(&saved.doc_id, move |d| search::search_page(d, 0, &query))
            .expect("search")
            .len();
        assert!(hits >= 1, "search({needle:?}) found nothing in {text:?}");
    }

    // The word landed on its box: the run is where the OCR rectangle maps to, and it was
    // scaled to fill it.
    let layer = with_doc(&saved.doc_id, |d| layer::layer(d, 0)).expect("text layer");
    let run = run_rect(&layer, "검색 가능한 한글 문서");
    let s = 72.0 / DPI as f32;
    let expected_left = 300.0 * s;
    let expected_width = (1150.0 - 300.0) * s;
    assert!(
        (run.l - expected_left).abs() < 4.0,
        "left {} vs {expected_left} (run {run:?})",
        run.l
    );
    let fill = run.width() / expected_width;
    assert!(
        (0.85..=1.10).contains(&fill),
        "the run was scaled to its box: {:.0} pt of {:.0} pt",
        run.width(),
        expected_width
    );

    // Re-running with `replaceExisting` must not double the text.
    let replaced = apply(&saved.doc_id, vec![ocr_page], true);
    assert!(replaced.doc_generation > 1);
    let text = page_text(&saved.doc_id, 0);
    assert_eq!(
        text.matches("Searchable").count(),
        1,
        "the previous layer was removed first: {text:?}"
    );
}

/// P1-7 여러 파일 OCR: a document the engine has open but no window is bound to goes through
/// the batch's exact command sequence — one `ocr_apply` per page, `save_document_as` to a new
/// `<name>-ocr.pdf`, `close_document` — and the source file is never written (no backup either).
#[test]
fn ocr_layer_batch_file_roundtrip() {
    let source = fixture("tracemonkey.pdf");
    let source_bytes = std::fs::read(&source).expect("read the source");
    let doc = open("tracemonkey.pdf");
    let doc_id = doc.doc_id.clone();
    let page_count = doc.info.page_count;

    for page in [0u16, 1] {
        let (width_px, height_px) = image_size(&doc_id, page);
        let marker = format!("SeePDFbatch{page}");
        let ocr_page = OcrPage {
            page,
            dpi: DPI,
            width_px,
            height_px,
            rotation: 0,
            lines: vec![line(
                &marker,
                [300.0, 300.0, 1100.0, 360.0],
                vec![word(&marker, [300.0, 300.0, 1100.0, 360.0])],
            )],
        };
        apply(&doc_id, vec![ocr_page], false);
    }

    let dir = out_dir().join("batch");
    std::fs::create_dir_all(&dir).expect("create the batch output dir");
    let target = dir.join("tracemonkey-ocr.pdf");
    let _ = std::fs::remove_file(&target);
    let saved = with_state({
        let doc_id = doc_id.clone();
        let target = target.display().to_string();
        move |st| seepdf_lib::engine::save::save(st, &doc_id, Some(&target), true)
    })
    .expect("save_document_as to the -ocr path");
    assert_eq!(saved.path, target.display().to_string());
    assert!(target.exists());

    // `close_document`, then the id is gone.
    with_state({
        let doc_id = doc_id.clone();
        move |st| registry::close(st, &doc_id)
    })
    .expect("close_document");
    assert!(
        with_doc(&doc_id, |_| Ok(())).is_err(),
        "the batch document is closed"
    );

    assert_eq!(
        std::fs::read(&source).expect("reread the source"),
        source_bytes,
        "the source file is untouched"
    );

    let output = reopen(std::fs::read(&target).expect("read the output"));
    assert_eq!(output.info.page_count, page_count);
    assert!(page_text(&output.doc_id, 0).contains("SeePDFbatch0"));
    assert!(page_text(&output.doc_id, 1).contains("SeePDFbatch1"));
}

/// Word boxes laid out left to right on one row, `char_px` a character and `gap_px` apart —
/// the regular spacing Vision's `boundingBoxForRange` reports for a printed line.
fn laid_out<'a>(
    words: &[&'a str],
    left: f32,
    top: f32,
    bottom: f32,
    char_px: f32,
    gap_px: f32,
) -> Vec<(&'a str, [f32; 4])> {
    let mut x = left;
    words
        .iter()
        .map(|w| {
            let right = x + w.chars().count() as f32 * char_px;
            let b = [x, top, right, bottom];
            x = right + gap_px;
            (*w, b)
        })
        .collect()
}

/// Image pixels (origin top-left) → Vision's normalised rect (origin bottom-left).
fn to_vision(b: [f32; 4], width_px: u32, height_px: u32) -> VisionRect {
    let (w, h) = (width_px as f64, height_px as f64);
    VisionRect {
        x: b[0] as f64 / w,
        y: 1.0 - b[3] as f64 / h,
        width: (b[2] - b[0]) as f64 / w,
        height: (b[3] - b[1]) as f64 / h,
    }
}

/// One `VNRecognizedTextObservation`: the line text, the union box and per-word boxes.
fn observation(words: &[(&str, [f32; 4])], width_px: u32, height_px: u32) -> VisionObservation {
    let mut line = words[0].1;
    for (_, b) in words {
        line = [
            line[0].min(b[0]),
            line[1].min(b[1]),
            line[2].max(b[2]),
            line[3].max(b[3]),
        ];
    }
    VisionObservation {
        text: words.iter().map(|(t, _)| *t).collect::<Vec<_>>().join(" "),
        confidence: 1.0,
        bbox: to_vision(line, width_px, height_px),
        words: words
            .iter()
            .map(|(t, b)| VisionWord {
                text: t.to_string(),
                confidence: 1.0,
                bbox: to_vision(*b, width_px, height_px),
            })
            .collect(),
    }
}

fn hits(doc_id: &str, needle: &str) -> Vec<seepdf_lib::ipc::types::SearchHit> {
    let query = search::Query::new(needle, false, false).expect("query");
    with_doc(doc_id, move |d| search::search_page(d, 0, &query)).expect("search")
}

/// Vision's word boxes are padded: on `gen/korean-300dpi.pdf` they sit only 7–13 px apart at
/// 300 DPI (tesseract's ink boxes: ~24 px), too close for PDFium to infer a word break, and the
/// layer read `검색가능한한글문서` / `Mixedscript:SeePDF2026version1.0`. The first two lines
/// below are Vision's own boxes for that page (P1-11 probe, macOS 26); the third is laid out
/// with the fourth line's measured spacing (31.4 px a character, 8.25 px gaps).
#[test]
fn ocr_layer_vision_words_extract_with_spaces() {
    let doc = blank_letter();
    let (width_px, height_px) = image_size(&doc.doc_id, 0);

    let title = [
        ("검색", [269.9, 423.0, 448.5, 525.18]),
        ("가능한", [461.25, 423.0, 741.75, 525.0]),
        ("한글", [754.5, 423.0, 958.5, 525.0]),
        ("문서", [971.25, 423.0, 1155.98, 525.18]),
    ];
    // Word bottoms drift down the line, as Vision reported them.
    let mixed = [
        ("SeePDF", [264.45, 674.91, 478.4, 738.21]),
        ("OCR", [485.58, 676.52, 608.02, 738.41]),
        ("정확도", [615.2, 677.47, 783.39, 739.7]),
        ("측정용", [790.57, 678.75, 958.76, 740.98]),
        ("고정", [965.94, 680.03, 1073.13, 741.81]),
        ("페이지입니다.", [1080.3, 680.87, 1431.37, 745.17]),
    ];
    let fox = laid_out(
        &[
            "The", "quick", "brown", "fox", "jumps", "over", "the", "lazy", "dog",
        ],
        264.8,
        938.0,
        1004.47,
        31.4,
        8.25,
    );
    let ocr_page = vision::normalize(
        &[
            observation(&fox, width_px, height_px),
            observation(&title, width_px, height_px),
            observation(&mixed, width_px, height_px),
        ],
        &VisionContext {
            page: 0,
            dpi: DPI,
            width_px,
            height_px,
            rotation: 0,
            min_confidence: vision::MIN_CONFIDENCE,
        },
    );
    assert_eq!(ocr_page.lines.len(), 3);
    assert_eq!(
        ocr_page.lines[0].words.len(),
        4,
        "Vision's per-word boxes survive"
    );

    apply(&doc.doc_id, vec![ocr_page.clone()], false);
    let bytes = save_bytes(&doc.doc_id);
    std::fs::write(out_dir().join("ocr-vision-spaces.pdf"), &bytes).expect("write sample");
    let saved = reopen(bytes);

    // Single spaces between words, a line break between lines.
    let text = page_text(&saved.doc_id, 0);
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    assert_eq!(
        lines,
        [
            "검색 가능한 한글 문서",
            "SeePDF OCR 정확도 측정용 고정 페이지입니다.",
            "The quick brown fox jumps over the lazy dog",
        ],
        "{text:?}"
    );
    assert!(!text.contains("  ") && !text.contains('\t'), "{text:?}");

    // Searchable across a Latin gap, a Hangul gap and the Latin → Hangul font change.
    for needle in ["quick brown", "가능한 한글", "OCR 정확도", "lazy dog"] {
        assert_eq!(
            hits(&saved.doc_id, needle).len(),
            1,
            "search({needle:?}) in {text:?}"
        );
    }

    // The space is a real glyph, not PDFium's guess, and each word still sits on its own box.
    let layer = with_doc(&saved.doc_id, |d| layer::layer(d, 0)).expect("text layer");
    let chars: Vec<char> = layer
        .chars
        .iter()
        .map(|c| char::from_u32(c.codepoint).unwrap_or('\u{fffd}'))
        .collect();
    let at = chars
        .windows(6)
        .position(|w| w.iter().collect::<String>() == "quick ")
        .expect("`quick ` is in the layer");
    let space = &layer.chars[at + 5];
    assert!(!space.is_generated(), "the space was written, not inferred");

    let s = 72.0 / DPI as f32;
    for (word, bbox) in [fox[1], fox[2], mixed[2], title[1]] {
        let run = run_rect(&layer, word);
        let (left, width) = (bbox[0] * s, (bbox[2] - bbox[0]) * s);
        assert!(
            (run.l - left).abs() < 1.0,
            "{word}: left {} vs {left} ({run:?})",
            run.l
        );
        let fill = run.width() / width;
        assert!(
            (0.9..=1.1).contains(&fill),
            "{word}: {:.1} pt of {width:.1} pt",
            run.width()
        );
        // The selection rect sits on the scanned line: the baseline is the box bottom.
        let bottom = (height_px as f32 - bbox[3]) * s;
        assert!(
            run.b <= bottom + 1.0 && run.t >= bottom,
            "{word}: {run:?} vs bottom {bottom}"
        );
    }
    // A phrase selection is one rect from the first word's box to the last one's.
    let phrase = hits(&saved.doc_id, "quick brown");
    assert_eq!(phrase[0].rects.len(), 1, "{:?}", phrase[0].rects);
    let rect = phrase[0].rects[0];
    assert!((rect.l - fox[1].1[0] * s).abs() < 1.0, "{rect:?}");
    assert!(
        (rect.r - fox[2].1[2] * s).abs() < 0.1 * (fox[2].1[2] - fox[2].1[0]) * s,
        "{rect:?}"
    );

    // `replaceExisting` takes the spaces away with their words.
    apply(&saved.doc_id, vec![ocr_page], true);
    assert_eq!(hits(&saved.doc_id, "quick brown").len(), 1);
    assert_eq!(page_text(&saved.doc_id, 0).matches("lazy dog").count(), 1);
}

/// `/Rotate 90`: the OCR boxes are display pixels, the text objects are unrotated user space,
/// and `FPDF_DeviceToPage` is the only thing that knows the difference.
#[test]
fn ocr_layer_rotation_roundtrip() {
    let doc = open("rotation.pdf");
    let rotated = doc
        .info
        .pages
        .iter()
        .position(|p| p.rotation == 90)
        .expect("rotation.pdf has a /Rotate 90 page") as u16;
    let geom = &doc.info.pages[rotated as usize];
    assert!(
        geom.width_pt > geom.height_pt,
        "the display size is landscape: {}x{}",
        geom.width_pt,
        geom.height_pt
    );

    let (width_px, height_px) = image_size(&doc.doc_id, rotated);
    assert!(
        width_px > height_px,
        "so is the image: {width_px}x{height_px}"
    );

    // A word box a third of the way down the image, running left to right on screen.
    let bbox = [400.0f32, 600.0, 1600.0, 680.0];
    let ocr_page = OcrPage {
        page: rotated,
        dpi: DPI,
        width_px,
        height_px,
        rotation: 90,
        lines: vec![line(
            "회전된 페이지 rotated",
            bbox,
            vec![word("회전된 페이지 rotated", bbox)],
        )],
    };
    apply(&doc.doc_id, vec![ocr_page], false);

    let bytes = save_bytes(&doc.doc_id);
    std::fs::write(out_dir().join("ocr-rotated.pdf"), &bytes).expect("write sample");
    let saved = reopen(bytes);

    let text = page_text(&saved.doc_id, rotated);
    assert!(
        text.contains("회전된 페이지 rotated"),
        "the rotated page's OCR text extracts: {text:?}"
    );
    let query = search::Query::new("회전된 페이지", false, false).expect("query");
    let hits = with_doc(&saved.doc_id, move |d| {
        search::search_page(d, rotated, &query)
    })
    .expect("search")
    .len();
    assert!(hits >= 1, "and is searchable");

    // The round trip: map the extracted characters' user-space rectangle back into image
    // pixels and compare with the box the words came from.
    let layer = with_doc(&saved.doc_id, move |d| layer::layer(d, rotated)).expect("text layer");
    let run = run_rect(&layer, "회전된 페이지 rotated");
    let back = with_state({
        let doc_id = saved.doc_id.clone();
        let rect = run;
        move |st| {
            let doc = st.doc_mut(&doc_id)?;
            seepdf_lib::engine::ocr::points_to_image_px(doc, rotated, width_px, height_px, rect)
        }
    })
    .expect("points_to_image_px");

    let tolerance = 24.0;
    assert!(
        (back[0] - bbox[0]).abs() < tolerance,
        "x0 {} vs {} (box {back:?})",
        back[0],
        bbox[0]
    );
    assert!(
        (back[2] - bbox[2]).abs() < tolerance * 2.0,
        "x1 {} vs {} (box {back:?})",
        back[2],
        bbox[2]
    );
    assert!(
        (back[3] - bbox[3]).abs() < tolerance,
        "y1 {} vs {} (box {back:?})",
        back[3],
        bbox[3]
    );

    // And it is still invisible on a rotated page.
    let plain = open("rotation.pdf");
    let before = render(&plain.doc_id, rotated, None);
    let after = render(&saved.doc_id, rotated, None);
    assert_eq!(before.2, after.2, "nothing was painted");
}

/// Bad input is refused before any page is touched.
#[test]
fn ocr_layer_rejects_bad_input() {
    let doc = open("tracemonkey.pdf");
    let generation = with_doc(&doc.doc_id, |d| Ok(d.generation)).expect("generation");

    let out_of_range = with_state({
        let doc_id = doc.doc_id.clone();
        move |st| {
            let mut progress = |_: usize, _: u16| {};
            ocr::apply(
                st,
                &doc_id,
                &[OcrPage {
                    page: 99,
                    dpi: DPI,
                    width_px: 100,
                    height_px: 100,
                    rotation: 0,
                    lines: vec![],
                }],
                false,
                &mut progress,
            )
        }
    });
    assert!(out_of_range.is_err());

    let no_geometry = with_state({
        let doc_id = doc.doc_id.clone();
        move |st| {
            let mut progress = |_: usize, _: u16| {};
            ocr::apply(
                st,
                &doc_id,
                &[OcrPage {
                    page: 0,
                    dpi: 0,
                    width_px: 0,
                    height_px: 0,
                    rotation: 0,
                    lines: vec![],
                }],
                false,
                &mut progress,
            )
        }
    });
    assert!(no_geometry.is_err());

    let after = with_doc(&doc.doc_id, |d| Ok(d.generation)).expect("generation");
    assert_eq!(after, generation, "a refused apply changes nothing");
}
