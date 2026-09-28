//! The `FPDFAnnot_*` pipeline — everything pdfium-render 0.9.4 cannot do safely.
//!
//! What lives here and why (annotations spike §1.8, §5; `ARCHITECTURE.md` §6.1):
//!
//! * **creation of Circle** (no `create_circle_annotation` in 0.9.4) and of any subtype by id;
//! * **`/InkList`** (`AddInkStroke`) — the high-level ink annotation only gets path objects in
//!   its appearance stream, so Acrobat sees no ink data;
//! * **border width** (`SetBorder`) and **opacity** (`/CA`, which is the alpha of the last
//!   `SetColor` call — there is no `FPDFAnnot_SetNumberValue`);
//! * **safe recolouring**: `FPDFAnnot_SetColor` returns false once an `/AP` exists and
//!   pdfium-render's fallback then casts `CPDF_AnnotContext*` to `CPDF_PageObject*` and
//!   **SIGSEGVs**. [`AnnotRef::clear_ap`] first, then colour, is the only safe order;
//! * **dictionary keys** (`/NM`, `/T`, `/Contents`, `/Subj`, `/DA`, `/CreationDate`);
//! * **quads in PDFium's TL, TR, BL, BR order** — `PdfQuadPoints::from_rect()` is
//!   BL,BR,TR,TL and renders a 1-px sliver;
//! * the **form-field reads** that need the form handle beside the annotation.
//!
//! [`AnnotRef`] is an RAII handle: it closes its `FPDF_ANNOTATION` on drop and borrows the
//! page it came from, so a handle can never outlive its page.

use crate::engine::raw::consts;
use crate::engine::raw::doc::decode_utf16le;
use crate::ipc::types::{Rect, Rgb};
use crate::ipc::{EngineError, ErrorCode};
use pdfium_render::prelude::{
    FPDF_ANNOTATION, FPDF_ANNOTATION_SUBTYPE, FPDF_DOCUMENT, FPDF_FORMHANDLE, FPDF_WCHAR, FS_POINTF,
    FS_QUADPOINTSF, FS_RECTF, PdfPage, PdfiumLibraryBindings,
};
use std::marker::PhantomData;
use std::os::raw::{c_int, c_ulong};

/// Which colour entry of the annotation dictionary a call addresses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorKind {
    /// `/C` — the stroke / markup colour.
    Stroke,
    /// `/IC` — the interior (fill) colour.
    Interior,
}

impl ColorKind {
    fn as_type(self) -> u32 {
        (match self {
            ColorKind::Stroke => consts::FPDFANNOT_COLORTYPE_COLOR,
            ColorKind::Interior => consts::FPDFANNOT_COLORTYPE_INTERIORCOLOR,
        }) as u32
    }
}

/// A live `FPDF_ANNOTATION`, closed on drop.
///
/// The lifetime is the page's: `FPDFPage_CloseAnnot` must run before the page is closed, and
/// the annotation index it came from is only meaningful while the page is loaded.
pub struct AnnotRef<'a> {
    bindings: &'static dyn PdfiumLibraryBindings,
    handle: FPDF_ANNOTATION,
    page: PhantomData<&'a ()>,
}

impl Drop for AnnotRef<'_> {
    fn drop(&mut self) {
        // SAFETY: `handle` was returned by FPDFPage_GetAnnot / FPDFPage_CreateAnnot, is
        // non-null (checked at construction) and is closed exactly once, here.
        unsafe { self.bindings.FPDFPage_CloseAnnot(self.handle) }
    }
}

// ---------------------------------------------------------------------------------------
// page-level entry points
// ---------------------------------------------------------------------------------------

/// `FPDFPage_GetAnnotCount`.
pub fn count(bindings: &dyn PdfiumLibraryBindings, page: &PdfPage<'_>) -> usize {
    // SAFETY: `page` is live for this borrow and owned by this thread.
    let n = unsafe { bindings.FPDFPage_GetAnnotCount(page.raw_handle()) };
    n.max(0) as usize
}

/// `FPDFPage_GetAnnot` — the annotation at `index`, or `notFound`.
pub fn get<'a>(
    bindings: &'static dyn PdfiumLibraryBindings,
    page: &'a PdfPage<'_>,
    index: usize,
) -> Result<AnnotRef<'a>, EngineError> {
    // SAFETY: `page` is live; an out-of-range index makes PDFium return NULL, checked below.
    let handle = unsafe { bindings.FPDFPage_GetAnnot(page.raw_handle(), index as c_int) };
    if handle.is_null() {
        return Err(EngineError::not_found(format!("annotation #{index}")));
    }
    Ok(AnnotRef {
        bindings,
        handle,
        page: PhantomData,
    })
}

