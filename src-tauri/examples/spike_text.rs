//! spike_text: text layer / search / selection / text editing / invisible OCR layer / save
//! for pdfium-render 0.9.4 against the bundled libpdfium (chromium/8057).
//!
//! Run from src-tauri:  cargo run --example spike_text
//! Outputs go to fixtures/out/spike_text_*.{pdf,png}.  Report: docs/spikes/text.md

use pdfium_render::prelude::*;
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::time::Instant;

type R<T> = Result<T, Box<dyn std::error::Error>>;

// ---------------------------------------------------------------------------------------------
// paths
// ---------------------------------------------------------------------------------------------

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf()
}

fn out_dir() -> PathBuf {
    let d = repo_root().join("fixtures/out");
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn pdfium_lib_path() -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources/pdfium");
    let name = if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        "libpdfium-aarch64-apple-darwin.dylib"
    } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
        "libpdfium-x86_64-apple-darwin.dylib"
    } else if cfg!(target_os = "windows") {
        "pdfium-x86_64-pc-windows-msvc.dll"
    } else {
        "libpdfium-x86_64-unknown-linux-gnu.so"
    };
    dir.join(name)
}

// ---------------------------------------------------------------------------------------------
// geometry helpers
// ---------------------------------------------------------------------------------------------

/// Axis-aligned rect in device pixels, origin top-left, y grows downward.
#[derive(Debug, Clone, Copy, PartialEq)]
struct DeviceRect {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
}

/// Convert a PDF user-space rect (points, origin bottom-left, y up) to device pixels
/// (origin top-left, y down) for an *unrotated* page rendered at `scale` px per point.
/// `page_box` is the visible page box (crop box) in points; for most files it is
/// (0,0)-(w,h) but a non-zero crop-box origin must be subtracted.
fn pdf_rect_to_device(rect: &PdfRect, page_box: &PdfRect, scale: f32) -> DeviceRect {
    DeviceRect {
        x: (rect.left().value - page_box.left().value) * scale,
        y: (page_box.top().value - rect.top().value) * scale,
        w: rect.width().value * scale,
        h: rect.height().value * scale,
    }
}

/// Inverse of `pdf_rect_to_device` for a single point (device px -> PDF points).
fn device_to_pdf_point(x_px: f32, y_px: f32, page_box: &PdfRect, scale: f32) -> (f32, f32) {
    (
        x_px / scale + page_box.left().value,
        page_box.top().value - y_px / scale,
    )
}

/// General page -> device affine matrix [a b c d e f] (dx = a*x + c*y + e, dy = b*x + d*y + f)
/// for a page with visible box `cb` (crop box, user space), display rotation `rotate_deg`
/// (0/90/180/270 clockwise, i.e. the page's /Rotate) and `scale` px per point.
/// The frontend can apply this single matrix to every char/word rect it receives.
fn page_to_device_matrix(cb: &PdfRect, rotate_deg: i32, scale: f32) -> [f32; 6] {
    let (cl, cbm, cr, ct) = (
        cb.left().value,
        cb.bottom().value,
        cb.right().value,
        cb.top().value,
    );
    let s = scale;
    match rotate_deg.rem_euclid(360) {
        90 => [0.0, s, s, 0.0, -cbm * s, -cl * s],
        180 => [-s, 0.0, 0.0, s, cr * s, -cbm * s],
        270 => [0.0, -s, -s, 0.0, ct * s, cr * s],
        _ => [s, 0.0, 0.0, -s, -cl * s, ct * s],
    }
}

fn apply_affine(m: &[f32; 6], x: f32, y: f32) -> (f32, f32) {
    (m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5])
}

/// Transform a PDF rect by the page->device matrix and return its device bounding box.
fn rect_to_device_with_matrix(m: &[f32; 6], r: &PdfRect) -> DeviceRect {
    let pts = [
        apply_affine(m, r.left().value, r.bottom().value),
        apply_affine(m, r.right().value, r.bottom().value),
        apply_affine(m, r.right().value, r.top().value),
        apply_affine(m, r.left().value, r.top().value),
    ];
    let x0 = pts.iter().map(|p| p.0).fold(f32::MAX, f32::min);
    let x1 = pts.iter().map(|p| p.0).fold(f32::MIN, f32::max);
    let y0 = pts.iter().map(|p| p.1).fold(f32::MAX, f32::min);
    let y1 = pts.iter().map(|p| p.1).fold(f32::MIN, f32::max);
    DeviceRect {
        x: x0,
        y: y0,
        w: x1 - x0,
        h: y1 - y0,
    }
}

fn union(a: &PdfRect, b: &PdfRect) -> PdfRect {
    PdfRect::new_from_values(
        a.bottom().value.min(b.bottom().value),
        a.left().value.min(b.left().value),
        a.top().value.max(b.top().value),
        a.right().value.max(b.right().value),
    )
}

fn fmt_rect(r: &PdfRect) -> String {
    format!(
        "[l={:.2} b={:.2} r={:.2} t={:.2} w={:.2} h={:.2}]",
        r.left().value,
        r.bottom().value,
        r.right().value,
        r.top().value,
        r.width().value,
        r.height().value
    )
}

fn fmt_quad(q: &PdfQuadPoints) -> String {
    fmt_rect(&q.to_rect())
}

fn fmt_matrix(m: &PdfMatrix) -> String {
    format!(
        "[{:.2} {:.2} {:.2} {:.2} {:.2} {:.2}]",
        m.a(),
        m.b(),
        m.c(),
        m.d(),
        m.e(),
        m.f()
    )
}

// ---------------------------------------------------------------------------------------------
// text layer model
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct TextCharInfo {
    index: usize,
    ch: char,
    loose: PdfRect,
    tight: Option<PdfRect>,
    origin: (f32, f32),
    font_size: f32,
    font_name: String,
    font_weight: Option<PdfFontWeight>,
    generated: bool,
}

#[derive(Debug, Clone)]
struct Word {
    text: String,
    rect: PdfRect,
    first: usize,
    last: usize,
    baseline: f32,
    font_size: f32,
}

#[derive(Debug, Clone)]
struct Line {
    words: Vec<Word>,
    rect: PdfRect,
    baseline: f32,
}

fn is_separator(c: &TextCharInfo) -> bool {
    c.ch.is_whitespace() || c.ch == '\u{fffe}' || c.ch == '\u{ffff}'
}

/// Group chars into words and lines. Words break on whitespace, on a baseline jump
/// (> 0.5 em) and on a horizontal gap (> 0.25 em) or a backwards x move (new line).
fn build_text_layer(chars: &[TextCharInfo]) -> Vec<Line> {
    let mut words: Vec<Word> = Vec::new();
    let mut cur: Option<Word> = None;

    let flush = |cur: &mut Option<Word>, words: &mut Vec<Word>| {
        if let Some(w) = cur.take() {
            if !w.text.is_empty() {
                words.push(w);
            }
        }
    };

    for c in chars {
        if is_separator(c) {
            flush(&mut cur, &mut words);
            continue;
        }
        let em = c.font_size.max(1.0);
        let start_new = match &cur {
            None => true,
            Some(w) => {
                let dy = (w.baseline - c.origin.1).abs();
                let gap = c.loose.left().value - w.rect.right().value;
                dy > 0.5 * em || gap > 0.25 * em || gap < -0.5 * em
            }
        };
        if start_new {
            flush(&mut cur, &mut words);
            cur = Some(Word {
                text: String::new(),
                rect: c.loose,
                first: c.index,
                last: c.index,
                baseline: c.origin.1,
                font_size: c.font_size,
            });
        }
        let w = cur.as_mut().unwrap();
        w.text.push(c.ch);
        w.rect = union(&w.rect, &c.loose);
        w.last = c.index;
    }
    flush(&mut cur, &mut words);

    // lines: consecutive words with the same baseline (within 0.5 em)
    let mut lines: Vec<Line> = Vec::new();
    for w in words {
        let em = w.font_size.max(1.0);
        let join = match lines.last() {
            Some(l) => (l.baseline - w.baseline).abs() <= 0.5 * em,
            None => false,
        };
        if join {
            let l = lines.last_mut().unwrap();
            l.rect = union(&l.rect, &w.rect);
            l.words.push(w);
        } else {
            lines.push(Line {
                rect: w.rect,
                baseline: w.baseline,
                words: vec![w],
            });
        }
    }
    lines
}

