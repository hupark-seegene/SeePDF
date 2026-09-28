//! Page-object raw calls: content marks, shared Form XObjects and in-place JPEG replacement.
//!
//! pdfium-render 0.9.4 keeps `object_handle()` `pub(crate)`, so every function here reaches the
//! object through its **page and index** (`FPDFPage_GetObject`) instead — the handle is borrowed
//! for the duration of the call and never stored. Used by `engine::stamp` (P1-4) and
//! `engine::compress` (P1-5).
//!
//! **Caller contract:** the page must have been opened with the `Manual` regeneration strategy
//! (`ScratchPage`) and the caller regenerates the content once at the end; objects inserted
//! here bypass pdfium-render's collection, which holds no cache of its own, so the high-level
//! `page.objects()` sees them immediately.

use crate::ipc::{EngineError, ErrorCode};
use pdfium_render::prelude::{
    PdfDocument, PdfPage, PdfiumLibraryBindings, FPDF_DOCUMENT, FPDF_FILEACCESS, FPDF_PAGE,
    FPDF_PAGEOBJECT, FPDF_PAGEOBJECTMARK, FPDF_XOBJECT, FS_MATRIX,
};
use std::ffi::c_void;
use std::marker::PhantomData;
use std::os::raw::{c_int, c_uchar, c_ulong};

/// `[a, b, c, d, e, f]` — PDF row-vector convention: `x' = a·x + c·y + e`, `y' = b·x + d·y + f`.
pub type Matrix = [f32; 6];

/// `FPDFPage_CountObjects`.
pub fn object_count(bindings: &dyn PdfiumLibraryBindings, page: &PdfPage<'_>) -> usize {
    // SAFETY: `page` is live for this borrow, on the engine thread.
    let n = unsafe { bindings.FPDFPage_CountObjects(page.raw_handle()) };
    n.max(0) as usize
}

fn object_at(
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

/// A content mark to put on an object: `/name <</key (value) …>> BDC … EMC` once the content
/// is regenerated (`/name BMC … EMC` when there are no params).
#[derive(Debug, Clone, Copy)]
pub struct Mark<'a> {
    pub name: &'a str,
    pub params: &'a [(&'a str, &'a str)],
}

impl<'a> Mark<'a> {
    pub fn plain(name: &'a str) -> Self {
        Self { name, params: &[] }
    }
}

/// `FPDFPageObj_AddMark` (+ `FPDFPageObjMark_SetStringParam` per param) on the object at
/// `index` of `page`, which belongs to `document`.
pub fn add_mark(
    bindings: &dyn PdfiumLibraryBindings,
    document: &PdfDocument<'_>,
    page: &PdfPage<'_>,
    index: usize,
    mark: Mark<'_>,
) -> Result<(), EngineError> {
    let handle = object_at(bindings, page, index)?;
    mark_handle(bindings, document.raw_handle(), handle, mark)
}

fn mark_handle(
    bindings: &dyn PdfiumLibraryBindings,
    document: FPDF_DOCUMENT,
    handle: FPDF_PAGEOBJECT,
    mark: Mark<'_>,
) -> Result<(), EngineError> {
    // SAFETY: `handle` belongs to a live page (or is a fresh, not-yet-inserted object).
    let created = unsafe { bindings.FPDFPageObj_AddMark(handle, mark.name) };
    if created.is_null() {
        return Err(EngineError::new(ErrorCode::Pdfium, "FPDFPageObj_AddMark failed"));
    }
    for (key, value) in mark.params {
        // SAFETY: `document` owns `handle`'s page (or will); `created` lives with the object.
        let ok = unsafe {
            bindings.FPDFPageObjMark_SetStringParam(document, handle, created, key, value)
        };
        if !bindings.is_true(ok) {
            return Err(EngineError::new(
                ErrorCode::Pdfium,
                "FPDFPageObjMark_SetStringParam failed",
            ));
        }
    }
    Ok(())
}

