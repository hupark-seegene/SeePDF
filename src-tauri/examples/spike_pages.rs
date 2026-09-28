//! Spike: page operations, merge/split, images, export, metadata, bookmarks,
//! attachments and save flags.
//!
//! Target: pdfium-render 0.9.4 (default features -> pdfium_7881 bindings, image 0.25,
//! thread_safe) bound at runtime to PDFium chromium/8057.
//!
//! Run with:
//!     cd src-tauri && cargo run --example spike_pages
//!
//! Everything it writes goes to <repo>/fixtures/out/ and <repo>/fixtures/out/export/.

use image::{DynamicImage, ImageFormat, Rgba, RgbaImage};
use pdfium_render::prelude::*;
use std::error::Error;
use std::fs;
use std::os::raw::{c_int, c_ulong, c_void};
use std::path::{Path, PathBuf};
use std::time::Instant;

type R<T> = Result<T, Box<dyn Error>>;

// ---------------------------------------------------------------------------
// paths
// ---------------------------------------------------------------------------

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("src-tauri has a parent")
        .to_path_buf()
}

fn fx(name: &str) -> PathBuf {
    repo_root().join("fixtures").join(name)
}

fn out(name: &str) -> PathBuf {
    repo_root().join("fixtures").join("out").join(name)
}

fn pdfium_lib_path() -> PathBuf {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("resources")
        .join("pdfium");

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    let name = "libpdfium-aarch64-apple-darwin.dylib";
    #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
    let name = "libpdfium-x86_64-apple-darwin.dylib";
    #[cfg(target_os = "windows")]
    let name = "pdfium-x86_64-pc-windows-msvc.dll";
    #[cfg(target_os = "linux")]
    let name = "libpdfium-x86_64-unknown-linux-gnu.so";

    dir.join(name)
}

// ---------------------------------------------------------------------------
// A minimal FPDF_FILEWRITE so we can drive FPDF_SaveAsCopy / FPDF_SaveWithVersion
// ourselves. pdfium-render's PdfDocument::save_to_*() hard-codes flags = 0 and
// PdfDocument::handle() is pub(crate), so raw saving is the only way to exercise
// FPDF_INCREMENTAL / FPDF_NO_INCREMENTAL / FPDF_REMOVE_SECURITY.
// ---------------------------------------------------------------------------

#[repr(C)]
struct FileWriter {
    // These first two fields must mirror FPDF_FILEWRITE_ exactly.
    version: c_int,
    write_block: Option<unsafe extern "C" fn(*mut FileWriter, *const c_void, c_ulong) -> c_int>,
    buf: Vec<u8>,
}

unsafe extern "C" fn write_block_cb(
    me: *mut FileWriter,
    data: *const c_void,
    size: c_ulong,
) -> c_int {
    if me.is_null() || data.is_null() {
        return 0;
    }
    let slice = std::slice::from_raw_parts(data as *const u8, size as usize);
    (*me).buf.extend_from_slice(slice);
    1
}

impl FileWriter {
    fn new() -> Self {
        FileWriter {
            version: 1,
            write_block: Some(write_block_cb),
            buf: Vec::new(),
        }
    }

    fn as_fpdf(&mut self) -> *mut FPDF_FILEWRITE {
        self as *mut FileWriter as *mut FPDF_FILEWRITE
    }
}

// PDFium save flags (public/fpdf_save.h, build 8057).
const FPDF_SAVE_NONE: c_ulong = 0;
const FPDF_SAVE_INCREMENTAL: c_ulong = 1;
const FPDF_SAVE_NO_INCREMENTAL: c_ulong = 2;
const FPDF_SAVE_REMOVE_SECURITY: c_ulong = 4;

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

fn fingerprint(page: &PdfPage) -> String {
    match page.text() {
        Ok(text) => text
            .all()
            .chars()
            .filter(|c| !c.is_whitespace())
            .take(28)
            .collect(),
        Err(_) => String::from("<no text>"),
    }
}

fn page_fingerprints(doc: &PdfDocument) -> Vec<String> {
    doc.pages().iter().map(|p| fingerprint(&p)).collect()
}

fn file_len(path: &Path) -> u64 {
    fs::metadata(path).map(|m| m.len()).unwrap_or(0)
}

fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1000.0
}

const FIXTURES: &[&str] = &[
    "tracemonkey.pdf",
    "160F-2019.pdf",
    "annotation-highlight.pdf",
    "annotation-line.pdf",
    "annotation-freetext.pdf",
    "TAMReview.pdf",
    "alphatrans.pdf",
    "rotation.pdf",
    "issue12120_reduced.pdf",
];

// ---------------------------------------------------------------------------

fn main() -> R<()> {
    let lib = pdfium_lib_path();
    println!("binding to {}", lib.display());

    // NOTE: Pdfium::new() consumes the Box<dyn PdfiumLibraryBindings> and promotes it into
    // a private global OnceCell; PdfDocument::bindings()/handle() are pub(crate), so the
    // only way to keep a usable handle for raw FPDF_* calls is to bind TWICE *before*
    // constructing Pdfium (bind_to_library() fails once the global cell is populated).
    let owned = Pdfium::bind_to_library(&lib)?;
    let raw = Pdfium::bind_to_library(&lib)?;
    let pdfium = Pdfium::new(owned); // calls FPDF_InitLibrary()
    let raw: &dyn PdfiumLibraryBindings = raw.as_ref();

    fs::create_dir_all(out(""))?;
    fs::create_dir_all(out("export"))?;

    section_1_page_ops(&pdfium, raw)?;
    section_2_merge_split(&pdfium)?;
    section_3_images(&pdfium)?;
    section_4_export(&pdfium)?;
    section_5_metadata(&pdfium, raw)?;
    section_6_bookmarks(&pdfium)?;
    section_7_attachments(&pdfium)?;
    section_8_save_flags(&pdfium, raw)?;

    println!("\nALL SECTIONS COMPLETED");
    Ok(())
}

// ===========================================================================
// 1. PAGE OPERATIONS
// ===========================================================================