fn collect_chars(text: &PdfPageText) -> R<Vec<TextCharInfo>> {
    let chars = text.chars();
    let mut out = Vec::with_capacity(chars.len());
    for c in chars.iter() {
        let loose = c.loose_bounds()?;
        out.push(TextCharInfo {
            index: c.index(),
            ch: c.unicode_char().unwrap_or('\u{fffd}'),
            loose,
            tight: c.tight_bounds().ok(),
            origin: c
                .origin()
                .map(|(x, y)| (x.value, y.value))
                .unwrap_or((loose.left().value, loose.bottom().value)),
            font_size: c.scaled_font_size().value,
            font_name: c.font_name(),
            font_weight: c.font_weight(),
            generated: c.is_generated().unwrap_or(false),
        });
    }
    Ok(out)
}

// ---------------------------------------------------------------------------------------------
// rendering helpers
// ---------------------------------------------------------------------------------------------

fn render_rgb(page: &PdfPage, scale: f32) -> R<image::RgbImage> {
    let cfg = PdfRenderConfig::new().scale_page_by_factor(scale);
    let bmp = page.render_with_config(&cfg)?;
    Ok(bmp.as_image()?.into_rgb8())
}

fn save_png(img: &image::RgbImage, name: &str) -> R<PathBuf> {
    let p = out_dir().join(name);
    img.save(&p)?;
    Ok(p)
}

fn crop_png(img: &image::RgbImage, r: &DeviceRect, pad: f32, name: &str) -> R<PathBuf> {
    let x = (r.x - pad).max(0.0) as u32;
    let y = (r.y - pad).max(0.0) as u32;
    let w = ((r.w + 2.0 * pad) as u32).min(img.width().saturating_sub(x));
    let h = ((r.h + 2.0 * pad) as u32).min(img.height().saturating_sub(y));
    let crop = image::imageops::crop_imm(img, x, y, w.max(1), h.max(1)).to_image();
    save_png(&crop, name)
}

/// Count "dark" pixels (any channel < 128) inside a device rect.
fn dark_pixels(img: &image::RgbImage, r: &DeviceRect) -> u32 {
    let mut n = 0;
    let x0 = r.x.max(0.0) as u32;
    let y0 = r.y.max(0.0) as u32;
    let x1 = ((r.x + r.w).ceil() as u32).min(img.width());
    let y1 = ((r.y + r.h).ceil() as u32).min(img.height());
    for y in y0..y1 {
        for x in x0..x1 {
            let p = img.get_pixel(x, y);
            if p[0] < 128 || p[1] < 128 || p[2] < 128 {
                n += 1;
            }
        }
    }
    n
}

// ---------------------------------------------------------------------------------------------
// sections
// ---------------------------------------------------------------------------------------------

fn section(title: &str) {
    println!("\n==================== {title} ====================");
}

/// 1. Text extraction with geometry + text layer + coordinate conversion check
fn spike_text_layer(pdfium: &Pdfium) -> R<()> {
    section("1. TEXT EXTRACTION / TEXT LAYER (tracemonkey.pdf p1)");
    let doc = pdfium.load_pdf_from_file(&repo_root().join("fixtures/tracemonkey.pdf"), None)?;
    let page = doc.pages().get(0)?;
    println!(
        "page size: {}x{} pt, page_size rect {}",
        page.width().value,
        page.height().value,
        fmt_rect(&page.page_size())
    );
    if let Ok(cb) = page.boundaries().crop() {
        println!("crop box: {}", fmt_rect(&cb.bounds));
    }

    let t0 = Instant::now();
    let text = page.text()?;
    println!(
        "FPDFText_LoadPage: {:?}, char count = {}",
        t0.elapsed(),
        text.len()
    );

    let t0 = Instant::now();
    let chars = collect_chars(&text)?;
    println!(
        "collected {} chars with loose+tight bounds, origin, font info in {:?}",
        chars.len(),
        t0.elapsed()
    );

    println!("first 12 chars:");
    for c in chars.iter().take(12) {
        println!(
            "  #{:<3} {:?} loose={} tight={} origin=({:.2},{:.2}) size={:.2} font={:?} weight={:?} gen={}",
            c.index,
            c.ch,
            fmt_rect(&c.loose),
            c.tight.as_ref().map(fmt_rect).unwrap_or_default(),
            c.origin.0,
            c.origin.1,
            c.font_size,
            c.font_name,
            c.font_weight,
            c.generated
        );
    }
    let generated = chars.iter().filter(|c| c.generated).count();
    println!("generated (pdfium-inserted) chars: {generated}");

    // segments (FPDFText_CountRects / GetRect): pdfium's own line/style boxes
    let t0 = Instant::now();
    let segs = text.segments();
    let n = segs.len();
    println!("segments(): {} rects in {:?}; first 5:", n, t0.elapsed());
    for (i, s) in segs.iter().take(5).enumerate() {
        println!("  seg {i}: {} text={:?}", fmt_rect(&s.bounds()), s.text());
    }

    // text layer
    let t0 = Instant::now();
    let lines = build_text_layer(&chars);
    let n_words: usize = lines.iter().map(|l| l.words.len()).sum();
    println!(
        "text layer: {} lines, {} words in {:?}",
        lines.len(),
        n_words,
        t0.elapsed()
    );
    for (i, l) in lines.iter().take(4).enumerate() {
        let s: Vec<&str> = l.words.iter().map(|w| w.text.as_str()).collect();
        println!(
            "  line {i} baseline={:.2} {} :: {}",
            l.baseline,
            fmt_rect(&l.rect),
            s.join(" ")
        );
    }

    // coordinate conversion check on a known word
    let scale = 2.0f32;
    let page_box = page.page_size();
    let word = lines
        .iter()
        .flat_map(|l| l.words.iter())
        .find(|w| w.text.starts_with("Trace"))
        .expect("word starting with 'Trace' on page 1");
    println!(
        "known word {:?} chars {}..={} pdf rect {}",
        word.text,
        word.first,
        word.last,
        fmt_rect(&word.rect)
    );
    let dev = pdf_rect_to_device(&word.rect, &page_box, scale);
    println!("  -> device @{scale}x: {:?}", dev);
    let m = page_to_device_matrix(&page_box, 0, scale);
    println!(
        "  -> via matrix {:?}: {:?}",
        m,
        rect_to_device_with_matrix(&m, &word.rect)
    );
    let cfg = PdfRenderConfig::new().scale_page_by_factor(scale);
    let (px_l, px_t) = page.points_to_pixels(word.rect.left(), word.rect.top(), &cfg)?;
    let (px_r, px_b) = page.points_to_pixels(word.rect.right(), word.rect.bottom(), &cfg)?;
    println!(
        "  pdfium FPDF_PageToDevice says: left/top=({px_l},{px_t}) right/bottom=({px_r},{px_b})"
    );
    let ok = (px_l as f32 - dev.x).abs() <= 1.0
        && (px_t as f32 - dev.y).abs() <= 1.0
        && (px_r as f32 - (dev.x + dev.w)).abs() <= 1.0
        && (px_b as f32 - (dev.y + dev.h)).abs() <= 1.0;
    println!("  conversion matches pdfium within 1px: {ok}");
    let back = device_to_pdf_point(dev.x, dev.y, &page_box, scale);
    println!(
        "  round trip device->pdf: ({:.2},{:.2}) expected ({:.2},{:.2})",
        back.0,
        back.1,
        word.rect.left().value,
        word.rect.top().value
    );

    // char hit test at the word centre (pdf coords)
    let cx = PdfPoints::new((word.rect.left().value + word.rect.right().value) / 2.0);
    let cy = PdfPoints::new((word.rect.bottom().value + word.rect.top().value) / 2.0);
    if let Some(c) =
        text.chars()
            .get_char_near_point(cx, PdfPoints::new(2.0), cy, PdfPoints::new(2.0))
    {
        println!(
            "  get_char_near_point(centre) -> #{} {:?}",
            c.index(),
            c.unicode_char()
        );
    }

    // render + crop to visually confirm
    let img = render_rgb(&page, scale)?;
    let p = save_png(&img, "spike_text_p1.png")?;
    println!("rendered page to {}", p.display());
    let p = crop_png(&img, &dev, 4.0, "spike_text_word_crop.png")?;
    println!(
        "cropped word box to {} (dark px inside = {})",
        p.display(),
        dark_pixels(&img, &dev)
    );

    // full-page timing for text layer build across all pages
    let t0 = Instant::now();
    let mut total_chars = 0;
    for i in 0..doc.pages().len() {
        let pg = doc.pages().get(i)?;
        let tx = pg.text()?;
        let cs = collect_chars(&tx)?;
        total_chars += cs.len();
        let _ = build_text_layer(&cs);
    }
    println!(
        "text layer for all {} pages ({} chars): {:?}",
        doc.pages().len(),
        total_chars,
        t0.elapsed()
    );
    Ok(())
}

