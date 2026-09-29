//! v0.3 pkg7-ocr — the OCR engine work of the v0.3 backlog, against real PDFium:
//!
//! * **O5 glyphless font** — the invisible layer written in the glyphless CID font extracts
//!   exactly what the old Hangul-subset layer extracted, paints nothing, and grows the file by
//!   at most a tenth of what the subset cost;
//! * **O2 auto-rotate** — `ocr_apply`'s `setRotation` turns the page in the same undo step as
//!   the layer; on macOS the whole path runs on a sideways scan with Apple Vision (detect at
//!   100 DPI → recognise turned → apply) and ends with `/Rotate 90` and searchable Korean;
//! * **O1 languages** — `ocr_capabilities` lists languages per engine; Vision reads `ja-JP`;
//! * **O3 Windows OCR** — on a Windows runner with the `en-US` recogniser, an English fixture.
//!
//! "Scans" are made here, not committed: a fixture page rendered to gray pixels (optionally
//! turned), written to a PNG and placed as the only object of a page of the image's size.

mod common;
use common::*;

use seepdf_lib::engine::ocr::{self, orientation, GrayPage};
use seepdf_lib::engine::registry;
use seepdf_lib::engine::render::tiles;
use seepdf_lib::engine::text::{layer, search};
use seepdf_lib::ipc::types::{
    BlankPageSize, OcrEngine, OcrLine, OcrPage, OcrWord, PageOp, PageSizePt, Rect,
};
use std::path::PathBuf;

const DPI: u32 = 300;