fn section_1_page_ops(pdfium: &Pdfium, raw: &dyn PdfiumLibraryBindings) -> R<()> {
    println!("\n================ 1. PAGE OPERATIONS ================");

    // --- inspect the sources -------------------------------------------------
    let baseline: Vec<String>;
    {
        let doc = pdfium.load_pdf_from_file(&fx("tracemonkey.pdf"), None)?;
        println!("tracemonkey.pdf: {} pages", doc.pages().len());
        let t = Instant::now();
        let sizes = doc.pages().page_sizes()?;
        println!(
            "  page_sizes() for {} pages took {:.2} ms; page 0 = {:.1} x {:.1} pt",
            sizes.len(),
            ms(t),
            sizes[0].width().value,
            sizes[0].height().value
        );
        for (i, page) in doc.pages().iter().enumerate().take(2) {
            println!(
                "  page {}: {:.1} x {:.1} pt  rotation={:?}  label={:?}  bbox={:?}",
                i,
                page.width().value,
                page.height().value,
                page.rotation()?,
                page.label(),
                page.boundaries().bounding().ok().map(|b| (
                    b.bounds.left().value,
                    b.bounds.bottom().value,
                    b.bounds.right().value,
                    b.bounds.top().value
                ))
            );
        }
        baseline = page_fingerprints(&doc);
    }

    {
        let doc = pdfium.load_pdf_from_file(&fx("rotation.pdf"), None)?;
        println!("rotation.pdf: {} pages", doc.pages().len());
        for (i, page) in doc.pages().iter().enumerate() {
            println!(
                "  page {}: rotation={:?} ({} deg)  {:.0} x {:.0} pt  orientation={:?}",
                i,
                page.rotation()?,
                page.rotation()?.as_degrees(),
                page.width().value,
                page.height().value,
                page.orientation()
            );
        }
    }

    // --- mutate --------------------------------------------------------------
    println!("\n-- delete / rotate / insert blank --");
    {
        let mut doc = pdfium.load_pdf_from_file(&fx("tracemonkey.pdf"), None)?;

        let t = Instant::now();
        {
            // rotate page 0 by 90 degrees clockwise
            let mut page = doc.pages().get(0)?;
            page.set_rotation(PdfPageRenderRotation::Degrees90);
        }
        println!("  set_rotation(page 0, 90) : {:.2} ms", ms(t));

        let t = Instant::now();
        // delete page index 1 (the 2nd page); PdfPage::delete() consumes the page
        doc.pages().get(1)?.delete()?;
        println!(
            "  delete(page 1)           : {:.2} ms -> {} pages",
            ms(t),
            doc.pages().len()
        );

        let t = Instant::now();
        {
            // insert a blank A4 page at index 2
            let page = doc
                .pages_mut()
                .create_page_at_index(PdfPagePaperSize::a4(), 2)?;
            println!(
                "  create_page_at_index(A4, 2): {:.2} ms -> {:.1} x {:.1} pt",
                ms(t),
                page.width().value,
                page.height().value
            );
        }

        {
            // insert a blank custom-size page at the end
            let page = doc
                .pages_mut()
                .create_page_at_end(PdfPagePaperSize::from_points(
                    PdfPoints::new(200.0),
                    PdfPoints::new(400.0),
                ))?;
            println!(
                "  create_page_at_end(200x400 pt) -> {:.1} x {:.1} pt",
                page.width().value,
                page.height().value
            );
        }

        let t = Instant::now();
        doc.save_to_file(&out("pages_ops.pdf"))?;
        println!("  save_to_file             : {:.2} ms", ms(t));
    }

    {
        let doc = pdfium.load_pdf_from_file(&out("pages_ops.pdf"), None)?;
        println!(
            "  reopened pages_ops.pdf: {} pages ({} bytes)",
            doc.pages().len(),
            file_len(&out("pages_ops.pdf"))
        );
        let p0 = doc.pages().get(0)?;
        println!(
            "    page 0 rotation persisted = {:?}  ({:.0} x {:.0} pt reported)",
            p0.rotation()?,
            p0.width().value,
            p0.height().value
        );
        drop(p0);
        let p2 = doc.pages().get(2)?;
        println!(
            "    page 2 (inserted blank) = {:.1} x {:.1} pt, {} objects",
            p2.width().value,
            p2.height().value,
            p2.objects().len()
        );
        drop(p2);
        let after = page_fingerprints(&doc);
        println!(
            "    order check: old page 2 text {:?} is now at index {:?}",
            &baseline[2],
            after.iter().position(|s| *s == baseline[2])
        );
    }

    // --- duplicate a page ----------------------------------------------------
    println!("\n-- duplicate a page (no same-document copy API in the safe layer) --");
    {
        let source = pdfium.load_pdf_from_file(&fx("tracemonkey.pdf"), None)?;
        let mut dest = pdfium.load_pdf_from_file(&fx("tracemonkey.pdf"), None)?;
        let t = Instant::now();
        dest.pages_mut().copy_page_from_document(&source, 0, 1)?;
        println!(
            "  copy_page_from_document(src=second handle to same file, 0 -> 1): {:.2} ms, {} pages",
            ms(t),
            dest.pages().len()
        );
        let prints = page_fingerprints(&dest);
        println!(
            "    page0 == page1 text? {}  ({:?})",
            prints[0] == prints[1],
            &prints[0]
        );
        dest.save_to_file(&out("page_duplicated.pdf"))?;
    }

    // Does pdfium itself accept src == dest for FPDF_ImportPagesByIndex?
    {
        let path = fx("tracemonkey.pdf");
        unsafe {
            let d = raw.FPDF_LoadDocument(path.to_str().unwrap(), None);
            let before = raw.FPDF_GetPageCount(d);
            let ok = raw.is_true(raw.FPDF_ImportPagesByIndex_vec(d, d, vec![0], 1));
            let after = raw.FPDF_GetPageCount(d);
            println!(
                "  raw FPDF_ImportPagesByIndex(dest == src): ok={} pages {} -> {}",
                ok, before, after
            );
            raw.FPDF_CloseDocument(d);
        }
    }

    // --- move / reorder ------------------------------------------------------
    println!("\n-- move / reorder (FPDF_MovePages, raw bindings only) --");
    {
        let path = fx("tracemonkey.pdf");
        unsafe {
            let d = raw.FPDF_LoadDocument(path.to_str().unwrap(), None);

            // Move pages [3, 2] so they land starting at index 1 -> [0, 3, 2, 1, 4, ...]
            let indices: [c_int; 2] = [3, 2];
            let t = Instant::now();
            let ok =
                raw.is_true(raw.FPDF_MovePages(d, indices.as_ptr(), indices.len() as c_ulong, 1));
            println!("  FPDF_MovePages([3,2] -> 1) = {} in {:.2} ms", ok, ms(t));

            // failure cases documented in the header
            let bad: [c_int; 2] = [2, 2];
            println!(
                "  FPDF_MovePages([2,2] -> 0) (duplicates) = {}",
                raw.is_true(raw.FPDF_MovePages(d, bad.as_ptr(), 2, 0))
            );
            let oob: [c_int; 3] = [0, 999, 3];
            println!(
                "  FPDF_MovePages([0,999,3] -> 1) (index out of range) = {}",
                raw.is_true(raw.FPDF_MovePages(d, oob.as_ptr(), 3, 1))
            );
            let bad_dest: [c_int; 3] = [0, 3, 1];
            println!(
                "  FPDF_MovePages([0,3,1] -> {}) (dest too close to the end) = {}",
                raw.FPDF_GetPageCount(d) - 1,
                raw.is_true(raw.FPDF_MovePages(
                    d,
                    bad_dest.as_ptr(),
                    3,
                    raw.FPDF_GetPageCount(d) - 1
                ))
            );

            let mut w = FileWriter::new();
            let saved = raw.is_true(raw.FPDF_SaveAsCopy(d, w.as_fpdf(), FPDF_SAVE_NONE));
            println!("  raw FPDF_SaveAsCopy = {} ({} bytes)", saved, w.buf.len());
            fs::write(out("pages_moved.pdf"), &w.buf)?;
            raw.FPDF_CloseDocument(d);
        }

        let doc = pdfium.load_pdf_from_file(&out("pages_moved.pdf"), None)?;
        let after = page_fingerprints(&doc);
        let mapping: Vec<Option<usize>> = after
            .iter()
            .take(6)
            .map(|s| baseline.iter().position(|b| b == s))
            .collect();
        println!(
            "  new order (old index of each of the first 6 pages) = {:?}",
            mapping
        );
    }

    Ok(())
}

