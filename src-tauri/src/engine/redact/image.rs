//! R1 (v0.3): blanking the pixels of a partly covered image — the scanned-page case.
//!
//! Before v0.3 an image that crossed a mark kept every pixel under the black box, so the scan
//! of a 주민등록번호 was still in the saved file. Now the image's own bitmap is read back
//! (`FPDFImageObj_GetBitmap`), every image pixel whose footprint on the page overlaps a mark
//! is set to black, and the bitmap is written back into the **same** image object — matrix,
//! clip path and graphics state are kept, so nothing moves:
//!
//! * a `/DCTDecode` (or `/JPXDecode`) original is re-encoded as JPEG (quality
//!   [`JPEG_QUALITY`]) and swapped in with `FPDFImageObj_LoadJpegFileInline`, the way
//!   `compress.rs` does, so a scan does not grow tenfold as Flate;
//! * anything else goes through `FPDFImageObj_SetBitmap` (PDFium writes `/FlateDecode`).
//!
//! **Which pixels.** The image matrix maps the unit square onto the page; pixel `(x, y)` of a
//! `w × h` image (row 0 at the top) covers `[x/w, (x+1)/w] × [1-(y+1)/h, 1-y/h]` of it. A
//! pixel is blanked when that parallelogram, mapped to the page, overlaps a mark with positive
//! area (separating-axis test), so rotated, skewed and flipped images are handled exactly.
//!
//! **Verification**, both before anything is reported as done:
//!
//! 1. *data*: the bitmap is read back from the object after the write and every blanked pixel
//!    must be black — exactly `0` for Flate; for JPEG at most [`JPEG_MAX`] per channel with a
//!    mean of at most [`JPEG_MEAN_MAX`] (re-encoding rings a little at the edges of the black
//!    area; that ringing comes from the *unmarked* neighbours, never from the old content,
//!    which was zeroed before encoding). Rendering the marked rect would prove nothing: the
//!    fill box is drawn on top of it. A failure is `verifyFailed` and the command rolls back.
//! 2. *fidelity*: the image is rasterised one pixel per image pixel, mask applied
//!    (`FPDFImageObj_GetRenderedBitmap` with the matrix swapped for `[w 0 0 h 0 0]` for the
//!    call), before and after; about as many pixels as were blanked may change, no more.
//!    PDFium hands palette images and inverted `/Decode` arrays back as raw indices, which
//!    would repaint the whole image; when far more changed than was blanked, the image is
//!    removed whole instead (secure, but a visible change the preview could not predict).
//!    The same rasterisation's alpha is how the preview finds an `/SMask` or `/Mask`, which
//!    `FPDFPageObj_HasTransparency` does not report for images.
//!
//! **Not blanked, removed whole** (`RedactImageObject.blank == false`): images with
//! transparency (an `/SMask` would be dropped by `SetBitmap` and the image repainted opaque),
//! stencil masks and 1-bit images that are not gray, palette (`/Indexed`), `/Separation`,
//! `/DeviceN` and pattern images, and bitmaps PDFium cannot hand back.

use super::raw::{self, RawBitmap};
use crate::engine::raw::object::Matrix;
use crate::ipc::types::Rect;
use crate::ipc::{EngineError, ErrorCode};
use pdfium_render::prelude::{PdfDocument, PdfPage, PdfiumLibraryBindings, FPDF_PAGEOBJECT};

/// JPEG quality of a re-encoded `/DCTDecode` image (a little above `compress.rs`: this is not
/// meant to shrink the file).
pub const JPEG_QUALITY: u8 = 92;
/// A blanked JPEG pixel may read back at most this bright per channel…
pub const JPEG_MAX: u8 = 96;
/// …and the blanked area at most this bright on average.
pub const JPEG_MEAN_MAX: f64 = 16.0;
/// A rasterised pixel "changed" when a channel moved by more than this.
const CHANGED: u8 = 40;
/// At most this many times the blanked pixels may change in the one-pixel-per-pixel
/// rasterisation, plus [`CHANGED_SLACK`] of the image (resampling, JPEG noise).
const CHANGED_FACTOR: f64 = 1.5;
const CHANGED_SLACK: f64 = 0.03;

/// `FPDF_COLORSPACE_*` values whose images PDFium hands back as real gray or RGB samples.
fn plain_colour_space(cs: i32) -> bool {
    // DeviceGray, DeviceRGB, DeviceCMYK, CalGray, CalRGB, Lab, ICCBased.
    (1..=7).contains(&cs)
}

fn gray_colour_space(cs: i32) -> bool {
    matches!(cs, 1 | 4)
}

