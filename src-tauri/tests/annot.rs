//! Stage 1 (a) — annotations.
//!
//! Every round-trip test does the full loop the DoD asks for:
//! **create → render the page (so pdfium writes `/AP`) → `save_to_bytes` → reopen → enumerate
//! → render again and count the pixels that changed inside the annotation's rect**, because
//! "the annotation is in the file" and "the annotation is visible in Preview" are different
//! claims and only the second one matters to a user.

mod common;
use common::*;

use seepdf_lib::engine::annot;
use seepdf_lib::engine::raw;
use seepdf_lib::engine::registry::{self, MutateOpts};
use seepdf_lib::engine::render::tiles;
use seepdf_lib::ipc::types::{
    Annot, AnnotKind, AnnotSpec, ChangeReason, Editability, InkSpec, LineSpec, MarkupSpec,
    NoteSpec, Rect, ShapeSpec, StampImage, StampSpec, TextAlign, TextBoxSpec,
};
use std::path::PathBuf;

// ---------------------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------------------

const RENDER_SCALE: f32 = 2.0;
/// Per-channel difference that counts as "this pixel changed".
const PIXEL_TOLERANCE: i32 = 24;

fn out_dir() -> PathBuf {
    let dir = fixture("out").join("stage1a");
    std::fs::create_dir_all(&dir).expect("create fixtures/out/stage1a");
    dir
}

fn create(doc_id: &str, page: u16, spec: AnnotSpec) -> String {
    let doc_id = doc_id.to_string();
    with_state(move |st| {
        registry::mutate(
            st,
            &doc_id,
            MutateOpts::new("undo.annotCreate", ChangeReason::Edit).page(page),
            |doc| annot::create::create(doc, page, &spec, None),
        )
    })
    .expect("create annotation")
}

fn list(doc_id: &str, page: u16) -> Vec<Annot> {
    let doc_id = doc_id.to_string();
    with_doc(&doc_id.clone(), move |doc| annot::list(doc, page)).expect("list annotations")
}

/// The pre-save appearance pass plus `save_to_bytes()` — exactly what `save_document` does.
fn save_bytes(doc_id: &str) -> Vec<u8> {
    let doc_id = doc_id.to_string();
    with_state(move |st| {
        tiles::generate_appearances(st, &doc_id)?;
        Ok(st.doc(&doc_id)?.to_bytes()?.to_vec())
    })
    .expect("save to bytes")
}

/// Opens saved bytes as a second document in the same engine.
fn reopen(bytes: Vec<u8>) -> TestDoc {
    let info = with_state(move |st| registry::open(st, None, bytes, None)).expect("reopen");
    let doc_id = info.doc_id.clone();
    TestDoc { info, doc_id }
}

/// RGBA pixels of `rect` (PDF points) at [`RENDER_SCALE`], through the one render config.
fn render_rect(doc_id: &str, page: u16, rect: Rect) -> (u32, u32, Vec<u8>) {
    let doc_id = doc_id.to_string();
    let buffer =
        with_state(move |st| tiles::render_raw_buffer(st, &doc_id, page, RENDER_SCALE, Some(rect)))
            .expect("render rect");
    let width = u32::from_le_bytes(buffer[8..12].try_into().unwrap());
    let height = u32::from_le_bytes(buffer[12..16].try_into().unwrap());
    (width, height, buffer[32..].to_vec())
}

/// How many pixels of `after` differ from `before` by more than the tolerance.
fn changed_pixels(before: &(u32, u32, Vec<u8>), after: &(u32, u32, Vec<u8>)) -> usize {
    assert_eq!(
        (before.0, before.1),
        (after.0, after.1),
        "the two renders must have the same size"
    );
    before
        .2
        .chunks_exact(4)
        .zip(after.2.chunks_exact(4))
        .filter(|(a, b)| (0..3).any(|i| (a[i] as i32 - b[i] as i32).abs() > PIXEL_TOLERANCE))
        .count()
}

/// Writes a saved document and a full-page PNG into `fixtures/out/stage1a/` so a human can
/// open them in Preview (QA-4).
fn write_artifacts(name: &str, bytes: &[u8], doc_id: &str, page: u16) {
    let dir = out_dir();
    std::fs::write(dir.join(format!("{name}.pdf")), bytes).expect("write pdf");
    let doc_id = doc_id.to_string();
    let png = with_state(move |st| {
        let raw = tiles::render(st, &tiles::RenderRequest::new(page_key(st, &doc_id, page)))?;
        seepdf_lib::engine::render::encode::encode_png(&raw)
    });
    if let Ok(png) = png {
        std::fs::write(dir.join(format!("{name}.png")), png).expect("write png");
    }
}

fn page_key(
    st: &seepdf_lib::engine::EngineState<'_>,
    doc_id: &str,
    page: u16,
) -> seepdf_lib::engine::render::cache::TileKey {
    use seepdf_lib::engine::render::cache::{Night, RenderKind, TileKey};
    let generation = st.doc(doc_id).map(|d| d.generation).unwrap_or(1);
    TileKey {
        doc: doc_id.to_string(),
        generation,
        page,
        scale_key: 100,
        rotation: 0,
        kind: RenderKind::Page,
        tx: 0,
        ty: 0,
        night: Night::Off,
        hl: false,
        forms: true,
    }
}

fn find<'a>(annots: &'a [Annot], id: &str) -> &'a Annot {
    annots
        .iter()
        .find(|a| a.id == id)
        .unwrap_or_else(|| panic!("annotation {id} is missing; got {:?}", ids(annots)))
}

fn ids(annots: &[Annot]) -> Vec<&str> {
    annots.iter().map(|a| a.id.as_str()).collect()
}

// ---------------------------------------------------------------------------------------
// raw wrappers (the surface Stage 1 (b) consumes)
// ---------------------------------------------------------------------------------------

