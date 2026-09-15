//! spike_render — pdfium-render 0.9.4 rendering + engine-lifetime spike for SeePDF.
//!
//! Run (from src-tauri/):
//!     cargo run --example spike_render              # debug build
//!     cargo run --release --example spike_render    # numbers quoted in docs/spikes/render.md
//!
//! Env overrides:
//!     SPIKE_FIXTURES=<dir>   directory containing tracemonkey.pdf / alphatrans.pdf
//!     SPIKE_PDFIUM=<file>    explicit libpdfium path (default: resources/pdfium/<host triple>)
//!     SPIKE_QUICK=1          skip the slow 8x full-page reference render in the tile section
//!
//! Findings are written up in docs/spikes/render.md. Everything here is deliberately
//! single-file and dependency-light so implementation agents can copy snippets verbatim.

use pdfium_render::prelude::*;
use std::collections::HashMap;
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

// ---------------------------------------------------------------------------------------------
// Small helpers
// ---------------------------------------------------------------------------------------------

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

fn mib(bytes: usize) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
}

/// Resident set size of this process in MiB, read from `ps` (macOS: KiB).
fn rss_mib() -> f64 {
    let out = std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &std::process::id().to_string()])
        .output();
    match out {
        Ok(o) => String::from_utf8_lossy(&o.stdout)
            .trim()
            .parse::<f64>()
            .map(|kib| kib / 1024.0)
            .unwrap_or(-1.0),
        Err(_) => -1.0,
    }
}

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn fixtures_dir() -> PathBuf {
    std::env::var_os("SPIKE_FIXTURES")
        .map(PathBuf::from)
        .unwrap_or_else(|| manifest_dir().join("..").join("fixtures"))
}

/// Host-specific libpdfium shipped in src-tauri/resources/pdfium/.
fn pdfium_lib_path() -> PathBuf {
    if let Some(p) = std::env::var_os("SPIKE_PDFIUM") {
        return PathBuf::from(p);
    }
    let file = if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        "libpdfium-aarch64-apple-darwin.dylib"
    } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
        "libpdfium-x86_64-apple-darwin.dylib"
    } else if cfg!(target_os = "windows") {
        "pdfium-x86_64-pc-windows-msvc.dll"
    } else {
        "libpdfium-x86_64-unknown-linux-gnu.so"
    };
    manifest_dir().join("resources").join("pdfium").join(file)
}

fn read_fixture(name: &str) -> Result<Vec<u8>, PdfiumError> {
    std::fs::read(fixtures_dir().join(name)).map_err(PdfiumError::IoError)
}

fn banner(title: &str) {
    println!();
    println!("=== {title} ===");
}

// ---------------------------------------------------------------------------------------------
// 1. Bind + document info
// ---------------------------------------------------------------------------------------------

/// Bind the shipped libpdfium exactly once and leak the `Pdfium` handle so that every
/// `PdfDocument<'static>` created from it can live in long-lived state.
///
/// `Pdfium::bind_to_library(path: impl AsRef<Path>) -> Result<Box<dyn PdfiumLibraryBindings>, PdfiumError>`
/// `Pdfium::new(bindings: Box<dyn PdfiumLibraryBindings>) -> Pdfium`
///
/// Note: `Pdfium::new` stores the bindings in a process-global `OnceCell` and calls
/// `FPDF_InitLibrary()`; a second `bind_to_library` returns
/// `PdfiumError::PdfiumLibraryBindingsAlreadyInitialized`. `Pdfium::drop` does NOT call
/// `FPDF_DestroyLibrary`, so leaking it costs nothing.
fn bind_pdfium() -> Result<&'static Pdfium, PdfiumError> {
    let path = pdfium_lib_path();
    let t = Instant::now();
    let bindings = Pdfium::bind_to_library(&path)?;
    let pdfium: &'static Pdfium = Box::leak(Box::new(Pdfium::new(bindings)));
    println!(
        "bound {} in {:.2} ms",
        path.file_name().unwrap().to_string_lossy(),
        ms(t.elapsed())
    );
    Ok(pdfium)
}