/// Can this image be blanked in place (vs. removed whole)? Static checks only — the preview
/// uses it, so it never mutates.
///
/// `page` must be a page whose objects may be touched without consequence (a throw-away
/// parse): the mask probe swaps the object's matrix for a moment ([`processed`]).
pub fn can_blank(
    bindings: &dyn PdfiumLibraryBindings,
    document: &PdfDocument<'_>,
    page: &PdfPage<'_>,
    handle: FPDF_PAGEOBJECT,
) -> bool {
    if raw::has_transparency(bindings, handle) {
        return false;
    }
    let Some((bpp, cs)) = raw::image_metadata(bindings, page, handle) else {
        return false;
    };
    if !plain_colour_space(cs) {
        return false;
    }
    if bpp < 8 && !(bpp == 1 && gray_colour_space(cs)) {
        return false;
    }
    let Some(bitmap) = raw::image_bitmap(bindings, handle) else {
        return false;
    };
    if bitmap.bpp() == 0 {
        return false;
    }
    // `FPDFPageObj_HasTransparency` does not look at an image's own `/SMask` or `/Mask`;
    // the rasterisation's alpha does.
    !processed(
        bindings,
        document,
        page,
        handle,
        bitmap.width,
        bitmap.height,
    )
    .is_some_and(|px| px.chunks_exact(4).any(|p| p[3] < 250))
}

/// The image rasterised one pixel per image pixel with its mask applied (BGRA rows): the
/// object's matrix is swapped for `[w, 0, 0, h, 0, 0]` for the call and restored after.
/// `None` when PDFium hands back another size.
fn processed(
    bindings: &dyn PdfiumLibraryBindings,
    document: &PdfDocument<'_>,
    page: &PdfPage<'_>,
    handle: FPDF_PAGEOBJECT,
    w: usize,
    h: usize,
) -> Option<Vec<u8>> {
    let m = raw::matrix(bindings, handle);
    raw::set_matrix(bindings, handle, [w as f32, 0.0, 0.0, h as f32, 0.0, 0.0]).ok()?;
    let out = raw::rendered_image(bindings, document, page, handle);
    raw::set_matrix(bindings, handle, m).ok()?;
    let (rw, rh, px) = out?;
    (rw == w && rh == h).then_some(px)
}

/// Outcome of [`blank`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Blanked {
    /// The pixels under the marks are black and verified; the rest is unchanged.
    Done,
    /// Nothing under the marks after all (only the bounds overlapped).
    Nothing,
    /// The re-encode would repaint the image outside the marks: remove it whole instead.
    Unfaithful,
}

/// Blanks the pixels of the image object at `index` under `rects` and verifies the result
/// (module docs). `jpeg` = the original is `/DCTDecode` or `/JPXDecode`.
pub fn blank(
    bindings: &dyn PdfiumLibraryBindings,
    document: &PdfDocument<'_>,
    page: &PdfPage<'_>,
    index: usize,
    rects: &[Rect],
    jpeg: bool,
) -> Result<Blanked, EngineError> {
    let handle = raw::object_at(bindings, page, index)?;
    let m = raw::matrix(bindings, handle);
    let Some(mut bitmap) = raw::image_bitmap(bindings, handle) else {
        return Ok(Blanked::Unfaithful);
    };
    let bpp = bitmap.bpp();
    if bpp == 0 {
        return Ok(Blanked::Unfaithful);
    }
    let covered = covered_pixels(m, bitmap.width, bitmap.height, rects);
    if covered.is_empty() {
        return Ok(Blanked::Nothing);
    }
    let (w, h) = (bitmap.width, bitmap.height);
    let before = processed(bindings, document, page, handle, w, h);

    for &(x, y) in &covered {
        let o = y * bitmap.stride + x * bpp;
        for byte in &mut bitmap.data[o..o + bpp.min(3)] {
            *byte = 0;
        }
        if bpp == 4 {
            // BGRx / BGRA: opaque black.
            bitmap.data[o + 3] = 255;
        }
    }

    if jpeg {
        let encoded = encode_jpeg(&bitmap)?;
        crate::engine::raw::object::replace_with_jpeg(bindings, page, index, &encoded)?;
    } else {
        raw::set_bitmap(bindings, page, handle, &mut bitmap)?;
    }

    // 1. the data really is black under the marks.
    let handle = raw::object_at(bindings, page, index)?;
    let Some(back) = raw::image_bitmap(bindings, handle) else {
        return Err(verify_failed("the blanked image could not be read back"));
    };
    if back.width != bitmap.width || back.height != bitmap.height || back.bpp() == 0 {
        return Err(verify_failed("the blanked image changed size"));
    }
    check_black(&back, &covered, jpeg)?;

    // 2. …and not much more than that changed (palette / Decode surprises).
    if let (Some(before), Some(after)) = (before, processed(bindings, document, page, handle, w, h))
    {
        let changed = before
            .chunks_exact(4)
            .zip(after.chunks_exact(4))
            .filter(|(p, q)| (0..4).any(|k| p[k].abs_diff(q[k]) > CHANGED))
            .count();
        let allowed = covered.len() as f64 * CHANGED_FACTOR + (w * h) as f64 * CHANGED_SLACK;
        if changed as f64 > allowed {
            return Ok(Blanked::Unfaithful);
        }
    }
    Ok(Blanked::Done)
}