/// 1b. Rotated pages: verify the page->device matrix against pdfium for every page of rotation.pdf
fn spike_rotation(pdfium: &Pdfium) -> R<()> {
    section("1b. ROTATED PAGES (rotation.pdf): page->device matrix vs FPDF_PageToDevice");
    let doc = pdfium.load_pdf_from_file(&repo_root().join("fixtures/rotation.pdf"), None)?;
    let scale = 1.5f32;
    for i in 0..doc.pages().len() {
        let page = doc.pages().get(i)?;
        let rot = match page.rotation()? {
            PdfPageRenderRotation::None => 0,
            PdfPageRenderRotation::Degrees90 => 90,
            PdfPageRenderRotation::Degrees180 => 180,
            PdfPageRenderRotation::Degrees270 => 270,
        };
        let crop = page
            .boundaries()
            .crop()
            .or_else(|_| page.boundaries().media())
            .map(|b| b.bounds)
            .unwrap_or(page.page_size());
        let cfg = PdfRenderConfig::new().scale_page_by_factor(scale);
        let m = page_to_device_matrix(&crop, rot, scale);
        let text = page.text()?;
        let chars = collect_chars(&text)?;
        let first = chars.iter().find(|c| !c.generated && !c.ch.is_whitespace());
        let Some(c) = first else {
            println!("page {}: rotate={rot} no text", i + 1);
            continue;
        };
        let mine = rect_to_device_with_matrix(&m, &c.loose);
        let (ax, ay) = page.points_to_pixels(c.loose.left(), c.loose.bottom(), &cfg)?;
        let (bx, by) = page.points_to_pixels(c.loose.right(), c.loose.top(), &cfg)?;
        let (px0, px1) = (ax.min(bx) as f32, ax.max(bx) as f32);
        let (py0, py1) = (ay.min(by) as f32, ay.max(by) as f32);
        let ok = (px0 - mine.x).abs() <= 1.0
            && (py0 - mine.y).abs() <= 1.0
            && (px1 - (mine.x + mine.w)).abs() <= 1.0
            && (py1 - (mine.y + mine.h)).abs() <= 1.0;
        println!(
            "page {}: rotate={rot} width/height(display)={}x{} crop={} char {:?} loose={} -> mine={:?} pdfium=({px0},{py0})-({px1},{py1}) match={ok}",
            i + 1,
            page.width().value,
            page.height().value,
            fmt_rect(&crop),
            c.ch,
            fmt_rect(&c.loose),
            mine
        );
        if i == 0 || rot != 0 {
            let img = render_rgb(&page, scale)?;
            crop_png(
                &img,
                &mine,
                6.0,
                &format!("spike_text_rot_p{}_char.png", i + 1),
            )?;
        }
    }
    Ok(())
}

/// 2. Search
fn spike_search(pdfium: &Pdfium) -> R<()> {
    section("2. SEARCH (tracemonkey.pdf, all pages)");
    let doc = pdfium.load_pdf_from_file(&repo_root().join("fixtures/tracemonkey.pdf"), None)?;

    for (label, opts) in [
        (
            "default (case-insensitive, substring)",
            PdfSearchOptions::new(),
        ),
        ("match_case", PdfSearchOptions::new().match_case(true)),
        ("whole word", PdfSearchOptions::new().match_whole_word(true)),
    ] {
        let t0 = Instant::now();
        let mut hits = 0;
        let mut rects = 0;
        let mut first: Option<(usize, PdfRect, String)> = None;
        for i in 0..doc.pages().len() {
            let page = doc.pages().get(i)?;
            let text = page.text()?;
            let search = text.search("monkey", &opts)?;
            for result in search.iter(PdfSearchDirection::SearchForward) {
                hits += 1;
                for seg in result.iter() {
                    rects += 1;
                    if first.is_none() {
                        first = Some((i as usize, seg.bounds(), seg.text()));
                    }
                }
            }
        }
        println!(
            "search 'monkey' {label}: {hits} hits / {rects} rects in {:?}; first: {:?}",
            t0.elapsed(),
            first.map(|(p, r, t)| format!("page {} {} text={:?}", p + 1, fmt_rect(&r), t))
        );
    }

    // timing breakdown: text page load vs search only
    let t0 = Instant::now();
    let mut pages_text = Vec::new();
    for i in 0..doc.pages().len() {
        let page = doc.pages().get(i)?;
        let text = page.text()?;
        pages_text.push(text.all());
    }
    println!(
        "text().all() for all pages: {:?} ({} total chars)",
        t0.elapsed(),
        pages_text.iter().map(|s| s.chars().count()).sum::<usize>()
    );
    let t0 = Instant::now();
    let n: usize = pages_text
        .iter()
        .map(|s| s.to_lowercase().matches("monkey").count())
        .sum();
    println!(
        "naive Rust substring search over cached text: {n} hits in {:?}",
        t0.elapsed()
    );

    // char-index based search over the char array (gives selection indices directly).
    // NOTE: compare on chars, not bytes: pdfium chars include non-ASCII (e.g. the asterisk operator).
    let page = doc.pages().get(0)?;
    let text = page.text()?;
    let chars = collect_chars(&text)?;
    let hay: Vec<char> = chars.iter().flat_map(|c| c.ch.to_lowercase()).collect();
    let needle: Vec<char> = "trace".chars().collect();
    let t0 = Instant::now();
    let mut n = 0;
    let mut i = 0;
    while i + needle.len() <= hay.len() {
        if hay[i..i + needle.len()] == needle[..] {
            let (start, end) = (i, i + needle.len() - 1);
            let rect = chars[start..=end]
                .iter()
                .fold(chars[start].loose, |acc, c| union(&acc, &c.loose));
            if n < 3 {
                let s: String = chars[start..=end].iter().map(|c| c.ch).collect();
                println!(
                    "  p1 hit {:?} chars {start}..={end} rect {}",
                    s,
                    fmt_rect(&rect)
                );
            }
            n += 1;
            i = end + 1;
        } else {
            i += 1;
        }
    }
    println!(
        "  p1 char-array search 'trace': {n} hits in {:?}",
        t0.elapsed()
    );

    // mapping a pdfium search hit (rect only) back to char indices: chars_inside_rect(bounds)
    let search = text.search("Trace", &PdfSearchOptions::new().match_case(true))?;
    if let Some(first) = search.find_next() {
        if let Ok(seg) = first.first() {
            let b = seg.bounds();
            match text.chars_inside_rect(b) {
                Ok(cs) => {
                    println!(
                    "  pdfium hit rect {} -> chars_inside_rect gives {:?}..{:?} ({} chars) = {:?}",
                    fmt_rect(&b),
                    cs.first_char_index(),
                    cs.last_char_index(),
                    cs.len(),
                    cs.iter().filter_map(|c| c.unicode_char()).collect::<String>()
                )
                }
                Err(e) => println!("  chars_inside_rect failed: {e}"),
            }
        }
    }
    Ok(())
}

