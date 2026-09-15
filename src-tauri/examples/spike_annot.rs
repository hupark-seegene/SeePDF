//! Spike: annotations, AcroForm, flatten and redaction with pdfium-render 0.9.4 (PDFium chromium/8057).
//!
//! Run from `src-tauri/`:  `cargo run --example spike_annot`
//! Outputs go to `fixtures/out/spike_annot_*.{pdf,png}`. Findings are printed as `RESULT | ...` lines
//! and summarised at the end. See docs/spikes/annotations.md for the write-up.
//!
//! Two layers are exercised side by side:
//!   * the high-level pdfium-render API (`Pdfium`, `PdfDocument`, `PdfPageAnnotations`, ...)
//!   * the raw FFI trait `PdfiumLibraryBindings` (obtained from a *second* `Pdfium::bind_to_library`
//!     call made before `Pdfium::new`), because pdfium-render 0.9.4 exposes **no** raw handle accessors,
//!     yet several things (ink lists, borders, AP removal, FORM_* text entry, circle creation) are raw-only.

use image::{Rgba, RgbaImage};
use pdfium_render::prelude::*;
use std::ffi::c_void;
use std::os::raw::{c_int, c_ulong};
use std::path::PathBuf;
use std::time::Instant;

type R<T> = Result<T, String>;

fn e<E: std::fmt::Debug>(err: E) -> String {
    format!("{err:?}")
}

// ---------------------------------------------------------------------------------------------
// Constants that pdfium-render does not re-export (values from bindgen/pdfium_7881.rs).
// ---------------------------------------------------------------------------------------------
const FPDF_ANNOT_RENDER_FLAG: c_int = 1; // FPDF_ANNOT
const AP_NORMAL: FPDF_ANNOT_APPEARANCEMODE = 0;
const COLORTYPE_COLOR: FPDFANNOT_COLORTYPE = 0;
const COLORTYPE_INTERIOR: FPDFANNOT_COLORTYPE = 1;
const FLAG_PRINT: c_int = 4;
const FLAT_PRINT: c_int = 1;
const FPDF_FORMFIELD_TEXTFIELD: c_int = 6;
const FPDF_FORMFIELD_CHECKBOX: c_int = 2;
const FILLMODE_ALTERNATE: c_int = 1;

const ST_TEXT: FPDF_ANNOTATION_SUBTYPE = 1;
const ST_LINE: FPDF_ANNOTATION_SUBTYPE = 4;
const ST_SQUARE: FPDF_ANNOTATION_SUBTYPE = 5;
const ST_CIRCLE: FPDF_ANNOTATION_SUBTYPE = 6;
const ST_FREETEXT: FPDF_ANNOTATION_SUBTYPE = 3;
const ST_INK: FPDF_ANNOTATION_SUBTYPE = 15;
const ST_REDACT: FPDF_ANNOTATION_SUBTYPE = 28;

const SUBTYPE_NAMES: [&str; 29] = [
    "Unknown", "Text", "Link", "FreeText", "Line", "Square", "Circle", "Polygon", "Polyline",
    "Highlight", "Underline", "Squiggly", "StrikeOut", "Stamp", "Caret", "Ink", "Popup",
    "FileAttachment", "Sound", "Movie", "Widget", "Screen", "PrinterMark", "TrapNet", "Watermark",
    "3D", "RichMedia", "XFAWidget", "Redact",
];

// ---------------------------------------------------------------------------------------------
// Paths
// ---------------------------------------------------------------------------------------------
fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}
fn fixture(name: &str) -> PathBuf {
    root().join("../fixtures").join(name)
}
fn out(name: &str) -> PathBuf {
    root().join("../fixtures/out").join(name)
}
fn pdfium_lib() -> PathBuf {
    let name = if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        "libpdfium-aarch64-apple-darwin.dylib"
    } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
        "libpdfium-x86_64-apple-darwin.dylib"
    } else if cfg!(target_os = "windows") {
        "pdfium-x86_64-pc-windows-msvc.dll"
    } else {
        "libpdfium-x86_64-unknown-linux-gnu.so"
    };
    root().join("resources/pdfium").join(name)
}

// ---------------------------------------------------------------------------------------------
// Result collection
// ---------------------------------------------------------------------------------------------
struct Report {
    lines: Vec<String>,
}
impl Report {
    fn add(&mut self, key: &str, value: impl std::fmt::Display) {
        let line = format!("{key} | {value}");
        println!("RESULT | {line}");
        self.lines.push(line);
    }
}

// ---------------------------------------------------------------------------------------------
// Rendered page + pixel probes. All probes take PDF user-space rects (y up) and convert.
// ---------------------------------------------------------------------------------------------
struct Png {
    img: RgbaImage,
    scale: f32,
    page_h: f32,
}

impl Png {
    fn from_page(page: &PdfPage, scale: f32, form: bool) -> R<Self> {
        let cfg = PdfRenderConfig::new()
            .scale_page_by_factor(scale)
            .render_form_data(form)
            .render_annotations(true);
        let bmp = page.render_with_config(&cfg).map_err(e)?;
        let img = bmp.as_image().map_err(e)?.to_rgba8();
        Ok(Png { img, scale, page_h: page.height().value })
    }

    fn save(&self, name: &str) -> R<()> {
        self.img.save(out(name)).map_err(e)
    }

    /// Save a crop of `r` (expanded by `margin` pt) for eyeballing.
    fn save_crop(&self, name: &str, r: &PdfRect, margin: f32) -> R<()> {
        let rr = rect(r.left().value - margin, r.bottom().value - margin, r.right().value + margin, r.top().value + margin);
        let (x0, y0, x1, y1) = self.px_rect(&rr);
        let crop = image::imageops::crop_imm(&self.img, x0, y0, (x1 - x0).max(1), (y1 - y0).max(1)).to_image();
        let safe: String = name.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect();
        crop.save(out(&format!("spike_annot_crop_{safe}.png"))).map_err(e)
    }

    fn px_rect(&self, r: &PdfRect) -> (u32, u32, u32, u32) {
        let x0 = (r.left().value * self.scale).max(0.0) as u32;
        let x1 = ((r.right().value * self.scale) as u32).min(self.img.width().saturating_sub(1));
        let y0 = ((self.page_h - r.top().value) * self.scale).max(0.0) as u32;
        let y1 = (((self.page_h - r.bottom().value) * self.scale) as u32)
            .min(self.img.height().saturating_sub(1));
        (x0, y0, x1, y1)
    }

    /// Number of pixels inside `r` whose RGB is within `tol` (per channel) of `rgb`.
    fn count_near(&self, r: &PdfRect, rgb: [u8; 3], tol: i32) -> usize {
        let (x0, y0, x1, y1) = self.px_rect(r);
        let mut n = 0;
        for y in y0..=y1 {
            for x in x0..=x1 {
                let Rgba([pr, pg, pb, _]) = *self.img.get_pixel(x, y);
                if (pr as i32 - rgb[0] as i32).abs() <= tol
                    && (pg as i32 - rgb[1] as i32).abs() <= tol
                    && (pb as i32 - rgb[2] as i32).abs() <= tol
                {
                    n += 1;
                }
            }
        }
        n
    }

    /// Pixels in `r` that differ from `other` by more than `tol` in any channel.
    fn count_diff(&self, other: &Png, r: &PdfRect, tol: i32) -> usize {
        let (x0, y0, x1, y1) = self.px_rect(r);
        let mut n = 0;
        for y in y0..=y1 {
            for x in x0..=x1 {
                let Rgba([a0, a1, a2, _]) = *self.img.get_pixel(x, y);
                let Rgba([b0, b1, b2, _]) = *other.img.get_pixel(x, y);
                if (a0 as i32 - b0 as i32).abs() > tol
                    || (a1 as i32 - b1 as i32).abs() > tol
                    || (a2 as i32 - b2 as i32).abs() > tol
                {
                    n += 1;
                }
            }
        }
        n
    }

    fn count_dark(&self, r: &PdfRect) -> usize {
        let (x0, y0, x1, y1) = self.px_rect(r);
        let mut n = 0;
        for y in y0..=y1 {
            for x in x0..=x1 {
                let Rgba([pr, pg, pb, _]) = *self.img.get_pixel(x, y);
                if (pr as u32 + pg as u32 + pb as u32) < 300 {
                    n += 1;
                }
            }
        }
        n
    }
}

fn rect(left: f32, bottom: f32, right: f32, top: f32) -> PdfRect {
    PdfRect::new_from_values(bottom, left, top, right)
}

/// QuadPoints in the order PDFium (and the PDF spec's viewers) expect for text markup:
/// (x1,y1)=top-left, (x2,y2)=top-right, (x3,y3)=bottom-left, (x4,y4)=bottom-right.
/// NOTE: `PdfQuadPoints::from_rect()` emits BL,BR,TR,TL, which PDFium normalises to a
/// zero-width rectangle -> the markup renders as a 1px sliver. Do not use it for markup.
/// Order-agnostic bounding rect of a quad. `PdfQuadPoints::to_rect()` assumes the
/// from_rect() corner order and returns an empty rect for PDFium-ordered quads.
fn quad_bounds(q: &PdfQuadPoints) -> PdfRect {
    let xs = [q.x1().value, q.x2().value, q.x3().value, q.x4().value];
    let ys = [q.y1().value, q.y2().value, q.y3().value, q.y4().value];
    let min = |a: &[f32; 4]| a.iter().cloned().fold(f32::MAX, f32::min);
    let max = |a: &[f32; 4]| a.iter().cloned().fold(f32::MIN, f32::max);
    rect(min(&xs), min(&ys), max(&xs), max(&ys))
}

fn quad_tl(r: &PdfRect) -> PdfQuadPoints {
    let (l, b, rt, t) = (r.left().value, r.bottom().value, r.right().value, r.top().value);
    PdfQuadPoints::new_from_values(l, t, rt, t, l, b, rt, b)
}

fn fmt_rect(r: &PdfRect) -> String {
    format!(
        "[l={:.1} b={:.1} r={:.1} t={:.1}]",
        r.left().value,
        r.bottom().value,
        r.right().value,
        r.top().value
    )
}

fn fmt_color(c: &Result<PdfColor, PdfiumError>) -> String {
    match c {
        Ok(c) => format!("rgba({},{},{},{})", c.red(), c.green(), c.blue(), c.alpha()),
        Err(_) => "n/a".to_string(),
    }
}

fn fmt_quad(q: &PdfQuadPoints) -> String {
    format!(
        "({:.1},{:.1})({:.1},{:.1})({:.1},{:.1})({:.1},{:.1})",
        q.x1().value,
        q.y1().value,
        q.x2().value,
        q.y2().value,
        q.x3().value,
        q.y3().value,
        q.x4().value,
        q.y4().value
    )
}