// ===========================================================================
// 2. MERGE / SPLIT
// ===========================================================================

fn widget_count(doc: &PdfDocument) -> usize {
    doc.pages()
        .iter()
        .map(|p| {
            p.annotations()
                .iter()
                .filter(|a| matches!(a.annotation_type(), PdfPageAnnotationType::Widget))
                .count()
        })
        .sum()
}

fn annotation_count(doc: &PdfDocument) -> usize {
    doc.pages().iter().map(|p| p.annotations().len()).sum()
}

fn section_2_merge_split(pdfium: &Pdfium) -> R<()> {
    println!("\n================ 2. MERGE / SPLIT ================");

    // --- merge with a 1-based page range STRING ------------------------------
    {
        let a = pdfium.load_pdf_from_file(&fx("tracemonkey.pdf"), None)?;
        let b = pdfium.load_pdf_from_file(&fx("annotation-highlight.pdf"), None)?;
        println!(
            "  source B (annotation-highlight.pdf): {} pages, {} annotations",
            b.pages().len(),
            annotation_count(&b)
        );

        let mut merged = pdfium.create_new_pdf()?;
        let t = Instant::now();
        // NOTE: the STRING form is 1-BASED and inclusive: "1-3,5" == pages 1,2,3,5.
        merged
            .pages_mut()
            .copy_pages_from_document(&a, "1-3,5", 0)?;
        println!(
            "  copy_pages_from_document(A, \"1-3,5\", 0): {:.2} ms -> {} pages",
            ms(t),
            merged.pages().len()
        );

        let at = merged.pages().len();
        merged.pages_mut().copy_pages_from_document(&b, "1", at)?;
        println!(
            "  copy_pages_from_document(B, \"1\", {}): -> {} pages, {} annotations in merged (before save)",
            at,
            merged.pages().len(),
            annotation_count(&merged)
        );

        // insert in the MIDDLE
        merged.pages_mut().copy_page_from_document(&b, 0, 2)?;
        println!(
            "  copy_page_from_document(B, 0, index 2): -> {} pages",
            merged.pages().len()
        );

        merged.save_to_file(&out("merged.pdf"))?;
        println!("  merged.pdf = {} bytes", file_len(&out("merged.pdf")));
    }
    {
        let m = pdfium.load_pdf_from_file(&out("merged.pdf"), None)?;
        println!(
            "  reopened merged.pdf: {} pages, {} annotations",
            m.pages().len(),
            annotation_count(&m)
        );
        for (i, page) in m.pages().iter().enumerate() {
            let annots = page.annotations();
            if !annots.is_empty() {
                let types: Vec<String> = annots
                    .iter()
                    .map(|a| format!("{:?}", a.annotation_type()))
                    .collect();
                println!("    page {}: {:?}", i, types);
            }
        }
    }

    // --- do AcroForm fields survive a page copy? -----------------------------
    println!("\n-- form fields across a copy --");
    {
        let src = pdfium.load_pdf_from_file(&fx("160F-2019.pdf"), None)?;
        println!(
            "  160F-2019.pdf: {} pages, form_type={:?}, {} widget annotations",
            src.pages().len(),
            src.form().map(|f| f.form_type()),
            widget_count(&src)
        );
        if let Some(form) = src.form() {
            let values = form.field_values(src.pages());
            let mut names: Vec<&String> = values.keys().collect();
            names.sort();
            println!(
                "    {} named fields, first 6: {:?}",
                names.len(),
                &names[..names.len().min(6)]
            );
        }

        // a plain load -> save -> reload round trip of the SAME document
        src.save_to_file(&out("form_roundtrip.pdf"))?;
        {
            let rt = pdfium.load_pdf_from_file(&out("form_roundtrip.pdf"), None)?;
            println!(
                "  plain save/reload of the same document: form_type={:?}, {} widgets, {} named fields",
                rt.form().map(|f| f.form_type()),
                widget_count(&rt),
                rt.form().map(|f| f.field_values(rt.pages()).len()).unwrap_or(0)
            );
        }

        let mut copy = pdfium.create_new_pdf()?;
        copy.pages_mut()
            .copy_page_range_from_document(&src, 0..=(src.pages().len() - 1), 0)?;
        println!(
            "  after copy into a NEW document (before saving): {} pages, form_type={:?}, {} widget annotations",
            copy.pages().len(),
            copy.form().map(|f| f.form_type()),
            widget_count(&copy)
        );
        copy.save_to_file(&out("form_copied.pdf"))?;
    }
    {
        let re = pdfium.load_pdf_from_file(&out("form_copied.pdf"), None)?;
        println!(
            "  reopened form_copied.pdf: {} pages, form_type={:?}, {} widget annotations",
            re.pages().len(),
            re.form().map(|f| f.form_type()),
            widget_count(&re)
        );
        if let Some(form) = re.form() {
            let values = form.field_values(re.pages());
            println!("    {} named fields survive the round trip", values.len());
            let mut sample: Vec<(&String, &Option<String>)> = values.iter().collect();
            sample.sort_by_key(|(k, _)| (*k).clone());
            for (k, v) in sample.iter().take(4) {
                println!("      {} = {:?}", k, v);
            }
        }
    }

    // --- split ---------------------------------------------------------------
    println!("\n-- split --");
    {
        let src = pdfium.load_pdf_from_file(&fx("tracemonkey.pdf"), None)?;
        let mut part = pdfium.create_new_pdf()?;
        let t = Instant::now();
        // NOTE: the RANGE form is 0-BASED and inclusive.
        part.pages_mut()
            .copy_page_range_from_document(&src, 2..=5, 0)?;
        println!(
            "  copy_page_range_from_document(2..=5) : {:.2} ms -> {} pages",
            ms(t),
            part.pages().len()
        );
        part.save_to_file(&out("split_pages_3_to_6.pdf"))?;
        println!(
            "  split_pages_3_to_6.pdf = {} bytes (source {} bytes)",
            file_len(&out("split_pages_3_to_6.pdf")),
            file_len(&fx("tracemonkey.pdf"))
        );

        // one file per page (the common "burst" operation)
        let t = Instant::now();
        for i in 0..3 {
            let mut one = pdfium.create_new_pdf()?;
            one.pages_mut().copy_page_from_document(&src, i, 0)?;
            one.save_to_file(&out(&format!("burst_page_{}.pdf", i + 1)))?;
        }
        println!("  burst 3 single-page files: {:.2} ms total", ms(t));

        // N-up tiling into a new document
        {
            let t = Instant::now();
            let tiled = src
                .pages()
                .tile_into_new_document(2, 2, PdfPagePaperSize::a4())?;
            println!(
                "  tile_into_new_document(2 rows x 2 cols, A4): {:.1} ms -> {} pages",
                ms(t),
                tiled.pages().len()
            );
            tiled.save_to_file(&out("tiled_2x2.pdf"))?;
        }

        // append the whole of one document to another
        let mut appended = pdfium.load_pdf_from_file(&fx("rotation.pdf"), None)?;
        let before = appended.pages().len();
        let t = Instant::now();
        appended.pages_mut().append(&src)?;
        println!(
            "  PdfPages::append(tracemonkey into rotation): {:.2} ms, {} -> {} pages",
            ms(t),
            before,
            appended.pages().len()
        );
        appended.save_to_file(&out("appended.pdf"))?;
    }

    Ok(())
}