fn section_document_info(pdfium: &'static Pdfium, name: &str) -> Result<PdfDocument<'static>, PdfiumError> {
    banner(&format!("1. document info: {name}"));
    let bytes = read_fixture(name)?;
    let byte_len = bytes.len();

    let t = Instant::now();
    // `Pdfium::load_pdf_from_byte_vec(&self, bytes: Vec<u8>, password: Option<&str>) -> Result<PdfDocument<'_>, PdfiumError>`
    // The document takes ownership of the Vec (pdfium reads lazily from it for the doc lifetime).
    let doc = pdfium.load_pdf_from_byte_vec(bytes, None)?;
    let t_load = t.elapsed();

    let t = Instant::now();
    let page_count = doc.pages().len(); // PdfPageIndex = c_int
    let t_count = t.elapsed();

    // `PdfPages::page_sizes(&self) -> Result<Vec<PdfRect>, PdfiumError>` (FPDF_GetPageSizeByIndexF; no page load)
    let t = Instant::now();
    let sizes = doc.pages().page_sizes()?;
    let t_sizes = t.elapsed();

    println!(
        "{name}: {byte_len} bytes, load {:.2} ms, page_count={page_count} ({:.3} ms), page_sizes() {:.3} ms",
        ms(t_load),
        ms(t_count),
        ms(t_sizes)
    );
    println!(
        "metadata: title={:?} author={:?} producer={:?} form={:?} version={:?}",
        doc.metadata().get(PdfDocumentMetadataTagType::Title).map(|t| t.value().to_string()),
        doc.metadata().get(PdfDocumentMetadataTagType::Author).map(|t| t.value().to_string()),
        doc.metadata().get(PdfDocumentMetadataTagType::Producer).map(|t| t.value().to_string()),
        doc.form().map(|f| f.form_type()),
        doc.version(),
    );

    // Per-page: load page (FPDF_LoadPage), size, rotation, label.
    let mut total_get = Duration::ZERO;
    for i in 0..page_count.min(3) {
        let t = Instant::now();
        // `PdfPages::get(&self, index: PdfPageIndex) -> Result<PdfPage<'a>, PdfiumError>` — note the
        // returned page borrows the *document's* lifetime 'a, not `&self`.
        let page = doc.pages().get(i)?;
        let t_get = t.elapsed();
        total_get += t_get;
        let rot = page.rotation().map(|r| r.as_degrees()).unwrap_or(-1.0);
        println!(
            "  page {i}: get() {:.3} ms, {:.1} x {:.1} pt (page_sizes: {:.1} x {:.1}), rotation={rot} deg, label={:?}, boundaries: mediabox={:?}",
            ms(t_get),
            page.width().value,
            page.height().value,
            sizes[i as usize].width().value,
            sizes[i as usize].height().value,
            page.label(),
            page.boundaries().media().ok().map(|b| (b.bounds.width().value, b.bounds.height().value)),
        );
    }
    if page_count > 3 {
        for i in 3..page_count {
            let t = Instant::now();
            let _page = doc.pages().get(i)?;
            total_get += t.elapsed();
        }
    }
    println!(
        "  pages().get() avg over {page_count} pages: {:.3} ms",
        ms(total_get) / page_count as f64
    );

    // Text of page 0 (`PdfPage::text(&self) -> Result<PdfPageText<'_>, PdfiumError>`).
    {
        let page = doc.pages().get(0)?;
        let t = Instant::now();
        let text = page.text()?;
        let all = text.all();
        println!(
            "  page 0 text(): {} chars in {:.3} ms; first 60: {:?}",
            all.chars().count(),
            ms(t.elapsed()),
            all.chars().take(60).collect::<String>()
        );
    }

    // Bookmarks / outline: `PdfBookmarks::iter()` is depth-first prefix order over the whole tree.
    let t = Instant::now();
    let mut n = 0usize;
    for bm in doc.bookmarks().iter() {
        if n < 5 {
            println!(
                "  bookmark: {:?} -> page {:?}",
                bm.title(),
                bm.destination().and_then(|d| d.page_index().ok())
            );
        }
        n += 1;
    }
    println!("  bookmarks: {n} total ({:.3} ms)", ms(t.elapsed()));

    Ok(doc)
}

// ---------------------------------------------------------------------------------------------
// 2. Full-page renders at 1x / 2x / 4x
// ---------------------------------------------------------------------------------------------

/// The render configuration the viewer will use for "screen" quality.
fn screen_config(scale: f32) -> PdfRenderConfig {
    PdfRenderConfig::new()
        .scale_page_by_factor(scale) // pixel size = points * scale (round)
        .render_form_data(true) // FPDF_RenderPageBitmap + FPDF_FFLDraw path
        .render_annotations(true) // FPDF_ANNOT
        .use_lcd_text_rendering(false) // FPDF_LCD_TEXT off: we composite in a browser, no subpixel
        .set_clear_color(PdfColor::WHITE)
}

struct RenderSample {
    label: String,
    width: i32,
    height: i32,
    render: Duration,
    rgba_copy: Duration,
    raw_copy: Duration,
    raw_len: usize,
}

fn render_timed(page: &PdfPage<'_>, label: &str, cfg: &PdfRenderConfig) -> Result<(RenderSample, Vec<u8>), PdfiumError> {
    let t = Instant::now();
    // `PdfPage::render_with_config(&self, config: &PdfRenderConfig) -> Result<PdfBitmap<'_>, PdfiumError>`
    let bitmap = page.render_with_config(cfg)?;
    let render = t.elapsed();

    let t = Instant::now();
    // `PdfBitmap::as_raw_bytes(&self) -> Vec<u8>` (BGRA copy, stride == width*4 for BGRA)
    let raw = bitmap.as_raw_bytes();
    let raw_copy = t.elapsed();

    let t = Instant::now();
    // `PdfBitmap::as_rgba_bytes(&self) -> Vec<u8>` (copy + BGRA->RGBA swizzle)
    let rgba = bitmap.as_rgba_bytes();
    let rgba_copy = t.elapsed();
    let raw_len = raw.len();

    Ok((
        RenderSample {
            label: label.to_string(),
            width: bitmap.width(),
            height: bitmap.height(),
            render,
            rgba_copy,
            raw_copy,
            raw_len,
        },
        rgba,
    ))
}

fn section_full_render(doc_name: &str, doc: &PdfDocument<'static>) -> Result<Vec<u8>, PdfiumError> {
    banner(&format!("2. full-page renders: {doc_name} page 0"));
    let page = doc.pages().get(0)?;
    let mut rows: Vec<RenderSample> = Vec::new();
    let mut rgba_2x: Vec<u8> = Vec::new();

    for scale in [1.0f32, 2.0, 4.0] {
        // warm-up render (font cache etc.) is not separated: first call at 1x includes it, so run twice.
        let (_, _) = render_timed(&page, "warm", &screen_config(scale))?;
        let (s, rgba) = render_timed(&page, &format!("{scale}x screen"), &screen_config(scale))?;
        if scale == 2.0 {
            rgba_2x = rgba;
        }
        rows.push(s);
    }

    // Variants at 2x.
    let (s, _) = render_timed(&page, "2x +LCD text", &screen_config(2.0).use_lcd_text_rendering(true))?;
    rows.push(s);
    let (s, _) = render_timed(&page, "2x no annots/forms", &screen_config(2.0).render_form_data(false).render_annotations(false))?;
    rows.push(s);
    let (s, _) = render_timed(&page, "2x reverse_byte_order", &screen_config(2.0).set_reverse_byte_order(true))?;
    rows.push(s);
    let (s, _) = render_timed(&page, "2x BGR (24bpp)", &screen_config(2.0).set_format(PdfBitmapFormat::BGR))?;
    rows.push(s);
    let (s, _) = render_timed(&page, "2x print quality", &screen_config(2.0).use_print_quality(true))?;
    rows.push(s);
    let (s, _) = render_timed(&page, "1x rotate 90", &screen_config(1.0).rotate(PdfPageRenderRotation::Degrees90, true))?;
    rows.push(s);
    let (s, _) = render_timed(&page, "thumbnail 200px", &PdfRenderConfig::new().thumbnail(200).set_clear_color(PdfColor::WHITE))?;
    rows.push(s);
    let (s, _) = render_timed(&page, "set_target_width 1000", &PdfRenderConfig::new().set_target_width(1000).set_clear_color(PdfColor::WHITE))?;
    rows.push(s);

    // Reusing a bitmap: `PdfBitmap::empty(width, height, PdfBitmapFormat) -> Result<PdfBitmap<'a>>` +
    // `PdfPage::render_into_bitmap_with_config(&self, &mut PdfBitmap, &PdfRenderConfig) -> Result<()>`.
    {
        let cfg = screen_config(2.0);
        let (w, h) = ((page.width().value * 2.0).round() as i32, (page.height().value * 2.0).round() as i32);
        let mut bmp = PdfBitmap::empty(w, h, PdfBitmapFormat::BGRA)?;
        let t = Instant::now();
        page.render_into_bitmap_with_config(&mut bmp, &cfg)?;
        let render = t.elapsed();
        let t = Instant::now();
        let raw = bmp.as_raw_bytes();
        let raw_copy = t.elapsed();
        let t = Instant::now();
        let _rgba = bmp.as_rgba_bytes();
        let rgba_copy = t.elapsed();
        rows.push(RenderSample { label: "2x into reused bitmap".into(), width: w, height: h, render, rgba_copy, raw_copy, raw_len: raw.len() });
    }

    println!("{:<26} {:>11} {:>11} {:>11} {:>11} {:>8}", "config", "pixels", "render ms", "raw ms", "rgba ms", "raw B/px");
    for r in &rows {
        println!(
            "{:<26} {:>5}x{:<5} {:>11.2} {:>11.2} {:>11.2} {:>8}",
            r.label,
            r.width,
            r.height,
            ms(r.render),
            ms(r.raw_copy),
            ms(r.rgba_copy),
            r.raw_len / (r.width * r.height) as usize
        );
    }

    // Alpha check: transparent clear colour keeps alpha (useful for dark-mode compositing).
    {
        let bmp = page.render_with_config(&screen_config(1.0).set_clear_color(PdfColor::new(0, 0, 0, 0)))?;
        let rgba = bmp.as_rgba_bytes();
        let transparent = rgba.chunks_exact(4).filter(|p| p[3] < 255).count();
        println!(
            "transparent clear colour: {} / {} pixels have alpha < 255 (format {:?})",
            transparent,
            rgba.len() / 4,
            bmp.format()
        );
    }

    // All pages at 2x, sequential, dropping bitmaps: what a "render every page once" pass costs.
    let n = doc.pages().len();
    let t = Instant::now();
    let mut px_total = 0usize;
    for i in 0..n {
        let p = doc.pages().get(i)?;
        let b = p.render_with_config(&screen_config(2.0))?;
        px_total += (b.width() * b.height()) as usize;
    }
    let all = t.elapsed();
    println!(
        "all {n} pages at 2x (get+render, no copy): {:.1} ms total, {:.1} ms/page, {:.1} Mpx",
        ms(all),
        ms(all) / n as f64,
        px_total as f64 / 1e6
    );

    Ok(rgba_2x)
}

// ---------------------------------------------------------------------------------------------
// 3. Tiled rendering
// ---------------------------------------------------------------------------------------------

/// Count differing pixels between a tile and the corresponding crop of a full render.
fn compare_tile(full: &[u8], full_w: i32, tile: &[u8], tile_w: i32, tile_h: i32, tx: i32, ty: i32) -> (usize, u8) {
    let mut diff = 0usize;
    let mut max_delta = 0u8;
    for y in 0..tile_h {
        let src = (((ty + y) * full_w + tx) * 4) as usize;
        let dst = ((y * tile_w) * 4) as usize;
        let a = &full[src..src + (tile_w * 4) as usize];
        let b = &tile[dst..dst + (tile_w * 4) as usize];
        for (pa, pb) in a.chunks_exact(4).zip(b.chunks_exact(4)) {
            if pa != pb {
                diff += 1;
                for c in 0..4 {
                    max_delta = max_delta.max(pa[c].abs_diff(pb[c]));
                }
            }
        }
    }
    (diff, max_delta)
}

/// Tile via the *form-data path*: `FPDF_RenderPageBitmap(bitmap, page, start_x=-tx, start_y=-ty,
/// size_x=full_w, size_y=full_h, ...)` into a tile-sized bitmap. Pdfium clips to the bitmap.
/// Works with form fields + annotations (FPDF_FFLDraw gets the same offsets).
fn tile_config_origin(scale: f32, tx: i32, ty: i32) -> PdfRenderConfig {
    screen_config(scale).set_origin(-tx, -ty)
}

/// Tile via the *matrix path*: `FPDF_RenderPageBitmapWithMatrix(bitmap, page, &matrix, &clip, flags)`.
/// `translate()` is applied BEFORE the scale factor (row-vector convention: M = T * S), so the
/// offsets are in points (device px / scale). Any transform or `clip()` disables form-data rendering.
fn tile_config_matrix(scale: f32, tx: i32, ty: i32, tile_w: i32, tile_h: i32) -> Result<PdfRenderConfig, PdfiumError> {
    screen_config(scale)
        .translate(PdfPoints::new(-(tx as f32) / scale), PdfPoints::new(-(ty as f32) / scale))?
        .clip(0, 0, tile_w, tile_h)
        .pipe_ok()
}

trait PipeOk: Sized {
    fn pipe_ok(self) -> Result<Self, PdfiumError> {
        Ok(self)
    }
}
impl PipeOk for PdfRenderConfig {}

fn render_tile(page: &PdfPage<'_>, cfg: &PdfRenderConfig, tile_w: i32, tile_h: i32) -> Result<(Vec<u8>, Duration), PdfiumError> {
    let mut bmp = PdfBitmap::empty(tile_w, tile_h, PdfBitmapFormat::BGRA)?;
    let t = Instant::now();
    page.render_into_bitmap_with_config(&mut bmp, cfg)?;
    let d = t.elapsed();
    Ok((bmp.as_rgba_bytes(), d))
}

fn section_tiles(doc: &PdfDocument<'static>) -> Result<(), PdfiumError> {
    banner("3. tiled rendering (tracemonkey page 0)");
    let page = doc.pages().get(0)?;
    let scale = 8.0f32;
    let full_w = (page.width().value * scale).round() as i32;
    let full_h = (page.height().value * scale).round() as i32;
    println!("8x full page = {full_w} x {full_h} px ({:.1} MiB RGBA)", mib((full_w * full_h * 4) as usize));

    let quick = std::env::var_os("SPIKE_QUICK").is_some();
    let full = if quick {
        None
    } else {
        let t = Instant::now();
        let bmp = page.render_with_config(&screen_config(scale))?;
        let d = t.elapsed();
        let rgba = bmp.as_rgba_bytes();
        println!("8x full render (reference): {:.1} ms", ms(d));
        Some(rgba)
    };

    let (tile_w, tile_h) = (1024, 1024);
    let (tx, ty) = (1024, 2048); // tile at column 1, row 2 of a 1024 grid

    // Variant A: set_origin (form-data path)
    let (tile_a, d_a) = render_tile(&page, &tile_config_origin(scale, tx, ty), tile_w, tile_h)?;
    // Variant B: matrix + clip
    let (tile_b, d_b) = render_tile(&page, &tile_config_matrix(scale, tx, ty, tile_w, tile_h)?, tile_w, tile_h)?;
    // Variant B': matrix with the y sign flipped (to document which convention is right)
    let cfg_b2 = screen_config(scale)
        .translate(PdfPoints::new(-(tx as f32) / scale), PdfPoints::new((ty as f32) / scale))?
        .clip(0, 0, tile_w, tile_h);
    let (tile_b2, _) = render_tile(&page, &cfg_b2, tile_w, tile_h)?;

    let nonwhite = |t: &[u8]| t.chunks_exact(4).filter(|p| p[0] != 255 || p[1] != 255 || p[2] != 255).count();
    println!(
        "tile {tile_w}x{tile_h} @({tx},{ty}) 8x: set_origin {:.2} ms ({} non-white px); matrix+clip {:.2} ms ({} non-white px); matrix y-flipped ({} non-white px)",
        ms(d_a),
        nonwhite(&tile_a),
        ms(d_b),
        nonwhite(&tile_b),
        nonwhite(&tile_b2)
    );
    if let Some(full) = &full {
        let (da, ma) = compare_tile(full, full_w, &tile_a, tile_w, tile_h, tx, ty);
        let (db, mb) = compare_tile(full, full_w, &tile_b, tile_w, tile_h, tx, ty);
        let (db2, mb2) = compare_tile(full, full_w, &tile_b2, tile_w, tile_h, tx, ty);
        println!(
            "pixel compare vs crop of full render: set_origin diff={da} px (max delta {ma}); matrix+clip diff={db} px (max delta {mb}); matrix y-flipped diff={db2} px (max delta {mb2})"
        );
        // Spot checks at a few points.
        for (x, y) in [(10, 10), (500, 300), (1000, 1000), (700, 20)] {
            let fi = ((((ty + y) * full_w) + (tx + x)) * 4) as usize;
            let ti = (((y * tile_w) + x) * 4) as usize;
            println!(
                "  spot ({x},{y}): full={:?} origin={:?} matrix={:?}",
                &full[fi..fi + 4],
                &tile_a[ti..ti + 4],
                &tile_b[ti..ti + 4]
            );
        }
    }
    let (da, _) = compare_tile(&tile_a, tile_w, &tile_b, tile_w, tile_h, 0, 0);
    println!("set_origin vs matrix+clip tile-to-tile diff: {da} px");

    // Tile-size sweep at 8x (both paths). Same page region, different tile sizes.
    println!("{:<10} {:>16} {:>16} {:>12}", "tile", "set_origin ms", "matrix+clip ms", "non-white %");
    for size in [256, 512, 1024, 2048] {
        let mut a = Duration::ZERO;
        let mut b = Duration::ZERO;
        let mut density = 0.0;
        let reps = 3;
        for _ in 0..reps {
            let (px, d) = render_tile(&page, &tile_config_origin(scale, tx, ty), size, size)?;
            a += d;
            density = nonwhite(&px) as f64 * 100.0 / (size * size) as f64;
            b += render_tile(&page, &tile_config_matrix(scale, tx, ty, size, size)?, size, size)?.1;
        }
        println!("{:<10} {:>16.2} {:>16.2} {:>12.2}", format!("{size}px"), ms(a) / reps as f64, ms(b) / reps as f64, density);
    }
    // Same sweep at a text-dense spot (first column body text: ~90pt from the left, ~330pt down).
    let (dx, dy) = ((90.0 * scale) as i32, (330.0 * scale) as i32);
    println!("dense region @({dx},{dy}):");
    for size in [256, 512, 1024] {
        let (px, d) = render_tile(&page, &tile_config_origin(scale, dx, dy), size, size)?;
        println!("{:<10} {:>16.2} {:>16} {:>12.2}", format!("{size}px"), ms(d), "-", nonwhite(&px) as f64 * 100.0 / (size * size) as f64);
    }

    // Cost of covering a whole page with tiles vs one full render, at 2x and 4x.
    for s in [2.0f32, 4.0] {
        let w = (page.width().value * s).round() as i32;
        let h = (page.height().value * s).round() as i32;
        let t = Instant::now();
        let _ = page.render_with_config(&screen_config(s))?;
        let full_ms = ms(t.elapsed());
        for size in [512, 1024] {
            let mut n = 0;
            let t = Instant::now();
            let mut y = 0;
            while y < h {
                let mut x = 0;
                while x < w {
                    let tw = size.min(w - x);
                    let th = size.min(h - y);
                    let mut bmp = PdfBitmap::empty(tw, th, PdfBitmapFormat::BGRA)?;
                    page.render_into_bitmap_with_config(&mut bmp, &tile_config_origin(s, x, y))?;
                    n += 1;
                    x += size;
                }
                y += size;
            }
            println!(
                "{s}x page ({w}x{h}): full render {:.1} ms vs {n} tiles of {size}px {:.1} ms ({:.2} ms/tile)",
                full_ms,
                ms(t.elapsed()),
                ms(t.elapsed()) / n as f64
            );
        }
    }

    // Empty region tile (bottom-right margin) — cheap? (pdfium still walks the display list)
    let (_, d) = render_tile(&page, &tile_config_origin(scale, full_w - 256, full_h - 256), 256, 256)?;
    println!("256px tile in blank margin: {:.2} ms", ms(d));

    Ok(())
}

// ---------------------------------------------------------------------------------------------
// 4. Encoding benchmark
// ---------------------------------------------------------------------------------------------

fn encode_png(rgba: &[u8], w: u32, h: u32, color: png::ColorType, comp: png::Compression, filter: png::Filter) -> Vec<u8> {
    let mut out = Vec::with_capacity(rgba.len() / 4);
    {
        let mut enc = png::Encoder::new(&mut out, w, h);
        enc.set_color(color);
        enc.set_depth(png::BitDepth::Eight);
        enc.set_compression(comp);
        enc.set_filter(filter);
        let mut writer = enc.write_header().expect("png header");
        writer.write_image_data(rgba).expect("png data");
    }
    out
}

fn rgba_to_rgb(rgba: &[u8]) -> Vec<u8> {
    let mut rgb = Vec::with_capacity(rgba.len() / 4 * 3);
    for p in rgba.chunks_exact(4) {
        rgb.extend_from_slice(&p[..3]);
    }
    rgb
}

fn encode_jpeg(rgb: &[u8], w: u32, h: u32, q: u8) -> Vec<u8> {
    let mut out = Vec::with_capacity(rgb.len() / 8);
    let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, q);
    enc.encode(rgb, w, h, image::ExtendedColorType::Rgb8).expect("jpeg");
    out
}