/// `move_pages` / `import_pages_by_index` / `flatten` / `save_as_copy` are (a)'s deliverable
/// to (b): prove they are callable, validate their arguments and actually change the file.
#[test]
fn annot_raw_wrappers_smoke() {
    let doc = open("gen/outline-labels.pdf");
    let doc_id = doc.doc_id.clone();

    let (count, bad_dup, bad_range, bad_dest) = with_state({
        let doc_id = doc_id.clone();
        move |st| {
            let bindings = st.doc(&doc_id)?.bindings();
            let pdf = st.doc(&doc_id)?.pdf();
            let count = raw::page::page_count(bindings, pdf);
            let bad_dup = raw::page::move_pages(bindings, pdf, &[0, 0], 1).is_err();
            let bad_range = raw::page::move_pages(bindings, pdf, &[999], 0).is_err();
            let bad_dest = raw::page::move_pages(bindings, pdf, &[0, 1], count).is_err();
            Ok((count, bad_dup, bad_range, bad_dest))
        }
    })
    .unwrap();
    assert_eq!(count, 6, "outline-labels.pdf has six pages");
    assert!(bad_dup, "duplicate indices must be rejected");
    assert!(bad_range, "out-of-range indices must be rejected");
    assert!(bad_dest, "a destination past the end must be rejected");

    // Move page 0 to the end, structurally (the LRU is flushed by `.structural()`).
    with_state({
        let doc_id = doc_id.clone();
        move |st| {
            registry::mutate(
                st,
                &doc_id,
                MutateOpts::new("undo.pageMove", ChangeReason::Pages).structural(),
                |doc| {
                    let bindings = doc.bindings();
                    raw::page::move_pages(bindings, doc.pdf(), &[0], 5)
                },
            )
        }
    })
    .unwrap();

    // Duplicate page 0 in place: src == dest.
    let after = with_state({
        let doc_id = doc_id.clone();
        move |st| {
            registry::mutate(
                st,
                &doc_id,
                MutateOpts::new("undo.pageDuplicate", ChangeReason::Pages).structural(),
                |doc| {
                    let bindings = doc.bindings();
                    raw::page::import_pages_by_index(bindings, doc.pdf(), doc.pdf(), &[0], 1)?;
                    Ok(raw::page::page_count(bindings, doc.pdf()))
                },
            )
        }
    })
    .unwrap();
    assert_eq!(after, 7, "the duplicate adds one page");

    // Flatten + save with explicit flags.
    let (outcome, bytes) = with_state({
        let doc_id = doc_id.clone();
        move |st| {
            let bindings = st.doc(&doc_id)?.bindings();
            let doc = st.doc_mut(&doc_id)?;
            let outcome = {
                let page = doc.page(0)?;
                raw::page::flatten(bindings, page, raw::page::FlattenMode::NormalDisplay)?
            };
            // The FPDF_PAGE is invalid after a flatten.
            doc.invalidate_page(0);
            let bytes = raw::save::save_as_copy(
                bindings,
                st.doc(&doc_id)?.pdf(),
                raw::save::SaveFlags::NoIncremental,
            )?;
            Ok((outcome, bytes))
        }
    })
    .unwrap();
    assert!(
        matches!(
            outcome,
            raw::page::FlattenOutcome::Flattened | raw::page::FlattenOutcome::NothingToDo
        ),
        "flatten must not fail"
    );
    assert!(bytes.starts_with(b"%PDF"), "save_as_copy writes a PDF");
    assert!(
        bytes.len() > 500,
        "save_as_copy wrote {} bytes",
        bytes.len()
    );

    let reopened = reopen(bytes);
    assert_eq!(reopened.info.page_count, 7);
}

// ---------------------------------------------------------------------------------------
// markup
// ---------------------------------------------------------------------------------------

#[test]
fn annot_markup_roundtrip() {
    let doc = open("tracemonkey.pdf");
    let quads = vec![
        Rect::new(100.0, 700.0, 200.0, 712.0),
        Rect::new(100.0, 680.0, 260.0, 692.0),
    ];
    let probe = Rect::new(95.0, 675.0, 265.0, 717.0);
    let before = render_rect(&doc.doc_id, 0, probe);

    let id = create(
        &doc.doc_id,
        0,
        AnnotSpec::Highlight(MarkupSpec {
            rects: quads.clone(),
            color: [255, 235, 0],
            opacity: 0.6,
            contents: Some("두 줄 형광펜".to_string()),
        }),
    );
    let underline = create(
        &doc.doc_id,
        0,
        AnnotSpec::Underline(MarkupSpec {
            rects: vec![Rect::new(100.0, 640.0, 220.0, 652.0)],
            color: [0, 0, 255],
            opacity: 1.0,
            contents: None,
        }),
    );
    let strikeout = create(
        &doc.doc_id,
        0,
        AnnotSpec::Strikeout(MarkupSpec {
            rects: vec![Rect::new(100.0, 620.0, 220.0, 632.0)],
            color: [255, 0, 0],
            opacity: 1.0,
            contents: None,
        }),
    );
    let squiggly = create(
        &doc.doc_id,
        0,
        AnnotSpec::Squiggly(MarkupSpec {
            rects: vec![Rect::new(100.0, 600.0, 220.0, 612.0)],
            color: [0, 160, 0],
            opacity: 1.0,
            contents: None,
        }),
    );

    let bytes = save_bytes(&doc.doc_id);
    write_artifacts("markup", &bytes, &doc.doc_id, 0);
    let saved = reopen(bytes);
    let annots = list(&saved.doc_id, 0);

    let highlight = find(&annots, &id);
    assert_eq!(highlight.kind, AnnotKind::Highlight);
    assert_eq!(highlight.subtype, "Highlight");
    assert_eq!(highlight.color, [255, 235, 0]);
    assert!(
        (highlight.opacity - 0.6).abs() < 0.01,
        "opacity survived as {}",
        highlight.opacity
    );
    assert_eq!(highlight.contents, "두 줄 형광펜");
    assert_eq!(highlight.editable, Editability::Full);
    assert!(highlight.printed, "created annotations carry /F 4");
    let read_quads = highlight.quads.as_ref().expect("two quads");
    assert_eq!(read_quads.len(), 2);
    for (want, got) in quads.iter().zip(read_quads) {
        assert!((want.l - got.l).abs() < 0.5 && (want.t - got.t).abs() < 0.5);
    }
    // /Rect is the union of the quads — Acrobat hit-tests against it.
    assert!(highlight.rect.l <= 100.5 && highlight.rect.r >= 259.5);

    for (id, kind) in [
        (&underline, AnnotKind::Underline),
        (&strikeout, AnnotKind::Strikeout),
        (&squiggly, AnnotKind::Squiggly),
    ] {
        assert_eq!(find(&annots, id).kind, kind);
    }

    let after = render_rect(&saved.doc_id, 0, probe);
    let changed = changed_pixels(&before, &after);
    assert!(
        changed > 500,
        "the reopened highlight must be visible: only {changed} pixels changed"
    );
}