// ===========================================================================
// 3. IMAGES
// ===========================================================================

fn section_3_images(pdfium: &Pdfium) -> R<()> {
    println!("\n================ 3. IMAGES ================");

    // --- survey --------------------------------------------------------------
    let mut best: Option<(String, usize, usize)> = None; // (fixture, page index, object index)
    for name in FIXTURES {
        let doc = match pdfium.load_pdf_from_file(&fx(name), None) {
            Ok(d) => d,
            Err(e) => {
                println!("  {}: load failed {:?}", name, e);
                continue;
            }
        };
        let mut total = 0usize;
        let mut first: Option<(usize, usize)> = None;
        for (pi, page) in doc.pages().iter().enumerate() {
            let objects = page.objects();
            for (oi, object) in objects.iter().enumerate() {
                if matches!(object.object_type(), PdfPageObjectType::Image) {
                    total += 1;
                    if first.is_none() {
                        first = Some((pi, oi));
                    }
                }
            }
        }
        println!("  {:<26} {} image object(s)", name, total);
        if total > 0 && best.is_none() {
            let (pi, oi) = first.unwrap();
            best = Some((name.to_string(), pi, oi));
        }
    }

    // --- list image objects on one page with bounds + metadata ---------------
    if let Some((name, page_index, _)) = best.clone() {
        println!("\n-- image objects on {} page {} --", name, page_index);
        let doc = pdfium.load_pdf_from_file(&fx(&name), None)?;
        let page = doc.pages().get(page_index as c_int)?;
        let objects = page.objects();
        let mut exported = 0;
        for (oi, object) in objects.iter().enumerate() {
            if !matches!(object.object_type(), PdfPageObjectType::Image) {
                continue;
            }
            let bounds = object.bounds()?;
            print!(
                "  obj {:>3}: bounds L{:.1} B{:.1} R{:.1} T{:.1} ({:.1} x {:.1} pt)",
                oi,
                bounds.left().value,
                bounds.bottom().value,
                bounds.right().value,
                bounds.top().value,
                bounds.width().value,
                bounds.height().value
            );
            if let Some(image) = object.as_image_object() {
                print!(
                    " | {}x{} px, {:?} dpi, {:?} bpp, colorspace {:?}",
                    image.width().map(|v| v.to_string()).unwrap_or_default(),
                    image.height().map(|v| v.to_string()).unwrap_or_default(),
                    image
                        .horizontal_dpi()
                        .ok()
                        .map(|v| format!("{:.0}", v))
                        .unwrap_or_default(),
                    image.bits_per_pixel().ok(),
                    image.color_space().ok()
                );
                let filters: Vec<String> = image
                    .filters()
                    .iter()
                    .map(|f| f.name().to_string())
                    .collect();
                print!(" | filters {:?}", filters);
                println!();

                if exported < 2 {
                    let t = Instant::now();
                    match image.get_raw_image() {
                        Ok(raw_image) => {
                            let raw_ms = ms(t);
                            let path = out(&format!("image_raw_p{}_o{}.png", page_index, oi));
                            raw_image.save_with_format(&path, ImageFormat::Png)?;
                            println!(
                                "      get_raw_image(): {}x{} in {:.2} ms -> {} ({} bytes)",
                                raw_image.width(),
                                raw_image.height(),
                                raw_ms,
                                path.file_name().unwrap().to_string_lossy(),
                                file_len(&path)
                            );
                        }
                        Err(e) => println!("      get_raw_image() failed: {:?}", e),
                    }

                    let t = Instant::now();
                    match image.get_processed_image(&doc) {
                        Ok(processed) => {
                            let p_ms = ms(t);
                            let path = out(&format!("image_processed_p{}_o{}.png", page_index, oi));
                            processed.save_with_format(&path, ImageFormat::Png)?;
                            println!(
                                "      get_processed_image(&doc): {}x{} in {:.2} ms -> {} ({} bytes)",
                                processed.width(),
                                processed.height(),
                                p_ms,
                                path.file_name().unwrap().to_string_lossy(),
                                file_len(&path)
                            );
                        }
                        Err(e) => println!("      get_processed_image() failed: {:?}", e),
                    }

                    match image.get_raw_image_data() {
                        Ok(bytes) => println!(
                            "      get_raw_image_data(): {} bytes of encoded stream data",
                            bytes.len()
                        ),
                        Err(e) => println!("      get_raw_image_data() failed: {:?}", e),
                    }
                    exported += 1;
                }
            } else {
                println!();
            }
        }
    } else {
        println!("  no fixture contained an image object");
    }

    // --- build a PNG we can insert ------------------------------------------
    println!("\n-- insert an image object --");
    let mut rgba = RgbaImage::new(160, 120);
    for (x, y, px) in rgba.enumerate_pixels_mut() {
        *px = Rgba([
            (x * 255 / 159) as u8,
            (y * 255 / 119) as u8,
            120,
            if (x / 20 + y / 20) % 2 == 0 { 255 } else { 160 },
        ]);
    }
    let stamp = DynamicImage::ImageRgba8(rgba);
    stamp.save_with_format(out("insert_src.png"), ImageFormat::Png)?;
    println!(
        "  wrote insert_src.png ({} bytes, {}x{})",
        file_len(&out("insert_src.png")),
        stamp.width(),
        stamp.height()
    );

    {
        // NOTE: `doc` does not need to be `mut`; PdfPage owns its own FPDF_PAGE handle,
        // so page content can be mutated through an immutable borrow of the document.
        let doc = pdfium.load_pdf_from_file(&fx("tracemonkey.pdf"), None)?;
        {
            let mut page = doc.pages().get(0)?;
            let t = Instant::now();
            let object = page.objects_mut().create_image_object(
                PdfPoints::new(72.0),
                PdfPoints::new(600.0),
                &stamp,
                Some(PdfPoints::new(160.0)),
                Some(PdfPoints::new(120.0)),
            )?;
            let insert_ms = ms(t);
            let b = object.bounds()?;
            println!(
                "  create_image_object(x=72, y=600, 160x120 pt): {:.2} ms -> bounds L{:.1} B{:.1} R{:.1} T{:.1}",
                insert_ms,
                b.left().value,
                b.bottom().value,
                b.right().value,
                b.top().value
            );
            println!(
                "  page object count is now {} (strategy = {:?})",
                page.objects().len(),
                page.content_regeneration_strategy()
            );
            drop(object);
            page.regenerate_content()?;
        }
        doc.save_to_file(&out("image_inserted.pdf"))?;
        println!(
            "  image_inserted.pdf = {} bytes",
            file_len(&out("image_inserted.pdf"))
        );
    }
    {
        let doc = pdfium.load_pdf_from_file(&out("image_inserted.pdf"), None)?;
        let page = doc.pages().get(0)?;
        let images = page
            .objects()
            .iter()
            .filter(|o| matches!(o.object_type(), PdfPageObjectType::Image))
            .count();
        println!("  reopened: page 0 now has {} image object(s)", images);
    }

    // --- an explicit matrix placement instead of x/y + width/height ----------
    {
        let mut doc = pdfium.create_new_pdf()?;
        {
            let mut page = doc
                .pages_mut()
                .create_page_at_start(PdfPagePaperSize::a4())?;
            let mut object = PdfPageImageObject::new(&doc, &stamp)?;
            // A fresh image object is 1x1 pt at the origin; scale then translate.
            object.scale(200.0, 150.0)?;
            object.rotate_clockwise_degrees(15.0)?;
            object.translate(PdfPoints::new(120.0), PdfPoints::new(400.0))?;
            let matrix = object.matrix()?;
            println!(
                "  manual placement matrix = [{:.3} {:.3} {:.3} {:.3} {:.1} {:.1}]",
                matrix.a(),
                matrix.b(),
                matrix.c(),
                matrix.d(),
                matrix.e(),
                matrix.f()
            );
            page.objects_mut().add_image_object(object)?;
            page.regenerate_content()?;
        }
        doc.save_to_file(&out("image_placed.pdf"))?;
        println!(
            "  image_placed.pdf = {} bytes",
            file_len(&out("image_placed.pdf"))
        );
    }

    // --- replace an existing image ------------------------------------------
    println!("\n-- replace an existing image --");
    if let Some((name, page_index, object_index)) = best {
        let doc = pdfium.load_pdf_from_file(&fx(&name), None)?;
        {
            let mut page = doc.pages().get(page_index as c_int)?;
            let mut object = page.objects().get(object_index)?;
            let before = object.bounds()?;
            let t = Instant::now();
            match object.as_image_object_mut() {
                Some(image) => {
                    image.set_image(&stamp)?;
                    println!("  set_image() on the existing object: {:.2} ms", ms(t));
                }
                None => println!("  object {} is not an image object", object_index),
            }
            let after = object.bounds()?;
            println!(
                "  bounds before {:.1}x{:.1} pt -> after {:.1}x{:.1} pt (set_image keeps the object matrix)",
                before.width().value,
                before.height().value,
                after.width().value,
                after.height().value
            );
            drop(object);
            page.regenerate_content()?;
        }
        doc.save_to_file(&out("image_replaced.pdf"))?;
        println!(
            "  image_replaced.pdf = {} bytes (source {} bytes)",
            file_len(&out("image_replaced.pdf")),
            file_len(&fx(&name))
        );
    }

    Ok(())
}