/// `FPDFPage_CreateAnnot` — a new annotation of `subtype`, appended to the page's `/Annots`.
///
/// Returns `unsupported` for the subtypes PDFium refuses to create (Line, Polygon, Polyline,
/// Redact, Caret — `CreateAnnot` returns NULL; annotations spike §2).
pub fn create<'a>(
    bindings: &'static dyn PdfiumLibraryBindings,
    page: &'a PdfPage<'_>,
    subtype: c_int,
) -> Result<AnnotRef<'a>, EngineError> {
    // SAFETY: `page` is live; PDFium returns NULL for subtypes it cannot create.
    let handle = unsafe {
        bindings.FPDFPage_CreateAnnot(page.raw_handle(), subtype as FPDF_ANNOTATION_SUBTYPE)
    };
    if handle.is_null() {
        return Err(EngineError::new(
            ErrorCode::Unsupported,
            format!("PDFium cannot create annotation subtype {subtype}"),
        ));
    }
    Ok(AnnotRef {
        bindings,
        handle,
        page: PhantomData,
    })
}

/// `FPDFPage_RemoveAnnot`. Indices shift, so remove **descending**.
pub fn remove(
    bindings: &dyn PdfiumLibraryBindings,
    page: &PdfPage<'_>,
    index: usize,
) -> Result<(), EngineError> {
    // SAFETY: `page` is live; a bad index makes PDFium return false, handled below.
    let ok = unsafe { bindings.FPDFPage_RemoveAnnot(page.raw_handle(), index as c_int) };
    if bindings.is_true(ok) {
        Ok(())
    } else {
        Err(EngineError::not_found(format!(
            "annotation #{index} could not be removed"
        )))
    }
}

/// `FPDFAnnot_IsSupportedSubtype` — whether `FPDFPage_CreateAnnot` can make this subtype.
pub fn is_supported_subtype(bindings: &dyn PdfiumLibraryBindings, subtype: c_int) -> bool {
    // SAFETY: no handles involved; the call only reads a static table.
    let ok = unsafe { bindings.FPDFAnnot_IsSupportedSubtype(subtype as FPDF_ANNOTATION_SUBTYPE) };
    bindings.is_true(ok)
}

// ---------------------------------------------------------------------------------------
// reads and writes on one annotation
// ---------------------------------------------------------------------------------------