/// The one gotcha that silently produces an invisible annotation: PDFium reads a quad as
/// TL, TR, BL, BR and `PdfQuadPoints::from_rect()` emits BL, BR, TR, TL, which normalises to
/// a zero-width rectangle (annotations spike §5.2).
#[test]
fn annot_quad_order() {
    let doc = open("tracemonkey.pdf");
    let rect = Rect::new(120.0, 560.0, 300.0, 575.0);
    let before = render_rect(&doc.doc_id, 0, rect);
    let id = create(
        &doc.doc_id,
        0,
        AnnotSpec::Highlight(MarkupSpec {
            rects: vec![rect],
            color: [0, 255, 255],
            opacity: 1.0,
            contents: None,
        }),
    );

    // The quad must come back as the same rectangle, not as a sliver.
    let annots = list(&doc.doc_id, 0);
    let quad = find(&annots, &id).quads.as_ref().unwrap()[0];
    assert!(
        (quad.width() - rect.width()).abs() < 0.5,
        "quad width {} should be {}",
        quad.width(),
        rect.width()
    );
    assert!(
        (quad.height() - rect.height()).abs() < 0.5,
        "quad height {} should be {} — a 1-px sliver means BL,BR,TR,TL order",
        quad.height(),
        rect.height()
    );

    let after = render_rect(&doc.doc_id, 0, rect);
    let changed = changed_pixels(&before, &after);
    let area = (before.0 * before.1) as usize;
    assert!(
        changed > area / 4,
        "a correctly ordered quad paints most of its rect: {changed} of {area} pixels"
    );
}

// ---------------------------------------------------------------------------------------
// ink, shapes, lines
// ---------------------------------------------------------------------------------------

#[test]
fn annot_ink_roundtrip() {
    let doc = open("tracemonkey.pdf");
    let probe = Rect::new(60.0, 380.0, 260.0, 520.0);
    let before = render_rect(&doc.doc_id, 0, probe);
    let paths = vec![
        vec![80.0, 400.0, 120.0, 480.0, 160.0, 400.0],
        vec![180.0, 400.0, 200.0, 500.0, 240.0, 420.0, 220.0, 400.0],
    ];
    let id = create(
        &doc.doc_id,
        0,
        AnnotSpec::Ink(InkSpec {
            paths: paths.clone(),
            color: [220, 0, 120],
            width: 3.0,
            opacity: 1.0,
        }),
    );
    let signature = create(
        &doc.doc_id,
        0,
        AnnotSpec::Signature(InkSpec {
            paths: vec![vec![300.0, 400.0, 340.0, 440.0, 380.0, 400.0]],
            color: [0, 0, 128],
            width: 2.0,
            opacity: 1.0,
        }),
    );

    let bytes = save_bytes(&doc.doc_id);
    write_artifacts("ink", &bytes, &doc.doc_id, 0);
    let saved = reopen(bytes);
    let annots = list(&saved.doc_id, 0);

    let ink = find(&annots, &id);
    assert_eq!(ink.kind, AnnotKind::Ink);
    assert_eq!(ink.subtype, "Ink");
    assert!((ink.border_width - 3.0).abs() < 0.01);
    let read_paths = ink.ink_paths.as_ref().expect("a real /InkList");
    assert_eq!(read_paths.len(), 2, "two strokes survived the save");
    assert_eq!(read_paths[0].len(), 6);
    assert_eq!(read_paths[1].len(), 8);
    for (want, got) in paths[0].iter().zip(&read_paths[0]) {
        assert!((want - got).abs() < 0.01, "{want} != {got}");
    }

    let sig = find(&annots, &signature);
    assert_eq!(sig.kind, AnnotKind::Signature, "/Subj marks the signature");
    assert_eq!(sig.subtype, "Ink");

    let after = render_rect(&saved.doc_id, 0, probe);
    assert!(
        changed_pixels(&before, &after) > 200,
        "the reopened ink must be visible"
    );
}

#[test]
fn annot_shapes_roundtrip() {
    let doc = open("tracemonkey.pdf");
    let square = Rect::new(80.0, 200.0, 200.0, 300.0);
    let circle = Rect::new(240.0, 200.0, 380.0, 300.0);
    let probe = Rect::new(70.0, 190.0, 390.0, 310.0);
    let before = render_rect(&doc.doc_id, 0, probe);

    let sq = create(
        &doc.doc_id,
        0,
        AnnotSpec::Square(ShapeSpec {
            rect: square,
            color: [0, 0, 255],
            fill_color: Some([200, 220, 255]),
            width: 2.0,
            opacity: 1.0,
        }),
    );
    // Circle has no high-level constructor in pdfium-render 0.9.4: raw only.
    let ci = create(
        &doc.doc_id,
        0,
        AnnotSpec::Circle(ShapeSpec {
            rect: circle,
            color: [255, 0, 0],
            fill_color: Some([255, 255, 0]),
            width: 3.0,
            opacity: 0.5,
        }),
    );

    let bytes = save_bytes(&doc.doc_id);
    write_artifacts("shapes", &bytes, &doc.doc_id, 0);
    let saved = reopen(bytes);
    let annots = list(&saved.doc_id, 0);

    let s = find(&annots, &sq);
    assert_eq!(s.kind, AnnotKind::Square);
    assert_eq!(s.color, [0, 0, 255]);
    assert_eq!(s.fill_color, Some([200, 220, 255]));
    assert!((s.border_width - 2.0).abs() < 0.01);

    let c = find(&annots, &ci);
    assert_eq!(c.kind, AnnotKind::Circle);
    assert_eq!(c.subtype, "Circle");
    assert_eq!(c.fill_color, Some([255, 255, 0]));
    assert!((c.border_width - 3.0).abs() < 0.01);
    assert!(
        (c.opacity - 0.5).abs() < 0.01,
        "/CA survived as {}",
        c.opacity
    );

    let after = render_rect(&saved.doc_id, 0, probe);
    assert!(
        changed_pixels(&before, &after) > 2000,
        "both shapes must be visible after reopening"
    );
}

