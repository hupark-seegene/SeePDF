//! Raw PDFium calls the v0.3 redaction needs (pkg1: R1 image blanking, R2 word-level split,
//! R4 groups). Same rules as `engine::raw`: handles come from `raw_handle()` / `FPDFPage_GetObject`
//! on a live page, are borrowed for the call and never stored across an edit, and everything
//! runs on the engine thread. Kept beside the redaction code (instead of `engine/raw/`) so the
//! package stays inside the files it owns.

use crate::engine::raw::object::Matrix;
use crate::ipc::types::Rect;
use crate::ipc::{EngineError, ErrorCode};
use pdfium_render::prelude::{
    PdfDocument, PdfPage, PdfiumLibraryBindings, FPDF_IMAGEOBJ_METADATA, FPDF_PAGEOBJECT,
    FPDF_PAGEOBJECTMARK, FPDF_TEXTPAGE, FS_MATRIX,
};
use std::collections::HashMap;
use std::os::raw::{c_int, c_ulong};

/// `FPDF_PAGEOBJ_*` (public/fpdf_edit.h).
pub const OBJ_TEXT: c_int = 1;
pub const OBJ_PATH: c_int = 2;
pub const OBJ_IMAGE: c_int = 3;
pub const OBJ_FORM: c_int = 5;
/// `FPDF_SEGMENT_BEZIERTO`.
const SEGMENT_BEZIER: c_int = 1;

/// A page object handle as a plain key (for maps). Never dereferenced.
pub type HandleKey = usize;

pub fn key(handle: FPDF_PAGEOBJECT) -> HandleKey {
    handle as HandleKey
}

/// `FPDFPage_GetObject`.
pub fn object_at(
    bindings: &dyn PdfiumLibraryBindings,
    page: &PdfPage<'_>,
    index: usize,
) -> Result<FPDF_PAGEOBJECT, EngineError> {
    // SAFETY: `page` is live for this borrow; an out-of-range index returns null.
    let handle = unsafe { bindings.FPDFPage_GetObject(page.raw_handle(), index as c_int) };
    if handle.is_null() {
        return Err(EngineError::not_found(format!("page object {index}")));
    }
    Ok(handle)
}

/// `FPDFPage_CountObjects`.
pub fn object_count(bindings: &dyn PdfiumLibraryBindings, page: &PdfPage<'_>) -> usize {
    // SAFETY: `page` is live for this borrow.
    let n = unsafe { bindings.FPDFPage_CountObjects(page.raw_handle()) };
    n.max(0) as usize
}

/// Top-level handle → index, for mapping what a text page reports back to object ids.
pub fn index_map(
    bindings: &dyn PdfiumLibraryBindings,
    page: &PdfPage<'_>,
) -> HashMap<HandleKey, u32> {
    let mut out = HashMap::new();
    for i in 0..object_count(bindings, page) {
        if let Ok(h) = object_at(bindings, page, i) {
            out.insert(key(h), i as u32);
        }
    }
    out
}

/// `FPDFPageObj_GetType`.
pub fn object_type(bindings: &dyn PdfiumLibraryBindings, handle: FPDF_PAGEOBJECT) -> c_int {
    // SAFETY: `handle` is a live page object (from `object_at` or a form walk).
    unsafe { bindings.FPDFPageObj_GetType(handle) }
}

/// `FPDFPageObj_GetMatrix`; identity when PDFium refuses.
pub fn matrix(bindings: &dyn PdfiumLibraryBindings, handle: FPDF_PAGEOBJECT) -> Matrix {
    let mut m = FS_MATRIX {
        a: 1.0,
        b: 0.0,
        c: 0.0,
        d: 1.0,
        e: 0.0,
        f: 0.0,
    };
    // SAFETY: `m` outlives the call; `handle` is live.
    let ok = unsafe { bindings.FPDFPageObj_GetMatrix(handle, &mut m) };
    if !bindings.is_true(ok) {
        return [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];
    }
    [m.a, m.b, m.c, m.d, m.e, m.f]
}