impl AnnotRef<'_> {
    /// The raw handle, for the few calls that are not wrapped here. Never store it.
    pub fn handle(&self) -> FPDF_ANNOTATION {
        self.handle
    }

    /// `FPDFAnnot_GetSubtype` — one of the `consts::FPDF_ANNOT_*` values.
    pub fn subtype(&self) -> c_int {
        // SAFETY: `self.handle` is live until this value drops.
        unsafe { self.bindings.FPDFAnnot_GetSubtype(self.handle) as c_int }
    }

    /// `FPDFAnnot_GetRect`, converted to the contract's `{l, b, r, t}` (`FS_RECTF` is
    /// `{left, top, right, bottom}`).
    pub fn rect(&self) -> Option<Rect> {
        let mut r = FS_RECTF {
            left: 0.0,
            top: 0.0,
            right: 0.0,
            bottom: 0.0,
        };
        // SAFETY: `self.handle` is live; `r` is a valid out-parameter.
        let ok = unsafe { self.bindings.FPDFAnnot_GetRect(self.handle, &mut r) };
        if !self.bindings.is_true(ok) {
            return None;
        }
        Some(Rect::new(
            r.left.min(r.right),
            r.bottom.min(r.top),
            r.left.max(r.right),
            r.bottom.max(r.top),
        ))
    }

    /// `FPDFAnnot_SetRect`. For Ink and Stamp this **must** happen before any object is
    /// appended: the appearance stream's BBox is the `/Rect` at that moment and objects
    /// outside it are clipped away (annotations spike §5.5).
    pub fn set_rect(&mut self, rect: Rect) -> bool {
        let r = FS_RECTF {
            left: rect.l.min(rect.r),
            top: rect.b.max(rect.t),
            right: rect.l.max(rect.r),
            bottom: rect.b.min(rect.t),
        };
        // SAFETY: `self.handle` is live; `r` is a valid in-parameter.
        let ok = unsafe { self.bindings.FPDFAnnot_SetRect(self.handle, &r) };
        self.bindings.is_true(ok)
    }

    /// `FPDFAnnot_GetColor` → `(rgb, alpha)`. `None` when the annotation has no such entry
    /// (common: markup colours that live only inside a third-party appearance stream).
    pub fn color(&self, kind: ColorKind) -> Option<(Rgb, u8)> {
        let (mut r, mut g, mut b, mut a) = (0u32, 0u32, 0u32, 0u32);
        // SAFETY: `self.handle` is live; the four out-parameters are valid.
        let ok = unsafe {
            self.bindings.FPDFAnnot_GetColor(
                self.handle,
                kind.as_type(),
                &mut r,
                &mut g,
                &mut b,
                &mut a,
            )
        };
        if !self.bindings.is_true(ok) {
            return None;
        }
        Some(([r as u8, g as u8, b as u8], a as u8))
    }

    /// `FPDFAnnot_SetColor`.
    ///
    /// Two rules, both learned the hard way (annotations spike §5.1, §5.7):
    /// 1. **every** call rewrites `/CA` from its alpha, so pass the same alpha for `/C` and
    ///    `/IC` or the second call silently resets the opacity;
    /// 2. it returns `false` whenever an `/AP` exists — call [`Self::clear_ap`] first.
    pub fn set_color(&mut self, kind: ColorKind, rgb: Rgb, alpha: u8) -> bool {
        // SAFETY: `self.handle` is live; the call takes plain integers.
        let ok = unsafe {
            self.bindings.FPDFAnnot_SetColor(
                self.handle,
                kind.as_type(),
                rgb[0] as u32,
                rgb[1] as u32,
                rgb[2] as u32,
                alpha as u32,
            )
        };
        self.bindings.is_true(ok)
    }

    /// `FPDFAnnot_GetBorder` → the `/BS /W` border width.
    pub fn border_width(&self) -> Option<f32> {
        let (mut h, mut v, mut w) = (0.0f32, 0.0f32, 0.0f32);
        // SAFETY: `self.handle` is live; the three out-parameters are valid.
        let ok = unsafe {
            self.bindings
                .FPDFAnnot_GetBorder(self.handle, &mut h, &mut v, &mut w)
        };
        if self.bindings.is_true(ok) {
            Some(w)
        } else {
            None
        }
    }

    /// `FPDFAnnot_SetBorder(0, 0, width)` — square corners, `width` points. The appearance
    /// generator reads this, so it must be set before the page is rendered.
    pub fn set_border_width(&mut self, width: f32) -> bool {
        // SAFETY: `self.handle` is live; the call takes plain floats.
        let ok = unsafe {
            self.bindings
                .FPDFAnnot_SetBorder(self.handle, 0.0, 0.0, width.max(0.0))
        };
        self.bindings.is_true(ok)
    }

    /// `/CA`, 0..1. There is no setter: opacity is the alpha of the last
    /// [`Self::set_color`] call.
    pub fn opacity(&self) -> Option<f32> {
        self.number("CA")
    }

    /// `FPDFAnnot_HasKey`.
    pub fn has_key(&self, key: &str) -> bool {
        // SAFETY: `self.handle` is live; `key` is a Rust `&str` the binding converts itself.
        let ok = unsafe { self.bindings.FPDFAnnot_HasKey(self.handle, key) };
        self.bindings.is_true(ok)
    }

    /// `FPDFAnnot_GetStringValue` — a UTF-16LE dictionary string (`/Contents`, `/T`, `/NM`,
    /// `/Subj`, `/DA`, `/CreationDate`, `/M`…). `None` when the key is absent or empty.
    pub fn string(&self, key: &str) -> Option<String> {
        // SAFETY: `self.handle` is live; a null buffer asks for the length only.
        let len = unsafe {
            self.bindings
                .FPDFAnnot_GetStringValue(self.handle, key, std::ptr::null_mut(), 0)
        } as usize;
        if len < 4 {
            // 0 = absent, 2 = just the UTF-16 terminator.
            return None;
        }
        let mut buffer = vec![0u8; len];
        // SAFETY: `buffer` is exactly `len` bytes, the length PDFium just asked for.
        unsafe {
            self.bindings.FPDFAnnot_GetStringValue(
                self.handle,
                key,
                buffer.as_mut_ptr() as *mut FPDF_WCHAR,
                len as c_ulong,
            )
        };
        decode_utf16le(&buffer)
    }

    /// `FPDFAnnot_SetStringValue` — writes a UTF-16LE dictionary string.
    pub fn set_string(&mut self, key: &str, value: &str) -> bool {
        // SAFETY: `self.handle` is live; the `_str` helper builds the UTF-16LE buffer and
        // keeps it alive across the call.
        let ok = unsafe {
            self.bindings
                .FPDFAnnot_SetStringValue_str(self.handle, key, value)
        };
        self.bindings.is_true(ok)
    }

    /// `FPDFAnnot_GetNumberValue`.
    pub fn number(&self, key: &str) -> Option<f32> {
        let mut value = 0.0f32;
        // SAFETY: `self.handle` is live; `value` is a valid out-parameter.
        let ok = unsafe {
            self.bindings
                .FPDFAnnot_GetNumberValue(self.handle, key, &mut value)
        };
        if self.bindings.is_true(ok) {
            Some(value)
        } else {
            None
        }
    }

    /// `FPDFAnnot_SetAP(NORMAL, NULL)` — **removes** the normal appearance stream.
    ///
    /// The first step of every edit: with an `/AP` present `FPDFAnnot_SetColor` fails and
    /// pdfium-render's high-level setters segfault, and a third-party AP would not follow the
    /// new geometry. After this, the next render regenerates the appearance.
    pub fn clear_ap(&mut self) -> bool {
        // SAFETY: `self.handle` is live; a null value is how PDFium's API spells "remove".
        let ok = unsafe {
            self.bindings.FPDFAnnot_SetAP(
                self.handle,
                consts::FPDF_ANNOT_APPEARANCEMODE_NORMAL,
                std::ptr::null(),
            )
        };
        self.bindings.is_true(ok)
    }

    /// `FPDFAnnot_SetAP(NORMAL, stream)` — writes an explicit appearance stream. The stream
    /// gets `BBox = /Rect` and **no `/Resources`**, so it can only use operators that need no
    /// resource dictionary (annotations spike §2).
    pub fn set_ap(&mut self, stream: &str) -> bool {
        // SAFETY: `self.handle` is live; the `_str` helper owns the UTF-16LE buffer.
        let ok = unsafe {
            self.bindings.FPDFAnnot_SetAP_str(
                self.handle,
                consts::FPDF_ANNOT_APPEARANCEMODE_NORMAL,
                stream,
            )
        };
        self.bindings.is_true(ok)
    }

    /// Byte length of the normal appearance stream, 0 when there is none. Used to decide
    /// whether an annotation was produced by another application ([`Self::clear_ap`] then
    /// matters) and by the tests that prove an `/AP` was written before save.
    pub fn ap_len(&self) -> usize {
        // SAFETY: `self.handle` is live; a null buffer asks for the length only.
        let len = unsafe {
            self.bindings.FPDFAnnot_GetAP(
                self.handle,
                consts::FPDF_ANNOT_APPEARANCEMODE_NORMAL,
                std::ptr::null_mut(),
                0,
            )
        } as usize;
        len.saturating_sub(2)
    }

    /// `FPDFAnnot_GetFlags` — the `/F` bit field (`consts::FPDF_ANNOT_FLAG_*`).
    pub fn flags(&self) -> c_int {
        // SAFETY: `self.handle` is live.
        unsafe { self.bindings.FPDFAnnot_GetFlags(self.handle) }
    }

    /// `FPDFAnnot_SetFlags`. Always include `FPDF_ANNOT_FLAG_PRINT` on annotations we create:
    /// pdfium-render never sets it and `FPDFPage_Flatten(FLAT_PRINT)` *deletes* annotations
    /// that lack it.
    pub fn set_flags(&mut self, flags: c_int) -> bool {
        // SAFETY: `self.handle` is live.
        let ok = unsafe { self.bindings.FPDFAnnot_SetFlags(self.handle, flags) };
        self.bindings.is_true(ok)
    }

    /// `FPDFAnnot_AddInkStroke` — appends one stroke to `/InkList`, returning its index.
    ///
    /// `points` is `[x0, y0, x1, y1, …]` in PDF user space. This is the only way to get a
    /// real `/InkList` (the high-level ink annotation only appends path objects to the
    /// appearance stream, which Acrobat renders but cannot edit).
    pub fn add_ink_stroke(&mut self, points: &[f32]) -> Result<usize, EngineError> {
        if points.len() < 4 || points.len() % 2 != 0 {
            return Err(EngineError::invalid(
                "ink stroke needs an even number of coordinates and at least two points",
            ));
        }
        let pts: Vec<FS_POINTF> = points
            .chunks_exact(2)
            .map(|c| FS_POINTF { x: c[0], y: c[1] })
            .collect();
        // SAFETY: `self.handle` is live; `pts` is a `len`-element array that outlives the call.
        let index = unsafe {
            self.bindings
                .FPDFAnnot_AddInkStroke(self.handle, pts.as_ptr(), pts.len())
        };
        if index < 0 {
            return Err(EngineError::new(
                ErrorCode::Pdfium,
                "FPDFAnnot_AddInkStroke failed",
            ));
        }
        Ok(index as usize)
    }

    /// `FPDFAnnot_RemoveInkList` — drops every stroke (used when an ink annotation's paths
    /// are replaced wholesale by `update_annotation`).
    pub fn remove_ink_list(&mut self) -> bool {
        // SAFETY: `self.handle` is live.
        let ok = unsafe { self.bindings.FPDFAnnot_RemoveInkList(self.handle) };
        self.bindings.is_true(ok)
    }

    /// `/InkList` as one `[x0, y0, x1, y1, …]` vector per stroke.
    pub fn ink_paths(&self) -> Vec<Vec<f32>> {
        // SAFETY: `self.handle` is live.
        let strokes = unsafe { self.bindings.FPDFAnnot_GetInkListCount(self.handle) };
        let mut out = Vec::with_capacity(strokes as usize);
        for i in 0..strokes {
            // SAFETY: `i < strokes`; a null buffer asks for the point count.
            let count = unsafe {
                self.bindings
                    .FPDFAnnot_GetInkListPath(self.handle, i, std::ptr::null_mut(), 0)
            };
            if count == 0 {
                continue;
            }
            let mut buffer = vec![FS_POINTF { x: 0.0, y: 0.0 }; count as usize];
            // SAFETY: `buffer` holds exactly `count` points, what PDFium just asked for.
            let written = unsafe {
                self.bindings
                    .FPDFAnnot_GetInkListPath(self.handle, i, buffer.as_mut_ptr(), count)
            } as usize;
            let mut flat = Vec::with_capacity(written * 2);
            for p in buffer.iter().take(written) {
                flat.push(p.x);
                flat.push(p.y);
            }
            out.push(flat);
        }
        out
    }

    /// `FPDFAnnot_GetLine` → `[x1, y1, x2, y2]` for `/Line` annotations written by other
    /// applications (PDFium cannot create them; SeePDF writes Ink + `/Subj` instead).
    pub fn line(&self) -> Option<[f32; 4]> {
        let mut start = FS_POINTF { x: 0.0, y: 0.0 };
        let mut end = FS_POINTF { x: 0.0, y: 0.0 };
        // SAFETY: `self.handle` is live; both out-parameters are valid.
        let ok = unsafe {
            self.bindings
                .FPDFAnnot_GetLine(self.handle, &mut start, &mut end)
        };
        if self.bindings.is_true(ok) {
            Some([start.x, start.y, end.x, end.y])
        } else {
            None
        }
    }

    /// `FPDFAnnot_CountAttachmentPoints` — the number of `/QuadPoints` quads.
    pub fn quad_count(&self) -> usize {
        // SAFETY: `self.handle` is live.
        unsafe { self.bindings.FPDFAnnot_CountAttachmentPoints(self.handle) }
    }

    /// The `/QuadPoints` quads as bounding rectangles, order-agnostic (a quad read from
    /// another producer may be in any corner order, so min/max is the only safe reduction).
    pub fn quads(&self) -> Vec<Rect> {
        let n = self.quad_count();
        let mut out = Vec::with_capacity(n);
        for i in 0..n {
            let mut q = FS_QUADPOINTSF {
                x1: 0.0,
                y1: 0.0,
                x2: 0.0,
                y2: 0.0,
                x3: 0.0,
                y3: 0.0,
                x4: 0.0,
                y4: 0.0,
            };
            // SAFETY: `i < n`; `q` is a valid out-parameter.
            let ok = unsafe {
                self.bindings
                    .FPDFAnnot_GetAttachmentPoints(self.handle, i, &mut q)
            };
            if !self.bindings.is_true(ok) {
                continue;
            }
            let xs = [q.x1, q.x2, q.x3, q.x4];
            let ys = [q.y1, q.y2, q.y3, q.y4];
            out.push(Rect::new(
                xs.iter().copied().fold(f32::MAX, f32::min),
                ys.iter().copied().fold(f32::MAX, f32::min),
                xs.iter().copied().fold(f32::MIN, f32::max),
                ys.iter().copied().fold(f32::MIN, f32::max),
            ));
        }
        out
    }

    /// `FPDFAnnot_AppendAttachmentPoints` with the quad built in **PDFium's order**:
    /// (x1,y1) = top-left, (x2,y2) = top-right, (x3,y3) = bottom-left, (x4,y4) = bottom-right.
    ///
    /// PDFium derives `left = x3, bottom = y3, right = x2, top = y2`; the BL,BR,TR,TL order
    /// `PdfQuadPoints::from_rect()` produces normalises to a zero-width rectangle and the
    /// appearance stream becomes a 1-px sliver (annotations spike §5.2).
    pub fn append_quad(&mut self, rect: Rect) -> bool {
        let q = quad_tl(rect);
        // SAFETY: `self.handle` is live; `q` is a valid in-parameter.
        let ok = unsafe {
            self.bindings
                .FPDFAnnot_AppendAttachmentPoints(self.handle, &q)
        };
        self.bindings.is_true(ok)
    }

    /// `FPDFAnnot_SetAttachmentPoints` — replaces quad `index`, same corner order as
    /// [`Self::append_quad`].
    pub fn set_quad(&mut self, index: usize, rect: Rect) -> bool {
        let q = quad_tl(rect);
        // SAFETY: `self.handle` is live; PDFium range-checks `index` and returns false.
        let ok = unsafe {
            self.bindings
                .FPDFAnnot_SetAttachmentPoints(self.handle, index, &q)
        };
        self.bindings.is_true(ok)
    }

    /// Stage 9 paragraph flow: moves every `/QuadPoints` quad by `(dx, dy)`, keeping each
    /// quad's corner order exactly as the producer wrote it. Returns how many quads moved.
    ///
    /// On a markup annotation with an `/AP`, `FPDFAnnot_SetAttachmentPoints` may *grow* the
    /// appearance `BBox` to the half-moved quads' bounds, which then squashes the drawing;
    /// callers clear the appearance first (see `objects::flow`).
    pub fn translate_quads(&mut self, dx: f32, dy: f32) -> usize {
        let mut moved = 0;
        for i in 0..self.quad_count() {
            let mut q = FS_QUADPOINTSF {
                x1: 0.0,
                y1: 0.0,
                x2: 0.0,
                y2: 0.0,
                x3: 0.0,
                y3: 0.0,
                x4: 0.0,
                y4: 0.0,
            };
            // SAFETY: `i < quad_count()`; `q` is a valid out-parameter.
            let ok = unsafe {
                self.bindings
                    .FPDFAnnot_GetAttachmentPoints(self.handle, i, &mut q)
            };
            if !self.bindings.is_true(ok) {
                continue;
            }
            let moved_q = FS_QUADPOINTSF {
                x1: q.x1 + dx,
                y1: q.y1 + dy,
                x2: q.x2 + dx,
                y2: q.y2 + dy,
                x3: q.x3 + dx,
                y3: q.y3 + dy,
                x4: q.x4 + dx,
                y4: q.y4 + dy,
            };
            // SAFETY: `self.handle` is live; `moved_q` is a valid in-parameter.
            let ok = unsafe {
                self.bindings
                    .FPDFAnnot_SetAttachmentPoints(self.handle, i, &moved_q)
            };
            if self.bindings.is_true(ok) {
                moved += 1;
            }
        }
        moved
    }

    /// `FPDFAnnot_SetURI` — a `/Link` annotation's URI action. Go-to-page destinations
    /// cannot be created through PDFium's public API (`engine::structure::links` writes them
    /// with lopdf).
    pub fn set_uri(&mut self, uri: &str) -> bool {
        // SAFETY: `self.handle` is live; the binding converts `uri` itself.
        let ok = unsafe { self.bindings.FPDFAnnot_SetURI(self.handle, uri) };
        self.bindings.is_true(ok)
    }

    /// `FPDFAnnot_SetBorder(0, 0, 0)` → `/Border [0 0 0]`: a link draws no box. Without it the
    /// PDF default `[0 0 1]` applies and several viewers stroke a 1 pt border round the link.
    pub fn set_no_border(&mut self) -> bool {
        // SAFETY: `self.handle` is live; the call takes plain floats.
        let ok = unsafe { self.bindings.FPDFAnnot_SetBorder(self.handle, 0.0, 0.0, 0.0) };
        self.bindings.is_true(ok)
    }

    /// The go-to-page target of a Link annotation: `FPDFAnnot_GetLink` → `FPDFLink_GetDest`,
    /// which reads `/Dest` (named destinations resolved) and falls back to a GoTo `/A`.
    /// `None` for a URI link, or a destination that names no page of this document.
    pub fn link_dest(&self, document: FPDF_DOCUMENT) -> Option<crate::ipc::types::LinkDest> {
        // SAFETY: `self.handle` is live; a non-Link annotation yields a null FPDF_LINK.
        let dest = unsafe {
            let link = self.bindings.FPDFAnnot_GetLink(self.handle);
            if link.is_null() {
                return None;
            }
            self.bindings.FPDFLink_GetDest(document, link)
        };
        super::outline::link_dest(self.bindings, document, dest)
    }

    /// The `/A <</S /URI …>>` target of a Link annotation, via `FPDFAnnot_GetLink` →
    /// `FPDFLink_GetAction` → `FPDFAction_GetURIPath` (7-bit ASCII, not UTF-16).
    pub fn uri(&self, document: FPDF_DOCUMENT) -> Option<String> {
        // SAFETY: `self.handle` is live; a non-Link annotation yields a null FPDF_LINK,
        // and every following call tolerates a null argument by returning 0 / null.
        unsafe {
            let link = self.bindings.FPDFAnnot_GetLink(self.handle);
            if link.is_null() {
                return None;
            }
            let action = self.bindings.FPDFLink_GetAction(link);
            if action.is_null() {
                return None;
            }
            let len =
                self.bindings
                    .FPDFAction_GetURIPath(document, action, std::ptr::null_mut(), 0) as usize;
            if len < 2 {
                return None;
            }
            let mut buffer = vec![0u8; len];
            let written = self.bindings.FPDFAction_GetURIPath(
                document,
                action,
                buffer.as_mut_ptr() as *mut std::os::raw::c_void,
                len as c_ulong,
            ) as usize;
            let end = written.min(len).saturating_sub(1);
            String::from_utf8(buffer[..end].to_vec()).ok()
        }
    }

    /// `FPDFAnnot_GetLinkedAnnot(key)` — the `/Popup`, `/Parent` or `/IRT` annotation.
    ///
    /// Read-only: nothing in PDFium's API can *create* such a link. The handle is a second
    /// wrapper around the same dictionary, so two annotations can only be compared by
    /// `/NM`, never by handle identity.
    pub fn linked(&self, key: &str) -> Option<AnnotRef<'_>> {
        // SAFETY: `self.handle` is live; a missing key yields NULL, checked below.
        let handle = unsafe { self.bindings.FPDFAnnot_GetLinkedAnnot(self.handle, key) };
        if handle.is_null() {
            return None;
        }
        Some(AnnotRef {
            bindings: self.bindings,
            handle,
            page: PhantomData,
        })
    }

    /// Whether `/Popup`, `/Parent` or `/IRT` resolves.
    pub fn has_linked(&self, key: &str) -> bool {
        self.linked(key).is_some()
    }

    // -- form fields (need the form handle beside the annotation) ------------------------

    /// `FPDFAnnot_GetFormFieldType` — one of `consts::FPDF_FORMFIELD_*`, or `-1`.
    pub fn form_field_type(&self, form: FPDF_FORMHANDLE) -> c_int {
        // SAFETY: `self.handle` is live and `form` is the form environment of its document.
        unsafe {
            self.bindings
                .FPDFAnnot_GetFormFieldType(form, self.handle)
        }
    }

    /// `FPDFAnnot_GetFormFieldName` — the fully qualified field name.
    pub fn form_field_name(&self, form: FPDF_FORMHANDLE) -> Option<String> {
        self.form_string(form, FormString::Name)
    }

    /// `FPDFAnnot_GetFormFieldValue` — `/V` as PDFium's form layer sees it.
    pub fn form_field_value(&self, form: FPDF_FORMHANDLE) -> Option<String> {
        self.form_string(form, FormString::Value)
    }

    /// `FPDFAnnot_GetFormFieldExportValue` — a checkbox/radio's "on" state name.
    pub fn form_field_export_value(&self, form: FPDF_FORMHANDLE) -> Option<String> {
        self.form_string(form, FormString::Export)
    }

    fn form_string(&self, form: FPDF_FORMHANDLE, which: FormString) -> Option<String> {
        // SAFETY (both calls): `self.handle` and `form` are live; the first pass passes a
        // null buffer to learn the length, the second a buffer of exactly that length.
        let len = unsafe {
            match which {
                FormString::Name => self.bindings.FPDFAnnot_GetFormFieldName(
                    form,
                    self.handle,
                    std::ptr::null_mut(),
                    0,
                ),
                FormString::Value => self.bindings.FPDFAnnot_GetFormFieldValue(
                    form,
                    self.handle,
                    std::ptr::null_mut(),
                    0,
                ),
                FormString::Export => self.bindings.FPDFAnnot_GetFormFieldExportValue(
                    form,
                    self.handle,
                    std::ptr::null_mut(),
                    0,
                ),
            }
        } as usize;
        if len < 4 {
            return None;
        }
        let mut buffer = vec![0u8; len];
        let ptr = buffer.as_mut_ptr() as *mut FPDF_WCHAR;
        // SAFETY: `buffer` is exactly `len` bytes.
        unsafe {
            match which {
                FormString::Name => {
                    self.bindings
                        .FPDFAnnot_GetFormFieldName(form, self.handle, ptr, len as c_ulong)
                }
                FormString::Value => {
                    self.bindings
                        .FPDFAnnot_GetFormFieldValue(form, self.handle, ptr, len as c_ulong)
                }
                FormString::Export => self.bindings.FPDFAnnot_GetFormFieldExportValue(
                    form,
                    self.handle,
                    ptr,
                    len as c_ulong,
                ),
            }
        };
        decode_utf16le(&buffer)
    }

    /// `FPDFAnnot_GetFormFieldFlags` — the `/Ff` bit field (`consts::FPDF_FORMFLAG_*`).
    pub fn form_field_flags(&self, form: FPDF_FORMHANDLE) -> c_int {
        // SAFETY: `self.handle` and `form` are live.
        unsafe {
            self.bindings
                .FPDFAnnot_GetFormFieldFlags(form, self.handle)
        }
    }

    /// `FPDFAnnot_IsChecked` — the current state of a checkbox or radio widget.
    pub fn is_checked(&self, form: FPDF_FORMHANDLE) -> bool {
        // SAFETY: `self.handle` and `form` are live.
        let ok = unsafe { self.bindings.FPDFAnnot_IsChecked(form, self.handle) };
        self.bindings.is_true(ok)
    }

    /// `FPDFAnnot_GetOptionCount` — `/Opt` entries of a combo or list box.
    pub fn option_count(&self, form: FPDF_FORMHANDLE) -> usize {
        // SAFETY: `self.handle` and `form` are live.
        let n = unsafe { self.bindings.FPDFAnnot_GetOptionCount(form, self.handle) };
        n.max(0) as usize
    }

    /// `FPDFAnnot_GetOptionLabel`.
    pub fn option_label(&self, form: FPDF_FORMHANDLE, index: usize) -> Option<String> {
        // SAFETY: `self.handle` and `form` are live; a null buffer asks for the length.
        let len = unsafe {
            self.bindings.FPDFAnnot_GetOptionLabel(
                form,
                self.handle,
                index as c_int,
                std::ptr::null_mut(),
                0,
            )
        } as usize;
        if len < 4 {
            return None;
        }
        let mut buffer = vec![0u8; len];
        // SAFETY: `buffer` is exactly `len` bytes.
        unsafe {
            self.bindings.FPDFAnnot_GetOptionLabel(
                form,
                self.handle,
                index as c_int,
                buffer.as_mut_ptr() as *mut FPDF_WCHAR,
                len as c_ulong,
            )
        };
        decode_utf16le(&buffer)
    }

    /// `FPDFAnnot_IsOptionSelected`.
    pub fn is_option_selected(&self, form: FPDF_FORMHANDLE, index: usize) -> bool {
        // SAFETY: `self.handle` and `form` are live.
        let ok = unsafe {
            self.bindings
                .FPDFAnnot_IsOptionSelected(form, self.handle, index as c_int)
        };
        self.bindings.is_true(ok)
    }

    /// `FPDFAnnot_GetFontSize` — the field's `/DA` font size (0 means "auto").
    pub fn form_font_size(&self, form: FPDF_FORMHANDLE) -> Option<f32> {
        let mut value = 0.0f32;
        // SAFETY: `self.handle` and `form` are live; `value` is a valid out-parameter.
        let ok = unsafe {
            self.bindings
                .FPDFAnnot_GetFontSize(form, self.handle, &mut value)
        };
        if self.bindings.is_true(ok) {
            Some(value)
        } else {
            None
        }
    }
}