/// PDFium cannot create a `/Line` annotation, so a line is an Ink carrying
/// `/Subj "SeePDF:Line"` and an arrow adds two head strokes.
#[test]
fn annot_line_subj_roundtrip() {
    let doc = open("tracemonkey.pdf");
    let probe = Rect::new(60.0, 90.0, 400.0, 180.0);
    let before = render_rect(&doc.doc_id, 0, probe);

    let line = create(
        &doc.doc_id,
        0,
        AnnotSpec::Line(LineSpec {
            p1: [80.0, 110.0],
            p2: [380.0, 110.0],
            color: [0, 0, 0],
            width: 2.0,
            opacity: 1.0,
            heads: None,
        }),
    );
    let arrow = create(
        &doc.doc_id,
        0,
        AnnotSpec::Arrow(LineSpec {
            p1: [80.0, 160.0],
            p2: [380.0, 160.0],
            color: [200, 0, 0],
            width: 2.0,
            opacity: 1.0,
            heads: None,
        }),
    );

    let bytes = save_bytes(&doc.doc_id);
    write_artifacts("line-arrow", &bytes, &doc.doc_id, 0);
    let saved = reopen(bytes);
    let annots = list(&saved.doc_id, 0);

    let l = find(&annots, &line);
    assert_eq!(l.kind, AnnotKind::Line, "/Subj round-tripped");
    assert_eq!(l.subtype, "Ink", "…while the file still holds an Ink");
    assert_eq!(l.ink_paths.as_ref().unwrap().len(), 1, "no arrow heads");
    let points = l.line_points.expect("line endpoints");
    assert!((points[0] - 80.0).abs() < 0.01 && (points[2] - 380.0).abs() < 0.01);

    let a = find(&annots, &arrow);
    assert_eq!(a.kind, AnnotKind::Arrow);
    assert_eq!(
        a.ink_paths.as_ref().unwrap().len(),
        3,
        "the segment plus two head strokes"
    );

    let after = render_rect(&saved.doc_id, 0, probe);
    assert!(
        changed_pixels(&before, &after) > 300,
        "line and arrow must be visible"
    );
}

// ---------------------------------------------------------------------------------------
// text box and stamps
// ---------------------------------------------------------------------------------------

/// A Korean text box. `/Helv` has no Hangul glyphs and PDFium's FreeText appearance
/// generator only knows the base-14 fonts, so this is a Stamp carrying real text objects
/// drawn with an embedded CID font (`ARCHITECTURE.md` §6.1).
#[test]
fn annot_textbox_korean() {
    let doc = open("tracemonkey.pdf");
    let rect = Rect::new(60.0, 430.0, 360.0, 500.0);
    let before = render_rect(&doc.doc_id, 0, rect);
    let text = "한글 텍스트 상자\nSeePDF 주석 테스트";

    let id = create(
        &doc.doc_id,
        0,
        AnnotSpec::Textbox(TextBoxSpec {
            rect,
            text: text.to_string(),
            font_size: 16.0,
            color: [0, 0, 160],
            align: TextAlign::Left,
            fill_color: Some([255, 255, 210]),
        }),
    );

    let bytes = save_bytes(&doc.doc_id);
    write_artifacts("textbox-korean", &bytes, &doc.doc_id, 0);
    let saved = reopen(bytes);
    let annots = list(&saved.doc_id, 0);

    let tb = find(&annots, &id);
    assert_eq!(tb.kind, AnnotKind::Textbox);
    assert_eq!(tb.subtype, "Stamp", "a text box is a Stamp on disk");
    assert_eq!(tb.text.as_deref(), Some(text));
    assert_eq!(tb.font_size, Some(16.0), "/DA carries the size");
    assert_eq!(tb.color, [0, 0, 160]);

    let after = render_rect(&saved.doc_id, 0, rect);
    let changed = changed_pixels(&before, &after);
    assert!(
        changed > 4000,
        "the Korean text box must render after reopening: {changed} pixels changed"
    );
}

#[test]
fn annot_stamp_image_roundtrip() {
    let doc = open("tracemonkey.pdf");
    let rect = Rect::new(400.0, 200.0, 500.0, 300.0);
    let before = render_rect(&doc.doc_id, 0, rect);

    // A 64×64 PNG written to the fixture output directory, then stamped from its path.
    let image_path = out_dir().join("stamp-source.png");
    write_test_png(&image_path);

    let id = create(
        &doc.doc_id,
        0,
        AnnotSpec::Stamp(StampSpec {
            rect,
            image: StampImage::Path {
                path: image_path.display().to_string(),
            },
            rotate: None,
            signature: false,
        }),
    );
    let builtin = create(
        &doc.doc_id,
        0,
        AnnotSpec::Stamp(StampSpec {
            rect: Rect::new(400.0, 320.0, 540.0, 360.0),
            image: StampImage::Builtin {
                builtin: "approved".to_string(),
            },
            rotate: None,
            signature: false,
        }),
    );

    let bytes = save_bytes(&doc.doc_id);
    write_artifacts("stamp", &bytes, &doc.doc_id, 0);
    let saved = reopen(bytes);
    let annots = list(&saved.doc_id, 0);

    let stamp = find(&annots, &id);
    assert_eq!(stamp.kind, AnnotKind::Stamp);
    assert_eq!(stamp.subtype, "Stamp");
    assert!((stamp.rect.l - rect.l).abs() < 0.5 && (stamp.rect.t - rect.t).abs() < 0.5);

    let label = find(&annots, &builtin);
    assert_eq!(label.stamp_kind.as_deref(), Some("approved"));

    let after = render_rect(&saved.doc_id, 0, rect);
    let changed = changed_pixels(&before, &after);
    let area = (before.0 * before.1) as usize;
    assert!(
        changed > area / 2,
        "the image stamp covers its rect: {changed} of {area} pixels"
    );
}

/// P1-12: the Korean stamp set. Each of 결재 / 승인 / 기밀 is a `Stamp` whose `/Subj` is the
/// builtin name, drawn with a red border **and** a Hangul label from the bundled subset — the
/// inner area (away from the border) must have red pixels, which is the label, so a font that
/// silently drops Hangul glyphs fails here. Three stamps embed the font once.
#[test]
fn annot_stamp_korean_builtin() {
    let doc = open("tracemonkey.pdf");
    let baseline = save_bytes(&doc.doc_id).len();
    let names = ["결재", "승인", "기밀"];
    let rects = [
        Rect::new(380.0, 600.0, 452.0, 672.0),
        Rect::new(460.0, 600.0, 532.0, 672.0),
        Rect::new(380.0, 520.0, 530.0, 570.0),
    ];
    let before: Vec<_> = rects
        .iter()
        .map(|r| render_rect(&doc.doc_id, 0, *r))
        .collect();

    let ids: Vec<String> = names
        .iter()
        .zip(rects.iter())
        .map(|(name, rect)| {
            create(
                &doc.doc_id,
                0,
                AnnotSpec::Stamp(StampSpec {
                    rect: *rect,
                    image: StampImage::Builtin {
                        builtin: name.to_string(),
                    },
                    rotate: None,
                    signature: false,
                }),
            )
        })
        .collect();

    let bytes = save_bytes(&doc.doc_id);
    write_artifacts("stamp-korean", &bytes, &doc.doc_id, 0);
    let growth = bytes.len().saturating_sub(baseline);
    assert!(
        growth < 900_000,
        "three Korean stamps must embed the Hangul subset once, grew by {growth} bytes"
    );

    let saved = reopen(bytes);
    let annots = list(&saved.doc_id, 0);
    for (((name, id), rect), before) in names.iter().zip(&ids).zip(&rects).zip(&before) {
        let stamp = find(&annots, id);
        assert_eq!(stamp.kind, AnnotKind::Stamp, "{name}");
        assert_eq!(stamp.subtype, "Stamp", "{name}");
        assert_eq!(
            stamp.stamp_kind.as_deref(),
            Some(*name),
            "/Subj is the builtin name"
        );

        let after = render_rect(&saved.doc_id, 0, *rect);
        let changed = changed_pixels(before, &after);
        assert!(
            changed > 300,
            "{name}: the stamp renders ({changed} pixels changed)"
        );

        // The label: red pixels in the middle 60 % of the box, where the border never is.
        let (w, h, px) = &after;
        let (w, h) = (*w as usize, *h as usize);
        let mut label_red = 0usize;
        for y in (h * 2 / 10)..(h * 8 / 10) {
            for x in (w * 2 / 10)..(w * 8 / 10) {
                let p = &px[(y * w + x) * 4..(y * w + x) * 4 + 3];
                if p[0] > 150 && p[1] < 110 && p[2] < 110 {
                    label_red += 1;
                }
            }
        }
        assert!(
            label_red > 80,
            "{name}: the Hangul label must be drawn in red inside the border ({label_red} px)"
        );
    }
}

