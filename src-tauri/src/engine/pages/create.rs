//! 이미지로 PDF 만들기 (v0.3 D1): a new untitled document with one page per PNG / JPEG.
//!
//! Two halves, so the slow part never sits on the engine thread:
//!
//! 1. [`prepare_image`] — **no PDFium**, runs on a blocking worker: reads the file, sniffs the
//!    format from its magic bytes (never the extension), reads the pixel size, the resolution
//!    (PNG `pHYs`, JPEG JFIF `APP0` density, EXIF `XResolution` / `ResolutionUnit`) and the EXIF
//!    orientation. A JPEG that needs no rotation is kept **as its own bytes** and embedded
//!    inline (`FPDFImageObj_LoadJpegFileInline`, a `/DCTDecode` stream): no re-encode, no
//!    quality loss, no size blow-up. A PNG, or a JPEG with an EXIF rotation / mirror, is decoded
//!    with `image` and the orientation applied, then embedded as a bitmap.
//! 2. [`build`] — on the engine thread: one `create_new_pdf`, one page per image, sized by
//!    [`layout`], saved and opened as an **untitled, dirty** document (like a merge), named after
//!    the first image.
//!
//! Resolution: an image that states none is taken at [`DEFAULT_DPI`] (96, what Windows and most
//! image-to-PDF tools assume). A page is at most 14 400 pt (200 in) on a side — PDF's own
//! limit — so a huge scan at a low stated DPI is scaled down to fit rather than refused.

use crate::engine::raw;
use crate::engine::registry;
use crate::engine::types::EngineState;
use crate::ipc::error::PdfiumResultExt;
use crate::ipc::types::{DocInfo, ImageFit, ImagePageSize};
use crate::ipc::{EngineError, ErrorCode};
use pdfium_render::prelude::{
    PdfPageContentRegenerationStrategy, PdfPageImageObject, PdfPageObjectsCommon, PdfPagePaperSize,
    PdfPoints,
};
use std::path::Path;

/// The resolution assumed for an image that states none.
pub const DEFAULT_DPI: f32 = 96.0;
/// PDF's largest page side, in points (ISO 32000 Annex C: 14 400 units).
pub const MAX_SIDE_PT: f32 = 14_400.0;
/// The most images one call accepts.
pub const MAX_IMAGES: usize = 2_000;
/// The largest margin accepted, in points (≈ 70 mm).
pub const MAX_MARGIN_PT: f32 = 200.0;

/// Page size, margin and fit for [`build`].
#[derive(Debug, Clone, Copy)]
pub struct ImagesToPdfOptions {
    pub page_size: ImagePageSize,
    /// Points on every side.
    pub margin_pt: f32,
    pub fit: ImageFit,
}

impl ImagesToPdfOptions {
    /// Validated options (`invalidArgument` for a negative, non-finite or huge margin).
    pub fn new(
        page_size: ImagePageSize,
        margin_pt: f32,
        fit: ImageFit,
    ) -> Result<Self, EngineError> {
        if !margin_pt.is_finite() || !(0.0..=MAX_MARGIN_PT).contains(&margin_pt) {
            return Err(EngineError::invalid(format!(
                "margin {margin_pt} pt is outside 0…{MAX_MARGIN_PT}"
            )));
        }
        Ok(Self {
            page_size,
            margin_pt,
            fit,
        })
    }
}

/// What gets embedded.
pub enum ImageData {
    /// The file's own JPEG bytes (no EXIF rotation to apply).
    Jpeg(Vec<u8>),
    /// A decoded image, orientation applied.
    Bitmap(image::DynamicImage),
}

/// One image, read and measured, ready for [`build`].
pub struct PreparedImage {
    /// File stem, for the document name.
    pub stem: String,
    /// Pixel size **as displayed** (after the EXIF orientation).
    pub width_px: u32,
    pub height_px: u32,
    /// Horizontal / vertical resolution, dots per inch, as displayed.
    pub dpi_x: f32,
    pub dpi_y: f32,
    pub data: ImageData,
}