/// The first mark called `name` on the object at `index`.
fn find_mark(
    bindings: &dyn PdfiumLibraryBindings,
    page: &PdfPage<'_>,
    index: usize,
    name: &str,
) -> Option<FPDF_PAGEOBJECTMARK> {
    let handle = object_at(bindings, page, index).ok()?;
    // SAFETY: as above.
    let count = unsafe { bindings.FPDFPageObj_CountMarks(handle) };
    (0..count).find_map(|i| {
        // SAFETY: `i` is in range; the mark handle lives as long as the object.
        let mark = unsafe { bindings.FPDFPageObj_GetMark(handle, i as c_ulong) };
        (!mark.is_null() && mark_name(bindings, mark).as_deref() == Some(name)).then_some(mark)
    })
}

/// Does the object at `index` carry a content mark called `name`?
pub fn has_mark(
    bindings: &dyn PdfiumLibraryBindings,
    page: &PdfPage<'_>,
    index: usize,
    name: &str,
) -> bool {
    find_mark(bindings, page, index, name).is_some()
}

/// `None` when the object at `index` has no mark called `name`; otherwise the mark's string
/// param `key`, if it has one (`Some(None)` for a parameterless mark).
pub fn mark_param(
    bindings: &dyn PdfiumLibraryBindings,
    page: &PdfPage<'_>,
    index: usize,
    name: &str,
    key: &str,
) -> Option<Option<String>> {
    let mark = find_mark(bindings, page, index, name)?;
    let mut len: c_ulong = 0;
    // SAFETY: a null buffer asks for the length only; `mark` lives with the object.
    let ok = unsafe {
        bindings.FPDFPageObjMark_GetParamStringValue(mark, key, std::ptr::null_mut(), 0, &mut len)
    };
    if !bindings.is_true(ok) || len < 2 {
        return Some(None);
    }
    let mut buf = vec![0u8; len as usize];
    // SAFETY: `buf` is `len` bytes, as PDFium asked for.
    let ok = unsafe {
        bindings.FPDFPageObjMark_GetParamStringValue(
            mark,
            key,
            buf.as_mut_ptr() as *mut _,
            len,
            &mut len,
        )
    };
    if !bindings.is_true(ok) {
        return Some(None);
    }
    Some(crate::engine::raw::doc::decode_utf16le(&buf))
}

fn mark_name(
    bindings: &dyn PdfiumLibraryBindings,
    mark: pdfium_render::prelude::FPDF_PAGEOBJECTMARK,
) -> Option<String> {
    let mut len: c_ulong = 0;
    // SAFETY: a null buffer asks for the length only.
    let ok = unsafe {
        bindings.FPDFPageObjMark_GetName(mark, std::ptr::null_mut(), 0, &mut len)
    };
    if !bindings.is_true(ok) || len < 2 {
        return None;
    }
    let mut buf = vec![0u8; len as usize];
    // SAFETY: `buf` is `len` bytes, as PDFium asked for.
    let ok = unsafe {
        bindings.FPDFPageObjMark_GetName(mark, buf.as_mut_ptr() as *mut _, len, &mut len)
    };
    if !bindings.is_true(ok) {
        return None;
    }
    crate::engine::raw::doc::decode_utf16le(&buf)
}

/// A Form XObject copied **once** into a destination document from a page of another one
/// (`FPDF_NewXObjectFromPage`). Every [`XObject::place`] creates a new page object that
/// references the same stream, so a logo stamped on 300 pages is stored once.
pub struct XObject<'a> {
    handle: FPDF_XOBJECT,
    /// The destination document (marks with params need it).
    dest: FPDF_DOCUMENT,
    bindings: &'static dyn PdfiumLibraryBindings,
    _docs: PhantomData<&'a ()>,
}

impl<'a> XObject<'a> {
    /// Copies page `src_index` of `src` (content and resources) into `dest` as a Form XObject
    /// whose `/BBox` is that page's media box.
    pub fn from_page(
        bindings: &'static dyn PdfiumLibraryBindings,
        dest: &'a PdfDocument<'_>,
        src: &'a PdfDocument<'_>,
        src_index: u16,
    ) -> Result<Self, EngineError> {
        // SAFETY: both documents are live for `'a`, on the engine thread.
        let handle = unsafe {
            bindings.FPDF_NewXObjectFromPage(
                dest.raw_handle(),
                src.raw_handle(),
                src_index as c_int,
            )
        };
        if handle.is_null() {
            return Err(EngineError::new(
                ErrorCode::Pdfium,
                "FPDF_NewXObjectFromPage failed",
            ));
        }
        Ok(Self {
            handle,
            dest: dest.raw_handle(),
            bindings,
            _docs: PhantomData,
        })
    }