/// P1-9: a typed signature is a PNG with a **transparent** background, placed as an image
/// stamp. The page must show through around the ink — an opaque box would hide the line the
/// signature is written on.
#[test]
fn annot_stamp_png_keeps_transparency() {
    let doc = open("tracemonkey.pdf");
    // Over body text, so "the page shows through" is measurable.
    let rect = Rect::new(100.0, 500.0, 260.0, 556.0);
    let before = render_rect(&doc.doc_id, 0, rect);

    // 160×56 px, transparent except a dark bar through the middle third.
    let (w, h) = (160u32, 56u32);
    let mut rgba = vec![0u8; (w * h * 4) as usize];
    for y in (h / 3)..(2 * h / 3) {
        for x in 0..w {
            let i = ((y * w + x) * 4) as usize;
            rgba[i..i + 4].copy_from_slice(&[17, 19, 24, 255]);
        }
    }
    let path = out_dir().join("signature-typed.png");
    {
        let file = std::fs::File::create(&path).expect("create png");
        let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), w, h);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder
            .write_header()
            .expect("png header")
            .write_image_data(&rgba)
            .expect("png data");
    }
    create(
        &doc.doc_id,
        0,
        AnnotSpec::Stamp(StampSpec {
            rect,
            image: StampImage::Path {
                path: path.display().to_string(),
            },
            rotate: None,
            signature: false,
        }),
    );
    let bytes = save_bytes(&doc.doc_id);
    let saved = reopen(bytes);
    let after = render_rect(&saved.doc_id, 0, rect);

    // Compare the top quarter (transparent in the PNG) and the middle band (the bar).
    let (rw, rh) = (after.0 as usize, after.1 as usize);
    let band = |img: &(u32, u32, Vec<u8>), y0: usize, y1: usize| -> (usize, usize) {
        let mut changed = 0usize;
        let mut total = 0usize;
        for y in y0..y1 {
            for x in 0..rw {
                let i = (y * rw + x) * 4;
                let (a, b) = (&before.2[i..i + 3], &img.2[i..i + 3]);
                total += 1;
                if (0..3).any(|c| (a[c] as i32 - b[c] as i32).abs() > PIXEL_TOLERANCE) {
                    changed += 1;
                }
            }
        }
        (changed, total)
    };
    // The check is only meaningful over ink: an opaque *white* box would pass it on paper.
    let dark_before = before.2[..rw * (rh / 4) * 4]
        .chunks_exact(4)
        .filter(|p| p[0] < 128 && p[1] < 128 && p[2] < 128)
        .count();
    assert!(
        dark_before > 50,
        "the test rect must sit over text ({dark_before} dark px)"
    );
    let (top_changed, top_total) = band(&after, 0, rh / 4);
    let (mid_changed, mid_total) = band(&after, rh * 4 / 10, rh * 6 / 10);
    assert!(
        top_changed * 20 < top_total,
        "transparent pixels must leave the page visible: {top_changed} of {top_total} changed"
    );
    assert!(
        mid_changed * 2 > mid_total,
        "the ink must be drawn: {mid_changed} of {mid_total} changed"
    );
}

fn write_test_png(path: &std::path::Path) {
    let (w, h) = (64u32, 64u32);
    let mut rgba = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            rgba.extend_from_slice(&[255, (x * 4) as u8, (y * 4) as u8, 255]);
        }
    }
    let file = std::fs::File::create(path).expect("create stamp source png");
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), w, h);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder
        .write_header()
        .expect("png header")
        .write_image_data(&rgba)
        .expect("png data");
}

// ---------------------------------------------------------------------------------------
// editing and deleting
// ---------------------------------------------------------------------------------------

/// Recolouring an annotation that already has an `/AP` is the one operation that crashes the
/// process if done naively: `FPDFAnnot_SetColor` returns false and pdfium-render's fallback
/// casts `CPDF_AnnotContext*` to `CPDF_PageObject*` (annotations spike §5.1). SeePDF's path
/// is `SetAP(NULL)` → `SetColor` → re-render.
///
/// The whole edit runs in a **child process** so a regression shows up as a signal rather
/// than as a corrupted test run, and so this test can assert "exit status 0".
#[test]
fn annot_recolor_after_reopen() {
    if std::env::var("SEEPDF_RECOLOR_CHILD").is_ok() {
        recolor_body();
        return;
    }
    let exe = std::env::current_exe().expect("test binary path");
    let output = std::process::Command::new(exe)
        .args([
            "--exact",
            "annot_recolor_after_reopen",
            "--nocapture",
            "--test-threads=1",
        ])
        .env("SEEPDF_RECOLOR_CHILD", "1")
        .output()
        .expect("spawn the recolour child");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "recolouring an annotation with an /AP must not crash: status {:?}\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}",
        output.status
    );
    assert!(
        stdout.contains("recolour ok"),
        "the child did not reach the end:\n{stdout}"
    );
}