fn out_dir() -> PathBuf {
    let dir = fixture("out").join("v03-ocr");
    std::fs::create_dir_all(&dir).expect("create fixtures/out/v03-ocr");
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

fn render(doc_id: &str, page: u16) -> Vec<u8> {
    let doc_id = doc_id.to_string();
    with_state(move |st| tiles::render_raw_buffer(st, &doc_id, page, 1.0, None)).expect("render")
}

fn rotation_of(doc_id: &str, page: u16) -> u16 {
    with_doc(doc_id, move |d| Ok(d.geom(page)?.rotation)).expect("geometry")
}

fn search_hits(doc_id: &str, page: u16, needle: &str) -> usize {
    let query = search::Query::new(needle, false, false).expect("query");
    with_doc(doc_id, move |d| search::search_page(d, page, &query))
        .expect("search")
        .len()
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

// ---------------------------------------------------------------------------------------
// O5 — the glyphless font
// ---------------------------------------------------------------------------------------

/// The pre-v0.3 layer, reproduced for the comparison: every non-Latin word in the bundled
/// Hangul subset (Latin-1 words were Helvetica then and are now), placed the same way —
/// `pixels_to_points` of the box's bottom corners, fitted to the box width, word spaces.
fn hangul_subset_layer(doc_id: &str, ocr_page: OcrPage) {
    use pdfium_render::prelude::*;
    let doc_id = doc_id.to_string();
    with_state(move |st| {
        registry::mutate(
            st,
            &doc_id,
            registry::MutateOpts::new("undo.ocrApply", seepdf_lib::ipc::types::ChangeReason::Ocr)
                .page(0),
            |doc| {
                let hangul = doc.hangul_token()?;
                let helvetica = doc.pdf_mut().fonts_mut().helvetica();
                let mut scratch = seepdf_lib::engine::annot::ScratchPage::open(doc, 0)?;
                let document = doc.pdf();
                let config = PdfRenderConfig::new()
                    .set_target_size(ocr_page.width_px as i32, ocr_page.height_px as i32);
                for l in &ocr_page.lines {
                    let size = l.row_height_px.unwrap() * 72.0 / ocr_page.dpi as f32;
                    let last = l.words.len() - 1;
                    for (i, w) in l.words.iter().enumerate() {
                        let [x0, _, x1, y1] = w.bbox;
                        let (bx, by) = scratch
                            .page
                            .pixels_to_points(x0 as i32, y1 as i32, &config)
                            .unwrap();
                        let (rx, _) = scratch
                            .page
                            .pixels_to_points(x1 as i32, y1 as i32, &config)
                            .unwrap();
                        let font = if w.text.chars().all(|c| (c as u32) < 0x100) {
                            helvetica
                        } else {
                            hangul
                        };
                        let mut object =
                            PdfPageTextObject::new(document, &w.text, font, PdfPoints::new(size))
                                .unwrap();
                        object
                            .set_render_mode(PdfPageTextRenderMode::Invisible)
                            .unwrap();
                        let natural = object.bounds().unwrap().to_rect().width().value;
                        if i < last {
                            object.set_text(format!("{} ", w.text)).unwrap();
                        }
                        object
                            .scale(((rx.value - bx.value) / natural).clamp(0.5, 2.0), 1.0)
                            .unwrap();
                        object.translate(bx, by).unwrap();
                        scratch.page.objects_mut().add_text_object(object).unwrap();
                    }
                }
                scratch.page.regenerate_content().unwrap();
                Ok(())
            },
        )
    })
    .expect("the Hangul-subset layer");
}

#[test]
fn ocr_glyphless_layer_is_small_and_extracts_like_the_hangul_font() {
    // The fixture scan, twice: one gets today's layer, the other the pre-v0.3 one.
    let scan = scan_of("gen/korean-300dpi.pdf", 150, 0);
    let base = save_bytes(&scan.doc_id);
    let old = reopen(base.clone());
    let new = reopen(base.clone());
    let image = gray(&new.doc_id, 0, DPI);
    let ocr_page = korean_ocr(0, image.width, image.height);

    let before = render(&new.doc_id, 0);
    apply(&new.doc_id, vec![ocr_page.clone()], vec![None]);
    assert_eq!(
        render(&new.doc_id, 0),
        before,
        "the glyphless layer paints nothing"
    );
    hangul_subset_layer(&old.doc_id, ocr_page);

    let new_bytes = save_bytes(&new.doc_id);
    let old_bytes = save_bytes(&old.doc_id);
    std::fs::write(out_dir().join("glyphless-layer.pdf"), &new_bytes).expect("write sample");
    std::fs::write(out_dir().join("hangul-subset-layer.pdf"), &old_bytes).expect("write sample");
    let grew_new = new_bytes.len() as i64 - base.len() as i64;
    let grew_old = old_bytes.len() as i64 - base.len() as i64;
    eprintln!(
        "OCR layer growth: glyphless {grew_new} B, Hangul subset {grew_old} B ({:.1} %)",
        grew_new as f64 * 100.0 / grew_old as f64
    );
    assert!(
        grew_old > 100_000,
        "the subset really was embedded: {grew_old} B"
    );
    assert!(
        grew_new * 10 <= grew_old,
        "the glyphless layer costs at most a tenth: {grew_new} B vs {grew_old} B"
    );

    // Same text after save + reopen, character for character — including 日本語 and 中文,
    // which the KS X 1001 subset only partly covered.
    let new_saved = reopen(new_bytes);
    let old_saved = reopen(old_bytes);
    let text_new = page_text(&new_saved.doc_id, 0);
    let text_old = page_text(&old_saved.doc_id, 0);
    assert!(
        text_new.contains("검색 가능한 한글 문서"),
        "Korean with spaces: {text_new:?}"
    );
    assert!(
        text_new.contains("SeePDF OCR 정확도 측정용"),
        "{text_new:?}"
    );
    assert!(text_new.contains("日本語の 中文 2026년"), "{text_new:?}");
    // The third line is where the subset fell short (its kana and hanja extract as NULs), so
    // the comparison covers the lines both fonts can write.
    let covered = |t: &str| t.lines().take(2).collect::<Vec<_>>().join("\n");
    assert_eq!(
        covered(&text_new),
        covered(&text_old),
        "extraction identical to the Hangul-subset layer"
    );
    for needle in ["검색 가능한", "정확도", "SeePDF", "日本語"] {
        assert!(
            search_hits(&new_saved.doc_id, 0, needle) >= 1,
            "search({needle:?}) in {text_new:?}"
        );
    }

    // Every character has a real box (the glyph is a rectangle, never painted).
    let text_layer = with_doc(&new_saved.doc_id, |d| layer::layer(d, 0)).expect("text layer");
    let hangul_boxes = text_layer
        .chars
        .iter()
        .filter(|c| c.codepoint >= 0xAC00 && c.codepoint <= 0xD7A3)
        .filter(|c| c.loose.width() > 0.5 && c.loose.height() > 0.5)
        .count();
    assert!(hangul_boxes >= 10, "Hangul characters have boxes");
}

// ---------------------------------------------------------------------------------------
// O2 — auto-rotate
// ---------------------------------------------------------------------------------------

/// `setRotation` sets `/Rotate` inside the OCR's own undo step, and the layer is written in
/// the new display space: the words read upright and one undo restores both.
#[test]
fn ocr_set_rotation_turns_the_page_in_the_same_undo_step() {
    // Sideways: the sheet was scanned turned 90° counter-clockwise, so reading it needs a
    // quarter turn clockwise.
    let scan = scan_of("gen/korean-300dpi.pdf", 150, 270);
    assert_eq!(rotation_of(&scan.doc_id, 0), 0);
    let depth = with_doc(&scan.doc_id, |d| Ok(d.history.undo_depth())).unwrap();

    // What the recogniser saw once the image was turned 90° clockwise: the upright page.
    let turned = orientation::rotate_gray(&gray(&scan.doc_id, 0, DPI), 90);
    let ocr_page = korean_ocr(90, turned.width, turned.height);

    // A result recognised at 0° cannot be applied with setRotation 90.
    let bad = korean_ocr(0, turned.width, turned.height);
    let doc_id = scan.doc_id.clone();
    let refused = with_state(move |st| {
        let mut progress = |_: usize, _: u16| {};
        ocr::apply_rotated_cancellable(
            st,
            &doc_id,
            &[bad],
            &[Some(90)],
            false,
            &mut progress,
            &|| false,
        )
    });
    assert!(refused.is_err());

    apply(&scan.doc_id, vec![ocr_page], vec![Some(90)]);
    assert_eq!(rotation_of(&scan.doc_id, 0), 90, "/Rotate 90");
    let text = page_text(&scan.doc_id, 0);
    assert!(text.contains("검색 가능한 한글 문서"), "{text:?}");
    assert_eq!(
        with_doc(&scan.doc_id, |d| Ok(d.history.undo_depth())).unwrap(),
        depth + 1,
        "rotation and layer are one undo step"
    );

    let saved = reopen(save_bytes(&scan.doc_id));
    assert_eq!(rotation_of(&saved.doc_id, 0), 90);
    assert!(search_hits(&saved.doc_id, 0, "한글 문서") >= 1);

    let doc_id = scan.doc_id.clone();
    with_state(move |st| registry::undo(st, &doc_id, false)).expect("undo");
    assert_eq!(rotation_of(&scan.doc_id, 0), 0, "undo turns the page back");
    assert!(
        page_text(&scan.doc_id, 0).trim().is_empty(),
        "and drops the layer"
    );
}

#[test]
fn ocr_orientation_pick_matches_the_frontend_rule() {
    use orientation::{pick, OrientationScore};
    let s = |rotation, confidence, words| OrientationScore {
        rotation,
        confidence,
        words,
    };
    assert_eq!(
        pick(&[
            s(0, 31.0, 6),
            s(90, 88.0, 40),
            s(180, 29.0, 4),
            s(270, 35.0, 7)
        ]),
        90
    );
    assert_eq!(
        pick(&[
            s(0, 88.0, 40),
            s(90, 31.0, 6),
            s(180, 29.0, 4),
            s(270, 35.0, 7)
        ]),
        0
    );
    assert_eq!(pick(&[s(0, 80.0, 40), s(90, 85.0, 40)]), 0, "within 15 %");
}

/// The whole O2 path on a real recogniser: a sideways scan is detected at 100 DPI, read turned,
/// applied with `setRotation`, and ends `/Rotate 90` with the Korean words searchable.
#[cfg(target_os = "macos")]
#[test]
fn ocr_auto_rotate_with_vision_reads_a_sideways_scan() {
    if !ocr::vision_available() {
        eprintln!("skipped: Vision cannot read Korean here");
        return;
    }
    let scan = scan_of("gen/korean-300dpi.pdf", 200, 270);
    let langs = vec!["kor".to_string(), "eng".to_string()];

    let small = gray(&scan.doc_id, 0, orientation::DETECT_DPI);
    let detected = ocr::detect_orientation(&small, &langs).expect("detect");
    eprintln!("orientation scores: {:?}", detected.scores);
    assert_eq!(detected.rotation, 90, "{:?}", detected.scores);

    // An upright page stays as it is.
    let upright = scan_of("gen/korean-300dpi.pdf", 200, 0);
    let stays = ocr::detect_orientation(&gray(&upright.doc_id, 0, orientation::DETECT_DPI), &langs)
        .expect("detect upright");
    assert_eq!(stays.rotation, 0, "{:?}", stays.scores);

    let full = orientation::rotate_gray(&gray(&scan.doc_id, 0, DPI), detected.rotation);
    let ocr_page = ocr::recognize_gray(&full, &langs).expect("Vision");
    assert_eq!(ocr_page.rotation, 90);
    apply(&scan.doc_id, vec![ocr_page], vec![Some(90)]);

    let saved = reopen(save_bytes(&scan.doc_id));
    assert_eq!(rotation_of(&saved.doc_id, 0), 90, "/Rotate 90");
    let text = page_text(&saved.doc_id, 0);
    for needle in ["검색", "한글", "문서", "정확도"] {
        assert!(text.contains(needle), "{needle:?} in {text:?}");
    }
    assert!(search_hits(&saved.doc_id, 0, "한글") >= 1);
}

// ---------------------------------------------------------------------------------------
// O1 — languages per engine
// ---------------------------------------------------------------------------------------

#[test]
fn ocr_capabilities_list_languages_per_engine() {
    let caps = ocr::capabilities();
    assert_eq!(caps.engine_languages.tesseract, ["kor", "eng"]);
    assert_eq!(caps.languages, ["kor", "eng"]);
    if caps.engines.contains(&OcrEngine::Vision) {
        assert!(caps.engine_languages.vision.iter().any(|l| l == "kor"));
    } else {
        assert!(caps.engine_languages.vision.is_empty());
    }
    if !caps.engines.contains(&OcrEngine::Windows) {
        assert!(caps.engine_languages.windows.is_empty());
    }
    let json = serde_json::to_value(&caps).unwrap();
    assert!(json["engineLanguages"]["tesseract"].is_array());
}

/// Vision on this Mac reads Japanese and Simplified Chinese (macOS 13+), so the 日本語 / 中文
/// chips appear for it, and a request for `ja-JP` is accepted and reads kana.
#[cfg(target_os = "macos")]
#[test]
fn ocr_vision_accepts_japanese() {
    if !ocr::vision_available() {
        eprintln!("skipped: Vision cannot read Korean here");
        return;
    }
    let langs = ocr::vision_languages();
    assert!(langs.iter().any(|l| l == "jpn"), "{langs:?}");
    assert!(langs.iter().any(|l| l == "chi_sim"), "{langs:?}");

    // Japanese drawn in the Mac's Arial Unicode (kana + kanji); the document is never saved,
    // so the 23 MB font only lives in memory for the test.
    let sample = "日本語のテキスト認識";
    let font = [
        "/System/Library/Fonts/Supplemental/Arial Unicode.ttf",
        "/Library/Fonts/Arial Unicode.ttf",
    ]
    .iter()
    .find_map(|p| std::fs::read(p).ok());
    let covered = font.is_some();
    let doc = open("gen/korean-300dpi.pdf");
    if let Some(font) = font {
        with_doc(&doc.doc_id, move |d| {
            use pdfium_render::prelude::*;
            let token = d
                .pdf_mut()
                .fonts_mut()
                .load_true_type_from_bytes(&font, true)
                .expect("load Arial Unicode");
            let mut page = d.pdf().pages().get(0).expect("page 0");
            page.objects_mut()
                .create_text_object(
                    PdfPoints::new(64.0),
                    PdfPoints::new(420.0),
                    sample,
                    token,
                    PdfPoints::new(28.0),
                )
                .expect("Japanese text");
            page.regenerate_content().expect("regenerate");
            Ok(())
        })
        .expect("draw Japanese");
    }
    // The drawing bypassed `mutate` (and the render cache): read a fresh copy.
    let doc = reopen(save_bytes(&doc.doc_id));
    let image = gray(&doc.doc_id, 0, DPI);
    let page = ocr::recognize_gray(&image, &["ja-JP".to_string()]).expect("Vision takes ja-JP");
    let text: String = page
        .lines
        .iter()
        .map(|l| l.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    eprintln!("Vision ja-JP read: {text:?}");
    if covered {
        assert!(
            text.contains("日本語") && text.contains("テキスト"),
            "kanji and kana read with ja-JP: {text:?}"
        );
    } else {
        assert!(!page.lines.is_empty());
    }
    // The sheet can ask for several at once (한국어 + English + 日本語 + 中文): Vision takes the
    // combination rather than refusing it.
    let all: Vec<String> = ["kor", "eng", "jpn", "chi_sim"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let mixed = ocr::recognize_gray(&image, &all).expect("Vision takes ko + en + ja + zh");
    assert!(!mixed.lines.is_empty());
}

// ---------------------------------------------------------------------------------------
// O3 — Windows.Media.Ocr
// ---------------------------------------------------------------------------------------

/// On a Windows runner with the English recogniser: page 1 of tracemonkey through the native
/// path, normalised like Vision's, and applied.
#[cfg(windows)]
#[test]
fn ocr_windows_reads_an_english_fixture() {
    let available = ocr::winocr::win::available_languages();
    eprintln!("Windows OCR recognisers: {available:?}");
    if !available
        .iter()
        .any(|t| ocr::winocr::app_code(t) == Some("eng"))
    {
        eprintln!("skipped: no English recogniser on this machine");
        return;
    }
    let caps = ocr::capabilities();
    assert!(caps.engines.contains(&OcrEngine::Windows));
    assert!(caps.engine_languages.windows.iter().any(|l| l == "eng"));

    let doc = open("tracemonkey.pdf");
    let image = gray(&doc.doc_id, 0, DPI);
    let page = ocr::recognize_gray(&image, &["eng".to_string()]).expect("Windows OCR");
    let text: String = page
        .lines
        .iter()
        .map(|l| l.text.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    for needle in ["Trace-based", "Just-in-Time", "Dynamic"] {
        assert!(text.contains(needle), "{needle:?} in {text:?}");
    }
    assert!(page.lines.iter().all(|l| l
        .words
        .iter()
        .all(|w| w.bbox[2] <= image.width as f32 + 1.0)));
}
