//! v0.3 D1 — 이미지로 PDF 만들기 (`create_from_images`) against real PDFium.
//!
//! The images are generated here (solid colours, known pixel sizes and resolutions), so the
//! page sizes the engine picks can be checked to the point: `size_pt = px / dpi × 72`.

mod common;
use common::*;

use seepdf_lib::engine::pages::create::{self, ImagesToPdfOptions};
use seepdf_lib::engine::registry;
use seepdf_lib::engine::render::tiles;
use seepdf_lib::ipc::types::{DocInfo, ImageFit, ImagePageSize};
use std::path::PathBuf;

fn out_dir() -> PathBuf {
    let dir = fixture("out").join("v03-create");
    std::fs::create_dir_all(&dir).expect("create fixtures/out/v03-create");
    dir
}

/// A solid PNG, with a `pHYs` chunk when `dpi` is given.
fn png(name: &str, w: u32, h: u32, rgb: [u8; 3], dpi: Option<f32>) -> PathBuf {
    let path = out_dir().join(name);
    let file = std::fs::File::create(&path).expect("create png");
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w, h);
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    if let Some(dpi) = dpi {
        let ppm = (dpi / 0.0254).round() as u32;
        enc.set_pixel_dims(Some(png::PixelDimensions {
            xppu: ppm,
            yppu: ppm,
            unit: png::Unit::Meter,
        }));
    }
    let mut writer = enc.write_header().expect("png header");
    let data: Vec<u8> = (0..w * h).flat_map(|_| rgb).collect();
    writer.write_image_data(&data).expect("png data");
    path
}

/// A solid JPEG with a JFIF density, optionally with an EXIF orientation.
fn jpeg(name: &str, w: u32, h: u32, rgb: [u8; 3], dpi: u16, orientation: Option<u16>) -> PathBuf {
    use image::codecs::jpeg::{JpegEncoder, PixelDensity};
    let img = image::RgbImage::from_pixel(w, h, image::Rgb(rgb));
    let mut bytes = Vec::new();
    let mut enc = JpegEncoder::new_with_quality(&mut bytes, 90);
    enc.set_pixel_density(PixelDensity::dpi(dpi));
    enc.encode_image(&img).expect("encode jpeg");
    if let Some(o) = orientation {
        // APP1 "Exif\0\0" + big-endian TIFF with one IFD0 entry: Orientation (SHORT) = o.
        let mut tiff = b"MM\0\x2a\0\0\0\x08".to_vec();
        tiff.extend_from_slice(&1u16.to_be_bytes());
        tiff.extend_from_slice(&0x0112u16.to_be_bytes());
        tiff.extend_from_slice(&3u16.to_be_bytes());
        tiff.extend_from_slice(&1u32.to_be_bytes());
        tiff.extend_from_slice(&o.to_be_bytes());
        tiff.extend_from_slice(&[0, 0]);
        tiff.extend_from_slice(&0u32.to_be_bytes());
        let mut app1 = b"Exif\0\0".to_vec();
        app1.extend(tiff);
        let mut seg = vec![0xFF, 0xE1];
        seg.extend_from_slice(&((app1.len() + 2) as u16).to_be_bytes());
        seg.extend(app1);
        bytes.splice(2..2, seg);
    }
    let path = out_dir().join(name);
    std::fs::write(&path, bytes).expect("write jpeg");
    path
}

fn create(paths: &[PathBuf], page_size: ImagePageSize, margin: f32, fit: ImageFit) -> TestDoc {
    let paths: Vec<String> = paths.iter().map(|p| p.display().to_string()).collect();
    let options = ImagesToPdfOptions::new(page_size, margin, fit).expect("options");
    let info: DocInfo = with_state(move |st| create::create_from_images(st, &paths, &options))
        .expect("create_from_images");
    let doc_id = info.doc_id.clone();
    TestDoc { info, doc_id }
}

fn near(a: f32, b: f32) -> bool {
    (a - b).abs() < 0.5
}

/// The RGB of the pixel at the centre of `page`, rendered at scale 1.
fn centre_rgb(doc_id: &str, page: u16) -> [u8; 3] {
    let doc_id = doc_id.to_string();
    let buffer = with_state(move |st| tiles::render_raw_buffer(st, &doc_id, page, 1.0, None))
        .expect("render");
    let width = u32::from_le_bytes(buffer[8..12].try_into().unwrap()) as usize;
    let height = u32::from_le_bytes(buffer[12..16].try_into().unwrap()) as usize;
    let at = 32 + ((height / 2) * width + width / 2) * 4;
    [buffer[at], buffer[at + 1], buffer[at + 2]]
}

