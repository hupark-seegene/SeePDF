//! Render pipeline: tile ↔ full-render equivalence, rotation sizes, thumbnails, the encoded
//! cache and the raw buffer format.

mod common;

use common::*;
use seepdf_lib::engine::render::cache::{Night, RenderKind, TileKey};
use seepdf_lib::engine::render::geometry;
use seepdf_lib::engine::render::tiles::{self, RenderRequest};
use seepdf_lib::engine::Lane;
use seepdf_lib::ipc::types::Rect;

fn key(doc: &TestDoc, kind: RenderKind, scale_key: u32, tx: u32, ty: u32) -> TileKey {
    TileKey {
        doc: doc.doc_id.clone(),
        generation: doc.info.doc_generation,
        page: 0,
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

struct Image {
    pixels: Vec<u8>,
    width: u32,
    height: u32,
}

fn render(key: TileKey) -> Image {
    engine()
        .call_blocking(Lane::Interactive, "test/render", move |st| {
            let raw = tiles::render(st, &RenderRequest::new(key))?;
            Ok(Image {
                pixels: raw.pixels,
                width: raw.width,
                height: raw.height,
            })
        })
        .expect("render")
}

/// A 512² `set_origin` tile at 8× equals the corresponding crop of the full render, within
/// the anti-aliasing rounding the render spike measured (427 / 1,048,576 px at max channel
/// delta 1 over a 1024² tile). This is the guarantee the whole tiling design rests on.
#[test]
fn render_tile_matches_crop() {
    let doc = open("tracemonkey.pdf");
    let scale_key = 800; // s = 8.0
    let (tx, ty) = (2u32, 5u32); // dense body text on tracemonkey p1 at 8x

    let full = render(key(&doc, RenderKind::Page, scale_key, 0, 0));
    let tile = render(key(&doc, RenderKind::Tile, scale_key, tx, ty));

    let (ox, oy, tw, th) =
        geometry::tile_rect(full.width, full.height, tx, ty).expect("tile is inside the page");
    assert_eq!((tile.width, tile.height), (tw, th));

    let mut differing_px = 0usize;
    let mut max_delta = 0u8;
    for y in 0..th {
        for x in 0..tw {
            let a = (((oy + y) * full.width + ox + x) * 4) as usize;
            let b = ((y * tw + x) * 4) as usize;
            let mut differs = false;
            for c in 0..4 {
                let delta = full.pixels[a + c].abs_diff(tile.pixels[b + c]);
                if delta > 0 {
                    differs = true;
                    max_delta = max_delta.max(delta);
                }
            }
            differing_px += usize::from(differs);
        }
    }
    let total = (tw * th) as usize;
    assert!(
        max_delta <= 1,
        "max channel delta {max_delta}; only anti-aliasing rounding is acceptable"
    );
    // The spike measured 427 / 1,048,576 px (0.04 %) on a 1024 px tile at 8x; 0.5 % leaves
    // room for a different tile without letting a real geometry bug through.
    assert!(
        differing_px * 200 <= total,
        "{differing_px} of {total} px differ ({:.3} %)",
        differing_px as f64 * 100.0 / total as f64
    );

    // The tile is not blank: this must be a real comparison.
    let non_white = tile.pixels.chunks_exact(4).filter(|px| px[0] < 200).count();
    assert!(
        non_white > 1000,
        "tile ({tx},{ty}) has only {non_white} dark px"
    );
}

/// `rotation.pdf` page 1 has `/Rotate 90`, so pdfium already reports 792×612 pt and a 1×
/// render is 792×612 px. A 90° *view* rotation on top swaps the output again.
#[test]
fn render_rotation_sizes() {
    let doc = open("rotation.pdf");
    assert_eq!(
        (doc.info.pages[0].width_pt, doc.info.pages[0].height_pt),
        (612.0, 792.0)
    );
    assert_eq!(
        (doc.info.pages[1].width_pt, doc.info.pages[1].height_pt),
        (792.0, 612.0),
        "the intrinsic /Rotate is already applied by pdfium"
    );
    assert_eq!(doc.info.pages[1].rotation, 90);

    let mut k = key(&doc, RenderKind::Page, 100, 0, 0);
    k.page = 1;
    let image = render(k.clone());
    assert_eq!((image.width, image.height), (792, 612));

    k.rotation = 90;
    let rotated = render(k.clone());
    assert_eq!(
        (rotated.width, rotated.height),
        (612, 792),
        "a 90 degree view rotation swaps the output dimensions"
    );

    // The geometry helper and the renderer must never disagree — the frontend lays out from
    // the helper and paints what the renderer produced.
    assert_eq!(
        geometry::page_pixels(&doc.info.pages[1], 90, 1.0),
        (rotated.width, rotated.height)
    );
}

/// Thumbnails keep the page's aspect ratio. Never `thumbnail(n)`, which squashes to n×n and
/// silently disables annotation and form rendering (pages spike §4).
#[test]
fn render_thumbnails_keep_aspect_ratio() {
    let doc = open("tracemonkey.pdf");
    let image = render(key(&doc, RenderKind::Thumb, 180, 0, 0));
    assert_eq!(image.width, 180);
    let expected = (180.0_f32 * 792.0 / 612.0).round() as u32;
    assert!(
        image.height.abs_diff(expected) <= 1,
        "{}x{} is not the page's 612:792 ratio",
        image.width,
        image.height
    );
}

/// `render_page_raw` produces the `SPRX` header of `IPC_CONTRACT.md` §10.2.
#[test]
fn render_raw_buffer_header() {
    let doc = open("tracemonkey.pdf");
    let doc_id = doc.doc_id.clone();
    let buffer = engine()
        .call_blocking(Lane::Interactive, "test/raw", move |st| {
            tiles::render_raw_buffer(st, &doc_id, 0, 1.0, None)
        })
        .expect("render_page_raw");

    assert_eq!(&buffer[0..4], b"SPRX");
    assert_eq!(u16::from_le_bytes([buffer[4], buffer[5]]), 1, "version");
    assert_eq!(u16::from_le_bytes([buffer[6], buffer[7]]), 1, "RGBA8");
    let u32_at = |o: usize| u32::from_le_bytes(buffer[o..o + 4].try_into().unwrap());
    let (width, height, stride) = (u32_at(8), u32_at(12), u32_at(16));
    assert_eq!((width, height), (612, 792));
    assert_eq!(stride, width * 4);
    assert_eq!(buffer.len(), 32 + (stride * height) as usize);

    // A rect crops in device space.
    let doc_id = doc.doc_id.clone();
    let cropped = engine()
        .call_blocking(Lane::Interactive, "test/raw-rect", move |st| {
            tiles::render_raw_buffer(
                st,
                &doc_id,
                0,
                1.0,
                Some(Rect::new(72.0, 600.0, 272.0, 700.0)),
            )
        })
        .expect("render_page_raw with a rect");
    let u32_at = |o: usize| u32::from_le_bytes(cropped[o..o + 4].try_into().unwrap());
    assert_eq!((u32_at(8), u32_at(12)), (200, 100));
}

/// The tile cache is keyed by generation and serves the protocol thread without pdfium.
#[test]
fn render_cache_keys_include_generation() {
    let doc = open("tracemonkey.pdf");
    let cache = &engine().shared.tiles;
    let k = key(&doc, RenderKind::Tile, 100, 0, 0);
    cache.insert(
        k.clone(),
        std::sync::Arc::new(seepdf_lib::engine::render::cache::EncodedImage {
            png: vec![0u8; 1024],
            width: 512,
            height: 512,
            render_ms: 0.0,
            encode_ms: 0.0,
        }),
    );
    assert!(cache.get(&k).is_some());

    let mut newer = k.clone();
    newer.generation += 1;
    assert!(cache.get(&newer).is_none(), "a new generation misses");

    cache.drop_older_generations(&doc.doc_id, k.generation + 1);
    assert!(cache.get(&k).is_none(), "the bump swept the dead entry");
}

/// P1-10 야간 모드: `night` changes **only** the clear colour (ARCHITECTURE §3.4). The page
/// background comes out transparent, so the page shell's night paper shows through, and the
/// ink is *not* inverted by the engine — the CSS filter on `.page-bitmaps` is the one and
/// only colour transform, so nothing on the page is ever inverted twice.
#[test]
fn render_night_is_transparent_not_inverted() {
    let doc = open("tracemonkey.pdf");
    // 3x, so glyph stems have solid interiors to compare (at 1x most ink is anti-aliasing).
    let day_key = key(&doc, RenderKind::Page, 300, 0, 0);
    let night_key = TileKey {
        night: Night::Dark,
        ..day_key.clone()
    };
    let sepia_key = TileKey {
        night: Night::Sepia,
        ..day_key.clone()
    };
    let day = render(day_key);
    let night = render(night_key);
    let sepia = render(sepia_key);
    assert_eq!((day.width, day.height), (night.width, night.height));
    assert_eq!(
        night.pixels, sepia.pixels,
        "dark and sepia share one bitmap"
    );

    // The top-left corner is page margin: opaque white by day, fully transparent at night.
    assert_eq!(&day.pixels[0..4], &[255, 255, 255, 255]);
    assert_eq!(
        night.pixels[3], 0,
        "night renders on a transparent clear colour"
    );

    let mut white = 0usize;
    let mut cleared = 0usize;
    let mut ink = 0usize;
    let mut max_ink_delta = 0u8;
    for (d, n) in day.pixels.chunks_exact(4).zip(night.pixels.chunks_exact(4)) {
        if d == [255, 255, 255, 255] {
            white += 1;
            cleared += usize::from(n[3] == 0);
        }
        // Solid glyph interiors are opaque in both renders; their colour must be identical.
        if n[3] == 255 && d[0] < 64 && d[1] < 64 && d[2] < 64 {
            ink += 1;
            for c in 0..3 {
                max_ink_delta = max_ink_delta.max(d[c].abs_diff(n[c]));
            }
        }
    }
    assert!(ink > 1_000, "the page has solid ink ({ink} px)");
    assert!(
        max_ink_delta <= 2,
        "the engine must not invert or tint the ink (max channel delta {max_ink_delta})"
    );
    assert!(
        cleared * 100 >= white * 99,
        "the white background is transparent at night ({cleared} of {white} px)"
    );
}
