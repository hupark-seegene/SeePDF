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
    assert_eq!(
        (a.0, a.1),
        (b.0, b.1),
        "the two renders must be the same size"
    );
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
        assert!(png
            .path
            .file_name()
            .unwrap()
            .to_string_lossy()
            .contains(&format!("{:03}", page + 1)));
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
    assert!(jpeg.path.extension().unwrap() == "jpg", "{:?}", jpeg.path);

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
    let before_annots =
        with_doc(&doc.doc_id, |d| Ok(d.page(0)?.annotations().len())).expect("annotation count");
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
    let printed = reopen(with_doc(&doc.doc_id, |d| Ok(d.to_bytes()?.to_vec())).expect("bytes"));
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

/// Widgets and non-widget annotations on one page.
fn kinds(doc_id: &str, page: u16) -> (usize, usize) {
    with_doc(doc_id, move |d| {
        let widgets = seepdf_lib::engine::form::list(d, Some(page))?.len();
        let markup = seepdf_lib::engine::annot::list(d, page)?
            .iter()
            .filter(|a| a.subtype != "Widget")
            .count();
        Ok((widgets, markup))
    })
    .expect("count annotations")
}

fn flatten(doc_id: &str, annotations: bool, forms: bool, name: &str) -> TestDoc {
    flatten_with(doc_id, annotations, forms, name, None)
}

fn flatten_with(
    doc_id: &str,
    annotations: bool,
    forms: bool,
    name: &str,
    password: Option<&str>,
) -> TestDoc {
    let path = out_dir().join(name);
    with_state({
        let doc_id = doc_id.to_string();
        let path = path.display().to_string();
        move |st| export::export_flattened(st, &doc_id, &path, annotations, forms, None)
    })
    .expect("export_flattened");
    let bytes = std::fs::read(&path).expect("read flattened");
    let password = password.map(str::to_owned);
    let info = with_state(move |st| registry::open(st, None, bytes, password)).expect("reopen");
    let doc_id = info.doc_id.clone();
    TestDoc { info, doc_id }
}

