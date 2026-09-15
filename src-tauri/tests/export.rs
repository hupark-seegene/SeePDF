//! Stage 1 (b) — export and print (`IPC_CONTRACT.md` §7.7).
//!
//! The image tests read the PNG/JPEG **header** back rather than trusting the renderer's
//! return value: `round(pt × dpi / 72)` is the acceptance criterion of F-24 and the only way
//! to check it is the file that was written.
//!
//! The flatten test is the one that matters most: the crate's `PdfPage::flatten()` is
//! `FLAT_PRINT`, which *deletes* annotations that lack the `/F 4` Print flag instead of drawing
//! them into the page. `fixtures/annotation-highlight.pdf`'s highlight has no Print flag, so
//! `FLAT_PRINT` would make it disappear and `FLAT_NORMALDISPLAY` bakes it in — a pixel
//! comparison tells the two apart.

mod common;
use common::*;

use seepdf_lib::engine::export;
use seepdf_lib::engine::raw;
use seepdf_lib::engine::registry;
use seepdf_lib::engine::render::tiles;
use seepdf_lib::engine::text::layer;
use seepdf_lib::ipc::types::{ImageFormat, Rect};
use std::path::PathBuf;

const DPI: u32 = 150;

fn out_dir() -> PathBuf {
    let dir = fixture("out").join("stage1b");
    std::fs::create_dir_all(&dir).expect("create fixtures/out/stage1b");
    dir
}

fn reopen(bytes: Vec<u8>) -> TestDoc {
    let info = with_state(move |st| registry::open(st, None, bytes, None)).expect("reopen");
    let doc_id = info.doc_id.clone();
    TestDoc { info, doc_id }
}

/// `(width, height)` straight out of the PNG IHDR / JPEG SOF header, without decoding.
fn image_size(path: &PathBuf) -> (u32, u32) {
    let bytes = std::fs::read(path).expect("read image");
    if bytes.starts_with(b"\x89PNG") {
        let w = u32::from_be_bytes(bytes[16..20].try_into().unwrap());
        let h = u32::from_be_bytes(bytes[20..24].try_into().unwrap());
        return (w, h);
    }
    // JPEG: walk the marker segments to the first SOFn.
    let mut i = 2usize;
    while i + 9 < bytes.len() {
        assert_eq!(bytes[i], 0xFF, "JPEG marker expected at {i}");
        let marker = bytes[i + 1];
        let len = u16::from_be_bytes([bytes[i + 2], bytes[i + 3]]) as usize;
        if (0xC0..=0xCF).contains(&marker) && !matches!(marker, 0xC4 | 0xC8 | 0xCC) {
            let h = u16::from_be_bytes([bytes[i + 5], bytes[i + 6]]) as u32;
            let w = u16::from_be_bytes([bytes[i + 7], bytes[i + 8]]) as u32;
            return (w, h);
        }
        i += 2 + len;
    }
    panic!("no SOF marker in {}", path.display());
}

/// RGBA pixels of one rectangle of a page (or the whole page with `None`) at 2x, through the
/// one render config.
fn render_area(doc_id: &str, page: u16, rect: Option<Rect>) -> (u32, u32, Vec<u8>) {
    let doc_id = doc_id.to_string();
    let buffer = with_state(move |st| tiles::render_raw_buffer(st, &doc_id, page, 2.0, rect))
        .expect("render page");
    let width = u32::from_le_bytes(buffer[8..12].try_into().unwrap());
    let height = u32::from_le_bytes(buffer[12..16].try_into().unwrap());
    (width, height, buffer[32..].to_vec())
}

/// Share of pixels that differ by more than 24 in any channel.
fn difference(a: &(u32, u32, Vec<u8>), b: &(u32, u32, Vec<u8>)) -> f64 {
    assert_eq!((a.0, a.1), (b.0, b.1), "the two renders must be the same size");
    let mut differing = 0usize;
    for (pa, pb) in a.2.chunks_exact(4).zip(b.2.chunks_exact(4)) {
        if pa
            .iter()
            .zip(pb)
            .any(|(x, y)| (*x as i32 - *y as i32).abs() > 24)
        {
            differing += 1;
        }
    }
    differing as f64 / (a.2.len() / 4) as f64
}

// ---------------------------------------------------------------------------------------

