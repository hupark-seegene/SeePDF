//! `FPDF_RenderPageBitmap` without the form-fill overlay — the `forms=0` render (F-20).
//!
//! pdfium-render always pairs `FPDF_RenderPageBitmap` with `FPDF_FFLDraw` when the document
//! has a form-fill environment, and its only way to skip `FPDF_FFLDraw` — `render_form_data(false)` —
//! switches the whole call to `FPDF_RenderPageBitmapWithMatrix`, which has no `start_x` /
//! `start_y` and therefore cannot render a 512 px tile of a larger page. Tiles are the one
//! thing SeePDF renders most, so instead of bending the matrix path we make the FFI call
//! ourselves, with exactly the arguments pdfium-render would have passed minus `FPDF_FFLDraw`.
//!
//! PDFium's own contract for the flag set (`fpdfview.h`): *"With the `FPDF_ANNOT` flag, it
//! renders all annotations that do not require user-interaction, which are **all annotations
//! except widget and popup annotations**"* — so dropping `FPDF_FFLDraw` removes every trace of
//! the AcroForm widgets (value text, background wash, borders, pushbutton captions) and leaves
//! the page's own content untouched. Measured on `fixtures/out/stage2/form-filled.pdf` at 2×:
//! 391 549 of 2 002 770 pixels differ between the two renders, all of them inside widget rects.
//!
//! Caller contract: engine thread, live `FPDF_PAGE`, and the returned buffer is RGBA8 (the
//! `FPDF_REVERSE_BYTE_ORDER` flag is part of `flags`, exactly as in the pdfium-render path).

use crate::ipc::{EngineError, ErrorCode};
use pdfium_render::prelude::{FPDF_BITMAP, FPDF_DWORD, PdfPage, PdfiumLibraryBindings};
use std::os::raw::{c_int, c_void};

/// `FPDFBitmap_BGRA` — 4 bytes per pixel. The only format SeePDF renders into.
const FPDF_BITMAP_BGRA: c_int = 4;

/// One page render into a caller-owned RGBA8 buffer, skipping `FPDF_FFLDraw`.
///
/// * `bitmap_w` / `bitmap_h` — the destination bitmap (a tile, or the whole page);
/// * `start_x` / `start_y` — where the page's top-left corner lands in that bitmap
///   (negative for a tile, i.e. `-tile_origin`);
/// * `size_x` / `size_y` — the size of the **whole** scaled page, as pdfium-render's
///   `output_width` / `output_height`;
/// * `rotate` — 0..=3, PDFium's own quarter-turn encoding;
/// * `flags` — `FPDF_ANNOT | FPDF_REVERSE_BYTE_ORDER [| FPDF_GRAYSCALE]`;
/// * `clear_color` — `0xAARRGGBB`, pre-filled before the render like pdfium-render does.
#[allow(clippy::too_many_arguments)]
pub fn render_page_without_form_data(
    bindings: &dyn PdfiumLibraryBindings,
    page: &PdfPage<'_>,
    bitmap_w: u32,
    bitmap_h: u32,
    start_x: i32,
    start_y: i32,
    size_x: u32,
    size_y: u32,
    rotate: c_int,
    flags: c_int,
    clear_color: FPDF_DWORD,
) -> Result<Vec<u8>, EngineError> {
    let stride = (bitmap_w as usize)
        .checked_mul(4)
        .ok_or_else(|| EngineError::invalid("bitmap width overflows"))?;
    let len = stride
        .checked_mul(bitmap_h as usize)
        .ok_or_else(|| EngineError::invalid("bitmap size overflows"))?;
    let mut buffer = vec![0u8; len];

    // SAFETY: `buffer` is `stride * height` bytes of our own allocation and outlives every
    // call below; PDFium writes into it and never frees it (`FPDFBitmap_Destroy` does not
    // touch an external buffer). The page handle is live and owned by this thread.
    let bitmap: FPDF_BITMAP = unsafe {
        bindings.FPDFBitmap_CreateEx(
            bitmap_w as c_int,
            bitmap_h as c_int,
            FPDF_BITMAP_BGRA,
            buffer.as_mut_ptr() as *mut c_void,
            stride as c_int,
        )
    };
    if bitmap.is_null() {
        return Err(EngineError::new(
            ErrorCode::Pdfium,
            format!("FPDFBitmap_CreateEx({bitmap_w}x{bitmap_h}) returned null"),
        ));
    }

    // SAFETY: `bitmap` is a live handle created immediately above; the page is live.
    unsafe {
        bindings.FPDFBitmap_FillRect(
            bitmap,
            0,
            0,
            bitmap_w as c_int,
            bitmap_h as c_int,
            clear_color,
        );
        bindings.FPDF_RenderPageBitmap(
            bitmap,
            page.raw_handle(),
            start_x as c_int,
            start_y as c_int,
            size_x as c_int,
            size_y as c_int,
            rotate,
            flags,
        );
        bindings.FPDFBitmap_Destroy(bitmap);
    }
    Ok(buffer)
}