impl PreparedImage {
    /// The image's own size in points at its resolution.
    pub fn natural_size_pt(&self) -> (f32, f32) {
        (
            self.width_px as f32 / self.dpi_x * 72.0,
            self.height_px as f32 / self.dpi_y * 72.0,
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Format {
    Png,
    Jpeg,
}

fn sniff(bytes: &[u8]) -> Option<Format> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some(Format::Png)
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some(Format::Jpeg)
    } else {
        None
    }
}

/// Is `path` an image [`prepare_image`] accepts? Magic bytes, not the extension.
pub fn is_supported_image(path: &Path) -> bool {
    use std::io::Read;
    let mut head = [0u8; 8];
    std::fs::File::open(path)
        .and_then(|mut f| f.read_exact(&mut head))
        .map(|()| sniff(&head).is_some())
        .unwrap_or(false)
}

/// Reads and measures one image. **No PDFium** — safe on any thread.
pub fn prepare_image(path: &Path) -> Result<PreparedImage, EngineError> {
    if !path.is_file() {
        return Err(EngineError::not_found(format!("{}", path.display())));
    }
    let bytes = std::fs::read(path).map_err(EngineError::from)?;
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "image".to_string());
    let bad = |e: &dyn std::fmt::Display| {
        EngineError::new(
            ErrorCode::InvalidArgument,
            format!("{}: {e}", path.display()),
        )
    };
    let format = sniff(&bytes).ok_or_else(|| {
        EngineError::new(
            ErrorCode::Unsupported,
            format!("{} is not a PNG or JPEG image", path.display()),
        )
    })?;
    let meta = match format {
        Format::Png => png_meta(&bytes),
        Format::Jpeg => jpeg_meta(&bytes),
    };
    let (dpi_x, dpi_y) = meta.dpi.unwrap_or((DEFAULT_DPI, DEFAULT_DPI));
    let orientation = meta.orientation.filter(|o| (2..=8).contains(o));

    if format == Format::Jpeg && orientation.is_none() {
        let (w, h) =
            image::ImageReader::with_format(std::io::Cursor::new(&bytes), image::ImageFormat::Jpeg)
                .into_dimensions()
                .map_err(|e| bad(&e))?;
        check_pixels(w, h, path)?;
        return Ok(PreparedImage {
            stem,
            width_px: w,
            height_px: h,
            dpi_x,
            dpi_y,
            data: ImageData::Jpeg(bytes),
        });
    }

    let fmt = match format {
        Format::Png => image::ImageFormat::Png,
        Format::Jpeg => image::ImageFormat::Jpeg,
    };
    let mut decoded = image::load_from_memory_with_format(&bytes, fmt).map_err(|e| bad(&e))?;
    let (mut dpi_x, mut dpi_y) = (dpi_x, dpi_y);
    if let Some(o) = orientation.and_then(image::metadata::Orientation::from_exif) {
        decoded.apply_orientation(o);
        // 5–8 swap the axes, and with them the two resolutions.
        if orientation.is_some_and(|o| o >= 5) {
            std::mem::swap(&mut dpi_x, &mut dpi_y);
        }
    }
    check_pixels(decoded.width(), decoded.height(), path)?;
    Ok(PreparedImage {
        stem,
        width_px: decoded.width(),
        height_px: decoded.height(),
        dpi_x,
        dpi_y,
        data: ImageData::Bitmap(decoded),
    })
}

fn check_pixels(w: u32, h: u32, path: &Path) -> Result<(), EngineError> {
    if w == 0 || h == 0 {
        return Err(EngineError::invalid(format!(
            "{} has no pixels",
            path.display()
        )));
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------
// Resolution and orientation
// ---------------------------------------------------------------------------------------

#[derive(Debug, Default, PartialEq)]
struct Meta {
    dpi: Option<(f32, f32)>,
    orientation: Option<u8>,
}

fn sane_dpi(x: f32, y: f32) -> Option<(f32, f32)> {
    let ok = |v: f32| v.is_finite() && (1.0..=10_000.0).contains(&v);
    (ok(x) && ok(y)).then_some((x, y))
}

/// PNG `pHYs`: pixels per metre (unit 1). Unit 0 is an aspect ratio only.
fn png_meta(bytes: &[u8]) -> Meta {
    let mut at = 8usize;
    while at + 8 <= bytes.len() {
        let len = u32::from_be_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]);
        let kind = &bytes[at + 4..at + 8];
        let data_start = at + 8;
        let Some(data_end) = data_start.checked_add(len as usize) else {
            break;
        };
        if data_end > bytes.len() {
            break;
        }
        if kind == b"pHYs" && len >= 9 {
            let d = &bytes[data_start..data_end];
            let x = u32::from_be_bytes([d[0], d[1], d[2], d[3]]) as f32;
            let y = u32::from_be_bytes([d[4], d[5], d[6], d[7]]) as f32;
            if d[8] == 1 {
                return Meta {
                    dpi: sane_dpi(x * 0.0254, y * 0.0254),
                    orientation: None,
                };
            }
            return Meta::default();
        }
        if kind == b"IDAT" || kind == b"IEND" {
            break;
        }
        at = data_end + 4; // CRC
    }
    Meta::default()
}

