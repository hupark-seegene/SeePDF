//! OCR feasibility spike – test-image producer.
//!
//! Produces the images that `scripts/spike-ocr.mjs` OCRs, plus ground-truth text files so the
//! Node script can compute a character error rate (CER):
//!
//! * `fixtures/out/ocr/tracemonkey-p1-300dpi.png`  – page 1 of `fixtures/tracemonkey.pdf` at 300 DPI
//! * `fixtures/out/ocr/tracemonkey-p1.txt`         – pdfium text extraction of that page (ground truth)
//! * `fixtures/out/ocr/korean-300dpi.png`          – synthetic Korean + English page at 300 DPI
//! * `fixtures/out/ocr/korean.txt`                 – the text that was placed on the synthetic page
//! * `fixtures/out/ocr/korean-synthetic.pdf`       – the synthetic PDF itself (for later spikes)
//!
//! It also runs a tiny proof of the "searchable text layer" mechanism the OCR feature needs:
//! invisible text objects (text render mode 3) are added to a page, the document is saved and
//! reloaded, and we confirm (a) pdfium extracts the text and (b) the rendered bitmap stays blank.
//!
//! Run from `src-tauri/`:
//!
//! ```sh
//! export PATH="$HOME/.cargo/bin:$PATH"
//! cargo run --example spike_ocr_render
//! ```

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use pdfium_render::prelude::*;

/// Target OCR resolution. 300 DPI is the classic sweet spot for Tesseract (it wants ~30 px x-height).
const DPI: f32 = 300.0;

/// The Korean + English lines placed on the synthetic page. Kept as (text, font size) so the
/// ground-truth file is exactly what is drawn.
const KO_LINES: &[(&str, f32)] = &[
    ("SeePDF 광학 문자 인식 테스트 페이지", 18.0),
    ("이 문서는 한국어와 영어가 섞인 스캔 문서를 흉내 내기 위해 만들어졌습니다.", 12.0),
    ("대한민국의 수도는 서울이며, 부산은 두 번째로 큰 도시입니다.", 12.0),
    ("계약서 제3조 (대금 지급) 갑은 을에게 2026년 9월 30일까지 금 1,250,000원을 지급한다.", 11.0),
    ("The quick brown fox jumps over the lazy dog. 0123456789", 12.0),
    ("Optical character recognition converts scanned images into searchable text.", 11.0),
    ("영수증 번호: KR-2026-0915-0042   합계: ₩ 48,500   부가세 포함", 11.0),
    ("Mixed line: 회의는 Tuesday 오후 3시에 Room 402에서 진행됩니다.", 12.0),
    ("작은 글씨 테스트 – 가나다라마바사아자차카타파하 – small 9pt text line", 9.0),
    ("띄어쓰기와 문장 부호, 그리고 괄호(소괄호)와 [대괄호]까지 인식되어야 합니다.", 11.0),
];

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("src-tauri has a parent")
        .to_path_buf()
}

fn pdfium_library_path() -> PathBuf {
    let triple = if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        "libpdfium-aarch64-apple-darwin.dylib"
    } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
        "libpdfium-x86_64-apple-darwin.dylib"
    } else if cfg!(target_os = "windows") {
        "pdfium-x86_64-pc-windows-msvc.dll"
    } else {
        "libpdfium-x86_64-unknown-linux-gnu.so"
    };
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("resources")
        .join("pdfium")
        .join(triple)
}

fn render_page_png(page: &PdfPage<'_>, out: &Path) -> Result<(u32, u32, f64), PdfiumError> {
    let t = Instant::now();
    let config = PdfRenderConfig::new()
        .scale_page_by_factor(DPI / 72.0)
        .render_form_data(true)
        .render_annotations(true)
        .set_text_smoothing(true);
    let bitmap = page.render_with_config(&config)?;
    let image = bitmap.as_image()?; // RGBA8
    let render_ms = t.elapsed().as_secs_f64() * 1000.0;
    // Save as 8-bit grayscale: OCR engines binarise anyway and the PNG is ~3x smaller.
    let gray = image.into_luma8();
    let (w, h) = (gray.width(), gray.height());
    gray.save(out).map_err(|_| PdfiumError::ImageError)?;
    Ok((w, h, render_ms))
}

