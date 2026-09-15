//! Scale ladder, page pixel size and the tile grid — `ARCHITECTURE.md` §3.1.
//!
//! The pixel arithmetic here is **exactly** pdfium-render's own
//! (`render_config.rs::apply_to_page`): `round(points_as_f32 * s)`, with width and height
//! swapped when a 90°/270° view rotation is applied with rotate-constraints. The frontend
//! mirrors it with `Math.fround`, and the engine's `X-Image-Width/Height` headers win on a
//! tie.

use crate::ipc::types::PageGeom;

/// Device-pixel edge of one tile.
pub const TILE: u32 = 512;
/// Whole-page bitmaps are used while `s <= 2.0` **and** the page is at most this many pixels.
pub const MAX_WHOLE_PAGE_PX: u32 = 2_500_000;
/// Above this device scale the viewer switches to tiles.
pub const MAX_WHOLE_PAGE_SCALE: f32 = 2.0;
/// `scaleKey` ceiling; above it the frontend CSS-upscales (`ARCHITECTURE.md` §3.1).
pub const MAX_SCALE_KEY: u32 = 800;
/// Refuse anything that would allocate more than this, whatever the caller asked for.
pub const HARD_MAX_PX: u32 = 40_000_000;

/// `s = scaleKey / 100` (`scaleKey = round(zoomPercent * devicePixelRatio)`).
pub fn scale_from_key(scale_key: u32) -> f32 {
    scale_key as f32 / 100.0
}

/// Display size of a page under a view rotation, in points.
pub fn display_size_pt(geom: &PageGeom, view_rotation: u16) -> (f32, f32) {
    if matches!(view_rotation % 360, 90 | 270) {
        (geom.height_pt, geom.width_pt)
    } else {
        (geom.width_pt, geom.height_pt)
    }
}

/// Pixel size of a whole page at device scale `s`.
pub fn page_pixels(geom: &PageGeom, view_rotation: u16, s: f32) -> (u32, u32) {
    let (w_pt, h_pt) = display_size_pt(geom, view_rotation);
    (
        ((w_pt * s).round() as i64).clamp(1, u32::MAX as i64) as u32,
        ((h_pt * s).round() as i64).clamp(1, u32::MAX as i64) as u32,
    )
}

/// Whole page, or tiles? (`ARCHITECTURE.md` D5)
pub fn should_tile(s: f32, width_px: u32, height_px: u32) -> bool {
    s > MAX_WHOLE_PAGE_SCALE || width_px.saturating_mul(height_px) > MAX_WHOLE_PAGE_PX
}

/// Number of tiles across and down for a page of `width_px` × `height_px`.
pub fn tile_grid(width_px: u32, height_px: u32) -> (u32, u32) {
    (width_px.div_ceil(TILE), height_px.div_ceil(TILE))
}

/// Origin and size of tile `(tx, ty)`, clipped to the page. `None` when the tile is
/// entirely outside the page (a 400, not an empty image).
pub fn tile_rect(width_px: u32, height_px: u32, tx: u32, ty: u32) -> Option<(u32, u32, u32, u32)> {
    let ox = tx.checked_mul(TILE)?;
    let oy = ty.checked_mul(TILE)?;
    if ox >= width_px || oy >= height_px {
        return None;
    }
    Some((ox, oy, TILE.min(width_px - ox), TILE.min(height_px - oy)))
}

/// Chebyshev distance of a tile from the viewport centre — the `Lane::Interactive`
/// priority, so visible tiles are rendered centre-out.
pub fn tile_priority(tx: u32, ty: u32, centre_tx: u32, centre_ty: u32) -> u32 {
    tx.abs_diff(centre_tx).max(ty.abs_diff(centre_ty))
}

/// Page → device matrix `[a b c d e f]` (`dx = a*x + c*y + e`, `dy = b*x + d*y + f`) for
/// PDF user space at scale `s`, built from the **crop box** plus `/Rotate` plus the view
/// rotation. Verified against `FPDF_PageToDevice` for 0° and 90° (text spike §2); the
/// frontend implements the same function in `src/viewer/geometry.ts`.
pub fn page_to_device(geom: &PageGeom, view_rotation: u16, s: f32) -> [f32; 6] {
    let deg = (geom.rotation as u32 + view_rotation as u32) % 360;
    let (l, b, r, t) = (geom.crop.l, geom.crop.b, geom.crop.r, geom.crop.t);
    match deg {
        90 => [0.0, s, s, 0.0, -b * s, -l * s],
        180 => [-s, 0.0, 0.0, s, r * s, -b * s],
        270 => [0.0, -s, -s, 0.0, t * s, r * s],
        _ => [s, 0.0, 0.0, -s, -l * s, t * s],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::types::Rect;

    fn geom(w: f32, h: f32, rotation: u16, crop: Rect) -> PageGeom {
        PageGeom {
            index: 0,
            width_pt: w,
            height_pt: h,
            rotation,
            crop,
            label: None,
        }
    }

    #[test]
    fn whole_page_pixels_match_pdfium_arithmetic() {
        let g = geom(612.0, 792.0, 0, Rect::new(0.0, 0.0, 612.0, 792.0));
        assert_eq!(page_pixels(&g, 0, 1.0), (612, 792));
        assert_eq!(page_pixels(&g, 0, 2.0), (1224, 1584));
        // A 90 degree view rotation swaps the output dimensions.
        assert_eq!(page_pixels(&g, 90, 1.0), (792, 612));
    }

    #[test]
    fn tiles_clip_at_the_page_edge() {
        let (w, h) = (1300u32, 1700u32);
        assert_eq!(tile_grid(w, h), (3, 4));
        assert_eq!(tile_rect(w, h, 0, 0), Some((0, 0, 512, 512)));
        assert_eq!(tile_rect(w, h, 2, 3), Some((1024, 1536, 276, 164)));
        assert_eq!(tile_rect(w, h, 3, 0), None);
    }

    #[test]
    fn matrix_maps_the_crop_box_corners() {
        let g = geom(612.0, 792.0, 0, Rect::new(0.0, 0.0, 612.0, 792.0));
        let m = page_to_device(&g, 0, 2.0);
        // Bottom-left of the crop box is the bottom-left of the device image.
        let (dx, dy) = (m[0] * 0.0 + m[2] * 0.0 + m[4], m[1] * 0.0 + m[3] * 0.0 + m[5]);
        assert_eq!((dx, dy), (0.0, 1584.0));
        // Top-right maps to (width, 0).
        let (dx, dy) = (
            m[0] * 612.0 + m[2] * 792.0 + m[4],
            m[1] * 612.0 + m[3] * 792.0 + m[5],
        );
        assert_eq!((dx, dy), (1224.0, 0.0));
    }
}
