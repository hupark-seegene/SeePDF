//! `perf_bench` — the engine half of the F-30 performance baseline.
//!
//! ```sh
//! cd src-tauri && cargo run --release --example perf_bench
//! ```
//!
//! Prints one JSON object on stdout (everything else goes to stderr), which
//! `scripts/perf-baseline.mjs` merges with the frontend, app and OCR rows before writing
//! `docs/perf/baseline.md`. Everything here runs on the real engine thread against the real
//! bundled PDFium, with **no window** — these are the rows where the UI is not in the way and
//! a headless measurement is both faster and far less noisy than driving the app.
//!
//! Every timing is a wall-clock measurement of the same call the protocol handler makes, with
//! the encoded-tile cache bypassed (a fresh `TileKey` per sample would hit it, so the bench
//! calls `tiles::render` directly). Distributions report p50 and p95 over `SAMPLES` pages.

use seepdf_lib::engine::render::cache::{Night, RenderKind, TileKey};
use seepdf_lib::engine::render::{geometry, tiles};
use seepdf_lib::engine::{registry, text, EngineHandle, Lane};
use seepdf_lib::ipc::EngineError;
use std::path::{Path, PathBuf};
use std::time::Instant;

/// Pages sampled for each distribution (tile / text-layer / thumbnail).
const SAMPLES: u16 = 14;

fn main() {
    match run() {
        Ok(json) => println!("{json}"),
        Err(e) => {
            eprintln!("perf_bench: {:?} — {}", e.code, e.message);
            std::process::exit(1);
        }
    }
}

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("src-tauri has a parent")
        .join("fixtures")
}

fn engine() -> Result<EngineHandle, EngineError> {
    let (library, dir) = seepdf_lib::app::pdfium_path::resolve_local().map_err(EngineError::io)?;
    seepdf_lib::engine::spawn(
        library,
        dir,
        None,
        std::env::temp_dir().join(format!("seepdf-perf-{}", std::process::id())),
        seepdf_lib::engine::render::cache::DEFAULT_CAPACITY_BYTES,
    )
}

/// Milliseconds of one closure, measured `n` times, as `(p50, p95)`.
fn percentiles(mut samples: Vec<f64>) -> (f64, f64) {
    if samples.is_empty() {
        return (0.0, 0.0);
    }
    samples.sort_by(|a, b| a.partial_cmp(b).expect("no NaN"));
    let at = |q: f64| {
        let i = ((samples.len() as f64 - 1.0) * q).round() as usize;
        samples[i]
    };
    (at(0.5), at(0.95))
}

fn ms_since(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1000.0
}

struct Doc {
    id: String,
    generation: u32,
    pages: u16,
}

fn open(engine: &EngineHandle, path: &Path) -> Result<(Doc, f64), EngineError> {
    let bytes = std::fs::read(path).map_err(|e| EngineError::io(format!("{path:?}: {e}")))?;
    let owned = path.to_path_buf();
    let t = Instant::now();
    let info = engine.call_blocking(Lane::Edit, "perf/open", move |st| {
        registry::open(st, Some(owned), bytes, None)
    })?;
    let elapsed = ms_since(t);
    Ok((
        Doc {
            id: info.doc_id,
            generation: info.doc_generation,
            pages: info.page_count,
        },
        elapsed,
    ))
}

fn key(doc: &Doc, page: u16, kind: RenderKind, scale_key: u32, tx: u32, ty: u32) -> TileKey {
    TileKey {
        doc: doc.id.clone(),
        generation: doc.generation,
        page,
        kind,
        scale_key,
        rotation: 0,
        tx,
        ty,
        night: Night::Off,
        hl: false,
        forms: true,
    }
}

/// One render, timed end to end on the caller's side (dispatch + pdfium + copy-out), which is
/// what the `<img>` actually waits for minus the PNG encode.
fn render_ms(engine: &EngineHandle, key: TileKey) -> Result<f64, EngineError> {
    let t = Instant::now();
    engine.call_blocking(Lane::Interactive, "perf/render", move |st| {
        tiles::render(st, &tiles::RenderRequest::new(key)).map(|raw| raw.pixels.len())
    })?;
    Ok(ms_since(t))
}