/// `round(pt × dpi / 72)` for every page, in both formats, plus an estimate that lands within
/// the ±25 % F-24 asks for.
#[test]
fn export_images_sizes() {
    let doc = open("tracemonkey.pdf");
    let dir = out_dir().join("export-images");
    let _ = std::fs::remove_dir_all(&dir);

    let mut total = 0u64;
    for page in [0u16, 1, 2] {
        let expected = with_state({
            let doc_id = doc.doc_id.clone();
            move |st| export::pixel_size(st, &doc_id, page, DPI)
        })
        .expect("pixel_size");

        let png = with_state({
            let doc_id = doc.doc_id.clone();
            let dir = dir.clone();
            move |st| {
                export::export_page_image(
                    st,
                    &doc_id,
                    page,
                    ImageFormat::Png,
                    DPI,
                    None,
                    &dir,
                    "tracemonkey",
                    false,
                )
            }
        })
        .expect("export_images");
        assert_eq!((png.width, png.height), expected);
        assert_eq!(image_size(&png.path), expected, "the PNG header agrees");
        assert!(png.path.file_name().unwrap().to_string_lossy().contains(&format!("{:03}", page + 1)));
        total += png.bytes;
    }
    // Letter at 150 DPI: 612 x 792 pt -> 1275 x 1650 px.
    assert_eq!(
        with_state({
            let doc_id = doc.doc_id.clone();
            move |st| export::pixel_size(st, &doc_id, 0, DPI)
        })
        .expect("pixel_size"),
        (1275, 1650)
    );

    // JPEG honours the quality knob and has no alpha.
    let jpeg = with_state({
        let doc_id = doc.doc_id.clone();
        let dir = dir.clone();
        move |st| {
            export::export_page_image(
                st,
                &doc_id,
                0,
                ImageFormat::Jpeg,
                DPI,
                Some(60),
                &dir,
                "tracemonkey",
                true,
            )
        }
    })
    .expect("export jpeg");
    assert_eq!(image_size(&jpeg.path), (1275, 1650));
    assert!(
        jpeg.path.extension().unwrap() == "jpg",
        "{:?}",
        jpeg.path
    );

    // The estimate samples real encodes, so it must be close to what three pages actually cost.
    let estimate = with_state({
        let doc_id = doc.doc_id.clone();
        move |st| export::estimate(st, &doc_id, &[0, 1, 2], ImageFormat::Png, DPI)
    })
    .expect("estimate_export");
    assert_eq!(estimate.sampled_pages, 3);
    let ratio = estimate.bytes as f64 / total as f64;
    assert!(
        (0.75..=1.25).contains(&ratio),
        "estimate {} vs actual {total} (ratio {ratio:.2})",
        estimate.bytes
    );

    // Out-of-range DPI is refused rather than silently clamped into a 2 GB bitmap.
    assert!(with_state({
        let doc_id = doc.doc_id.clone();
        move |st| export::estimate(st, &doc_id, &[0], ImageFormat::Png, 5000)
    })
    .is_err());
}

/// Text export reproduces `page.text().all()` per page with a form feed between pages (F-25).
#[test]
fn export_text() {
    let doc = open("tracemonkey.pdf");
    let path = out_dir().join("tracemonkey.txt");
    let result = with_state({
        let doc_id = doc.doc_id.clone();
        let path = path.display().to_string();
        move |st| export::export_text(st, &doc_id, &[0, 1, 2], &path)
    })
    .expect("export_text");

    let written = std::fs::read_to_string(&path).expect("read the export");
    assert_eq!(result.chars, written.chars().count() as u64);
    let pages: Vec<&str> = written.split('\u{0C}').collect();
    assert_eq!(pages.len(), 3, "one form feed between pages");

    for (i, page) in pages.iter().enumerate() {
        let expected = with_doc(&doc.doc_id, move |d| {
            Ok(layer::page_text(d, i as u16)?.text.clone())
        })
        .expect("page text");
        assert_eq!(*page, expected, "page {i} matches page.text().all()");
    }
    assert!(written.contains("Trace-based Just-in-Time"));

    // An empty page list means the whole document.
    let all = out_dir().join("tracemonkey-all.txt");
    let result = with_state({
        let doc_id = doc.doc_id.clone();
        let path = all.display().to_string();
        move |st| export::export_text(st, &doc_id, &[], &path)
    })
    .expect("export_text");
    assert!(result.chars > 80_000, "14 pages, ~84k characters");
}