/// 3. Selection between two char indices
fn spike_selection(pdfium: &Pdfium) -> R<()> {
    section("3. SELECTION (tracemonkey.pdf p1)");
    let doc = pdfium.load_pdf_from_file(&repo_root().join("fixtures/tracemonkey.pdf"), None)?;
    let page = doc.pages().get(0)?;
    let text = page.text()?;
    let chars = text.chars();

    // pick a selection that spans two lines: from char 10 to char 120
    let (a, b) = (10usize, 120usize);

    // (a) per-char loose bounds grouped by line -> selection rects + selected string
    let t0 = Instant::now();
    let mut selected = String::new();
    let mut rects: Vec<PdfRect> = Vec::new();
    let mut cur: Option<(PdfRect, f32)> = None; // (rect, baseline)
    for i in a..=b {
        let c = chars.get(i)?;
        if let Some(ch) = c.unicode_char() {
            selected.push(ch);
        }
        if c.unicode_char()
            .map(|ch| ch == '\r' || ch == '\n')
            .unwrap_or(false)
        {
            continue;
        }
        let lb = c.loose_bounds()?;
        let base = c.origin()?.1.value;
        match &mut cur {
            Some((r, bl)) if (*bl - base).abs() < 0.5 * c.scaled_font_size().value => {
                *r = union(r, &lb);
            }
            _ => {
                if let Some((r, _)) = cur.take() {
                    rects.push(r);
                }
                cur = Some((lb, base));
            }
        }
    }
    if let Some((r, _)) = cur.take() {
        rects.push(r);
    }
    println!(
        "(a) per-char method in {:?}: {} rects",
        t0.elapsed(),
        rects.len()
    );
    for r in &rects {
        println!("    {}", fmt_rect(r));
    }
    println!("    selected text: {:?}", selected);

    // (b) pdfium's own rects for a char range: segments_subset(start, count)
    let t0 = Instant::now();
    let segs = text.segments_subset(a, b - a + 1);
    println!(
        "(b) segments_subset({a},{}) in {:?}: {} rects",
        b - a + 1,
        t0.elapsed(),
        segs.len()
    );
    for s in segs.iter() {
        println!("    {} text={:?}", fmt_rect(&s.bounds()), s.text());
    }

    // (c) selection by drag rectangle in page space -> chars_inside_rect
    let drag = PdfRect::new_from_values(680.0, 80.0, 720.0, 400.0);
    match text.chars_inside_rect(drag) {
        Ok(cs) => {
            let s: String = cs.iter().filter_map(|c| c.unicode_char()).collect();
            println!(
                "(c) chars_inside_rect({}) -> {} chars (first {:?} .. last {:?}): {:?}",
                fmt_rect(&drag),
                cs.len(),
                cs.first_char_index(),
                cs.last_char_index(),
                s.chars().take(80).collect::<String>()
            );
        }
        Err(e) => println!("(c) chars_inside_rect error: {e:?}"),
    }
    println!(
        "    inside_rect(drag) = {:?}",
        text.inside_rect(drag).chars().take(80).collect::<String>()
    );
    Ok(())
}

#[derive(Debug, Clone)]
struct TextObjInfo {
    index: usize,
    text: String,
    rect: PdfRect,
    font_name: String,
    font_family: String,
    embedded: Option<bool>,
    unscaled_size: f32,
    scaled_size: f32,
    fill: PdfColor,
    matrix: PdfMatrix,
    render_mode: PdfPageTextRenderMode,
}

fn obj_key(m: &PdfMatrix, size: f32, text: &str) -> (i64, i64, i64, i64, i64, i64, i64, String) {
    let q = |v: f32| (v * 100.0).round() as i64;
    (
        q(m.a()),
        q(m.b()),
        q(m.c()),
        q(m.d()),
        q(m.e()),
        q(m.f()),
        q(size),
        text.to_owned(),
    )
}

fn scan_text_objects(page: &PdfPage, text: &PdfPageText, verbose: bool) -> R<Vec<TextObjInfo>> {
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    let mut out = Vec::new();
    let t0 = Instant::now();
    for (i, obj) in page.objects().iter().enumerate() {
        *counts
            .entry(format!("{:?}", obj.object_type()))
            .or_default() += 1;
        if let Some(t) = obj.as_text_object() {
            let font = t.font();
            out.push(TextObjInfo {
                index: i,
                text: text.for_object(t),
                rect: t.bounds()?.to_rect(),
                font_name: font.name(),
                font_family: font.family(),
                embedded: font.is_embedded().ok(),
                unscaled_size: t.unscaled_font_size().value,
                scaled_size: t.scaled_font_size().value,
                fill: t.fill_color()?,
                matrix: t.matrix()?,
                render_mode: t.render_mode(),
            });
        }
    }
    println!(
        "scanned {} objects ({:?}) in {:?}; {} text objects",
        page.objects().len(),
        counts,
        t0.elapsed(),
        out.len()
    );
    if verbose {
        for o in out.iter().take(8) {
            println!(
                "  obj#{:<3} {:?} rect={} font={:?}/{:?} emb={:?} size={:.2}/{:.2} fill={:?} mode={:?} m={}",
                o.index,
                o.text.chars().take(40).collect::<String>(),
                fmt_rect(&o.rect),
                o.font_name,
                o.font_family,
                o.embedded,
                o.unscaled_size,
                o.scaled_size,
                o.fill,
                o.render_mode,
                fmt_matrix(&o.matrix)
            );
        }
    }
    Ok(out)
}

/// Map every char index -> owning text object index.
/// (A) slow: PdfPageTextObject::chars(&text) per object (O(objects x chars) FFI calls).
/// (B) fast: PdfPageTextChar::text_object() per char + matrix/size/text key lookup.
fn map_chars_to_objects(text: &PdfPageText, objs: &[TextObjInfo]) -> R<Vec<Option<usize>>> {
    let page_objs = text.chars().len();
    // (B)
    let t0 = Instant::now();
    let mut key_to_idx: HashMap<_, usize> = HashMap::new();
    for o in objs {
        key_to_idx
            .entry(obj_key(&o.matrix, o.unscaled_size, &o.text))
            .or_insert(o.index);
    }
    let mut map: Vec<Option<usize>> = vec![None; page_objs];
    let mut misses = 0;
    for c in text.chars().iter() {
        if let Ok(t) = c.text_object() {
            // NOTE: PdfPageTextObject::text() loads a fresh FPDF_TEXTPAGE on every call (~0.45 ms);
            // always go through the already-open PdfPageText with for_object() in loops.
            let key = obj_key(
                &t.matrix()?,
                t.unscaled_font_size().value,
                &text.for_object(&t),
            );
            match key_to_idx.get(&key) {
                Some(&i) => map[c.index()] = Some(i),
                None => misses += 1,
            }
        }
    }
    let mapped = map.iter().filter(|m| m.is_some()).count();
    println!(
        "char->object map (B: char.text_object() + key): {mapped}/{page_objs} chars mapped, {misses} key misses, in {:?}",
        t0.elapsed()
    );
    Ok(map)
}

/// Group text objects into paragraph-like blocks: sort by top (desc) then left, merge an object
/// into the current block when its baseline is within 1.6 em below the block's last line and it
/// overlaps the block horizontally.
fn group_blocks(objs: &[TextObjInfo]) -> Vec<Vec<usize>> {
    let mut order: Vec<usize> = (0..objs.len()).collect();
    order.sort_by(|&a, &b| {
        let (ra, rb) = (&objs[a].rect, &objs[b].rect);
        rb.top()
            .value
            .partial_cmp(&ra.top().value)
            .unwrap()
            .then(ra.left().value.partial_cmp(&rb.left().value).unwrap())
    });
    let mut blocks: Vec<(Vec<usize>, PdfRect, f32)> = Vec::new(); // (members, rect, last baseline)
    for i in order {
        let o = &objs[i];
        let em = o.scaled_size.max(1.0);
        let base = o.matrix.f();
        let target = blocks.iter_mut().find(|(_, r, last_base)| {
            let x_overlap =
                o.rect.left().value < r.right().value && o.rect.right().value > r.left().value;
            let dy = *last_base - base;
            x_overlap && dy >= -0.5 * em && dy <= 1.6 * em
        });
        match target {
            Some((members, r, last_base)) => {
                members.push(i);
                *r = union(r, &o.rect);
                if base < *last_base {
                    *last_base = base;
                }
            }
            None => blocks.push((vec![i], o.rect, base)),
        }
    }
    blocks.into_iter().map(|(m, _, _)| m).collect()
}