#[test]
fn create_three_images_at_their_own_resolution() {
    let a = png("a-144dpi.png", 200, 100, [220, 30, 30], Some(144.0));
    let b = jpeg("b-300dpi.jpg", 300, 150, [30, 30, 220], 300, None);
    let c = png("c-nodpi.png", 96, 48, [30, 200, 30], None);
    let doc = create(&[a, b, c], ImagePageSize::Original, 0.0, ImageFit::Contain);

    assert_eq!(doc.info.page_count, 3);
    assert!(doc.info.path.is_none(), "untitled until Save As");
    assert!(doc.info.dirty, "never saved: closing must ask");
    assert_eq!(doc.info.name, "a-144dpi.pdf");
    let sizes: Vec<(f32, f32)> = doc
        .info
        .pages
        .iter()
        .map(|p| (p.width_pt, p.height_pt))
        .collect();
    // 200 px at 144 dpi = 100 pt; 300 px at 300 dpi = 72 pt; 96 px at the 96 dpi default = 72 pt
    assert!(
        near(sizes[0].0, 100.0) && near(sizes[0].1, 50.0),
        "{sizes:?}"
    );
    assert!(
        near(sizes[1].0, 72.0) && near(sizes[1].1, 36.0),
        "{sizes:?}"
    );
    assert!(
        near(sizes[2].0, 72.0) && near(sizes[2].1, 36.0),
        "{sizes:?}"
    );

    // Each page shows its own image, edge to edge.
    let red = centre_rgb(&doc.doc_id, 0);
    assert!(red[0] > 180 && red[1] < 80, "page 1 is red: {red:?}");
    let blue = centre_rgb(&doc.doc_id, 1);
    assert!(
        blue[2] > 180 && blue[0] < 80,
        "page 2 (JPEG, embedded as is) is blue: {blue:?}"
    );
    let green = centre_rgb(&doc.doc_id, 2);
    assert!(
        green[1] > 160 && green[0] < 80,
        "page 3 is green: {green:?}"
    );

    // The JPEG went in as /DCTDecode (its own bytes), not re-encoded.
    let doc_id = doc.doc_id.clone();
    let bytes = with_state(move |st| Ok(st.doc(&doc_id)?.to_bytes()?.to_vec())).unwrap();
    let parsed = lopdf::Document::load_mem(&bytes).expect("lopdf");
    let dct = parsed.objects.values().any(|o| {
        o.as_stream().is_ok_and(|s| {
            s.dict
                .get(b"Filter")
                .ok()
                .and_then(|f| f.as_name().ok())
                .is_some_and(|f| f == b"DCTDecode")
        })
    });
    assert!(dct, "the JPEG is embedded with /DCTDecode");

    // Saved and reopened, the three pages are still there.
    let reopened = with_state(move |st| registry::open(st, None, bytes, None)).unwrap();
    assert_eq!(reopened.page_count, 3);
    let _ = with_state(move |st| registry::close(st, &reopened.doc_id));
}

#[test]
fn create_on_a4_with_margin_and_exif_rotation() {
    let wide = png("wide.png", 400, 200, [200, 40, 40], Some(72.0));
    // 300 × 150 px stored, EXIF 6 (rotate 90° clockwise): shown as 150 × 300.
    let rotated = jpeg("rotated.jpg", 300, 150, [40, 40, 200], 300, Some(6));
    let doc = create(
        &[wide.clone(), rotated.clone()],
        ImagePageSize::A4,
        36.0,
        ImageFit::Contain,
    );
    let p0 = &doc.info.pages[0];
    assert!(
        near(p0.width_pt, 841.89) && near(p0.height_pt, 595.28),
        "a landscape image gets a landscape A4: {p0:?}"
    );
    let p1 = &doc.info.pages[1];
    assert!(
        near(p1.width_pt, 595.28) && near(p1.height_pt, 841.89),
        "portrait after the EXIF rotation: {p1:?}"
    );

    let original = create(&[rotated], ImagePageSize::Original, 0.0, ImageFit::Contain);
    let p = &original.info.pages[0];
    assert!(
        near(p.width_pt, 36.0) && near(p.height_pt, 72.0),
        "EXIF 6 swaps the page: {p:?}"
    );
    let blue = centre_rgb(&original.doc_id, 0);
    assert!(blue[2] > 160, "{blue:?}");
}

#[test]
fn create_refuses_what_is_not_an_image() {
    let not_image = out_dir().join("not-an-image.png");
    std::fs::write(&not_image, b"%PDF-1.7 definitely not a png").unwrap();
    let path = not_image.display().to_string();
    let options = ImagesToPdfOptions::new(ImagePageSize::A4, 0.0, ImageFit::Contain).unwrap();
    let err = with_state(move |st| create::create_from_images(st, &[path], &options))
        .expect_err("a PDF named .png is refused");
    assert_eq!(err.code, seepdf_lib::ipc::ErrorCode::Unsupported);
    assert!(create::is_supported_image(&png(
        "ok.png",
        4,
        4,
        [0, 0, 0],
        None
    )));
    assert!(!create::is_supported_image(&not_image));
}

#[test]
fn clipboard_bytes_go_to_a_temp_image() {
    let bytes = std::fs::read(png("clip.png", 8, 8, [1, 2, 3], None)).unwrap();
    let path = seepdf_lib::commands::documents::write_temp_image_bytes(&bytes).expect("write");
    assert!(path.extension().is_some_and(|e| e == "png"));
    assert!(create::prepare_image(&path).is_ok());
    let err = seepdf_lib::commands::documents::write_temp_image_bytes(b"GIF89a....")
        .expect_err("a GIF is refused");
    assert_eq!(err.code, seepdf_lib::ipc::ErrorCode::Unsupported);
    let _ = std::fs::remove_file(path);
}
