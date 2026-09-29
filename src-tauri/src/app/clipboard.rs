//! v0.3 integration (D1): the system clipboard's image, read natively.
//!
//! 파일 ▸ 클립보드에서 새로 만들기 used `navigator.clipboard.read()`, which WebKit refuses
//! (`NotAllowedError`) outside a user gesture — and a native-menu click reaches the webview as a
//! Tauri event, with no user activation — so the menu item never worked on macOS. The image is
//! read here instead, with no gesture needed: `NSPasteboard` (PNG, else TIFF turned into PNG by
//! `NSBitmapImageRep`) on macOS; the registered `PNG` format, else `CF_DIB` turned into PNG, on
//! Windows. The frontend keeps `navigator.clipboard.read` as the fallback.

use crate::ipc::EngineError;

const PNG_MAGIC: &[u8] = b"\x89PNG\r\n\x1a\n";

/// The clipboard's image as PNG bytes, `None` when it holds no image. Must run on the main
/// thread on macOS (AppKit), which a synchronous Tauri command does.
pub fn read_image_png() -> Result<Option<Vec<u8>>, EngineError> {
    read_platform()
}

#[cfg(target_os = "macos")]
fn read_platform() -> Result<Option<Vec<u8>>, EngineError> {
    use objc2_app_kit::{
        NSBitmapImageFileType, NSBitmapImageRep, NSPasteboard, NSPasteboardTypePNG,
        NSPasteboardTypeTIFF,
    };
    use objc2_foundation::NSDictionary;
    let board = NSPasteboard::generalPasteboard();
    if let Some(png) = board.dataForType(unsafe { NSPasteboardTypePNG }) {
        let bytes = png.to_vec();
        if bytes.starts_with(PNG_MAGIC) {
            return Ok(Some(bytes));
        }
    }
    // Screenshots and most apps also (or only) offer TIFF.
    let Some(tiff) = board.dataForType(unsafe { NSPasteboardTypeTIFF }) else {
        return Ok(None);
    };
    let Some(rep) = NSBitmapImageRep::imageRepWithData(&tiff) else {
        return Ok(None);
    };
    let properties = NSDictionary::new();
    let png =
        unsafe { rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &properties) };
    Ok(png.map(|d| d.to_vec()))
}