/// Glyph coverage probe: pdfium's FPDFFont_GetGlyphWidth/GetGlyphPath take a *Unicode code point*
/// and map it through the font's encoding; a missing glyph yields width 0 / null path.
fn probe_glyphs(label: &str, font: &PdfFont, size: PdfPoints, sample: &str) {
    let glyphs = font.glyphs();
    let n = glyphs.len();
    let mut parts = Vec::new();
    for ch in sample.chars() {
        // PdfFontGlyphIndex is u16: code points above U+FFFF cannot be probed through this API.
        let s = match u16::try_from(ch as u32)
            .map_err(|_| ())
            .and_then(|cp| glyphs.get(cp).map_err(|_| ()))
        {
            Ok(g) => {
                let w = g.width_at_font_size(size).value;
                let segs = g.segments_at_font_size(size).map(|p| p.len()).ok();
                format!("{ch:?}:w={w:.2},segs={segs:?}")
            }
            Err(_) => format!("{ch:?}:idx>=len"),
        };
        parts.push(s);
    }
    println!(
        "glyph probe {label} ({:?}/{:?} embedded={:?}) glyphs.len()={n}: {}",
        font.name(),
        font.family(),
        font.is_embedded().ok(),
        parts.join(" ")
    );
}

/// 4. Page object model + editing on a copy of tracemonkey
fn spike_editing(pdfium: &Pdfium) -> R<()> {
    section("4. PAGE OBJECT MODEL + EDITING (tracemonkey.pdf p1 -> spike_text_edited.pdf)");
    let src = repo_root().join("fixtures/tracemonkey.pdf");
    let mut doc = pdfium.load_pdf_from_file(&src, None)?;

    // --- scan
    let (
        target_set_text,
        target_replace,
        target_translate,
        target_scale,
        target_fontsize,
        target_delete,
        n_objs_before,
        n_text_before,
        deleted_text,
        fontsize_text,
    );
    {
        let page = doc.pages().get(0)?;
        let text = page.text()?;
        let objs = scan_text_objects(&page, &text, true)?;
        let _map = map_chars_to_objects(&text, &objs)?;
        // (A) the slow way, for timing comparison
        let t0 = Instant::now();
        let mut n = 0usize;
        for obj in page.objects().iter() {
            if let Some(t) = obj.as_text_object() {
                n += t.chars(&text).map(|c| c.len()).unwrap_or(0);
            }
        }
        println!(
            "char->object map (A: text_object.chars(&text) per object): {n} chars in {:?}",
            t0.elapsed()
        );
        let blocks = group_blocks(&objs);
        println!("paragraph-like blocks: {}", blocks.len());
        for (bi, b) in blocks.iter().take(5).enumerate() {
            let joined: String = b
                .iter()
                .map(|&i| objs[i].text.as_str())
                .collect::<Vec<_>>()
                .join("|");
            println!(
                "  block {bi}: {} objs :: {:?}",
                b.len(),
                joined.chars().take(90).collect::<String>()
            );
        }
        // pick targets: first object whose text contains "Trace" (title), then some body objects
        target_set_text = objs
            .iter()
            .find(|o| o.text.contains("Trace"))
            .map(|o| o.index)
            .expect("title object");
        target_replace = objs
            .iter()
            .find(|o| o.text == "Languages")
            .map(|o| o.index)
            .expect("Languages object");
        let body: Vec<&TextObjInfo> = objs.iter().filter(|o| o.text.len() > 30).collect();
        target_translate = body[1].index;
        target_scale = body[2].index;
        target_fontsize = body[3].index;
        fontsize_text = body[3].text.clone();
        target_delete = body[4].index;
        deleted_text = body[4].text.clone();
        n_objs_before = page.objects().len();
        n_text_before = objs.len();
        println!(
            "targets: set_text=#{target_set_text} replace=#{target_replace} translate=#{target_translate} scale=#{target_scale} fontsize=#{target_fontsize} delete=#{target_delete} ({:?})",
            deleted_text.chars().take(40).collect::<String>()
        );
        // glyph coverage of the title's subset font vs a built-in font
        let title = page.objects().get(target_set_text)?;
        let tf = title.as_text_object().unwrap().font();
        probe_glyphs("title subset font", &tf, PdfPoints::new(10.0), " SeXQ한");
    }

    // --- fonts (need &mut doc, so do it before holding a page)
    let helv = doc.fonts_mut().helvetica();
    let helv_bold = doc.fonts_mut().helvetica_bold();
    let times_bold = doc.fonts_mut().times_bold();
    let t0 = Instant::now();
    let gothic = doc
        .fonts_mut()
        .load_true_type_from_file("/System/Library/Fonts/Supplemental/AppleGothic.ttf", true);
    println!(
        "load_true_type_from_file(AppleGothic.ttf, cid=true): {:?} in {:?}",
        gothic.as_ref().map(|_| "ok").map_err(|e| e.to_string()),
        t0.elapsed()
    );
    if let Some(f) = doc.fonts().get(helv) {
        probe_glyphs("built-in Helvetica", f, PdfPoints::new(10.0), " SeXQ한");
    }
    if let Ok(g) = gothic {
        if let Some(f) = doc.fonts().get(g) {
            probe_glyphs("AppleGothic", f, PdfPoints::new(10.0), " SeXQ한");
        }
    }

    // --- edit
    let mut page = doc.pages().get(0)?;
    page.set_content_regeneration_strategy(PdfPageContentRegenerationStrategy::Manual);

    // 4a set_text on existing object (subset font!)
    {
        let mut obj = page.objects().get(target_set_text)?;
        let t = obj.as_text_object_mut().unwrap();
        println!("set_text: before {:?}", t.text());
        t.set_text("SeePDF edited title")?;
        println!("set_text: after  {:?} (FPDFText_SetText ok)", t.text());
        // NOTE: set_text does NOT trigger auto-regeneration; we regenerate manually below.
        // Inspect the new chars through a fresh text page (built from in-memory objects):
        let text = page.text()?;
        let cs = t.chars(&text)?;
        let widths: Vec<String> = cs
            .iter()
            .map(|c| {
                format!(
                    "{:?}={:.1}",
                    c.unicode_char().unwrap_or('?'),
                    c.loose_bounds().map(|b| b.width().value).unwrap_or(-1.0)
                )
            })
            .collect();
        println!(
            "  chars after set_text (loose widths): {}",
            widths.join(" ")
        );
    }
    // 4a' replace-object model: new object with a substitute built-in font at the old matrix
    {
        let (old_m, size, fill, old_bounds) = {
            let old = page.objects().get(target_replace)?;
            let old_t = old.as_text_object().unwrap();
            (
                old.matrix()?,
                old_t.unscaled_font_size(),
                old.fill_color()?,
                old.bounds()?,
            )
        };
        let mut new_t =
            PdfPageTextObject::new(&doc, "Languages (replaced, Times-Bold)", times_bold, size)?;
        new_t.apply_matrix(old_m)?;
        new_t.set_fill_color(fill)?;
        let new_obj = page.objects_mut().add_text_object(new_t)?;
        println!(
            "replace model: old {} m={} size={:.2} -> new {} (added at index {})",
            fmt_quad(&old_bounds),
            fmt_matrix(&old_m),
            size.value,
            fmt_quad(&new_obj.bounds()?),
            page.objects().len() - 1
        );
        drop(new_obj);
        // remove the old one (index unchanged since we appended)
        page.objects_mut().remove_object_at_index(target_replace)?;
    }
    // indices shift by -1 after the removal above for everything after target_replace
    let shift = |i: usize| if i > target_replace { i - 1 } else { i };
    // 4b translate
    {
        let mut obj = page.objects().get(shift(target_translate))?;
        let before = obj.bounds()?.to_rect();
        obj.translate(PdfPoints::new(30.0), PdfPoints::new(-5.0))?;
        let after = obj.bounds()?.to_rect();
        println!(
            "translate(+30,-5): {} -> {}",
            fmt_rect(&before),
            fmt_rect(&after)
        );
    }
    // 4c scale via matrix (about page origin; must re-anchor)
    {
        let mut obj = page.objects().get(shift(target_scale))?;
        let before = obj.bounds()?.to_rect();
        let (x, y) = obj.get_translation();
        obj.translate(-x, -y)?;
        obj.scale(1.5, 1.5)?;
        obj.translate(x, y)?;
        let after = obj.bounds()?.to_rect();
        let t = obj.as_text_object().unwrap();
        println!(
            "scale(1.5) about own origin: {} -> {} ; unscaled_font_size={:.2} scaled_font_size={:.2}",
            fmt_rect(&before),
            fmt_rect(&after),
            t.unscaled_font_size().value,
            t.scaled_font_size().value
        );
    }
    // 4d font size via FPDFTextObj_SetFontSize
    {
        let mut obj = page.objects().get(shift(target_fontsize))?;
        let before = obj.bounds()?.to_rect();
        let (r, unscaled) = {
            let t = obj.as_text_object_mut().unwrap();
            let r = t.set_unscaled_font_size(PdfPoints::new(14.0));
            (r, t.unscaled_font_size().value)
        };
        let after = obj.bounds()?.to_rect();
        println!(
            "set_unscaled_font_size(14): {:?}; unscaled now {:.2}; bounds {} -> {} (bounds cached until regen/reload)",
            r.map_err(|e| e.to_string()),
            unscaled,
            fmt_rect(&before),
            fmt_rect(&after)
        );
    }
    // 4e delete
    {
        let idx = shift(target_delete);
        let removed = page.objects_mut().remove_object_at_index(idx)?;
        println!(
            "remove_object_at_index({idx}) -> removed text() (detached => empty) {:?}; objects now {}",
            removed.as_text_object().map(|t| t.text()),
            page.objects().len()
        );
        // `removed` is dropped here -> FPDFPageObj_Destroy (ownership is now unowned)
    }
    // 4f add Helvetica text
    {
        let mut obj = page.objects_mut().create_text_object(
            PdfPoints::new(72.0),
            PdfPoints::new(40.0),
            "Hello from SeePDF (Helvetica 14pt, red)",
            helv,
            PdfPoints::new(14.0),
        )?;
        obj.set_fill_color(PdfColor::new(200, 0, 0, 255))?;
        println!(
            "create_text_object(Helvetica): bounds {}",
            fmt_quad(&obj.bounds()?)
        );
        let mut obj2 = page.objects_mut().create_text_object(
            PdfPoints::new(72.0),
            PdfPoints::new(22.0),
            "Bold Helvetica 10pt",
            helv_bold,
            PdfPoints::new(10.0),
        )?;
        obj2.set_fill_color(PdfColor::new(0, 0, 160, 255))?;
    }
    // 4g add TrueType Korean text
    if let Ok(gothic) = gothic {
        let mut obj = page.objects_mut().create_text_object(
            PdfPoints::new(340.0),
            PdfPoints::new(40.0),
            "한글 테스트 AppleGothic",
            gothic,
            PdfPoints::new(16.0),
        )?;
        obj.set_fill_color(PdfColor::new(0, 120, 0, 255))?;
        println!(
            "create_text_object(AppleGothic, 한글 테스트): bounds {}",
            fmt_quad(&obj.bounds()?)
        );
    }

    // --- regenerate + save
    let t0 = Instant::now();
    page.regenerate_content()?;
    println!(
        "regenerate_content (FPDFPage_GenerateContent): {:?}",
        t0.elapsed()
    );
    drop(page);

    let out = out_dir().join("spike_text_edited.pdf");
    let t0 = Instant::now();
    doc.save_to_file(&out)?;
    let save_t = t0.elapsed();
    let src_len = std::fs::metadata(&src)?.len();
    let out_len = std::fs::metadata(&out)?.len();
    println!(
        "save_to_file: {:?}; size {} -> {} bytes (delta {:+})",
        save_t,
        src_len,
        out_len,
        out_len as i64 - src_len as i64
    );
    drop(doc);

    // --- reopen and verify
    let doc2 = pdfium.load_pdf_from_file(&out, None)?;
    let page2 = doc2.pages().get(0)?;
    let text2 = page2.text()?;
    let all = text2.all();
    println!(
        "reopened: {} objects (was {}), text objects: {} (was {})",
        page2.objects().len(),
        n_objs_before,
        page2
            .objects()
            .iter()
            .filter(|o| o.as_text_object().is_some())
            .count(),
        n_text_before
    );
    for needle in [
        "SeePDF edited title",
        "Languages (replaced",
        "Hello from SeePDF",
        "Bold Helvetica",
        "한글 테스트",
        "한글\t테스트",
    ] {
        println!("  extract contains {:?}: {}", needle, all.contains(needle));
    }
    println!(
        "  deleted object text still present: {}",
        all.contains(deleted_text.trim())
    );
    let objs2 = scan_text_objects(&page2, &text2, false)?;
    for o in objs2.iter().filter(|o| {
        o.text.contains("SeePDF")
            || o.text.contains("한글")
            || o.text.contains("Helvetica")
            || o.text.contains("replaced")
            || o.text.starts_with(fontsize_text.trim_end())
    }) {
        println!(
            "  obj#{} {:?} font={:?}/{:?} embedded={:?} size={:.1}/{:.1} fill={:?} rect={}",
            o.index,
            o.text.chars().take(40).collect::<String>(),
            o.font_name,
            o.font_family,
            o.embedded,
            o.unscaled_size,
            o.scaled_size,
            o.fill,
            fmt_rect(&o.rect)
        );
    }
    let img = render_rgb(&page2, 2.0)?;
    let p = save_png(&img, "spike_text_edited_p1.png")?;
    println!("rendered edited page to {}", p.display());
    let pb = page2.page_size();
    let bottom = pdf_rect_to_device(&PdfRect::new_from_values(10.0, 60.0, 60.0, 560.0), &pb, 2.0);
    crop_png(&img, &bottom, 0.0, "spike_text_edited_bottom.png")?;
    let top = pdf_rect_to_device(
        &PdfRect::new_from_values(670.0, 60.0, 720.0, 560.0),
        &pb,
        2.0,
    );
    crop_png(&img, &top, 0.0, "spike_text_edited_top.png")?;
    if let Some(o) = objs2
        .iter()
        .find(|o| o.text.starts_with(fontsize_text.trim_end()))
    {
        let r = pdf_rect_to_device(&o.rect, &pb, 2.0);
        crop_png(&img, &r, 6.0, "spike_text_edited_fontsize.png")?;
    }
    println!("crops: spike_text_edited_top.png (set_text + replace), spike_text_edited_bottom.png (new objects), spike_text_edited_fontsize.png");
    Ok(())
}

