//! P1-11 — macOS Vision OCR (`ocr_recognize_native`), end to end on the real framework.
//!
//! * **accuracy** — `fixtures/gen/korean-300dpi.pdf` rendered exactly as the command renders
//!   it (the `/ocr` image, on the engine thread), recognised by Vision on *this* thread, scored
//!   with the metric of `scripts/ocr-accuracy.mjs` against the text the generator drew. The
//!   gate is the F-21 budget tesseract already meets (Hangul CER ≤ 3 %); Vision measured 0.21 %
//!   in the spike.
//! * **geometry** — Vision boxes are image pixels with the origin top-left, like tesseract's:
//!   the line Vision reads as the title sits where PDFium's own text layer puts it.
//! * **apply** — the Vision `OcrPage` goes through `ocr_apply` unchanged, as one undo step, and
//!   the result is searchable.
//! * **spike image** — when `fixtures/out/ocr/korean-300dpi.png` (the 10-line page the tesseract
//!   accuracy gate uses, gitignored) is present, the same score on it, for a like-for-like
//!   comparison with `ocr-accuracy.json`.
//!
//! Everything is `cfg(target_os = "macos")`; elsewhere the file only checks that the command
//! half reports `unsupported`.

mod common;
use common::*;

use seepdf_lib::engine::ocr;

// ---------------------------------------------------------------------------------------
// The metric of scripts/ocr-accuracy.mjs
// ---------------------------------------------------------------------------------------