#[cfg(windows)]
fn read_platform() -> Result<Option<Vec<u8>>, EngineError> {
    use windows::core::w;
    use windows::Win32::Foundation::HGLOBAL;
    use windows::Win32::System::DataExchange::{
        CloseClipboard, GetClipboardData, IsClipboardFormatAvailable, OpenClipboard,
        RegisterClipboardFormatW,
    };
    use windows::Win32::System::Memory::{GlobalLock, GlobalSize, GlobalUnlock};
    const CF_DIB: u32 = 8;

    // Another application may hold the clipboard for a moment.
    let mut opened = false;
    for _ in 0..10 {
        if unsafe { OpenClipboard(None) }.is_ok() {
            opened = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    if !opened {
        return Err(EngineError::io(
            "the clipboard is in use by another application",
        ));
    }
    struct Close;
    impl Drop for Close {
        fn drop(&mut self) {
            let _ = unsafe { CloseClipboard() };
        }
    }
    let _close = Close;
    let read = |format: u32| -> Option<Vec<u8>> {
        unsafe {
            IsClipboardFormatAvailable(format).ok()?;
            let handle = GetClipboardData(format).ok()?;
            let global = HGLOBAL(handle.0);
            let ptr = GlobalLock(global);
            if ptr.is_null() {
                return None;
            }
            let bytes = std::slice::from_raw_parts(ptr as *const u8, GlobalSize(global)).to_vec();
            let _ = GlobalUnlock(global);
            Some(bytes)
        }
    };
    let png_format = unsafe { RegisterClipboardFormatW(w!("PNG")) };
    if png_format != 0 {
        if let Some(bytes) = read(png_format).filter(|b| b.starts_with(PNG_MAGIC)) {
            return Ok(Some(bytes));
        }
    }
    match read(CF_DIB) {
        Some(dib) => dib_to_png(&dib).map(Some),
        None => Ok(None),
    }
}

#[cfg(not(any(target_os = "macos", windows)))]
fn read_platform() -> Result<Option<Vec<u8>>, EngineError> {
    Ok(None)
}

/// A packed DIB (`CF_DIB`: a `BITMAPINFOHEADER` or later header, the colour masks, then the
/// pixels) as PNG. 24- and 32-bit uncompressed (`BI_RGB`) or `BI_BITFIELDS` images — what
/// screenshots and browsers put there; anything else is `unsupported`.
pub fn dib_to_png(dib: &[u8]) -> Result<Vec<u8>, EngineError> {
    let unsupported = |why: &str| {
        EngineError::new(
            crate::ipc::ErrorCode::Unsupported,
            format!("the clipboard image cannot be read: {why}"),
        )
    };
    let u32_at = |o: usize| -> Option<u32> {
        dib.get(o..o + 4)
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    };
    let header = u32_at(0).ok_or_else(|| unsupported("too short"))? as usize;
    if header < 40 || dib.len() < header {
        return Err(unsupported("no bitmap header"));
    }
    let width = u32_at(4).unwrap_or(0) as i32;
    let height = u32_at(8).unwrap_or(0) as i32;
    let bits = u16::from_le_bytes([dib[14], dib[15]]);
    let compression = u32_at(16).unwrap_or(u32::MAX);
    if width <= 0 || height == 0 || width > 32_768 || height.unsigned_abs() > 32_768 {
        return Err(unsupported("bad size"));
    }
    const BI_RGB: u32 = 0;
    const BI_BITFIELDS: u32 = 3;
    let (w, h) = (width as usize, height.unsigned_abs() as usize);
    let mut offset = header;
    let masks = match (bits, compression) {
        (24, BI_RGB) => None,
        (32, BI_RGB) => Some([0x00FF_0000, 0x0000_FF00, 0x0000_00FF, 0xFF00_0000]),
        (32, BI_BITFIELDS) => {
            // With a 40-byte header the three masks follow it; later headers (V4 / V5) hold
            // them at the same offset.
            let m = |i: usize| u32_at(40 + 4 * i).unwrap_or(0);
            if header == 40 {
                offset += 12;
            }
            let alpha = if header >= 56 { m(3) } else { 0xFF00_0000 };
            Some([m(0), m(1), m(2), alpha])
        }
        _ => {
            return Err(unsupported(&format!(
                "{bits}-bit, compression {compression}"
            )))
        }
    };
    let stride = (w * bits as usize).div_ceil(32) * 4;
    if dib.len() < offset + stride * h {
        return Err(unsupported("truncated pixels"));
    }
    let channel = |px: u32, mask: u32| -> u8 {
        if mask == 0 {
            return 0;
        }
        let v = (px & mask) >> mask.trailing_zeros();
        let max = mask >> mask.trailing_zeros();
        ((v * 255 + max / 2) / max) as u8
    };
    let mut rgba = vec![0u8; w * h * 4];
    let mut any_alpha = false;
    for y in 0..h {
        // Positive height: bottom-up rows.
        let src_row = if height > 0 { h - 1 - y } else { y };
        let row = &dib[offset + src_row * stride..];
        for x in 0..w {
            let out = &mut rgba[(y * w + x) * 4..(y * w + x) * 4 + 4];
            match masks {
                None => {
                    let p = &row[x * 3..x * 3 + 3];
                    out.copy_from_slice(&[p[2], p[1], p[0], 255]);
                }
                Some([r, g, b, a]) => {
                    let p = &row[x * 4..x * 4 + 4];
                    let px = u32::from_le_bytes([p[0], p[1], p[2], p[3]]);
                    let alpha = channel(px, a);
                    any_alpha |= alpha != 0;
                    out.copy_from_slice(&[channel(px, r), channel(px, g), channel(px, b), alpha]);
                }
            }
        }
    }
    // A 32-bit DIB whose fourth byte is unused reads all-zero alpha: that is an opaque image.
    if masks.is_some() && !any_alpha {
        rgba.chunks_exact_mut(4).for_each(|p| p[3] = 255);
    }
    let image = image::RgbaImage::from_raw(w as u32, h as u32, rgba)
        .ok_or_else(|| unsupported("pixel buffer"))?;
    let mut out = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(image)
        .write_to(&mut out, image::ImageFormat::Png)
        .map_err(|e| unsupported(&e.to_string()))?;
    Ok(out.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(width: i32, height: i32, bits: u16, compression: u32) -> Vec<u8> {
        let mut h = Vec::new();
        h.extend_from_slice(&40u32.to_le_bytes());
        h.extend_from_slice(&width.to_le_bytes());
        h.extend_from_slice(&height.to_le_bytes());
        h.extend_from_slice(&1u16.to_le_bytes());
        h.extend_from_slice(&bits.to_le_bytes());
        h.extend_from_slice(&compression.to_le_bytes());
        h.extend_from_slice(&[0u8; 20]);
        h
    }

    fn decode(png: &[u8]) -> image::RgbaImage {
        image::load_from_memory(png).expect("png").to_rgba8()
    }

    #[test]
    fn a_bottom_up_24_bit_dib_becomes_png() {
        // 2 × 2: bottom row blue, red; top row green, white. Rows padded to 8 bytes.
        let mut dib = header(2, 2, 24, 0);
        dib.extend_from_slice(&[255, 0, 0, 0, 0, 255, 0, 0]); // bottom: blue, red (BGR)
        dib.extend_from_slice(&[0, 255, 0, 255, 255, 255, 0, 0]); // top: green, white
        let img = decode(&dib_to_png(&dib).unwrap());
        assert_eq!(img.dimensions(), (2, 2));
        assert_eq!(img.get_pixel(0, 0).0, [0, 255, 0, 255]);
        assert_eq!(img.get_pixel(1, 0).0, [255, 255, 255, 255]);
        assert_eq!(img.get_pixel(0, 1).0, [0, 0, 255, 255]);
        assert_eq!(img.get_pixel(1, 1).0, [255, 0, 0, 255]);
    }

    #[test]
    fn a_top_down_32_bit_bitfields_dib_with_unused_alpha_is_opaque() {
        let mut dib = header(1, -2, 32, 3);
        for mask in [0x00FF_0000u32, 0x0000_FF00, 0x0000_00FF] {
            dib.extend_from_slice(&mask.to_le_bytes());
        }
        dib.extend_from_slice(&[0, 0, 255, 0]); // red, alpha byte 0
        dib.extend_from_slice(&[255, 0, 0, 0]); // blue
        let img = decode(&dib_to_png(&dib).unwrap());
        assert_eq!(img.get_pixel(0, 0).0, [255, 0, 0, 255]);
        assert_eq!(img.get_pixel(0, 1).0, [0, 0, 255, 255]);
    }

    #[test]
    fn unsupported_dibs_are_refused() {
        assert!(dib_to_png(&header(2, 2, 8, 0)).is_err());
        assert!(dib_to_png(&[1, 2, 3]).is_err());
        assert!(dib_to_png(&header(2, 2, 24, 0)).is_err(), "no pixels");
    }
}