/// JPEG: JFIF `APP0` density (units 1 = dpi, 2 = dots per cm) wins; else EXIF `APP1`
/// `XResolution` / `YResolution` / `ResolutionUnit`. The EXIF orientation comes from `APP1`.
fn jpeg_meta(bytes: &[u8]) -> Meta {
    let mut jfif: Option<(f32, f32)> = None;
    let mut exif = Meta::default();
    let mut at = 2usize;
    while at + 4 <= bytes.len() {
        if bytes[at] != 0xFF {
            break;
        }
        let marker = bytes[at + 1];
        if marker == 0xD8 || marker == 0x01 || (0xD0..=0xD7).contains(&marker) {
            at += 2;
            continue;
        }
        // SOS or any frame header: no more metadata segments of interest.
        if marker == 0xDA
            || (0xC0..=0xCF).contains(&marker) && marker != 0xC4 && marker != 0xC8 && marker != 0xCC
        {
            break;
        }
        let len = u16::from_be_bytes([bytes[at + 2], bytes[at + 3]]) as usize;
        if len < 2 || at + 2 + len > bytes.len() {
            break;
        }
        let data = &bytes[at + 4..at + 2 + len];
        if marker == 0xE0 && data.len() >= 12 && data.starts_with(b"JFIF\0") {
            let units = data[7];
            let x = u16::from_be_bytes([data[8], data[9]]) as f32;
            let y = u16::from_be_bytes([data[10], data[11]]) as f32;
            jfif = match units {
                1 => sane_dpi(x, y),
                2 => sane_dpi(x * 2.54, y * 2.54),
                _ => None,
            };
        } else if marker == 0xE1 && data.starts_with(b"Exif\0\0") {
            exif = tiff_meta(&data[6..]);
        }
        at += 2 + len;
    }
    Meta {
        dpi: jfif.or(exif.dpi),
        orientation: exif.orientation,
    }
}

/// IFD0 of an EXIF TIFF block: orientation (0x0112), resolution (0x011A / 0x011B, RATIONAL)
/// and its unit (0x0128: 2 inch, 3 cm).
fn tiff_meta(t: &[u8]) -> Meta {
    if t.len() < 8 {
        return Meta::default();
    }
    let le = match &t[0..2] {
        b"II" => true,
        b"MM" => false,
        _ => return Meta::default(),
    };
    let u16_at = |o: usize| -> Option<u16> {
        let b = t.get(o..o + 2)?;
        Some(if le {
            u16::from_le_bytes([b[0], b[1]])
        } else {
            u16::from_be_bytes([b[0], b[1]])
        })
    };
    let u32_at = |o: usize| -> Option<u32> {
        let b = t.get(o..o + 4)?;
        Some(if le {
            u32::from_le_bytes([b[0], b[1], b[2], b[3]])
        } else {
            u32::from_be_bytes([b[0], b[1], b[2], b[3]])
        })
    };
    let rational = |o: usize| -> Option<f32> {
        let off = u32_at(o)? as usize;
        let n = u32_at(off)? as f32;
        let d = u32_at(off + 4)? as f32;
        (d > 0.0).then_some(n / d)
    };
    let Some(ifd) = u32_at(4).map(|o| o as usize) else {
        return Meta::default();
    };
    let Some(count) = u16_at(ifd) else {
        return Meta::default();
    };
    let (mut orientation, mut xres, mut yres, mut unit) = (None, None, None, 2u16);
    for i in 0..count as usize {
        let e = ifd + 2 + i * 12;
        let Some(tag) = u16_at(e) else { break };
        match tag {
            0x0112 => orientation = u16_at(e + 8).map(|v| v as u8),
            0x011A => xres = rational(e + 8),
            0x011B => yres = rational(e + 8),
            0x0128 => unit = u16_at(e + 8).unwrap_or(2),
            _ => {}
        }
    }
    let scale = match unit {
        2 => Some(1.0),
        3 => Some(2.54),
        _ => None,
    };
    let dpi = match (xres, yres, scale) {
        (Some(x), y, Some(s)) => sane_dpi(x * s, y.unwrap_or(x) * s),
        _ => None,
    };
    Meta { dpi, orientation }
}

// ---------------------------------------------------------------------------------------
// Layout
// ---------------------------------------------------------------------------------------