fn section_encode(rgba: &[u8], w: i32, h: i32) {
    banner(&format!("4. encode benchmark: tracemonkey page 0 @2x ({w}x{h} RGBA)"));
    let (w, h) = (w as u32, h as u32);
    let raw = rgba.len();
    println!("{:<34} {:>10} {:>11} {:>8}", "encoding", "bytes", "encode ms", "ratio");
    println!("{:<34} {:>10} {:>11.2} {:>8.2}", "raw RGBA", raw, 0.0, 1.0);

    let bench = |label: &str, f: &mut dyn FnMut() -> Vec<u8>| {
        let _ = f(); // warm
        let reps = 3;
        let t = Instant::now();
        let mut out = Vec::new();
        for _ in 0..reps {
            out = f();
        }
        let d = ms(t.elapsed()) / reps as f64;
        println!("{:<34} {:>10} {:>11.2} {:>8.2}", label, out.len(), d, raw as f64 / out.len() as f64);
    };

    bench("PNG RGBA Fast/NoFilter", &mut || encode_png(rgba, w, h, png::ColorType::Rgba, png::Compression::Fast, png::Filter::NoFilter));
    bench("PNG RGBA Fastest/NoFilter", &mut || encode_png(rgba, w, h, png::ColorType::Rgba, png::Compression::Fastest, png::Filter::NoFilter));
    bench("PNG RGBA Fast/Sub", &mut || encode_png(rgba, w, h, png::ColorType::Rgba, png::Compression::Fast, png::Filter::Sub));
    bench("PNG RGBA Fast/Adaptive", &mut || encode_png(rgba, w, h, png::ColorType::Rgba, png::Compression::Fast, png::Filter::Adaptive));
    bench("PNG RGBA Balanced/Adaptive", &mut || encode_png(rgba, w, h, png::ColorType::Rgba, png::Compression::Balanced, png::Filter::Adaptive));

    let t = Instant::now();
    let rgb = rgba_to_rgb(rgba);
    println!("(rgba->rgb strip: {:.2} ms)", ms(t.elapsed()));
    bench("PNG RGB Fast/NoFilter (+strip)", &mut || encode_png(&rgba_to_rgb(rgba), w, h, png::ColorType::Rgb, png::Compression::Fast, png::Filter::NoFilter));
    bench("JPEG q85 RGB (+strip)", &mut || encode_jpeg(&rgba_to_rgb(rgba), w, h, 85));
    bench("JPEG q85 RGB (pre-stripped)", &mut || encode_jpeg(&rgb, w, h, 85));
    bench("JPEG q75 RGB (pre-stripped)", &mut || encode_jpeg(&rgb, w, h, 75));
    bench("JPEG q92 RGB (pre-stripped)", &mut || encode_jpeg(&rgb, w, h, 92));
}