/// `FPDFPageObj_SetMatrix`.
pub fn set_matrix(
    bindings: &dyn PdfiumLibraryBindings,
    handle: FPDF_PAGEOBJECT,
    m: Matrix,
) -> Result<(), EngineError> {
    let fs = FS_MATRIX {
        a: m[0],
        b: m[1],
        c: m[2],
        d: m[3],
        e: m[4],
        f: m[5],
    };
    // SAFETY: `fs` outlives the call; `handle` is live.
    let ok = unsafe { bindings.FPDFPageObj_SetMatrix(handle, &fs) };
    if !bindings.is_true(ok) {
        return Err(EngineError::new(
            ErrorCode::Pdfium,
            "FPDFPageObj_SetMatrix failed",
        ));
    }
    Ok(())
}

/// `FPDFPageObj_GetBounds` in the object's own space (page space for a top-level object,
/// form space for a child of a form).
pub fn bounds(bindings: &dyn PdfiumLibraryBindings, handle: FPDF_PAGEOBJECT) -> Option<Rect> {
    let (mut l, mut b, mut r, mut t) = (0f32, 0f32, 0f32, 0f32);
    // SAFETY: the four out-params outlive the call; `handle` is live.
    let ok = unsafe { bindings.FPDFPageObj_GetBounds(handle, &mut l, &mut b, &mut r, &mut t) };
    bindings.is_true(ok).then(|| Rect::new(l, b, r, t))
}

/// `FPDFPageObj_HasTransparency`.
pub fn has_transparency(bindings: &dyn PdfiumLibraryBindings, handle: FPDF_PAGEOBJECT) -> bool {
    // SAFETY: `handle` is live.
    bindings.is_true(unsafe { bindings.FPDFPageObj_HasTransparency(handle) })
}

/// The length of the embedded font program of a text object's font (`FPDFFont_GetFontData`).
/// Zero for a Type3 font, which has no program — PDFium's content generator cannot write
/// Type3 text at all, so such a run is never re-emitted.
pub fn font_data_len(bindings: &dyn PdfiumLibraryBindings, handle: FPDF_PAGEOBJECT) -> usize {
    // SAFETY: `handle` is a live text object; the font belongs to the document.
    let font = unsafe { bindings.FPDFTextObj_GetFont(handle) };
    if font.is_null() {
        return 0;
    }
    let mut len: usize = 0;
    // SAFETY: a null buffer asks for the length only.
    let ok = unsafe { bindings.FPDFFont_GetFontData(font, std::ptr::null_mut(), 0, &mut len) };
    if bindings.is_true(ok) {
        len
    } else {
        0
    }
}

/// `FPDFText_SetPositions`: every character's offset from the object's origin, except the
/// first, in text space. After `set_text`, PDFium's content generator writes these as a `TJ`
/// with kerning, so they survive the save exactly (verified: `tests/redact.rs`).
pub fn set_positions(
    bindings: &dyn PdfiumLibraryBindings,
    handle: FPDF_PAGEOBJECT,
    positions: &[f32],
) -> Result<(), EngineError> {
    if positions.is_empty() {
        return Ok(());
    }
    // SAFETY: `positions` outlives the call and holds `len` floats.
    let ok = unsafe { bindings.FPDFText_SetPositions(handle, positions.as_ptr(), positions.len()) };
    if !bindings.is_true(ok) {
        return Err(EngineError::new(
            ErrorCode::Pdfium,
            "FPDFText_SetPositions failed",
        ));
    }
    Ok(())
}

/// Does a path have Bézier segments? (Outlined glyphs and logos do; rules, boxes and table
/// grids do not.)
pub fn path_has_curves(bindings: &dyn PdfiumLibraryBindings, handle: FPDF_PAGEOBJECT) -> bool {
    // SAFETY: `handle` is a live path object.
    let n = unsafe { bindings.FPDFPath_CountSegments(handle) };
    (0..n).any(|i| {
        // SAFETY: `i` is in range; a null segment is skipped.
        let seg = unsafe { bindings.FPDFPath_GetPathSegment(handle, i) };
        // SAFETY: `seg` is non-null and belongs to the live path.
        !seg.is_null() && unsafe { bindings.FPDFPathSegment_GetType(seg) } == SEGMENT_BEZIER
    })
}