/// Bug hunt: `export_flattened` ignored its `annotations` / `forms` selection — PDFium's
/// `FPDFPage_Flatten` bakes every visible annotation and drops the whole `/Annots` array, so
/// 양식 병합 off still turned every field into static ink, and 주석 병합 off still baked every
/// comment. Each checkbox now does only what it says, on a rotated page too.
#[test]
fn export_flatten_respects_the_selection() {
    let doc = open("160F-2019.pdf");
    // A quarter turn first, so the baked appearances have to land right on a rotated page.
    with_state({
        let doc_id = doc.doc_id.clone();
        move |st| {
            seepdf_lib::engine::pages::apply_ops(
                st,
                &doc_id,
                vec![seepdf_lib::ipc::types::PageOp::Rotate {
                    pages: vec![0],
                    delta: 90,
                }],
            )
        }
    })
    .expect("rotate");
    let rect = Rect::new(60.0, 600.0, 260.0, 630.0);
    with_state({
        let doc_id = doc.doc_id.clone();
        move |st| {
            registry::mutate(
                st,
                &doc_id,
                registry::MutateOpts::new(
                    "undo.annotCreate",
                    seepdf_lib::ipc::types::ChangeReason::Edit,
                )
                .page(0),
                |d| {
                    seepdf_lib::engine::annot::create::create(
                        d,
                        0,
                        &seepdf_lib::ipc::types::AnnotSpec::Square(
                            seepdf_lib::ipc::types::ShapeSpec {
                                rect,
                                color: [220, 30, 30],
                                fill_color: Some([250, 220, 40]),
                                width: 2.0,
                                opacity: 1.0,
                            },
                        ),
                        None,
                    )
                },
            )
        }
    })
    .expect("add a comment");
    let (widgets, markup) = kinds(&doc.doc_id, 0);
    assert!(widgets > 0 && markup > 0, "page 0 has fields and a comment");
    let before_comment = render_area(&doc.doc_id, 0, Some(rect));
    let before_page = render_area(&doc.doc_id, 0, None);

    // 주석 병합 only: the comment is ink, every field is still a field.
    let comments_baked = flatten(&doc.doc_id, true, false, "flatten-annotations-only.pdf");
    assert_eq!(
        kinds(&comments_baked.doc_id, 0),
        (widgets, 0),
        "fields stay fillable, the comment is gone from /Annots"
    );
    let delta = difference(
        &before_comment,
        &render_area(&comments_baked.doc_id, 0, Some(rect)),
    );
    assert!(
        delta < 0.02,
        "the comment is baked in place: {:.1}%",
        delta * 100.0
    );
    let delta = difference(&before_page, &render_area(&comments_baked.doc_id, 0, None));
    assert!(
        delta < 0.01,
        "the page looks the same: {:.2}%",
        delta * 100.0
    );

    // 양식 병합 only: the fields are ink, the comment is still an annotation.
    let fields_baked = flatten(&doc.doc_id, false, true, "flatten-forms-only.pdf");
    assert_eq!(
        kinds(&fields_baked.doc_id, 0),
        (0, markup),
        "the fields are baked, the comment stays"
    );
    let delta = difference(&before_page, &render_area(&fields_baked.doc_id, 0, None));
    assert!(
        delta < 0.01,
        "the page looks the same: {:.2}%",
        delta * 100.0
    );

    // Both: nothing interactive is left (the old behaviour, unchanged).
    let all = flatten(&doc.doc_id, true, true, "flatten-both.pdf");
    assert_eq!(kinds(&all.doc_id, 0), (0, 0));

    // The partial copies do not duplicate the page's fonts and images.
    let size = |name: &str| std::fs::metadata(out_dir().join(name)).unwrap().len();
    let full = size("flatten-both.pdf");
    for name in ["flatten-annotations-only.pdf", "flatten-forms-only.pdf"] {
        assert!(
            size(name) < full + full / 5,
            "{name} is {} bytes against {full} for the full flatten",
            size(name)
        );
    }

    // An encrypted document stays encrypted, and the kept fields are still fields.
    let protected = out_dir().join("flatten-protected.pdf");
    let _ = std::fs::remove_file(&protected);
    with_state({
        let doc_id = doc.doc_id.clone();
        let path = protected.display().to_string();
        move |st| {
            seepdf_lib::engine::security::set_password(
                st,
                &doc_id,
                &path,
                Some("pw"),
                "owner",
                seepdf_lib::ipc::types::PermissionsRequest::default(),
            )
        }
    })
    .expect("set_password");
    let bytes = std::fs::read(&protected).unwrap();
    let info = with_state(move |st| registry::open(st, None, bytes, Some("pw".into())))
        .expect("open the protected copy");
    let locked = TestDoc {
        doc_id: info.doc_id.clone(),
        info,
    };
    assert_eq!(kinds(&locked.doc_id, 0), (widgets, markup));
    let kept = flatten_with(
        &locked.doc_id,
        true,
        false,
        "flatten-protected-annots.pdf",
        Some("pw"),
    );
    assert!(kept.info.encrypted);
    assert_eq!(kinds(&kept.doc_id, 0), (widgets, 0));
    let bytes = std::fs::read(out_dir().join("flatten-protected-annots.pdf")).unwrap();
    let refused = with_state(move |st| registry::open(st, None, bytes, None).map(|i| i.doc_id));
    assert!(refused.is_err(), "still needs the password");
    let kept = flatten_with(
        &locked.doc_id,
        false,
        true,
        "flatten-protected-forms.pdf",
        Some("pw"),
    );
    assert_eq!(kinds(&kept.doc_id, 0), (0, markup));
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

// ---------------------------------------------------------------------------------------
// v0.3 pkg8 — print variant (X7), print temp files (X8), n-up (X2)
// ---------------------------------------------------------------------------------------

use seepdf_lib::engine::render::cache::{Night, PrintAnnots, RenderKind, TileKey};

/// One whole-page render at 1× through `tiles::render` with the given kind (RGBA).
fn render_kind(doc_id: &str, page: u16, kind: RenderKind) -> (u32, u32, Vec<u8>) {
    let doc_id = doc_id.to_string();
    let raw = with_state(move |st| {
        let generation = st.doc(&doc_id)?.generation;
        let key = TileKey {
            doc: doc_id.clone(),
            generation,
            page,
            kind,
            scale_key: 100,
            rotation: 0,
            tx: 0,
            ty: 0,
            night: Night::Off,
            hl: false,
            forms: true,
        };
        tiles::render(st, &tiles::RenderRequest::new(key))
    })
    .expect("render");
    (raw.width, raw.height, raw.pixels)
}

/// Share of pixels inside `r` (PDF points, y-up, page height `h_pt`, 1×) that differ by more
/// than 24 in any channel.
fn region_difference(a: &(u32, u32, Vec<u8>), b: &(u32, u32, Vec<u8>), r: Rect, h_pt: f32) -> f64 {
    assert_eq!((a.0, a.1), (b.0, b.1));
    let (x0, x1) = (r.l.max(0.0) as u32, (r.r as u32).min(a.0));
    let (y0, y1) = ((h_pt - r.t).max(0.0) as u32, ((h_pt - r.b) as u32).min(a.1));
    let (mut total, mut differing) = (0usize, 0usize);
    for y in y0..y1 {
        for x in x0..x1 {
            let i = ((y * a.0 + x) * 4) as usize;
            total += 1;
            if (0..3).any(|c| (a.2[i + c] as i32 - b.2[i + c] as i32).abs() > 24) {
                differing += 1;
            }
        }
    }
    differing as f64 / total.max(1) as f64
}

fn create_annot(doc_id: &str, page: u16, spec: seepdf_lib::ipc::types::AnnotSpec) -> String {
    let doc_id = doc_id.to_string();
    with_state(move |st| {
        registry::mutate(
            st,
            &doc_id,
            registry::MutateOpts::new(
                "undo.annotCreate",
                seepdf_lib::ipc::types::ChangeReason::Edit,
            )
            .page(page),
            |doc| seepdf_lib::engine::annot::create::create(doc, page, &spec, None),
        )
    })
    .expect("create annotation")
}

/// Sets `/F` on the annotation whose `/NM` is `id`.
fn set_annot_flags(doc_id: &str, page: u16, id: &str, flags: i32) {
    let doc_id = doc_id.to_string();
    let id = id.to_string();
    with_state(move |st| {
        registry::mutate(
            st,
            &doc_id,
            registry::MutateOpts::new("test.flags", seepdf_lib::ipc::types::ChangeReason::Edit)
                .page(page),
            |d| {
                let bindings = d.bindings();
                let page = d.page(page)?;
                for i in 0..raw::annot::count(bindings, page) {
                    if let Some(mut a) = raw::annot::slot(bindings, page, i) {
                        if a.string("NM").as_deref() == Some(id.as_str()) {
                            a.set_flags(flags);
                        }
                    }
                }
                Ok(())
            },
        )
    })
    .expect("set flags");
}

fn square(rect: Rect, rgb: [u8; 3]) -> seepdf_lib::ipc::types::AnnotSpec {
    seepdf_lib::ipc::types::AnnotSpec::Square(seepdf_lib::ipc::types::ShapeSpec {
        rect,
        color: rgb,
        fill_color: Some(rgb),
        width: 2.0,
        opacity: 1.0,
    })
}

/// X7: the print variant (`/page?print=…`) passes `FPDF_PRINTING`, so an annotation with
/// NoView | Print (a print-only watermark) is drawn on paper and not on screen, one without
/// Print is drawn on screen and not on paper, and 문서만 draws neither.
#[test]
fn print_variant_honours_print_and_noview_flags() {
    use raw::consts::{FPDF_ANNOT_FLAG_NOVIEW, FPDF_ANNOT_FLAG_PRINT};
    let doc = open("tracemonkey.pdf");
    let h = doc.info.pages[0].height_pt;
    let print_only = Rect::new(60.0, 40.0, 200.0, 120.0);
    let screen_only = Rect::new(380.0, 40.0, 520.0, 120.0);
    let a = create_annot(&doc.doc_id, 0, square(print_only, [0, 160, 0]));
    set_annot_flags(
        &doc.doc_id,
        0,
        &a,
        FPDF_ANNOT_FLAG_NOVIEW | FPDF_ANNOT_FLAG_PRINT,
    );
    let b = create_annot(&doc.doc_id, 0, square(screen_only, [200, 0, 0]));
    set_annot_flags(&doc.doc_id, 0, &b, 0);

    let screen = render_kind(&doc.doc_id, 0, RenderKind::Page);
    let print = render_kind(&doc.doc_id, 0, RenderKind::Print(PrintAnnots::All));
    let bare = render_kind(&doc.doc_id, 0, RenderKind::Print(PrintAnnots::None));

    // NoView | Print: on paper, not on screen.
    assert!(
        region_difference(&print, &bare, print_only, h) > 0.5,
        "printed"
    );
    assert!(
        region_difference(&screen, &bare, print_only, h) < 0.02,
        "not on screen"
    );
    // No Print flag: on screen, not on paper.
    assert!(
        region_difference(&screen, &bare, screen_only, h) > 0.5,
        "on screen"
    );
    assert!(
        region_difference(&print, &bare, screen_only, h) < 0.02,
        "not printed"
    );

    // The screen render is untouched by the print variant.
    assert_eq!(render_kind(&doc.doc_id, 0, RenderKind::Page).2, screen.2);
}

/// X7: 문서와 도장·서명 keeps signatures (and stamps) and hides every other annotation — for
/// that one render only.
#[test]
fn print_variant_stamps_mode_keeps_only_signatures() {
    let doc = open("tracemonkey.pdf");
    let h = doc.info.pages[0].height_pt;
    let sig_rect = Rect::new(60.0, 40.0, 200.0, 120.0);
    let box_rect = Rect::new(380.0, 40.0, 520.0, 120.0);
    create_annot(
        &doc.doc_id,
        0,
        seepdf_lib::ipc::types::AnnotSpec::Signature(seepdf_lib::ipc::types::InkSpec {
            paths: vec![(0..=40)
                .flat_map(|i| {
                    let t = i as f32 / 40.0;
                    [65.0 + 130.0 * t, 45.0 + 70.0 * (t * 12.0).sin().abs()]
                })
                .collect()],
            color: [0, 0, 160],
            width: 6.0,
            opacity: 1.0,
        }),
    );
    create_annot(&doc.doc_id, 0, square(box_rect, [200, 0, 0]));

    let all = render_kind(&doc.doc_id, 0, RenderKind::Print(PrintAnnots::All));
    let stamps = render_kind(&doc.doc_id, 0, RenderKind::Print(PrintAnnots::Stamps));
    let bare = render_kind(&doc.doc_id, 0, RenderKind::Print(PrintAnnots::None));
    assert!(
        region_difference(&stamps, &bare, sig_rect, h) > 0.03,
        "the signature prints"
    );
    assert!(region_difference(&stamps, &all, sig_rect, h) < 0.01);
    assert!(
        region_difference(&all, &bare, box_rect, h) > 0.5,
        "the square prints in 문서와 주석"
    );
    assert!(
        region_difference(&stamps, &bare, box_rect, h) < 0.02,
        "…but not in 문서와 도장·서명"
    );

    // The hide was transient: 문서와 주석 renders the same again, and no flag is left set.
    assert_eq!(
        render_kind(&doc.doc_id, 0, RenderKind::Print(PrintAnnots::All)).2,
        all.2
    );
    let flags = with_doc(&doc.doc_id, |d| {
        let bindings = d.bindings();
        let page = d.page(0)?;
        Ok((0..raw::annot::count(bindings, page))
            .filter_map(|i| raw::annot::slot(bindings, page, i).map(|a| a.flags()))
            .collect::<Vec<_>>())
    })
    .expect("flags");
    assert!(
        flags
            .iter()
            .all(|f| f & raw::consts::FPDF_ANNOT_FLAG_HIDDEN == 0),
        "{flags:?}"
    );
}

/// X7: the handler path flattens with `FLAT_PRINT` and the 주석 option: a NoView | Print
/// annotation is baked in, a screen-only one is not, and 문서만 bakes neither.
#[test]
fn print_prepare_honours_flags_and_the_annotation_option() {
    use raw::consts::{FPDF_ANNOT_FLAG_NOVIEW, FPDF_ANNOT_FLAG_PRINT};
    let doc = open("tracemonkey.pdf");
    let h = doc.info.pages[0].height_pt;
    let print_only = Rect::new(60.0, 40.0, 200.0, 120.0);
    let screen_only = Rect::new(380.0, 40.0, 520.0, 120.0);
    let a = create_annot(&doc.doc_id, 0, square(print_only, [0, 160, 0]));
    set_annot_flags(
        &doc.doc_id,
        0,
        &a,
        FPDF_ANNOT_FLAG_NOVIEW | FPDF_ANNOT_FLAG_PRINT,
    );
    let b = create_annot(&doc.doc_id, 0, square(screen_only, [200, 0, 0]));
    set_annot_flags(&doc.doc_id, 0, &b, 0);

    let prepare = |annots: PrintAnnots| {
        let doc_id = doc.doc_id.clone();
        let path =
            with_state(move |st| export::print_prepare_with(st, &doc_id, Some(&[0]), annots))
                .expect("print_prepare");
        assert!(
            path.starts_with(export::print_temp_dir()),
            "{}",
            path.display()
        );
        let printed = reopen(std::fs::read(&path).expect("read"));
        let _ = std::fs::remove_file(&path);
        let pixels = render_kind(&printed.doc_id, 0, RenderKind::Page);
        let count =
            with_doc(&printed.doc_id, |d| Ok(d.page(0)?.annotations().len())).expect("annots");
        (pixels, count)
    };
    let (all, all_annots) = prepare(PrintAnnots::All);
    let (bare, bare_annots) = prepare(PrintAnnots::None);
    assert_eq!(
        (all_annots, bare_annots),
        (0, 0),
        "everything is flattened or dropped"
    );
    assert!(
        region_difference(&all, &bare, print_only, h) > 0.5,
        "NoView|Print is baked in"
    );
    assert!(
        region_difference(&all, &bare, screen_only, h) < 0.02,
        "a screen-only annotation is not"
    );
}

/// X8: `cleanup_print_dir` deletes files older than the limit and keeps fresh ones;
/// `cleanup_print_temp` (startup / exit) empties the print temp directory.
#[test]
fn cleanup_print_temp_removes_stale_files() {
    let dir = out_dir().join("print-temp-sweep");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let stale = dir.join("stale.pdf");
    let fresh = dir.join("fresh.pdf");
    std::fs::write(&stale, b"%PDF-1.7 stale").unwrap();
    std::fs::write(&fresh, b"%PDF-1.7 fresh").unwrap();
    let old = std::time::SystemTime::now() - std::time::Duration::from_secs(11 * 60);
    std::fs::File::options()
        .write(true)
        .open(&stale)
        .unwrap()
        .set_modified(old)
        .unwrap();

    assert_eq!(
        export::cleanup_print_dir(&dir, export::PRINT_TEMP_MAX_AGE),
        1
    );
    assert!(!stale.exists(), "the stale copy is gone");
    assert!(
        fresh.exists(),
        "a copy the OS handler may still be reading stays"
    );
    assert_eq!(
        export::cleanup_print_dir(&dir, std::time::Duration::ZERO),
        1
    );
    assert!(!fresh.exists());
    assert_eq!(
        export::cleanup_print_dir(&dir.join("missing"), std::time::Duration::ZERO),
        0
    );
    // `cleanup_print_temp()` is `cleanup_print_dir(print_temp_dir(), 0)`; it is not called
    // here because the other tests of this process print into that directory concurrently.
    // `print_prepare_honours_flags_and_the_annotation_option` checks that every print file
    // lands in `print_temp_dir()`.
}

/// X2: 8 pages at 4-up give 2 sheets, a booklet of 8 gives 4 landscape sides in
/// saddle-stitch order, and 2-up of portrait pages is one landscape sheet per 2 pages.
#[test]
fn nup_layouts() {
    use seepdf_lib::engine::export::nup::{self, NupOptions, NupOrder, NupPaper};
    let doc = open("tracemonkey.pdf");
    let pages: Vec<u16> = (0..8).collect();
    let run = |opts: NupOptions| {
        let doc_id = doc.doc_id.clone();
        let pages = pages.clone();
        let (bytes, count) =
            with_state(move |st| nup::make_nup_bytes(st, &doc_id, Some(&pages), &opts))
                .expect("make_nup");
        let reopened = reopen(bytes);
        assert_eq!(reopened.info.page_count as u32, count);
        reopened
    };
    let base = NupOptions {
        per_sheet: 4,
        order: NupOrder::Across,
        booklet: false,
        paper: NupPaper::A4,
        annots: PrintAnnots::All,
    };
    let four = run(base);
    assert_eq!(four.info.page_count, 2, "8 pages at 4-up");
    let g = &four.info.pages[0];
    assert!((g.width_pt - nup::A4.0).abs() < 1.0 && (g.height_pt - nup::A4.1).abs() < 1.0);

    let down = run(NupOptions {
        order: NupOrder::Down,
        ..base
    });
    assert_eq!(down.info.page_count, 2);

    let two = run(NupOptions {
        per_sheet: 2,
        ..base
    });
    assert_eq!(two.info.page_count, 4);
    assert!(
        two.info.pages[0].width_pt > two.info.pages[0].height_pt,
        "2-up is landscape"
    );

    let nine = run(NupOptions {
        per_sheet: 9,
        ..base
    });
    assert_eq!(nine.info.page_count, 1);

    let booklet = run(NupOptions {
        booklet: true,
        ..base
    });
    assert_eq!(booklet.info.page_count, 4, "8 pages = 2 sheets = 4 sides");
    assert!(booklet.info.pages[0].width_pt > booklet.info.pages[0].height_pt);
    let order: Vec<usize> = nup::booklet_order(8)
        .into_iter()
        .map(|p| p.unwrap() + 1)
        .collect();
    assert_eq!(order, vec![8, 1, 2, 7, 6, 3, 4, 5]);

    // The sheets carry the pages' content: the first 4-up sheet has the pages' text.
    let text = with_doc(&four.doc_id, |d| Ok(layer::page_text(d, 0)?.text.len())).expect("text");
    assert!(
        text > 1000,
        "the 4-up sheet has the pages' text ({text} bytes)"
    );

    let bad = with_state({
        let doc_id = doc.doc_id.clone();
        move |st| {
            nup::make_nup_bytes(
                st,
                &doc_id,
                None,
                &NupOptions {
                    per_sheet: 3,
                    ..base
                },
            )
        }
    });
    assert!(bad.is_err(), "3-up is not a layout");
}

// ---------------------------------------------------------------------------------------
// v0.3 pkg8 — export additions (X3) and text flow (X6)
// ---------------------------------------------------------------------------------------

use pdfium_render::prelude::*;
use seepdf_lib::engine::export::job::JobSink;
use seepdf_lib::engine::export::pagejob;
use seepdf_lib::ipc::types::JobEvent;
use std::sync::{Arc, Mutex};

/// Runs a page job started by `start` to its terminal event and returns every event.
fn run_job(start: impl FnOnce(JobSink) -> seepdf_lib::ipc::types::JobId) -> Vec<JobEvent> {
    let events: Arc<Mutex<Vec<JobEvent>>> = Arc::default();
    let sink_events = events.clone();
    let sink: JobSink = Arc::new(move |e| sink_events.lock().unwrap().push(e));
    start(sink);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    loop {
        let done = events.lock().unwrap().iter().any(|e| {
            matches!(
                e,
                JobEvent::Done { .. } | JobEvent::Cancelled { .. } | JobEvent::Error { .. }
            )
        });
        if done {
            break;
        }
        assert!(std::time::Instant::now() < deadline, "job did not finish");
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let out = events.lock().unwrap().clone();
    out
}

fn outputs(events: &[JobEvent]) -> Vec<String> {
    match events.last() {
        Some(JobEvent::Done { outputs, .. }) => outputs.clone().unwrap_or_default(),
        other => panic!("the job did not finish with done: {other:?}"),
    }
}

fn swatch(w: u32, h: u32, rgb: [u8; 3]) -> image::DynamicImage {
    image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(w, h, |x, y| {
        let t = ((x + y) % 7) as u8 * 8;
        image::Rgb([rgb[0].saturating_add(t), rgb[1], rgb[2]])
    }))
}

/// Two A4 pages built with PDFium: headings, Korean body text in the bundled Hangul font, and
/// images (two on page 1, one on page 2).
fn korean_flow_doc() -> TestDoc {
    let bytes = with_state(|st| {
        let mut doc = st.pdfium.create_new_pdf().expect("new pdf");
        let font = seepdf_lib::engine::fonts::load(&mut doc)?;
        let text = |page: &mut PdfPage, x: f32, y: f32, s: &str, size: f32| {
            page.objects_mut()
                .create_text_object(
                    PdfPoints::new(x),
                    PdfPoints::new(y),
                    s,
                    font,
                    PdfPoints::new(size),
                )
                .expect("text");
        };
        let lines = [
            "첫째 문단입니다. 한글 본문이 여러 줄에",
            "걸쳐 이어지고 문단으로 묶여야 합니다.",
            "세 번째 줄까지 같은 문단입니다.",
        ];
        {
            let mut page = doc
                .pages_mut()
                .create_page_at_end(PdfPagePaperSize::a4())
                .expect("page 1");
            text(&mut page, 72.0, 770.0, "분기 보고서", 26.0);
            for (i, l) in lines.iter().enumerate() {
                text(&mut page, 72.0, 730.0 - 14.0 * i as f32, l, 11.0);
            }
            text(&mut page, 72.0, 470.0, "두 번째 절", 16.0);
            text(&mut page, 72.0, 440.0, "그림 아래의 문단입니다.", 11.0);
            page.regenerate_content().expect("regenerate");
        }
        {
            let mut page = doc
                .pages_mut()
                .create_page_at_end(PdfPagePaperSize::a4())
                .expect("page 2");
            text(&mut page, 72.0, 770.0, "둘째 쪽의 본문입니다.", 11.0);
            page.regenerate_content().expect("regenerate");
        }
        let images: [(u16, f32, [u8; 3]); 3] = [
            (0, 520.0, [200, 30, 30]),
            (0, 300.0, [30, 30, 200]),
            (1, 600.0, [30, 160, 30]),
        ];
        for (index, y, rgb) in images {
            let mut object = PdfPageImageObject::new_with_size(
                &doc,
                &swatch(160, 100, rgb),
                PdfPoints::new(160.0),
                PdfPoints::new(100.0),
            )
            .expect("image");
            object
                .translate(PdfPoints::new(72.0), PdfPoints::new(y))
                .expect("place");
            let mut page = doc.pages().get(index as i32).expect("page");
            page.objects_mut().add_image_object(object).expect("add");
            page.regenerate_content().expect("regenerate");
        }
        Ok(doc.save_to_bytes().expect("save"))
    })
    .expect("build the Korean flow document");
    reopen(bytes)
}

/// X3 이미지 추출: one PNG per embedded image, named `<base>-p<page>-<n>.png`.
#[test]
fn embedded_images_count_matches() {
    let doc = korean_flow_doc();
    let dir = out_dir().join("embedded");
    let _ = std::fs::remove_dir_all(&dir);
    let events = run_job(|sink| {
        pagejob::embedded_images(
            engine(),
            sink,
            doc.doc_id.clone(),
            vec![0, 1],
            dir.clone(),
            "보고서".into(),
        )
    });
    let mut files = outputs(&events);
    files.sort();
    let names: Vec<String> = files
        .iter()
        .map(|p| {
            PathBuf::from(p)
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    assert_eq!(
        names,
        vec!["보고서-p1-1.png", "보고서-p1-2.png", "보고서-p2-1.png"]
    );
    for f in &files {
        let (w, h) = image_size(&PathBuf::from(f));
        assert_eq!((w, h), (160, 100), "the image's own pixels, not a render");
    }
}

/// X3 하나의 이미지로 이어 붙이기: the height is the sum of the page heights, the width the
/// widest page; a request over the pixel cap lowers the DPI instead of failing.
#[test]
fn stitched_image_height_is_the_sum() {
    let doc = open("rotation.pdf");
    let pages: Vec<u16> = (0..doc.info.page_count).collect();
    let plan = with_state({
        let doc_id = doc.doc_id.clone();
        let pages = pages.clone();
        move |st| export::stitch_plan(st, &doc_id, &pages, 72)
    })
    .expect("plan");
    let sizes: Vec<(u32, u32)> = pages
        .iter()
        .map(|&p| {
            let doc_id = doc.doc_id.clone();
            with_state(move |st| export::pixel_size(st, &doc_id, p, 72)).unwrap()
        })
        .collect();
    let sum: u32 = sizes.iter().map(|s| s.1).sum();
    let widest = sizes.iter().map(|s| s.0).max().unwrap();
    assert!(!plan.lowered);
    assert_eq!((plan.width, plan.height), (widest, sum));

    let path = out_dir().join("stitched.png");
    let events = run_job(|sink| {
        pagejob::stitched_image(
            engine(),
            sink,
            doc.doc_id.clone(),
            pages.clone(),
            plan,
            ImageFormat::Png,
            path.clone(),
        )
    });
    assert_eq!(outputs(&events), vec![path.display().to_string()]);
    assert_eq!(
        image_size(&path),
        (widest, sum),
        "stitched height is the sum"
    );

    // 500 pages at 600 DPI would be ~2.3 Gpx: the plan lowers the DPI under the cap.
    let long = open("gen/500p.pdf");
    let big = with_state({
        let doc_id = long.doc_id.clone();
        move |st| {
            let pages: Vec<u16> = (0..40).collect();
            export::stitch_plan(st, &doc_id, &pages, 600)
        }
    })
    .expect("plan with a lowered DPI");
    assert!(big.lowered && big.dpi < 600, "{big:?}");
    assert!(big.height <= export::STITCH_MAX_SIDE);
    assert!((big.width as u64) * (big.height as u64) <= export::STITCH_MAX_PX);
}

/// X3 여러 페이지 TIFF: one frame per page, at the requested DPI.
#[test]
fn tiff_has_one_frame_per_page() {
    let doc = open("tracemonkey.pdf");
    let path = out_dir().join("pages.tiff");
    let events = run_job(|sink| {
        pagejob::tiff(
            engine(),
            sink,
            doc.doc_id.clone(),
            vec![0, 1, 2],
            72,
            path.clone(),
        )
        .expect("start")
    });
    assert_eq!(outputs(&events), vec![path.display().to_string()]);
    let file = std::fs::File::open(&path).expect("tiff written");
    let mut decoder = tiff::decoder::Decoder::new(std::io::BufReader::new(file)).expect("decode");
    let mut frames = 1;
    assert_eq!(decoder.dimensions().unwrap(), (612, 792));
    while decoder.more_images() {
        decoder.next_image().expect("next frame");
        frames += 1;
    }
    assert_eq!(frames, 3, "one frame per page");
    let mut part = path.as_os_str().to_owned();
    part.push(".part");
    assert!(!PathBuf::from(part).exists(), "the .part file was renamed");
}

/// X3 레이아웃 유지: the second column of a table starts at the same column on every row.
#[test]
fn preserve_layout_keeps_columns_aligned() {
    let bytes = with_state(|st| {
        let mut doc = st.pdfium.create_new_pdf().expect("new pdf");
        let font = doc.fonts_mut().helvetica();
        {
            let mut page = doc
                .pages_mut()
                .create_page_at_end(PdfPagePaperSize::a4())
                .expect("page");
            let rows = [("Name", "Value"), ("A", "1"), ("Longer name", "12345")];
            for (i, (a, b)) in rows.iter().enumerate() {
                let y = 700.0 - 16.0 * i as f32;
                for (x, s) in [(72.0, *a), (300.0, *b)] {
                    page.objects_mut()
                        .create_text_object(
                            PdfPoints::new(x),
                            PdfPoints::new(y),
                            s,
                            font,
                            PdfPoints::new(11.0),
                        )
                        .expect("text");
                }
            }
            page.regenerate_content().expect("regenerate");
        }
        Ok(doc.save_to_bytes().expect("save"))
    })
    .unwrap();
    let doc = reopen(bytes);
    let path = out_dir().join("layout.txt");
    with_state({
        let doc_id = doc.doc_id.clone();
        let path = path.display().to_string();
        move |st| export::export_text_with(st, &doc_id, &[0], &path, true)
    })
    .expect("export");
    let text = std::fs::read_to_string(&path).unwrap();
    let rows: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    assert_eq!(rows.len(), 3, "{text}");
    let columns: Vec<usize> = rows
        .iter()
        .zip(["Value", "1", "12345"])
        .map(|(row, cell)| row.rfind(cell).expect(cell))
        .collect();
    assert!(
        columns.iter().all(|&c| c == columns[0]),
        "the second column stays aligned: {columns:?}\n{text}"
    );
    assert!(rows[0].starts_with("Name") && rows[2].starts_with("Longer name"));

    // Without the option it is the plain text export.
    with_state({
        let doc_id = doc.doc_id.clone();
        let path = path.display().to_string();
        move |st| export::export_text_with(st, &doc_id, &[0], &path, false)
    })
    .expect("export");
    assert!(!std::fs::read_to_string(&path).unwrap().contains("       "));
}

fn flow(doc: &TestDoc, format: export::textflow::FlowFormat, name: &str) -> Vec<String> {
    let path = out_dir().join(name);
    let events = run_job(|sink| {
        pagejob::text_flow(
            engine(),
            sink,
            doc.doc_id.clone(),
            vec![0, 1],
            format,
            path.clone(),
            "분기 보고서".into(),
        )
    });
    outputs(&events)
}

fn unzip(path: &str, entry: &str) -> String {
    use std::io::Read;
    let file = std::fs::File::open(path).expect("zip");
    let mut archive = zip::ZipArchive::new(file).expect("a zip archive");
    let mut out = String::new();
    archive
        .by_name(entry)
        .unwrap_or_else(|_| panic!("{entry} in {path}"))
        .read_to_string(&mut out)
        .expect("utf-8");
    out
}

/// X6: the DOCX is a zip whose `word/document.xml` holds the Korean text, the headings as
/// Heading styles and the images as media parts.
#[test]
fn text_flow_docx() {
    use export::textflow::FlowFormat;
    let doc = korean_flow_doc();
    let out = flow(&doc, FlowFormat::Docx, "flow.docx");
    assert_eq!(out.len(), 1);
    let xml = unzip(&out[0], "word/document.xml");
    assert!(xml.starts_with("<?xml"));
    assert!(xml.contains("첫째 문단입니다. 한글 본문이 여러 줄에 걸쳐 이어지고 문단으로 묶여야 합니다. 세 번째 줄까지 같은 문단입니다."), "{xml}");
    assert!(xml.contains(
        r#"<w:pStyle w:val="Heading1"/></w:pPr><w:r><w:t xml:space="preserve">분기 보고서"#
    ));
    assert!(xml.contains(
        r#"<w:pStyle w:val="Heading2"/></w:pPr><w:r><w:t xml:space="preserve">두 번째 절"#
    ));
    assert_eq!(xml.matches("<w:drawing>").count(), 3, "three images");
    // Reading order: the first image comes before the heading below it.
    assert!(xml.find("rIdImg1").unwrap() < xml.find("두 번째 절").unwrap());
    let rels = unzip(&out[0], "word/_rels/document.xml.rels");
    assert!(rels.contains("media/image3.png"));
    assert!(unzip(&out[0], "[Content_Types].xml").contains("wordprocessingml.document.main+xml"));
    assert!(unzip(&out[0], "word/styles.xml").contains(r#"w:styleId="Heading1""#));
}

/// X6: the HWPX has `mimetype` first and stored, and `Contents/section0.xml` holds the text.
#[test]
fn text_flow_hwpx() {
    use export::textflow::FlowFormat;
    let doc = korean_flow_doc();
    let out = flow(&doc, FlowFormat::Hwpx, "flow.hwpx");
    let file = std::fs::File::open(&out[0]).unwrap();
    let mut archive = zip::ZipArchive::new(file).unwrap();
    {
        let first = archive.by_index(0).unwrap();
        assert_eq!(first.name(), "mimetype");
        assert_eq!(first.compression(), zip::CompressionMethod::Stored);
    }
    assert_eq!(unzip(&out[0], "mimetype"), "application/hwp+zip");
    let sec = unzip(&out[0], "Contents/section0.xml");
    assert!(sec.contains("<hp:t>분기 보고서</hp:t>"), "{sec}");
    assert!(sec.contains("첫째 문단입니다."));
    assert!(
        sec.contains("<hp:secPr"),
        "the first paragraph carries the page setup"
    );
    assert!(unzip(&out[0], "Contents/content.hpf").contains("section0.xml"));
    assert!(unzip(&out[0], "Contents/header.xml").contains("<hh:charPr id=\"1\""));
    assert!(unzip(&out[0], "META-INF/container.xml").contains("Contents/content.hpf"));
}

/// X6: HTML and Markdown carry the headings, the paragraphs and the images.
#[test]
fn text_flow_html_and_markdown() {
    use export::textflow::FlowFormat;
    let doc = korean_flow_doc();
    let html = flow(&doc, FlowFormat::Html, "flow.html");
    let page = std::fs::read_to_string(&html[0]).unwrap();
    assert!(page.contains("<h1>분기 보고서</h1>"), "{page}");
    assert!(page.contains("<h2>두 번째 절</h2>"));
    assert!(page.contains("<p>그림 아래의 문단입니다.</p>"));
    assert_eq!(page.matches("data:image/png;base64,").count(), 3);

    let md = flow(&doc, FlowFormat::Md, "flow.md");
    assert_eq!(md.len(), 4, "the .md and three images: {md:?}");
    let text = std::fs::read_to_string(&md[0]).unwrap();
    assert!(text.contains("# 분기 보고서\n"), "{text}");
    assert!(text.contains("## 두 번째 절\n"));
    assert!(text.contains("![](flow_images/image-1.png)"));
    assert!(PathBuf::from(&md[1]).is_file());
}
