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
            dashed: false,
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
            dashed: false,
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
            measure: None,
            dashed: false,
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
            measure: None,
            dashed: false,
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
            dashed: false,
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
            dashed: false,
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
            dashed: false,
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

// ---------------------------------------------------------------------------------------
// Bug hunt regressions
// ---------------------------------------------------------------------------------------

/// `annotation-highlight.pdf` with page 1's `/Annots` prefixed by a `null` and a reference to
/// an object that does not exist — what producer-damaged files look like.
fn damaged_annots_bytes() -> Vec<u8> {
    use lopdf::Object;
    let mut doc = lopdf::Document::load(fixture("annotation-highlight.pdf")).expect("lopdf load");
    let page_id = *doc.get_pages().get(&1).expect("page 1");
    let annots: Vec<Object> = {
        let page = doc.get_dictionary(page_id).expect("page dict");
        match page.get(b"Annots").expect("the fixture has /Annots") {
            Object::Reference(id) => doc.get_object(*id).unwrap().as_array().unwrap().clone(),
            Object::Array(a) => a.clone(),
            other => panic!("unexpected /Annots {other:?}"),
        }
    };
    let mut damaged = vec![Object::Null, Object::Reference((999_999, 0))];
    damaged.extend(annots);
    doc.get_dictionary_mut(page_id)
        .unwrap()
        .set("Annots", Object::Array(damaged));
    let mut out = Vec::new();
    doc.save_to(&mut out).expect("lopdf save");
    out
}

/// Bug hunt: `FPDFPage_GetAnnotCount` counts every `/Annots` slot while `FPDFPage_GetAnnot`
/// returns NULL for a `null` / dangling entry, and every enumeration propagated that as
/// `notFound` — no annotation or field on the page could be listed, and `create_annotation`
/// failed *after* it had committed. A bad slot is skipped instead.
#[test]
fn annot_bad_annots_entries_are_skipped() {
    let doc = reopen(damaged_annots_bytes());
    let slots = with_doc(&doc.doc_id, |d| {
        let bindings = d.bindings();
        let page = d.page(0)?;
        Ok(raw::annot::count(bindings, page))
    })
    .unwrap();
    assert!(slots >= 3, "the two bad slots are really there ({slots})");

    let listed = with_doc(&doc.doc_id, |d| annot::list(d, 0));
    let listed = listed.expect("listing a page with bad /Annots slots");
    assert!(
        listed.iter().any(|a| a.kind == AnnotKind::Highlight),
        "{listed:?}"
    );
    let id = listed[0].id.clone();

    // Every other walk over the page's annotations.
    with_doc(&doc.doc_id, |d| {
        seepdf_lib::engine::form::list(d, None)?;
        Ok(())
    })
    .expect("list_form_fields");
    with_doc(&doc.doc_id, {
        let id = id.clone();
        move |d| annot::set_hidden(d, 0, &[id], true)
    })
    .expect("set_annotations_hidden");
    with_doc(&doc.doc_id, {
        let id = id.clone();
        move |d| annot::set_hidden(d, 0, &[id], false)
    })
    .expect("set_annotations_hidden back");

    let created = create(
        &doc.doc_id,
        0,
        AnnotSpec::Square(ShapeSpec {
            rect: Rect::new(20.0, 20.0, 60.0, 60.0),
            color: [200, 0, 0],
            fill_color: None,
            width: 1.0,
            opacity: 1.0,
            dashed: false,
        }),
    );
    let after = list(&doc.doc_id, 0);
    assert!(after.iter().any(|a| a.id == created));

    with_state({
        let doc_id = doc.doc_id.clone();
        let id = id.clone();
        move |st| {
            registry::mutate(
                st,
                &doc_id,
                MutateOpts::new("undo.annotEdit", ChangeReason::Edit).page(0),
                |d| {
                    annot::update::update(
                        d,
                        0,
                        &id,
                        &seepdf_lib::ipc::types::AnnotPatch {
                            contents: Some("edited".into()),
                            ..Default::default()
                        },
                    )
                },
            )
        }
    })
    .expect("update_annotation");
    with_state({
        let doc_id = doc.doc_id.clone();
        let ids = vec![created.clone()];
        move |st| {
            registry::mutate(
                st,
                &doc_id,
                MutateOpts::new("undo.annotDelete", ChangeReason::Edit).page(0),
                |d| annot::delete(d, 0, &ids),
            )
        }
    })
    .expect("delete_annotations");
    let last = list(&doc.doc_id, 0);
    assert!(last.iter().all(|a| a.id != created));
    assert_eq!(
        last.iter()
            .find(|a| a.id == id)
            .map(|a| a.contents.as_str()),
        Some("edited")
    );
}