// ---------------------------------------------------------------------------------------------
// 5. Engine ownership model
// ---------------------------------------------------------------------------------------------

fn assert_send<T: Send>() {}
fn assert_sync<T: Sync>() {}
/// What `tauri::App::manage<T>` requires.
fn assert_manageable<T: Send + Sync + 'static>() {}

type DocId = u32;

/// Model (b): everything behind a global lock, documents borrow a leaked `&'static Pdfium`.
/// Pages are kept alongside; the `Drop` impl guarantees pages close before their documents
/// (FPDF_ClosePage after FPDF_CloseDocument is use-after-free inside pdfium).
struct Engine {
    pdfium: &'static Pdfium,
    docs: HashMap<DocId, PdfDocument<'static>>,
    pages: HashMap<(DocId, PdfPageIndex), PdfPage<'static>>,
    next_id: DocId,
}

impl Engine {
    fn new(pdfium: &'static Pdfium) -> Self {
        Engine { pdfium, docs: HashMap::new(), pages: HashMap::new(), next_id: 1 }
    }

    fn open(&mut self, bytes: Vec<u8>) -> Result<DocId, PdfiumError> {
        let doc = self.pdfium.load_pdf_from_byte_vec(bytes, None)?;
        let id = self.next_id;
        self.next_id += 1;
        self.docs.insert(id, doc);
        Ok(id)
    }