fn norm(s: &str) -> String {
    let mapped: String = s
        .chars()
        .filter(|c| *c != '\u{AD}')
        .map(|c| match c {
            '\u{2018}' | '\u{2019}' | '\u{201A}' => '\'',
            '\u{201C}' | '\u{201D}' | '\u{201E}' => '"',
            '\u{2010}'..='\u{2015}' | '\u{2212}' => '-',
            c => c,
        })
        .collect();
    mapped.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn levenshtein(a: &[char], b: &[char]) -> usize {
    if a.is_empty() {
        return b.len();
    }
    if b.is_empty() {
        return a.len();
    }
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0usize; b.len() + 1];
    for (i, ca) in a.iter().enumerate() {
        cur[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            cur[j + 1] = (prev[j + 1] + 1)
                .min(cur[j] + 1)
                .min(prev[j] + usize::from(ca != cb));
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

fn is_hangul(c: char) -> bool {
    matches!(c as u32, 0x1100..=0x11FF | 0x3130..=0x318F | 0xAC00..=0xD7AF)
}

/// `(cer, hangul_cer)` exactly as `score()` in `scripts/ocr-accuracy.mjs` computes them.
fn score(truth: &str, hyp: &str) -> (f64, f64) {
    let t: Vec<char> = norm(truth).chars().collect();
    let h: Vec<char> = norm(hyp).chars().collect();
    let cer = levenshtein(&t, &h) as f64 / t.len().max(1) as f64;
    let ht: Vec<char> = t.iter().copied().filter(|c| is_hangul(*c)).collect();
    let hh: Vec<char> = h.iter().copied().filter(|c| is_hangul(*c)).collect();
    let hangul = levenshtein(&ht, &hh) as f64 / ht.len().max(1) as f64;
    (cer, hangul)
}

#[test]
fn metric_matches_the_accuracy_script() {
    assert_eq!(score("가나다 abc", "가나다 abc"), (0.0, 0.0));
    let (cer, hangul) = score("가나다라", "가나라");
    assert!((cer - 0.25).abs() < 1e-9 && (hangul - 0.25).abs() < 1e-9);
    // Whitespace runs and dash variants do not count.
    assert_eq!(score("a  –  b", "a - b").0, 0.0);
}

// ---------------------------------------------------------------------------------------
// macOS: the real Vision framework
// ---------------------------------------------------------------------------------------

#[cfg(target_os = "macos")]
mod mac {
    use super::*;
    use seepdf_lib::engine::ocr::vision;
    use seepdf_lib::engine::registry;
    use seepdf_lib::engine::text::search;
    use seepdf_lib::ipc::types::OcrPage;
    use std::time::Instant;

    const DPI: u32 = 300;

    /// What `examples/gen_fixtures.rs` `korean_page()` draws on `gen/korean-300dpi.pdf`.
    const KOREAN_FIXTURE_TEXT: &str = "검색 가능한 한글 문서\n\
        SeePDF OCR 정확도 측정용 고정 페이지입니다.\n\
        이 문장은 300 DPI 로 렌더링하면 또렷하게 읽힙니다.\n\
        Mixed script: SeePDF 2026 version 1.0";

    /// FEATURES F-21 / `normalize.ts` `HANGUL_CER_BUDGET`.
    const HANGUL_CER_BUDGET: f64 = 0.03;

    fn page_text(page: &OcrPage) -> String {
        page.lines
            .iter()
            .map(|l| l.text.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn render(doc_id: &str, page: u16) -> ocr::GrayPage {
        let doc_id = doc_id.to_string();
        with_state(move |st| ocr::render_page_gray(st, &doc_id, page, DPI)).expect("render")
    }

    fn langs() -> Vec<String> {
        vec!["ko-KR".to_string(), "en-US".to_string()]
    }

    #[test]
    fn vision_is_advertised_on_this_mac() {
        // The build machine is macOS 26: Vision reads Korean here, so it must be listed.
        assert!(
            ocr::vision_available(),
            "Vision should read ko-KR on macOS 13+"
        );
        let caps = ocr::capabilities();
        assert!(caps
            .engines
            .contains(&seepdf_lib::ipc::types::OcrEngine::Vision));
    }

    #[test]
    fn vision_reads_the_korean_fixture() {
        let doc = open("gen/korean-300dpi.pdf");
        let t0 = Instant::now();
        let image = render(&doc.doc_id, 0);
        let render_ms = t0.elapsed().as_secs_f64() * 1000.0;
        assert_eq!(image.pixels.len(), (image.width * image.height) as usize);

        // Cold (model load) and warm runs, for the report.
        let t1 = Instant::now();
        let page = ocr::recognize_gray(&image, &langs()).expect("Vision");
        let cold_ms = t1.elapsed().as_secs_f64() * 1000.0;
        let mut warm = Vec::new();
        for _ in 0..3 {
            let t = Instant::now();
            let again = ocr::recognize_gray(&image, &langs()).expect("Vision (warm)");
            warm.push(t.elapsed().as_secs_f64() * 1000.0);
            assert_eq!(
                page_text(&again),
                page_text(&page),
                "Vision is deterministic"
            );
        }
        let warm_ms = warm.iter().sum::<f64>() / warm.len() as f64;

        let text = page_text(&page);
        let (cer, hangul) = score(KOREAN_FIXTURE_TEXT, &text);
        println!(
            "[vision] gen/korean-300dpi.pdf {}x{} px: render {render_ms:.0} ms, Vision cold \
             {cold_ms:.0} ms, warm {warm_ms:.0} ms; CER {:.2} %, Hangul CER {:.2} %\n{text}",
            image.width,
            image.height,
            cer * 100.0,
            hangul * 100.0
        );
        assert!(
            hangul <= HANGUL_CER_BUDGET,
            "Hangul CER {:.2} % over the {:.0} % budget:\n{text}",
            hangul * 100.0,
            HANGUL_CER_BUDGET * 100.0
        );
        assert!(cer <= 0.05, "CER {:.2} %:\n{text}", cer * 100.0);

        // The OcrPage is the contract's: image geometry, page rotation, sane word boxes.
        assert_eq!((page.page, page.dpi), (0, DPI));
        assert_eq!((page.width_px, page.height_px), (image.width, image.height));
        assert_eq!(page.lines.len(), 4, "{text}");
        for line in &page.lines {
            assert!(line.row_height_px.unwrap_or(0.0) > 0.0);
            for w in &line.words {
                let [x0, y0, x1, y1] = w.bbox;
                assert!(x0 < x1 && y0 < y1, "{w:?}");
                assert!(x0 >= -2.0 && y0 >= -2.0, "{w:?}");
                assert!(
                    x1 <= image.width as f32 + 2.0 && y1 <= image.height as f32 + 2.0,
                    "{w:?}"
                );
                assert!((30.0..=100.0).contains(&w.confidence), "{w:?}");
            }
            // Words are not the whole line each (per-word boxes or the split fallback).
            if line.words.len() > 1 {
                let lw = line.bbox[2] - line.bbox[0];
                assert!(
                    line.words.iter().all(|w| w.bbox[2] - w.bbox[0] < lw * 0.9),
                    "{line:?}"
                );
            }
        }
        for line in &page.lines {
            let boxes: Vec<String> = line
                .words
                .iter()
                .map(|w| format!("{}[{:.0}..{:.0}]", w.text, w.bbox[0], w.bbox[2]))
                .collect();
            println!("[vision] {}", boxes.join(" "));
        }
        let words: Vec<&str> = page.lines[0]
            .words
            .iter()
            .map(|w| w.text.as_str())
            .collect();
        assert_eq!(
            words,
            ["검색", "가능한", "한글", "문서"],
            "Hangul spacing survives"
        );
    }

    #[test]
    fn vision_boxes_land_on_the_text() {
        let doc = open("gen/korean-300dpi.pdf");
        let image = render(&doc.doc_id, 0);
        let page = ocr::recognize_gray(&image, &langs()).expect("Vision");
        let title = page
            .lines
            .iter()
            .find(|l| l.text.contains("한글"))
            .expect("the title line");

        // Where PDFium's own text layer has the same line, in the same image pixels.
        let query = search::Query::new("검색 가능한 한글 문서", false, false).expect("query");
        let hits =
            with_doc(&doc.doc_id, move |d| search::search_page(d, 0, &query)).expect("search");
        let rect = hits[0].rects[0];
        let (w, h) = (image.width, image.height);
        let truth = with_doc(&doc.doc_id, move |d| {
            ocr::points_to_image_px(d, 0, w, h, rect)
        })
        .expect("points to px");

        let line_h = truth[3] - truth[1];
        for (i, (got, want)) in title.bbox.iter().zip(truth.iter()).enumerate() {
            assert!(
                (got - want).abs() < line_h * 0.6,
                "edge {i}: Vision {:?} vs text layer {truth:?}",
                title.bbox
            );
        }
    }

    #[test]
    fn vision_page_applies_as_one_undo_step() {
        let doc = open("gen/korean-300dpi.pdf");
        let image = render(&doc.doc_id, 0);
        let page = ocr::recognize_gray(&image, &langs()).expect("Vision");
        let doc_id = doc.doc_id.clone();
        let before = with_doc(&doc_id, |d| Ok(d.generation)).expect("generation");
        let hits_before = count_hits(&doc_id, "정확도");
        // Phrases across word gaps: Vision's padded word boxes sit too close together for PDFium
        // to infer the spaces, so `ocr_apply` writes them (the layer read `SeePDF2026version`).
        let phrases = ["가능한 한글", "SeePDF 2026 version"];
        let phrases_before: Vec<usize> = phrases.iter().map(|p| count_hits(&doc_id, p)).collect();
        let id = doc_id.clone();
        let info = with_state(move |st| {
            let mut progress = |_: usize, _: u16| {};
            ocr::apply(st, &id, &[page], false, &mut progress)
        })
        .expect("ocr_apply");
        assert_eq!(info.doc_generation, before + 1, "one undo step");
        assert_eq!(
            count_hits(&doc_id, "정확도"),
            hits_before + 1,
            "the invisible Vision layer is searchable"
        );
        for (phrase, before) in phrases.iter().zip(&phrases_before) {
            assert_eq!(
                count_hits(&doc_id, phrase),
                before + 1,
                "{phrase:?} across a word gap"
            );
        }
        let id = doc_id.clone();
        with_state(move |st| registry::undo(st, &id, false).map(|_| ())).expect("undo");
        assert_eq!(
            count_hits(&doc_id, "정확도"),
            hits_before,
            "undone in one step"
        );
    }

    fn count_hits(doc_id: &str, needle: &str) -> usize {
        let query = search::Query::new(needle, false, false).expect("query");
        with_doc(doc_id, move |d| search::search_page(d, 0, &query))
            .expect("search")
            .len()
    }

    /// The tesseract accuracy gate's own image, when a spike run left it behind.
    #[test]
    fn vision_on_the_spike_korean_image() {
        let dir = fixture("out").join("ocr");
        let (png, truth) = (dir.join("korean-300dpi.png"), dir.join("korean.txt"));
        if !png.is_file() || !truth.is_file() {
            println!(
                "[vision] skipped: {} is missing (cargo run --example spike_ocr_render)",
                png.display()
            );
            return;
        }
        let gray = image::open(&png).expect("decode png").to_luma8();
        let (width, height) = gray.dimensions();
        let image = ocr::GrayPage {
            page: 0,
            dpi: DPI,
            pixels: gray.into_raw(),
            width,
            height,
            rotation: 0,
            render_ms: 0.0,
        };
        let _ = ocr::recognize_gray(&image, &langs()).expect("warm up");
        let t = Instant::now();
        let page = ocr::recognize_gray(&image, &langs()).expect("Vision");
        let ms = t.elapsed().as_secs_f64() * 1000.0;
        let text = page_text(&page);
        let (cer, hangul) = score(&std::fs::read_to_string(&truth).expect("truth"), &text);
        println!(
            "[vision] korean-300dpi.png {width}x{height}: {ms:.0} ms warm; \
             CER {:.2} %, Hangul CER {:.2} %",
            cer * 100.0,
            hangul * 100.0
        );
        assert!(
            hangul <= HANGUL_CER_BUDGET,
            "Hangul CER {:.2} %:\n{text}",
            hangul * 100.0
        );
    }

    #[test]
    fn language_codes_reach_vision() {
        // Tesseract codes from an old frontend are mapped, not rejected.
        let doc = open("gen/korean-300dpi.pdf");
        let image = render(&doc.doc_id, 0);
        let page = ocr::recognize_gray(&image, &["kor".to_string(), "eng".to_string()])
            .expect("tesseract codes map to Vision's");
        assert!(page_text(&page).contains("한글"));
        let unknown = ["xx-XX".to_string()];
        let err = vision::mac::recognize(&image.pixels, image.width, image.height, &unknown)
            .expect_err("an unknown language is refused");
        assert_eq!(err.code, seepdf_lib::ipc::ErrorCode::Unsupported);
    }
}

#[cfg(not(any(target_os = "macos", windows)))]
#[test]
fn native_ocr_is_unsupported_off_macos() {
    assert!(!ocr::vision_available());
    let doc = open("gen/korean-300dpi.pdf");
    let id = doc.doc_id.clone();
    let image = with_state(move |st| ocr::render_page_gray(st, &id, 0, 150)).expect("render");
    let err = ocr::recognize_gray(&image, &[]).expect_err("no Vision here");
    assert_eq!(err.code, seepdf_lib::ipc::ErrorCode::Unsupported);
}

/// v0.3 O3: on Windows the native path is Windows.Media.Ocr, not Vision — it reads the page with
/// an installed Korean / English recogniser (the default when no language is asked for) and
/// refuses with `unsupported` only on a machine that has neither. (The v0.3.0 CI run on main
/// failed here: this test predated O3 and expected `unsupported` on every non-macOS host.)
#[cfg(windows)]
#[test]
fn native_ocr_off_macos_is_windows_ocr_on_windows() {
    assert!(!ocr::vision_available());
    let doc = open("gen/korean-300dpi.pdf");
    let id = doc.doc_id.clone();
    let image = with_state(move |st| ocr::render_page_gray(st, &id, 0, 150)).expect("render");
    let installed = ocr::winocr::win::available_languages();
    let has_default = installed
        .iter()
        .any(|t| matches!(ocr::winocr::app_code(t), Some("kor" | "eng")));
    match ocr::recognize_gray(&image, &[]) {
        Ok(page) => {
            assert!(
                has_default,
                "read without a kor / eng recogniser: {installed:?}"
            );
            assert_eq!((page.width_px, page.height_px), (image.width, image.height));
        }
        Err(err) => {
            assert!(
                !has_default,
                "a kor / eng recogniser is installed ({installed:?}): {err:?}"
            );
            assert_eq!(err.code, seepdf_lib::ipc::ErrorCode::Unsupported);
        }
    }
}