/// Where one image goes: the page size and the image's box on it, in points (y up).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placement {
    pub page_w: f32,
    pub page_h: f32,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

/// The page and the image box for an image of natural size `(w, h)` points. Pure.
///
/// * `original` — the page is the image plus the margin on every side (scaled down to the
///   14 400 pt limit when needed); `fit` does not apply.
/// * `a4` / `letter` — portrait, or landscape for a landscape image; the image is centred in
///   the page less its margins, `contain` scaled (up or down) to fill that box, `actual` at
///   its own size unless that does not fit, then scaled down.
pub fn layout(natural: (f32, f32), options: &ImagesToPdfOptions) -> Placement {
    let (w, h) = (natural.0.max(0.01), natural.1.max(0.01));
    let m = options.margin_pt;
    let named = match options.page_size {
        ImagePageSize::Original => None,
        ImagePageSize::A4 => Some((595.28f32, 841.89f32)),
        ImagePageSize::Letter => Some((612.0f32, 792.0f32)),
    };
    match named {
        None => {
            let scale = ((MAX_SIDE_PT - 2.0 * m) / w)
                .min((MAX_SIDE_PT - 2.0 * m) / h)
                .min(1.0);
            let (iw, ih) = ((w * scale).max(1.0), (h * scale).max(1.0));
            Placement {
                page_w: iw + 2.0 * m,
                page_h: ih + 2.0 * m,
                x: m,
                y: m,
                w: iw,
                h: ih,
            }
        }
        Some((pw, ph)) => {
            let (pw, ph) = if w > h { (ph, pw) } else { (pw, ph) };
            let bw = (pw - 2.0 * m).max(1.0);
            let bh = (ph - 2.0 * m).max(1.0);
            let fit = (bw / w).min(bh / h);
            let scale = match options.fit {
                ImageFit::Contain => fit,
                ImageFit::Actual => fit.min(1.0),
            };
            let (iw, ih) = (w * scale, h * scale);
            Placement {
                page_w: pw,
                page_h: ph,
                x: (pw - iw) / 2.0,
                y: (ph - ih) / 2.0,
                w: iw,
                h: ih,
            }
        }
    }
}

// ---------------------------------------------------------------------------------------
// Build (engine thread)
// ---------------------------------------------------------------------------------------

/// `create_from_images`, second half: the document, opened untitled and **dirty** (it has never
/// been saved, so closing asks and autosave keeps it), named `<first image>.pdf`.
pub fn build(
    st: &mut EngineState<'_>,
    images: &[PreparedImage],
    options: &ImagesToPdfOptions,
) -> Result<DocInfo, EngineError> {
    if images.is_empty() {
        return Err(EngineError::invalid("no image was given"));
    }
    if images.len() > MAX_IMAGES {
        return Err(EngineError::invalid(format!(
            "at most {MAX_IMAGES} images per document"
        )));
    }
    let pdfium = st.pdfium;
    let bytes = {
        let mut doc = pdfium
            .create_new_pdf()
            .map_err(|e| EngineError::pdfium("create document", e))?;
        for (i, img) in images.iter().enumerate() {
            let at = layout(img.natural_size_pt(), options);
            let mut object = match &img.data {
                ImageData::Jpeg(bytes) => PdfPageImageObject::new_from_jpeg_reader(
                    &doc,
                    std::io::Cursor::new(bytes.as_slice()),
                )
                .ctx(&format!("embed JPEG {}", i + 1))?,
                ImageData::Bitmap(image) => {
                    PdfPageImageObject::new(&doc, image).ctx(&format!("embed image {}", i + 1))?
                }
            };
            object.scale(at.w, at.h).ctx("scale image")?;
            object
                .translate(PdfPoints::new(at.x), PdfPoints::new(at.y))
                .ctx("place image")?;
            let mut page = doc
                .pages_mut()
                .create_page_at_end(PdfPagePaperSize::from_points(
                    PdfPoints::new(at.page_w),
                    PdfPoints::new(at.page_h),
                ))
                .ctx(&format!("create page {}", i + 1))?;
            page.set_content_regeneration_strategy(PdfPageContentRegenerationStrategy::Manual);
            page.objects_mut()
                .add_image_object(object)
                .ctx("add image to page")?;
            page.regenerate_content().ctx("write page content")?;
        }
        raw::save::save_as_copy(
            raw::bindings(pdfium),
            &doc,
            raw::save::SaveFlags::NoIncremental,
        )?
    };
    let info = registry::open(st, None, bytes, None)?;
    let doc = st.doc_mut(&info.doc_id)?;
    doc.display_name = Some(format!("{}.pdf", images[0].stem));
    doc.saved_generation = 0;
    Ok(doc.info())
}

