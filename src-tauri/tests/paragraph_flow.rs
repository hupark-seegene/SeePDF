//! Stage 9 — what follows an edited paragraph (`IPC_CONTRACT.md` §7.4b "flow"), real PDFium.
//!
//! The user's report: "글을 추가로 입력하면, 뒤에 문장은 겹쳐 보이는 문제가 있어" — a longer
//! paragraph was drawn over the text below it. These tests pin the push / overlap / fit flows,
//! the movable-set rules (column, obstacles, running footer, stamps), annotation moves, the
//! dry run and the one-step undo. Most run on PDFs generated here (Helvetica 12 pt, leading
//! 14.4), so the expectations are exact; one runs on tracemonkey's two-column first page.

mod common;
use common::*;

use seepdf_lib::engine::annot;
use seepdf_lib::engine::objects::{self, paragraph};
use seepdf_lib::engine::registry::{self, MutateOpts};
use seepdf_lib::engine::render::tiles;
use seepdf_lib::engine::save;
use seepdf_lib::engine::stamp;
use seepdf_lib::engine::text::layer;
use seepdf_lib::ipc::types::{
    AllPages, Annot, AnnotKind, AnnotSpec, ChangeReason, FlowBlocked, InkSpec, MarkupSpec,
    NotEditableReason, PageObject, PageObjectType, PageSelection, PageStampSource, PageStampSpec,
    ParagraphAlign, ParagraphEdit, ParagraphEditResult, ParagraphFlow, ParagraphProbe, Rect,
    ShapeSpec, StampAnchor, StampImage, StampRole, StampSpec, TextEditStrategy,
};
use seepdf_lib::ipc::{EngineError, ErrorCode};

// ---------------------------------------------------------------------------------------
// Generated fixtures
// ---------------------------------------------------------------------------------------

const LEADING: f32 = 14.4;
/// The clearance a push keeps between the new text and what follows: a line's leading minus
/// its size (14.4 − 12).
const CLEAR: f32 = LEADING - 12.0;

/// The line-grid bottom of a 12 pt paragraph whose last baseline is `last`: the flow measures
/// the growth, the gap and the free space from it, not from the ink.
fn grid(last: f32) -> f32 {
    last - 0.25 * 12.0
}

const PARA_A: [&str; 4] = [
    "The quick brown fox jumps over the lazy dog and keeps",
    "running across the wide green field until the sun com-",
    "pletely sets behind the distant hills and it gets quiet",
    "at last.",
];
const PARA_A_TEXT: &str = "The quick brown fox jumps over the lazy dog and keeps running across \
    the wide green field until the sun completely sets behind the distant hills and it gets \
    quiet at last.";
/// Appended to paragraph A: two to three more lines at 12 pt in A's box.
/// Appended to paragraph A: one to two more lines.
const ONE_MORE: &str = "Plus a few more words so that the paragraph needs one more line of text.";
const EXTRA: &str = "Then the stars come out one by one above the silent valley, the wind \
    turns cold and the fox curls up in its den to sleep until the morning light returns.";

/// `lines` as one Helvetica 12 pt paragraph with leading 14.4, first baseline at `(x, y)`.
fn para(x: f32, y: f32, lines: &[&str]) -> String {
    let mut s = format!("BT /F1 12 Tf {LEADING} TL {x} {y} Td\n");
    for (i, line) in lines.iter().enumerate() {
        if i > 0 {
            s.push_str("T* ");
        }
        s.push_str(&format!("({line}) Tj\n"));
    }
    s.push_str("ET\n");
    s
}

/// A one-page 612 × 792 PDF with `content`, fonts F1 = Helvetica, F2 = Helvetica-Bold, and
/// `widgets` (annotation dictionaries, `/P` = 3 0 R) listed as AcroForm fields.
fn pdf(content: &str, widgets: &[&str]) -> Vec<u8> {
    pdf_rotated(content, widgets, 0)
}

/// [`pdf`] with a `/Rotate` on the page.
fn pdf_rotated(content: &str, widgets: &[&str], rotate: u16) -> Vec<u8> {
    let first_annot = 7;
    let refs: String = (0..widgets.len())
        .map(|i| format!("{} 0 R ", first_annot + i))
        .collect();
    let catalog = if widgets.is_empty() {
        "<< /Type /Catalog /Pages 2 0 R >>".to_string()
    } else {
        format!(
            "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [{refs}] /DA (/Helv 0 Tf 0 g) \
             /DR << /Font << /Helv 5 0 R >> >> >> >>"
        )
    };
    let annots = if widgets.is_empty() {
        String::new()
    } else {
        format!(" /Annots [{refs}]")
    };
    let mut objects =
        vec![
        catalog,
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
        format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] \
             /Rotate {rotate} /Resources << /Font << /F1 5 0 R /F2 6 0 R >> >> \
             /Contents 4 0 R{annots} >>"
        ),
        format!("<< /Length {} >>\nstream\n{content}endstream", content.len()),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>"
            .to_string(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica-Bold /Encoding /WinAnsiEncoding >>"
            .to_string(),
    ];
    objects.extend(widgets.iter().map(|w| w.to_string()));
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, body) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{body}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(
        format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes(),
    );
    for off in offsets {
        out.extend_from_slice(format!("{off:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            objects.len() + 1
        )
        .as_bytes(),
    );
    out
}

/// Heading, paragraph A (700 → 656.8), paragraph C (620, 605.6), paragraph D (570), a rotated
/// line in the right margin and a page number at the bottom (the running footer).
fn flow_content() -> String {
    let mut c = String::from("BT /F2 18 Tf 72 740 Td (A Heading Line) Tj ET\n");
    c.push_str(&para(72.0, 700.0, &PARA_A));
    c.push_str("BT /F1 12 Tf 72 620 Td (Second paragraph) Tj ET\n");
    c.push_str("BT /F1 12 Tf 175 620 Td (starts here with enough words to wrap) Tj ET\n");
    c.push_str("BT /F1 12 Tf 72 605.6 Td (and ends here.) Tj ET\n");
    c.push_str("BT /F1 12 Tf 72 570 Td (Third paragraph follows here.) Tj ET\n");
    c.push_str("BT /F1 12 Tf 0 1 -1 0 560 300 Tm (Rotated words here) Tj ET\n");
    c.push_str("BT /F1 10 Tf 300 40 Td (1) Tj ET\n");
    c
}

fn open_bytes(bytes: Vec<u8>) -> TestDoc {
    let info = with_state(move |st| registry::open(st, None, bytes, None)).expect("open bytes");
    let doc_id = info.doc_id.clone();
    TestDoc { info, doc_id }
}

// ---------------------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------------------

fn probe(doc_id: &str, x: f32, y: f32) -> ParagraphProbe {
    let doc_id = doc_id.to_string();
    with_doc(&doc_id, move |d| paragraph::probe(d, 0, [x, y]))
        .expect("probe_paragraph")
        .unwrap_or_else(|| panic!("no paragraph at ({x}, {y})"))
}

fn generation(doc_id: &str) -> u32 {
    with_doc(doc_id, |d| Ok(d.generation)).unwrap()
}

fn objects_of(doc_id: &str) -> Vec<PageObject> {
    let doc_id = doc_id.to_string();
    with_doc(&doc_id, |d| Ok(objects::list(d, 0)?.objects)).unwrap()
}

fn annots_of(doc_id: &str) -> Vec<Annot> {
    let doc_id = doc_id.to_string();
    with_doc(&doc_id, |d| annot::list(d, 0)).unwrap()
}

fn edit_of(
    p: &ParagraphProbe,
    text: &str,
    flow: Option<ParagraphFlow>,
    dry_run: bool,
) -> ParagraphEdit {
    ParagraphEdit {
        object_ids: p.object_ids.clone(),
        text: text.to_string(),
        width: None,
        font_size_pt: None,
        color: None,
        align: None,
        flow,
        dry_run,
    }
}

fn run(
    doc_id: &str,
    expect: u32,
    edit: ParagraphEdit,
    allow: bool,
) -> Result<ParagraphEditResult, EngineError> {
    let doc_id = doc_id.to_string();
    with_state(move |st| paragraph::edit(st, &doc_id, 0, expect, edit, allow))
}