fn recolor_body() {
    let doc = open("tracemonkey.pdf");
    let rect = Rect::new(80.0, 200.0, 200.0, 300.0);
    let id = create(
        &doc.doc_id,
        0,
        AnnotSpec::Square(ShapeSpec {
            rect,
            color: [0, 128, 0],
            fill_color: Some([0, 255, 0]),
            width: 2.0,
            opacity: 1.0,
        }),
    );

    // Save + reopen: now the annotation has an /AP on disk, which is what breaks the naive
    // path. Render once so an appearance stream definitely exists in memory too.
    let bytes = save_bytes(&doc.doc_id);
    let saved = reopen(bytes);
    let green = render_rect(&saved.doc_id, 0, rect);

    let patch = seepdf_lib::ipc::types::AnnotPatch {
        color: Some([255, 0, 255]),
        fill_color: Some(Some([255, 200, 255])),
        opacity: Some(0.8),
        contents: Some("recoloured".to_string()),
        ..Default::default()
    };
    let previous = {
        let doc_id = saved.doc_id.clone();
        let id = id.clone();
        with_state(move |st| {
            registry::mutate(
                st,
                &doc_id,
                MutateOpts::new("undo.annotEdit", ChangeReason::Edit).page(0),
                |doc| annot::update::update(doc, 0, &id, &patch),
            )
        })
        .expect("recolour")
    };
    assert_eq!(previous.color, [0, 128, 0], "previous state is reported");

    let magenta = render_rect(&saved.doc_id, 0, rect);
    assert!(
        changed_pixels(&green, &magenta) > 1000,
        "the new colour must be visible after the AP was regenerated"
    );

    let annots = list(&saved.doc_id, 0);
    let updated = find(&annots, &id);
    assert_eq!(updated.color, [255, 0, 255]);
    assert_eq!(updated.fill_color, Some([255, 200, 255]));
    assert!((updated.opacity - 0.8).abs() < 0.01);
    assert_eq!(updated.contents, "recoloured");

    // …and it survives a second save + reopen.
    let bytes = save_bytes(&saved.doc_id);
    write_artifacts("recolour", &bytes, &saved.doc_id, 0);
    let twice = reopen(bytes);
    let annots = list(&twice.doc_id, 0);
    assert_eq!(find(&annots, &id).color, [255, 0, 255]);

    println!("recolour ok");
}

#[test]
fn annot_delete_persists() {
    let doc = open("tracemonkey.pdf");
    let keep = create(
        &doc.doc_id,
        0,
        AnnotSpec::Note(NoteSpec {
            at: [40.0, 720.0],
            color: [255, 200, 0],
            contents: "남겨둘 메모".to_string(),
        }),
    );
    let drop_me = create(
        &doc.doc_id,
        0,
        AnnotSpec::Note(NoteSpec {
            at: [40.0, 680.0],
            color: [255, 0, 0],
            contents: "삭제할 메모".to_string(),
        }),
    );
    assert_eq!(list(&doc.doc_id, 0).len(), 2);

    let missing = {
        let doc_id = doc.doc_id.clone();
        with_state(move |st| {
            registry::mutate(
                st,
                &doc_id,
                MutateOpts::new("undo.annotDelete", ChangeReason::Edit).page(0),
                |doc| annot::delete(doc, 0, &["not-an-id".to_string()]),
            )
        })
    };
    assert!(missing.is_err(), "deleting an unknown id is notFound");
    assert_eq!(
        list(&doc.doc_id, 0).len(),
        2,
        "a failed mutate leaves the document untouched"
    );

    {
        let doc_id = doc.doc_id.clone();
        let drop_me = drop_me.clone();
        with_state(move |st| {
            registry::mutate(
                st,
                &doc_id,
                MutateOpts::new("undo.annotDelete", ChangeReason::Edit).page(0),
                |doc| annot::delete(doc, 0, &[drop_me]),
            )
        })
        .expect("delete");
    }

    let bytes = save_bytes(&doc.doc_id);
    write_artifacts("delete", &bytes, &doc.doc_id, 0);
    let saved = reopen(bytes);
    let annots = list(&saved.doc_id, 0);
    assert_eq!(annots.len(), 1, "one annotation left after save + reopen");
    assert_eq!(annots[0].id, keep);
    assert_eq!(annots[0].kind, AnnotKind::Note);
    assert_eq!(annots[0].contents, "남겨둘 메모");
}

/// `/NM` is assigned to annotations that arrive without one, so the frontend always has a
/// stable id (`IPC_CONTRACT.md` §7.1).
#[test]
fn annot_ids_assigned_to_foreign_annotations() {
    let doc = open("annotation-highlight.pdf");
    let annots = list(&doc.doc_id, 0);
    assert!(!annots.is_empty(), "the fixture has annotations");
    for a in &annots {
        assert!(!a.id.is_empty(), "every annotation has an id");
    }
    // Stable across calls.
    let again = list(&doc.doc_id, 0);
    assert_eq!(ids(&annots), ids(&again));
}

/// An existing `/Line` annotation from another producer is readable (PDFium exposes
/// `FPDFAnnot_GetLine`) even though it cannot be created.
#[test]
fn annot_reads_foreign_line() {
    let doc = open("annotation-line.pdf");
    let annots = list(&doc.doc_id, 0);
    let line = annots
        .iter()
        .find(|a| a.subtype == "Line")
        .expect("annotation-line.pdf has a Line");
    assert_eq!(line.kind, AnnotKind::Line);
    let p = line.line_points.expect("endpoints from FPDFAnnot_GetLine");
    assert!(p[0] > 0.0 && p[2] > 0.0, "endpoints were read: {p:?}");
}

/// Link annotations: `FPDFAnnot_SetURI` works and the URI reads back. `AnnotSpec` has no
/// `link` variant (creation is P2 in `IPC_CONTRACT.md` §7.1), so the tool calls the engine
/// entry point directly — this pins it.
#[test]
fn annot_link_roundtrip() {
    let doc = open("tracemonkey.pdf");
    let rect = Rect::new(120.0, 540.0, 300.0, 556.0);
    let uri = "https://example.com/seepdf";
    let id = {
        let doc_id = doc.doc_id.clone();
        with_state(move |st| {
            registry::mutate(
                st,
                &doc_id,
                MutateOpts::new("undo.annotCreate", ChangeReason::Edit).page(0),
                |doc| annot::create::create_link(doc, 0, rect, &[rect], uri),
            )
        })
        .expect("create link")
    };

    let bytes = save_bytes(&doc.doc_id);
    let saved = reopen(bytes);
    let annots = list(&saved.doc_id, 0);
    let link = find(&annots, &id);
    assert_eq!(link.kind, AnnotKind::Link);
    assert_eq!(link.subtype, "Link");
    assert_eq!(link.uri.as_deref(), Some(uri), "the URI survived the save");
    assert_eq!(
        link.quads.as_ref().map(|q| q.len()),
        Some(1),
        "the click area is a quad, not just /Rect"
    );
}