    /// Appends a form object drawing this XObject to `page` with `matrix`, optionally tagged
    /// with the content mark `mark`.
    pub fn place(
        &self,
        page: &PdfPage<'_>,
        matrix: Matrix,
        mark: Option<Mark<'_>>,
    ) -> Result<(), EngineError> {
        let b = self.bindings;
        // SAFETY: `self.handle` is live until drop.
        let object = unsafe { b.FPDF_NewFormObjectFromXObject(self.handle) };
        if object.is_null() {
            return Err(EngineError::new(
                ErrorCode::Pdfium,
                "FPDF_NewFormObjectFromXObject failed",
            ));
        }
        let m = FS_MATRIX {
            a: matrix[0],
            b: matrix[1],
            c: matrix[2],
            d: matrix[3],
            e: matrix[4],
            f: matrix[5],
        };
        let setup = (|| {
            // SAFETY: `object` is a fresh, unowned page object; `m` outlives the call.
            let ok = unsafe { b.FPDFPageObj_SetMatrix(object, &m) };
            if !b.is_true(ok) {
                return Err(EngineError::new(ErrorCode::Pdfium, "FPDFPageObj_SetMatrix failed"));
            }
            if let Some(mark) = mark {
                mark_handle(b, self.dest, object, mark)?;
            }
            Ok(())
        })();
        if let Err(e) = setup {
            // SAFETY: not yet owned by a page, so it is ours to destroy.
            unsafe { b.FPDFPageObj_Destroy(object) };
            return Err(e);
        }
        // SAFETY: `page` is live; ownership of `object` passes to the page (and PDFium frees
        // it itself on failure).
        let ok = unsafe { b.FPDFPage_InsertObject(page.raw_handle(), object) };
        if !b.is_true(ok) {
            return Err(EngineError::new(ErrorCode::Pdfium, "FPDFPage_InsertObject failed"));
        }
        Ok(())
    }
}

impl Drop for XObject<'_> {
    fn drop(&mut self) {
        // SAFETY: closes only the wrapper; placed form objects keep the stream alive.
        unsafe { self.bindings.FPDF_CloseXObject(self.handle) };
    }
}

/// Stage 8 `duplicate_objects`: moves the objects at `indices` of `from` to the end of `to`,
/// translated by `(dx, dy)` — clip path and soft-mask matrices included, so a clipped image
/// does not end up clipped away.
///
/// `from` must be a **second, throw-away parse** of a page (never regenerated, dropped
/// afterwards): its page dictionary is untouched, and what moves are complete copies of the
/// objects that share the document's fonts, images and Form XObjects by reference. Both pages
/// must belong to the same document (PDFium cannot move objects across documents).
pub fn transplant(
    bindings: &dyn PdfiumLibraryBindings,
    from: &PdfPage<'_>,
    indices: &[usize],
    to: &PdfPage<'_>,
    dx: f32,
    dy: f32,
) -> Result<(), EngineError> {
    // Handles first: every removal shifts the indices after it.
    let handles = indices
        .iter()
        .map(|&i| object_at(bindings, from, i))
        .collect::<Result<Vec<_>, _>>()?;
    for handle in handles {
        // SAFETY: `handle` belongs to `from`, which is live; ownership passes to us.
        let ok = unsafe { bindings.FPDFPage_RemoveObject(from.raw_handle(), handle) };
        if !bindings.is_true(ok) {
            return Err(EngineError::new(ErrorCode::Pdfium, "FPDFPage_RemoveObject failed"));
        }
        let (dx, dy) = (dx as f64, dy as f64);
        // SAFETY: `handle` is an unowned, live page object.
        unsafe {
            bindings.FPDFPageObj_Transform(handle, 1.0, 0.0, 0.0, 1.0, dx, dy);
            bindings.FPDFPageObj_TransformClipPath(handle, 1.0, 0.0, 0.0, 1.0, dx, dy);
        }
        // SAFETY: `to` is live and in the same document; the page takes ownership.
        let ok = unsafe { bindings.FPDFPage_InsertObject(to.raw_handle(), handle) };
        if !bindings.is_true(ok) {
            return Err(EngineError::new(ErrorCode::Pdfium, "FPDFPage_InsertObject failed"));
        }
    }
    Ok(())
}