fn verify_failed(message: &str) -> EngineError {
    EngineError::new(ErrorCode::VerifyFailed, message.to_string())
}

/// Every blanked pixel reads back black (module docs, verification 1).
fn check_black(
    back: &RawBitmap,
    covered: &[(usize, usize)],
    jpeg: bool,
) -> Result<(), EngineError> {
    let bpp = back.bpp();
    let channels = bpp.min(3);
    let mut sum: u64 = 0;
    let mut max: u8 = 0;
    for &(x, y) in covered {
        let o = y * back.stride + x * bpp;
        for &v in &back.data[o..o + channels] {
            max = max.max(v);
            sum += v as u64;
        }
    }
    let mean = sum as f64 / (covered.len() * channels).max(1) as f64;
    let ok = if jpeg {
        max <= JPEG_MAX && mean <= JPEG_MEAN_MAX
    } else {
        max == 0
    };
    if !ok {
        return Err(verify_failed(&format!(
            "image pixels under the marks are not blank (max {max}, mean {mean:.1})"
        )));
    }
    Ok(())
}

/// Encodes a bitmap as a baseline JPEG (gray stays gray).
fn encode_jpeg(bitmap: &RawBitmap) -> Result<Vec<u8>, EngineError> {
    let (w, h, bpp) = (bitmap.width, bitmap.height, bitmap.bpp());
    let mut out = Vec::new();
    let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, JPEG_QUALITY);
    let result = if bpp == 1 {
        let mut luma = image::GrayImage::new(w as u32, h as u32);
        for (y, row) in luma.rows_mut().enumerate() {
            for (x, px) in row.enumerate() {
                px.0[0] = bitmap.data[y * bitmap.stride + x];
            }
        }
        encoder.encode_image(&luma)
    } else {
        let mut rgb = image::RgbImage::new(w as u32, h as u32);
        for (y, row) in rgb.rows_mut().enumerate() {
            for (x, px) in row.enumerate() {
                let o = y * bitmap.stride + x * bpp;
                px.0 = [bitmap.data[o + 2], bitmap.data[o + 1], bitmap.data[o]];
            }
        }
        encoder.encode_image(&rgb)
    };
    result.map_err(|e| EngineError::new(ErrorCode::Pdfium, format!("encode JPEG: {e}")))?;
    Ok(out)
}

/// `m` applied to a unit-square point.
fn apply(m: Matrix, u: f32, v: f32) -> (f32, f32) {
    (m[0] * u + m[2] * v + m[4], m[1] * u + m[3] * v + m[5])
}

/// The inverse of `m`, `None` when it is singular.
fn invert(m: Matrix) -> Option<Matrix> {
    let det = m[0] * m[3] - m[1] * m[2];
    if det.abs() < 1e-9 {
        return None;
    }
    let (a, b, c, d) = (m[3] / det, -m[1] / det, -m[2] / det, m[0] / det);
    Some([a, b, c, d, -(m[4] * a + m[5] * c), -(m[4] * b + m[5] * d)])
}

/// The pixels of a `w × h` image drawn with matrix `m` whose footprint overlaps any of
/// `rects` with positive area, as `(x, y)` with row 0 at the top. Deduplicated, row-major.
pub fn covered_pixels(m: Matrix, w: usize, h: usize, rects: &[Rect]) -> Vec<(usize, usize)> {
    let Some(inv) = invert(m) else {
        return Vec::new();
    };
    let (wf, hf) = (w as f32, h as f32);
    let mut hit = vec![false; w * h];
    for r in rects {
        // The mark's corners in pixel space bound the candidates.
        let corners = [(r.l, r.b), (r.r, r.b), (r.r, r.t), (r.l, r.t)];
        let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for (px, py) in corners {
            let (u, v) = apply(inv, px, py);
            let (x, y) = (u * wf, (1.0 - v) * hf);
            x0 = x0.min(x);
            y0 = y0.min(y);
            x1 = x1.max(x);
            y1 = y1.max(y);
        }
        let xs = (x0.floor() - 1.0).max(0.0) as usize
            ..=((x1.ceil() + 1.0).min(wf - 1.0).max(0.0) as usize);
        let ys = (y0.floor() - 1.0).max(0.0) as usize
            ..=((y1.ceil() + 1.0).min(hf - 1.0).max(0.0) as usize);
        if x1 < 0.0 || y1 < 0.0 || x0 > wf || y0 > hf {
            continue;
        }
        for y in ys {
            for x in xs.clone() {
                if hit[y * w + x] {
                    continue;
                }
                let (u0, u1) = (x as f32 / wf, (x + 1) as f32 / wf);
                let (v0, v1) = (1.0 - (y + 1) as f32 / hf, 1.0 - y as f32 / hf);
                let quad = [
                    apply(m, u0, v0),
                    apply(m, u1, v0),
                    apply(m, u1, v1),
                    apply(m, u0, v1),
                ];
                if quad_overlaps_rect(&quad, r) {
                    hit[y * w + x] = true;
                }
            }
        }
    }
    let mut out = Vec::new();
    for y in 0..h {
        for x in 0..w {
            if hit[y * w + x] {
                out.push((x, y));
            }
        }
    }
    out
}