/// The serialised document with the trailer `/ID` masked (its second half is random per save).
fn bytes_of(doc_id: &str) -> Vec<u8> {
    let id = doc_id.to_string();
    let mut bytes = with_state(move |st| save::serialize(st, &id)).expect("serialize");
    let mut from = 0;
    while let Some(pos) = find(&bytes[from..], b"/ID") {
        let start = from + pos;
        let end = find(&bytes[start..], b"]").map_or(bytes.len(), |e| start + e + 1);
        for b in &mut bytes[start..end] {
            *b = b'#';
        }
        from = end;
    }
    bytes
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

fn page_text(doc_id: &str) -> String {
    let id = doc_id.to_string();
    let bytes = with_state(move |st| save::serialize(st, &id)).expect("serialize");
    let reopened = open_bytes(bytes);
    let id = reopened.doc_id.clone();
    let text = with_doc(&id, move |d| Ok(layer::page_text(d, 0)?.text.clone())).unwrap();
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn text_of(o: &PageObject) -> String {
    o.text.clone().unwrap_or_default()
}

/// The objects of `before` that survive an edit of `removed`, paired with their counterparts
/// in `after`: the edit removes the paragraph's objects and appends the new ones, so the
/// survivors keep their relative order at the front of the list.
fn survivors<'a>(
    before: &'a [PageObject],
    removed: &[u32],
    after: &'a [PageObject],
) -> Vec<(&'a PageObject, &'a PageObject)> {
    let kept: Vec<&PageObject> = before
        .iter()
        .filter(|o| !removed.contains(&o.object_id))
        .collect();
    assert!(
        after.len() >= kept.len(),
        "{} < {}",
        after.len(),
        kept.len()
    );
    kept.into_iter().zip(after.iter()).collect()
}

fn same_bits(a: &PageObject, b: &PageObject) -> bool {
    a.matrix
        .iter()
        .zip(&b.matrix)
        .all(|(x, y)| x.to_bits() == y.to_bits())
        && [a.rect.l, a.rect.b, a.rect.r, a.rect.t]
            .iter()
            .zip([b.rect.l, b.rect.b, b.rect.r, b.rect.t])
            .all(|(x, y)| x.to_bits() == y.to_bits())
}

fn moved_by(before: &PageObject, after: &PageObject, dy: f32) -> bool {
    (after.rect.t - (before.rect.t + dy)).abs() < 0.02
        && (after.rect.b - (before.rect.b + dy)).abs() < 0.02
        && (after.rect.l - before.rect.l).abs() < 0.02
        && (after.matrix[5] - (before.matrix[5] + dy)).abs() < 0.02
}

fn is_c_or_d(text: &str) -> bool {
    ["Second", "starts here", "and ends", "Third"]
        .iter()
        .any(|k| text.contains(k))
}

// ---------------------------------------------------------------------------------------
// (1) + (3) push / pull on a single column; the running footer stays
// ---------------------------------------------------------------------------------------

#[test]
fn flow_push_moves_what_follows_by_the_growth_and_pulls_it_back_up() {
    let doc = open_bytes(pdf(&flow_content(), &[]));
    let before = objects_of(&doc.doc_id);
    let a = probe(&doc.doc_id, 200.0, 690.0);
    assert_eq!(a.lines, 4);
    let long = format!("{PARA_A_TEXT} {EXTRA}");

    // The dry run reports the plan and changes nothing.
    let dry = run(
        &doc.doc_id,
        a.doc_generation,
        edit_of(&a, &long, None, true),
        false,
    )
    .expect("dry run");
    assert_eq!(generation(&doc.doc_id), a.doc_generation);
    assert!(dry.lines >= 6, "{}", dry.lines);
    let growth = (dry.lines - 4) as f32 * LEADING;
    assert!(
        (dry.shifted_pt - growth).abs() < 0.02,
        "{} vs {growth}",
        dry.shifted_pt
    );
    assert_eq!(dry.moved_objects, 4, "C's three objects and D's one");
    assert_eq!(dry.blocked, None);
    assert_eq!(dry.overflow_pt, 0.0);
    assert!(
        dry.objects.objects.is_empty(),
        "a dry run does not list the page again"
    );
    assert_eq!(dry.objects.doc_generation, a.doc_generation);
    assert_eq!(dry.past_bottom_pt, 0.0);
    let band = dry.moved_band.expect("content moves: the band is reported");
    assert!(
        band.t > a.rect.b && band.t < a.rect.b + 4.0 && band.l <= 72.5,
        "{band:?}"
    );
    assert!(before.len() > 4);

    // The real edit does exactly what the dry run said.
    let res = run(
        &doc.doc_id,
        a.doc_generation,
        edit_of(&a, &long, Some(ParagraphFlow::Push), false),
        false,
    )
    .expect("push");
    assert_eq!(
        (res.lines, res.shifted_pt, res.moved_objects),
        (dry.lines, dry.shifted_pt, dry.moved_objects)
    );
    assert_eq!(res.rect, dry.rect);
    assert_eq!(res.room_pt, dry.room_pt);
    let after = res.objects.objects.clone();
    let mut moved = 0;
    for (b, n) in survivors(&before, &a.object_ids, &after) {
        assert_eq!(b.object_type, n.object_type);
        assert_eq!(text_of(b), text_of(n));
        if is_c_or_d(&text_of(b)) {
            assert!(
                moved_by(b, n, -res.shifted_pt),
                "{:?} → {:?}",
                b.rect,
                n.rect
            );
            moved += 1;
        } else {
            // Heading, rotated margin text and the page number (the running footer).
            assert!(
                same_bits(b, n),
                "{:?} moved: {:?} → {:?}",
                text_of(b),
                b.rect,
                n.rect
            );
        }
    }
    assert_eq!(moved, 4);
    // No overlap: the new text ends above the moved paragraph C.
    let c_top = after
        .iter()
        .filter(|o| is_c_or_d(&text_of(o)))
        .map(|o| o.rect.t)
        .fold(f32::MIN, f32::max);
    assert!(
        res.rect.b > c_top,
        "new text {:?} vs C top {c_top}",
        res.rect
    );
    let text = page_text(&doc.doc_id);
    assert!(text.contains("quiet at last. Then the stars"), "{text}");
    assert!(text.contains("Second paragraph starts here"), "{text}");

    // Shorter: the content below is pulled up with the paragraph's bottom.
    let a2 = probe(&doc.doc_id, 80.0, 705.0);
    assert_eq!(a2.lines, res.lines);
    let before2 = objects_of(&doc.doc_id);
    let short = run(
        &doc.doc_id,
        a2.doc_generation,
        edit_of(&a2, "Short text now.", None, false),
        false,
    )
    .expect("shorter");
    assert_eq!(short.lines, 1);
    let pulled = (a2.lines - 1) as f32 * LEADING;
    assert!(
        (short.shifted_pt + pulled).abs() < 0.02,
        "{} vs -{pulled}",
        short.shifted_pt
    );
    assert_eq!(short.moved_objects, 4);
    for (b, n) in survivors(&before2, &a2.object_ids, &short.objects.objects) {
        if is_c_or_d(&text_of(b)) {
            assert!(moved_by(b, n, pulled), "{:?} → {:?}", b.rect, n.rect);
        } else {
            assert!(same_bits(b, n), "{:?} moved", text_of(b));
        }
    }
    // C is back one blank gap below the one-line paragraph: 700 − 14.4·3 + 14.4·… = the
    // original distance from A's last baseline (656.8 → 620 = 36.8) below the new last line.
    let c_first = short
        .objects
        .objects
        .iter()
        .find(|o| text_of(o).contains("Second"))
        .unwrap();
    assert!(
        (c_first.matrix[5] - (700.0 - 36.8)).abs() < 0.05,
        "{}",
        c_first.matrix[5]
    );
}

/// (3) A page number far below the body is the running footer: it is never moved and it is
/// in the way — the push stops above it (`blocked: obstacle`).
#[test]
fn flow_running_footer_is_never_moved_and_stops_the_push() {
    let mut c = para(72.0, 200.0, &PARA_A);
    c.push_str("BT /F1 12 Tf 72 120 Td (Closing line of the body.) Tj ET\n");
    c.push_str("BT /F1 10 Tf 280 40 Td (Page 1 of 1) Tj ET\n");
    let doc = open_bytes(pdf(&c, &[]));
    let before = objects_of(&doc.doc_id);
    let closing = before
        .iter()
        .find(|o| text_of(o).contains("Closing"))
        .unwrap()
        .clone();
    let footer = before
        .iter()
        .find(|o| text_of(o).contains("Page 1"))
        .unwrap()
        .clone();
    let a = probe(&doc.doc_id, 150.0, 190.0);
    assert_eq!(a.lines, 4);

    // Two more lines fit between the closing line and the footer.
    let two = format!("{PARA_A_TEXT} {EXTRA}");
    let dry = run(
        &doc.doc_id,
        a.doc_generation,
        edit_of(&a, &two, None, true),
        false,
    )
    .unwrap();
    let room = closing.rect.b - footer.rect.t;
    assert!(
        (dry.room_pt - room).abs() < 0.02,
        "{} vs {room}",
        dry.room_pt
    );
    assert_eq!(dry.blocked, None);

    // Much more text does not: blocked by the footer, pushed as far as it goes; the paragraph
    // spacing above the closing line (beyond a line's clearance, from A's line grid) absorbs
    // part of the rest.
    let long = format!("{PARA_A_TEXT} {EXTRA} {EXTRA} {EXTRA}");
    let res = run(
        &doc.doc_id,
        a.doc_generation,
        edit_of(&a, &long, None, false),
        false,
    )
    .unwrap();
    let growth = (res.lines - 4) as f32 * LEADING;
    let gap = grid(200.0 - 3.0 * LEADING) - closing.rect.t - CLEAR;
    assert!(growth > room + gap, "{growth} vs {room} + {gap}");
    assert_eq!(res.blocked, Some(FlowBlocked::Obstacle));
    assert!((res.room_pt - room).abs() < 0.02);
    assert!(
        (res.shifted_pt - room).abs() < 0.02,
        "pushed as far as the room goes"
    );
    let expect = growth - room - gap;
    assert!(
        (res.overflow_pt - expect).abs() < 0.05,
        "{} vs {expect}",
        res.overflow_pt
    );
    let after = res.objects.objects.clone();
    let footer_after = after
        .iter()
        .find(|o| text_of(o).contains("Page 1"))
        .unwrap();
    assert!(same_bits(&footer, footer_after), "the footer never moves");
    let closing_after = after
        .iter()
        .find(|o| text_of(o).contains("Closing"))
        .unwrap();
    assert!(moved_by(&closing, closing_after, -room));
}

/// On a `/Rotate 90` page, "below" is the paragraph's own reading direction (−y in user
/// space, where its lines are laid out), not the screen's.
#[test]
fn flow_rotated_page_pushes_along_the_text_direction() {
    let doc = open_bytes(pdf_rotated(&flow_content(), &[], 90));
    let before = objects_of(&doc.doc_id);
    let a = probe(&doc.doc_id, 200.0, 690.0);
    let res = run(
        &doc.doc_id,
        a.doc_generation,
        edit_of(&a, &format!("{PARA_A_TEXT} {EXTRA}"), None, false),
        false,
    )
    .unwrap();
    assert!((res.shifted_pt - (res.lines - 4) as f32 * LEADING).abs() < 0.02);
    assert_eq!(res.moved_objects, 4);
    for (b, n) in survivors(&before, &a.object_ids, &res.objects.objects) {
        if is_c_or_d(&text_of(b)) {
            assert!(
                moved_by(b, n, -res.shifted_pt),
                "{:?} → {:?}",
                b.rect,
                n.rect
            );
        } else {
            assert!(same_bits(b, n), "{:?} moved", text_of(b));
        }
    }
}

// ---------------------------------------------------------------------------------------
// (2) two columns — tracemonkey page 0
// ---------------------------------------------------------------------------------------

#[test]
fn flow_tracemonkey_right_column_moves_only_the_right_column() {
    let doc = open("tracemonkey.pdf");
    let before = objects_of(&doc.doc_id);
    // "Compilers for statically typed languages rely on type information…" (12 lines).
    let p = probe(&doc.doc_id, 420.0, 400.0);
    assert!(p.rect.l > 305.0 && p.lines >= 10, "{p:?}");
    let text = format!(
        "{} This sentence is appended so that the paragraph grows by a line or two.",
        p.text
    );
    let res = run(
        &doc.doc_id,
        p.doc_generation,
        edit_of(&p, &text, None, false),
        false,
    )
    .expect("push in the right column");
    assert!(res.shifted_pt > 5.0, "{res:?}");
    assert_eq!(res.blocked, None);
    assert_eq!(res.overflow_pt, 0.0);
    let tol = 0.25 * p.font_size_pt;
    let mut moved = 0u32;
    let mut left = 0;
    for (b, n) in survivors(&before, &p.object_ids, &res.objects.objects) {
        assert_eq!(text_of(b), text_of(n));
        let right_below = b.rect.l > 305.0 && b.rect.t <= p.rect.b + tol;
        if right_below {
            assert!(
                moved_by(b, n, -res.shifted_pt),
                "{:?}: {:?} → {:?}",
                text_of(b),
                b.rect,
                n.rect
            );
            moved += 1;
        } else {
            // Left column (and everything above) byte-for-byte unchanged.
            assert!(
                same_bits(b, n),
                "{:?}: {:?} → {:?}",
                text_of(b),
                b.rect,
                n.rect
            );
            if b.rect.r < 300.0 {
                left += 1;
            }
        }
    }
    assert!(left > 40, "{left} left-column objects checked");
    assert_eq!(moved, res.moved_objects);
    assert!(
        moved >= 20,
        "the three right-column paragraphs below: {moved}"
    );
}

// ---------------------------------------------------------------------------------------
// (4) obstacles and the page bottom, (5) fit
// ---------------------------------------------------------------------------------------

/// A + C with a full-width figure (x 72 … 540, top 595) just below C: C may move less than
/// one line.
fn obstacle_content() -> String {
    let mut c = String::from("BT /F2 18 Tf 72 740 Td (A Heading Line) Tj ET\n");
    c.push_str(&para(72.0, 700.0, &PARA_A));
    c.push_str(&para(
        72.0,
        620.0,
        &["Second paragraph starts here with words.", "and ends here."],
    ));
    c.push_str("0.5 g 72 445 468 150 re f 0 g\n");
    c.push_str(&para(72.0, 400.0, &["Text below the figure never moves."]));
    c
}

#[test]
fn flow_full_width_obstacle_stops_the_push() {
    let doc = open_bytes(pdf(&obstacle_content(), &[]));
    let before = objects_of(&doc.doc_id);
    let c_bottom = before
        .iter()
        .filter(|o| text_of(o).contains("Second") || text_of(o).contains("and ends"))
        .map(|o| o.rect.b)
        .fold(f32::MAX, f32::min);
    let figure = before
        .iter()
        .find(|o| o.object_type == seepdf_lib::ipc::types::PageObjectType::Path)
        .unwrap()
        .clone();
    assert!((figure.rect.t - 595.0).abs() < 0.01, "{:?}", figure.rect);
    let room = c_bottom - 595.0;
    assert!(room > 3.0 && room < 14.0, "{room}");

    let a = probe(&doc.doc_id, 200.0, 690.0);
    // One more line fits: C moves by the room and the paragraph spacing above C takes the
    // rest — nothing overlaps, so the push is not blocked (the verifier's report).
    let one = format!("{PARA_A_TEXT} {ONE_MORE}");
    let dry = run(
        &doc.doc_id,
        a.doc_generation,
        edit_of(&a, &one, None, true),
        false,
    )
    .unwrap();
    assert_eq!(dry.lines, 5, "{dry:?}");
    assert_eq!((dry.blocked, dry.overflow_pt), (None, 0.0), "{dry:?}");
    assert!((dry.shifted_pt - room).abs() < 0.02);
    let overlap = run(
        &doc.doc_id,
        a.doc_generation,
        edit_of(&a, &one, Some(ParagraphFlow::Overlap), true),
        false,
    )
    .unwrap();
    assert_eq!(overlap.overflow_pt, 0.0, "the gap alone holds one line");

    // Three more lines do not.
    let longer = format!("{PARA_A_TEXT} {ONE_MORE} {EXTRA}");
    let dry = run(
        &doc.doc_id,
        a.doc_generation,
        edit_of(&a, &longer, None, true),
        false,
    )
    .unwrap();
    assert!(dry.lines >= 7, "{}", dry.lines);
    let growth = (dry.lines - 4) as f32 * LEADING;
    let c_top = before
        .iter()
        .filter(|o| text_of(o).contains("Second"))
        .map(|o| o.rect.t)
        .fold(f32::MIN, f32::max);
    // The gap is measured from the paragraph's line grid to C's top, less a line's clearance.
    let gap = grid(700.0 - 3.0 * LEADING) - c_top - CLEAR;
    assert_eq!(dry.blocked, Some(FlowBlocked::Obstacle));
    assert!(
        (dry.room_pt - room).abs() < 0.02,
        "{} vs {room}",
        dry.room_pt
    );
    assert!(
        (dry.overflow_pt - (growth - room - gap)).abs() < 0.02,
        "{} vs {}",
        dry.overflow_pt,
        growth - room - gap
    );
    assert!((dry.shifted_pt - room).abs() < 0.02);

    // Committing the blocked push moves C by the room; the figure and the text below it stay.
    let res = run(
        &doc.doc_id,
        a.doc_generation,
        edit_of(&a, &longer, None, false),
        false,
    )
    .unwrap();
    assert_eq!(res.blocked, Some(FlowBlocked::Obstacle));
    // …and the new text's ink ends `overflowPt` (± the descent, less the clearance it counts)
    // below C's moved top.
    let c_top_after = res
        .objects
        .objects
        .iter()
        .filter(|o| text_of(o).contains("Second"))
        .map(|o| o.rect.t)
        .fold(f32::MIN, f32::max);
    let ink_overlap = c_top_after - res.rect.b;
    assert!(
        (ink_overlap + CLEAR - res.overflow_pt).abs() < 3.5,
        "{ink_overlap} vs {}",
        res.overflow_pt
    );
    for (b, n) in survivors(&before, &a.object_ids, &res.objects.objects) {
        if text_of(b).contains("Second") || text_of(b).contains("and ends") {
            assert!(moved_by(b, n, -room), "{:?} → {:?}", b.rect, n.rect);
        } else {
            assert!(
                same_bits(b, n),
                "{:?} {:?} moved",
                b.object_type,
                text_of(b)
            );
        }
    }
}

#[test]
fn flow_page_bottom_blocks() {
    let mut c = String::from("BT /F2 18 Tf 72 740 Td (A Heading Line) Tj ET\n");
    c.push_str(&para(72.0, 100.0, &PARA_A));
    let doc = open_bytes(pdf(&c, &[]));
    let a = probe(&doc.doc_id, 150.0, 90.0);
    assert_eq!(a.lines, 4);
    let long = format!("{PARA_A_TEXT} {EXTRA} {EXTRA}");
    let dry = run(
        &doc.doc_id,
        a.doc_generation,
        edit_of(&a, &long, None, true),
        false,
    )
    .unwrap();
    // Nothing below: the paragraph's line grid may grow down to 18 pt above the crop box edge.
    let room = grid(100.0 - 3.0 * LEADING) - 18.0;
    assert!(
        (dry.room_pt - room).abs() < 0.02,
        "{} vs {room}",
        dry.room_pt
    );
    assert_eq!(dry.blocked, Some(FlowBlocked::PageBottom));
    let growth = (dry.lines - 4) as f32 * LEADING;
    // Nothing is overlapped: the text runs past the page's bottom margin instead.
    assert_eq!(dry.overflow_pt, 0.0);
    assert!(
        (dry.past_bottom_pt - (growth - room)).abs() < 0.05,
        "{} vs {}",
        dry.past_bottom_pt,
        growth - room
    );
    assert_eq!(
        (dry.shifted_pt, dry.moved_objects, dry.moved_band),
        (0.0, 0, None),
        "nothing to move"
    );
    let over = run(
        &doc.doc_id,
        a.doc_generation,
        edit_of(&a, &long, Some(ParagraphFlow::Overlap), true),
        false,
    )
    .unwrap();
    assert_eq!(over.overflow_pt, 0.0);
    assert!(over.past_bottom_pt > 0.0);
}

#[test]
fn flow_fit_shrinks_to_the_original_height() {
    let doc = open_bytes(pdf(&obstacle_content(), &[]));
    let before = objects_of(&doc.doc_id);
    let a = probe(&doc.doc_id, 200.0, 690.0);
    let longer = format!("{PARA_A_TEXT} {ONE_MORE}");
    let dry = run(
        &doc.doc_id,
        a.doc_generation,
        edit_of(&a, &longer, Some(ParagraphFlow::Fit), true),
        false,
    )
    .unwrap();
    let scale = dry.fit_scale.expect("fit reports its scale");
    assert!((0.7..1.0).contains(&scale), "{scale}");
    assert_eq!(dry.overflow_pt, 0.0);
    assert_eq!(
        (dry.shifted_pt, dry.moved_objects, dry.blocked),
        (0.0, 0, None)
    );
    assert_eq!(dry.lines, 4);

    let res = run(
        &doc.doc_id,
        a.doc_generation,
        edit_of(&a, &longer, Some(ParagraphFlow::Fit), false),
        false,
    )
    .unwrap();
    assert_eq!(res.fit_scale, Some(scale));
    assert!(
        res.rect.b >= a.rect.b - 1.0,
        "fits the original height: {:?} vs {:?}",
        res.rect,
        a.rect
    );
    // Nothing else moved.
    for (b, n) in survivors(&before, &a.object_ids, &res.objects.objects) {
        assert!(same_bits(b, n), "{:?} moved", text_of(b));
    }
    let again = probe(&doc.doc_id, 80.0, 700.0);
    assert!(
        (again.font_size_pt - 12.0 * scale).abs() < 0.05,
        "{}",
        again.font_size_pt
    );
    assert!(
        (again.line_height_pt - LEADING * scale).abs() < 0.05,
        "{}",
        again.line_height_pt
    );
    assert!(page_text(&doc.doc_id).contains(&format!("quiet at last. {ONE_MORE}")));

    // The same edit with `overlap` reports how far the text runs over C and moves nothing.
    let b = probe(&doc.doc_id, 80.0, 700.0);
    let over = run(
        &doc.doc_id,
        b.doc_generation,
        edit_of(
            &b,
            &format!("{PARA_A_TEXT} {EXTRA} {EXTRA}"),
            Some(ParagraphFlow::Overlap),
            true,
        ),
        false,
    )
    .unwrap();
    assert!(
        over.overflow_pt > 0.0 && over.shifted_pt == 0.0 && over.blocked.is_none(),
        "{over:?}"
    );
    // overflow = growth of the line grid − the free gap down to C's top.
    let c_top = before
        .iter()
        .filter(|o| text_of(o).contains("Second"))
        .map(|o| o.rect.t)
        .fold(f32::MIN, f32::max);
    let grid_bottom = |first: f32, lines: u32, leading: f32, size: f32| {
        first - (lines - 1) as f32 * leading - 0.25 * size
    };
    let growth = grid_bottom(700.0, b.lines, b.line_height_pt, b.font_size_pt)
        - grid_bottom(700.0, over.lines, b.line_height_pt, b.font_size_pt);
    let clear = b.line_height_pt - b.font_size_pt;
    let free = grid_bottom(700.0, b.lines, b.line_height_pt, b.font_size_pt) - c_top - clear;
    assert!(
        (over.overflow_pt - (growth - free)).abs() < 0.05,
        "{} vs {}",
        over.overflow_pt,
        growth - free
    );
}

// ---------------------------------------------------------------------------------------
// (6) dry run, (7) annotations / widgets / stamps, (8) undo, (9) stale
// ---------------------------------------------------------------------------------------

#[test]
fn flow_dry_run_leaves_the_document_byte_identical() {
    let doc = open_bytes(pdf(&flow_content(), &[]));
    let g0 = generation(&doc.doc_id);
    let bytes0 = bytes_of(&doc.doc_id);
    let depth0 = with_doc(&doc.doc_id, |d| Ok(d.history.undo_depth())).unwrap();
    let a = probe(&doc.doc_id, 200.0, 690.0);
    let long = format!("{PARA_A_TEXT} {EXTRA}");
    let korean =
        "안녕하세요 반갑습니다. 한글 문단 편집을 시험합니다. 줄바꿈은 띄어쓰기에서 일어나야 \
                  합니다. 조금 더 길게 써서 아래 내용이 밀리는지 확인합니다.";

    let push = run(
        &doc.doc_id,
        a.doc_generation,
        edit_of(&a, &long, None, true),
        false,
    )
    .unwrap();
    let fit = run(
        &doc.doc_id,
        a.doc_generation,
        edit_of(&a, &long, Some(ParagraphFlow::Fit), true),
        false,
    )
    .unwrap();
    // A substitute font is measured in a throwaway document: nothing is embedded here.
    let refused = run(
        &doc.doc_id,
        a.doc_generation,
        edit_of(&a, korean, None, true),
        false,
    )
    .expect_err("consent is still required for a dry run");
    assert_eq!(refused.code, ErrorCode::FontCoverage);
    let ko = run(
        &doc.doc_id,
        a.doc_generation,
        edit_of(&a, korean, None, true),
        true,
    )
    .unwrap();
    assert!(ko.lines >= 2);

    assert_eq!(generation(&doc.doc_id), g0);
    assert_eq!(
        with_doc(&doc.doc_id, |d| Ok(d.history.undo_depth())).unwrap(),
        depth0
    );
    assert!(!with_doc(&doc.doc_id, |d| Ok(d.dirty())).unwrap());
    assert!(
        bytes_of(&doc.doc_id) == bytes0,
        "a dry run must not change the serialised bytes"
    );

    // …and the real edits report what their dry runs predicted.
    let real = run(&doc.doc_id, g0, edit_of(&a, korean, None, false), true).unwrap();
    assert_eq!(
        (real.lines, real.shifted_pt, real.moved_objects),
        (ko.lines, ko.shifted_pt, ko.moved_objects)
    );
    assert_eq!(real.rect, ko.rect);
    assert!(push.shifted_pt > 0.0 && fit.fit_scale.is_some());
}

/// Annotation ids of [`annotated_doc`].
struct Marks {
    highlight: String,
    square: String,
    ink: String,
    stamp: String,
}

fn create_annot(doc_id: &str, spec: AnnotSpec) -> String {
    let doc_id = doc_id.to_string();
    with_state(move |st| {
        registry::mutate(
            st,
            &doc_id,
            MutateOpts::new("undo.annotCreate", ChangeReason::Edit).page(0),
            |d| annot::create::create(d, 0, &spec, None),
        )
    })
    .expect("create annotation")
}

/// A text-field widget far below, a highlight on paragraph C, a red square around C, an ink
/// stroke beside D and a wide SeePDF watermark in the middle of the page.
fn annotated_doc() -> (TestDoc, Marks) {
    let widget = "<< /Type /Annot /Subtype /Widget /FT /Tx /T (field1) /Rect [72 200 300 220] \
                  /F 4 /P 3 0 R /DA (/Helv 12 Tf 0 g) /V (value) >>";
    let doc = open_bytes(pdf(&flow_content(), &[widget]));
    let c_line = objects_of(&doc.doc_id)
        .into_iter()
        .find(|o| text_of(o).contains("Second paragraph"))
        .unwrap();
    let highlight = create_annot(
        &doc.doc_id,
        AnnotSpec::Highlight(MarkupSpec {
            rects: vec![c_line.rect],
            color: [255, 230, 0],
            opacity: 0.5,
            contents: None,
        }),
    );
    let square = create_annot(
        &doc.doc_id,
        AnnotSpec::Square(ShapeSpec {
            rect: Rect::new(68.0, 600.0, 376.0, 634.0),
            color: [255, 0, 0],
            fill_color: None,
            width: 2.0,
            opacity: 1.0,
            dashed: false,
        }),
    );
    let ink = create_annot(
        &doc.doc_id,
        AnnotSpec::Ink(InkSpec {
            paths: vec![vec![345.0, 574.0, 356.0, 578.0, 366.0, 575.0]],
            color: [0, 0, 255],
            width: 2.0,
            opacity: 1.0,
        }),
    );
    // A built-in 승인-style stamp: its appearance stream *is* its content.
    let stamp = create_annot(
        &doc.doc_id,
        AnnotSpec::Stamp(StampSpec {
            rect: Rect::new(250.0, 568.0, 340.0, 598.0),
            image: StampImage::Builtin {
                builtin: "approved".into(),
            },
            rotate: None,
            signature: false,
        }),
    );
    let doc_id = doc.doc_id.clone();
    let watermark = PageStampSpec {
        role: StampRole::Watermark,
        source: PageStampSource::Text {
            text: "CONFIDENTIAL".into(),
            font_size_pt: 60.0,
            color: [200, 0, 0],
        },
        anchor: StampAnchor::Mc,
        margin_pt: 0.0,
        rotate_deg: 0.0,
        opacity: 0.3,
        pages: PageSelection::All(AllPages::All),
        bates: Default::default(),
        behind: false,
    };
    with_state(move |st| stamp::add_stamp(st, &doc_id, &watermark)).expect("watermark");
    (
        doc,
        Marks {
            highlight,
            square,
            ink,
            stamp,
        },
    )
}

fn is_blue(px: &[u8]) -> bool {
    px[0] < 90 && px[1] < 90 && px[2] > 200
}

fn is_green(px: &[u8]) -> bool {
    px[0] < 90 && px[1] > 100 && px[2] < 120
}

/// Pixels of `rect` (2×) that pass `keep` (RGBA).
fn count_pixels(doc_id: &str, rect: Rect, keep: fn(&[u8]) -> bool) -> usize {
    let id = doc_id.to_string();
    let buffer = with_state(move |st| tiles::render_raw_buffer(st, &id, 0, 2.0, Some(rect)))
        .expect("render");
    buffer[32..].chunks_exact(4).filter(|px| keep(px)).count()
}

fn is_yellow(px: &[u8]) -> bool {
    px[0] > 200 && px[1] > 180 && px[2] < 170
}

fn is_red(px: &[u8]) -> bool {
    px[0] > 200 && px[1] < 90 && px[2] < 90
}

#[test]
fn flow_moves_annotations_with_the_text_but_not_widgets_or_stamps() {
    let (doc, marks) = annotated_doc();
    let highlight = marks.highlight.clone();
    // Render once so every annotation has its appearance stream — the case where a move
    // could misplace a drawing.
    let square_before = annots_of(&doc.doc_id)
        .into_iter()
        .find(|a| a.id == marks.square)
        .unwrap();
    assert!(count_pixels(&doc.doc_id, square_before.rect, is_red) > 100);
    let before = objects_of(&doc.doc_id);
    let annots_before = annots_of(&doc.doc_id);
    let hl = annots_before
        .iter()
        .find(|a| a.id == highlight)
        .unwrap()
        .clone();
    let ink = annots_before
        .iter()
        .find(|a| a.id == marks.ink)
        .unwrap()
        .clone();
    let stamp_before = annots_before
        .iter()
        .find(|a| a.id == marks.stamp)
        .unwrap()
        .clone();
    let green_before = count_pixels(&doc.doc_id, stamp_before.rect, is_green);
    assert!(green_before > 100, "{green_before} stamp pixels before");
    assert!(count_pixels(&doc.doc_id, ink.rect, is_blue) > 20);
    let widget = annots_before
        .iter()
        .find(|a| a.kind == AnnotKind::Widget)
        .unwrap()
        .clone();
    let mark = before
        .iter()
        .find(|o| text_of(o).contains("CONFIDENTIAL"))
        .unwrap()
        .clone();
    let d_bottom = before
        .iter()
        .find(|o| text_of(o).contains("Third"))
        .unwrap()
        .rect
        .b;
    // The watermark spans the column and sits below D: were it content, it would be an
    // obstacle (room ≈ D − its top). It is a stamp, so the widget is the first obstacle.
    assert!(
        mark.rect.t < d_bottom && mark.rect.l < 100.0 && mark.rect.r > 500.0,
        "{:?}",
        mark.rect
    );

    let a = probe(&doc.doc_id, 200.0, 690.0);
    let long = format!("{PARA_A_TEXT} {EXTRA}");
    let dry = run(
        &doc.doc_id,
        a.doc_generation,
        edit_of(&a, &long, None, true),
        false,
    )
    .unwrap();
    assert!(
        (dry.room_pt - (d_bottom - widget.rect.t)).abs() < 0.05,
        "{} vs {}",
        dry.room_pt,
        d_bottom - widget.rect.t
    );
    assert_eq!(dry.blocked, None);
    assert_eq!(dry.moved_annotations, 4, "highlight, square, ink, stamp");

    let res = run(
        &doc.doc_id,
        a.doc_generation,
        edit_of(&a, &long, None, false),
        false,
    )
    .unwrap();
    assert_eq!((res.moved_objects, res.moved_annotations), (4, 4));
    let dy = -res.shifted_pt;
    let annots_after = annots_of(&doc.doc_id);
    let hl_after = annots_after.iter().find(|a| a.id == highlight).unwrap();
    assert!(
        (hl_after.rect.t - (hl.rect.t + dy)).abs() < 0.05,
        "{:?} → {:?}",
        hl.rect,
        hl_after.rect
    );
    assert!((hl_after.rect.b - (hl.rect.b + dy)).abs() < 0.05);
    let q0 = hl.quads.as_ref().unwrap()[0];
    let q1 = hl_after.quads.as_ref().unwrap()[0];
    assert!(
        (q1.t - (q0.t + dy)).abs() < 0.05 && (q1.l - q0.l).abs() < 0.05,
        "{q0:?} → {q1:?}"
    );
    // …and it is drawn there: the cleared appearance is regenerated at the moved quads.
    let at_new = count_pixels(&doc.doc_id, q1, is_yellow);
    assert!(
        at_new > 100,
        "{at_new} highlight pixels at the moved quad {q1:?}"
    );
    assert!(
        count_pixels(&doc.doc_id, q0, is_yellow) < at_new / 10,
        "old place not highlighted"
    );
    // The square keeps its appearance, which follows the moved /Rect.
    let square_after = annots_after.iter().find(|a| a.id == marks.square).unwrap();
    assert!((square_after.rect.t - (square_before.rect.t + dy)).abs() < 0.05);
    let top_edge = |r: &Rect| Rect::new(r.l, r.t - 3.0, r.r, r.t);
    let red_new = count_pixels(&doc.doc_id, top_edge(&square_after.rect), is_red);
    assert!(
        red_new > 200,
        "{red_new} red pixels on the moved square's top edge"
    );
    assert!(count_pixels(&doc.doc_id, top_edge(&square_before.rect), is_red) < red_new / 10);
    // The ink stroke's points moved with it.
    let ink_after = annots_after.iter().find(|a| a.id == marks.ink).unwrap();
    let (p0, p1) = (
        &ink.ink_paths.as_ref().unwrap()[0],
        &ink_after.ink_paths.as_ref().unwrap()[0],
    );
    assert_eq!(p0.len(), p1.len());
    for (k, (x, y)) in p0.iter().zip(p1).enumerate() {
        let expect = if k % 2 == 1 { x + dy } else { *x };
        assert!((y - expect).abs() < 0.05, "ink point {k}: {x} → {y}");
    }
    assert!((ink_after.rect.b - (ink.rect.b + dy)).abs() < 0.05);
    let blue_new = count_pixels(&doc.doc_id, ink_after.rect, is_blue);
    assert!(blue_new > 20, "{blue_new} ink pixels at the moved rect");
    assert!(count_pixels(&doc.doc_id, ink.rect, is_blue) < blue_new / 5 + 1);
    // The stamp's baked appearance follows its /Rect, undistorted.
    let stamp_after = annots_after.iter().find(|a| a.id == marks.stamp).unwrap();
    assert!((stamp_after.rect.t - (stamp_before.rect.t + dy)).abs() < 0.05);
    let green_new = count_pixels(&doc.doc_id, stamp_after.rect, is_green);
    assert!(
        green_new * 10 >= green_before * 9,
        "{green_new} stamp pixels at the moved rect vs {green_before} before"
    );
    // The old place, less the strip the moved stamp still covers (a 30 pt stamp moved by less
    // than its height overlaps its old rect at the bottom).
    let old = stamp_before.rect;
    let vacated = Rect::new(old.l, old.b.max(stamp_after.rect.t) + 1.0, old.r, old.t);
    let green_old = count_pixels(&doc.doc_id, vacated, is_green);
    assert!(
        green_old < green_before / 10,
        "{green_old} stamp pixels left at the old place"
    );
    let widget_after = annots_after
        .iter()
        .find(|a| a.kind == AnnotKind::Widget)
        .unwrap();
    assert_eq!(widget_after.rect, widget.rect, "widgets never move");
    let mark_after = res
        .objects
        .objects
        .iter()
        .find(|o| text_of(o).contains("CONFIDENTIAL"))
        .unwrap();
    assert!(same_bits(&mark, mark_after), "the watermark never moves");

    // (8) One undo step restores every moved object and the annotation.
    let label = with_doc(&doc.doc_id, |d| Ok(d.history.undo_label())).unwrap();
    assert_eq!(label.as_deref(), Some("undo.paragraphEdit"));
    let id = doc.doc_id.clone();
    with_state(move |st| registry::undo(st, &id, false)).expect("undo");
    let restored = objects_of(&doc.doc_id);
    assert_eq!(restored.len(), before.len());
    for (b, r) in before.iter().zip(&restored) {
        assert_eq!(text_of(b), text_of(r));
        assert!(
            (b.rect.t - r.rect.t).abs() < 0.01 && (b.rect.l - r.rect.l).abs() < 0.01,
            "{:?}: {:?} vs {:?}",
            text_of(b),
            b.rect,
            r.rect
        );
    }
    let hl_back = annots_of(&doc.doc_id)
        .into_iter()
        .find(|a| a.id == highlight)
        .unwrap();
    let close = |x: &Rect, y: &Rect| (x.t - y.t).abs() < 0.05 && (x.b - y.b).abs() < 0.05;
    assert!(
        close(&hl_back.rect, &hl.rect),
        "{:?} vs {:?}",
        hl_back.rect,
        hl.rect
    );
    assert_eq!(probe(&doc.doc_id, 200.0, 690.0).text, PARA_A_TEXT);
}

// v0.3 pkg4-annotations-stamps-objects (verification round 1): SeePDF's own lines are real
// `/Line`s now (not Ink), so the flow must move `/L` — and `/Vertices`, `/CL` and SeePDF's
// callout mirrors — with the `/Rect`, or the next edit (rebuilt from those keys) snaps the
// shape back to where it was before the flow.
#[test]
fn flow_moves_line_polygon_and_callout_geometry_in_one_step() {
    use seepdf_lib::engine::annot::create::create_in;
    use seepdf_lib::engine::annot::update::update_in;
    use seepdf_lib::ipc::types::{AnnotPatch, CalloutSpec, LineSpec, PolySpec, TextAlign};
    let (doc, _marks) = annotated_doc();
    let create = |spec: AnnotSpec| {
        let doc_id = doc.doc_id.clone();
        with_state(move |st| create_in(st, &doc_id, 0, &spec, None, None)).expect("create")
    };
    let line = create(AnnotSpec::Line(LineSpec {
        p1: [345.0, 576.0],
        p2: [366.0, 576.0],
        color: [0, 0, 255],
        width: 2.0,
        opacity: 1.0,
        heads: Some([false, true]),
        measure: None,
        dashed: false,
    }));
    let polygon = create(AnnotSpec::Polygon(PolySpec {
        vertices: vec![300.0, 570.0, 330.0, 570.0, 315.0, 585.0],
        color: [0, 128, 0],
        fill_color: None,
        width: 1.0,
        opacity: 1.0,
        cloudy: false,
        dashed: false,
        measure: None,
    }));
    let callout = create(AnnotSpec::Callout(CalloutSpec {
        rect: Rect::new(100.0, 565.0, 200.0, 585.0),
        text: "note".into(),
        font_size: 10.0,
        color: [0, 0, 0],
        align: TextAlign::Left,
        fill_color: None,
        callout: vec![90.0, 590.0, 100.0, 575.0],
    }));
    let find = |id: &str| {
        annots_of(&doc.doc_id)
            .into_iter()
            .find(|a| a.id == id)
            .unwrap()
    };
    let (line0, poly0, call0) = (find(&line), find(&polygon), find(&callout));
    let depth0 = with_doc(&doc.doc_id, |d| Ok(d.history.undo_depth())).unwrap();

    let a = probe(&doc.doc_id, 200.0, 690.0);
    let long = format!("{PARA_A_TEXT} {EXTRA}");
    let res = run(
        &doc.doc_id,
        a.doc_generation,
        edit_of(&a, &long, None, false),
        false,
    )
    .unwrap();
    let dy = -res.shifted_pt;
    assert!(dy < -10.0, "the push moved things: {dy}");
    let shifted = |v: &[f32]| -> Vec<f32> {
        v.iter()
            .enumerate()
            .map(|(k, x)| if k % 2 == 1 { x + dy } else { *x })
            .collect()
    };
    let near = |a: &[f32], b: &[f32]| {
        a.len() == b.len() && a.iter().zip(b).all(|(x, y)| (x - y).abs() < 0.05)
    };
    let (line1, poly1, call1) = (find(&line), find(&polygon), find(&callout));
    for (before, after) in [(&line0, &line1), (&poly0, &poly1), (&call0, &call1)] {
        assert!(
            (after.rect.t - (before.rect.t + dy)).abs() < 0.1,
            "{:?}: {:?} → {:?}",
            before.kind,
            before.rect,
            after.rect
        );
    }
    let l0 = line0.line_points.unwrap();
    let l1 = line1.line_points.unwrap();
    assert!(
        near(&l1, &shifted(&l0)),
        "/L did not move with the flow: {l0:?} → {l1:?} dy={dy}"
    );
    let (v0, v1) = (
        poly0.vertices.clone().unwrap(),
        poly1.vertices.clone().unwrap(),
    );
    assert!(near(&v1, &shifted(&v0)), "/Vertices: {v0:?} → {v1:?}");
    let (c0, c1) = (
        call0.callout.clone().unwrap(),
        call1.callout.clone().unwrap(),
    );
    assert!(near(&c1, &shifted(&c0)), "/CL: {c0:?} → {c1:?}");
    assert_eq!(
        line1.heads, line0.heads,
        "the arrow head survives the rewrite"
    );
    // The text, the moves and the lopdf pass are one undo step.
    let (depth1, label) = with_doc(&doc.doc_id, |d| {
        Ok((d.history.undo_depth(), d.history.undo_label()))
    })
    .unwrap();
    assert_eq!(depth1, depth0 + 1);
    assert_eq!(label.as_deref(), Some("undo.paragraphEdit"));

    // A later edit rebuilds from the keys: nothing jumps back.
    for id in [&line, &polygon] {
        let (doc_id, id) = (doc.doc_id.clone(), id.clone());
        with_state(move |st| {
            update_in(
                st,
                &doc_id,
                0,
                &id,
                &AnnotPatch {
                    color: Some([200, 0, 0]),
                    ..AnnotPatch::default()
                },
            )
        })
        .expect("recolour");
    }
    let (line2, poly2) = (find(&line), find(&polygon));
    assert!(
        (line2.rect.t - line1.rect.t).abs() < 0.1,
        "a colour edit moved the line back: {:?} → {:?}",
        line1.rect,
        line2.rect
    );
    assert_eq!(line2.border_width, 2.0, "a colour edit keeps the width");
    assert!(near(&line2.line_points.unwrap(), &l1));
    assert!(
        (poly2.rect.t - poly1.rect.t).abs() < 0.1,
        "a colour edit moved the polygon back"
    );
    assert!(near(&poly2.vertices.clone().unwrap(), &v1));

    // Undo both recolours and the flow: everything is where it started.
    for _ in 0..3 {
        let id = doc.doc_id.clone();
        with_state(move |st| registry::undo(st, &id, false)).expect("undo");
    }
    assert!(near(&find(&line).line_points.unwrap(), &l0));
    assert!(near(&find(&polygon).vertices.unwrap(), &v0));
    assert!(near(&find(&callout).callout.unwrap(), &c0));
    assert_eq!(probe(&doc.doc_id, 200.0, 690.0).text, PARA_A_TEXT);
}

#[test]
fn flow_stale_generation_is_refused() {
    let doc = open_bytes(pdf(&flow_content(), &[]));
    let a = probe(&doc.doc_id, 200.0, 690.0);
    for dry_run in [true, false] {
        let err = run(
            &doc.doc_id,
            a.doc_generation + 1,
            edit_of(&a, "x y z", None, dry_run),
            false,
        )
        .expect_err("wrong generation");
        assert_eq!(err.code, ErrorCode::Stale, "dryRun {dry_run}");
    }
    assert_eq!(generation(&doc.doc_id), a.doc_generation);
}

// ---------------------------------------------------------------------------------------
// Stage 9 verification round: one regression test per verified finding
// ---------------------------------------------------------------------------------------

/// A one-page PDF with the given content streams (in order), extra page resources and extra
/// objects numbered from 7 (after the fonts at 5 and 6; object 4 is unused).
fn pdf_streams(streams: &[&str], resources: &str, extra: &[String]) -> Vec<u8> {
    let first_stream = 7 + extra.len();
    let contents: String = (0..streams.len())
        .map(|i| format!("{} 0 R ", first_stream + i))
        .collect();
    let mut objects = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
        format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << \
             /F1 5 0 R /F2 6 0 R >> {resources} >> /Contents [{contents}] >>"
        ),
        "<< >>".to_string(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>"
            .to_string(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica-Bold /Encoding /WinAnsiEncoding >>"
            .to_string(),
    ];
    objects.extend(extra.iter().cloned());
    for s in streams {
        objects.push(format!("<< /Length {} >>\nstream\n{s}endstream", s.len()));
    }
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, body) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{body}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(
        format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes(),
    );
    for off in offsets {
        out.extend_from_slice(format!("{off:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            objects.len() + 1
        )
        .as_bytes(),
    );
    out
}

/// `lines` as one Helvetica paragraph of `size` / `leading`, first baseline at `(x, y)`.
fn para_sized(x: f32, y: f32, size: f32, leading: f32, lines: &[&str]) -> String {
    let mut s = format!("BT /F1 {size} Tf {leading} TL {x} {y} Td\n");
    for (i, line) in lines.iter().enumerate() {
        if i > 0 {
            s.push_str("T* ");
        }
        s.push_str(&format!("({line}) Tj\n"));
    }
    s.push_str("ET\n");
    s
}

fn ink_overlap(a: &Rect, b: &Rect) -> f32 {
    let w = a.r.min(b.r) - a.l.max(b.l);
    let h = a.t.min(b.t) - a.b.max(b.b);
    if w > 0.3 && h > 0.3 {
        w * h
    } else {
        0.0
    }
}

/// A SeePDF footer stamp (a page number at the default 36 pt margin) is in the way: the push
/// stops above it and reports what is left, instead of drawing the body over the page number.
#[test]
fn flow_seepdf_footer_stamp_stops_the_push() {
    let body: Vec<String> = (0..40)
        .map(|k| format!("Body line {k} of the text that follows the edited paragraph here."))
        .collect();
    let body_refs: Vec<&str> = body.iter().map(String::as_str).collect();
    let mut c = para(72.0, 700.0, &PARA_A);
    c.push_str(&para(72.0, 626.8, &body_refs)); // last baseline 65.2
    let doc = open_bytes(pdf(&c, &[]));
    let id = doc.doc_id.clone();
    let footer = PageStampSpec {
        role: StampRole::Footer,
        source: PageStampSource::Text {
            text: "Page {{page}}".into(),
            font_size_pt: 12.0,
            color: [0, 0, 0],
        },
        anchor: StampAnchor::Bc,
        margin_pt: 36.0,
        rotate_deg: 0.0,
        opacity: 1.0,
        pages: PageSelection::All(AllPages::All),
        bates: Default::default(),
        behind: false,
    };
    with_state(move |st| stamp::add_stamp(st, &id, &footer)).expect("footer stamp");
    let before = objects_of(&doc.doc_id);
    let stamp_obj = before
        .iter()
        .find(|o| text_of(o).contains("Page 1"))
        .unwrap()
        .clone();
    let lowest = before
        .iter()
        .filter(|o| text_of(o).starts_with("Body line"))
        .map(|o| o.rect.b)
        .fold(f32::MAX, f32::min);
    let a = probe(&doc.doc_id, 200.0, 690.0);
    let long = format!("{PARA_A_TEXT} {EXTRA} {EXTRA}");
    let dry = run(
        &doc.doc_id,
        a.doc_generation,
        edit_of(&a, &long, None, true),
        false,
    )
    .unwrap();
    assert_eq!(dry.blocked, Some(FlowBlocked::Obstacle), "{dry:?}");
    assert!(
        (dry.room_pt - (lowest - stamp_obj.rect.t)).abs() < 0.05,
        "{} vs {}",
        dry.room_pt,
        lowest - stamp_obj.rect.t
    );
    assert!(dry.overflow_pt > 0.0);

    let res = run(
        &doc.doc_id,
        a.doc_generation,
        edit_of(&a, &long, None, false),
        false,
    )
    .unwrap();
    let stamp_after = res
        .objects
        .objects
        .iter()
        .find(|o| text_of(o).contains("Page 1"))
        .unwrap();
    assert!(same_bits(&stamp_obj, stamp_after), "the stamp never moves");
    for o in res
        .objects
        .objects
        .iter()
        .filter(|o| text_of(o).starts_with("Body line"))
    {
        assert!(
            ink_overlap(&o.rect, &stamp_after.rect) == 0.0,
            "{:?} {:?} was pushed over the footer stamp {:?}",
            text_of(o),
            o.rect,
            stamp_after.rect
        );
    }
}

/// After a Hangul-substituted edit of a justified paragraph (one SeePDF-Hangul object per
/// word), the paragraph probes back whole: no stretched word gap splits a line, so a second
/// edit replaces every word instead of drawing over orphans.
#[test]
fn flow_substituted_justified_paragraph_probes_back_whole() {
    let doc = open("tracemonkey.pdf");
    let p = probe(&doc.doc_id, 420.0, 400.0);
    assert_eq!(p.align, ParagraphAlign::Justify);
    let first = run(
        &doc.doc_id,
        p.doc_generation,
        edit_of(&p, &format!("{} 한글 문장.", p.text), None, false),
        true,
    )
    .expect("substituted edit");
    let p2 = probe(&doc.doc_id, 420.0, 400.0);
    assert_eq!(p2.lines, first.lines, "{p2:?}");
    assert_eq!(p2.align, ParagraphAlign::Justify);
    assert!(p2.text.ends_with("한글 문장."), "{}", p2.text);
    assert_eq!(
        probe(&doc.doc_id, 420.0, 340.0).object_ids,
        p2.object_ids,
        "one paragraph, every line"
    );
    let hangul = |objects: &[PageObject], ids: &[u32]| {
        objects
            .iter()
            .filter(|o| o.font_name.as_deref().is_some_and(|f| f.contains("Hangul")))
            .filter(|o| !ids.contains(&o.object_id))
            .count()
    };
    assert_eq!(
        hangul(&first.objects.objects, &p2.object_ids),
        0,
        "no orphaned words"
    );

    let second = run(
        &doc.doc_id,
        p2.doc_generation,
        edit_of(
            &p2,
            &format!("{} 두 번째 편집입니다.", p2.text),
            None,
            false,
        ),
        true,
    )
    .expect("second edit");
    let p3 = probe(&doc.doc_id, 420.0, 400.0);
    assert_eq!(p3.lines, second.lines);
    assert!(p3.text.ends_with("두 번째 편집입니다."), "{}", p3.text);
    assert_eq!(
        hangul(&second.objects.objects, &p3.object_ids),
        0,
        "no orphaned words"
    );
}

/// PDFium cannot write an inline image or a shading back: an edit whose rewrite would drop one
/// is refused — at the probe, the dry run and the write — instead of losing it; it is allowed
/// when the image sits in a content stream the edit does not rewrite.
#[test]
fn flow_refuses_to_drop_inline_images_and_shadings() {
    let inline = "q 200 0 0 20 72 740 cm BI /W 2 /H 1 /CS /G /BPC 8 /F /AHx ID 00FF> EI Q\n";
    let paragraph = para(72.0, 700.0, &PARA_A);
    let shading = "<< /ShadingType 2 /ColorSpace /DeviceRGB /Coords [72 0 540 0] /Function \
                   << /FunctionType 2 /Domain [0 1] /C0 [1 0 0] /C1 [0 0 1] /N 1 >> >>"
        .to_string();
    let same_stream = [
        (
            "inline image",
            pdf_streams(&[&format!("{inline}{paragraph}")], "", &[]),
        ),
        (
            "shading",
            pdf_streams(
                &[&format!("{paragraph}q 72 300 468 100 re W n /Sh1 sh Q\n")],
                "/Shading << /Sh1 7 0 R >>",
                &[shading],
            ),
        ),
    ];
    let longer = format!("{PARA_A_TEXT} {ONE_MORE}");
    for (name, bytes) in same_stream {
        let doc = open_bytes(bytes);
        let before = objects_of(&doc.doc_id);
        assert!(
            before.iter().any(|o| matches!(
                o.object_type,
                PageObjectType::Image | PageObjectType::Shading
            )),
            "{name}: {before:?}"
        );
        let p = probe(&doc.doc_id, 200.0, 690.0);
        assert_eq!(
            (p.strategy, p.reason),
            (
                TextEditStrategy::Refused,
                Some(NotEditableReason::UnwritableContent)
            ),
            "{name}"
        );
        for dry_run in [true, false] {
            let e = run(
                &doc.doc_id,
                p.doc_generation,
                edit_of(&p, &longer, None, dry_run),
                false,
            )
            .expect_err("refused");
            assert_eq!(e.code, ErrorCode::Unsupported, "{name}");
            assert!(
                e.detail
                    .as_deref()
                    .unwrap_or("")
                    .starts_with("unwritableContent"),
                "{name}: {e:?}"
            );
        }
        assert_eq!(
            generation(&doc.doc_id),
            p.doc_generation,
            "{name}: nothing written"
        );
        assert_eq!(
            objects_of(&doc.doc_id).len(),
            before.len(),
            "{name}: nothing lost"
        );
    }

    // The inline image in a stream of its own: the edit rewrites only the paragraph's stream.
    let doc = open_bytes(pdf_streams(&[inline, &paragraph], "", &[]));
    let p = probe(&doc.doc_id, 200.0, 690.0);
    assert_eq!(p.strategy, TextEditStrategy::InPlace, "{p:?}");
    let res = run(
        &doc.doc_id,
        p.doc_generation,
        edit_of(&p, &longer, None, false),
        false,
    )
    .expect("allowed");
    let images = |objects: &[PageObject]| {
        objects
            .iter()
            .filter(|o| o.object_type == PageObjectType::Image)
            .count()
    };
    assert_eq!(
        images(&res.objects.objects),
        1,
        "the inline image is still there"
    );

    // TAMReview's header logo and rule are inline images in the body's stream.
    let doc = open("TAMReview.pdf");
    let p = probe(&doc.doc_id, 200.0, 520.0);
    assert_eq!(
        p.reason,
        Some(NotEditableReason::UnwritableContent),
        "{p:?}"
    );
}

/// On a two-column page the running footer is found per column: editing tracemonkey's left
/// "1. Introduction" paragraph leaves the ACM copyright block (the left column's footer) where
/// it is, although the right column's lines run down past it.
#[test]
fn flow_two_column_footer_is_found_per_column() {
    let doc = open("tracemonkey.pdf");
    let before = objects_of(&doc.doc_id);
    let p = probe(&doc.doc_id, 150.0, 185.0);
    assert!(p.rect.r < 300.0, "{p:?}");
    let block_top = before
        .iter()
        .find(|o| text_of(o).contains("Permission to make digital"))
        .unwrap()
        .rect
        .t;
    let text = format!(
        "{} This sentence is appended so that the paragraph grows by a line or two.",
        p.text
    );
    let res = run(
        &doc.doc_id,
        p.doc_generation,
        edit_of(&p, &text, None, false),
        false,
    )
    .unwrap();
    assert_eq!(res.moved_objects, 0, "{res:?}");
    // Nothing moves: the room is the paragraph's own, from its line grid down to the block.
    let last = before
        .iter()
        .filter(|o| p.object_ids.contains(&o.object_id))
        .map(|o| o.matrix[5])
        .fold(f32::MAX, f32::min);
    let room = last - 0.25 * p.font_size_pt - block_top;
    assert!(
        (res.room_pt - room).abs() < 0.05,
        "{} vs {room}",
        res.room_pt
    );
    for (b, n) in survivors(&before, &p.object_ids, &res.objects.objects) {
        assert!(
            same_bits(b, n),
            "{:?}: {:?} → {:?}",
            text_of(b),
            b.rect,
            n.rect
        );
    }
}

/// A page number at LaTeX's `\footskip` (30 pt below the last baseline: a clear gap under
/// 2 × the leading) is the running footer too: it never moves.
#[test]
fn flow_latex_page_number_is_the_footer() {
    let lines = [
        PARA_A[0], PARA_A[1], PARA_A[2], PARA_A[0], PARA_A[1], "at last.",
    ];
    let mut c = String::new();
    let mut y = 700.0;
    for _ in 0..7 {
        c.push_str(&para_sized(72.0, y, 10.0, 12.0, &lines));
        y -= 5.0 * 12.0 + 18.0;
    }
    let last = y + 18.0;
    c.push_str(&format!("BT /F1 10 Tf 303 {} Td (7) Tj ET\n", last - 30.0));
    let doc = open_bytes(pdf(&c, &[]));
    let before = objects_of(&doc.doc_id);
    let number = before.iter().find(|o| text_of(o) == "7").unwrap().clone();
    let p = probe(&doc.doc_id, 200.0, 700.0 - 78.0);
    let res = run(
        &doc.doc_id,
        p.doc_generation,
        edit_of(&p, &format!("{} {ONE_MORE}", p.text), None, false),
        false,
    )
    .unwrap();
    assert!(res.shifted_pt > 0.0, "{res:?}");
    let after = res
        .objects
        .objects
        .iter()
        .find(|o| text_of(o) == "7")
        .unwrap();
    assert!(
        same_bits(&number, after),
        "the page number moved: {:?} → {:?}",
        number.rect,
        after.rect
    );
    let lowest = before
        .iter()
        .filter(|o| text_of(o) != "7")
        .map(|o| o.rect.b)
        .fold(f32::MAX, f32::min);
    assert!(
        (res.room_pt - (lowest - number.rect.t)).abs() < 0.05,
        "{} vs {}",
        res.room_pt,
        lowest - number.rect.t
    );
}

/// Editing a large heading does not move the page number: the footer gap is measured against
/// the page's line pitch, not the edited paragraph's leading.
#[test]
fn flow_heading_edit_keeps_the_page_number() {
    let mut c = String::from(
        "BT /F2 24 Tf 28.8 TL 72 720 Td (A Large Heading That Wraps) Tj T* (Onto Two Lines) Tj ET\n",
    );
    let mut y = 640.0;
    while y > 100.0 {
        c.push_str(&para(72.0, y, &PARA_A));
        y -= 72.0;
    }
    c.push_str("BT /F1 10 Tf 300 45 Td (3) Tj ET\n");
    let doc = open_bytes(pdf(&c, &[]));
    let before = objects_of(&doc.doc_id);
    let number = before.iter().find(|o| text_of(o) == "3").unwrap().clone();
    let h = probe(&doc.doc_id, 150.0, 722.0);
    assert!(h.font_size_pt > 20.0, "{h:?}");
    let res = run(
        &doc.doc_id,
        h.doc_generation,
        edit_of(
            &h,
            "A Large Heading That Wraps Onto Two Lines And Then Some More Words",
            None,
            false,
        ),
        false,
    )
    .unwrap();
    let after = res
        .objects
        .objects
        .iter()
        .find(|o| text_of(o) == "3")
        .unwrap();
    assert!(
        same_bits(&number, after),
        "the page number moved: {:?} → {:?}",
        number.rect,
        after.rect
    );
}

/// A box drawn around the paragraph keeps its content inside: what follows the paragraph in
/// the box moves at most to the box's bottom edge, never across it.
#[test]
fn flow_box_around_the_paragraph_is_an_obstacle() {
    let mut c = para(80.0, 700.0, &PARA_A[..3]);
    c.push_str(&para(
        80.0,
        647.2,
        &[
            "Second paragraph inside the box, first line.",
            "and its last line.",
        ],
    ));
    let box_b = 632.8 - 12.0;
    c.push_str(&format!(
        "0 0 1 RG 1.5 w 70 {box_b} 470 {} re S 0 G\n",
        716.0 - box_b
    ));
    c.push_str(&para(
        72.0,
        box_b - 30.0,
        &["Text below the box never moves."],
    ));
    let doc = open_bytes(pdf(&c, &[]));
    let before = objects_of(&doc.doc_id);
    let a = probe(&doc.doc_id, 150.0, 690.0);
    assert_eq!(a.lines, 3);
    let res = run(
        &doc.doc_id,
        a.doc_generation,
        edit_of(&a, &format!("{} {EXTRA}", a.text), None, false),
        false,
    )
    .unwrap();
    assert_eq!(res.blocked, Some(FlowBlocked::Obstacle), "{res:?}");
    let inner_edge = box_b + 0.75;
    for (b, n) in survivors(&before, &a.object_ids, &res.objects.objects) {
        if b.object_type == PageObjectType::Text && b.rect.b > box_b {
            assert!(
                n.rect.b >= inner_edge - 0.01,
                "{:?} crosses the box: {:?}",
                text_of(b),
                n.rect
            );
        } else {
            assert!(same_bits(b, n), "{:?} moved", text_of(b));
        }
    }
}

/// A line wrapped beside a figure that sticks out of the column straddles the figure's top: it
/// cannot move, so the push stops at its top instead of drawing the line above over it.
#[test]
fn flow_line_beside_a_figure_stops_the_push() {
    let mut c = para(
        72.0,
        700.0,
        &[
            "Left column paragraph that the user",
            "is going to edit and make longer",
            "than it was before by typing.",
            "End of it.",
        ],
    );
    c.push_str("BT /F1 12 Tf 72 650 Td (Following line in column.) Tj ET\n");
    c.push_str("0.6 g 200 560 220 70 re f 0 g\n");
    c.push_str("BT /F1 12 Tf 72 628 Td (Beside the fig) Tj ET\n");
    let doc = open_bytes(pdf(&c, &[]));
    let a = probe(&doc.doc_id, 120.0, 690.0);
    let text = format!(
        "{} Plus one more line of text that is long enough to wrap.",
        a.text
    );
    let res = run(
        &doc.doc_id,
        a.doc_generation,
        edit_of(&a, &text, None, false),
        false,
    )
    .unwrap();
    assert_eq!(res.blocked, Some(FlowBlocked::Obstacle), "{res:?}");
    let after = &res.objects.objects;
    let following = after
        .iter()
        .find(|o| text_of(o).contains("Following"))
        .unwrap()
        .rect;
    let beside = after
        .iter()
        .find(|o| text_of(o).contains("Beside"))
        .unwrap()
        .rect;
    assert!(
        following.b >= beside.t - 0.01,
        "the moved line {following:?} covers {beside:?}"
    );
}

// ---------------------------------------------------------------------------------------
// Verified defects (round 2): the text block, list markers, a letter's signature block,
// descenders, page backgrounds, content in a stream that cannot be rewritten, content
// streams cut mid-object, header stamps, justified paragraphs with long words
// ---------------------------------------------------------------------------------------

const BODY_LINE: &str =
    "Body text that runs the full width of the text block from margin to margin.";
const MORE: &str =
    "This sentence is appended so that the paragraph grows by one more line or two of text here.";

/// The new text objects of an edit (appended after the survivors) must not overlap any
/// survivor's ink.
fn assert_no_overlap(label: &str, before: &[PageObject], removed: &[u32], after: &[PageObject]) {
    let kept = before.len() - removed.len();
    for n in &after[kept..] {
        for o in &after[..kept] {
            let inner = Rect::new(o.rect.l, o.rect.b + 0.5, o.rect.r, o.rect.t - 0.5);
            assert_eq!(
                ink_overlap(&inner, &n.rect),
                0.0,
                "{label}: new {:?} {:?} overlaps {:?} {:?}",
                text_of(n),
                n.rect,
                text_of(o),
                o.rect
            );
        }
    }
}

/// A paragraph narrower than the body text under it — one short line, a ragged paragraph, a
/// block quote — pushes that text down instead of drawing over it: the column is the text
/// block, not the edited paragraph's own width (the body lines used to be "obstacles").
#[test]
fn flow_body_text_wider_than_a_short_paragraph_follows_it() {
    let body = |y: f32| para(72.0, y, &[BODY_LINE; 4]);
    let cases = [
        (
            "one short line",
            format!(
                "BT /F1 12 Tf 72 700 Td (Thank you for your letter of 5 May.) Tj ET\n{}",
                body(671.2)
            ),
            (100.0, 703.0),
        ),
        (
            "ragged paragraph",
            format!(
                "{}{}",
                para(
                    72.0,
                    700.0,
                    &[
                        "A short ragged paragraph whose lines",
                        "stop well before the right margin."
                    ]
                ),
                body(656.8)
            ),
            (100.0, 703.0),
        ),
        (
            "block quote",
            format!(
                "{}{}",
                para(
                    108.0,
                    700.0,
                    &[
                        "An indented block quote with two lines of text that",
                        "stays inside the indented margins on both sides."
                    ]
                ),
                body(656.8)
            ),
            (200.0, 703.0),
        ),
    ];
    for (label, content, at) in cases {
        let doc = open_bytes(pdf(&content, &[]));
        let before = objects_of(&doc.doc_id);
        let p = probe(&doc.doc_id, at.0, at.1);
        let res = run(
            &doc.doc_id,
            p.doc_generation,
            edit_of(&p, &format!("{} {MORE}", p.text), None, false),
            false,
        )
        .expect(label);
        assert_eq!(
            (res.blocked, res.overflow_pt),
            (None, 0.0),
            "{label}: {res:?}"
        );
        assert!(
            res.shifted_pt > 10.0 && res.moved_objects == 4,
            "{label}: {res:?}"
        );
        for (b, n) in survivors(&before, &p.object_ids, &res.objects.objects) {
            assert!(
                moved_by(b, n, -res.shifted_pt),
                "{label}: {:?} {:?} → {:?}",
                text_of(b),
                b.rect,
                n.rect
            );
        }
        assert_no_overlap(label, &before, &p.object_ids, &res.objects.objects);
    }
}

/// A Word-style list (marker in the hanging indent, text a quarter inch right of it): editing
/// one item moves every following item's marker with its text, and the paragraph after the
/// list moves whole — its short last line is not an obstacle inside it.
#[test]
fn flow_list_markers_move_with_their_items() {
    let item = |y: f32, lines: &[&str]| {
        format!(
            "BT /F1 11 Tf 90 {y} Td (\\225) Tj ET\n{}",
            para_sized(108.0, y, 11.0, 13.4, lines)
        )
    };
    let mut c = para_sized(
        72.0,
        720.0,
        11.0,
        13.4,
        &["The list below has three items, each with a marker in the hanging indent."],
    );
    c.push_str(&item(
        680.0,
        &[
            "First item of the list with enough words to wrap onto a second line of",
            "text inside its own indented column.",
        ],
    ));
    c.push_str(&item(
        645.0,
        &[
            "Second item of the list, also long enough to wrap onto a second line of",
            "text inside the same indented column.",
        ],
    ));
    c.push_str(&item(610.0, &["Third item, a single line."]));
    c.push_str(&para_sized(
        72.0,
        580.0,
        11.0,
        13.4,
        &[
            "A closing paragraph after the list, at the body's own left margin, with",
            "a second line.",
        ],
    ));
    let doc = open_bytes(pdf(&c, &[]));
    let before = objects_of(&doc.doc_id);
    let p = probe(&doc.doc_id, 200.0, 682.0);
    assert_eq!(p.lines, 2, "{p:?}");
    let res = run(
        &doc.doc_id,
        p.doc_generation,
        edit_of(&p, &format!("{} {MORE}", p.text), None, false),
        false,
    )
    .unwrap();
    assert!(res.shifted_pt > 10.0 && res.blocked.is_none(), "{res:?}");
    let mut moved = 0;
    for (b, n) in survivors(&before, &p.object_ids, &res.objects.objects) {
        if b.rect.t < p.rect.b {
            // markers, items and the closing paragraph alike
            assert!(
                moved_by(b, n, -res.shifted_pt),
                "{:?} {:?} → {:?}",
                text_of(b),
                b.rect,
                n.rect
            );
            moved += 1;
        } else {
            assert!(same_bits(b, n), "{:?} moved", text_of(b));
        }
    }
    assert_eq!(moved, 7, "two markers, three item lines, two closing lines");
}

/// A letter: the signature block near the page bottom (after the space left for the
/// signature) is not a running footer — it follows the letter, and the space stays.
#[test]
fn flow_letter_signature_block_moves_with_the_letter() {
    let body: Vec<String> = (0..20)
        .map(|k| format!("Letter body line {k} explaining the details of the renewal terms."))
        .collect();
    let refs: Vec<&str> = body.iter().map(String::as_str).collect();
    let mut c = para(
        72.0,
        700.0,
        &[
            "Thank you for your letter about the contract renewal. We have read the",
            "proposal carefully and we agree with the terms described in section two.",
        ],
    );
    c.push_str(&para(72.0, 656.8, &refs));
    c.push_str("BT /F1 12 Tf 72 240 Td (Sincerely,) Tj ET\n");
    c.push_str(&para(
        72.0,
        148.0,
        &["John Doe", "Director of Operations", "Example Company"],
    ));
    let doc = open_bytes(pdf(&c, &[]));
    let before = objects_of(&doc.doc_id);
    let find = |objs: &[PageObject], k: &str| {
        objs.iter()
            .find(|o| text_of(o).starts_with(k))
            .unwrap()
            .clone()
    };
    let p = probe(&doc.doc_id, 200.0, 697.0);
    let res = run(
        &doc.doc_id,
        p.doc_generation,
        edit_of(&p, &format!("{} {MORE} {MORE}", p.text), None, false),
        false,
    )
    .unwrap();
    assert!(res.shifted_pt > 20.0 && res.blocked.is_none(), "{res:?}");
    let after = &res.objects.objects;
    for k in ["Sincerely", "John Doe", "Director", "Example Company"] {
        assert!(
            moved_by(&find(&before, k), &find(after, k), -res.shifted_pt),
            "{k} did not follow the letter"
        );
    }
}

/// A push that the paragraph spacing absorbs (the room runs out first) never reports a clean
/// result while the new text's descenders reach into what follows: the spacing is measured
/// from the paragraph's line grid, and a line's clearance is kept.
#[test]
fn flow_descenders_never_overlap_what_the_gap_absorbed() {
    const TAIL: &str = "Then the stars come out one by one above the silent valley, the wind turns cold \
        and the fox curls up in its den to sleep until the morning light returns, gypsy pygmy jiggy.";
    let text = format!("{PARA_A_TEXT} {TAIL}");
    let doc_with = |figure_top: f32| {
        let mut c = String::from("BT /F2 18 Tf 72 740 Td (A Heading Line) Tj ET\n");
        c.push_str(&para(72.0, 700.0, &PARA_A));
        c.push_str(&para(
            72.0,
            620.0,
            &["Second paragraph starts here with words.", "and ends here."],
        ));
        c.push_str(&format!(
            "0.5 g 60 {} 492 120 re f 0 g\n",
            figure_top - 120.0
        ));
        open_bytes(pdf(&c, &[]))
    };
    let c_of = |objs: &[PageObject]| -> Vec<Rect> {
        objs.iter()
            .filter(|o| text_of(o).contains("Second") || text_of(o).contains("and ends"))
            .map(|o| o.rect)
            .collect()
    };
    // Where the old rule said "room + ink gap = growth + 1 pt: fits".
    let far = doc_with(300.0);
    let a = probe(&far.doc_id, 200.0, 690.0);
    let dry = run(
        &far.doc_id,
        a.doc_generation,
        edit_of(&a, &text, None, true),
        false,
    )
    .unwrap();
    let growth = (dry.lines - a.lines) as f32 * LEADING;
    let c0 = c_of(&objects_of(&far.doc_id));
    let (c_top, c_bottom) = (
        c0.iter().map(|r| r.t).fold(f32::MIN, f32::max),
        c0.iter().map(|r| r.b).fold(f32::MAX, f32::min),
    );
    let room = growth + 1.0 - (a.rect.b - c_top);
    assert!(room > 0.0, "{room}");
    let doc = doc_with(c_bottom - room);
    let a = probe(&doc.doc_id, 200.0, 690.0);
    let res = run(
        &doc.doc_id,
        a.doc_generation,
        edit_of(&a, &text, None, false),
        false,
    )
    .unwrap();
    let c_top_after = c_of(&res.objects.objects)
        .iter()
        .map(|r| r.t)
        .fold(f32::MIN, f32::max);
    let overlap = c_top_after - res.rect.b;
    assert!(
        res.blocked.is_some() && res.overflow_pt >= overlap,
        "reported {:?} / overflow {} while the new ink overlaps C by {overlap} pt",
        res.blocked,
        res.overflow_pt
    );
}

/// A page background (Word / Chrome "page colour": one rect as large as the page) is not a
/// frame around the paragraph: with nothing below, the text runs past the page's bottom
/// margin — nothing is "overlapped".
#[test]
fn flow_page_background_is_not_content_below() {
    let mut c = String::from("1 g 0 0 612 792 re f 0 g\n");
    c.push_str("BT /F2 18 Tf 72 740 Td (A Heading Line) Tj ET\n");
    c.push_str(&para(72.0, 100.0, &PARA_A));
    let doc = open_bytes(pdf(&c, &[]));
    let a = probe(&doc.doc_id, 150.0, 90.0);
    let text = format!("{PARA_A_TEXT} {EXTRA} {EXTRA}");
    for flow in [ParagraphFlow::Push, ParagraphFlow::Overlap] {
        let dry = run(
            &doc.doc_id,
            a.doc_generation,
            edit_of(&a, &text, Some(flow), true),
            false,
        )
        .unwrap();
        assert_eq!(dry.overflow_pt, 0.0, "{flow:?}: {dry:?}");
        assert!(dry.past_bottom_pt > 0.0, "{flow:?}: {dry:?}");
    }
}

/// The paragraph's stream is writable but the content below it shares a stream with an
/// inline image: that content cannot move, so it is an obstacle — the push is blocked (and
/// `overlap` / `fit` stay possible) instead of refusing the whole edit after the user typed.
#[test]
fn flow_content_below_in_an_unwritable_stream_stays() {
    let a = para(72.0, 700.0, &PARA_A);
    let below = format!(
        "{}q 200 0 0 20 72 560 cm BI /W 2 /H 1 /CS /G /BPC 8 /F /AHx ID 00FF> EI Q\n",
        para(
            72.0,
            620.0,
            &["Second paragraph starts here with words.", "and ends here."]
        )
    );
    let doc = open_bytes(pdf_streams(&[&a, &below], "", &[]));
    let before = objects_of(&doc.doc_id);
    let p = probe(&doc.doc_id, 200.0, 690.0);
    assert_eq!(p.strategy, TextEditStrategy::InPlace, "{p:?}");
    let longer = format!("{PARA_A_TEXT} {EXTRA} {EXTRA}");
    let dry = run(
        &doc.doc_id,
        p.doc_generation,
        edit_of(&p, &longer, None, true),
        false,
    )
    .expect("push dry run");
    assert_eq!(
        (dry.blocked, dry.moved_objects, dry.shifted_pt),
        (Some(FlowBlocked::Obstacle), 0, 0.0),
        "{dry:?}"
    );
    assert!(dry.overflow_pt > 0.0, "{dry:?}");
    let fit = run(
        &doc.doc_id,
        p.doc_generation,
        edit_of(&p, &longer, Some(ParagraphFlow::Fit), true),
        false,
    );
    assert!(fit.is_ok(), "{fit:?}");
    // A shorter paragraph cannot pull it up either; the edit itself goes through.
    let short = run(
        &doc.doc_id,
        p.doc_generation,
        edit_of(&p, "Short now.", None, true),
        false,
    )
    .unwrap();
    assert_eq!(short.shifted_pt, 0.0);
    // The blocked push, committed (the prompt's "overlap the rest"), writes and loses nothing.
    let res = run(
        &doc.doc_id,
        p.doc_generation,
        edit_of(&p, &longer, None, false),
        false,
    )
    .expect("write");
    assert_eq!(res.shifted_pt, 0.0);
    for (b, n) in survivors(&before, &p.object_ids, &res.objects.objects) {
        assert!(
            same_bits(b, n),
            "{:?} {:?} moved",
            b.object_type,
            text_of(b)
        );
    }
    let images = res
        .objects
        .objects
        .iter()
        .filter(|o| o.object_type == PageObjectType::Image)
        .count();
    assert_eq!(images, 1, "the inline image is still there");
}

fn is_dark(px: &[u8]) -> bool {
    (px[0] as u32 + px[1] as u32 + px[2] as u32) < 300
}

/// A page's content streams are one stream cut into pieces, and a producer may cut inside an
/// object or a `q … Q` block. Rewriting only the paragraph's piece used to draw the next
/// piece's text at another object's matrix and leave a clip open over the rest of the page
/// (160F-2019.pdf: the "6)" superscript landed on a footnote line and the right column went
/// blank). The rewrite is rehearsed, and such a page is regenerated whole.
#[test]
fn flow_split_content_streams_keep_the_rest_of_the_page() {
    // Piece 0 opens a clip over the heading band that piece 1 closes; piece 1 ends with the
    // text state of a line that piece 2 shows.
    let s0 = "q 0 690 612 102 re W n\nBT /F2 18 Tf 72 740 Td (A Heading Line) Tj ET\n".to_string();
    let s1 = format!("Q\n{}BT /F1 12 Tf 300 500 Td\n", para(72.0, 670.0, &PARA_A));
    let s2 = "(Split words) Tj ET\nBT /F1 12 Tf 72 400 Td (Below in the last stream) Tj ET\n"
        .to_string();
    let doc = open_bytes(pdf_streams(&[&s0, &s1, &s2], "", &[]));
    let before = objects_of(&doc.doc_id);
    let split = before
        .iter()
        .find(|o| text_of(o).contains("Split"))
        .unwrap()
        .clone();
    assert!(
        (split.matrix[4] - 300.0).abs() < 0.01 && (split.matrix[5] - 500.0).abs() < 0.01,
        "{split:?}"
    );
    let below = Rect::new(70.0, 395.0, 260.0, 412.0);
    let split_box = Rect::new(298.0, 496.0, 380.0, 512.0);
    let (dark_below, dark_split) = (
        count_pixels(&doc.doc_id, below, is_dark),
        count_pixels(&doc.doc_id, split_box, is_dark),
    );
    assert!(dark_below > 50 && dark_split > 50);
    let p = probe(&doc.doc_id, 200.0, 660.0);
    for flow in [ParagraphFlow::Overlap, ParagraphFlow::Push] {
        let dry = run(
            &doc.doc_id,
            p.doc_generation,
            edit_of(&p, &format!("{PARA_A_TEXT} {ONE_MORE}"), Some(flow), true),
            false,
        );
        assert!(dry.is_ok(), "{flow:?}: {dry:?}");
    }
    let res = run(
        &doc.doc_id,
        p.doc_generation,
        edit_of(
            &p,
            &format!("{PARA_A_TEXT} {ONE_MORE}"),
            Some(ParagraphFlow::Overlap),
            false,
        ),
        false,
    )
    .unwrap();
    for (b, n) in survivors(&before, &p.object_ids, &res.objects.objects) {
        assert!(
            same_bits(b, n),
            "{:?}: {:?} → {:?}",
            text_of(b),
            b.matrix,
            n.matrix
        );
    }
    // Everything still renders: nothing is left under the heading band's clip.
    assert!(
        count_pixels(&doc.doc_id, below, is_dark) * 10 >= dark_below * 9,
        "the last piece went blank"
    );
    assert!(
        count_pixels(&doc.doc_id, split_box, is_dark) * 10 >= dark_split * 9,
        "the split line moved"
    );
    let new_text = Rect::new(res.rect.l, res.rect.b, res.rect.r, 650.0);
    assert!(
        count_pixels(&doc.doc_id, new_text, is_dark) > 200,
        "the new text is clipped away"
    );
}

/// The same on the real file: any paragraph edit on 160F's first page keeps every other
/// object where it was, and the right column (the "6)" superscript and its row) still renders.
#[test]
fn flow_160f_edit_keeps_the_superscript_and_the_right_column() {
    let doc = open("160F-2019.pdf");
    let before = objects_of(&doc.doc_id);
    let region = Rect::new(440.0, 420.0, 560.0, 470.0);
    let dark = count_pixels(&doc.doc_id, region, is_dark);
    let p = probe(&doc.doc_id, 380.0, 325.0);
    assert_eq!(p.strategy, TextEditStrategy::InPlace, "{p:?}");
    let res = run(
        &doc.doc_id,
        p.doc_generation,
        edit_of(
            &p,
            &format!("{} {MORE}", p.text),
            Some(ParagraphFlow::Overlap),
            false,
        ),
        true,
    )
    .unwrap();
    for (b, n) in survivors(&before, &p.object_ids, &res.objects.objects) {
        assert!(
            b.object_type == n.object_type
                && (0..6).all(|k| (b.matrix[k] - n.matrix[k]).abs() <= 0.05),
            "{:?} {:?}: {:?} → {:?}",
            b.object_type,
            text_of(b),
            b.matrix,
            n.matrix
        );
    }
    let now = count_pixels(&doc.doc_id, region, is_dark);
    assert!(
        now * 10 >= dark * 9 && dark > 200,
        "the right column went blank: {dark} → {now} dark pixels"
    );
}

/// A SeePDF header stamp 4 pt above a paragraph's first line is not part of the paragraph:
/// the probe leaves it out and the edit neither deletes it nor weaves its text in.
#[test]
fn flow_header_stamp_never_joins_the_paragraph() {
    let body: Vec<String> = (0..30)
        .map(|k| format!("Body line {k} of the text that follows the edited paragraph."))
        .collect();
    let refs: Vec<&str> = body.iter().map(String::as_str).collect();
    let mut c = para(
        72.0,
        740.0,
        &[
            "The edited paragraph near the top of the page, which gets longer when",
            "the user types more words into it.",
        ],
    );
    c.push_str(&para(72.0, 700.0, &refs));
    let doc = open_bytes(pdf(&c, &[]));
    let id = doc.doc_id.clone();
    let header = PageStampSpec {
        role: StampRole::Header,
        source: PageStampSource::Text {
            text: "Page {{page}} of {{pages}}".into(),
            font_size_pt: 10.0,
            color: [0, 0, 0],
        },
        anchor: StampAnchor::Tc,
        margin_pt: 36.0,
        rotate_deg: 0.0,
        opacity: 1.0,
        pages: PageSelection::All(AllPages::All),
        bates: Default::default(),
        behind: false,
    };
    with_state(move |st| stamp::add_stamp(st, &id, &header)).expect("header stamp");
    let before = objects_of(&doc.doc_id);
    let stamp_obj = before
        .iter()
        .find(|o| text_of(o).contains("Page 1"))
        .unwrap()
        .clone();
    let p = probe(&doc.doc_id, 200.0, 735.0);
    assert!(!p.object_ids.contains(&stamp_obj.object_id), "{p:?}");
    assert!(!p.text.contains("Page 1"), "{}", p.text);
    let res = run(
        &doc.doc_id,
        p.doc_generation,
        edit_of(&p, &format!("{} {MORE}", p.text), None, false),
        false,
    )
    .unwrap();
    let kept = res
        .objects
        .objects
        .iter()
        .find(|o| text_of(o).contains("Page 1"))
        .expect("the stamp is still there");
    assert!(same_bits(&stamp_obj, kept), "the stamp never moves");
    let again = probe(&doc.doc_id, 200.0, 735.0);
    assert!(
        !again.text.contains("Page 1") && again.text.ends_with(MORE),
        "{}",
        again.text
    );
    // An edit naming the stamp's object is refused.
    let mut e = edit_of(&p, "x", None, true);
    e.object_ids = vec![kept.object_id];
    let generation = res.objects.doc_generation;
    let err = run(&doc.doc_id, generation, e, false).expect_err("a stamp is not paragraph text");
    assert_eq!(err.code, ErrorCode::InvalidArgument, "{err:?}");
}

/// A justified paragraph whose new text has long words keeps its alignment: lines that could
/// not be stretched to the right edge (the word gap is capped) still count as justified, so
/// the next edit does not turn it ragged.
#[test]
fn flow_justified_paragraph_with_long_words_stays_justified() {
    let doc = open("tracemonkey.pdf");
    let p = probe(&doc.doc_id, 420.0, 400.0);
    assert_eq!(p.align, ParagraphAlign::Justify, "{p:?}");
    let add = "Counterintuitively, the internationalization infrastructure responsibilities include \
               characterization, compartmentalization and telecommunications interoperability considerations.";
    let res = run(
        &doc.doc_id,
        p.doc_generation,
        edit_of(&p, &format!("{} {add}", p.text), None, false),
        false,
    )
    .unwrap();
    let again = probe(&doc.doc_id, 420.0, 400.0);
    assert_eq!(again.lines, res.lines);
    assert_eq!(again.align, ParagraphAlign::Justify, "{again:?}");
}

/// Emptying a paragraph deletes it and pulls what follows up into its place (the top of the
/// first thing below moves to the paragraph's top) in one undo step — it used to leave a hole.
/// `overlap` only deletes.
#[test]
fn flow_emptied_paragraph_pulls_what_follows_into_its_place() {
    let doc = open_bytes(pdf(&flow_content(), &[]));
    let before = objects_of(&doc.doc_id);
    let a = probe(&doc.doc_id, 200.0, 690.0);
    let c_top = before
        .iter()
        .filter(|o| is_c_or_d(&text_of(o)))
        .map(|o| o.rect.t)
        .fold(f32::MIN, f32::max);
    let pull = a.rect.t - c_top;
    for text in ["", "   ", "\n"] {
        let dry = run(
            &doc.doc_id,
            a.doc_generation,
            edit_of(&a, text, None, true),
            false,
        )
        .expect("dry run");
        assert_eq!(
            (dry.lines, dry.moved_objects, dry.blocked),
            (0, 4, None),
            "{text:?}: {dry:?}"
        );
        assert!(
            (dry.shifted_pt + pull).abs() < 0.02,
            "{text:?}: {} vs -{pull}",
            dry.shifted_pt
        );
    }
    let over = run(
        &doc.doc_id,
        a.doc_generation,
        edit_of(&a, "", Some(ParagraphFlow::Overlap), true),
        false,
    )
    .unwrap();
    assert_eq!((over.shifted_pt, over.moved_objects), (0.0, 0));
    assert_eq!(
        generation(&doc.doc_id),
        a.doc_generation,
        "dry runs change nothing"
    );

    let res = run(
        &doc.doc_id,
        a.doc_generation,
        edit_of(&a, "", None, false),
        false,
    )
    .expect("emptied");
    let after = &res.objects.objects;
    assert_eq!(
        after.len(),
        before.len() - a.object_ids.len(),
        "only the paragraph's objects are gone"
    );
    for (b, n) in survivors(&before, &a.object_ids, after) {
        if is_c_or_d(&text_of(b)) {
            assert!(
                moved_by(b, n, pull),
                "{:?} {:?} → {:?}",
                text_of(b),
                b.rect,
                n.rect
            );
        } else {
            assert!(same_bits(b, n), "{:?} moved", text_of(b));
        }
    }
    let c_top_after = after
        .iter()
        .filter(|o| is_c_or_d(&text_of(o)))
        .map(|o| o.rect.t)
        .fold(f32::MIN, f32::max);
    assert!(
        (c_top_after - a.rect.t).abs() < 0.02,
        "C took A's place: {c_top_after} vs {}",
        a.rect.t
    );
    // One undo step brings the paragraph and the layout back.
    let id = doc.doc_id.clone();
    with_state(move |st| registry::undo(st, &id, false)).expect("undo");
    let restored = objects_of(&doc.doc_id);
    assert_eq!(restored.len(), before.len());
    for (b, n) in before.iter().zip(&restored) {
        assert!(same_bits(b, n), "{:?} not restored", text_of(b));
    }
}