/// `(bits per pixel, FPDF_COLORSPACE_*)` of an image object.
pub fn image_metadata(
    bindings: &dyn PdfiumLibraryBindings,
    page: &PdfPage<'_>,
    handle: FPDF_PAGEOBJECT,
) -> Option<(u32, c_int)> {
    let mut meta = FPDF_IMAGEOBJ_METADATA {
        width: 0,
        height: 0,
        horizontal_dpi: 0.0,
        vertical_dpi: 0.0,
        bits_per_pixel: 0,
        colorspace: 0,
        marked_content_id: 0,
    };
    // SAFETY: `meta` outlives the call; both handles are live.
    let ok =
        unsafe { bindings.FPDFImageObj_GetImageMetadata(handle, page.raw_handle(), &mut meta) };
    bindings
        .is_true(ok)
        .then_some((meta.bits_per_pixel, meta.colorspace))
}

/// A rasterisation of the image object with its mask and matrix applied
/// (`FPDFImageObj_GetRenderedBitmap`), as `(width, height, BGRA rows)`; roughly one pixel per
/// point. Used only as a coarse before/after comparison.
pub fn rendered_image(
    bindings: &dyn PdfiumLibraryBindings,
    document: &PdfDocument<'_>,
    page: &PdfPage<'_>,
    handle: FPDF_PAGEOBJECT,
) -> Option<(usize, usize, Vec<u8>)> {
    // SAFETY: document, page and object are live and belong together.
    let bitmap = unsafe {
        bindings.FPDFImageObj_GetRenderedBitmap(document.raw_handle(), page.raw_handle(), handle)
    };
    if bitmap.is_null() {
        return None;
    }
    // SAFETY: `bitmap` is ours until destroyed below.
    let (w, h, stride, format, buffer) = unsafe {
        (
            bindings.FPDFBitmap_GetWidth(bitmap).max(0) as usize,
            bindings.FPDFBitmap_GetHeight(bitmap).max(0) as usize,
            bindings.FPDFBitmap_GetStride(bitmap).max(0) as usize,
            bindings.FPDFBitmap_GetFormat(bitmap),
            bindings.FPDFBitmap_GetBuffer(bitmap) as *const u8,
        )
    };
    let bpp: usize = match format {
        1 => 1, // FPDFBitmap_Gray
        2 => 3, // FPDFBitmap_BGR
        3 | 4 => 4,
        _ => 0,
    };
    let out = if bpp == 0 || buffer.is_null() || w == 0 || h == 0 {
        None
    } else {
        let mut pixels = Vec::with_capacity(w * h * 4);
        for y in 0..h {
            // SAFETY: row `y` of a `stride`-wide buffer PDFium owns until the destroy below.
            let row = unsafe { std::slice::from_raw_parts(buffer.add(y * stride), w * bpp) };
            for x in 0..w {
                let p = &row[x * bpp..x * bpp + bpp];
                match bpp {
                    1 => pixels.extend_from_slice(&[p[0], p[0], p[0], 255]),
                    3 => pixels.extend_from_slice(&[p[0], p[1], p[2], 255]),
                    _ => pixels.extend_from_slice(&[p[0], p[1], p[2], p[3]]),
                }
            }
        }
        Some((w, h, pixels))
    };
    // SAFETY: created by `GetRenderedBitmap` for us; not used afterwards.
    unsafe { bindings.FPDFBitmap_Destroy(bitmap) };
    out
}

/// A decoded bitmap copied out of PDFium: `format` is `FPDFBitmap_*` (1 gray, 2 BGR,
/// 3 BGRx, 4 BGRA), rows are `stride` bytes apart.
#[derive(Debug, Clone)]
pub struct RawBitmap {
    pub width: usize,
    pub height: usize,
    pub stride: usize,
    pub format: c_int,
    pub data: Vec<u8>,
}

impl RawBitmap {
    /// Bytes per pixel of `format`, `0` for a format we do not handle.
    pub fn bpp(&self) -> usize {
        match self.format {
            1 => 1,
            2 => 3,
            3 | 4 => 4,
            _ => 0,
        }
    }
}