/// Stage 9 paragraph flow: moves the object at `index` of `page` by `(dx, dy)` in place —
/// `FPDFPageObj_Transform` plus `FPDFPageObj_TransformClipPath`, so a clipped image or a
/// clipped text run keeps its clip (the object and its clip move together). A shading object
/// is the exception: PDFium's `CPDF_ShadingObject::Transform` already moves its clip path.
///
/// The object is marked dirty; the caller regenerates the page content once at the end.
pub fn translate(
    bindings: &dyn PdfiumLibraryBindings,
    page: &PdfPage<'_>,
    index: usize,
    dx: f32,
    dy: f32,
) -> Result<(), EngineError> {
    let handle = object_at(bindings, page, index)?;
    let (dx, dy) = (dx as f64, dy as f64);
    // SAFETY: `handle` belongs to the live `page`; the calls take plain numbers.
    let kind = unsafe { bindings.FPDFPageObj_GetType(handle) };
    // SAFETY: as above; both transforms act on this one object only (its clip path is a
    // private copy-on-write value).
    unsafe {
        bindings.FPDFPageObj_Transform(handle, 1.0, 0.0, 0.0, 1.0, dx, dy);
        if kind != FPDF_PAGEOBJ_SHADING {
            bindings.FPDFPageObj_TransformClipPath(handle, 1.0, 0.0, 0.0, 1.0, dx, dy);
        }
    }
    Ok(())
}

/// `FPDF_PAGEOBJ_SHADING` (public/fpdf_edit.h).
const FPDF_PAGEOBJ_SHADING: c_int = 4;

/// `FPDFImageObj_LoadJpegFileInline` on the **existing** image object at `index`: the stream
/// is replaced by `jpeg` (a complete JPEG file, stored as `/DCTDecode`), while the object's
/// matrix, clip path and graphics state are kept — which a remove + re-add would lose.
pub fn replace_with_jpeg(
    bindings: &dyn PdfiumLibraryBindings,
    page: &PdfPage<'_>,
    index: usize,
    jpeg: &[u8],
) -> Result<(), EngineError> {
    if jpeg.is_empty() {
        return Err(EngineError::invalid("empty JPEG data"));
    }
    let handle = object_at(bindings, page, index)?;
    let source = Source { data: jpeg };
    let mut access = FPDF_FILEACCESS {
        m_FileLen: jpeg.len() as c_ulong,
        m_GetBlock: Some(read_block),
        m_Param: &source as *const Source<'_> as *mut c_void,
    };
    let mut pages: [FPDF_PAGE; 1] = [page.raw_handle()];
    // SAFETY: `access` and `source` outlive the call (Inline reads everything before it
    // returns); `pages` names the live page so PDFium drops its cached decode of the image.
    let ok = unsafe {
        bindings.FPDFImageObj_LoadJpegFileInline(pages.as_mut_ptr(), 1, handle, &mut access)
    };
    if !bindings.is_true(ok) {
        return Err(EngineError::new(
            ErrorCode::Pdfium,
            "FPDFImageObj_LoadJpegFileInline failed",
        ));
    }
    Ok(())
}

struct Source<'a> {
    data: &'a [u8],
}

/// `FPDF_FILEACCESS::m_GetBlock` over an in-memory slice.
unsafe extern "C" fn read_block(
    param: *mut c_void,
    position: c_ulong,
    buf: *mut c_uchar,
    size: c_ulong,
) -> c_int {
    // SAFETY: `param` is the `Source` set up by `replace_with_jpeg`, alive for the call.
    let source = unsafe { &*(param as *const Source<'_>) };
    let (start, len) = (position as usize, size as usize);
    let Some(end) = start.checked_add(len) else {
        return 0;
    };
    if end > source.data.len() || buf.is_null() {
        return 0;
    }
    // SAFETY: PDFium guarantees `buf` holds `size` bytes; the source range was checked.
    unsafe { std::ptr::copy_nonoverlapping(source.data.as_ptr().add(start), buf, len) };
    1
}