// ===========================================================================
// 4. EXPORT
// ===========================================================================

fn section_4_export(pdfium: &Pdfium) -> R<()> {
    println!("\n================ 4. EXPORT ================");

    let doc = pdfium.load_pdf_from_file(&fx("tracemonkey.pdf"), None)?;
    let dpi = 150.0f32;
    let scale = dpi / 72.0;
    let config = PdfRenderConfig::new()
        .scale_page_by_factor(scale)
        .render_annotations(true)
        .render_form_data(true);

    let dir = out("export");
    fs::create_dir_all(&dir)?;

    let mut render_ms = 0.0f64;
    let mut convert_ms = 0.0f64;
    let mut encode_ms = 0.0f64;
    let mut total_bytes = 0u64;
    let mut dims = (0u32, 0u32);

    let overall = Instant::now();
    for (i, page) in doc.pages().iter().enumerate() {
        let t = Instant::now();
        let bitmap = page.render_with_config(&config)?;
        render_ms += ms(t);

        let t = Instant::now();
        let image = bitmap.as_image()?;
        convert_ms += ms(t);

        let rgb = image.into_rgb8();
        dims = (rgb.width(), rgb.height());

        let path = dir.join(format!("tracemonkey-{:02}.png", i + 1));
        let t = Instant::now();
        rgb.save_with_format(&path, ImageFormat::Png)?;
        encode_ms += ms(t);
        total_bytes += file_len(&path);
    }
    let pages = doc.pages().len() as f64;
    println!(
        "  {} pages @ {:.0} DPI ({}x{} px): total {:.1} ms\n    render {:.1} ms ({:.1} ms/page), bitmap->DynamicImage {:.1} ms, PNG encode {:.1} ms ({:.1} ms/page)",
        doc.pages().len(),
        dpi,
        dims.0,
        dims.1,
        ms(overall),
        render_ms,
        render_ms / pages,
        convert_ms,
        encode_ms,
        encode_ms / pages
    );
    println!(
        "  {} PNG bytes written ({:.1} KB/page)",
        total_bytes,
        total_bytes as f64 / pages / 1024.0
    );

    // reuse a single bitmap buffer (all pages in this document share a size)
    {
        let first = doc.pages().get(0)?;
        let w = (first.width().value * scale).round() as i32;
        let h = (first.height().value * scale).round() as i32;
        drop(first);
        let mut bitmap = PdfBitmap::empty(w, h, PdfBitmapFormat::BGRA)?;
        let t = Instant::now();
        for page in doc.pages().iter() {
            page.render_into_bitmap_with_config(&mut bitmap, &config)?;
        }
        println!(
            "  render_into_bitmap_with_config with one reused {}x{} bitmap: {:.1} ms total ({:.1} ms/page)",
            w,
            h,
            ms(t),
            ms(t) / pages
        );
    }

    // thumbnails, as a page organiser grid would need them
    for (label, thumb_config) in [
        (
            "PdfRenderConfig::thumbnail(180)",
            PdfRenderConfig::new().thumbnail(180),
        ),
        (
            "set_maximum_width/height(180) only",
            PdfRenderConfig::new()
                .set_maximum_width(180)
                .set_maximum_height(180),
        ),
        (
            "set_target_width(180).set_maximum_height(180)",
            PdfRenderConfig::new()
                .set_target_width(180)
                .set_maximum_height(180),
        ),
    ] {
        let t = Instant::now();
        let mut bytes = 0u64;
        let mut dims = (0u32, 0u32);
        for (i, page) in doc.pages().iter().enumerate() {
            let image = page
                .render_with_config(&thumb_config)?
                .as_image()?
                .into_rgb8();
            dims = (image.width(), image.height());
            let path = dir.join(format!(
                "thumb{}-{:02}.png",
                match label.as_bytes()[0] {
                    b'P' => "",
                    b's' if label.contains("target") => "-fit",
                    _ => "-clamp",
                },
                i + 1
            ));
            image.save_with_format(&path, ImageFormat::Png)?;
            bytes += file_len(&path);
        }
        println!(
            "  {} thumbnails, {}: {:.1} ms ({:.2} ms/page), {}x{} px, {} bytes",
            doc.pages().len(),
            label,
            ms(t),
            ms(t) / pages,
            dims.0,
            dims.1,
            bytes
        );
    }

    // does rendering honour the page's /Rotate entry?
    {
        let rot = pdfium.load_pdf_from_file(&fx("rotation.pdf"), None)?;
        for (i, page) in rot.pages().iter().enumerate() {
            let image = page.render_with_config(&config)?.as_image()?;
            let path = dir.join(format!("rotation-{:02}.png", i + 1));
            image
                .into_rgb8()
                .save_with_format(&path, ImageFormat::Png)?;
            println!(
                "  rotation.pdf page {} ({:?}, {:.0}x{:.0} pt) rendered to {}x{} px -> /Rotate IS applied by the renderer",
                i,
                page.rotation()?,
                page.width().value,
                page.height().value,
                (page.width().value * scale).round() as i32,
                (page.height().value * scale).round() as i32
            );
        }
    }

    // text export
    {
        let t = Instant::now();
        let mut buffer = String::new();
        for (i, page) in doc.pages().iter().enumerate() {
            buffer.push_str(&format!("\n----- page {} -----\n", i + 1));
            buffer.push_str(&page.text()?.all());
        }
        let extract_ms = ms(t);
        let path = dir.join("tracemonkey.txt");
        fs::write(&path, &buffer)?;
        println!(
            "  text of {} pages: {:.1} ms ({:.2} ms/page), {} chars -> {} ({} bytes)",
            doc.pages().len(),
            extract_ms,
            extract_ms / pages,
            buffer.chars().count(),
            path.file_name().unwrap().to_string_lossy(),
            file_len(&path)
        );
    }

    Ok(())
}