/// `set_annotations_hidden` is a **view** change (`IPC_CONTRACT.md` §7.1): it flips `/F`
/// HIDDEN, does not go through `registry::mutate`, does not bump the generation and does not
/// dirty the document — but it does change what the page renders.
#[test]
fn annot_hidden_is_transient() {
    let doc = open("tracemonkey.pdf");
    let rect = Rect::new(80.0, 200.0, 200.0, 300.0);
    let before = render_rect(&doc.doc_id, 0, rect);
    let id = create(
        &doc.doc_id,
        0,
        AnnotSpec::Square(ShapeSpec {
            rect,
            color: [255, 0, 0],
            fill_color: Some([255, 200, 200]),
            width: 2.0,
            opacity: 1.0,
        }),
    );
    let visible = render_rect(&doc.doc_id, 0, rect);
    assert!(
        changed_pixels(&before, &visible) > 1000,
        "the square is drawn"
    );

    let generation = {
        let doc_id = doc.doc_id.clone();
        with_state(move |st| Ok(st.doc(&doc_id)?.generation)).unwrap()
    };
    let changed = {
        let doc_id = doc.doc_id.clone();
        let id = id.clone();
        with_doc(&doc_id, move |doc| annot::set_hidden(doc, 0, &[id], true)).expect("hide")
    };
    assert_eq!(changed, 1);

    let after = {
        let doc_id = doc.doc_id.clone();
        with_state(move |st| Ok(st.doc(&doc_id)?.generation)).unwrap()
    };
    assert_eq!(after, generation, "hiding does not bump the generation");

    let annots = list(&doc.doc_id, 0);
    assert!(find(&annots, &id).hidden, "the /F HIDDEN bit is set");
    let hidden = render_rect(&doc.doc_id, 0, rect);
    assert!(
        changed_pixels(&before, &hidden) < 100,
        "a hidden annotation is not rendered"
    );
}

/// Changing the number of markup quads cannot be done in place — PDFium can append and
/// replace a quad but never remove one — so `update_annotation` re-creates the annotation
/// with the same `/NM`. The id, contents and author must survive that.
#[test]
fn annot_update_markup_rects_rebuild() {
    let doc = open("tracemonkey.pdf");
    let id = create(
        &doc.doc_id,
        0,
        AnnotSpec::Highlight(MarkupSpec {
            rects: vec![
                Rect::new(100.0, 700.0, 200.0, 712.0),
                Rect::new(100.0, 680.0, 260.0, 692.0),
            ],
            color: [255, 235, 0],
            opacity: 0.6,
            contents: Some("두 줄".to_string()),
        }),
    );

    // Three quads now: the rebuild path.
    let patch = seepdf_lib::ipc::types::AnnotPatch {
        rects: Some(vec![
            Rect::new(100.0, 700.0, 200.0, 712.0),
            Rect::new(100.0, 680.0, 260.0, 692.0),
            Rect::new(100.0, 660.0, 180.0, 672.0),
        ]),
        author: Some("SeePDF".to_string()),
        ..Default::default()
    };
    let previous = {
        let doc_id = doc.doc_id.clone();
        let id = id.clone();
        with_state(move |st| {
            registry::mutate(
                st,
                &doc_id,
                MutateOpts::new("undo.annotEdit", ChangeReason::Edit).page(0),
                |doc| annot::update::update(doc, 0, &id, &patch),
            )
        })
        .expect("update the quads")
    };
    assert_eq!(previous.quads.as_ref().map(|q| q.len()), Some(2));

    let bytes = save_bytes(&doc.doc_id);
    let saved = reopen(bytes);
    let annots = list(&saved.doc_id, 0);
    assert_eq!(annots.len(), 1, "the rebuild left exactly one annotation");
    let updated = find(&annots, &id);
    assert_eq!(
        updated.quads.as_ref().map(|q| q.len()),
        Some(3),
        "three quads after the rebuild"
    );
    assert_eq!(updated.id, id, "the /NM is preserved across a rebuild");
    assert_eq!(updated.contents, "두 줄");
    assert_eq!(updated.author.as_deref(), Some("SeePDF"));
    assert_eq!(updated.color, [255, 235, 0]);
}

/// Editing a text box goes through the **rebuild** path: its glyphs are baked page objects
/// inside the stamp's appearance stream, and `FPDFAnnot_UpdateObject` is never called by
/// pdfium-render. Two things this pins that were wrong the first time round:
/// the rebuilt box keeps the *new* text (not the old `/Contents`), and its appearance stream
/// is not cleared afterwards — a Stamp's AP **is** its content and PDFium never regenerates
/// one, so `SetAP(NULL)` would erase the box.
#[test]
fn annot_textbox_edit_keeps_its_appearance() {
    let doc = open("tracemonkey.pdf");
    let rect = Rect::new(60.0, 430.0, 360.0, 500.0);
    let blank = render_rect(&doc.doc_id, 0, rect);
    let id = create(
        &doc.doc_id,
        0,
        AnnotSpec::Textbox(TextBoxSpec {
            rect,
            text: "before".to_string(),
            font_size: 16.0,
            color: [0, 0, 160],
            align: TextAlign::Left,
            fill_color: Some([255, 255, 210]),
        }),
    );

    let patch = seepdf_lib::ipc::types::AnnotPatch {
        text: Some("고친 텍스트".to_string()),
        font_size: Some(20.0),
        color: Some([200, 0, 0]),
        author: Some("SeePDF".to_string()),
        ..Default::default()
    };
    {
        let doc_id = doc.doc_id.clone();
        let id = id.clone();
        with_state(move |st| {
            registry::mutate(
                st,
                &doc_id,
                MutateOpts::new("undo.annotEdit", ChangeReason::Edit).page(0),
                |doc| annot::update::update(doc, 0, &id, &patch),
            )
        })
        .expect("edit the text box");
    }

    let bytes = save_bytes(&doc.doc_id);
    write_artifacts("textbox-edited", &bytes, &doc.doc_id, 0);
    let saved = reopen(bytes);
    let annots = list(&saved.doc_id, 0);
    assert_eq!(annots.len(), 1, "the rebuild left exactly one annotation");
    let tb = find(&annots, &id);
    assert_eq!(tb.text.as_deref(), Some("고친 텍스트"), "the new text won");
    assert_eq!(tb.font_size, Some(20.0));
    assert_eq!(tb.color, [200, 0, 0]);
    assert_eq!(tb.author.as_deref(), Some("SeePDF"));

    let after = render_rect(&saved.doc_id, 0, rect);
    assert!(
        changed_pixels(&blank, &after) > 4000,
        "the edited text box is still drawn — its appearance stream was not cleared"
    );
}