fn run() -> Result<String, EngineError> {
    let engine = engine()?;
    let fx = fixtures();
    let mut out: Vec<(String, String)> = Vec::new();
    let mut push = |k: &str, v: String| out.push((k.to_string(), v));

    // --- open -------------------------------------------------------------------------
    let (trace, open_trace) = open(&engine, &fx.join("tracemonkey.pdf"))?;
    push("open.tracemonkey.ms", format!("{open_trace:.1}"));
    push("doc.tracemonkey.pages", trace.pages.to_string());

    // --- tiles at 2x and 4x -----------------------------------------------------------
    // 2x is the whole-page path the viewer uses below `MAX_WHOLE_PAGE_SCALE`; 4x is the
    // 512 px tile mosaic above it. Both sample the same pages so the two rows are comparable.
    let pages: Vec<u16> = (0..SAMPLES.min(trace.pages)).collect();
    let mut page_2x = Vec::new();
    for &p in &pages {
        page_2x.push(render_ms(&engine, key(&trace, p, RenderKind::Page, 200, 0, 0))?);
    }
    let (p50, p95) = percentiles(page_2x);
    push("render.page.2x.p50.ms", format!("{p50:.2}"));
    push("render.page.2x.p95.ms", format!("{p95:.2}"));

    let mut tile_4x = Vec::new();
    for &p in &pages {
        let geom_w = geometry::page_pixels(&trace_geom(&engine, &trace, p)?, 0, 4.0);
        let (cols, rows) = geometry::tile_grid(geom_w.0, geom_w.1);
        for ty in 0..rows.min(3) {
            for tx in 0..cols.min(2) {
                tile_4x.push(render_ms(
                    &engine,
                    key(&trace, p, RenderKind::Tile, 400, tx, ty),
                )?);
            }
        }
    }
    let n_tiles = tile_4x.len();
    let (p50, p95) = percentiles(tile_4x);
    push("render.tile.4x.p50.ms", format!("{p50:.2}"));
    push("render.tile.4x.p95.ms", format!("{p95:.2}"));
    push("render.tile.4x.samples", n_tiles.to_string());

    let mut tile_2x = Vec::new();
    for &p in &pages {
        let (w, h) = geometry::page_pixels(&trace_geom(&engine, &trace, p)?, 0, 2.0);
        let (cols, rows) = geometry::tile_grid(w, h);
        for ty in 0..rows.min(2) {
            for tx in 0..cols.min(2) {
                tile_2x.push(render_ms(
                    &engine,
                    key(&trace, p, RenderKind::Tile, 200, tx, ty),
                )?);
            }
        }
    }
    let (p50, p95) = percentiles(tile_2x);
    push("render.tile.2x.p50.ms", format!("{p50:.2}"));
    push("render.tile.2x.p95.ms", format!("{p95:.2}"));

    // --- thumbnails -------------------------------------------------------------------
    let mut thumbs = Vec::new();
    for &p in &pages {
        thumbs.push(render_ms(&engine, key(&trace, p, RenderKind::Thumb, 240, 0, 0))?);
    }
    let (p50, p95) = percentiles(thumbs);
    push("render.thumb.p50.ms", format!("{p50:.2}"));
    push("render.thumb.p95.ms", format!("{p95:.2}"));

    // --- text layer -------------------------------------------------------------------
    // The cache would answer the second call, so each page is measured exactly once on a
    // document whose layers have never been built.
    let (fresh, _) = open(&engine, &fx.join("tracemonkey.pdf"))?;
    let mut layers = Vec::new();
    for &p in &pages {
        let id = fresh.id.clone();
        let t = Instant::now();
        engine.call_blocking(Lane::Interactive, "perf/text", move |st| {
            let doc = st.doc_mut(&id)?;
            text::layer::layer(doc, p).map(|l| l.chars.len())
        })?;
        layers.push(ms_since(t));
    }
    let (p50, p95) = percentiles(layers);
    push("text.layer.p50.ms", format!("{p50:.2}"));
    push("text.layer.p95.ms", format!("{p95:.2}"));
    close(&engine, &fresh.id);

    // --- search all pages -------------------------------------------------------------
    // "monkey" over all 14 pages of tracemonkey — the 62 hits F-07 pins. Measured on a
    // freshly opened document so the text layers are built as part of the search, which is
    // what a user's first ⌘F actually pays.
    let (search_doc, _) = open(&engine, &fx.join("tracemonkey.pdf"))?;
    let id = search_doc.id.clone();
    let count = search_doc.pages;
    let t = Instant::now();
    let hits = engine.call_blocking(Lane::Interactive, "perf/search", move |st| {
        let query = text::search::Query::new("monkey", false, false)
            .ok_or_else(|| EngineError::invalid("empty query"))?;
        let mut total = 0usize;
        for page in 0..count {
            let doc = st.doc_mut(&id)?;
            total += text::search::search_page(doc, page, &query)?.len();
        }
        Ok(total)
    })?;
    push("search.tracemonkey.ms", format!("{:.1}", ms_since(t)));
    push("search.tracemonkey.hits", hits.to_string());
    close(&engine, &search_doc.id);

    // --- save round trip ---------------------------------------------------------------
    // serialize (the bytes PDFium would write) + reopen, which is the round trip F-23 is
    // measured on. Writing to disk is left out on purpose: it is the filesystem's number.
    let id = trace.id.clone();
    let t = Instant::now();
    let bytes = engine.call_blocking(Lane::Edit, "perf/serialize", move |st| {
        seepdf_lib::engine::save::serialize(st, &id)
    })?;
    let serialize_ms = ms_since(t);
    let size = bytes.len();
    let t = Instant::now();
    let reopened = engine.call_blocking(Lane::Edit, "perf/reopen", move |st| {
        registry::open(st, None, bytes, None)
    })?;
    let reopen_ms = ms_since(t);
    push("save.serialize.ms", format!("{serialize_ms:.1}"));
    push("save.reopen.ms", format!("{reopen_ms:.1}"));
    push(
        "save.roundtrip.ms",
        format!("{:.1}", serialize_ms + reopen_ms),
    );
    push("save.bytes", size.to_string());
    close(&engine, &reopened.doc_id);
    close(&engine, &trace.id);

    // --- 500 pages: open, first page, RSS ----------------------------------------------
    let big = fx.join("gen").join("500p.pdf");
    if big.exists() {
        let rss_before = seepdf_lib::engine::stats::rss_bytes();
        let (doc, open_ms) = open(&engine, &big)?;
        push("open.500p.ms", format!("{open_ms:.1}"));
        push("doc.500p.pages", doc.pages.to_string());
        let first = render_ms(&engine, key(&doc, 0, RenderKind::Page, 200, 0, 0))?;
        push("render.500p.firstPage.ms", format!("{first:.2}"));
        // "Scrolling" headless: render 40 pages at 2x plus their thumbnails, which is what a
        // drag through the rail costs, then read RSS.
        for p in 0..40u16.min(doc.pages) {
            render_ms(&engine, key(&doc, p, RenderKind::Page, 200, 0, 0))?;
            render_ms(&engine, key(&doc, p, RenderKind::Thumb, 240, 0, 0))?;
        }
        let rss = seepdf_lib::engine::stats::rss_bytes();
        push(
            "rss.500p.mb",
            format!("{:.1}", rss as f64 / (1024.0 * 1024.0)),
        );
        push(
            "rss.baseline.mb",
            format!("{:.1}", rss_before as f64 / (1024.0 * 1024.0)),
        );
        close(&engine, &doc.id);
    }

    let mut json = String::from("{");
    for (i, (k, v)) in out.iter().enumerate() {
        if i > 0 {
            json.push(',');
        }
        let numeric = v.parse::<f64>().is_ok();
        if numeric {
            json.push_str(&format!("\"{k}\":{v}"));
        } else {
            json.push_str(&format!("\"{k}\":\"{v}\""));
        }
    }
    json.push('}');
    Ok(json)
}

fn trace_geom(
    engine: &EngineHandle,
    doc: &Doc,
    page: u16,
) -> Result<seepdf_lib::ipc::types::PageGeom, EngineError> {
    let id = doc.id.clone();
    engine.call_blocking(Lane::Interactive, "perf/geom", move |st| {
        Ok(st.doc_mut(&id)?.geom(page)?.clone())
    })
}

fn close(engine: &EngineHandle, doc_id: &str) {
    let id = doc_id.to_string();
    let _ = engine.call_blocking(Lane::Edit, "perf/close", move |st| registry::close(st, &id));
}