    fn close(&mut self, id: DocId) {
        self.pages.retain(|(d, _), _| *d != id); // pages first!
        self.docs.remove(&id);
    }

    fn page(&mut self, id: DocId, index: PdfPageIndex) -> Result<&PdfPage<'static>, PdfiumError> {
        if !self.pages.contains_key(&(id, index)) {
            let doc = self.docs.get(&id).ok_or(PdfiumError::PageIndexOutOfBounds)?;
            let page = doc.pages().get(index)?; // PdfPage<'static> because PdfDocument<'static>
            self.pages.insert((id, index), page);
        }
        Ok(&self.pages[&(id, index)])
    }

    fn render_rgba(&mut self, id: DocId, index: PdfPageIndex, cfg: &PdfRenderConfig) -> Result<(i32, i32, Vec<u8>), PdfiumError> {
        let page = self.page(id, index)?;
        let bmp = page.render_with_config(cfg)?;
        Ok((bmp.width(), bmp.height(), bmp.as_rgba_bytes()))
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        self.pages.clear();
        self.docs.clear();
    }
}

/// Model (a): a dedicated engine thread. The thread owns `Pdfium` on its own stack; documents
/// borrow it locally, so no leaking / 'static is needed and nothing pdfium-related ever
/// crosses a thread boundary (only plain `Vec<u8>` results do).
enum Cmd {
    Ping(crossbeam_channel::Sender<()>),
    Open(Vec<u8>, crossbeam_channel::Sender<Result<DocId, String>>),
    Render {
        doc: DocId,
        page: PdfPageIndex,
        scale: f32,
        reply: crossbeam_channel::Sender<Result<(i32, i32, Vec<u8>), String>>,
    },
    Close(DocId),
    Shutdown,
}