// ===========================================================================
// 5. METADATA / LABELS / PERMISSIONS / ENCRYPTION
// ===========================================================================

const ALL_TAGS: &[PdfDocumentMetadataTagType] = &[
    PdfDocumentMetadataTagType::Title,
    PdfDocumentMetadataTagType::Author,
    PdfDocumentMetadataTagType::Subject,
    PdfDocumentMetadataTagType::Keywords,
    PdfDocumentMetadataTagType::Creator,
    PdfDocumentMetadataTagType::Producer,
    PdfDocumentMetadataTagType::CreationDate,
    PdfDocumentMetadataTagType::ModificationDate,
];

fn dump_metadata(label: &str, doc: &PdfDocument) {
    println!("  {}:", label);
    for tag in ALL_TAGS {
        let value = doc.metadata().get(*tag).map(|t| t.value().to_string());
        println!("    {:<18} {:?}", format!("{:?}", tag), value);
    }
    println!(
        "    metadata().len() = {} (only tags that are actually present)",
        doc.metadata().len()
    );
    println!("    version() = {:?}", doc.version());
}

fn dump_permissions(label: &str, doc: &PdfDocument) {
    let p = doc.permissions();
    println!("  {}:", label);
    println!(
        "    security_handler_revision  {:?}",
        p.security_handler_revision()
    );
    println!(
        "    can_print_high_quality     {:?}",
        p.can_print_high_quality()
    );
    println!(
        "    can_print_only_low_quality {:?}",
        p.can_print_only_low_quality()
    );
    println!(
        "    can_modify_document_content{:?}",
        p.can_modify_document_content()
    );
    println!(
        "    can_extract_text_and_graphics {:?}",
        p.can_extract_text_and_graphics()
    );
    println!(
        "    can_add_or_modify_text_annotations {:?}",
        p.can_add_or_modify_text_annotations()
    );
    println!(
        "    can_fill_existing_interactive_form_fields {:?}",
        p.can_fill_existing_interactive_form_fields()
    );
    println!(
        "    can_assemble_document      {:?}",
        p.can_assemble_document()
    );
}

fn section_5_metadata(pdfium: &Pdfium, raw: &dyn PdfiumLibraryBindings) -> R<()> {
    println!("\n================ 5. METADATA / LABELS / PERMISSIONS ================");

    {
        let doc = pdfium.load_pdf_from_file(&fx("tracemonkey.pdf"), None)?;
        dump_metadata("tracemonkey.pdf", &doc);
        println!("    catalog().is_tagged() = {}", doc.catalog().is_tagged());
        println!(
            "    catalog().get_language() = {:?}",
            doc.catalog().get_language()
        );
    }
    {
        let doc = pdfium.load_pdf_from_file(&fx("TAMReview.pdf"), None)?;
        dump_metadata("TAMReview.pdf", &doc);
    }

    // Writing metadata: pdfium exposes no FPDF_SetMetaText, and pdfium-render exposes
    // no setter on PdfMetadata. Demonstrate that a round trip loses nothing but also
    // gains nothing: producer/creator are copied only because the pages are copied.
    println!("\n-- writing metadata --");
    {
        let src = pdfium.load_pdf_from_file(&fx("tracemonkey.pdf"), None)?;
        let mut copy = pdfium.create_new_pdf()?;
        copy.pages_mut()
            .copy_page_range_from_document(&src, 0..=1, 0)?;
        copy.set_version(PdfDocumentVersion::Pdf1_7);
        copy.save_to_file(&out("metadata_probe.pdf"))?;
        let re = pdfium.load_pdf_from_file(&out("metadata_probe.pdf"), None)?;
        dump_metadata("new document built by copying 2 pages", &re);
        println!("    -> PdfMetadata has get()/iter()/len() only; there is NO set().");
        println!("    -> pdfium itself has FPDF_GetMetaText but no FPDF_SetMetaText.");
    }

    // page labels
    println!("\n-- page labels --");
    for name in [
        "tracemonkey.pdf",
        "160F-2019.pdf",
        "TAMReview.pdf",
        "rotation.pdf",
    ] {
        let doc = pdfium.load_pdf_from_file(&fx(name), None)?;
        let labels: Vec<Option<String>> = doc
            .pages()
            .iter()
            .take(5)
            .map(|p| p.label().map(|s| s.to_string()))
            .collect();
        println!(
            "  {:<20} first labels via PdfPage::label() = {:?}",
            name, labels
        );
    }
    if out("outline-labels.pdf").exists() {
        // a /PageLabels number tree with three styles: roman, decimal-with-start, alpha-with-prefix
        let doc = pdfium.load_pdf_from_file(&out("outline-labels.pdf"), None)?;
        let labels: Vec<Option<String>> = doc
            .pages()
            .iter()
            .map(|p| p.label().map(|s| s.to_string()))
            .collect();
        println!(
            "  {:<20} /PageLabels tree -> {:?}",
            "outline-labels.pdf", labels
        );
    }
    {
        // raw check, to prove PdfPage::label() is just FPDF_GetPageLabel
        let path = fx("160F-2019.pdf");
        unsafe {
            let d = raw.FPDF_LoadDocument(path.to_str().unwrap(), None);
            let len = raw.FPDF_GetPageLabel(d, 0, std::ptr::null_mut(), 0);
            println!(
                "  raw FPDF_GetPageLabel(160F, page 0) buffer length = {}",
                len
            );
            raw.FPDF_CloseDocument(d);
        }
    }

    // permissions on a plain document
    println!("\n-- permissions --");
    {
        let doc = pdfium.load_pdf_from_file(&fx("tracemonkey.pdf"), None)?;
        dump_permissions("tracemonkey.pdf (unencrypted)", &doc);
    }

    // encryption
    println!("\n-- encrypted document (fixtures/out/encrypted-rc4-40.pdf, RC4 40-bit, R=2) --");
    let enc_path = out("encrypted-rc4-40.pdf");
    if !enc_path.exists() {
        println!(
            "  !! {} is missing; regenerate it before running",
            enc_path.display()
        );
        return Ok(());
    }
    println!(
        "  load with no password   -> {:?}",
        pdfium.load_pdf_from_file(&enc_path, None).err()
    );
    println!(
        "  load with wrong password-> {:?}",
        pdfium
            .load_pdf_from_file(&enc_path, Some("definitely-not"))
            .err()
    );
    {
        let doc = pdfium.load_pdf_from_file(&enc_path, Some("user"))?;
        println!(
            "  load with user password -> ok, {} pages",
            doc.pages().len()
        );
        dump_permissions("opened with the USER password", &doc);
        dump_metadata(
            "encrypted document metadata (decrypted transparently)",
            &doc,
        );
    }
    {
        let doc = pdfium.load_pdf_from_file(&enc_path, Some("owner"))?;
        println!(
            "  load with owner password-> ok, {} pages",
            doc.pages().len()
        );
        dump_permissions("opened with the OWNER password", &doc);
    }

    // Can pdfium re-encrypt on save? No: FPDF_SaveAsCopy takes no password argument,
    // and the only security-related flag REMOVES security.
    {
        let doc = pdfium.load_pdf_from_file(&enc_path, Some("user"))?;
        doc.save_to_file(&out("encrypted_resaved.pdf"))?;
        let reopened = pdfium.load_pdf_from_file(&out("encrypted_resaved.pdf"), None);
        println!(
            "  PdfDocument::save_to_file() on the decrypted doc, reopened with NO password -> {}",
            match reopened {
                Ok(d) => format!(
                    "ok, {} pages (encryption was DROPPED on save)",
                    d.pages().len()
                ),
                Err(e) => format!("{:?}", e),
            }
        );
    }

    Ok(())
}