#[derive(Clone, Copy)]
enum FormString {
    Name,
    Value,
    Export,
}

/// A `FS_QUADPOINTSF` in PDFium's TL, TR, BL, BR corner order.
fn quad_tl(rect: Rect) -> FS_QUADPOINTSF {
    let (l, r) = (rect.l.min(rect.r), rect.l.max(rect.r));
    let (b, t) = (rect.b.min(rect.t), rect.b.max(rect.t));
    FS_QUADPOINTSF {
        x1: l,
        y1: t,
        x2: r,
        y2: t,
        x3: l,
        y3: b,
        x4: r,
        y4: b,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quad_is_top_left_first() {
        let q = quad_tl(Rect::new(10.0, 20.0, 50.0, 40.0));
        // TL, TR, BL, BR — PDFium reads left from x3, bottom from y3, right from x2, top y2.
        assert_eq!((q.x1, q.y1), (10.0, 40.0));
        assert_eq!((q.x2, q.y2), (50.0, 40.0));
        assert_eq!((q.x3, q.y3), (10.0, 20.0));
        assert_eq!((q.x4, q.y4), (50.0, 20.0));
    }

    #[test]
    fn quad_normalises_an_inverted_rect() {
        let q = quad_tl(Rect::new(50.0, 40.0, 10.0, 20.0));
        assert_eq!((q.x3, q.y3), (10.0, 20.0));
        assert_eq!((q.x2, q.y2), (50.0, 40.0));
    }
}