fn spawn_engine_thread() -> (crossbeam_channel::Sender<Cmd>, std::thread::JoinHandle<()>) {
    let (tx, rx) = crossbeam_channel::unbounded::<Cmd>();
    let handle = std::thread::Builder::new()
        .name("pdf-engine".into())
        .stack_size(16 * 1024 * 1024)
        .spawn(move || {
            // The library is already bound process-wide by main(); `Pdfium::default()` returns a
            // fresh handle when bindings exist (it short-circuits on
            // PdfiumLibraryBindingsAlreadyInitialized). In the real app this thread would call
            // Pdfium::bind_to_library + Pdfium::new itself, so no `&'static` is needed anywhere.
            let pdfium = Pdfium::default();
            let mut docs: HashMap<DocId, PdfDocument<'_>> = HashMap::new();
            let mut pages: HashMap<(DocId, PdfPageIndex), PdfPage<'_>> = HashMap::new();
            let mut next = 1;
            for cmd in rx {
                match cmd {
                    Cmd::Ping(r) => {
                        let _ = r.send(());
                    }
                    Cmd::Open(bytes, r) => {
                        let res = pdfium.load_pdf_from_byte_vec(bytes, None).map(|d| {
                            let id = next;
                            next += 1;
                            docs.insert(id, d);
                            id
                        });
                        let _ = r.send(res.map_err(|e| e.to_string()));
                    }
                    Cmd::Render { doc, page, scale, reply } => {
                        let res = (|| {
                            if !pages.contains_key(&(doc, page)) {
                                let d = docs.get(&doc).ok_or_else(|| "no such doc".to_string())?;
                                pages.insert((doc, page), d.pages().get(page).map_err(|e| e.to_string())?);
                            }
                            let p = &pages[&(doc, page)];
                            let b = p.render_with_config(&screen_config(scale)).map_err(|e| e.to_string())?;
                            Ok((b.width(), b.height(), b.as_rgba_bytes()))
                        })();
                        let _ = reply.send(res);
                    }
                    Cmd::Close(id) => {
                        pages.retain(|(d, _), _| *d != id);
                        docs.remove(&id);
                    }
                    Cmd::Shutdown => break,
                }
            }
            pages.clear();
            docs.clear();
        })
        .expect("spawn engine thread");
    (tx, handle)
}