// ---------------------------------------------------------------------------------------------
// Raw FFI helpers over `PdfiumLibraryBindings`.
// ---------------------------------------------------------------------------------------------
#[derive(Clone, Copy)]
struct Raw(&'static dyn PdfiumLibraryBindings);

#[repr(C)]
struct FileWriter {
    fw: FPDF_FILEWRITE,
    buf: Vec<u8>,
}

unsafe extern "C" fn write_block(this: *mut FPDF_FILEWRITE, data: *const c_void, size: c_ulong) -> c_int {
    let w = this as *mut FileWriter;
    let slice = std::slice::from_raw_parts(data as *const u8, size as usize);
    (*w).buf.extend_from_slice(slice);
    1
}

fn form_fill_info() -> Box<FPDF_FORMFILLINFO> {
    Box::new(FPDF_FORMFILLINFO {
        version: 2,
        Release: None,
        FFI_Invalidate: None,
        FFI_OutputSelectedRect: None,
        FFI_SetCursor: None,
        FFI_SetTimer: None,
        FFI_KillTimer: None,
        FFI_GetLocalTime: None,
        FFI_OnChange: None,
        FFI_GetPage: None,
        FFI_GetCurrentPage: None,
        FFI_GetRotation: None,
        FFI_ExecuteNamedAction: None,
        FFI_SetTextFieldFocus: None,
        FFI_DoURIAction: None,
        FFI_DoGoToAction: None,
        m_pJsPlatform: std::ptr::null_mut(),
        xfa_disabled: 0,
        FFI_DisplayCaret: None,
        FFI_GetCurrentPageIndex: None,
        FFI_SetCurrentPage: None,
        FFI_GotoURL: None,
        FFI_GetPageViewRect: None,
        FFI_PageEvent: None,
        FFI_PopupMenu: None,
        FFI_OpenFile: None,
        FFI_EmailTo: None,
        FFI_UploadTo: None,
        FFI_GetPlatform: None,
        FFI_GetLanguage: None,
        FFI_DownloadFromURL: None,
        FFI_PostRequestURL: None,
        FFI_PutRequestURL: None,
        FFI_OnFocusChange: None,
        FFI_DoURIActionWithKeyboardModifier: None,
    })
}

impl Raw {
    fn b(&self) -> &'static dyn PdfiumLibraryBindings {
        self.0
    }

    fn get_string(&self, annot: FPDF_ANNOTATION, key: &str) -> Option<String> {
        unsafe {
            if !self.b().is_true(self.b().FPDFAnnot_HasKey(annot, key)) {
                return None;
            }
            let len = self.b().FPDFAnnot_GetStringValue(annot, key, std::ptr::null_mut(), 0);
            if len <= 2 {
                return Some(String::new());
            }
            let mut buf = vec![0u16; (len as usize) / 2];
            self.b().FPDFAnnot_GetStringValue(annot, key, buf.as_mut_ptr() as *mut FPDF_WCHAR, len);
            Some(String::from_utf16_lossy(&buf).trim_end_matches('\0').to_string())
        }
    }

    fn get_number(&self, annot: FPDF_ANNOTATION, key: &str) -> Option<f32> {
        let mut v: f32 = 0.0;
        unsafe {
            if self.b().is_true(self.b().FPDFAnnot_GetNumberValue(annot, key, &mut v)) {
                Some(v)
            } else {
                None
            }
        }
    }

    /// Normal-mode appearance stream content, `None` if the annotation has no /AP /N.
    fn get_ap(&self, annot: FPDF_ANNOTATION) -> Option<String> {
        unsafe {
            let len = self.b().FPDFAnnot_GetAP(annot, AP_NORMAL, std::ptr::null_mut(), 0);
            if len <= 2 {
                return None;
            }
            let mut buf = vec![0u16; (len as usize) / 2];
            self.b().FPDFAnnot_GetAP(annot, AP_NORMAL, buf.as_mut_ptr() as *mut FPDF_WCHAR, len);
            Some(String::from_utf16_lossy(&buf).trim_end_matches('\0').to_string())
        }
    }

    fn ink_paths(&self, annot: FPDF_ANNOTATION) -> Vec<Vec<(f32, f32)>> {
        let mut paths = vec![];
        unsafe {
            let n = self.b().FPDFAnnot_GetInkListCount(annot);
            for i in 0..n {
                let count = self.b().FPDFAnnot_GetInkListPath(annot, i, std::ptr::null_mut(), 0);
                let mut buf = vec![FS_POINTF { x: 0.0, y: 0.0 }; count as usize];
                if count > 0 {
                    self.b().FPDFAnnot_GetInkListPath(annot, i, buf.as_mut_ptr(), count);
                }
                paths.push(buf.iter().map(|p| (p.x, p.y)).collect());
            }
        }
        paths
    }

    fn line(&self, annot: FPDF_ANNOTATION) -> Option<((f32, f32), (f32, f32))> {
        let mut s = FS_POINTF { x: 0.0, y: 0.0 };
        let mut t = FS_POINTF { x: 0.0, y: 0.0 };
        unsafe {
            if self.b().is_true(self.b().FPDFAnnot_GetLine(annot, &mut s, &mut t)) {
                Some(((s.x, s.y), (t.x, t.y)))
            } else {
                None
            }
        }
    }

    fn vertex_count(&self, annot: FPDF_ANNOTATION) -> usize {
        unsafe { self.b().FPDFAnnot_GetVertices(annot, std::ptr::null_mut(), 0) as usize }
    }

    fn border(&self, annot: FPDF_ANNOTATION) -> Option<(f32, f32, f32)> {
        let (mut h, mut v, mut w) = (0f32, 0f32, 0f32);
        unsafe {
            if self.b().is_true(self.b().FPDFAnnot_GetBorder(annot, &mut h, &mut v, &mut w)) {
                Some((h, v, w))
            } else {
                None
            }
        }
    }

    fn rect(&self, annot: FPDF_ANNOTATION) -> Option<FS_RECTF> {
        let mut r = FS_RECTF { left: 0.0, top: 0.0, right: 0.0, bottom: 0.0 };
        unsafe {
            if self.b().is_true(self.b().FPDFAnnot_GetRect(annot, &mut r)) {
                Some(r)
            } else {
                None
            }
        }
    }

    fn set_rect(&self, annot: FPDF_ANNOTATION, r: &PdfRect) -> bool {
        let fr = FS_RECTF {
            left: r.left().value,
            top: r.top().value,
            right: r.right().value,
            bottom: r.bottom().value,
        };
        unsafe { self.b().is_true(self.b().FPDFAnnot_SetRect(annot, &fr)) }
    }

    fn has_linked(&self, annot: FPDF_ANNOTATION, key: &str) -> bool {
        unsafe {
            let h = self.b().FPDFAnnot_GetLinkedAnnot(annot, key);
            if h.is_null() {
                false
            } else {
                self.b().FPDFPage_CloseAnnot(h);
                true
            }
        }
    }

    fn color(&self, annot: FPDF_ANNOTATION, ty: FPDFANNOT_COLORTYPE) -> Option<[u32; 4]> {
        let (mut r, mut g, mut b, mut a) = (0u32, 0u32, 0u32, 0u32);
        unsafe {
            if self.b().is_true(self.b().FPDFAnnot_GetColor(annot, ty, &mut r, &mut g, &mut b, &mut a)) {
                Some([r, g, b, a])
            } else {
                None
            }
        }
    }

    fn set_color(&self, annot: FPDF_ANNOTATION, ty: FPDFANNOT_COLORTYPE, c: [u32; 4]) -> bool {
        unsafe { self.b().is_true(self.b().FPDFAnnot_SetColor(annot, ty, c[0], c[1], c[2], c[3])) }
    }

    fn save(&self, doc: FPDF_DOCUMENT, name: &str) -> R<usize> {
        let mut w = Box::new(FileWriter {
            fw: FPDF_FILEWRITE { version: 1, WriteBlock: Some(write_block) },
            buf: Vec::new(),
        });
        let ok = unsafe { self.b().is_true(self.b().FPDF_SaveAsCopy(doc, &mut w.fw as *mut FPDF_FILEWRITE, 0)) };
        if !ok {
            return Err("FPDF_SaveAsCopy failed".into());
        }
        std::fs::write(out(name), &w.buf).map_err(e)?;
        Ok(w.buf.len())
    }

    fn text(&self, page: FPDF_PAGE) -> String {
        unsafe {
            let tp = self.b().FPDFText_LoadPage(page);
            let n = self.b().FPDFText_CountChars(tp);
            let mut buf = vec![0u16; (n + 1) as usize];
            let got = self.b().FPDFText_GetText(tp, 0, n, buf.as_mut_ptr());
            self.b().FPDFText_ClosePage(tp);
            String::from_utf16_lossy(&buf[..got.max(0) as usize]).trim_end_matches('\0').to_string()
        }
    }

    /// Render with FPDF_ANNOT, optionally followed by FPDF_FFLDraw for live form state.
    fn render(&self, page: FPDF_PAGE, form: Option<FPDF_FORMHANDLE>, scale: f32) -> Png {
        unsafe {
            let pw = self.b().FPDF_GetPageWidthF(page);
            let ph = self.b().FPDF_GetPageHeightF(page);
            let w = (pw * scale) as c_int;
            let h = (ph * scale) as c_int;
            let bmp = self.b().FPDFBitmap_Create(w, h, 0);
            let _ = self.b().FPDFBitmap_FillRect(bmp, 0, 0, w, h, 0xFFFF_FFFF);
            self.b().FPDF_RenderPageBitmap(bmp, page, 0, 0, w, h, 0, FPDF_ANNOT_RENDER_FLAG);
            if let Some(f) = form {
                self.b().FPDF_FFLDraw(f, bmp, page, 0, 0, w, h, 0, FPDF_ANNOT_RENDER_FLAG);
            }
            let stride = self.b().FPDFBitmap_GetStride(bmp) as usize;
            let buf = self.b().FPDFBitmap_GetBuffer(bmp) as *const u8;
            let mut img = RgbaImage::new(w as u32, h as u32);
            for y in 0..h as usize {
                let row = std::slice::from_raw_parts(buf.add(y * stride), (w as usize) * 4);
                for x in 0..w as usize {
                    let p = &row[x * 4..x * 4 + 4]; // BGRx
                    img.put_pixel(x as u32, y as u32, Rgba([p[2], p[1], p[0], 255]));
                }
            }
            self.b().FPDFBitmap_Destroy(bmp);
            Png { img, scale, page_h: ph }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Section 1: enumerate existing annotations (high-level + raw), delete one, save, confirm.
// ---------------------------------------------------------------------------------------------
fn describe_annotation(i: usize, a: &PdfPageAnnotation) -> String {
    let objs = a.objects().len();
    let quads: Vec<String> = a.attachment_points().iter().map(|q| fmt_quad(&q)).collect();
    format!(
        "#{i} {:?} supported={} bounds={} contents={:?} author={:?} modified={:?} created={:?} name={:?} stroke={} fill={} hidden={} print={} objects={} quads={}",
        a.annotation_type(),
        a.is_supported(),
        a.bounds().map(|r| fmt_rect(&r)).unwrap_or_else(|_| "n/a".into()),
        a.contents(),
        a.creator(),
        a.modification_date(),
        a.creation_date(),
        a.name(),
        fmt_color(&a.stroke_color()),
        fmt_color(&a.fill_color()),
        a.is_hidden(),
        a.is_printed(),
        objs,
        quads.join(" ")
    )
}

fn raw_describe_page(raw: Raw, path: &PathBuf, page_index: i32) -> R<Vec<String>> {
    let mut lines = vec![];
    unsafe {
        let doc = raw.b().FPDF_LoadDocument(path.to_str().unwrap(), None);
        if doc.is_null() {
            return Err("raw load failed".into());
        }
        let page = raw.b().FPDF_LoadPage(doc, page_index);
        let n = raw.b().FPDFPage_GetAnnotCount(page);
        for i in 0..n {
            let a = raw.b().FPDFPage_GetAnnot(page, i);
            let st = raw.b().FPDFAnnot_GetSubtype(a);
            let name = SUBTYPE_NAMES.get(st as usize).copied().unwrap_or("?");
            let ink = raw.ink_paths(a);
            let ink_desc: Vec<String> = ink.iter().map(|p| format!("{}pts", p.len())).collect();
            let ap = raw.get_ap(a);
            lines.push(format!(
                "#{i} {name} CA={:?} border={:?} inkPaths=[{}] line={:?} vertices={} popupLink={} parentLink={} IRT={} AP_len={:?} Subj={:?} DA={:?} BS/W={:?}",
                raw.get_number(a, "CA"),
                raw.border(a),
                ink_desc.join(","),
                raw.line(a),
                raw.vertex_count(a),
                raw.has_linked(a, "Popup"),
                raw.has_linked(a, "Parent"),
                raw.has_linked(a, "IRT"),
                ap.as_ref().map(|s| s.len()),
                raw.get_string(a, "Subj"),
                raw.get_string(a, "DA"),
                raw.get_number(a, "BS"),
            ));
            raw.b().FPDFPage_CloseAnnot(a);
        }
        raw.b().FPDF_ClosePage(page);
        raw.b().FPDF_CloseDocument(doc);
    }
    Ok(lines)
}

fn section1_enumerate(pdfium: &Pdfium, raw: Raw, rep: &mut Report) -> R<()> {
    println!("\n==== Section 1: enumerate existing annotations ====");
    for name in ["annotation-highlight.pdf", "annotation-line.pdf", "annotation-freetext.pdf", "160F-2019.pdf"] {
        let path = fixture(name);
        let t = Instant::now();
        let document = pdfium.load_pdf_from_file(&path, None).map_err(e)?;
        let pages = document.pages().len();
        let mut total = 0usize;
        for (pi, page) in document.pages().iter().enumerate() {
            let annots = page.annotations();
            total += annots.len();
            if name == "160F-2019.pdf" && pi > 0 {
                continue; // widgets are covered in section 4
            }
            for (i, a) in annots.iter().enumerate() {
                if name == "160F-2019.pdf" && i >= 3 {
                    println!("  page {pi}: ... ({} annotations total on this page, rest omitted)", annots.len());
                    break;
                }
                println!("  page {pi}: {}", describe_annotation(i, &a));
                if let Some(link) = a.as_link_annotation() {
                    match link.link() {
                        Ok(l) => {
                            let uri = l.action().and_then(|act| act.as_uri_action().and_then(|u| u.uri().ok()));
                            let dest = l.destination().and_then(|d| d.page_index().ok());
                            println!("      link: uri={uri:?} dest_page={dest:?}");
                        }
                        Err(err) => println!("      link: no FPDF_LINK ({err:?})"),
                    }
                }
            }
            if name != "160F-2019.pdf" {
                for l in raw_describe_page(raw, &path, pi as i32)? {
                    println!("  raw  {pi}: {l}");
                }
            }
        }
        rep.add(
            &format!("enumerate {name}"),
            format!("{pages} pages, {total} annotations, {:?}", t.elapsed()),
        );
    }

    // Delete: remove annotation #0 from annotation-highlight.pdf, save, re-open.
    {
        let path = fixture("annotation-highlight.pdf");
        let document = pdfium.load_pdf_from_file(&path, None).map_err(e)?;
        let mut page = document.pages().get(0).map_err(e)?;
        let before = page.annotations().len();
        let first = page.annotations_mut().get(0).map_err(e)?; // via *_mut(): lifetime is the page's 'a, not the borrow
        let deleted_type = first.annotation_type();
        page.annotations_mut().delete_annotation(first).map_err(e)?;
        let after_mem = page.annotations().len();
        drop(page);
        document.save_to_file(&out("spike_annot_highlight_deleted.pdf")).map_err(e)?;
        let re = pdfium.load_pdf_from_file(&out("spike_annot_highlight_deleted.pdf"), None).map_err(e)?;
        let after_disk = re.pages().get(0).map_err(e)?.annotations().len();
        rep.add(
            "delete annotation",
            format!("deleted #0 ({deleted_type:?}): count {before} -> {after_mem} in memory -> {after_disk} after save+reopen"),
        );
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Section 2: create every annotation type on tracemonkey page 0 (high-level API).
// ---------------------------------------------------------------------------------------------
struct Probe {
    name: &'static str,
    rect: PdfRect,
    color: [u8; 3],
    tol: i32,
}

fn find_word(page: &PdfPage, word: &str, nth: usize) -> Option<PdfRect> {
    let text = page.text().ok()?;
    let search = text.search(word, &PdfSearchOptions::new()).ok()?;
    let mut idx = 0;
    while let Some(segments) = search.find_next() {
        if idx == nth {
            let seg = segments.iter().next()?;
            return Some(seg.bounds());
        }
        idx += 1;
    }
    None
}

fn section2_create(pdfium: &Pdfium, raw: Raw, rep: &mut Report) -> R<Vec<Probe>> {
    println!("\n==== Section 2: create annotations on tracemonkey.pdf page 0 (high-level) ====");
    let mut document = pdfium.load_pdf_from_file(&fixture("tracemonkey.pdf"), None).map_err(e)?;
    let helv = document.fonts_mut().helvetica();
    let baseline = {
        let page = document.pages().get(0).map_err(e)?;
        let t = Instant::now();
        let png = Png::from_page(&page, 2.0, true)?;
        rep.add("bench render tracemonkey p0 @2x (1224x1584)", format!("{:?}", t.elapsed()));
        png.save("spike_annot_baseline.png")?;
        png
    };

    // Micro-benchmark: cost of the default AutomaticOnEveryChange strategy (FPDFPage_GenerateContent per call).
    for strategy in [PdfPageContentRegenerationStrategy::AutomaticOnEveryChange, PdfPageContentRegenerationStrategy::Manual] {
        let d = pdfium.load_pdf_from_file(&fixture("tracemonkey.pdf"), None).map_err(e)?;
        let mut p = d.pages().get(0).map_err(e)?;
        p.set_content_regeneration_strategy(strategy);
        let t = Instant::now();
        for i in 0..20 {
            let mut a = p.annotations_mut().create_square_annotation().map_err(e)?;
            a.set_bounds(rect(10.0 + i as f32, 10.0, 30.0 + i as f32, 30.0)).map_err(e)?;
            a.set_stroke_color(PdfColor::RED).map_err(e)?;
        }
        rep.add(&format!("bench create 20 squares with {strategy:?}"), format!("{:?}", t.elapsed()));
    }

    let mut probes: Vec<Probe> = vec![];
    let mut page = document.pages().get(0).map_err(e)?;
    println!("  page size {}x{} pt, regeneration strategy {:?}", page.width().value, page.height().value, page.content_regeneration_strategy());
    // Annotation-only edits do not need content-stream regeneration; avoid FPDFPage_GenerateContent on every call.
    page.set_content_regeneration_strategy(PdfPageContentRegenerationStrategy::Manual);

    // Word positions for text-markup annotations (fall back to fixed rects if not found).
    let w_trace0 = find_word(&page, "trace", 0).unwrap_or(rect(100.0, 700.0, 160.0, 712.0));
    let w_trace1 = find_word(&page, "trace", 1).unwrap_or(rect(100.0, 680.0, 160.0, 692.0));
    let w_compiler = find_word(&page, "compiler", 0).unwrap_or(rect(100.0, 600.0, 160.0, 612.0));
    let w_javascript = find_word(&page, "JavaScript", 0).unwrap_or(rect(100.0, 580.0, 160.0, 592.0));
    let w_abstract = find_word(&page, "Abstract", 0).unwrap_or(rect(100.0, 560.0, 160.0, 572.0));
    println!(
        "  words: trace0={} trace1={} compiler={} JavaScript={} Abstract={}",
        fmt_rect(&w_trace0), fmt_rect(&w_trace1), fmt_rect(&w_compiler), fmt_rect(&w_javascript), fmt_rect(&w_abstract)
    );

    let t_create = Instant::now();

    // 1. Highlight with two explicit quads (two different words), yellow, 60% opacity via alpha.
    {
        let union = rect(
            w_trace0.left().value.min(w_trace1.left().value),
            w_trace0.bottom().value.min(w_trace1.bottom().value),
            w_trace0.right().value.max(w_trace1.right().value),
            w_trace0.top().value.max(w_trace1.top().value),
        );
        let mut a = page.annotations_mut().create_highlight_annotation().map_err(e)?;
        a.set_bounds(union).map_err(e)?;
        a.set_stroke_color(PdfColor::new(255, 255, 0, 153)).map_err(e)?; // alpha -> /CA 0.6
        a.attachment_points_mut().create_attachment_point_at_end(quad_tl(&w_trace0)).map_err(e)?;
        a.attachment_points_mut().create_attachment_point_at_end(quad_tl(&w_trace1)).map_err(e)?;
        a.set_contents("Highlight with two quads").map_err(e)?;
        a.set_creator("SeePDF spike").map_err(e)?;
        a.set_is_printed(true).map_err(e)?;
        probes.push(Probe { name: "highlight(quad0)", rect: w_trace0, color: [255, 255, 0], tol: 90 });
        probes.push(Probe { name: "highlight(quad1)", rect: w_trace1, color: [255, 255, 0], tol: 90 });
    }
    // 2. Underline (blue)
    {
        let mut a = page.annotations_mut().create_underline_annotation().map_err(e)?;
        a.set_bounds(w_compiler).map_err(e)?;
        a.set_stroke_color(PdfColor::BLUE).map_err(e)?;
        a.attachment_points_mut().create_attachment_point_at_end(quad_tl(&w_compiler)).map_err(e)?;
        a.set_is_printed(true).map_err(e)?;
        probes.push(Probe { name: "underline", rect: w_compiler, color: [0, 0, 255], tol: 60 });
    }
    // 3. Strikeout (red)
    {
        let mut a = page.annotations_mut().create_strikeout_annotation().map_err(e)?;
        a.set_bounds(w_javascript).map_err(e)?;
        a.set_stroke_color(PdfColor::RED).map_err(e)?;
        a.attachment_points_mut().create_attachment_point_at_end(quad_tl(&w_javascript)).map_err(e)?;
        a.set_is_printed(true).map_err(e)?;
        probes.push(Probe { name: "strikeout", rect: w_javascript, color: [255, 0, 0], tol: 60 });
    }
    // 4. Squiggly (green)
    {
        let mut a = page.annotations_mut().create_squiggly_annotation().map_err(e)?;
        a.set_bounds(w_abstract).map_err(e)?;
        a.set_stroke_color(PdfColor::new(0, 160, 0, 255)).map_err(e)?;
        a.attachment_points_mut().create_attachment_point_at_end(quad_tl(&w_abstract)).map_err(e)?;
        a.set_is_printed(true).map_err(e)?;
        probes.push(Probe { name: "squiggly", rect: w_abstract, color: [0, 160, 0], tol: 70 });
    }
    // 5. Ink via page-objects (pdfium-render has no ink-list API): two strokes, red, 3pt.
    {
        let r = rect(8.0, 300.0, 46.0, 500.0);
        let mut a = page.annotations_mut().create_ink_annotation().map_err(e)?;
        a.set_bounds(r).map_err(e)?; // MUST precede add_object: the AP BBox is taken from /Rect at creation.
        a.set_stroke_color(PdfColor::RED).map_err(e)?;
        a.set_is_printed(true).map_err(e)?;
        for (sx, ex) in [(12.0f32, 42.0f32), (42.0, 12.0)] {
            let mut p = PdfPagePathObject::new(&document, PdfPoints::new(sx), PdfPoints::new(310.0), Some(PdfColor::RED), Some(PdfPoints::new(3.0)), None).map_err(e)?;
            p.line_to(PdfPoints::new(ex), PdfPoints::new(400.0)).map_err(e)?;
            p.line_to(PdfPoints::new(sx), PdfPoints::new(490.0)).map_err(e)?;
            a.objects_mut().add_path_object(p).map_err(e)?;
        }
        probes.push(Probe { name: "ink(path objects)", rect: r, color: [255, 0, 0], tol: 60 });
    }
    // 6. Square, blue stroke + light-blue fill.
    {
        let r = rect(565.0, 600.0, 605.0, 660.0);
        let mut a = page.annotations_mut().create_square_annotation().map_err(e)?;
        a.set_bounds(r).map_err(e)?;
        a.set_stroke_color(PdfColor::BLUE).map_err(e)?;
        a.set_fill_color(PdfColor::new(200, 220, 255, 255)).map_err(e)?;
        a.set_is_printed(true).map_err(e)?;
        probes.push(Probe { name: "square(fill)", rect: rect(572.0, 607.0, 598.0, 653.0), color: [200, 220, 255], tol: 30 });
    }
    // 7. Circle: NO high-level constructor in 0.9.4 -> raw section.
    // 8. Line: pdfium cannot create Line annotations -> raw section.
    // 9. FreeText (pdfium generates no AP for FreeText -> expected invisible).
    {
        let r = rect(300.0, 750.0, 560.0, 780.0);
        let mut a = page.annotations_mut().create_free_text_annotation("SeePDF FreeText").map_err(e)?;
        a.set_bounds(r).map_err(e)?;
        a.set_stroke_color(PdfColor::BLUE).map_err(e)?;
        a.set_is_printed(true).map_err(e)?;
        probes.push(Probe { name: "freetext(no AP)", rect: r, color: [0, 0, 255], tol: 80 });
    }
    // 9b. FreeText emulation: Stamp containing a text object + background path (portable AP).
    {
        let r = rect(60.0, 750.0, 290.0, 780.0);
        let mut a = page.annotations_mut().create_stamp_annotation().map_err(e)?;
        a.set_bounds(r).map_err(e)?;
        a.set_is_printed(true).map_err(e)?;
        a.set_contents("SeePDF stamp-text").map_err(e)?;
        let bg = PdfPagePathObject::new_rect(&document, r, Some(PdfColor::BLUE), Some(PdfPoints::new(1.0)), Some(PdfColor::new(255, 255, 200, 255))).map_err(e)?;
        a.objects_mut().add_path_object(bg).map_err(e)?;
        let mut t = PdfPageTextObject::new(&document, "SeePDF text-in-stamp 한글", helv, PdfPoints::new(14.0)).map_err(e)?;
        t.set_fill_color(PdfColor::new(0, 0, 200, 255)).map_err(e)?;
        t.translate(PdfPoints::new(66.0), PdfPoints::new(760.0)).map_err(e)?; // BEFORE add: no FPDFAnnot_UpdateObject after
        a.objects_mut().add_text_object(t).map_err(e)?;
        probes.push(Probe { name: "stamp(text+bg)", rect: r, color: [0, 0, 200], tol: 80 });
    }
    // 10. Stamp with an image (64x64 gradient, magenta-ish).
    {
        let r = rect(565.0, 400.0, 605.0, 440.0);
        let mut a = page.annotations_mut().create_stamp_annotation().map_err(e)?;
        a.set_bounds(r).map_err(e)?;
        a.set_is_printed(true).map_err(e)?;
        let mut img = RgbaImage::new(64, 64);
        for (x, y, p) in img.enumerate_pixels_mut() {
            *p = Rgba([255, (x * 4) as u8, (y * 4) as u8, 255]);
        }
        let dynimg = image::DynamicImage::ImageRgba8(img);
        let mut obj = PdfPageImageObject::new_with_size(&document, &dynimg, PdfPoints::new(40.0), PdfPoints::new(40.0)).map_err(e)?;
        obj.translate(PdfPoints::new(565.0), PdfPoints::new(400.0)).map_err(e)?;
        a.objects_mut().add_image_object(obj).map_err(e)?;
        probes.push(Probe { name: "stamp(image)", rect: rect(566.0, 401.0, 604.0, 439.0), color: [255, 60, 60], tol: 50 });
    }
    // 11. Link (URI) over "Abstract" + a quad; invisible by design.
    {
        let mut a = page.annotations_mut().create_link_annotation("https://example.com/seepdf").map_err(e)?;
        a.set_bounds(w_abstract).map_err(e)?;
        a.attachment_points_mut().create_attachment_point_at_end(quad_tl(&w_abstract)).map_err(e)?;
    }
    // 12. Text (sticky note).
    {
        let r = rect(10.0, 700.0, 30.0, 720.0);
        let mut a = page.annotations_mut().create_text_annotation("Sticky note contents").map_err(e)?;
        a.set_bounds(r).map_err(e)?;
        a.set_stroke_color(PdfColor::YELLOW).map_err(e)?;
        a.set_creator("SeePDF spike").map_err(e)?;
        a.set_is_printed(true).map_err(e)?;
        probes.push(Probe { name: "text(sticky)", rect: r, color: [255, 255, 0], tol: 90 });
    }
    // 13. Popup (standalone; cannot be linked to a parent via the public API).
    {
        let r = rect(565.0, 300.0, 605.0, 380.0);
        let mut a = page.annotations_mut().create_popup_annotation().map_err(e)?;
        a.set_bounds(r).map_err(e)?;
        a.set_contents("Popup body").map_err(e)?;
        probes.push(Probe { name: "popup(unlinked)", rect: r, color: [255, 255, 0], tol: 90 });
    }
    rep.add("bench create 13 annotations (Manual regeneration)", format!("{:?}", t_create.elapsed()));
    rep.add("annotations after create", page.annotations().len());

    // Save WITHOUT rendering first, then render, then save again.
    let t = Instant::now();
    drop(page);
    document.save_to_file(&out("spike_annot_norender.pdf")).map_err(e)?;
    rep.add("bench save (no render)", format!("{:?}", t.elapsed()));

    let page = document.pages().get(0).map_err(e)?;
    let before_save = Png::from_page(&page, 2.0, true)?;
    before_save.save("spike_annot_before_save.png")?;
    drop(page);
    let t = Instant::now();
    document.save_to_file(&out("spike_annot.pdf")).map_err(e)?;
    rep.add("bench save (after render)", format!("{:?}", t.elapsed()));
    drop(document);

    // Re-open both and compare.
    for (file, label) in [("spike_annot_norender.pdf", "norender"), ("spike_annot.pdf", "rendered")] {
        let re = pdfium.load_pdf_from_file(&out(file), None).map_err(e)?;
        let page = re.pages().get(0).map_err(e)?;
        println!("  reopened {file}: {} annotations", page.annotations().len());
        for (i, a) in page.annotations().iter().enumerate() {
            println!("    {}", describe_annotation(i, &a));
            if let Some(link) = a.as_link_annotation() {
                let uri = link.link().ok().and_then(|l| l.action().and_then(|act| act.as_uri_action().and_then(|u| u.uri().ok())));
                println!("      link uri={uri:?}");
            }
        }
        let ap: Vec<String> = raw_describe_page(raw, &out(file), 0)?;
        for l in &ap {
            println!("    raw {l}");
        }
        let png = Png::from_page(&page, 2.0, true)?;
        png.save(&format!("spike_annot_reopened_{label}.png"))?;
        for p in &probes {
            let n = png.count_near(&p.rect, p.color, p.tol);
            let base = baseline.count_near(&p.rect, p.color, p.tol);
            let diff = png.count_diff(&baseline, &p.rect, 40);
            rep.add(
                &format!("render[{label}] {}", p.name),
                format!("{} px near {:?} (baseline {}), {} px differ from baseline -> {}", n, p.color, base, diff, if n > base || diff > 50 { "VISIBLE" } else { "NOT VISIBLE" }),
            );
            png.save_crop(&format!("{}_{}", label, p.name), &p.rect, 6.0)?;
        }
    }
    Ok(probes)
}

// ---------------------------------------------------------------------------------------------
// Section 2b: raw FFI creation (circle, ink-list, line, freetext AP, opacity, border).
// ---------------------------------------------------------------------------------------------
fn section2b_raw_create(pdfium: &Pdfium, raw: Raw, rep: &mut Report) -> R<()> {
    println!("\n==== Section 2b: raw FFI creation on tracemonkey.pdf page 0 ====");
    let supported: Vec<&str> = (0..29)
        .filter(|&st| unsafe { raw.b().is_true(raw.b().FPDFAnnot_IsSupportedSubtype(st as FPDF_ANNOTATION_SUBTYPE)) })
        .map(|st| SUBTYPE_NAMES[st as usize])
        .collect();
    rep.add("FPDFAnnot_IsSupportedSubtype (creatable)", supported.join(","));
    let obj_supported: Vec<&str> = (0..29)
        .filter(|&st| unsafe { raw.b().is_true(raw.b().FPDFAnnot_IsObjectSupportedSubtype(st as FPDF_ANNOTATION_SUBTYPE)) })
        .map(|st| SUBTYPE_NAMES[st as usize])
        .collect();
    rep.add("FPDFAnnot_IsObjectSupportedSubtype (AppendObject)", obj_supported.join(","));

    let baseline_raw;
    unsafe {
        let path = fixture("tracemonkey.pdf");
        let doc = raw.b().FPDF_LoadDocument(path.to_str().unwrap(), None);
        let page = raw.b().FPDF_LoadPage(doc, 0);
        baseline_raw = raw.render(page, None, 2.0);

        // Circle with 50% opacity (alpha 128 -> /CA), yellow interior, 3pt border.
        let circle = raw.b().FPDFPage_CreateAnnot(page, ST_CIRCLE);
        rep.add("raw create Circle", format!("handle null={}", circle.is_null()));
        if !circle.is_null() {
            raw.set_rect(circle, &rect(565.0, 500.0, 605.0, 560.0));
            rep.add("raw circle SetColor(C, alpha=128)", raw.set_color(circle, COLORTYPE_COLOR, [255, 0, 0, 128]));
            // Every FPDFAnnot_SetColor call rewrites /CA from its alpha: pass the same alpha for IC.
            raw.set_color(circle, COLORTYPE_INTERIOR, [255, 255, 0, 128]);
            rep.add("raw circle SetBorder(0,0,3)", raw.b().is_true(raw.b().FPDFAnnot_SetBorder(circle, 0.0, 0.0, 3.0)));
            rep.add("raw circle CA readback", format!("{:?}", raw.get_number(circle, "CA")));
            rep.add("raw circle border readback", format!("{:?}", raw.border(circle)));
            raw.b().FPDFAnnot_SetFlags(circle, FLAG_PRINT);
            raw.b().FPDFPage_CloseAnnot(circle);
        }
        // Ink with a real /InkList (two strokes), blue, 4pt.
        let ink = raw.b().FPDFPage_CreateAnnot(page, ST_INK);
        rep.add("raw create Ink", format!("handle null={}", ink.is_null()));
        if !ink.is_null() {
            raw.set_rect(ink, &rect(8.0, 100.0, 46.0, 280.0));
            raw.set_color(ink, COLORTYPE_COLOR, [0, 0, 255, 255]);
            raw.b().FPDFAnnot_SetBorder(ink, 0.0, 0.0, 4.0);
            let s1: Vec<FS_POINTF> = (0..10).map(|i| FS_POINTF { x: 12.0 + (i % 2) as f32 * 30.0, y: 110.0 + i as f32 * 8.0 }).collect();
            let s2: Vec<FS_POINTF> = (0..10).map(|i| FS_POINTF { x: 12.0 + i as f32 * 3.3, y: 200.0 + (i % 2) as f32 * 60.0 }).collect();
            let i1 = raw.b().FPDFAnnot_AddInkStroke(ink, s1.as_ptr(), s1.len());
            let i2 = raw.b().FPDFAnnot_AddInkStroke(ink, s2.as_ptr(), s2.len());
            rep.add("raw ink AddInkStroke", format!("indices {i1},{i2}; InkList count {}", raw.b().FPDFAnnot_GetInkListCount(ink)));
            raw.b().FPDFAnnot_SetFlags(ink, FLAG_PRINT);
            raw.b().FPDFPage_CloseAnnot(ink);
        }
        // Line / Redact / Polygon: expected unsupported.
        for (st, name) in [(ST_LINE, "Line"), (ST_REDACT, "Redact"), (7, "Polygon"), (8, "Polyline"), (17, "FileAttachment")] {
            let h = raw.b().FPDFPage_CreateAnnot(page, st);
            rep.add(&format!("raw create {name}"), format!("handle null={}", h.is_null()));
            if !h.is_null() {
                raw.b().FPDFPage_CloseAnnot(h);
            }
        }
        // FreeText with DA only, then with an explicit AP stream (no font resources: works only in PDFium).
        let ft = raw.b().FPDFPage_CreateAnnot(page, ST_FREETEXT);
        rep.add("raw create FreeText", format!("handle null={}", ft.is_null()));
        if !ft.is_null() {
            let r = rect(60.0, 700.0, 290.0, 730.0);
            raw.set_rect(ft, &r);
            raw.b().FPDFAnnot_SetStringValue_str(ft, "DA", "0 0 1 rg /Helv 12 Tf");
            raw.b().FPDFAnnot_SetStringValue_str(ft, "Contents", "raw FreeText via DA");
            raw.b().FPDFAnnot_SetFlags(ft, FLAG_PRINT);
            let png = raw.render(page, None, 2.0);
            let n = png.count_diff(&baseline_raw, &r, 40);
            rep.add("raw FreeText DA-only render", format!("{n} px differ -> {}; AP generated by pdfium: {:?}", if n > 0 { "VISIBLE" } else { "NOT VISIBLE (no AP generated)" }, raw.get_ap(ft).map(|s| s.chars().take(160).collect::<String>())));
            png.save_crop("raw_freetext_DA_only", &r, 6.0)?;
            let ap = "0 0 1 RG 1 1 0.8 rg 60 700 230 30 re B BT 1 0 0 rg /Helv 14 Tf 66 710 Td (raw FreeText with SetAP) Tj ET";
            let ok = raw.b().is_true(raw.b().FPDFAnnot_SetAP_str(ft, AP_NORMAL, ap));
            let png = raw.render(page, None, 2.0);
            let n = png.count_diff(&baseline_raw, &r, 40);
            rep.add("raw FreeText SetAP render", format!("SetAP ok={ok}, {n} px differ -> {}", if n > 0 { "VISIBLE" } else { "NOT VISIBLE" }));
            rep.add("raw FreeText AP readback", format!("{:?}", raw.get_ap(ft).map(|s| s.len())));
            raw.b().FPDFPage_CloseAnnot(ft);
        }
        // Sticky note: check what AP pdfium generates.
        let tx = raw.b().FPDFPage_CreateAnnot(page, ST_TEXT);
        if !tx.is_null() {
            raw.set_rect(tx, &rect(10.0, 650.0, 30.0, 670.0));
            raw.set_color(tx, COLORTYPE_COLOR, [255, 200, 0, 255]);
            raw.b().FPDFAnnot_SetStringValue_str(tx, "Contents", "raw sticky");
            raw.b().FPDFAnnot_SetFlags(tx, FLAG_PRINT);
            let _ = raw.render(page, None, 1.0);
            rep.add("raw Text(sticky) generated AP", format!("{:?}", raw.get_ap(tx)));
            raw.b().FPDFPage_CloseAnnot(tx);
        }
        // Square via raw for the later re-color test.
        let sq = raw.b().FPDFPage_CreateAnnot(page, ST_SQUARE);
        if !sq.is_null() {
            raw.set_rect(sq, &rect(565.0, 200.0, 605.0, 260.0));
            raw.set_color(sq, COLORTYPE_COLOR, [0, 128, 0, 255]);
            raw.set_color(sq, COLORTYPE_INTERIOR, [0, 255, 0, 255]);
            raw.b().FPDFAnnot_SetBorder(sq, 0.0, 0.0, 2.0);
            raw.b().FPDFAnnot_SetFlags(sq, FLAG_PRINT);
            raw.b().FPDFPage_CloseAnnot(sq);
        }

        let png = raw.render(page, None, 2.0);
        png.save("spike_annot_raw_before_save.png")?;
        // GetAP after rendering: AP now generated for supported types.
        for l in {
            let n = raw.b().FPDFPage_GetAnnotCount(page);
            let mut v = vec![];
            for i in 0..n {
                let a = raw.b().FPDFPage_GetAnnot(page, i);
                let st = raw.b().FPDFAnnot_GetSubtype(a);
                v.push(format!("#{i} {} AP_len={:?}", SUBTYPE_NAMES[st as usize], raw.get_ap(a).map(|s| s.len())));
                raw.b().FPDFPage_CloseAnnot(a);
            }
            v
        } {
            println!("  after render: {l}");
        }
        let bytes = raw.save(doc, "spike_annot_raw.pdf")?;
        rep.add("raw save spike_annot_raw.pdf", format!("{bytes} bytes"));
        raw.b().FPDF_ClosePage(page);
        raw.b().FPDF_CloseDocument(doc);
    }

    // FreeText persistence: does the AP pdfium generates from /DA survive FPDF_SaveAsCopy?
    unsafe {
        let path = fixture("tracemonkey.pdf");
        let doc = raw.b().FPDF_LoadDocument(path.to_str().unwrap(), None);
        let page = raw.b().FPDF_LoadPage(doc, 0);
        let r = rect(60.0, 700.0, 290.0, 730.0);
        let ft = raw.b().FPDFPage_CreateAnnot(page, ST_FREETEXT);
        raw.set_rect(ft, &r);
        raw.b().FPDFAnnot_SetStringValue_str(ft, "DA", "0 0 1 rg /Helv 12 Tf");
        raw.b().FPDFAnnot_SetStringValue_str(ft, "Contents", "persist me");
        raw.b().FPDFAnnot_SetFlags(ft, FLAG_PRINT);
        let _ = raw.render(page, None, 1.0);
        let in_mem = raw.get_ap(ft);
        raw.b().FPDFPage_CloseAnnot(ft);
        raw.save(doc, "spike_annot_freetext_persist.pdf")?;
        raw.b().FPDF_ClosePage(page);
        raw.b().FPDF_CloseDocument(doc);
        let doc = raw.b().FPDF_LoadDocument(out("spike_annot_freetext_persist.pdf").to_str().unwrap(), None);
        let page = raw.b().FPDF_LoadPage(doc, 0);
        let ft = raw.b().FPDFPage_GetAnnot(page, 0);
        let on_disk = raw.get_ap(ft);
        let png = raw.render(page, None, 2.0);
        let n = png.count_diff(&baseline_raw, &r, 40);
        let after_render = raw.get_ap(ft);
        rep.add("raw FreeText(DA) AP persistence", format!("in-memory after render: {:?} bytes; on disk after save: {:?}; reopened render {} px differ; AP after reopened render: {:?}", in_mem.map(|s| s.len()), on_disk.map(|s| s.len()), n, after_render.map(|s| s.len())));
        png.save_crop("raw_freetext_persist_reopened", &r, 6.0)?;
        // Workaround: copy the generated AP into a real /AP stream with FPDFAnnot_SetAP, then save again.
        if let Some(ap) = raw.get_ap(ft) {
            let ok = raw.b().is_true(raw.b().FPDFAnnot_SetAP_str(ft, AP_NORMAL, &ap));
            raw.b().FPDFPage_CloseAnnot(ft);
            raw.save(doc, "spike_annot_freetext_persist2.pdf")?;
            let d2 = raw.b().FPDF_LoadDocument(out("spike_annot_freetext_persist2.pdf").to_str().unwrap(), None);
            let p2 = raw.b().FPDF_LoadPage(d2, 0);
            let a2 = raw.b().FPDFPage_GetAnnot(p2, 0);
            rep.add("raw FreeText(DA) SetAP(copy of generated AP) persisted", format!("SetAP ok={ok}; AP on disk {:?} bytes", raw.get_ap(a2).map(|s| s.len())));
            raw.b().FPDFPage_CloseAnnot(a2);
            raw.b().FPDF_ClosePage(p2);
            raw.b().FPDF_CloseDocument(d2);
        } else {
            raw.b().FPDFPage_CloseAnnot(ft);
        }
        raw.b().FPDF_ClosePage(page);
        raw.b().FPDF_CloseDocument(doc);
    }

    // Re-open with the high-level API and verify.
    let re = pdfium.load_pdf_from_file(&out("spike_annot_raw.pdf"), None).map_err(e)?;
    let page = re.pages().get(0).map_err(e)?;
    for (i, a) in page.annotations().iter().enumerate() {
        println!("  reopened raw: {}", describe_annotation(i, &a));
    }
    for l in raw_describe_page(raw, &out("spike_annot_raw.pdf"), 0)? {
        println!("  reopened raw ffi: {l}");
    }
    let png = Png::from_page(&page, 2.0, true)?;
    png.save("spike_annot_raw_reopened.png")?;
    let checks = [
        ("circle(interior yellow, 50% CA)", rect(575.0, 515.0, 595.0, 545.0), [255u8, 255, 128], 40),
        ("ink(InkList)", rect(8.0, 100.0, 46.0, 280.0), [0, 0, 255], 60),
        ("freetext(SetAP)", rect(60.0, 700.0, 290.0, 730.0), [255, 0, 0], 80),
        ("square(raw)", rect(570.0, 205.0, 600.0, 255.0), [0, 255, 0], 60),
        ("text(sticky raw)", rect(10.0, 650.0, 30.0, 670.0), [255, 200, 0], 90),
    ];
    for (name, r, color, tol) in checks {
        let n = png.count_near(&r, color, tol);
        let diff = png.count_diff(&baseline_raw, &r, 40);
        rep.add(&format!("render[raw reopened] {name}"), format!("{n} px near {color:?}, {diff} px differ -> {}", if n > 0 || diff > 50 { "VISIBLE" } else { "NOT VISIBLE" }));
        png.save_crop(&format!("raw_{name}"), &r, 6.0)?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Section 3: modify (move/resize/color/contents), flatten.
// ---------------------------------------------------------------------------------------------
fn section3_modify_flatten(pdfium: &Pdfium, raw: Raw, rep: &mut Report, probes: &[Probe]) -> R<()> {
    println!("\n==== Section 3: modify + flatten ====");
    let mut moved_highlight_quad: Option<PdfRect> = None;
    let baseline = {
        let d = pdfium.load_pdf_from_file(&fixture("tracemonkey.pdf"), None).map_err(e)?;
        let p = d.pages().get(0).map_err(e)?;
        Png::from_page(&p, 2.0, true)?
    };

    // --- 3a. High-level modifications on the re-opened file (annotations now have APs).
    {
        let document = pdfium.load_pdf_from_file(&out("spike_annot.pdf"), None).map_err(e)?;
        let mut page = document.pages().get(0).map_err(e)?;
        page.set_content_regeneration_strategy(PdfPageContentRegenerationStrategy::Manual);
        let n = page.annotations().len();
        let mut square_idx = None;
        let mut highlight_idx = None;
        for i in 0..n {
            let a = page.annotations().get(i).map_err(e)?;
            match a.annotation_type() {
                PdfPageAnnotationType::Square if square_idx.is_none() => square_idx = Some(i),
                PdfPageAnnotationType::Highlight if highlight_idx.is_none() => highlight_idx = Some(i),
                _ => {}
            }
        }
        // Move + resize the square: x 565..605 -> 480..540, y 600..660 -> 560..640 (bigger).
        let moved = rect(480.0, 560.0, 540.0, 640.0);
        if let Some(i) = square_idx {
            let mut a = page.annotations().get(i).map_err(e)?;
            rep.add("modify square set_bounds", format!("{:?}", a.set_bounds(moved).map(|_| "ok")));
            rep.add("modify square set_contents", format!("{:?}", a.set_contents("moved square").map(|_| "ok")));
            // set_fill_color / set_stroke_color on an annotation that already has an AP is exercised
            // in the child process only: FPDFAnnot_SetColor returns false when /AP exists and
            // pdfium-render then calls FPDFPageObj_SetFillColor on the *annotation handle* (UB).
        }
        // Move the highlight's quads/rect down by 200pt (keeping AP) to see what pdfium does.
        let mut moved_quad = None;
        moved_highlight_quad = None;
        if let Some(i) = highlight_idx {
            let mut a = page.annotations().get(i).map_err(e)?;
            let old = a.bounds().map_err(e)?;
            let new = rect(old.left().value, old.bottom().value - 200.0, old.right().value, old.top().value - 200.0);
            let hl = a.as_highlight_annotation_mut().unwrap();
            let q = hl.attachment_points().get(0).map_err(e)?;
            let nq = PdfQuadPoints::new_from_values(q.x1().value, q.y1().value - 200.0, q.x2().value, q.y2().value - 200.0, q.x3().value, q.y3().value - 200.0, q.x4().value, q.y4().value - 200.0);
            hl.attachment_points_mut().set_attachment_point_at_index(0, nq).map_err(e)?;
            let q1 = hl.attachment_points().get(1).map_err(e)?;
            let nq1 = PdfQuadPoints::new_from_values(q1.x1().value, q1.y1().value - 200.0, q1.x2().value, q1.y2().value - 200.0, q1.x3().value, q1.y3().value - 200.0, q1.x4().value, q1.y4().value - 200.0);
            hl.attachment_points_mut().set_attachment_point_at_index(1, nq1).map_err(e)?;
            hl.set_bounds(new).map_err(e)?;
            moved_quad = Some((quad_bounds(&nq), quad_bounds(&q)));
            moved_highlight_quad = Some(quad_bounds(&nq));
            let tr = nq.to_rect();
            rep.add("PdfQuadPoints::to_rect() on PDFium-ordered quad", format!("to_rect={} vs min/max bounds={} (order-agnostic: OK)", fmt_rect(&tr), fmt_rect(&quad_bounds(&nq))));
        }
        let png = Png::from_page(&page, 2.0, true)?;
        png.save("spike_annot_modified.png")?;
        png.save_crop("modified_square_new", &rect(470.0, 550.0, 550.0, 650.0), 6.0)?;
        png.save_crop("modified_highlight_old_and_new", &rect(70.0, 150.0, 230.0, 720.0), 6.0)?;
        let n_new = png.count_near(&rect(486.0, 566.0, 534.0, 634.0), [200, 220, 255], 30);
        let n_old = png.count_near(&rect(572.0, 607.0, 598.0, 653.0), [200, 220, 255], 30);
        rep.add("modify square moved render", format!("{n_new} fill px at new rect, {n_old} at old rect -> {}", if n_new > 0 && n_old == 0 { "MOVED OK (AP re-mapped via Rect/BBox matrix)" } else { "PROBLEM" }));
        if let Some((nr, or)) = moved_quad {
            let a = png.count_diff(&baseline, &nr, 40);
            let b = png.count_diff(&baseline, &or, 40);
            rep.add("modify highlight quads moved -200 render (stale AP kept)", format!("new quad: {a} px differ from baseline; old quad: {b} px differ -> {}", if a > 50 && b < 50 { "MOVED OK (pdfium maps generated markup APs onto QuadPoints bounds)" } else { "PROBLEM" }));
        }
        drop(page);
        document.save_to_file(&out("spike_annot_modified.pdf")).map_err(e)?;
    }

    // --- 3b. Raw: remove AP, recolor, re-render -> AP regenerated with new colour.
    unsafe {
        let path = out("spike_annot_modified.pdf");
        let doc = raw.b().FPDF_LoadDocument(path.to_str().unwrap(), None);
        let page = raw.b().FPDF_LoadPage(doc, 0);
        let n = raw.b().FPDFPage_GetAnnotCount(page);
        for i in 0..n {
            let a = raw.b().FPDFPage_GetAnnot(page, i);
            if raw.b().FPDFAnnot_GetSubtype(a) == ST_SQUARE {
                let before = raw.set_color(a, COLORTYPE_INTERIOR, [255, 0, 255, 255]);
                let removed = raw.b().is_true(raw.b().FPDFAnnot_SetAP(a, AP_NORMAL, std::ptr::null()));
                let after = raw.set_color(a, COLORTYPE_INTERIOR, [255, 0, 255, 255]);
                rep.add("raw recolor square", format!("SetColor with AP present={before}; SetAP(NULL) removed AP={removed}; SetColor after removal={after}"));
                raw.b().FPDFPage_CloseAnnot(a);
                break;
            }
            raw.b().FPDFPage_CloseAnnot(a);
        }
        // Same recipe for the moved highlight: drop the stale AP so pdfium regenerates it from the new QuadPoints.
        for i in 0..n {
            let a = raw.b().FPDFPage_GetAnnot(page, i);
            if raw.b().FPDFAnnot_GetSubtype(a) == 9 {
                let removed = raw.b().is_true(raw.b().FPDFAnnot_SetAP(a, AP_NORMAL, std::ptr::null()));
                rep.add("raw highlight SetAP(NULL) after quad move", removed);
                raw.b().FPDFPage_CloseAnnot(a);
                break;
            }
            raw.b().FPDFPage_CloseAnnot(a);
        }
        let png = raw.render(page, None, 2.0);
        let m = png.count_near(&rect(486.0, 566.0, 534.0, 634.0), [255, 0, 255], 40);
        rep.add("raw recolor square render", format!("{m} magenta px -> {}", if m > 0 { "RECOLORED (AP regenerated on render)" } else { "NOT RECOLORED" }));
        if let Some(hq) = moved_highlight_quad {
            let y = png.count_diff(&baseline, &hq, 40);
            rep.add("raw highlight regenerated at moved quad", format!("{y} px differ at new quad after AP removal + render -> {}", if y > 50 { "REGENERATED" } else { "MISSING" }));
            png.save_crop("modified_highlight_regenerated", &hq, 6.0)?;
        }
        raw.save(doc, "spike_annot_recolored.pdf")?;
        raw.b().FPDF_ClosePage(page);
        raw.b().FPDF_CloseDocument(doc);
        let re = pdfium.load_pdf_from_file(&out("spike_annot_recolored.pdf"), None).map_err(e)?;
        let p = re.pages().get(0).map_err(e)?;
        let png = Png::from_page(&p, 2.0, true)?;
        png.save("spike_annot_recolored.png")?;
        let m = png.count_near(&rect(486.0, 566.0, 534.0, 634.0), [255, 0, 255], 40);
        rep.add("raw recolor square persisted", format!("{m} magenta px after save+reopen"));
    }

    // --- 3c. Flatten (pdfium-render uses FPDFPage_Flatten(FLAT_PRINT) + reload).
    for (file, label) in [("spike_annot.pdf", "print-flag set"), ("spike_annot_noprint.pdf", "print-flag clear")] {
        if label == "print-flag clear" {
            // Build a variant where the print flag is cleared on every annotation.
            let d = pdfium.load_pdf_from_file(&out("spike_annot.pdf"), None).map_err(e)?;
            let p = d.pages().get(0).map_err(e)?;
            for i in 0..p.annotations().len() {
                let mut a = p.annotations().get(i).map_err(e)?;
                a.set_is_printed(false).map_err(e)?;
            }
            drop(p);
            d.save_to_file(&out(file)).map_err(e)?;
        }
        let document = pdfium.load_pdf_from_file(&out(file), None).map_err(e)?;
        let mut page = document.pages().get(0).map_err(e)?;
        let (annots_before, objs_before) = (page.annotations().len(), page.objects().len());
        let t = Instant::now();
        let res = page.flatten();
        let dt = t.elapsed();
        let (annots_after, objs_after) = (page.annotations().len(), page.objects().len());
        let png = Png::from_page(&page, 2.0, true)?;
        png.save(&format!("spike_annot_flattened_{}.png", if label.contains("set") { "print" } else { "noprint" }))?;
        let mut visible = vec![];
        for p in probes {
            let n = png.count_near(&p.rect, p.color, p.tol);
            let base = baseline.count_near(&p.rect, p.color, p.tol);
            visible.push(format!("{}={}", p.name, if n > base { "vis" } else { "gone" }));
        }
        rep.add(
            &format!("flatten [{label}]"),
            format!("{res:?} in {dt:?}; annots {annots_before}->{annots_after}, page objects {objs_before}->{objs_after}; {}", visible.join(" ")),
        );
        drop(page);
        let outname = format!("spike_annot_flattened_{}.pdf", if label.contains("set") { "print" } else { "noprint" });
        document.save_to_file(&out(&outname)).map_err(e)?;
        let re = pdfium.load_pdf_from_file(&out(&outname), None).map_err(e)?;
        let p = re.pages().get(0).map_err(e)?;
        rep.add(&format!("flatten [{label}] reopened"), format!("annots {}, objects {}, text chars {}", p.annotations().len(), p.objects().len(), p.text().map(|t| t.len()).unwrap_or(-1)));
    }
    // Raw FPDFPage_Flatten(FLAT_NORMALDISPLAY) keeps annotations that lack the Print flag.
    unsafe {
        let path = out("spike_annot_noprint.pdf");
        let doc = raw.b().FPDF_LoadDocument(path.to_str().unwrap(), None);
        let page = raw.b().FPDF_LoadPage(doc, 0);
        let before = raw.b().FPDFPage_GetAnnotCount(page);
        let rc = raw.b().FPDFPage_Flatten(page, 0 /* FLAT_NORMALDISPLAY */);
        let ok = raw.b().is_true(raw.b().FPDFPage_GenerateContent(page));
        raw.b().FPDF_ClosePage(page);
        // Flatten edits the dictionaries; the page must be reloaded before rendering (pdfium bug 2055).
        let page = raw.b().FPDF_LoadPage(doc, 0);
        let after = raw.b().FPDFPage_GetAnnotCount(page);
        let png = raw.render(page, None, 2.0);
        let mut visible = vec![];
        for p in probes {
            let n = png.count_near(&p.rect, p.color, p.tol);
            let base = baseline.count_near(&p.rect, p.color, p.tol);
            visible.push(format!("{}={}", p.name, if n > base { "vis" } else { "gone" }));
        }
        rep.add("raw flatten FLAT_NORMALDISPLAY [print-flag clear]", format!("rc={rc} (1=success,2=nothing) GenerateContent={ok}; annots {before}->{after}; {}", visible.join(" ")));
        raw.save(doc, "spike_annot_flattened_normaldisplay.pdf")?;
        raw.b().FPDF_ClosePage(page);
        raw.b().FPDF_CloseDocument(doc);
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Section 4: AcroForm.
// ---------------------------------------------------------------------------------------------
fn field_summary(field: &PdfFormField) -> String {
    match field.field_type() {
        PdfFormFieldType::Text => format!("Text value={:?} multiline={}", field.as_text_field().unwrap().value(), field.as_text_field().unwrap().is_multiline()),
        PdfFormFieldType::Checkbox => {
            let c = field.as_checkbox_field().unwrap();
            format!("Checkbox checked={:?} export={:?} idx={}", c.is_checked(), c.group_value(), c.index_in_group())
        }
        PdfFormFieldType::RadioButton => {
            let r = field.as_radio_button_field().unwrap();
            format!("Radio checked={:?} group_value={:?} idx={}", r.is_checked(), r.group_value(), r.index_in_group())
        }
        PdfFormFieldType::ComboBox => {
            let c = field.as_combo_box_field().unwrap();
            let opts: Vec<String> = c.options().iter().map(|o| format!("{}{}", o.label().cloned().unwrap_or_default(), if o.is_set() { "*" } else { "" })).collect();
            format!("Combo value={:?} options={:?}", c.value(), opts)
        }
        PdfFormFieldType::ListBox => {
            let l = field.as_list_box_field().unwrap();
            let opts: Vec<String> = l.options().iter().map(|o| format!("{}{}", o.label().cloned().unwrap_or_default(), if o.is_set() { "*" } else { "" })).collect();
            format!("List value={:?} options={:?}", l.value(), opts)
        }
        PdfFormFieldType::Signature => "Signature".to_string(),
        PdfFormFieldType::PushButton => "PushButton".to_string(),
        PdfFormFieldType::Unknown => "Unknown".to_string(),
    }
}

fn section4_forms(pdfium: &Pdfium, raw: Raw, rep: &mut Report) -> R<()> {
    println!("\n==== Section 4: AcroForm (160F-2019.pdf) ====");
    let path = fixture("160F-2019.pdf");
    let document = pdfium.load_pdf_from_file(&path, None).map_err(e)?;
    let form = document.form();
    rep.add("form present", format!("{} type={:?}", form.is_some(), form.map(|f| f.form_type())));
    let mut counts = std::collections::BTreeMap::new();
    let mut text_target: Option<(PdfPageIndex, usize, PdfRect, Option<String>)> = None;
    let mut check_target: Option<(PdfPageIndex, usize, PdfRect, bool, PdfFormFieldType)> = None;
    for (pi, page) in document.pages().iter().enumerate() {
        for (i, a) in page.annotations().iter().enumerate() {
            if let Some(field) = a.as_form_field() {
                *counts.entry(format!("{:?}", field.field_type())).or_insert(0) += 1;
                let r = a.bounds().map_err(e)?;
                if i < 12 {
                    println!("  p{pi} #{i} {:?} {} {} readonly={}", field.name(), field_summary(field), fmt_rect(&r), field.is_read_only());
                }
                if text_target.is_none() && field.field_type() == PdfFormFieldType::Text && r.width().value > 80.0 && r.height().value > 10.0 {
                    text_target = Some((pi as PdfPageIndex, i, r, field.name()));
                }
                if field.field_type() == PdfFormFieldType::Checkbox && !matches!(check_target, Some((_, _, _, _, PdfFormFieldType::Checkbox))) {
                    check_target = Some((pi as PdfPageIndex, i, r, field.as_checkbox_field().unwrap().is_checked().unwrap_or(false), PdfFormFieldType::Checkbox));
                } else if check_target.is_none() && field.field_type() == PdfFormFieldType::RadioButton {
                    check_target = Some((pi as PdfPageIndex, i, r, field.as_radio_button_field().unwrap().is_checked().unwrap_or(false), PdfFormFieldType::RadioButton));
                }
            }
        }
    }
    rep.add("form field counts", format!("{counts:?}"));
    if let Some(f) = form {
        let values = f.field_values(document.pages());
        println!("  field_values(): {} entries; sample {:?}", values.len(), values.iter().take(4).collect::<Vec<_>>());
    }
    let (tp, ti, trect, tname) = text_target.ok_or("no text field found")?;
    let (cp, ci, crect, cwas, ckind) = check_target.ok_or("no checkbox/radio found")?;
    println!("  text target p{tp}#{ti} {tname:?} {}; toggle target ({ckind:?}) p{cp}#{ci} {} was {cwas}", fmt_rect(&trect), fmt_rect(&crect));

    // --- 4a. High-level set_value / set_checked (writes /V and /AS on the widget dict only).
    let base_png = Png::from_page(&document.pages().get(tp).map_err(e)?, 2.0, true)?;
    base_png.save("spike_annot_form_baseline.png")?;
    {
        let page = document.pages().get(tp).map_err(e)?;
        let page_c = document.pages().get(cp).map_err(e)?;
        {
            let mut a = page.annotations().get(ti).map_err(e)?;
            let f = a.as_form_field_mut().unwrap().as_text_field_mut().unwrap();
            rep.add("form high-level set_value", format!("{:?}", f.set_value("SeePDF 스파이크 12345").map(|_| "ok")));
            rep.add("form high-level value readback (same session)", format!("{:?}", f.value()));
            let mut c = page_c.annotations().get(ci).map_err(e)?;
            let ff = c.as_form_field_mut().unwrap();
            if ckind == PdfFormFieldType::Checkbox {
                let cb = ff.as_checkbox_field_mut().unwrap();
                rep.add("form high-level checkbox set_checked", format!("{:?} -> is_checked {:?}", cb.set_checked(!cwas).map(|_| "ok"), cb.is_checked()));
            } else {
                let rb = ff.as_radio_button_field_mut().unwrap();
                rep.add("form high-level radio set_checked", format!("{:?} -> is_checked {:?} group_value {:?}", rb.set_checked().map(|_| "ok"), rb.is_checked(), rb.group_value()));
            }
        }
        let png = Png::from_page(&page, 2.0, true)?;
        png.save("spike_annot_form_highlevel.png")?;
        png.save_crop("form_highlevel_text", &trect, 6.0)?;
        png.save_crop("form_highlevel_checkbox", &crect, 6.0)?;
        let d = png.count_diff(&base_png, &trect, 40);
        rep.add("form high-level render (same session)", format!("text rect: {d} px differ from baseline -> {}", if d > 50 { "VALUE VISIBLE" } else { "VALUE NOT VISIBLE (AP not regenerated)" }));
        if cp == tp {
            let d = png.count_diff(&base_png, &crect, 40);
            rep.add("form high-level toggle render (same session)", format!("{d} px differ from baseline"));
        }
        drop(page);
        drop(page_c);
    }
    document.save_to_file(&out("spike_annot_form_highlevel.pdf")).map_err(e)?;
    drop(document);
    {
        let re = pdfium.load_pdf_from_file(&out("spike_annot_form_highlevel.pdf"), None).map_err(e)?;
        let page = re.pages().get(tp).map_err(e)?;
        let a = page.annotations().get(ti).map_err(e)?;
        let v = a.as_form_field().and_then(|f| f.as_text_field().map(|t| t.value()));
        let page_c = re.pages().get(cp).map_err(e)?;
        let c = page_c.annotations().get(ci).map_err(e)?;
        let cv = c.as_form_field().map(|f| match f.field_type() {
            PdfFormFieldType::Checkbox => f.as_checkbox_field().unwrap().is_checked(),
            _ => f.as_radio_button_field().unwrap().is_checked(),
        });
        let png = Png::from_page(&page, 2.0, true)?;
        png.save("spike_annot_form_highlevel_reopened.png")?;
        png.save_crop("form_highlevel_reopened_text", &trect, 6.0)?;
        png.save_crop("form_highlevel_reopened_toggle", &crect, 6.0)?;
        let d = png.count_diff(&base_png, &trect, 40);
        let dc = png.count_diff(&base_png, &crect, 40);
        rep.add("form high-level reopened", format!("text value={v:?} toggle={cv:?}; text rect {d} px differ; toggle rect {dc} px differ"));
    }

    // --- 4b. Raw FORM_* interaction (focus + type + kill focus) -> pdfium regenerates the AP.
    unsafe {
        let doc = raw.b().FPDF_LoadDocument(path.to_str().unwrap(), None);
        let mut ffi = form_fill_info();
        let form = raw.b().FPDFDOC_InitFormFillEnvironment(doc, &mut *ffi as *mut FPDF_FORMFILLINFO);
        rep.add("raw FPDFDOC_InitFormFillEnvironment", format!("null={}", form.is_null()));
        raw.b().FPDF_SetFormFieldHighlightAlpha(form, 0);
        let page = raw.b().FPDF_LoadPage(doc, tp as c_int);
        raw.b().FORM_OnAfterLoadPage(page, form);
        let cx = ((trect.left().value + trect.right().value) / 2.0) as f64;
        let cy = ((trect.bottom().value + trect.top().value) / 2.0) as f64;
        let ok1 = raw.b().FORM_OnLButtonDown(form, page, 0, cx, cy);
        let ok2 = raw.b().FORM_OnLButtonUp(form, page, 0, cx, cy);
        for ch in "Raw FORM 입력".chars() {
            raw.b().FORM_OnChar(form, page, ch as c_int, 0);
        }
        let killed = raw.b().FORM_ForceToKillFocus(form);
        rep.add("raw FORM click+type", format!("LButtonDown={ok1} Up={ok2} killfocus={killed}"));
        let annot = raw.b().FPDFPage_GetAnnot(page, ti as c_int);
        let ft = raw.b().FPDFAnnot_GetFormFieldType(form, annot);
        let len = raw.b().FPDFAnnot_GetFormFieldValue(form, annot, std::ptr::null_mut(), 0);
        let mut buf = vec![0u16; (len as usize) / 2];
        raw.b().FPDFAnnot_GetFormFieldValue(form, annot, buf.as_mut_ptr(), len);
        let value = String::from_utf16_lossy(&buf).trim_end_matches('\0').to_string();
        rep.add("raw FORM value readback", format!("type={ft} (TEXTFIELD={FPDF_FORMFIELD_TEXTFIELD}) value={value:?} AP_len={:?}", raw.get_ap(annot).map(|s| s.len())));
        raw.b().FPDFPage_CloseAnnot(annot);
        // Toggle the checkbox by clicking on it (same page only).
        if cp == tp {
            let ccx = ((crect.left().value + crect.right().value) / 2.0) as f64;
            let ccy = ((crect.bottom().value + crect.top().value) / 2.0) as f64;
            raw.b().FORM_OnLButtonDown(form, page, 0, ccx, ccy);
            raw.b().FORM_OnLButtonUp(form, page, 0, ccx, ccy);
            raw.b().FORM_ForceToKillFocus(form);
            let ca = raw.b().FPDFPage_GetAnnot(page, ci as c_int);
            let checked = raw.b().is_true(raw.b().FPDFAnnot_IsChecked(form, ca));
            let cft = raw.b().FPDFAnnot_GetFormFieldType(form, ca);
            rep.add("raw FORM checkbox click", format!("type={cft} (CHECKBOX={FPDF_FORMFIELD_CHECKBOX}) checked now {checked} (was {cwas})"));
            raw.b().FPDFPage_CloseAnnot(ca);
        }
        let png = raw.render(page, Some(form), 2.0);
        png.save("spike_annot_form_raw.png")?;
        png.save_crop("form_raw_text", &trect, 6.0)?;
        png.save_crop("form_raw_checkbox", &crect, 6.0)?;
        let d = png.count_diff(&base_png, &trect, 40);
        rep.add("raw FORM render (FFLDraw, same session)", format!("text rect {d} px differ from baseline -> {}", if d > 50 { "VALUE VISIBLE" } else { "NOT VISIBLE" }));
        let bytes = raw.save(doc, "spike_annot_form_raw.pdf")?;
        rep.add("raw FORM save", format!("{bytes} bytes"));
        raw.b().FORM_OnBeforeClosePage(page, form);
        raw.b().FPDF_ClosePage(page);
        raw.b().FPDFDOC_ExitFormFillEnvironment(form);
        raw.b().FPDF_CloseDocument(doc);
    }
    {
        let re = pdfium.load_pdf_from_file(&out("spike_annot_form_raw.pdf"), None).map_err(e)?;
        let page = re.pages().get(tp).map_err(e)?;
        let a = page.annotations().get(ti).map_err(e)?;
        let v = a.as_form_field().and_then(|f| f.as_text_field().map(|t| t.value()));
        let c = re.pages().get(cp).map_err(e)?.annotations().get(ci).map_err(e)?.as_form_field().map(|f| match f.field_type() {
            PdfFormFieldType::Checkbox => f.as_checkbox_field().unwrap().is_checked(),
            _ => f.as_radio_button_field().unwrap().is_checked(),
        });
        let png = Png::from_page(&page, 2.0, true)?;
        png.save("spike_annot_form_raw_reopened.png")?;
        png.save_crop("form_raw_reopened_text", &trect, 6.0)?;
        let d = png.count_diff(&base_png, &trect, 40);
        let png_noform = Png::from_page(&page, 2.0, false)?;
        png_noform.save_crop("form_raw_reopened_text_noformdata", &trect, 6.0)?;
        let d2 = png_noform.count_diff(&base_png, &trect, 40);
        let dt = png.count_diff(&base_png, &crect, 40);
        rep.add("raw FORM reopened", format!("text value={v:?} toggle={c:?}; text rect px differ: with FFLDraw {d} / without {d2}; toggle rect {dt}"));
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Section 5: redaction (approach A: remove text objects + black box).
// ---------------------------------------------------------------------------------------------
fn section5_redaction(pdfium: &Pdfium, rep: &mut Report) -> R<()> {
    println!("\n==== Section 5: redaction on tracemonkey.pdf page 0 ====");
    let document = pdfium.load_pdf_from_file(&fixture("tracemonkey.pdf"), None).map_err(e)?;
    let mut page = document.pages().get(0).map_err(e)?;
    let target_word = "Gal";
    let target = find_word(&page, target_word, 0).ok_or("target word not found")?;
    let target = rect(target.left().value - 1.0, target.bottom().value - 1.0, target.right().value + 1.0, target.top().value + 1.0);
    let text_before = page.text().map_err(e)?.all();
    let count_before = text_before.matches(target_word).count();
    println!("  target {:?} at {} ; occurrences before: {count_before}", target_word, fmt_rect(&target));

    // Collect overlapping text objects (whole objects: pdfium cannot split a text object).
    let mut victims: Vec<(usize, String, PdfRect)> = vec![];
    let n_objs = page.objects().len();
    for i in 0..n_objs {
        let o = page.objects().get(i).map_err(e)?;
        if let Some(t) = o.as_text_object() {
            if let Ok(b) = o.bounds() {
                let br = b.to_rect();
                if br.does_overlap(&target) {
                    victims.push((i, t.text(), br));
                }
            }
        }
    }
    for (i, t, r) in &victims {
        println!("  removing text object #{i} {} text={:?}", fmt_rect(r), t);
    }
    rep.add("redaction victims", format!("{} text objects overlap the word rect (collateral text shown above)", victims.len()));
    let t0 = Instant::now();
    for (i, _, _) in victims.iter().rev() {
        page.objects_mut().remove_object_at_index(*i).map_err(e)?;
    }
    page.objects_mut().create_path_object_rect(target, None, None, Some(PdfColor::BLACK)).map_err(e)?;
    page.regenerate_content().map_err(e)?;
    rep.add("bench redaction remove+box+regenerate", format!("{:?}", t0.elapsed()));
    let text_mem = page.text().map_err(e)?.all();
    rep.add("redaction text after (in memory)", format!("occurrences {} (chars {} -> {})", text_mem.matches(target_word).count(), text_before.len(), text_mem.len()));
    let png = Png::from_page(&page, 2.0, true)?;
    png.save("spike_annot_redacted.png")?;
    png.save_crop("redacted", &rect(target.left().value - 80.0, target.bottom().value - 20.0, target.right().value + 80.0, target.top().value + 20.0), 0.0)?;
    rep.add("redaction black box render", format!("{} black px in target rect", png.count_near(&target, [0, 0, 0], 20)));
    drop(page);
    document.save_to_file(&out("spike_annot_redacted.pdf")).map_err(e)?;
    let re = pdfium.load_pdf_from_file(&out("spike_annot_redacted.pdf"), None).map_err(e)?;
    let p = re.pages().get(0).map_err(e)?;
    let text_disk = p.text().map_err(e)?.all();
    rep.add("redaction text after (reopened)", format!("occurrences of {target_word:?}: {} ; objects {}", text_disk.matches(target_word).count(), p.objects().len()));
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Dangerous sub-test run in a child process: set_stroke_color on an annotation that has an AP.
// pdfium-render falls back to FPDFPageObj_SetFillColor(annot as FPDF_PAGEOBJECT), which is UB.
// ---------------------------------------------------------------------------------------------
fn color_on_ap_child() -> R<()> {
    let lib = pdfium_lib();
    let pdfium = Pdfium::new(Pdfium::bind_to_library(&lib).map_err(e)?);
    let document = pdfium.load_pdf_from_file(&out("spike_annot.pdf"), None).map_err(e)?;
    let page = document.pages().get(0).map_err(e)?;
    let mut done = 0;
    for i in 0..page.annotations().len() {
        let mut a = page.annotations().get(i).map_err(e)?;
        match a.annotation_type() {
            PdfPageAnnotationType::Square => {
                let r = a.set_fill_color(PdfColor::new(255, 180, 180, 255));
                println!("child: set_fill_color on square-with-AP -> {r:?}; readback {}", fmt_color(&a.fill_color()));
                done += 1;
            }
            PdfPageAnnotationType::Highlight => {
                let r = a.set_stroke_color(PdfColor::GREEN_YELLOW);
                println!("child: set_stroke_color on highlight-with-AP -> {r:?}; readback {}", fmt_color(&a.stroke_color()));
                done += 1;
            }
            _ => {}
        }
    }
    if done == 2 { Ok(()) } else { Err("annotations missing".into()) }
}

fn main() {
    if std::env::args().nth(1).as_deref() == Some("color-on-ap") {
        std::process::exit(match color_on_ap_child() {
            Ok(()) => 0,
            Err(err) => {
                eprintln!("child error: {err}");
                2
            }
        });
    }
    if let Err(err) = run() {
        eprintln!("SPIKE FAILED: {err}");
        std::process::exit(1);
    }
}

fn run() -> R<()> {
    std::fs::create_dir_all(out("")).map_err(e)?;
    let lib = pdfium_lib();
    println!("pdfium library: {}", lib.display());
    // Raw bindings first (must happen before Pdfium::new, which seals the global bindings cell).
    let raw = Raw(Box::leak(Pdfium::bind_to_library(&lib).map_err(e)?));
    let pdfium = Pdfium::new(Pdfium::bind_to_library(&lib).map_err(e)?);
    let mut rep = Report { lines: vec![] };
    let t = Instant::now();

    section1_enumerate(&pdfium, raw, &mut rep)?;
    let probes = section2_create(&pdfium, raw, &mut rep)?;
    section2b_raw_create(&pdfium, raw, &mut rep)?;
    section3_modify_flatten(&pdfium, raw, &mut rep, &probes)?;
    section4_forms(&pdfium, raw, &mut rep)?;
    section5_redaction(&pdfium, &mut rep)?;

    // Child-process UB probe.
    let exe = std::env::current_exe().map_err(e)?;
    let outp = std::process::Command::new(exe).arg("color-on-ap").output().map_err(e)?;
    let status = outp.status;
    let stdout = String::from_utf8_lossy(&outp.stdout).trim().to_string();
    rep.add("set_*_color on annotation with AP (child process)", format!("exit={status:?}; {stdout}"));

    println!("\n==== SUMMARY ({:?} total) ====", t.elapsed());
    for l in &rep.lines {
        println!("{l}");
    }
    Ok(())
}