/// P1-12 hide-while-dragging, as the frontend sequences it (`src/annot/dragHide.ts`):
/// hide → (drag) → **unhide** → one `update_annotation`. Undo must bring the annotation back
/// **visible** at its old place. The second half pins why the order matters: an update made
/// while the HIDDEN bit is set snapshots it, and undo restores a hidden annotation.
#[test]
fn annot_drag_hide_unhides_before_the_undo_snapshot() {
    let doc = open("tracemonkey.pdf");
    let rect = Rect::new(80.0, 200.0, 200.0, 300.0);
    let id = create(
        &doc.doc_id,
        0,
        AnnotSpec::Square(ShapeSpec {
            rect,
            color: [255, 0, 0],
            fill_color: None,
            width: 2.0,
            opacity: 1.0,
        }),
    );
    let set_hidden = |hidden: bool| {
        let (doc_id, id) = (doc.doc_id.clone(), id.clone());
        with_doc(&doc_id, move |d| annot::set_hidden(d, 0, &[id], hidden)).expect("set hidden")
    };
    let move_by = |dx: f32| {
        let (doc_id, id) = (doc.doc_id.clone(), id.clone());
        let patch = seepdf_lib::ipc::types::AnnotPatch {
            rect: Some(Rect::new(rect.l + dx, rect.b, rect.r + dx, rect.t)),
            ..Default::default()
        };
        with_state(move |st| {
            registry::mutate(
                st,
                &doc_id,
                MutateOpts::new("undo.annotEdit", ChangeReason::Edit)
                    .page(0)
                    .keeps_text(),
                |d| annot::update::update(d, 0, &id, &patch),
            )
        })
        .expect("move");
    };
    let undo = || {
        let doc_id = doc.doc_id.clone();
        with_state(move |st| registry::undo(st, &doc_id, false)).expect("undo");
    };

    // The frontend's order: hide, unhide, then the one update.
    set_hidden(true);
    set_hidden(false);
    move_by(40.0);
    let moved = list(&doc.doc_id, 0);
    assert!(!find(&moved, &id).hidden);
    assert!((find(&moved, &id).rect.l - (rect.l + 40.0)).abs() < 0.5);
    undo();
    let back = list(&doc.doc_id, 0);
    assert!(
        !find(&back, &id).hidden,
        "undo brings the annotation back visible"
    );
    assert!(
        (find(&back, &id).rect.l - rect.l).abs() < 0.5,
        "at its old place"
    );

    // The wrong order: the update snapshots the HIDDEN bit, and undo restores it hidden.
    set_hidden(true);
    move_by(40.0);
    undo();
    assert!(
        find(&list(&doc.doc_id, 0), &id).hidden,
        "an update made while hidden snapshots /F HIDDEN — which is why dragHide.ts unhides first"
    );
}

/// Stage 8: a typed / image signature is a Stamp with `signature: true` — written with
/// `/Subj "SeePDF:Signature"`, read back (also after save → reopen) as `kind: signature`,
/// editable, movable in place; a plain image stamp stays a stamp.
#[test]
fn annot_stamp_signature_reads_back_as_signature() {
    let doc = open("tracemonkey.pdf");
    let image_path = out_dir().join("signature-source.png");
    write_test_png(&image_path);
    let rect = Rect::new(100.0, 100.0, 220.0, 150.0);
    let signature = create(
        &doc.doc_id,
        0,
        AnnotSpec::Stamp(StampSpec {
            rect,
            image: StampImage::Path {
                path: image_path.display().to_string(),
            },
            rotate: None,
            signature: true,
        }),
    );
    let plain = create(
        &doc.doc_id,
        0,
        AnnotSpec::Stamp(StampSpec {
            rect: Rect::new(300.0, 100.0, 360.0, 160.0),
            image: StampImage::Path {
                path: image_path.display().to_string(),
            },
            rotate: None,
            signature: false,
        }),
    );
    let annots = list(&doc.doc_id, 0);
    let s = find(&annots, &signature);
    assert_eq!(s.kind, AnnotKind::Signature);
    assert_eq!(s.subtype, "Stamp");
    assert_eq!(s.stamp_kind.as_deref(), Some(annot::SUBJ_SIGNATURE));
    assert_eq!(s.editable, seepdf_lib::ipc::types::Editability::Full);
    assert_eq!(find(&annots, &plain).kind, AnnotKind::Stamp);

    // Moving it keeps it a signature (the Stamp-backed in-place path, not an Ink rebuild).
    let (d, id) = (doc.doc_id.clone(), signature.clone());
    let moved = Rect::new(120.0, 400.0, 240.0, 450.0);
    with_state(move |st| {
        registry::mutate(
            st,
            &d,
            MutateOpts::new("undo.annotEdit", seepdf_lib::ipc::types::ChangeReason::Edit).page(0),
            |doc| {
                annot::update::update(
                    doc,
                    0,
                    &id,
                    &seepdf_lib::ipc::types::AnnotPatch {
                        rect: Some(moved),
                        ..Default::default()
                    },
                )
            },
        )
    })
    .expect("move the signature");

    let saved = reopen(save_bytes(&doc.doc_id));
    let annots = list(&saved.doc_id, 0);
    let s = find(&annots, &signature);
    assert_eq!(s.kind, AnnotKind::Signature);
    assert!(
        (s.rect.b - moved.b).abs() < 0.5 && (s.rect.l - moved.l).abs() < 0.5,
        "{:?}",
        s.rect
    );
    assert_eq!(find(&annots, &plain).kind, AnnotKind::Stamp);

    // The wire shape: `signature` only when true.
    let spec = serde_json::to_value(AnnotSpec::Stamp(StampSpec {
        rect,
        image: StampImage::Builtin {
            builtin: "approved".into(),
        },
        rotate: None,
        signature: false,
    }))
    .unwrap();
    assert!(spec.get("signature").is_none());
    let parsed: AnnotSpec = serde_json::from_value(serde_json::json!({
        "kind": "stamp", "rect": { "l": 0.0, "b": 0.0, "r": 1.0, "t": 1.0 },
        "image": { "path": "/x.png" }, "signature": true
    }))
    .unwrap();
    assert!(matches!(
        parsed,
        AnnotSpec::Stamp(StampSpec {
            signature: true,
            ..
        })
    ));
}