fn section_ownership(pdfium: &'static Pdfium) -> Result<(), PdfiumError> {
    banner("5. engine ownership model");

    // --- Send/Sync probes (all compile under feature "thread_safe") ---
    assert_send::<Pdfium>();
    assert_sync::<Pdfium>();
    assert_send::<PdfDocument<'static>>();
    assert_sync::<PdfDocument<'static>>();
    assert_send::<PdfPage<'static>>();
    assert_sync::<PdfPage<'static>>();
    assert_send::<PdfPages<'static>>();
    assert_send::<PdfBitmap<'static>>();
    assert_sync::<PdfBitmap<'static>>();
    assert_send::<PdfPageText<'static>>();
    assert_sync::<PdfPageText<'static>>();
    assert_send::<PdfRenderConfig>();
    assert_sync::<PdfRenderConfig>();
    assert_manageable::<Mutex<Engine>>();
    assert_manageable::<Engine>();
    println!("Send/Sync probes: Pdfium, PdfDocument<'static>, PdfPage<'static>, PdfPages, PdfBitmap, PdfPageText, PdfRenderConfig are all Send + Sync; Mutex<Engine> satisfies tauri::Manager::manage bounds");
    println!("(without feature thread_safe every one of those `unsafe impl Send/Sync` is cfg'd out and none of it compiles)");

    // --- Model (b): leaked &'static Pdfium + documents/pages in a struct ---
    let mut engine = Engine::new(pdfium);
    let a = engine.open(read_fixture("tracemonkey.pdf")?)?;
    let b = engine.open(read_fixture("alphatrans.pdf")?)?;
    let (wa, ha, _) = engine.render_rgba(a, 0, &screen_config(1.0))?;
    let (wb, hb, _) = engine.render_rgba(b, 0, &screen_config(1.0))?;
    let (wa2, _, _) = engine.render_rgba(a, 1, &screen_config(1.0))?;
    println!("two docs open at once: doc{a} p0 {wa}x{ha}, doc{b} p0 {wb}x{hb}, doc{a} p1 {wa2}px wide — interleaved renders OK");
    engine.close(b);
    let (wa3, _, _) = engine.render_rgba(a, 2, &screen_config(1.0))?;
    println!("after closing doc{b}: doc{a} p2 still renders ({wa3}px wide); cached pages for doc{b} were dropped before the document");

    // Cached page (kept open) vs pages().get() every time:
    {
        let t = Instant::now();
        for _ in 0..20 {
            engine.render_rgba(a, 3, &screen_config(1.0))?;
        }
        let cached = ms(t.elapsed()) / 20.0;
        let doc = &engine.docs[&a];
        let t = Instant::now();
        for _ in 0..20 {
            let p = doc.pages().get(3)?;
            let b = p.render_with_config(&screen_config(1.0))?;
            let _ = b.as_rgba_bytes();
        }
        let fresh = ms(t.elapsed()) / 20.0;
        println!("1x render p3: cached PdfPage {cached:.2} ms/render vs pages().get() each time {fresh:.2} ms/render");
    }

    // --- Concurrency under thread_safe: does a second thread buy anything? ---
    {
        let shared = Arc::new(Mutex::new(engine));
        let render_n = move |eng: &Arc<Mutex<Engine>>, n: usize, off: PdfPageIndex| -> Duration {
            let t = Instant::now();
            for i in 0..n {
                let mut e = eng.lock().unwrap();
                e.render_rgba(a, (i as PdfPageIndex + off) % 14, &screen_config(2.0)).unwrap();
            }
            t.elapsed()
        };
        let _warm = render_n(&shared, 14, 0); // loads + caches all 14 PdfPages
        let single = render_n(&shared, 14, 0);
        let t = Instant::now();
        let h1 = {
            let s = Arc::clone(&shared);
            std::thread::spawn(move || render_n(&s, 7, 0))
        };
        let h2 = {
            let s = Arc::clone(&shared);
            std::thread::spawn(move || render_n(&s, 7, 7))
        };
        h1.join().unwrap();
        h2.join().unwrap();
        let two = t.elapsed();
        println!(
            "14 renders @2x through Mutex<Engine>: 1 thread {:.1} ms, 2 threads {:.1} ms (pdfium is serialised by the crate's global lock; no parallel speedup possible)",
            ms(single),
            ms(two)
        );

        // Direct shared-reference concurrency (PdfDocument: Sync) — relies on the crate's per-call
        // lock. NOTE: FPDF_RenderPageBitmapWithMatrix / FPDFBitmap_CreateEx / FPDFText_* are NOT
        // locked in 0.9.4's thread_safe.rs, so this is only sound for the FPDF_RenderPageBitmap path.
        let eng = shared.lock().unwrap();
        let doc: &PdfDocument<'static> = &eng.docs[&a];
        let t = Instant::now();
        std::thread::scope(|s| {
            for k in 0..2 {
                s.spawn(move || {
                    for i in 0..7 {
                        let p = doc.pages().get(k * 7 + i).unwrap();
                        let _ = p.render_with_config(&screen_config(2.0)).unwrap();
                    }
                });
            }
        });
        println!("14 renders @2x from 2 threads sharing &PdfDocument (no Mutex): {:.1} ms — same wall time, confirms global serialisation", ms(t.elapsed()));
        drop(eng);
    }

    // --- Model (a): dedicated engine thread over crossbeam ---
    {
        let (tx, handle) = spawn_engine_thread();
        let (rtx, rrx) = crossbeam_channel::bounded(1);
        // channel round-trip overhead
        let t = Instant::now();
        for _ in 0..1000 {
            tx.send(Cmd::Ping(rtx.clone())).unwrap();
            rrx.recv().unwrap();
        }
        let ping = t.elapsed();
        let (otx, orx) = crossbeam_channel::bounded(1);
        tx.send(Cmd::Open(read_fixture("tracemonkey.pdf")?, otx)).unwrap();
        let id = orx.recv().unwrap().expect("open");
        let (ptx, prx) = crossbeam_channel::bounded(1);
        let t = Instant::now();
        tx.send(Cmd::Render { doc: id, page: 0, scale: 2.0, reply: ptx.clone() }).unwrap();
        let (w, h, px) = prx.recv().unwrap().expect("render");
        let first = t.elapsed();
        let t = Instant::now();
        tx.send(Cmd::Render { doc: id, page: 0, scale: 2.0, reply: ptx }).unwrap();
        let _ = prx.recv().unwrap().expect("render");
        let second = t.elapsed();
        println!(
            "engine thread: ping round-trip {:.1} us; render p0@2x {w}x{h} ({:.1} MiB) first {:.1} ms (incl. page load), second {:.1} ms",
            ping.as_secs_f64() * 1e6 / 1000.0,
            mib(px.len()),
            ms(first),
            ms(second)
        );
        tx.send(Cmd::Close(id)).unwrap();
        tx.send(Cmd::Shutdown).unwrap();
        handle.join().unwrap();
        println!("engine thread shut down cleanly (docs/pages dropped on the engine thread)");
    }

    // --- Dropping a DOCUMENT while one of its pages is alive: the borrow checker allows it! ---
    // PdfPage<'a> borrows the Pdfium lifetime 'a, not the PdfDocument, so this compiles. At runtime
    // FPDF_ClosePage after FPDF_CloseDocument is a use-after-free inside pdfium. Never run it;
    // the guard exists only so the compiler proves the point.
    if std::env::var_os("SPIKE_UAF").is_some() {
        let doc = pdfium.load_pdf_from_byte_vec(read_fixture("alphatrans.pdf")?, None)?;
        let page: PdfPage<'static> = doc.pages().get(0)?;
        drop(doc); // compiles fine
        println!("{}", page.width().value); // UB: page outlived its document
    }
    println!("drop(doc) while a PdfPage<'static> is alive: COMPILES (no lifetime tie) — engine code must drop pages before documents");

    // --- Dropping a page while holding text(): compile-time error, see docs/spikes/render.md ---
    // PdfPageText<'a> holds `page: &'a PdfPage<'a>`; this does not compile:
    //     let page = doc.pages().get(0)?;
    //     let text = page.text()?;
    //     drop(page);                // error[E0505]: cannot move out of `page` because it is borrowed
    //     println!("{}", text.all());
    println!("drop(page) while PdfPageText alive: rejected at compile time (E0505), verified separately");

    Ok(())
}

