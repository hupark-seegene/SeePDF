//! The one render configuration and the four things it produces — `ARCHITECTURE.md` §3.2.
//!
//! Never the matrix / `clip()` path: any `translate`/`scale`/`apply_matrix`/`clip` on a
//! `PdfRenderConfig` silently sets `render_form_data = false`, so form fields stop being
//! drawn (render spike §3). Tiles use `set_origin(-tx, -ty)` into a tile-sized bitmap,
//! which pdfium clips for us and which was verified pixel-exact against a crop of the full
//! render (427 / 1,048,576 px differing, max channel delta 1).

use crate::engine::render::cache::{Night, RenderKind, TileKey};
use crate::engine::render::encode::RawImage;
use crate::engine::render::geometry;
use crate::engine::types::EngineState;
use crate::ipc::error::PdfiumResultExt;
use crate::ipc::{EngineError, ErrorCode};
use pdfium_render::prelude::*;
use std::time::Instant;

/// Everything the engine needs to produce one image; built by the protocol handler from the
/// URL, or by a command.
#[derive(Debug, Clone)]
pub struct RenderRequest {
    pub key: TileKey,
}

impl RenderRequest {
    pub fn new(key: TileKey) -> Self {
        Self { key }
    }
}

fn rotation_of(deg: u16) -> PdfPageRenderRotation {
    match deg % 360 {
        90 => PdfPageRenderRotation::Degrees90,
        180 => PdfPageRenderRotation::Degrees180,
        270 => PdfPageRenderRotation::Degrees270,
        _ => PdfPageRenderRotation::None,
    }
}

/// The only render configuration in the project.
fn base_config(rotation: u16, night: Night, hl: bool) -> PdfRenderConfig {
    let mut config = PdfRenderConfig::new()
        // The intrinsic /Rotate is already applied by pdfium; this is the *view* rotation.
        .rotate(rotation_of(rotation), true)
        .render_form_data(true)
        .render_annotations(true)
        // Subpixel AA is wrong once the webview composites the tile.
        .use_lcd_text_rendering(false)
        // Never BGR: as_rgba_bytes() costs 83.6 ms on a 2x page.
        .set_format(PdfBitmapFormat::BGRA)
        // pdfium then writes RGBA, so as_raw_bytes() needs no swizzle (0.16 ms).
        .set_reverse_byte_order(true);
    config = if night.is_on() {
        // Composite over the UI's dark background instead of inverting white.
        config.set_clear_color(PdfColor::new(0, 0, 0, 0))
    } else {
        config.set_clear_color(PdfColor::WHITE)
    };
    if hl {
        config = config.highlight_all_form_fields(PdfColor::new(0x2F, 0x6F, 0xED, 0x40));
    }
    config
}

/// Renders one tile / page / thumbnail / OCR image on the engine thread.
pub fn render(st: &mut EngineState<'_>, req: &RenderRequest) -> Result<RawImage, EngineError> {
    let key = &req.key;
    let doc = st
        .docs
        .get(&key.doc)
        .ok_or_else(|| EngineError::not_found(format!("unknown document '{}'", key.doc)))?;
    if doc.generation != key.generation {
        return Err(EngineError::stale(format!(
            "generation {} is not the document's current generation {}",
            key.generation, doc.generation
        )));
    }
    let geom = doc.geom(key.page)?.clone();

    let (target, origin) = match key.kind {
        RenderKind::Tile => {
            let s = geometry::scale_from_key(key.scale_key);
            let (w, h) = geometry::page_pixels(&geom, key.rotation, s);
            let (ox, oy, tw, th) = geometry::tile_rect(w, h, key.tx, key.ty).ok_or_else(|| {
                EngineError::invalid(format!(
                    "tile ({},{}) is outside the {w}x{h} page",
                    key.tx, key.ty
                ))
            })?;
            (Target::Scale(s, tw, th), Some((ox, oy)))
        }
        RenderKind::Page => {
            let s = geometry::scale_from_key(key.scale_key);
            let (w, h) = geometry::page_pixels(&geom, key.rotation, s);
            (Target::Scale(s, w, h), None)
        }
        RenderKind::Thumb => {
            let width = key.scale_key.clamp(16, 4096);
            (Target::Thumb(width), None)
        }
        RenderKind::Ocr => {
            let s = (key.scale_key.clamp(72, 1200) as f32) / 72.0;
            let (w, h) = geometry::page_pixels(&geom, key.rotation, s);
            (Target::Scale(s, w, h), None)
        }
    };

    if let Target::Scale(_, w, h) = target {
        if w.saturating_mul(h) > geometry::HARD_MAX_PX {
            return Err(EngineError::invalid(format!(
                "{w}x{h} px exceeds the render ceiling"
            )));
        }
    }

    let mut config = base_config(key.rotation, key.night, key.hl);
    let grayscale = key.kind == RenderKind::Ocr;
    if grayscale {
        config = config.use_grayscale_rendering(true);
    }
    let (width, height) = match target {
        Target::Scale(s, w, h) => {
            config = config.scale_page_by_factor(s);
            (w, h)
        }
        Target::Thumb(w) => {
            // Never `thumbnail(n)`: it squashes the page to n x n and silently turns off
            // annotation and form rendering (pages spike §4).
            config = config
                .set_target_width(w as Pixels)
                .set_maximum_height((w * 2) as Pixels);
            let (w_pt, h_pt) = geometry::display_size_pt(&geom, key.rotation);
            let s = (w as f32) / w_pt;
            let s = s.min(((w * 2) as f32) / h_pt);
            (
                (w_pt * s).round().max(1.0) as u32,
                (h_pt * s).round().max(1.0) as u32,
            )
        }
    };
    if let Some((ox, oy)) = origin {
        config = config.set_origin(-(ox as Pixels), -(oy as Pixels));
    }

    let page_index = key.page;
    let doc = st
        .docs
        .get_mut(&key.doc)
        .ok_or_else(|| EngineError::not_found(format!("unknown document '{}'", key.doc)))?;
    let page = doc.page(page_index)?;

    let t0 = Instant::now();
    let mut bitmap = PdfBitmap::empty(width as Pixels, height as Pixels, PdfBitmapFormat::BGRA)
        .ctx("allocate bitmap")?;
    page.render_into_bitmap_with_config(&mut bitmap, &config)
        .ctx("render page")?;
    let pixels = bitmap.as_raw_bytes();
    let render_ms = t0.elapsed().as_secs_f64() * 1000.0;
    st.shared.stats.record_tile_ms(render_ms);

    Ok(RawImage {
        pixels,
        width,
        height,
        render_ms,
        grayscale,
    })
}