/// `FPDFImageObj_GetBitmap`: the image's own pixels, without its mask and matrix.
pub fn image_bitmap(
    bindings: &dyn PdfiumLibraryBindings,
    handle: FPDF_PAGEOBJECT,
) -> Option<RawBitmap> {
    // SAFETY: `handle` is a live image object; the bitmap is ours until destroyed below.
    let bitmap = unsafe { bindings.FPDFImageObj_GetBitmap(handle) };
    if bitmap.is_null() {
        return None;
    }
    // SAFETY: `bitmap` is live until the destroy below.
    let (width, height, stride, format, buffer) = unsafe {
        (
            bindings.FPDFBitmap_GetWidth(bitmap).max(0) as usize,
            bindings.FPDFBitmap_GetHeight(bitmap).max(0) as usize,
            bindings.FPDFBitmap_GetStride(bitmap).max(0) as usize,
            bindings.FPDFBitmap_GetFormat(bitmap),
            bindings.FPDFBitmap_GetBuffer(bitmap) as *const u8,
        )
    };
    let out = (!buffer.is_null() && width > 0 && height > 0).then(|| {
        // SAFETY: PDFium's buffer is `stride × height` bytes and alive until the destroy.
        let data = unsafe { std::slice::from_raw_parts(buffer, stride * height) }.to_vec();
        RawBitmap {
            width,
            height,
            stride,
            format,
            data,
        }
    });
    // SAFETY: ours; not used afterwards.
    unsafe { bindings.FPDFBitmap_Destroy(bitmap) };
    out
}

/// `FPDFImageObj_SetBitmap` with the page named, so PDFium drops its cached decode of the
/// image and the next render or `GetBitmap` sees the new pixels. Matrix, clip path and
/// graphics state are kept. PDFium writes the new stream as `/FlateDecode`.
pub fn set_bitmap(
    bindings: &dyn PdfiumLibraryBindings,
    page: &PdfPage<'_>,
    handle: FPDF_PAGEOBJECT,
    bitmap: &mut RawBitmap,
) -> Result<(), EngineError> {
    // SAFETY: `bitmap.data` outlives the wrapper, which is destroyed before returning.
    let wrapper = unsafe {
        bindings.FPDFBitmap_CreateEx(
            bitmap.width as c_int,
            bitmap.height as c_int,
            bitmap.format,
            bitmap.data.as_mut_ptr() as *mut std::ffi::c_void,
            bitmap.stride as c_int,
        )
    };
    if wrapper.is_null() {
        return Err(EngineError::new(
            ErrorCode::Pdfium,
            "FPDFBitmap_CreateEx failed",
        ));
    }
    let mut pages = [page.raw_handle()];
    // SAFETY: all handles are live; `pages` outlives the call; PDFium copies the pixels.
    let ok = unsafe { bindings.FPDFImageObj_SetBitmap(pages.as_mut_ptr(), 1, handle, wrapper) };
    // SAFETY: the wrapper does not own `bitmap.data`; freeing it leaves the buffer alone.
    unsafe { bindings.FPDFBitmap_Destroy(wrapper) };
    if !bindings.is_true(ok) {
        return Err(EngineError::new(
            ErrorCode::Pdfium,
            "FPDFImageObj_SetBitmap failed",
        ));
    }
    Ok(())
}

/// Removes the object at `index` from `page` and destroys it (`FPDFPage_RemoveObject` +
/// `FPDFPageObj_Destroy`).
pub fn remove_and_destroy(
    bindings: &dyn PdfiumLibraryBindings,
    page: &PdfPage<'_>,
    handle: FPDF_PAGEOBJECT,
) -> Result<(), EngineError> {
    // SAFETY: `handle` belongs to the live `page`; ownership passes to us.
    let ok = unsafe { bindings.FPDFPage_RemoveObject(page.raw_handle(), handle) };
    if !bindings.is_true(ok) {
        return Err(EngineError::new(
            ErrorCode::Pdfium,
            "FPDFPage_RemoveObject failed",
        ));
    }
    // SAFETY: removed above, so it is ours to free.
    unsafe { bindings.FPDFPageObj_Destroy(handle) };
    Ok(())
}

/// Moves the object at `from_index` of `from` (a throw-away second parse of a page of the
/// same document) into `to` at `at` (`FPDFPage_InsertObjectAtIndex`). Returns its handle.
pub fn transplant_at(
    bindings: &dyn PdfiumLibraryBindings,
    from: &PdfPage<'_>,
    from_index: usize,
    to: &PdfPage<'_>,
    at: usize,
) -> Result<FPDF_PAGEOBJECT, EngineError> {
    let handle = object_at(bindings, from, from_index)?;
    // SAFETY: `handle` belongs to the live `from`; ownership passes to us.
    let ok = unsafe { bindings.FPDFPage_RemoveObject(from.raw_handle(), handle) };
    if !bindings.is_true(ok) {
        return Err(EngineError::new(
            ErrorCode::Pdfium,
            "FPDFPage_RemoveObject failed",
        ));
    }
    insert_at(bindings, to, handle, at)?;
    Ok(handle)
}