fn all_white(page: &PdfPage<'_>) -> Result<bool, PdfiumError> {
    let bitmap = page.render_with_config(&PdfRenderConfig::new().scale_page_by_factor(2.0))?;
    let bytes = bitmap.as_rgba_bytes();
    Ok(bytes
        .chunks_exact(4)
        .all(|px| px[0] == 255 && px[1] == 255 && px[2] == 255))
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = repo_root();
    let out_dir = root.join("fixtures").join("out").join("ocr");
    fs::create_dir_all(&out_dir)?;

    let lib = pdfium_library_path();
    println!("pdfium: {}", lib.display());
    let pdfium = Pdfium::new(Pdfium::bind_to_library(&lib)?);

    // ------------------------------------------------------------------------------------------
    // 1. tracemonkey.pdf page 1 at 300 DPI + ground truth
    // ------------------------------------------------------------------------------------------
    {
        let doc = pdfium.load_pdf_from_file(&root.join("fixtures").join("tracemonkey.pdf"), None)?;
        let page = doc.pages().first()?;
        println!(
            "tracemonkey p1: {:.1} x {:.1} pt, rotation {:?}",
            page.width().value,
            page.height().value,
            page.rotation()?
        );
        let png = out_dir.join("tracemonkey-p1-300dpi.png");
        let (w, h, ms) = render_page_png(&page, &png)?;
        println!(
            "  rendered {}x{} px in {:.0} ms -> {} ({} KB)",
            w,
            h,
            ms,
            png.display(),
            fs::metadata(&png)?.len() / 1024
        );
        let text = page.text()?.all();
        let txt = out_dir.join("tracemonkey-p1.txt");
        fs::write(&txt, &text)?;
        println!(
            "  ground truth: {} chars, {} words -> {}",
            text.chars().count(),
            text.split_whitespace().count(),
            txt.display()
        );
    }

    // ------------------------------------------------------------------------------------------
    // 2. Synthetic Korean + English page
    // ------------------------------------------------------------------------------------------
    {
        let mut doc = pdfium.create_new_pdf()?;

        // Font: try the .ttc first (task requirement), fall back to single-face TTFs with Hangul.
        let candidates = [
            "/System/Library/Fonts/AppleSDGothicNeo.ttc",
            "/System/Library/Fonts/Supplemental/AppleGothic.ttf",
            "/System/Library/Fonts/Supplemental/AppleMyungjo.ttf",
            "/System/Library/Fonts/Supplemental/Arial Unicode.ttf",
        ];
        let mut font: Option<(PdfFontToken, &str)> = None;
        for path in candidates {
            if !Path::new(path).exists() {
                println!("font {path}: not present");
                continue;
            }
            match doc.fonts_mut().load_true_type_from_file(path, true) {
                Ok(token) => {
                    println!("font {path}: loaded OK (is_cid_font = true)");
                    font = Some((token, path));
                    break;
                }
                Err(e) => println!("font {path}: FAILED to load: {e:?}"),
            }
        }
        let (font, font_path) = font.ok_or("no Korean-capable font could be loaded")?;

        let mut page = doc.pages_mut().create_page_at_end(PdfPagePaperSize::a4())?;
        let page_h = page.height().value;
        let margin_x = 56.0_f32; // ~20 mm
        let mut y = page_h - 80.0; // baseline of first line, from the bottom edge
        let mut truth = String::new();
        for (text, size) in KO_LINES {
            let mut obj = PdfPageTextObject::new(&doc, *text, font, PdfPoints::new(*size))?;
            obj.set_fill_color(PdfColor::new(0, 0, 0, 255))?;
            obj.translate(PdfPoints::new(margin_x), PdfPoints::new(y))?;
            page.objects_mut().add_text_object(obj)?;
            truth.push_str(text);
            truth.push('\n');
            y -= size * 2.2;
        }

        let pdf_path = out_dir.join("korean-synthetic.pdf");
        doc.save_to_file(&pdf_path)?;
        println!(
            "synthetic PDF ({}): {} KB (font embedded from {})",
            pdf_path.display(),
            fs::metadata(&pdf_path)?.len() / 1024,
            font_path
        );

        // Confirm pdfium can round-trip the Hangul through its own text extraction.
        let extracted = page.text()?.all();
        let hangul_ok = extracted.contains("서울") && extracted.contains("부가세");
        println!("  pdfium text extraction of synthetic page contains Hangul: {hangul_ok}");

        let png = out_dir.join("korean-300dpi.png");
        let (w, h, ms) = render_page_png(&page, &png)?;
        println!(
            "  rendered {}x{} px in {:.0} ms -> {} ({} KB)",
            w,
            h,
            ms,
            png.display(),
            fs::metadata(&png)?.len() / 1024
        );
        let txt = out_dir.join("korean.txt");
        fs::write(&txt, &truth)?;
        println!(
            "  ground truth: {} chars, {} words -> {}",
            truth.chars().count(),
            truth.split_whitespace().count(),
            txt.display()
        );
    }

    // ------------------------------------------------------------------------------------------
    // 3. Proof: invisible text layer (render mode 3) is searchable and does not paint pixels
    // ------------------------------------------------------------------------------------------
    {
        let mut doc = pdfium.create_new_pdf()?;
        let font = doc
            .fonts_mut()
            .load_true_type_from_file("/System/Library/Fonts/Supplemental/AppleGothic.ttf", true)?;
        let mut page = doc.pages_mut().create_page_at_end(PdfPagePaperSize::a4())?;

        // Simulate two OCR word boxes coming back from the OCR engine, in PDF points
        // (x, baseline_y, box_width, font_size_from_box_height).
        let words: [(&str, f32, f32, f32, f32); 2] = [
            ("검색가능한", 72.0, 700.0, 90.0, 14.0),
            ("Searchable", 170.0, 700.0, 80.0, 14.0),
        ];
        for (word, x, y, box_w, size) in words {
            let mut obj = PdfPageTextObject::new(&doc, word, font, PdfPoints::new(size))?;
            obj.set_render_mode(PdfPageTextRenderMode::Invisible)?;
            // Horizontal scale so the glyph run fits the OCR box width exactly.
            let nb = obj.bounds()?;
            let natural_w = nb.right().value - nb.left().value;
            let sx = if natural_w > 0.0 { box_w / natural_w } else { 1.0 };
            obj.scale(sx, 1.0)?;
            obj.translate(PdfPoints::new(x), PdfPoints::new(y))?;
            let added = page.objects_mut().add_text_object(obj)?;
            let b = added.bounds()?;
            println!(
                "  invisible word '{}' natural width {:.1} pt, scaled x{:.3}, bounds l={:.1} b={:.1} r={:.1} t={:.1}",
                word, natural_w, sx, b.left().value, b.bottom().value, b.right().value, b.top().value
            );
        }
        println!(
            "  page renders all-white with invisible text: {}",
            all_white(&page)?
        );

        // Save, reload, extract: proves the text survives a round trip and is searchable.
        let bytes = doc.save_to_bytes()?;
        let reloaded = pdfium.load_pdf_from_byte_slice(&bytes, None)?;
        let rp = reloaded.pages().first()?;
        let extracted = rp.text()?.all();
        println!("  reloaded text: {:?}", extracted);
        let rt = rp.text()?;
        let hits = rt
            .search("Searchable", &PdfSearchOptions::new())?
            .iter(PdfSearchDirection::SearchForward)
            .count();
        println!(
            "  search('Searchable') hits: {}   contains Hangul word: {}",
            hits,
            extracted.contains("검색가능한")
        );
        let proof = out_dir.join("invisible-text-proof.pdf");
        fs::write(&proof, &bytes)?;
        println!("  wrote {}", proof.display());
    }

    // ------------------------------------------------------------------------------------------
    // 4. End-to-end: 300-DPI "scan" image + OCR word boxes (from scripts/spike-ocr.mjs) ->
    //    searchable PDF. Times AutomaticOnEveryChange vs Manual content regeneration.
    // ------------------------------------------------------------------------------------------
    {
        let words_json = out_dir.join("ocr-tracemonkey-eng-best_int.words.json");
        if !words_json.exists() {
            println!(
                "part 4 skipped: {} missing (run `node scripts/spike-ocr.mjs` first)",
                words_json.display()
            );
        } else {
            #[derive(serde::Deserialize)]
            struct Bbox {
                x0: f32,
                y0: f32,
                x1: f32,
                y1: f32,
            }
            #[derive(serde::Deserialize)]
            struct OcrWord {
                t: String,
                c: f32,
                b: Bbox,
            }
            let words: Vec<OcrWord> = serde_json::from_str(&fs::read_to_string(&words_json)?)?;
            let scan = image::open(out_dir.join("tracemonkey-p1-300dpi.png"))?;
            let (img_w, img_h) = (scan.width() as f32, scan.height() as f32);
            let s = 72.0 / DPI; // image pixels -> PDF points
            let (page_w, page_h) = (img_w * s, img_h * s);

            for manual in [false, true] {
                let strategy = if manual {
                    PdfPageContentRegenerationStrategy::Manual
                } else {
                    PdfPageContentRegenerationStrategy::AutomaticOnEveryChange
                };
                let t = Instant::now();
                let mut doc = pdfium.create_new_pdf()?;
                // Latin-only words: the built-in Helvetica needs no embedding (tiny file).
                // Words with non-Latin-1 characters must use an embedded CID font instead.
                let font = doc.fonts_mut().helvetica();
                let mut page = doc.pages_mut().create_page_at_end(PdfPagePaperSize::Custom(
                    PdfPoints::new(page_w),
                    PdfPoints::new(page_h),
                ))?;
                page.set_content_regeneration_strategy(strategy);
                page.objects_mut().create_image_object(
                    PdfPoints::new(0.0),
                    PdfPoints::new(0.0),
                    &scan,
                    Some(PdfPoints::new(page_w)),
                    Some(PdfPoints::new(page_h)),
                )?;
                let t_img = t.elapsed();
                let (mut added, mut skipped) = (0usize, 0usize);
                for w in &words {
                    let box_w = (w.b.x1 - w.b.x0) * s;
                    let box_h = (w.b.y1 - w.b.y0) * s;
                    if w.t.trim().is_empty() || w.c < 30.0 || box_w <= 0.0 || box_h <= 0.0 {
                        skipped += 1;
                        continue;
                    }
                    // Font size from the box height (word box ~ 0.87 em for mixed-case Latin),
                    // then horizontal scale so the glyph run spans exactly the box width.
                    let font_size = box_h * 1.15;
                    let mut obj = PdfPageTextObject::new(&doc, &w.t, font, PdfPoints::new(font_size))?;
                    obj.set_render_mode(PdfPageTextRenderMode::Invisible)?;
                    let nb = obj.bounds()?;
                    let natural_w = nb.right().value - nb.left().value;
                    if natural_w > 0.0 {
                        obj.scale(box_w / natural_w, 1.0)?;
                    }
                    // Baseline ~ box bottom + a descender allowance (a line baseline from the OCR
                    // engine is better when available).
                    let baseline_y = page_h - w.b.y1 * s + box_h * 0.2;
                    obj.translate(PdfPoints::new(w.b.x0 * s), PdfPoints::new(baseline_y))?;
                    page.objects_mut().add_text_object(obj)?;
                    added += 1;
                }
                if manual {
                    page.regenerate_content()?;
                }
                let t_words = t.elapsed() - t_img;
                let out = out_dir.join(format!(
                    "tracemonkey-p1-searchable-{}.pdf",
                    if manual { "manual" } else { "auto" }
                ));
                doc.save_to_file(&out)?;
                let total = t.elapsed();
                println!(
                    "searchable PDF ({}): image {:.0} ms, {} words added ({} skipped) in {:.0} ms, total incl. save {:.0} ms, {} KB",
                    if manual { "Manual regen" } else { "AutomaticOnEveryChange" },
                    t_img.as_secs_f64() * 1000.0,
                    added,
                    skipped,
                    t_words.as_secs_f64() * 1000.0,
                    total.as_secs_f64() * 1000.0,
                    fs::metadata(&out)?.len() / 1024
                );
                let re = pdfium.load_pdf_from_file(&out, None)?;
                let rp = re.pages().first()?;
                let rt = rp.text()?;
                let hits = rt
                    .search("Trace-based", &PdfSearchOptions::new())?
                    .iter(PdfSearchDirection::SearchForward)
                    .count();
                println!(
                    "  reloaded: {} chars extracted, search('Trace-based') hits = {}",
                    rt.all().chars().count(),
                    hits
                );
            }
        }
    }

    println!("done.");
    Ok(())
}