enum Target {
    /// `(scale, width_px, height_px)`.
    Scale(f32, u32, u32),
    /// Target width in px.
    Thumb(u32),
}

/// `render_page_raw` (`IPC_CONTRACT.md` §10.2): RGBA8 with a 32-byte `SPRX` header, for the
/// snapshot tool and the print path. Tiles never use this.
pub fn render_raw_buffer(
    st: &mut EngineState<'_>,
    doc_id: &str,
    page: u16,
    scale: f32,
    rect: Option<crate::ipc::types::Rect>,
) -> Result<Vec<u8>, EngineError> {
    let scale = scale.clamp(0.05, 16.0);
    let doc = st
        .docs
        .get(doc_id)
        .ok_or_else(|| EngineError::not_found(format!("unknown document '{doc_id}'")))?;
    let geom = doc.geom(page)?.clone();
    let (full_w, full_h) = geometry::page_pixels(&geom, 0, scale);

    // A `rect` in PDF points is cropped in device space with the same `set_origin` path.
    let (ox, oy, width, height) = match rect {
        None => (0, 0, full_w, full_h),
        Some(r) => {
            let m = geometry::page_to_device(&geom, 0, scale);
            let x0 = (m[0] * r.l + m[2] * r.t + m[4]).round().max(0.0) as u32;
            let y0 = (m[1] * r.l + m[3] * r.t + m[5]).round().max(0.0) as u32;
            let x1 = (m[0] * r.r + m[2] * r.b + m[4]).round().max(0.0) as u32;
            let y1 = (m[1] * r.r + m[3] * r.b + m[5]).round().max(0.0) as u32;
            let (x0, x1) = (x0.min(x1).min(full_w), x0.max(x1).min(full_w));
            let (y0, y1) = (y0.min(y1).min(full_h), y0.max(y1).min(full_h));
            if x1 <= x0 || y1 <= y0 {
                return Err(EngineError::invalid("rect is empty in device space"));
            }
            (x0, y0, x1 - x0, y1 - y0)
        }
    };
    if width.saturating_mul(height) > geometry::HARD_MAX_PX {
        return Err(EngineError::invalid("render_page_raw exceeds the ceiling"));
    }

    let mut config = base_config(0, Night::Off, false).scale_page_by_factor(scale);
    if (ox, oy) != (0, 0) {
        config = config.set_origin(-(ox as Pixels), -(oy as Pixels));
    }
    let doc = st
        .docs
        .get_mut(doc_id)
        .ok_or_else(|| EngineError::not_found(format!("unknown document '{doc_id}'")))?;
    let pdf_page = doc.page(page)?;
    let mut bitmap = PdfBitmap::empty(width as Pixels, height as Pixels, PdfBitmapFormat::BGRA)
        .ctx("allocate bitmap")?;
    pdf_page
        .render_into_bitmap_with_config(&mut bitmap, &config)
        .ctx("render page")?;
    let pixels = bitmap.as_raw_bytes();

    let stride = width * 4;
    let mut out = Vec::with_capacity(32 + pixels.len());
    out.extend_from_slice(b"SPRX");
    out.extend_from_slice(&1u16.to_le_bytes()); // version
    out.extend_from_slice(&1u16.to_le_bytes()); // format: 1 = RGBA8
    out.extend_from_slice(&width.to_le_bytes());
    out.extend_from_slice(&height.to_le_bytes());
    out.extend_from_slice(&stride.to_le_bytes());
    out.extend_from_slice(&[0u8; 12]); // reserved
    out.extend_from_slice(&pixels[..(stride as usize * height as usize).min(pixels.len())]);
    Ok(out)
}

/// One `set_target_width(32)` render per touched page, so pdfium writes the `/AP` streams
/// before a save (annotations spike §3.2). Stage 1 (b)'s save path calls this.
pub fn generate_appearances(st: &mut EngineState<'_>, doc_id: &str) -> Result<(), EngineError> {
    let pages: Vec<u16> = st.doc(doc_id)?.touched.iter().copied().collect();
    if pages.is_empty() {
        return Ok(());
    }
    let config = base_config(0, Night::Off, false).set_target_width(32);
    for page_index in pages {
        let doc = st.doc_mut(doc_id)?;
        let page = doc.page(page_index)?;
        let mut bitmap = PdfBitmap::empty(32, 64, PdfBitmapFormat::BGRA).ctx("allocate bitmap")?;
        page.render_into_bitmap_with_config(&mut bitmap, &config)
            .map_err(|e| {
                EngineError::new(
                    ErrorCode::Pdfium,
                    format!("appearance render of page {page_index}: {e:?}"),
                )
            })?;
    }
    st.doc_mut(doc_id)?.touched.clear();
    Ok(())
}