/// Bug hunt: `list_annotations` stamps a `/NM` on an annotation that has none (in memory, no
/// new generation), but the first edit after open used to snapshot the **file** bytes — which
/// have no `/NM` — so undoing that edit reloaded the file and the next list handed out new
/// ids: every id the frontend held (selection, 주석 목록) was `notFound`.
#[test]
fn annot_ids_survive_undo_of_the_first_edit() {
    // The fixture's annotations carry a `/NM`; strip it, as LibreOffice and scanners write.
    let mut stripped = lopdf::Document::load(fixture("annotation-highlight.pdf")).unwrap();
    let mut removed = 0;
    for object in stripped.objects.values_mut() {
        if let Ok(dict) = object.as_dict_mut() {
            if dict.remove(b"NM").is_some() {
                removed += 1;
            }
        }
    }
    assert!(removed > 0, "the fixture had /NM entries to strip");
    let path = out_dir().join("no-nm.pdf");
    stripped.save(&path).unwrap();
    // Opened from a path: a clean document, so the first edit takes the free snapshot path.
    let info = with_state({
        let path = path.clone();
        move |st| registry::open(st, Some(path.clone()), std::fs::read(&path)?, None)
    })
    .expect("open the stripped copy");
    let doc = TestDoc {
        doc_id: info.doc_id.clone(),
        info,
    };
    let before = list(&doc.doc_id, 0);
    assert!(!before.is_empty());
    let ids: Vec<String> = before.iter().map(|a| a.id.clone()).collect();

    with_state({
        let doc_id = doc.doc_id.clone();
        let id = ids[0].clone();
        move |st| {
            registry::mutate(
                st,
                &doc_id,
                MutateOpts::new("undo.annotEdit", ChangeReason::Edit).page(0),
                |d| {
                    annot::update::update(
                        d,
                        0,
                        &id,
                        &seepdf_lib::ipc::types::AnnotPatch {
                            contents: Some("moved".into()),
                            ..Default::default()
                        },
                    )
                },
            )
        }
    })
    .expect("first edit");
    for redo in [false, true] {
        with_state({
            let doc_id = doc.doc_id.clone();
            move |st| registry::undo(st, &doc_id, redo)
        })
        .expect("undo / redo");
        let now: Vec<String> = list(&doc.doc_id, 0).iter().map(|a| a.id.clone()).collect();
        assert_eq!(
            now, ids,
            "annotation ids are stable across undo (redo: {redo})"
        );
    }
}

// ---------------------------------------------------------------------------------------
// v0.3 pkg4-annotations-stamps-objects
// ---------------------------------------------------------------------------------------

mod pkg4 {
    use super::*;
    use seepdf_lib::engine::annot::create::{create_in, letterbox};
    use seepdf_lib::engine::annot::update::update_in;
    use seepdf_lib::engine::pages::boxes;
    use seepdf_lib::ipc::types::{
        AnnotPatch, CalloutSpec, MeasureUnit, PageSelection, PolySpec, ResizeMode, ResizeTarget,
        StampShape,
    };

    fn make(doc_id: &str, page: u16, spec: AnnotSpec, author: Option<&'static str>) -> String {
        let doc_id = doc_id.to_string();
        with_state(move |st| create_in(st, &doc_id, page, &spec, None, author)).expect("create_in")
    }

    fn patch(doc_id: &str, page: u16, id: &str, p: AnnotPatch) -> Annot {
        let (doc_id, id) = (doc_id.to_string(), id.to_string());
        with_state(move |st| update_in(st, &doc_id, page, &id, &p)).expect("update_in")
    }

    fn undo(doc_id: &str) {
        let doc_id = doc_id.to_string();
        with_state(move |st| registry::undo(st, &doc_id, false)).expect("undo");
    }

    fn undo_depth(doc_id: &str) -> usize {
        let doc_id = doc_id.to_string();
        with_doc(&doc_id, |d| Ok(d.history.undo_depth())).unwrap()
    }

    /// The annotation dictionary named `id` on page 0 of `bytes`, read with lopdf.
    fn lopdf_dict(bytes: &[u8], id: &str) -> lopdf::Dictionary {
        let doc = lopdf::Document::load_mem(bytes).expect("lopdf parse");
        let page = *doc.get_pages().get(&1).unwrap();
        let annots = match doc.get_dictionary(page).unwrap().get(b"Annots").unwrap() {
            lopdf::Object::Array(a) => a.clone(),
            lopdf::Object::Reference(r) => doc.get_object(*r).unwrap().as_array().unwrap().clone(),
            _ => panic!("no /Annots"),
        };
        for entry in annots {
            let dict = match entry {
                lopdf::Object::Reference(r) => doc.get_dictionary(r).unwrap().clone(),
                lopdf::Object::Dictionary(d) => d,
                _ => continue,
            };
            let nm = dict
                .get(b"NM")
                .ok()
                .and_then(|o| lopdf::decode_text_string(o).ok());
            if nm.as_deref() == Some(id) {
                return dict;
            }
        }
        panic!("annotation {id} not found with lopdf");
    }

    fn names(dict: &lopdf::Dictionary, key: &[u8]) -> Vec<String> {
        match dict.get(key) {
            Ok(lopdf::Object::Array(a)) => a
                .iter()
                .filter_map(|o| o.as_name().ok())
                .map(|n| String::from_utf8_lossy(n).to_string())
                .collect(),
            Ok(lopdf::Object::Name(n)) => vec![String::from_utf8_lossy(n).to_string()],
            _ => Vec::new(),
        }
    }