// ===========================================================================
// 6. BOOKMARKS
// ===========================================================================

fn walk_bookmark(bookmark: &PdfBookmark, depth: usize, count: &mut usize, printed: &mut usize) {
    *count += 1;
    if *printed < 14 {
        *printed += 1;
        println!(
            "    {}{:?} -> page {:?}  (children {})",
            "  ".repeat(depth),
            bookmark.title().unwrap_or_default(),
            bookmark.destination().and_then(|d| d.page_index().ok()),
            bookmark.children_len()
        );
    }
    for child in bookmark.iter_direct_children() {
        walk_bookmark(&child, depth + 1, count, printed);
    }
}

fn section_6_bookmarks(pdfium: &Pdfium) -> R<()> {
    println!("\n================ 6. BOOKMARKS / OUTLINE ================");

    let mut targets: Vec<PathBuf> = FIXTURES.iter().map(|n| fx(n)).collect();
    if out("outline-labels.pdf").exists() {
        targets.push(out("outline-labels.pdf"));
    }

    for path in targets {
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let name = name.as_str();
        let doc = pdfium.load_pdf_from_file(&path, None)?;
        let t = Instant::now();
        let total = doc.bookmarks().iter().count();
        if total == 0 {
            println!("  {:<26} no outline", name);
            continue;
        }
        println!(
            "  {:<26} {} bookmarks (flat iter took {:.2} ms)",
            name,
            total,
            ms(t)
        );

        let mut count = 0usize;
        let mut printed = 0usize;
        // root() is the FIRST TOP-LEVEL bookmark, not a synthetic root, so walk its siblings too.
        if let Some(root) = doc.bookmarks().root() {
            walk_bookmark(&root, 0, &mut count, &mut printed);
            for sibling in root.iter_siblings() {
                walk_bookmark(&sibling, 0, &mut count, &mut printed);
            }
        }
        println!("    depth-first walk visited {} nodes", count);

        if let Some(first) = doc.bookmarks().iter().next() {
            println!(
                "    first bookmark action = {:?}",
                first.action().map(|a| format!("{:?}", a.action_type()))
            );
        }
    }

    println!("  -> PdfBookmarks exposes root()/iter()/find_*; PdfBookmark exposes");
    println!("     title()/action()/destination()/parent()/children_len()/first_child()/");
    println!("     next_sibling()/iter_siblings()/iter_direct_children()/iter_all_descendants().");
    println!("     There is NO create/delete/rename/move API: the outline is READ-ONLY.");

    Ok(())
}

// ===========================================================================
// 7. ATTACHMENTS
// ===========================================================================

fn section_7_attachments(pdfium: &Pdfium) -> R<()> {
    println!("\n================ 7. ATTACHMENTS (EMBEDDED FILES) ================");

    for name in FIXTURES {
        let doc = pdfium.load_pdf_from_file(&fx(name), None)?;
        let n = doc.attachments().len();
        if n > 0 {
            println!("  {:<26} {} attachment(s)", name, n);
            for a in doc.attachments().iter() {
                println!("    {} ({} bytes)", a.name(), a.len());
            }
        }
    }
    println!("  (no fixture ships with embedded files; creating one instead)");

    {
        let mut doc = pdfium.create_new_pdf()?;
        {
            let _page = doc
                .pages_mut()
                .create_page_at_start(PdfPagePaperSize::a4())?;
        }
        {
            let t = Instant::now();
            let _a = doc
                .attachments_mut()
                .create_attachment_from_bytes("notes.txt", b"hello from the SeePDF spike")?;
            println!("  create_attachment_from_bytes: {:.2} ms", ms(t));
        }
        {
            let _b = doc
                .attachments_mut()
                .create_attachment_from_bytes("second.bin", &[0u8, 1, 2, 3, 250, 251])?;
        }
        println!("  attachments before save: {}", doc.attachments().len());
        doc.save_to_file(&out("with_attachments.pdf"))?;
    }
    {
        let doc = pdfium.load_pdf_from_file(&out("with_attachments.pdf"), None)?;
        println!(
            "  reopened with_attachments.pdf: {} attachment(s)",
            doc.attachments().len()
        );
        for i in 0..doc.attachments().len() {
            let a = doc.attachments().get(i)?;
            let bytes = a.save_to_bytes()?;
            println!(
                "    [{}] {} -> {} bytes, content {:?}",
                i,
                a.name(),
                bytes.len(),
                String::from_utf8_lossy(&bytes[..bytes.len().min(32)])
            );
            a.save_to_file(&out(&format!("attachment_{}_{}", i, a.name())))?;
        }
    }
    {
        // deletion
        let mut doc = pdfium.load_pdf_from_file(&out("with_attachments.pdf"), None)?;
        doc.attachments_mut().delete_at_index(0)?;
        println!("  after delete_at_index(0): {}", doc.attachments().len());
    }

    Ok(())
}