/// `FPDFPage_InsertObjectAtIndex` (clamped to the end); the page takes ownership.
pub fn insert_at(
    bindings: &dyn PdfiumLibraryBindings,
    page: &PdfPage<'_>,
    handle: FPDF_PAGEOBJECT,
    at: usize,
) -> Result<(), EngineError> {
    let at = at.min(object_count(bindings, page));
    // SAFETY: `handle` is an unowned live object of the same document; `page` takes it.
    let ok = unsafe { bindings.FPDFPage_InsertObjectAtIndex(page.raw_handle(), handle, at) };
    if !bindings.is_true(ok) {
        // PDFium does not free a rejected object.
        // SAFETY: still unowned, so ours to free.
        unsafe { bindings.FPDFPageObj_Destroy(handle) };
        return Err(EngineError::new(
            ErrorCode::Pdfium,
            "FPDFPage_InsertObjectAtIndex failed",
        ));
    }
    Ok(())
}

/// `FPDF_OBJECT_STRING`.
const OBJECT_STRING: c_int = 3;

/// The content marks of a page object (`FPDFPageObj_CountMarks` / `FPDFPageObj_GetMark`).
/// PDFium shares one mark item between every object of a parse inside the same `BDC`, so a
/// change to a mark reaches all of them.
pub fn marks(
    bindings: &dyn PdfiumLibraryBindings,
    handle: FPDF_PAGEOBJECT,
) -> Vec<FPDF_PAGEOBJECTMARK> {
    // SAFETY: `handle` is a live page object.
    let n = unsafe { bindings.FPDFPageObj_CountMarks(handle) };
    (0..n.max(0))
        .filter_map(|i| {
            // SAFETY: `i` is in range; the mark lives with the object.
            let m = unsafe { bindings.FPDFPageObj_GetMark(handle, i as c_ulong) };
            (!m.is_null()).then_some(m)
        })
        .collect()
}

/// The string-valued parameters of a content mark (`/Alt`, `/ActualText`, `/E`, `/Lang`, …)
/// as `(key, value)`. Works for inline property lists and named ones (`/Properties`).
pub fn mark_strings(
    bindings: &dyn PdfiumLibraryBindings,
    mark: FPDF_PAGEOBJECTMARK,
) -> Vec<(String, String)> {
    // SAFETY: `mark` is live (from `marks`).
    let n = unsafe { bindings.FPDFPageObjMark_CountParams(mark) };
    let mut out = Vec::new();
    for i in 0..n.max(0) {
        let Some(key) = utf16_out(|buf, len, out_len| {
            // SAFETY: `buf` is null (length query) or `len` bytes; `mark` is live.
            unsafe {
                bindings.FPDFPageObjMark_GetParamKey(
                    mark,
                    i as c_ulong,
                    buf as *mut _,
                    len,
                    out_len,
                )
            }
        }) else {
            continue;
        };
        // SAFETY: `mark` is live.
        if unsafe { bindings.FPDFPageObjMark_GetParamValueType(mark, &key) } != OBJECT_STRING {
            continue;
        }
        let value = utf16_out(|buf, len, out_len| {
            // SAFETY: as above.
            unsafe {
                bindings.FPDFPageObjMark_GetParamStringValue(
                    mark,
                    &key,
                    buf as *mut _,
                    len,
                    out_len,
                )
            }
        });
        out.push((key, value.unwrap_or_default()));
    }
    out
}

/// `FPDFPageObjMark_RemoveParam` — marks the object dirty so the regenerated content writes
/// the property list without `key`.
pub fn remove_mark_param(
    bindings: &dyn PdfiumLibraryBindings,
    handle: FPDF_PAGEOBJECT,
    mark: FPDF_PAGEOBJECTMARK,
    key: &str,
) -> bool {
    // SAFETY: `mark` belongs to `handle`, which is live.
    bindings.is_true(unsafe { bindings.FPDFPageObjMark_RemoveParam(handle, mark, key) })
}