    fn nums(dict: &lopdf::Dictionary, key: &[u8]) -> Vec<f32> {
        dict.get(key)
            .unwrap()
            .as_array()
            .unwrap()
            .iter()
            .map(|o| match o {
                lopdf::Object::Integer(i) => *i as f32,
                lopdf::Object::Real(r) => *r,
                _ => f32::NAN,
            })
            .collect()
    }

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 0.05
    }

    fn square(rect: Rect) -> AnnotSpec {
        AnnotSpec::Square(ShapeSpec {
            rect,
            color: [200, 0, 0],
            fill_color: None,
            width: 2.0,
            opacity: 1.0,
            dashed: false,
        })
    }

    /// A9: 설정 ▸ 작성자 is `/T` on a markup, a built-in stamp and a text stamp; blank = none.
    #[test]
    fn author_is_written_on_every_new_annotation() {
        let doc = open("tracemonkey.pdf");
        let hl = make(
            &doc.doc_id,
            0,
            AnnotSpec::Highlight(MarkupSpec {
                rects: vec![Rect::new(100.0, 600.0, 200.0, 612.0)],
                color: [255, 212, 0],
                opacity: 0.4,
                contents: None,
            }),
            Some("홍길동"),
        );
        let stamp = make(
            &doc.doc_id,
            0,
            AnnotSpec::Stamp(StampSpec {
                rect: Rect::new(300.0, 300.0, 364.0, 364.0),
                image: StampImage::Builtin {
                    builtin: "결재".into(),
                },
                rotate: None,
                signature: false,
            }),
            Some("  홍길동 "),
        );
        let anonymous = make(
            &doc.doc_id,
            0,
            square(Rect::new(50.0, 50.0, 90.0, 90.0)),
            Some("  "),
        );
        let saved = reopen(save_bytes(&doc.doc_id));
        let annots = list(&saved.doc_id, 0);
        assert_eq!(find(&annots, &hl).author.as_deref(), Some("홍길동"));
        assert_eq!(
            find(&annots, &stamp).author.as_deref(),
            Some("홍길동"),
            "trimmed"
        );
        assert_eq!(
            find(&annots, &anonymous).author,
            None,
            "a blank author writes no /T"
        );
    }

    /// A1: a centred text box stays centred through a colour edit (a rebuild), reads back
    /// `align`, and a patched alignment sticks.
    #[test]
    fn textbox_alignment_survives_edits() {
        let doc = open("tracemonkey.pdf");
        let id = make(
            &doc.doc_id,
            0,
            AnnotSpec::Textbox(TextBoxSpec {
                rect: Rect::new(100.0, 400.0, 300.0, 440.0),
                text: "가운데 정렬".into(),
                font_size: 14.0,
                color: [0, 0, 0],
                align: TextAlign::Center,
                fill_color: None,
            }),
            None,
        );
        assert_eq!(
            find(&list(&doc.doc_id, 0), &id).align,
            Some(TextAlign::Center)
        );
        patch(
            &doc.doc_id,
            0,
            &id,
            AnnotPatch {
                color: Some([0, 0, 200]),
                ..AnnotPatch::default()
            },
        );
        let after = list(&doc.doc_id, 0);
        let a = find(&after, &id);
        assert_eq!(
            a.align,
            Some(TextAlign::Center),
            "a colour edit keeps the alignment"
        );
        assert_eq!(a.color, [0, 0, 200]);
        patch(
            &doc.doc_id,
            0,
            &id,
            AnnotPatch {
                align: Some(TextAlign::Right),
                ..AnnotPatch::default()
            },
        );
        let saved = reopen(save_bytes(&doc.doc_id));
        let a = find(&list(&saved.doc_id, 0), &id).clone();
        assert_eq!(a.align, Some(TextAlign::Right), "round-trips through save");
        assert_eq!(a.text.as_deref(), Some("가운데 정렬"));
    }

    /// A1: heads on a SeePDF ink line turn it into an arrow (both ends), and on a real `/Line`
    /// they become `/LE [/OpenArrow /OpenArrow]`.
    #[test]
    fn heads_patch_turns_a_line_into_an_arrow() {
        let doc = open("tracemonkey.pdf");
        // The legacy Ink line (what a signed file that still saves incrementally gets, or an
        // encrypted one whose encryption cannot be rewritten).
        let ink = create(
            &doc.doc_id,
            0,
            AnnotSpec::Line(LineSpec {
                p1: [80.0, 110.0],
                p2: [380.0, 110.0],
                color: [0, 0, 0],
                width: 2.0,
                opacity: 1.0,
                heads: None,
                measure: None,
                dashed: false,
            }),
        );
        assert_eq!(
            find(&list(&doc.doc_id, 0), &ink).heads,
            Some([false, false])
        );
        patch(
            &doc.doc_id,
            0,
            &ink,
            AnnotPatch {
                heads: Some([true, true]),
                ..AnnotPatch::default()
            },
        );
        let a = find(&list(&doc.doc_id, 0), &ink).clone();
        assert_eq!(a.kind, AnnotKind::Arrow);
        assert_eq!(a.heads, Some([true, true]));
        assert_eq!(
            a.ink_paths.as_ref().unwrap().len(),
            5,
            "segment + two heads"
        );

        // The real /Line.
        let real = make(
            &doc.doc_id,
            0,
            AnnotSpec::Line(LineSpec {
                p1: [80.0, 200.0],
                p2: [380.0, 200.0],
                color: [0, 0, 0],
                width: 2.0,
                opacity: 1.0,
                heads: None,
                measure: None,
                dashed: false,
            }),
            None,
        );
        let before = find(&list(&doc.doc_id, 0), &real).clone();
        assert_eq!(
            (before.subtype.as_str(), before.kind),
            ("Line", AnnotKind::Line)
        );
        patch(
            &doc.doc_id,
            0,
            &real,
            AnnotPatch {
                heads: Some([true, true]),
                ..AnnotPatch::default()
            },
        );
        let bytes = save_bytes(&doc.doc_id);
        let dict = lopdf_dict(&bytes, &real);
        assert_eq!(names(&dict, b"LE"), ["OpenArrow", "OpenArrow"]);
        let saved = reopen(bytes);
        let a = find(&list(&saved.doc_id, 0), &real).clone();
        assert_eq!((a.kind, a.heads), (AnnotKind::Arrow, Some([true, true])));
    }

    /// A10: 인쇄 off clears the `/F` Print bit (and survives a text-box rebuild and a save).
    #[test]
    fn printed_flag_round_trips() {
        let doc = open("tracemonkey.pdf");
        let sq = make(
            &doc.doc_id,
            0,
            square(Rect::new(100.0, 100.0, 200.0, 180.0)),
            None,
        );
        let tb = make(
            &doc.doc_id,
            0,
            AnnotSpec::Textbox(TextBoxSpec {
                rect: Rect::new(100.0, 400.0, 300.0, 440.0),
                text: "인쇄 안 함".into(),
                font_size: 12.0,
                color: [0, 0, 0],
                align: TextAlign::Left,
                fill_color: None,
            }),
            None,
        );
        assert!(find(&list(&doc.doc_id, 0), &sq).printed);
        for id in [&sq, &tb] {
            patch(
                &doc.doc_id,
                0,
                id,
                AnnotPatch {
                    printed: Some(false),
                    ..AnnotPatch::default()
                },
            );
        }
        // A rebuild (new text) keeps the flag.
        patch(
            &doc.doc_id,
            0,
            &tb,
            AnnotPatch {
                text: Some("바뀐 글".into()),
                ..AnnotPatch::default()
            },
        );
        let saved = reopen(save_bytes(&doc.doc_id));
        let annots = list(&saved.doc_id, 0);
        assert!(!find(&annots, &sq).printed);
        let t = find(&annots, &tb);
        assert!(!t.printed, "the rebuilt text box is still not printed");
        assert_eq!(t.text.as_deref(), Some("바뀐 글"));
        patch(
            &saved.doc_id,
            0,
            &sq,
            AnnotPatch {
                printed: Some(true),
                ..AnnotPatch::default()
            },
        );
        assert!(find(&list(&saved.doc_id, 0), &sq).printed);
    }

    /// A2: real `/Line` (measuring), `/Polygon` (cloudy, filled), `/PolyLine` and a
    /// `FreeText` callout — each visible, one undo step, and read back by PDFium with its
    /// subtype and geometry after a save; the dictionaries hold what other viewers need.
    #[test]
    fn lopdf_kinds_round_trip() {
        let doc = open("tracemonkey.pdf");
        let probe = Rect::new(40.0, 40.0, 580.0, 360.0);
        let before = render_rect(&doc.doc_id, 0, probe);
        let depth = undo_depth(&doc.doc_id);

        let line = make(
            &doc.doc_id,
            0,
            AnnotSpec::Arrow(LineSpec {
                p1: [60.0, 60.0],
                p2: [276.0, 60.0],
                color: [200, 0, 0],
                width: 2.0,
                opacity: 1.0,
                heads: Some([false, true]),
                measure: Some(MeasureUnit::Mm),
                dashed: false,
            }),
            Some("홍길동"),
        );
        let polygon = make(
            &doc.doc_id,
            0,
            AnnotSpec::Polygon(PolySpec {
                vertices: vec![300.0, 80.0, 420.0, 80.0, 420.0, 180.0, 300.0, 180.0],
                color: [0, 90, 200],
                fill_color: Some([200, 220, 255]),
                width: 1.5,
                opacity: 0.8,
                cloudy: true,
                dashed: false,
                measure: None,
            }),
            None,
        );
        let polyline = make(
            &doc.doc_id,
            0,
            AnnotSpec::Polyline(PolySpec {
                vertices: vec![60.0, 200.0, 120.0, 260.0, 180.0, 200.0, 240.0, 260.0],
                color: [0, 140, 60],
                fill_color: None,
                width: 3.0,
                opacity: 1.0,
                cloudy: false,
                dashed: true,
                measure: Some(MeasureUnit::Pt),
            }),
            None,
        );
        assert_eq!(undo_depth(&doc.doc_id), depth + 3, "one undo step each");
        let callout = make(
            &doc.doc_id,
            0,
            AnnotSpec::Callout(CalloutSpec {
                rect: Rect::new(440.0, 260.0, 570.0, 300.0),
                text: "설명선 확인".into(),
                font_size: 12.0,
                color: [0, 0, 0],
                align: TextAlign::Left,
                fill_color: Some([255, 255, 200]),
                callout: vec![380.0, 220.0, 420.0, 280.0, 440.0, 280.0],
            }),
            None,
        );
        assert_eq!(
            undo_depth(&doc.doc_id),
            depth + 4,
            "a callout is one undo step too"
        );

        let after = render_rect(&doc.doc_id, 0, probe);
        assert!(changed_pixels(&before, &after) > 500, "all four are drawn");

        let bytes = save_bytes(&doc.doc_id);
        write_artifacts("pkg4-lopdf-kinds", &bytes, &doc.doc_id, 0);
        let l = lopdf_dict(&bytes, &line);
        assert_eq!(names(&l, b"Subtype"), ["Line"]);
        assert_eq!(nums(&l, b"L"), [60.0, 60.0, 276.0, 60.0]);
        assert_eq!(names(&l, b"LE"), ["None", "OpenArrow"]);
        assert_eq!(names(&l, b"IT"), ["LineDimension"]);
        assert!(l.get(b"AP").is_ok(), "own appearance");
        let p = lopdf_dict(&bytes, &polygon);
        assert_eq!(names(&p, b"Subtype"), ["Polygon"]);
        let be = p.get(b"BE").unwrap().as_dict().unwrap();
        assert_eq!(be.get(b"S").unwrap().as_name().unwrap(), b"C");
        assert_eq!(nums(&p, b"IC").len(), 3);
        let pl = lopdf_dict(&bytes, &polyline);
        assert_eq!(names(&pl, b"Subtype"), ["PolyLine"]);
        let bs = pl.get(b"BS").unwrap().as_dict().unwrap();
        assert_eq!(bs.get(b"S").unwrap().as_name().unwrap(), b"D");
        let c = lopdf_dict(&bytes, &callout);
        assert_eq!(names(&c, b"Subtype"), ["FreeText"]);
        assert_eq!(names(&c, b"IT"), ["FreeTextCallout"]);
        assert_eq!(nums(&c, b"CL"), [380.0, 220.0, 420.0, 280.0, 440.0, 280.0]);
        assert_eq!(nums(&c, b"RD").len(), 4);

        let saved = reopen(bytes);
        let annots = list(&saved.doc_id, 0);
        let a = find(&annots, &line);
        assert_eq!((a.kind, a.subtype.as_str()), (AnnotKind::Arrow, "Line"));
        assert_eq!(a.line_points, Some([60.0, 60.0, 276.0, 60.0]));
        assert_eq!(a.heads, Some([false, true]));
        assert_eq!(a.measure, Some(MeasureUnit::Mm));
        assert_eq!(a.author.as_deref(), Some("홍길동"));
        assert_eq!(a.color, [200, 0, 0]);
        assert_eq!(a.editable, Editability::Full);
        let g = find(&annots, &polygon);
        assert_eq!(g.kind, AnnotKind::Polygon);
        assert_eq!(
            g.vertices.as_deref(),
            Some(&[300.0, 80.0, 420.0, 80.0, 420.0, 180.0, 300.0, 180.0][..])
        );
        assert!(g.cloudy);
        assert_eq!(g.fill_color, Some([200, 220, 255]));
        let pl = find(&annots, &polyline);
        assert_eq!(pl.kind, AnnotKind::Polyline);
        assert_eq!(pl.vertices.as_ref().map(Vec::len), Some(8));
        assert!(pl.dashed);
        let c = find(&annots, &callout);
        assert_eq!(c.kind, AnnotKind::Callout);
        assert_eq!(c.text.as_deref(), Some("설명선 확인"));
        assert_eq!(c.callout.as_ref().map(Vec::len), Some(6));
        let r = c.rect;
        assert!(
            close(r.l, 440.0) && close(r.t, 300.0),
            "the rect is the text box: {r:?}"
        );

        // The callout was one step: one undo removes it entirely.
        undo(&doc.doc_id);
        assert!(!list(&doc.doc_id, 0).iter().any(|a| a.id == callout));
        assert!(list(&doc.doc_id, 0).iter().any(|a| a.id == polyline));
    }

    /// A2: editing a lopdf annotation redraws it **in place** — same object (a reply's `/IRT`
    /// still resolves), new geometry and colour — in one undo step; a callout's text edit keeps
    /// its leader line.
    #[test]
    fn lopdf_kinds_edit_in_place() {
        let doc = open("tracemonkey.pdf");
        let polygon = make(
            &doc.doc_id,
            0,
            AnnotSpec::Polygon(PolySpec {
                vertices: vec![300.0, 80.0, 420.0, 80.0, 360.0, 180.0],
                color: [0, 90, 200],
                fill_color: None,
                width: 1.5,
                opacity: 1.0,
                cloudy: false,
                dashed: false,
                measure: Some(MeasureUnit::Mm),
            }),
            None,
        );
        let reply = {
            let (doc_id, parent) = (doc.doc_id.clone(), polygon.clone());
            with_state(move |st| annot::reply::reply(st, &doc_id, 0, &parent, "답글", None))
                .expect("reply")
        };
        let depth = undo_depth(&doc.doc_id);
        patch(
            &doc.doc_id,
            0,
            &polygon,
            AnnotPatch {
                vertices: Some(vec![310.0, 90.0, 430.0, 90.0, 370.0, 190.0]),
                rect: Some(Rect::new(0.0, 0.0, 1.0, 1.0)),
                color: Some([200, 0, 0]),
                ..AnnotPatch::default()
            },
        );
        assert_eq!(undo_depth(&doc.doc_id), depth + 1);
        let annots = list(&doc.doc_id, 0);
        let g = find(&annots, &polygon);
        assert_eq!(
            g.vertices.as_deref(),
            Some(&[310.0, 90.0, 430.0, 90.0, 370.0, 190.0][..])
        );
        assert_eq!(g.color, [200, 0, 0]);
        assert!(
            g.rect.l < 310.0 && g.rect.r > 430.0,
            "the rect follows the vertices"
        );
        assert_eq!(
            find(&annots, &reply).in_reply_to.as_deref(),
            Some(polygon.as_str()),
            "the reply still points at the same dictionary"
        );
        undo(&doc.doc_id);
        let g = find(&list(&doc.doc_id, 0), &polygon).clone();
        assert_eq!(
            g.vertices.as_deref(),
            Some(&[300.0, 80.0, 420.0, 80.0, 360.0, 180.0][..])
        );

        let callout = make(
            &doc.doc_id,
            0,
            AnnotSpec::Callout(CalloutSpec {
                rect: Rect::new(440.0, 260.0, 570.0, 300.0),
                text: "처음".into(),
                font_size: 12.0,
                color: [0, 0, 0],
                align: TextAlign::Center,
                fill_color: None,
                callout: vec![380.0, 220.0, 440.0, 280.0],
            }),
            None,
        );
        let depth = undo_depth(&doc.doc_id);
        patch(
            &doc.doc_id,
            0,
            &callout,
            AnnotPatch {
                text: Some("바뀐 설명".into()),
                contents: Some("바뀐 설명".into()),
                ..AnnotPatch::default()
            },
        );
        assert_eq!(
            undo_depth(&doc.doc_id),
            depth + 1,
            "rebuild + leader = one step"
        );
        let c = find(&list(&doc.doc_id, 0), &callout).clone();
        assert_eq!(c.kind, AnnotKind::Callout);
        assert_eq!(c.text.as_deref(), Some("바뀐 설명"));
        assert_eq!(c.callout, Some(vec![380.0, 220.0, 440.0, 280.0]));
        assert_eq!(c.align, Some(TextAlign::Center));
    }

    /// T1: a picked image keeps its aspect — the engine letterboxes it inside a square rect —
    /// and `image_preview` reports the image's own size.
    #[test]
    fn stamp_image_is_letterboxed_and_previewed() {
        let dir = out_dir();
        let path = dir.join("wide-400x100.png");
        let img = image::RgbaImage::from_pixel(400, 100, image::Rgba([220, 0, 0, 255]));
        img.save(&path).expect("write png");

        let (w, h, png) = seepdf_lib::commands::images::preview_png(&path, 128).expect("preview");
        assert_eq!((w, h), (400, 100));
        let thumb = image::load_from_memory(&png).expect("preview decodes");
        assert_eq!(
            (thumb.width(), thumb.height()),
            (128, 32),
            "aspect kept, longest side 128"
        );

        let fit = letterbox(Rect::new(0.0, 0.0, 96.0, 96.0), 4.0);
        assert!(
            close(fit.l, 0.0) && close(fit.r, 96.0) && close(fit.b, 36.0) && close(fit.t, 60.0)
        );

        let doc = open("tracemonkey.pdf");
        let rect = Rect::new(250.0, 450.0, 346.0, 546.0);
        let top_band = Rect::new(252.0, 520.0, 344.0, 544.0);
        let middle = Rect::new(252.0, 490.0, 344.0, 506.0);
        let (top_before, mid_before) = (
            render_rect(&doc.doc_id, 0, top_band),
            render_rect(&doc.doc_id, 0, middle),
        );
        make(
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
            None,
        );
        let top_after = render_rect(&doc.doc_id, 0, top_band);
        let mid_after = render_rect(&doc.doc_id, 0, middle);
        assert!(
            changed_pixels(&mid_before, &mid_after) > 1000,
            "the image is in the middle"
        );
        assert_eq!(
            changed_pixels(&top_before, &top_after),
            0,
            "not stretched into the top of the square"
        );
    }

    /// T2: a text stamp expands `{{date}}` / `{{author}}` into its appearance; quick marks draw.
    #[test]
    fn text_stamp_expands_tokens_and_quick_marks_draw() {
        let doc = open("tracemonkey.pdf");
        let today = chrono::Local::now().format("%Y.%m.%d").to_string();
        let id = make(
            &doc.doc_id,
            0,
            AnnotSpec::Stamp(StampSpec {
                rect: Rect::new(100.0, 300.0, 260.0, 340.0),
                image: StampImage::Text {
                    text: "{{date}} {{author}}".into(),
                    color: [206, 32, 41],
                    shape: StampShape::Round,
                },
                rotate: None,
                signature: false,
            }),
            Some("홍길동"),
        );
        let expected = format!("{today} 홍길동");
        let a = find(&list(&doc.doc_id, 0), &id).clone();
        assert_eq!(a.kind, AnnotKind::Stamp);
        assert_eq!(a.contents, expected);
        // The appearance itself says it: flatten a saved copy (the AP becomes page content)
        // and extract the page text.
        let copy = reopen(save_bytes(&doc.doc_id));
        let said = {
            let doc_id = copy.doc_id.clone();
            with_doc(&doc_id, move |d| {
                let bindings = d.bindings();
                raw::page::flatten(bindings, d.page(0)?, raw::page::FlattenMode::NormalDisplay)?;
                d.invalidate_page(0);
                Ok(seepdf_lib::engine::text::layer::page_text(d, 0)?
                    .text
                    .clone())
            })
            .unwrap()
        };
        assert!(
            said.contains(&expected),
            "the stamp's appearance says {expected}"
        );

        let probe = Rect::new(380.0, 380.0, 560.0, 440.0);
        let before = render_rect(&doc.doc_id, 0, probe);
        for (i, mark) in ["check", "cross", "dot"].iter().enumerate() {
            let x = 390.0 + i as f32 * 60.0;
            let id = make(
                &doc.doc_id,
                0,
                AnnotSpec::Stamp(StampSpec {
                    rect: Rect::new(x, 390.0, x + 40.0, 430.0),
                    image: StampImage::Builtin {
                        builtin: mark.to_string(),
                    },
                    rotate: None,
                    signature: false,
                }),
                None,
            );
            assert_eq!(
                find(&list(&doc.doc_id, 0), &id).stamp_kind.as_deref(),
                Some(*mark)
            );
        }
        let after = render_rect(&doc.doc_id, 0, probe);
        assert!(changed_pixels(&before, &after) > 300, "the marks are drawn");
    }

    /// A8: resizing a page maps a third-party `/Line`'s `/L` with the same matrix as its
    /// `/Rect` — one undo step.
    #[test]
    fn resize_maps_a_foreign_line() {
        let doc = open("annotation-line.pdf");
        let line = list(&doc.doc_id, 0)
            .into_iter()
            .find(|a| a.subtype == "Line")
            .expect("a Line");
        let (r0, p0) = (line.rect, line.line_points.unwrap());
        let depth = undo_depth(&doc.doc_id);
        {
            let doc_id = doc.doc_id.clone();
            with_state(move |st| {
                boxes::resize_pages(
                    st,
                    &doc_id,
                    &PageSelection::List(vec![0]),
                    ResizeTarget::Size { w: 300.0, h: 400.0 },
                    ResizeMode::ScaleContent,
                )
            })
            .expect("resize");
        }
        assert_eq!(
            undo_depth(&doc.doc_id),
            depth + 1,
            "resize + lopdf pass = one step"
        );
        let moved = list(&doc.doc_id, 0)
            .into_iter()
            .find(|a| a.id == line.id)
            .expect("same line");
        let (r1, p1) = (moved.rect, moved.line_points.unwrap());
        let s = r1.width() / r0.width();
        assert!(
            (r1.height() / r0.height() - s).abs() < 0.01,
            "uniform scale"
        );
        assert!(s < 1.0, "the page got smaller");
        for (x0, y0, x1, y1) in [(p0[0], p0[1], p1[0], p1[1]), (p0[2], p0[3], p1[2], p1[3])] {
            assert!(
                close(x1, r1.l + (x0 - r0.l) * s),
                "x mapped like the rect: {p0:?} → {p1:?}"
            );
            assert!(
                close(y1, r1.b + (y0 - r0.b) * s),
                "y mapped like the rect: {p0:?} → {p1:?}"
            );
        }
        undo(&doc.doc_id);
        let back = list(&doc.doc_id, 0)
            .into_iter()
            .find(|a| a.subtype == "Line")
            .unwrap();
        assert_eq!(back.line_points, Some(p0));
    }

    fn batch(
        doc_id: &str,
        ops: Vec<seepdf_lib::ipc::types::AnnotOp>,
    ) -> Result<Vec<String>, seepdf_lib::ipc::EngineError> {
        let doc_id = doc_id.to_string();
        with_state(move |st| {
            seepdf_lib::engine::annot::update::batch_in(st, &doc_id, 0, &ops, Some("Batch"))
        })
    }

    fn ink(paths: Vec<Vec<f32>>, width: f32) -> AnnotSpec {
        AnnotSpec::Ink(InkSpec {
            paths,
            color: [0, 0, 255],
            width,
            opacity: 1.0,
        })
    }

    /// Verification round 1 (A3): one partial-eraser scrub across several strokes — the
    /// patches of the split strokes and the delete of an erased one — is ONE undo step.
    #[test]
    fn batch_partial_erase_is_one_undo_step() {
        use seepdf_lib::ipc::types::AnnotOp;
        let doc = open("tracemonkey.pdf");
        let a = make(
            &doc.doc_id,
            0,
            ink(vec![vec![80.0, 100.0, 120.0, 100.0]], 2.0),
            None,
        );
        let b = make(
            &doc.doc_id,
            0,
            ink(vec![vec![80.0, 120.0, 120.0, 120.0]], 2.0),
            None,
        );
        let c = make(
            &doc.doc_id,
            0,
            ink(vec![vec![98.0, 110.0, 102.0, 110.0]], 2.0),
            None,
        );
        let before = list(&doc.doc_id, 0);
        let depth = undo_depth(&doc.doc_id);
        let split = |y: f32| vec![vec![80.0, y, 95.0, y], vec![105.0, y, 120.0, y]];
        let created = batch(
            &doc.doc_id,
            vec![
                AnnotOp::Update {
                    id: a.clone(),
                    patch: AnnotPatch {
                        paths: Some(split(100.0)),
                        ..AnnotPatch::default()
                    },
                },
                AnnotOp::Update {
                    id: b.clone(),
                    patch: AnnotPatch {
                        paths: Some(split(120.0)),
                        ..AnnotPatch::default()
                    },
                },
                AnnotOp::Delete {
                    ids: vec![c.clone()],
                },
            ],
        )
        .expect("batch");
        assert!(created.is_empty());
        let after = list(&doc.doc_id, 0);
        let paths_of = |l: &[Annot], id: &str| {
            l.iter()
                .find(|x| x.id == id)
                .and_then(|x| x.ink_paths.clone())
                .map(|p| p.len())
        };
        assert_eq!(paths_of(&after, &a), Some(2));
        assert_eq!(paths_of(&after, &b), Some(2));
        assert!(after.iter().all(|x| x.id != c), "the erased stroke is gone");
        let label = with_doc(&doc.doc_id, |d| Ok(d.history.undo_label())).unwrap();
        assert_eq!(undo_depth(&doc.doc_id), depth + 1, "one undo step");
        assert_eq!(label.as_deref(), Some("undo.annotEdit"));
        undo(&doc.doc_id);
        let back = list(&doc.doc_id, 0);
        assert_eq!(paths_of(&back, &a), Some(1));
        assert_eq!(paths_of(&back, &b), Some(1));
        assert_eq!(
            back.len(),
            before.len(),
            "one undo brings the whole scrub back"
        );
        assert_eq!(undo_depth(&doc.doc_id), depth);
    }

    /// Verification round 1 (A4): a pen stroke split into pressure bands is created — and
    /// undone — as one step; `created` names the pieces in order, all with 작성자.
    #[test]
    fn batch_pressure_stroke_is_one_undo_step() {
        use seepdf_lib::ipc::types::AnnotOp;
        let doc = open("tracemonkey.pdf");
        let before = list(&doc.doc_id, 0).len();
        let depth = undo_depth(&doc.doc_id);
        let ops = [(0.8, 1.0), (1.0, 2.0), (1.3, 3.0)]
            .iter()
            .map(|&(w, k)| AnnotOp::Create {
                spec: ink(vec![vec![100.0 * k, 300.0, 100.0 * k + 90.0, 310.0]], w),
                id: None,
            })
            .collect();
        let created = batch(&doc.doc_id, ops).expect("batch");
        assert_eq!(created.len(), 3);
        let after = list(&doc.doc_id, 0);
        for id in &created {
            let a = after.iter().find(|a| &a.id == id).expect("created");
            assert_eq!(a.author.as_deref(), Some("Batch"));
        }
        assert_eq!(undo_depth(&doc.doc_id), depth + 1);
        let label = with_doc(&doc.doc_id, |d| Ok(d.history.undo_label())).unwrap();
        assert_eq!(label.as_deref(), Some("undo.annotCreate"));
        undo(&doc.doc_id);
        assert_eq!(
            list(&doc.doc_id, 0).len(),
            before,
            "one undo removes every band"
        );
        // …and one redo brings all three back, with their ids.
        let id = doc.doc_id.clone();
        with_state(move |st| registry::undo(st, &id, true)).expect("redo");
        let redone = list(&doc.doc_id, 0);
        assert!(created.iter().all(|id| redone.iter().any(|a| &a.id == id)));
    }

    /// A batch is all or nothing: an op that fails undoes the ones before it, and leaves no
    /// undo or redo entry behind.
    #[test]
    fn batch_failure_rolls_back_everything() {
        use seepdf_lib::ipc::types::AnnotOp;
        let doc = open("tracemonkey.pdf");
        let a = make(
            &doc.doc_id,
            0,
            ink(vec![vec![80.0, 100.0, 120.0, 100.0]], 2.0),
            None,
        );
        let before = list(&doc.doc_id, 0);
        let depth = undo_depth(&doc.doc_id);
        let err = batch(
            &doc.doc_id,
            vec![
                AnnotOp::Update {
                    id: a.clone(),
                    patch: AnnotPatch {
                        paths: Some(vec![vec![80.0, 100.0, 90.0, 100.0]]),
                        ..AnnotPatch::default()
                    },
                },
                AnnotOp::Create {
                    spec: ink(vec![vec![10.0, 10.0, 50.0, 50.0]], 1.0),
                    id: None,
                },
                AnnotOp::Delete {
                    ids: vec!["no-such-annotation".into()],
                },
            ],
        )
        .expect_err("the delete fails");
        assert!(!err.message.is_empty());
        let after = list(&doc.doc_id, 0);
        assert_eq!(after.len(), before.len(), "the create was rolled back");
        let a_after = after.iter().find(|x| x.id == a).unwrap();
        assert_eq!(
            a_after.ink_paths.as_ref().unwrap()[0].len(),
            4,
            "the update was rolled back"
        );
        let (d, redo) = with_doc(&doc.doc_id, |d| {
            Ok((d.history.undo_depth(), d.history.redo_depth()))
        })
        .unwrap();
        assert_eq!(
            (d, redo),
            (depth, 0),
            "no undo or redo entry is left behind"
        );
        let empty = batch(&doc.doc_id, Vec::new()).expect_err("empty");
        assert_eq!(empty.code, seepdf_lib::ipc::ErrorCode::InvalidArgument);
    }
}