/// `FPDFPage_Flatten(FLAT_NORMALDISPLAY)` bakes the annotations into the page content: the
/// flattened file has no annotations left **and renders the same**. `FLAT_PRINT` would have
/// deleted the highlight (it has no `/F 4`), which the pixel comparison catches.
#[test]
fn export_flatten_normaldisplay() {
    let doc = open("annotation-highlight.pdf");
    let before_annots = with_doc(&doc.doc_id, |d| Ok(d.page(0)?.annotations().len()))
        .expect("annotation count");
    assert!(before_annots > 0, "the fixture has annotations");

    // The fixture's producer set `/F 4`, so strip it: an annotation *without* the Print flag is
    // exactly the case `FLAT_PRINT` deletes and `FLAT_NORMALDISPLAY` draws, and a file from
    // another application is full of them.
    with_state({
        let doc_id = doc.doc_id.clone();
        move |st| {
            registry::mutate(
                st,
                &doc_id,
                registry::MutateOpts::new(
                    "test.clearPrintFlag",
                    seepdf_lib::ipc::types::ChangeReason::Edit,
                )
                .page(0),
                |d| {
                    let bindings = d.bindings();
                    let page = d.page(0)?;
                    let mut annot = raw::annot::get(bindings, page, 0)?;
                    annot.set_flags(raw::consts::FPDF_ANNOT_FLAG_NONE);
                    Ok(())
                },
            )
        }
    })
    .expect("clear /F");
    let printable = with_doc(&doc.doc_id, |d| {
        let bindings = d.bindings();
        let page = d.page(0)?;
        let annot = raw::annot::get(bindings, page, 0)?;
        Ok(annot.flags() & raw::consts::FPDF_ANNOT_FLAG_PRINT != 0)
    })
    .expect("read /F");
    assert!(!printable, "the highlight is now non-printable");

    // Compare inside the annotation's own rectangle: over a whole page a highlight is a
    // fraction of a percent of the pixels, which cannot tell "kept" from "deleted" apart.
    let rect = with_doc(&doc.doc_id, |d| {
        Ok(seepdf_lib::engine::annot::list(d, 0)?[0].rect)
    })
    .expect("annotation rect");
    let before = render_area(&doc.doc_id, 0, Some(rect));
    let path = out_dir().join("flattened.pdf");
    with_state({
        let doc_id = doc.doc_id.clone();
        let path = path.display().to_string();
        move |st| export::export_flattened(st, &doc_id, &path, true, true, None)
    })
    .expect("export_flattened");

    let flat = reopen(std::fs::read(&path).expect("read flattened"));
    let after_annots =
        with_doc(&flat.doc_id, |d| Ok(d.page(0)?.annotations().len())).expect("annotations");
    assert_eq!(after_annots, 0, "flattening removes the annotation objects");

    let after = render_area(&flat.doc_id, 0, Some(rect));
    let delta = difference(&before, &after);
    assert!(
        delta < 0.02,
        "FLAT_NORMALDISPLAY keeps the non-printable highlight: {:.1}% of the pixels inside its \
         rect differ",
        delta * 100.0
    );

    // The contrast, and the reason the crate's `flatten` feature stays off: the same page
    // through FLAT_PRINT loses the annotation entirely.
    let printed = reopen(
        with_doc(&doc.doc_id, |d| Ok(d.to_bytes()?.to_vec())).expect("bytes"),
    );
    with_state({
        let doc_id = printed.doc_id.clone();
        move |st| {
            let d = st.doc_mut(&doc_id)?;
            let bindings = d.bindings();
            let page = d.page(0)?;
            raw::page::flatten(bindings, page, raw::page::FlattenMode::Print)?;
            d.invalidate_page(0);
            Ok(())
        }
    })
    .expect("FLAT_PRINT");
    let print_flattened = render_area(&printed.doc_id, 0, Some(rect));
    let print_delta = difference(&before, &print_flattened);
    assert!(
        print_delta > delta * 5.0 && print_delta > 0.2,
        "FLAT_PRINT deletes an annotation without /F 4, so its rectangle must change a lot \
         ({:.1}% vs {:.1}% for NORMALDISPLAY) — this is why `PdfPage::flatten()` and the \
         crate's `flatten` feature are never used",
        print_delta * 100.0,
        delta * 100.0
    );

    // The document itself is untouched: flattening works on a scratch copy.
    let still_there =
        with_doc(&doc.doc_id, |d| Ok(d.page(0)?.annotations().len())).expect("annotations");
    assert_eq!(still_there, before_annots);
}

/// `print_prepare` writes a flattened temp copy and returns its path; a page selection is
/// applied before the flattening.
#[test]
fn export_print_prepare() {
    let doc = open("annotation-highlight.pdf");
    let path = with_state({
        let doc_id = doc.doc_id.clone();
        move |st| export::print_prepare(st, &doc_id, None)
    })
    .expect("print_prepare");
    assert!(path.is_file(), "{}", path.display());
    let prepared = reopen(std::fs::read(&path).expect("read"));
    assert_eq!(prepared.info.page_count, 1);
    assert_eq!(
        with_doc(&prepared.doc_id, |d| Ok(d.page(0)?.annotations().len())).expect("annots"),
        0,
        "the print copy is flattened"
    );

    let doc = open("tracemonkey.pdf");
    let path = with_state({
        let doc_id = doc.doc_id.clone();
        move |st| export::print_prepare(st, &doc_id, Some(&[2, 3]))
    })
    .expect("print_prepare with a range");
    let prepared = reopen(std::fs::read(&path).expect("read"));
    assert_eq!(prepared.info.page_count, 2, "only the selected pages");
}

/// `crop` reports the unrotated crop box, which is what the page→device matrix is built from.
#[test]
fn export_reports_crop_box() {
    let doc = open("rotation.pdf");
    let crop = with_state({
        let doc_id = doc.doc_id.clone();
        move |st| export::crop(st, &doc_id, 1)
    })
    .expect("crop");
    assert_eq!(crop, Rect::new(0.0, 0.0, 612.0, 792.0), "unrotated");
    let size = with_state({
        let doc_id = doc.doc_id.clone();
        move |st| export::pixel_size(st, &doc_id, 1, 72)
    })
    .expect("pixel_size");
    assert_eq!(size, (792, 612), "the exported image is the rotated size");
}