/// 4'. Font loading matrix on a fresh document (which font files load, do they render Hangul,
/// what does embedding cost in bytes)
fn spike_fonts(pdfium: &Pdfium) -> R<()> {
    section("4b. FONT LOADING MATRIX (new doc per font)");
    let candidates: [(&str, &str, bool); 5] = [
        (
            "Arial.ttf",
            "/System/Library/Fonts/Supplemental/Arial.ttf",
            false,
        ),
        (
            "Arial Unicode.ttf",
            "/System/Library/Fonts/Supplemental/Arial Unicode.ttf",
            true,
        ),
        (
            "AppleGothic.ttf",
            "/System/Library/Fonts/Supplemental/AppleGothic.ttf",
            true,
        ),
        (
            "AppleSDGothicNeo.ttc",
            "/System/Library/Fonts/AppleSDGothicNeo.ttc",
            true,
        ),
        (
            "Arial.ttf(cid=true)",
            "/System/Library/Fonts/Supplemental/Arial.ttf",
            true,
        ),
    ];
    for (label, path, cid) in candidates {
        let file_len = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        let mut doc = pdfium.create_new_pdf()?;
        let t0 = Instant::now();
        let tok = doc.fonts_mut().load_true_type_from_file(path, cid);
        let load_t = t0.elapsed();
        let tok = match tok {
            Ok(t) => t,
            Err(e) => {
                println!("{label:<22} file={file_len:>9}B load FAILED: {e} ({load_t:?})");
                continue;
            }
        };
        let (name, family) = doc
            .fonts()
            .get(tok)
            .map(|f| (f.name(), f.family()))
            .unwrap_or_default();
        let mut page = doc
            .pages_mut()
            .create_page_at_start(PdfPagePaperSize::a4())?;
        page.set_content_regeneration_strategy(PdfPageContentRegenerationStrategy::Manual);
        let res = page.objects_mut().create_text_object(
            PdfPoints::new(50.0),
            PdfPoints::new(700.0),
            "한글 테스트 Hangul ABC",
            tok,
            PdfPoints::new(24.0),
        );
        let bounds = match &res {
            Ok(o) => o.bounds().map(|b| fmt_quad(&b)).unwrap_or_default(),
            Err(e) => format!("create_text_object failed: {e}"),
        };
        page.regenerate_content()?;
        drop(page);
        let safe = label.replace(['.', ' ', '(', ')', '='], "_");
        let out = out_dir().join(format!("spike_text_font_{safe}.pdf"));
        let t0 = Instant::now();
        let bytes = doc.save_to_bytes()?;
        let save_t = t0.elapsed();
        std::fs::write(&out, &bytes)?;
        drop(doc);
        // reopen: extract + render + count dark pixels in text area
        let doc2 = pdfium.load_pdf_from_file(&out, None)?;
        let page2 = doc2.pages().get(0)?;
        let extracted = page2.text()?.all();
        let img = render_rgb(&page2, 2.0)?;
        let obj_rect = page2
            .objects()
            .iter()
            .find_map(|o| o.as_text_object().and_then(|t| t.bounds().ok()))
            .map(|q| q.to_rect())
            .unwrap_or(PdfRect::ZERO);
        let dev = pdf_rect_to_device(&obj_rect, &page2.page_size(), 2.0);
        let dark = dark_pixels(&img, &dev);
        crop_png(&img, &dev, 4.0, &format!("spike_text_font_{safe}.png"))?;
        let emb = page2
            .objects()
            .iter()
            .find_map(|o| o.as_text_object().and_then(|t| t.font().is_embedded().ok()));
        println!(
            "{label:<22} file={file_len:>9}B load={load_t:?} name={name:?} family={family:?} bounds={bounds} pdf={}B save={save_t:?} embedded={emb:?} extracted={:?}",
            bytes.len(),
            extracted,
        );
        println!(
            "{:<22} dark_px={dark} hangul_extracted={}",
            "",
            extracted.contains("한글 테스트")
        );
    }
    Ok(())
}