/// Separating-axis test between a convex quad (a mapped pixel) and an axis-aligned rect;
/// `true` only for an overlap of positive area.
fn quad_overlaps_rect(q: &[(f32, f32); 4], r: &Rect) -> bool {
    const EPS: f32 = 1e-4;
    let (mut qx0, mut qy0, mut qx1, mut qy1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for &(x, y) in q {
        qx0 = qx0.min(x);
        qy0 = qy0.min(y);
        qx1 = qx1.max(x);
        qy1 = qy1.max(y);
    }
    if qx1 <= r.l + EPS || qx0 >= r.r - EPS || qy1 <= r.b + EPS || qy0 >= r.t - EPS {
        return false;
    }
    let rc = [(r.l, r.b), (r.r, r.b), (r.r, r.t), (r.l, r.t)];
    for i in 0..2 {
        let (ax, ay) = q[i];
        let (bx, by) = q[i + 1];
        let n = (-(by - ay), bx - ax);
        let len = (n.0 * n.0 + n.1 * n.1).sqrt();
        if len < 1e-12 {
            continue;
        }
        let proj = |p: (f32, f32)| (p.0 * n.0 + p.1 * n.1) / len;
        let (mut q0, mut q1) = (f32::MAX, f32::MIN);
        for &p in q {
            let d = proj(p);
            q0 = q0.min(d);
            q1 = q1.max(d);
        }
        let (mut r0, mut r1) = (f32::MAX, f32::MIN);
        for &p in &rc {
            let d = proj(p);
            r0 = r0.min(d);
            r1 = r1.max(d);
        }
        if q1 <= r0 + EPS || r1 <= q0 + EPS {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn axis_aligned_pixels_under_a_mark() {
        // A 10×10 image on a 100×100 pt square at (0, 0): one pixel = 10 pt.
        let m = [100.0, 0.0, 0.0, 100.0, 0.0, 0.0];
        // The bottom-left 20×20 pt: pixels x 0..2, rows 8..10 (row 0 is the top).
        let px = covered_pixels(m, 10, 10, &[Rect::new(0.0, 0.0, 20.0, 20.0)]);
        assert_eq!(px, vec![(0, 8), (1, 8), (0, 9), (1, 9)]);
        // A mark that only touches a pixel edge takes nothing from the neighbour.
        let px = covered_pixels(m, 10, 10, &[Rect::new(20.0, 20.0, 30.0, 30.0)]);
        assert_eq!(px, vec![(2, 7)]);
    }

    #[test]
    fn rotated_and_flipped_matrices() {
        // 90° rotation: the unit square's u axis points up the page.
        let m = [0.0, 100.0, -100.0, 0.0, 100.0, 0.0];
        let px = covered_pixels(m, 10, 10, &[Rect::new(90.0, 0.0, 100.0, 10.0)]);
        // Page (95, 5): u = 0.05, v = 0.05 → x 0, row 9.
        assert_eq!(px, vec![(0, 9)]);
        // A vertical flip puts row 0 at the bottom.
        let flipped = [100.0, 0.0, 0.0, -100.0, 0.0, 100.0];
        let px = covered_pixels(flipped, 10, 10, &[Rect::new(0.0, 0.0, 10.0, 10.0)]);
        assert_eq!(px, vec![(0, 0)]);
    }

    #[test]
    fn a_mark_off_the_image_covers_nothing() {
        let m = [100.0, 0.0, 0.0, 100.0, 0.0, 0.0];
        assert!(covered_pixels(m, 10, 10, &[Rect::new(200.0, 200.0, 300.0, 300.0)]).is_empty());
        assert!(covered_pixels([0.0; 6], 10, 10, &[Rect::new(0.0, 0.0, 1.0, 1.0)]).is_empty());
    }
}