// ---------------------------------------------------------------------------------------------
// 6. Memory
// ---------------------------------------------------------------------------------------------

fn section_memory(pdfium: &'static Pdfium) -> Result<(), PdfiumError> {
    banner("6. memory (RSS via ps)");
    let base = rss_mib();
    println!("RSS before load: {base:.1} MiB");
    let doc = pdfium.load_pdf_from_byte_vec(read_fixture("tracemonkey.pdf")?, None)?;
    println!("RSS after load tracemonkey.pdf: {:.1} MiB", rss_mib());
    let n = doc.pages().len();
    let mut cache: Vec<Vec<u8>> = Vec::new();
    let mut pages: Vec<PdfPage<'static>> = Vec::new();
    for i in 0..n {
        let p = doc.pages().get(i)?;
        let b = p.render_with_config(&screen_config(2.0))?;
        let rgba = b.as_rgba_bytes();
        drop(b);
        cache.push(rgba);
        pages.push(p);
    }
    let held: usize = cache.iter().map(|v| v.len()).sum();
    // Fresh page objects vs cache: also time the peak of one 4x render (transient bitmap).
    {
        let before = rss_mib();
        let p = doc.pages().get(0)?;
        let b = p.render_with_config(&screen_config(4.0))?;
        let during = rss_mib();
        drop(b);
        println!("one 4x render of page 0 ({}x{} BGRA, {:.1} MiB): RSS {before:.1} -> {during:.1} MiB", 2448, 3168, mib(2448 * 3168 * 4));
    }
    println!(
        "RSS after rendering all {n} pages @2x, holding {n} open PdfPages + {:.1} MiB of RGBA: {:.1} MiB",
        mib(held),
        rss_mib()
    );
    drop(cache);
    println!("RSS after dropping RGBA cache (pages still open): {:.1} MiB", rss_mib());
    drop(pages);
    println!("RSS after closing pages: {:.1} MiB", rss_mib());
    drop(doc);
    println!("RSS after closing document: {:.1} MiB", rss_mib());
    Ok(())
}

// ---------------------------------------------------------------------------------------------

fn run() -> Result<(), PdfiumError> {
    println!("spike_render — pdfium-render 0.9.4, libpdfium {}", pdfium_lib_path().display());
    println!("profile: {}", if cfg!(debug_assertions) { "debug" } else { "release" });
    println!("RSS at start: {:.1} MiB", rss_mib());
    let pdfium = bind_pdfium()?;
    println!("RSS after bind: {:.1} MiB", rss_mib());

    if std::env::var("SPIKE_ONLY").as_deref() == Ok("memory") {
        // Clean-process measurement: run with `/usr/bin/time -l` for max RSS.
        section_memory(pdfium)?;
        return Ok(());
    }

    let trace = section_document_info(pdfium, "tracemonkey.pdf")?;
    let alpha = section_document_info(pdfium, "alphatrans.pdf")?;
    if fixtures_dir().join("rotation.pdf").exists() {
        let _ = section_document_info(pdfium, "rotation.pdf")?;
    }

    let rgba_2x = section_full_render("tracemonkey.pdf", &trace)?;
    let _ = section_full_render("alphatrans.pdf", &alpha)?;

    section_tiles(&trace)?;

    let p0 = trace.pages().get(0)?;
    let w = (p0.width().value * 2.0).round() as i32;
    let h = (p0.height().value * 2.0).round() as i32;
    drop(p0);
    section_encode(&rgba_2x, w, h);

    drop(alpha);
    drop(trace);
    section_ownership(pdfium)?;
    section_memory(pdfium)?;

    println!();
    println!("done. RSS at exit: {:.1} MiB", rss_mib());
    std::io::stdout().flush().ok();
    Ok(())
}

fn main() {
    if let Err(e) = run() {
        eprintln!("spike_render failed: {e:?}");
        std::process::exit(1);
    }
}