/// 4c. Text inside Form XObjects: can we read it, and does editing it persist?
fn spike_xobject(pdfium: &Pdfium) -> R<()> {
    section("4c. TEXT INSIDE FORM XOBJECTS (scan fixtures, try set_text + translate)");
    let fixtures = [
        "160F-2019.pdf",
        "annotation-highlight.pdf",
        "annotation-line.pdf",
        "annotation-freetext.pdf",
        "TAMReview.pdf",
        "alphatrans.pdf",
        "rotation.pdf",
        "issue12120_reduced.pdf",
        "tracemonkey.pdf",
    ];
    let mut candidate: Option<(String, PdfPageIndex, usize, usize)> = None; // file, page, form idx, inner text idx
    for f in fixtures {
        let path = repo_root().join("fixtures").join(f);
        let doc = match pdfium.load_pdf_from_file(&path, None) {
            Ok(d) => d,
            Err(e) => {
                println!("{f}: load failed {e}");
                continue;
            }
        };
        let mut forms = 0;
        let mut forms_with_text = 0;
        let mut inner_text = 0;
        for pi in 0..doc.pages().len() {
            let page = doc.pages().get(pi)?;
            for (oi, obj) in page.objects().iter().enumerate() {
                if let Some(form) = obj.as_x_object_form_object() {
                    forms += 1;
                    let mut has_text = false;
                    for ii in 0..form.len() {
                        if let Ok(inner) = form.get(ii) {
                            if inner.as_text_object().is_some() {
                                inner_text += 1;
                                has_text = true;
                                if candidate.is_none() {
                                    candidate = Some((f.to_string(), pi, oi, ii));
                                }
                            }
                        }
                    }
                    if has_text {
                        forms_with_text += 1;
                    }
                }
            }
        }
        println!(
            "{f}: pages={} form xobjects={forms} (with text: {forms_with_text}, inner text objs={inner_text})",
            doc.pages().len()
        );
    }
    let Some((f, pi, oi, ii)) = candidate else {
        println!("no fixture with text inside a form xobject; skipping edit test");
        return Ok(());
    };
    println!(
        "editing {f} page {} form obj #{oi} inner text #{ii}",
        pi + 1
    );
    let doc = pdfium.load_pdf_from_file(&repo_root().join("fixtures").join(&f), None)?;
    let mut page = doc.pages().get(pi)?;
    page.set_content_regeneration_strategy(PdfPageContentRegenerationStrategy::Manual);
    let (before_text, before_bounds);
    {
        let obj = page.objects().get(oi)?;
        let form = obj.as_x_object_form_object().unwrap();
        let mut inner = form.get(ii)?;
        let text_page = page.text()?;
        before_bounds = inner.bounds()?.to_rect();
        let t = inner.as_text_object_mut().unwrap();
        before_text = text_page.for_object(t);
        println!(
            "  inner text before: {:?} bounds {} font {:?}",
            before_text,
            fmt_rect(&before_bounds),
            t.font().name()
        );
        let r1 = t.set_text("XOBJ EDITED");
        let r2 = inner.translate(PdfPoints::new(20.0), PdfPoints::new(0.0));
        println!(
            "  set_text -> {:?}, translate -> {:?}, in-memory text now {:?}, bounds {}",
            r1.map_err(|e| e.to_string()),
            r2.map_err(|e| e.to_string()),
            inner.as_text_object().unwrap().text(),
            fmt_rect(&inner.bounds()?.to_rect())
        );
    }
    let r = page.regenerate_content();
    println!("  regenerate_content -> {:?}", r.map_err(|e| e.to_string()));
    drop(page);
    let out = out_dir().join("spike_text_xobject_edited.pdf");
    doc.save_to_file(&out)?;
    drop(doc);
    let doc2 = pdfium.load_pdf_from_file(&out, None)?;
    let page2 = doc2.pages().get(pi)?;
    let text2 = page2.text()?;
    let obj = page2.objects().get(oi)?;
    match obj.as_x_object_form_object() {
        Some(form) => {
            let inner = form.get(ii)?;
            let t = inner.as_text_object().unwrap();
            println!(
                "  after reopen: inner text {:?} bounds {} ; edit persisted: {}",
                text2.for_object(t),
                fmt_rect(&inner.bounds()?.to_rect()),
                text2.for_object(t).contains("XOBJ EDITED")
            );
        }
        None => println!(
            "  after reopen: object #{oi} is no longer a form xobject ({:?})",
            obj.object_type()
        ),
    }
    println!(
        "  page text contains 'XOBJ EDITED': {}",
        text2.all().contains("XOBJ EDITED")
    );
    Ok(())
}