/// Both halves in one call — the engine-thread path tests use.
pub fn create_from_images(
    st: &mut EngineState<'_>,
    paths: &[String],
    options: &ImagesToPdfOptions,
) -> Result<DocInfo, EngineError> {
    let images: Vec<PreparedImage> = paths
        .iter()
        .map(|p| prepare_image(Path::new(p)))
        .collect::<Result<_, _>>()?;
    build(st, &images, options)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts(page_size: ImagePageSize, margin_pt: f32, fit: ImageFit) -> ImagesToPdfOptions {
        ImagesToPdfOptions::new(page_size, margin_pt, fit).unwrap()
    }

    #[test]
    fn original_is_the_image_plus_its_margin() {
        let p = layout(
            (144.0, 72.0),
            &opts(ImagePageSize::Original, 10.0, ImageFit::Contain),
        );
        assert_eq!((p.page_w, p.page_h), (164.0, 92.0));
        assert_eq!((p.x, p.y, p.w, p.h), (10.0, 10.0, 144.0, 72.0));
        // a giant image is scaled down to the PDF page limit
        let big = layout(
            (30_000.0, 15_000.0),
            &opts(ImagePageSize::Original, 0.0, ImageFit::Contain),
        );
        assert!((big.page_w - MAX_SIDE_PT).abs() < 0.01);
        assert!((big.page_h - MAX_SIDE_PT / 2.0).abs() < 0.01);
    }

    #[test]
    fn a4_turns_landscape_and_centres() {
        let p = layout(
            (800.0, 400.0),
            &opts(ImagePageSize::A4, 0.0, ImageFit::Contain),
        );
        assert!(
            p.page_w > p.page_h,
            "a landscape image gets a landscape page"
        );
        assert!((p.w - 841.89).abs() < 0.01);
        assert!((p.y - (595.28 - p.h) / 2.0).abs() < 0.01);
        // actual: a small image keeps its size, centred
        let small = layout(
            (100.0, 50.0),
            &opts(ImagePageSize::Letter, 36.0, ImageFit::Actual),
        );
        assert_eq!((small.w, small.h), (100.0, 50.0));
        assert!((small.x - (792.0 - 100.0) / 2.0).abs() < 0.01);
        // contain scales a small one up to the box less the margins
        let up = layout(
            (100.0, 200.0),
            &opts(ImagePageSize::Letter, 36.0, ImageFit::Contain),
        );
        assert!((up.h - (792.0 - 72.0)).abs() < 0.01);
    }

    #[test]
    fn margins_are_validated() {
        assert!(ImagesToPdfOptions::new(ImagePageSize::A4, -1.0, ImageFit::Contain).is_err());
        assert!(ImagesToPdfOptions::new(ImagePageSize::A4, f32::NAN, ImageFit::Contain).is_err());
        assert!(ImagesToPdfOptions::new(ImagePageSize::A4, 500.0, ImageFit::Contain).is_err());
    }

    #[test]
    fn exif_orientation_and_resolution() {
        // Big-endian TIFF, 3 entries: Orientation 6, XResolution 300/1, ResolutionUnit 2.
        let mut t = b"MM\0\x2a\0\0\0\x08".to_vec();
        t.extend_from_slice(&3u16.to_be_bytes());
        let entry = |tag: u16, kind: u16, value: [u8; 4]| {
            let mut e = tag.to_be_bytes().to_vec();
            e.extend_from_slice(&kind.to_be_bytes());
            e.extend_from_slice(&1u32.to_be_bytes());
            e.extend_from_slice(&value);
            e
        };
        t.extend(entry(0x0112, 3, [0, 6, 0, 0]));
        let rational_at = 8 + 2 + 3 * 12 + 4;
        t.extend(entry(0x011A, 5, (rational_at as u32).to_be_bytes()));
        t.extend(entry(0x0128, 3, [0, 2, 0, 0]));
        t.extend_from_slice(&0u32.to_be_bytes());
        t.extend_from_slice(&300u32.to_be_bytes());
        t.extend_from_slice(&1u32.to_be_bytes());
        let m = tiff_meta(&t);
        assert_eq!(m.orientation, Some(6));
        assert_eq!(m.dpi, Some((300.0, 300.0)));
    }
}