/// The two-call PDFium pattern for a UTF-16LE out-string: ask for the length, then fill.
fn utf16_out(call: impl Fn(*mut u8, c_ulong, *mut c_ulong) -> i32) -> Option<String> {
    let mut len: c_ulong = 0;
    if call(std::ptr::null_mut(), 0, &mut len) == 0 || len < 2 {
        return None;
    }
    let mut buf = vec![0u8; len as usize];
    if call(buf.as_mut_ptr(), len, &mut len) == 0 {
        return None;
    }
    crate::engine::raw::doc::decode_utf16le(&buf)
}

/// The children of a form object, in drawing order (`FPDFFormObj_CountObjects` /
/// `FPDFFormObj_GetObject`).
pub fn form_children(
    bindings: &dyn PdfiumLibraryBindings,
    form: FPDF_PAGEOBJECT,
) -> Vec<FPDF_PAGEOBJECT> {
    // SAFETY: `form` is a live form object.
    let n = unsafe { bindings.FPDFFormObj_CountObjects(form) };
    (0..n.max(0))
        .filter_map(|i| {
            // SAFETY: `i` is in range.
            let c = unsafe { bindings.FPDFFormObj_GetObject(form, i as _) };
            (!c.is_null()).then_some(c)
        })
        .collect()
}

/// One character of a text page, with the page object that drew it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RawChar {
    pub unicode: u32,
    pub generated: bool,
    /// Glyph origin (page space).
    pub origin: (f32, f32),
    /// Tight glyph box (page space).
    pub tight: Rect,
    /// The drawing text object's handle — a top-level object or a child of a form.
    pub object: HandleKey,
}

/// A text page built from the page's **in-memory** objects (`FPDFText_LoadPage`), so it sees
/// edits before the content is regenerated. Closed on drop.
pub struct TextPage<'a> {
    handle: FPDF_TEXTPAGE,
    bindings: &'a dyn PdfiumLibraryBindings,
}

impl<'a> TextPage<'a> {
    pub fn load(
        bindings: &'a dyn PdfiumLibraryBindings,
        page: &PdfPage<'_>,
    ) -> Result<Self, EngineError> {
        // SAFETY: `page` is live; the text page is closed on drop, before the page goes.
        let handle = unsafe { bindings.FPDFText_LoadPage(page.raw_handle()) };
        if handle.is_null() {
            return Err(EngineError::new(
                ErrorCode::Pdfium,
                "FPDFText_LoadPage failed",
            ));
        }
        Ok(Self { handle, bindings })
    }

    pub fn chars(&self) -> Vec<RawChar> {
        let b = self.bindings;
        // SAFETY: `self.handle` is live until drop; every index below is in range.
        let n = unsafe { b.FPDFText_CountChars(self.handle) }.max(0);
        let mut out = Vec::with_capacity(n as usize);
        for i in 0..n {
            // SAFETY: as above.
            let (unicode, generated, object) = unsafe {
                (
                    b.FPDFText_GetUnicode(self.handle, i),
                    b.FPDFText_IsGenerated(self.handle, i) == 1,
                    b.FPDFText_GetTextObject(self.handle, i),
                )
            };
            let (mut x, mut y) = (0f64, 0f64);
            // SAFETY: out-params outlive the call.
            unsafe { b.FPDFText_GetCharOrigin(self.handle, i, &mut x, &mut y) };
            let (mut l, mut r, mut bo, mut t) = (0f64, 0f64, 0f64, 0f64);
            // SAFETY: out-params outlive the call.
            let boxed =
                unsafe { b.FPDFText_GetCharBox(self.handle, i, &mut l, &mut r, &mut bo, &mut t) };
            let tight = if b.is_true(boxed) {
                Rect::new(l as f32, bo as f32, r as f32, t as f32)
            } else {
                Rect::new(x as f32, y as f32, x as f32, y as f32)
            };
            out.push(RawChar {
                unicode,
                generated,
                origin: (x as f32, y as f32),
                tight,
                object: key(object),
            });
        }
        out
    }
}

impl Drop for TextPage<'_> {
    fn drop(&mut self) {
        // SAFETY: opened in `load`, closed exactly once.
        unsafe { self.bindings.FPDFText_ClosePage(self.handle) };
    }
}