/// 5. Invisible OCR text layer
fn spike_ocr_layer(pdfium: &Pdfium) -> R<()> {
    section("5. INVISIBLE OCR TEXT LAYER (new doc -> spike_text_ocr.pdf)");
    let mut doc = pdfium.create_new_pdf()?;
    let helv = doc.fonts_mut().helvetica();
    let mut page = doc
        .pages_mut()
        .create_page_at_start(PdfPagePaperSize::a4())?;
    page.set_content_regeneration_strategy(PdfPageContentRegenerationStrategy::Manual);

    // a light-grey "scanned image" placeholder rect
    let img_rect = PdfRect::new_from_values(600.0, 50.0, 700.0, 500.0);
    page.objects_mut().create_path_object_rect(
        img_rect,
        None,
        None,
        Some(PdfColor::new(230, 230, 230, 255)),
    )?;

    // OCR "word boxes" we want to cover exactly: (text, rect)
    let words: [(&str, PdfRect); 3] = [
        (
            "Invisible",
            PdfRect::new_from_values(660.0, 60.0, 680.0, 160.0),
        ),
        ("OCR", PdfRect::new_from_values(660.0, 170.0, 680.0, 220.0)),
        (
            "layer42",
            PdfRect::new_from_values(620.0, 60.0, 640.0, 200.0),
        ),
    ];
    let mut visible_ref = None;
    for (i, (word, target)) in words.iter().enumerate() {
        // create at origin, measure natural width at font size = box height, then scale x to fit
        let size = target.height();
        let mut obj = page.objects_mut().create_text_object(
            PdfPoints::ZERO,
            PdfPoints::ZERO,
            *word,
            helv,
            size,
        )?;
        let natural = obj.bounds()?.to_rect();
        let sx = target.width().value / natural.width().value.max(0.01);
        obj.scale(sx, 1.0)?;
        // baseline: put the glyph box bottom at target bottom (bounds are relative to origin 0,0)
        obj.translate(
            target.left() - PdfPoints::new(natural.left().value * sx),
            target.bottom() - natural.bottom(),
        )?;
        let t = obj.as_text_object_mut().unwrap();
        t.set_render_mode(PdfPageTextRenderMode::Invisible)?;
        let fitted = obj.bounds()?.to_rect();
        println!(
            "word {i} {:?}: target {} natural {} sx={:.3} fitted {} mode={:?} visible={}",
            word,
            fmt_rect(target),
            fmt_rect(&natural),
            sx,
            fmt_rect(&fitted),
            obj.as_text_object().unwrap().render_mode(),
            obj.as_text_object().unwrap().is_visible()
        );
        if i == 0 {
            visible_ref = Some(fitted);
        }
    }
    // a visible control word next to them
    page.objects_mut().create_text_object(
        PdfPoints::new(60.0),
        PdfPoints::new(560.0),
        "VISIBLE control",
        helv,
        PdfPoints::new(20.0),
    )?;
    page.regenerate_content()?;
    drop(page);
    let out = out_dir().join("spike_text_ocr.pdf");
    doc.save_to_file(&out)?;
    println!(
        "saved {} ({} bytes)",
        out.display(),
        std::fs::metadata(&out)?.len()
    );
    drop(doc);

    // reopen: extractable + searchable + not rendered
    let doc2 = pdfium.load_pdf_from_file(&out, None)?;
    let page2 = doc2.pages().get(0)?;
    let text2 = page2.text()?;
    let all = text2.all();
    println!("reopened text().all() = {:?}", all);
    let search = text2.search("ocr", &PdfSearchOptions::new())?;
    let hits: Vec<String> = search
        .iter(PdfSearchDirection::SearchForward)
        .flat_map(|r| r.iter().map(|s| fmt_rect(&s.bounds())).collect::<Vec<_>>())
        .collect();
    println!("search 'ocr' (case-insensitive) hits: {:?}", hits);
    for c in text2.chars().iter().take(3) {
        println!(
            "  char #{} {:?} render_mode={:?} loose={}",
            c.index(),
            c.unicode_char(),
            c.render_mode().map_err(|e| e.to_string()),
            fmt_rect(&c.loose_bounds()?)
        );
    }
    let modes: Vec<(String, PdfPageTextRenderMode)> = page2
        .objects()
        .iter()
        .filter_map(|o| o.as_text_object().map(|t| (t.text(), t.render_mode())))
        .collect();
    println!("text objects after reopen: {:?}", modes);

    let img = render_rgb(&page2, 2.0)?;
    let p = save_png(&img, "spike_text_ocr.png")?;
    let inv = pdf_rect_to_device(&visible_ref.unwrap(), &page2.page_size(), 2.0);
    let ctl = pdf_rect_to_device(
        &PdfRect::new_from_values(555.0, 60.0, 580.0, 220.0),
        &page2.page_size(),
        2.0,
    );
    println!(
        "render {}: dark px in invisible word box = {} ; in VISIBLE control box = {}",
        p.display(),
        dark_pixels(&img, &inv),
        dark_pixels(&img, &ctl)
    );
    let crop = image::imageops::crop_imm(&img, 80, 160, 900, 400).to_image();
    save_png(&crop, "spike_text_ocr_crop.png")?;
    Ok(())
}

/// 6. Save behaviour
fn spike_save(pdfium: &Pdfium) -> R<()> {
    section("6. SAVE (tracemonkey.pdf round trip)");
    let src = repo_root().join("fixtures/tracemonkey.pdf");
    let src_len = std::fs::metadata(&src)?.len();
    let doc = pdfium.load_pdf_from_file(&src, None)?;
    println!("version: {:?}", doc.version());
    let t0 = Instant::now();
    let bytes = doc.save_to_bytes()?;
    println!(
        "save_to_bytes (unmodified, FPDF_SaveAsCopy flags=0): {} -> {} bytes ({:+}) in {:?}",
        src_len,
        bytes.len(),
        bytes.len() as i64 - src_len as i64,
        t0.elapsed()
    );
    let head = String::from_utf8_lossy(&bytes[..16]).to_string();
    let tail = String::from_utf8_lossy(&bytes[bytes.len() - 64..]).to_string();
    println!("  header {:?} tail {:?}", head.trim(), tail.trim());
    println!(
        "  incremental update?  startxref count in output = {} (1 => full rewrite)",
        bytes.windows(9).filter(|w| w == b"startxref").count()
    );
    let out = out_dir().join("spike_text_resaved.pdf");
    let t0 = Instant::now();
    doc.save_to_file(&out)?;
    println!("save_to_file: {:?} -> {}", t0.elapsed(), out.display());
    // touch one page and save again: does the size change meaningfully?
    drop(doc);
    let mut doc = pdfium.load_pdf_from_file(&src, None)?;
    let helv = doc.fonts_mut().helvetica();
    {
        let mut page = doc.pages().get(3)?;
        page.set_content_regeneration_strategy(PdfPageContentRegenerationStrategy::Manual);
        page.objects_mut().create_text_object(
            PdfPoints::new(72.0),
            PdfPoints::new(30.0),
            "page 4 touched",
            helv,
            PdfPoints::new(9.0),
        )?;
        page.regenerate_content()?;
    }
    let t0 = Instant::now();
    let bytes2 = doc.save_to_bytes()?;
    println!(
        "save after editing 1 page: {} bytes ({:+} vs unmodified resave) in {:?}",
        bytes2.len(),
        bytes2.len() as i64 - bytes.len() as i64,
        t0.elapsed()
    );
    // regenerate content on an untouched page: does it change the stream?
    let doc3 = pdfium.load_pdf_from_file(&src, None)?;
    {
        let mut page = doc3.pages().get(0)?;
        page.regenerate_content()?;
    }
    let bytes3 = doc3.save_to_bytes()?;
    println!(
        "save after regenerate_content() on untouched page 1: {} bytes ({:+} vs unmodified resave)",
        bytes3.len(),
        bytes3.len() as i64 - bytes.len() as i64
    );
    // set_text without regenerate_content: is it lost?
    let doc4 = pdfium.load_pdf_from_file(&src, None)?;
    {
        let mut page = doc4.pages().get(0)?;
        page.set_content_regeneration_strategy(PdfPageContentRegenerationStrategy::Manual);
        let mut obj = page.objects().get(0)?;
        obj.as_text_object_mut().unwrap().set_text("LOST?")?;
        // no regenerate_content(), Manual strategy => nothing written on drop
    }
    let bytes4 = doc4.save_to_bytes()?;
    let doc4b = pdfium.load_pdf_from_byte_vec(bytes4, None)?;
    let p = doc4b.pages().get(0)?;
    println!(
        "set_text without regenerate_content (Manual): persisted = {}",
        p.text()?.all().contains("LOST?")
    );
    // same but with the default AutomaticOnEveryChange strategy (set_text alone does not trigger it)
    let doc5 = pdfium.load_pdf_from_file(&src, None)?;
    {
        let page = doc5.pages().get(0)?;
        let mut obj = page.objects().get(0)?;
        obj.as_text_object_mut().unwrap().set_text("LOST2?")?;
    }
    let bytes5 = doc5.save_to_bytes()?;
    let doc5b = pdfium.load_pdf_from_byte_vec(bytes5, None)?;
    let p = doc5b.pages().get(0)?;
    println!(
        "set_text without regenerate_content (AutomaticOnEveryChange default): persisted = {}",
        p.text()?.all().contains("LOST2?")
    );
    Ok(())
}

fn main() -> R<()> {
    let lib = pdfium_lib_path();
    println!("binding pdfium: {}", lib.display());
    let t0 = Instant::now();
    let pdfium = Pdfium::new(Pdfium::bind_to_library(&lib)?);
    println!("bound in {:?}", t0.elapsed());

    spike_text_layer(&pdfium)?;
    spike_rotation(&pdfium)?;
    spike_search(&pdfium)?;
    spike_selection(&pdfium)?;
    spike_editing(&pdfium)?;
    spike_fonts(&pdfium)?;
    spike_xobject(&pdfium)?;
    spike_ocr_layer(&pdfium)?;
    spike_save(&pdfium)?;
    println!("\nDONE. outputs in {}", out_dir().display());
    Ok(())
}