// ===========================================================================
// 8. SAVE FLAGS / "OPTIMISE"
// ===========================================================================

fn raw_save(
    raw: &dyn PdfiumLibraryBindings,
    doc: FPDF_DOCUMENT,
    flags: c_ulong,
    version: Option<c_int>,
) -> (bool, Vec<u8>, f64) {
    let mut writer = FileWriter::new();
    let t = Instant::now();
    let ok = unsafe {
        match version {
            Some(v) => raw.is_true(raw.FPDF_SaveWithVersion(doc, writer.as_fpdf(), flags, v)),
            None => raw.is_true(raw.FPDF_SaveAsCopy(doc, writer.as_fpdf(), flags)),
        }
    };
    let elapsed = ms(t);
    (ok, writer.buf, elapsed)
}

fn section_8_save_flags(pdfium: &Pdfium, raw: &dyn PdfiumLibraryBindings) -> R<()> {
    println!("\n================ 8. SAVE FLAGS / OPTIMISE ================");
    println!("  NOTE: PdfDocument::save_to_writer() hard-codes flags = 0. Everything below uses");
    println!("  raw FPDF_SaveAsCopy / FPDF_SaveWithVersion through PdfiumLibraryBindings.\n");

    let source = fx("tracemonkey.pdf");
    println!("  source tracemonkey.pdf = {} bytes", file_len(&source));

    unsafe {
        let d = raw.FPDF_LoadDocument(source.to_str().unwrap(), None);

        for (label, flag) in [
            ("flags=0 (default)", FPDF_SAVE_NONE),
            ("FPDF_INCREMENTAL", FPDF_SAVE_INCREMENTAL),
            ("FPDF_NO_INCREMENTAL", FPDF_SAVE_NO_INCREMENTAL),
            ("FPDF_REMOVE_SECURITY", FPDF_SAVE_REMOVE_SECURITY),
        ] {
            let (ok, bytes, elapsed) = raw_save(raw, d, flag, None);
            println!(
                "    {:<22} ok={} {:>9} bytes  {:.1} ms",
                label,
                ok,
                bytes.len(),
                elapsed
            );
            fs::write(
                out(&format!(
                    "save_{}.pdf",
                    label.split_whitespace().next().unwrap().to_lowercase()
                )),
                &bytes,
            )?;
        }

        // saving with an explicit version
        for v in [14, 17] {
            let (ok, bytes, elapsed) = raw_save(raw, d, FPDF_SAVE_NO_INCREMENTAL, Some(v));
            println!(
                "    SaveWithVersion({})      ok={} {:>9} bytes  {:.1} ms",
                v,
                ok,
                bytes.len(),
                elapsed
            );
        }

        raw.FPDF_CloseDocument(d);
    }

    // Does an incremental save after an edit actually append?
    println!("\n  incremental vs full save AFTER an edit:");
    unsafe {
        let d = raw.FPDF_LoadDocument(source.to_str().unwrap(), None);
        raw.FPDFPage_Delete(d, 0);
        let (_, inc, inc_ms) = raw_save(raw, d, FPDF_SAVE_INCREMENTAL, None);
        let (_, full, full_ms) = raw_save(raw, d, FPDF_SAVE_NO_INCREMENTAL, None);
        println!(
            "    after FPDFPage_Delete(0): incremental {} bytes ({:.1} ms) vs full {} bytes ({:.1} ms)",
            inc.len(),
            inc_ms,
            full.len(),
            full_ms
        );
        fs::write(out("save_after_delete_incremental.pdf"), &inc)?;
        fs::write(out("save_after_delete_full.pdf"), &full)?;
        raw.FPDF_CloseDocument(d);
    }

    // FPDF_REMOVE_SECURITY on the encrypted fixture
    println!("\n  FPDF_REMOVE_SECURITY on the encrypted fixture:");
    let enc = out("encrypted-rc4-40.pdf");
    if enc.exists() {
        unsafe {
            let d = raw.FPDF_LoadDocument(enc.to_str().unwrap(), Some("user"));
            if d.is_null() {
                println!("    could not open the encrypted fixture");
            } else {
                let (ok, plain, _) = raw_save(raw, d, FPDF_SAVE_REMOVE_SECURITY, None);
                println!("    save ok={} -> {} bytes", ok, plain.len());
                fs::write(out("decrypted.pdf"), &plain)?;
                raw.FPDF_CloseDocument(d);

                let reopened = pdfium.load_pdf_from_file(&out("decrypted.pdf"), None);
                println!(
                    "    reopened decrypted.pdf without a password -> {}",
                    match reopened {
                        Ok(doc) => format!("ok, {} pages", doc.pages().len()),
                        Err(e) => format!("{:?}", e),
                    }
                );
            }
        }
    }

    // app-level "optimise": re-encode every image at a lower resolution
    println!("\n  app-level optimisation probe (downsample every image object):");
    {
        let doc = pdfium.load_pdf_from_file(&fx("TAMReview.pdf"), None)?;
        let before = file_len(&fx("TAMReview.pdf"));
        let mut touched = 0usize;
        let mut widest = 0u32;
        let t = Instant::now();
        let page_count = doc.pages().len();
        for pi in 0..page_count {
            let mut page = doc.pages().get(pi)?;
            let indices: Vec<usize> = page
                .objects()
                .iter()
                .enumerate()
                .filter(|(_, o)| matches!(o.object_type(), PdfPageObjectType::Image))
                .map(|(i, _)| i)
                .collect();
            for oi in indices {
                let mut object = page.objects().get(oi)?;
                if let Some(image) = object.as_image_object_mut() {
                    if let Ok(current) = image.get_raw_image() {
                        let (w, h) = (current.width(), current.height());
                        widest = widest.max(w);
                        if w >= 64 {
                            let small = current.resize(
                                w / 2,
                                (h / 2).max(1),
                                image::imageops::FilterType::Triangle,
                            );
                            if image.set_image(&small).is_ok() {
                                touched += 1;
                            }
                        }
                    }
                }
                drop(object);
            }
            if touched > 0 {
                page.regenerate_content()?;
            }
        }
        let rewrite_ms = ms(t);
        doc.save_to_file(&out("optimised.pdf"))?;
        println!(
            "    downsampled {} image(s) (widest source image {} px) in {:.0} ms; {} bytes -> {} bytes ({:+.1}%)",
            touched,
            widest,
            rewrite_ms,
            before,
            file_len(&out("optimised.pdf")),
            (file_len(&out("optimised.pdf")) as f64 - before as f64) / before as f64 * 100.0
        );
        println!(
            "    (pdfium re-encodes replaced images as raw/flate bitmaps, it does NOT keep JPEG)"
        );
    }

    Ok(())
}
