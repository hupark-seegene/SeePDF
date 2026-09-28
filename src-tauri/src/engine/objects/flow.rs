//! Stage 9 — the content that follows an edited paragraph (`IPC_CONTRACT.md` §7.4b, "flow").
//!
//! A reflowed paragraph that grew used to be drawn over the lines below it — the user's report
//! was "글을 추가로 입력하면, 뒤에 문장은 겹쳐 보이는 문제가 있어". [`plan`] decides what
//! follows the paragraph and how far it may move, [`decide`] turns that into a shift for the
//! requested [`ParagraphFlow`], and `paragraph::edit` applies it **inside the same
//! `registry::mutate` as the text itself**, so the edit and every move are one undo step.
//!
//! ## Direction
//!
//! Everything is in the page's user space. The paragraph detector only accepts text that is
//! upright in user space and `layout` places lines at `baseline − row × leading`, so "below"
//! is −y on every page. On a `/Rotate 90` page that is the paragraph's own reading direction
//! (what the text's lines follow), not the screen's; the crop box is used in user space too.
//!
//! ## The rules
//!
//! * **Column** — the horizontal range of the original rect ∪ the new rect, widened to the
//!   paragraph's **text block** (`text_block`): a text line below that overlaps the column and
//!   starts (ends) within `BLOCK_EDGE` × size of its left (right) edge, or wraps it on both
//!   sides, belongs to the same block — the body under a one-line, ragged or block-quote
//!   paragraph follows it — unless it also reaches into another column's content (`beside`:
//!   a caption spanning two columns stays an obstacle).
//! * **Below** — an object whose top is at or below the paragraph's *original* bottom
//!   (+ 0.25 × font size of tolerance). Anything above is never moved.
//! * **In-column** — the object overlaps the column by ≥ 60 % of its own width **and** sticks
//!   out of it by ≤ 15 % of the column width on either side. On a two-column page a
//!   right-column paragraph's column never takes in left-column content (no overlap at all).
//! * **Continuation lines** — a text line that starts at the left edge of a movable line just
//!   above it (`CONTINUE_EDGE`, `CONTINUE_STEP`) is movable too: a paragraph's short last
//!   line is never an obstacle inside its own paragraph.
//! * **List markers** — a narrow object (`MARKER_WIDTH`) just left of a movable line's text
//!   (`MARKER_GAP`), on its baseline band, that belongs to no other column moves with it.
//! * **Obstacles** — what does not follow the paragraph but is in its way:
//!   - objects below that overlap the column without being in it (full-width figures, tables
//!     spanning columns);
//!   - AcroForm widgets below that overlap it;
//!   - SeePDF **header / footer stamps** below that overlap it (`SeePDF:Stamp` with role
//!     `header` / `footer`: a page number stamped at the bottom margin);
//!   - the **running footer** of the column;
//!   - the bottom edge of a **container**: a non-text object that overlaps at least half of the
//!     column, starts above the paragraph's bottom and ends below it, and is not taller than
//!     half the page (a frame or shaded box around the paragraph, a table grid drawn as one
//!     path — not a page background). Content inside it moves at most down to its bottom edge
//!     (3 pt above its bounds, clear of a stroke), never across it;
//!   - an in-column object that **straddles** an obstacle's top (a line wrapped beside a figure
//!     that sticks out of the column): it cannot move, so its own top is in the way.
//! * **Running footer** — found among the body content that overlaps the column (so the other
//!   column of a two-column page never hides it): the lowest band, separated from everything
//!   above by a gap ≥ 2 × the page's line pitch (1.5 × for a short item — narrower than a
//!   quarter of the column, a page number at LaTeX's `\footskip` or Word's footer distance;
//!   0.75 × for a short item inside the bottom margin) and starting in the bottom 20 % of the
//!   crop box, that also **looks like a footer**: short, inside the bottom inch, centred, set
//!   smaller than the body above it (footnotes, copyright blocks) or under a thin rule. A
//!   letter's signature block is none of these and follows the text. The **line pitch** is the
//!   median baseline step of the text in the column, so it does not depend on which paragraph
//!   is edited (a large heading's leading would otherwise hide the page number). Never moved;
//!   its top is an obstacle. Not a footer when the edited paragraph is part of it.
//! * **Movable** — below, in the column (or a continuation line or a marker), not footer, and
//!   not below the first obstacle (its bottom at or above the highest obstacle top). Text,
//!   image, path, Form XObject and shading objects; the paragraph's own objects are replaced,
//!   not moved.
//! * **Never moved, never an obstacle** — watermark stamps (and stamps without a role),
//!   invisible text (an OCR layer stays on its scan) and every object above the paragraph.
//! * **Page bottom margin** (the *floor*) — `max(crop.b, min(lowest body bottom, crop.b +
//!   18 pt))`: content may move into the page's bottom margin down to 18 pt above the crop
//!   box edge, never below content that already sits lower, and never out of the crop box.
//!   "Body" leaves out stamps, the footer and objects taller than half the page (backgrounds,
//!   frames).
//! * **Room** — from the bottom of the movable stack (the paragraph's own line-grid bottom
//!   when nothing is movable) down to the higher of the first obstacle's top and the floor; ≥ 0.
//! * **Gap** and **free** — measured from the paragraph's **line grid** (last baseline − ¼
//!   size, the reference of the growth) and keeping a line's clearance (`leading − size`) above
//!   what follows: the gap reaches the top of the highest movable object, the free space the
//!   first content or obstacle below (or the floor, without clearance). A push moves the stack
//!   by the growth (or by what the new ink needs, if a deep descender needs more) up to the
//!   room, and is **blocked** only when the new ink still comes closer than the clearance to
//!   what follows; that remainder is the overflow.
//! * **Nothing below** — no content, footer, header / footer stamp or widget below the
//!   paragraph in its column (a frame's edge does not count): the paragraph itself grows toward
//!   the floor. Past it, a push is blocked with **no** overflow (nothing is overlapped) and the
//!   distance past the floor is reported separately (`pastBottomPt`).
//! * **Annotations** in the column below the paragraph whose vertical centre lies inside the
//!   moved band (top ≤ the paragraph's bottom + tolerance, bottom ≥ the stack's bottom −
//!   tolerance) move by the same distance; widgets and popups never move. See [`move_annots`]
//!   for what "move" means per kind. The band is reported (`movedBand`) so the UI can move
//!   pending 영역 표시 marks the same way.

use crate::engine::raw::{self, consts};
use crate::ipc::types::{FlowBlocked, ParagraphFlow, Rect};
use crate::ipc::EngineError;
use pdfium_render::prelude::{PdfPage, PdfiumLibraryBindings};
use std::collections::HashSet;
use std::os::raw::c_int;

/// "Below" tolerance, × the paragraph's font size.
const BELOW_TOL: f32 = 0.25;
/// In-column: overlap with the column ≥ this fraction of the object's own width…
const IN_COLUMN_OVERLAP: f32 = 0.6;
/// …and no side sticking out by more than this fraction of the column width.
const IN_COLUMN_OVERHANG: f32 = 0.15;
/// Narrower than this (a vertical rule) and the object's centre decides.
const NARROW: f32 = 0.5;
/// Running footer: separated from the body by at least this × the page's line pitch…
const FOOTER_GAP: f32 = 2.0;
/// …or this × the pitch when the footer is short (a page number)…
const FOOTER_GAP_SHORT: f32 = 1.5;
/// …or this × the pitch when it is short and lies in the page's bottom margin (a page number
/// under a full page: clearly more than a line's gap, less than a blank line)…
const FOOTER_GAP_MARGIN: f32 = 0.75;
/// …"short" meaning no wider than this fraction of the column…
const FOOTER_SHORT: f32 = 0.25;
/// …and starting in this bottom fraction of the crop box.
const FOOTER_BAND: f32 = 0.2;
/// A footer also has to look like one (see [`running_footer`]): it lies in the page's bottom
/// margin — its top at most this far above the crop box edge (Word's 1" margin)…
const FOOTER_MARGIN: f32 = 72.0;
/// …or it is set smaller than the body text above it (at most this × its median size)…
const FOOTER_SMALLER: f32 = 0.9;
/// …or it is centred in the column (its centre within this fraction of the column width of the
/// column's, and not as wide as the column)…
const FOOTER_CENTRED: f32 = 0.05;
/// …"not as wide" meaning narrower than this fraction of the column…
const FOOTER_NARROW: f32 = 0.8;
/// …or a thin rule (a footnote separator) starts it: a non-text object at most this tall.
const FOOTER_RULE: f32 = 2.0;
/// Text below that shares the paragraph's text block widens the column: a line that overlaps
/// it and starts (or ends) within this × size of its left (right) edge, or wraps it on both
/// sides — the body text under a short, ragged or indented paragraph.
const BLOCK_EDGE: f32 = 2.5;
/// A line continues a movable line above it when it starts within this × size of that line's
/// left edge…
const CONTINUE_EDGE: f32 = 0.5;
/// …and its baseline is at most this × size below (farther is a blank line or more).
const CONTINUE_STEP: f32 = 2.2;
/// A list marker (bullet, dash, number) left of a movable line: at most this × size away from
/// the line's text…
const MARKER_GAP: f32 = 2.0;
/// …and at most this × size wide.
const MARKER_WIDTH: f32 = 3.0;
/// Fewer baseline steps than this in the column and the pitch falls back to the paragraph's
/// own leading.
const PITCH_MIN_STEPS: usize = 3;
/// Two baselines closer than this × the smaller size are one line (a superscript), not a step.
const PITCH_MIN_STEP: f32 = 0.8;
/// Taller than this fraction of the crop box: a background or frame, not body content.
const TALL: f32 = 0.5;
/// A container overlaps at least this fraction of the column's width.
const CONTAINER_WIDTH: f32 = 0.5;
/// Content inside a container stops this far above its bottom bound: PDFium's bounds of a
/// stroked path reach about a line width past the edge, so this clears strokes up to 2 pt.
const CONTAINER_INSET: f32 = 3.0;
/// The page bottom margin the flow may use, above the crop box edge.
const BOTTOM_MARGIN: f32 = 18.0;
/// A movable object may reach this far past the first obstacle's top and still count as
/// sitting above it (touching bounds).
const EDGE_TOL: f32 = 1.0;
/// Shifts smaller than this are not worth rewriting the page for.
pub const MIN_SHIFT: f32 = 0.05;

/// What a top-level page object is for the flow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemKind {
    /// Body content: may move, may be an obstacle, counts for the footer and the floor.
    Content,
    /// A watermark stamp (or a stamp without a role): invisible to the flow.
    Stamp,
    /// A SeePDF header / footer stamp: never moved, an obstacle when it is in the way.
    Fixed,
    /// One of the edited paragraph's own objects (replaced, never moved).
    Paragraph,
    /// Left alone: an object type PDFium cannot write back, or invisible text (an OCR layer
    /// belongs to the scan image under it, which never moves either).
    Ignored,
}

/// The baseline and rendered size of an upright, visible text object (the line pitch).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TextLine {
    pub baseline: f32,
    pub size: f32,
}

/// One top-level page object.
#[derive(Debug, Clone, Copy)]
pub struct Item {
    pub index: usize,
    pub bounds: Rect,
    pub kind: ItemKind,
    /// `Some` for upright, visible text.
    pub line: Option<TextLine>,
}

/// One annotation of the page.
#[derive(Debug, Clone, Copy)]
pub struct AnnotItem {
    pub index: usize,
    pub subtype: c_int,
    pub rect: Rect,
}

/// The paragraph before and after, in the page's user space.
#[derive(Debug, Clone, Copy)]
pub struct Geometry {
    pub crop: Rect,
    /// Ink bounds of the original paragraph.
    pub original: Rect,
    /// Ink bounds of the new text.
    pub new_rect: Rect,
    /// The original paragraph's font size (the tolerances).
    pub size: f32,
    /// The original paragraph's leading: the line pitch when the column has too few lines to
    /// measure one, and the clearance kept above what follows (`leading − size`).
    pub leading: f32,
    /// The original paragraph's grid bottom: its last baseline minus the nominal descent. The
    /// growth is measured on this grid, so the gap and the free space below are too.
    pub grid_bottom: f32,
}

/// What follows the paragraph and how far it may go.
#[derive(Debug, Clone)]
pub struct Plan {
    /// Page object indices that follow the paragraph, ascending.
    pub movable: Vec<usize>,
    /// Annotation indices that move with them, ascending.
    pub annots: Vec<usize>,
    /// How far the movable stack can move down (≥ 0).
    pub room: f32,
    /// What bounds `room`.
    pub limit: FlowBlocked,
    /// The gap between the original bottom and the first content below it (or the floor):
    /// how far the paragraph can grow before it covers anything when nothing moves.
    pub free: f32,
    /// The gap between the original bottom and the top of the highest movable object (0 when
    /// nothing is movable): what a push can use up before the text touches what follows.
    pub gap: f32,
    /// The top of the highest movable object (`None` when nothing is movable): where what
    /// follows starts, which an emptied paragraph's place is taken from.
    pub stack_top: Option<f32>,
    /// Nothing below the paragraph in its column and nothing in its way: only the floor.
    pub nothing_below: bool,
    /// The running footer's object indices (never moved).
    pub footer: Vec<usize>,
    /// The region (before the move) whose content and annotations move: the column, from the
    /// paragraph's bottom (+ tolerance) down to the stack's bottom (− tolerance). `None` when
    /// nothing is movable.
    pub band: Option<Rect>,
}

/// What the flow does for one edit.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Decision {
    /// > 0 = the movable set goes down by this much, < 0 = up, 0 = nothing moves.
    pub shift: f32,
    pub blocked: Option<FlowBlocked>,
    /// How far the new text still extends over the content below.
    pub overflow: f32,
    /// Nothing below: how far the new text runs past the floor.
    pub past_bottom: f32,
}

fn valid(r: &Rect) -> bool {
    [r.l, r.b, r.r, r.t].iter().all(|v| v.is_finite())
        && r.r >= r.l
        && r.t >= r.b
        && (r.width() > 0.0 || r.height() > 0.0)
        && *r != Rect::ZERO
}

#[derive(Debug, Clone, Copy)]
struct Column {
    l: f32,
    r: f32,
}

impl Column {
    fn of(a: &Rect, b: &Rect) -> Self {
        Self {
            l: a.l.min(b.l),
            r: a.r.max(b.r),
        }
    }

    fn width(&self) -> f32 {
        self.r - self.l
    }

    fn centre_inside(&self, r: &Rect) -> bool {
        let cx = (r.l + r.r) / 2.0;
        cx >= self.l && cx <= self.r
    }

    fn overlap(&self, r: &Rect) -> f32 {
        self.r.min(r.r) - self.l.max(r.l)
    }

    fn overlaps(&self, r: &Rect) -> bool {
        if r.width() < NARROW {
            return self.centre_inside(r);
        }
        self.overlap(r) > 0.0
    }

    fn contains(&self, r: &Rect) -> bool {
        let w = r.width();
        if w < NARROW {
            return self.centre_inside(r);
        }
        let slack = IN_COLUMN_OVERHANG * self.width();
        self.overlap(r) >= IN_COLUMN_OVERLAP * w && r.l >= self.l - slack && r.r <= self.r + slack
    }
}

/// Body content for the footer and the floor: not a stamp, sane bounds, not a page-sized
/// background, at least partly inside the crop box vertically.
fn body<'a>(items: &'a [Item], crop: &Rect) -> impl Iterator<Item = &'a Item> + 'a {
    let crop = *crop;
    items.iter().filter(move |it| {
        matches!(it.kind, ItemKind::Content | ItemKind::Paragraph)
            && valid(&it.bounds)
            && it.bounds.height() <= TALL * crop.height()
            && it.bounds.t > crop.b
            && it.bounds.b < crop.t
    })
}

/// The line pitch of the text in the column: the median step between consecutive baselines
/// (superscripts folded into their line). `fallback` when the column has too few lines.
fn line_pitch(items: &[Item], crop: &Rect, column: &Column, fallback: f32) -> f32 {
    let mut lines: Vec<TextLine> = body(items, crop)
        .filter(|it| column.overlaps(&it.bounds))
        .filter_map(|it| it.line)
        .filter(|l| l.baseline.is_finite() && l.size > 0.0)
        .collect();
    lines.sort_by(|a, b| b.baseline.total_cmp(&a.baseline));
    let mut steps: Vec<f32> = Vec::new();
    let mut prev: Option<TextLine> = None;
    for l in lines {
        if let Some(p) = prev {
            let step = p.baseline - l.baseline;
            if step < PITCH_MIN_STEP * p.size.min(l.size) {
                // The same line (or a raised / lowered part of it): the larger text's
                // baseline is the line's.
                prev = Some(if l.size > p.size { l } else { p });
                continue;
            }
            steps.push(step);
        }
        prev = Some(l);
    }
    if steps.len() < PITCH_MIN_STEPS {
        return fallback;
    }
    steps.sort_by(f32::total_cmp);
    steps[steps.len() / 2]
}

/// The running footer of the column: `(top, indices)`, see the module docs.
fn running_footer(items: &[Item], g: &Geometry, column: &Column) -> Option<(f32, Vec<usize>)> {
    let mut sorted: Vec<&Item> = body(items, &g.crop)
        .filter(|it| column.overlaps(&it.bounds))
        .collect();
    sorted.sort_by(|a, b| a.bounds.b.total_cmp(&b.bounds.b));
    let pitch = line_pitch(items, &g.crop, column, g.leading.max(g.size));
    let mut top = f32::MIN;
    let (mut left, mut right) = (f32::MAX, f32::MIN);
    let mut split = None;
    for (k, it) in sorted.iter().enumerate() {
        if k > 0 {
            let short = right - left <= FOOTER_SHORT * column.width();
            let in_margin = top <= g.crop.b + FOOTER_MARGIN;
            let need = match (short, in_margin) {
                (true, true) => FOOTER_GAP_MARGIN,
                (true, false) => FOOTER_GAP_SHORT,
                _ => FOOTER_GAP,
            } * pitch;
            if it.bounds.b - top >= need {
                split = Some(k);
                break;
            }
        }
        top = top.max(it.bounds.t);
        left = left.min(it.bounds.l);
        right = right.max(it.bounds.r);
    }
    let split = split?;
    let cluster = &sorted[..split];
    if top > g.crop.b + FOOTER_BAND * g.crop.height()
        || cluster.iter().any(|it| it.kind == ItemKind::Paragraph)
    {
        return None;
    }
    // A block at the page bottom is not a footer just because a gap separates it (a letter's
    // signature block after the space for the signature): it must look like one.
    let width = right - left;
    let short = width <= FOOTER_SHORT * column.width();
    let in_margin = top <= g.crop.b + FOOTER_MARGIN;
    let centre = (left + right) / 2.0;
    let centred = width < FOOTER_NARROW * column.width()
        && (centre - (column.l + column.r) / 2.0).abs() <= FOOTER_CENTRED * column.width();
    let median = |mut v: Vec<f32>| -> Option<f32> {
        v.sort_by(f32::total_cmp);
        (!v.is_empty()).then(|| v[v.len() / 2])
    };
    let body_size = median(sorted[split..].iter().filter_map(|it| it.line).map(|l| l.size).collect());
    let footer_size = cluster
        .iter()
        .filter_map(|it| it.line)
        .map(|l| l.size)
        .reduce(f32::max);
    let smaller = matches!((footer_size, body_size), (Some(f), Some(b)) if f <= FOOTER_SMALLER * b);
    let ruled = cluster
        .iter()
        .max_by(|a, b| a.bounds.t.total_cmp(&b.bounds.t))
        .is_some_and(|it| it.line.is_none() && it.bounds.height() <= FOOTER_RULE);
    if !(short || in_margin || centred || smaller || ruled) {
        return None;
    }
    let mut indices: Vec<usize> = cluster.iter().map(|it| it.index).collect();
    indices.sort_unstable();
    Some((top, indices))
}

/// See the module docs ("page bottom margin").
fn floor(items: &[Item], footer: &HashSet<usize>, g: &Geometry) -> f32 {
    let lowest = body(items, &g.crop)
        .filter(|it| !footer.contains(&it.index))
        .map(|it| it.bounds.b)
        .fold(f32::MAX, f32::min);
    let margin = g.crop.b + BOTTOM_MARGIN;
    lowest.min(margin).max(g.crop.b)
}

/// Content of another column (or a sidebar), `(index, bounds)`: body content from the
/// paragraph's top down that does not overlap `column` and does not hug its edge (list markers
/// and margin labels — within [`MARKER_GAP`] + [`MARKER_WIDTH`] of it — are the column's own).
fn beside(items: &[Item], g: &Geometry, column: &Column) -> Vec<(usize, Rect)> {
    let hug = (MARKER_GAP + MARKER_WIDTH) * g.size;
    body(items, &g.crop)
        .filter(|it| it.kind == ItemKind::Content && it.bounds.b < g.original.t)
        .filter(|it| !column.overlaps(&it.bounds))
        .filter(|it| {
            let r = &it.bounds;
            !(r.r <= column.l && r.l >= column.l - hug || r.l >= column.r && r.r <= column.r + hug)
        })
        .map(|it| (it.index, it.bounds))
        .collect()
}

/// `r` reaches across into content beside the column that sits between the paragraph's top and
/// `r`'s bottom: a line spanning two columns, not a line of this one.
fn reaches_across(r: &Rect, beside: &[(usize, Rect)]) -> bool {
    beside.iter().any(|(_, y)| y.l < r.r && y.r > r.l && y.t > r.b)
}

/// The paragraph's column widened to its text block: body text below that overlaps the column
/// and shares its left or right margin (or wraps it on both sides) belongs to the same block —
/// the text under a short one-line paragraph, a ragged paragraph or a block quote — unless it
/// also reaches into another column's content (a caption spanning two columns).
fn text_block(items: &[Item], g: &Geometry) -> Column {
    let mut column = Column::of(&g.original, &g.new_rect);
    let below = g.original.b + BELOW_TOL * g.size;
    let edge = BLOCK_EDGE * g.size;
    let lines: Vec<&Item> = body(items, &g.crop)
        .filter(|it| it.kind == ItemKind::Content && it.line.is_some() && it.bounds.t <= below)
        .collect();
    loop {
        let others = beside(items, g, &column);
        let mut grown = column;
        for it in &lines {
            let r = &it.bounds;
            if column.contains(r) || !column.overlaps(r) {
                continue;
            }
            let joins = (r.l - column.l).abs() <= edge
                || (r.r - column.r).abs() <= edge
                || (r.l <= column.l + edge && r.r >= column.r - edge);
            if joins && !reaches_across(r, &others) {
                grown.l = grown.l.min(r.l);
                grown.r = grown.r.max(r.r);
            }
        }
        if grown.l >= column.l && grown.r <= column.r {
            return column;
        }
        column = grown;
    }
}

/// What follows the paragraph, what is in its way and how far it may move.
pub fn plan(items: &[Item], annots: &[AnnotItem], g: &Geometry) -> Plan {
    let tol = BELOW_TOL * g.size;
    let column = text_block(items, g);
    let bottom = g.original.b;
    let below = |r: &Rect| r.t <= bottom + tol;

    let (footer_top, footer) = match running_footer(items, g, &column) {
        Some((top, indices)) => (Some(top), indices),
        None => (None, Vec::new()),
    };
    let footer_set: HashSet<usize> = footer.iter().copied().collect();

    // The first obstacle below the paragraph (highest top) and the in-column candidates.
    let mut obstacle = footer_top;
    let raise = |obstacle: &mut Option<f32>, top: f32| {
        *obstacle = Some(obstacle.map_or(top, |o: f32| o.max(top)));
    };
    // Content, a footer, a header / footer stamp or a widget below the paragraph: what follows
    // it or is in its way (a container's edge is not "content below").
    let mut anything_below = footer_top.is_some();
    let mut candidates: Vec<&Item> = Vec::new();
    // Below and overlapping the column without being in it: obstacles unless they continue a
    // movable line.
    let mut overlapping: Vec<&Item> = Vec::new();
    // Below, beside the column: list markers, or nothing to the flow.
    let mut outside: Vec<&Item> = Vec::new();
    for it in items {
        if !valid(&it.bounds) || footer_set.contains(&it.index) {
            continue;
        }
        match it.kind {
            ItemKind::Fixed => {
                if below(&it.bounds) && column.overlaps(&it.bounds) {
                    raise(&mut obstacle, it.bounds.t);
                    anything_below = true;
                }
            }
            ItemKind::Content if !below(&it.bounds) => {
                // A frame / shaded box / table grid around the paragraph: what it holds stays
                // inside it. A page background or page frame is not a container.
                let container = it.line.is_none()
                    && it.bounds.b < bottom - tol
                    && it.bounds.height() <= TALL * g.crop.height()
                    && column.overlap(&it.bounds) >= CONTAINER_WIDTH * column.width();
                if container {
                    raise(&mut obstacle, it.bounds.b + CONTAINER_INSET);
                }
            }
            ItemKind::Content => {
                if column.contains(&it.bounds) {
                    candidates.push(it);
                } else if column.overlaps(&it.bounds) {
                    overlapping.push(it);
                } else {
                    outside.push(it);
                }
            }
            _ => {}
        }
    }
    let others = beside(items, g, &column);
    // Lines that continue a movable line above them at its left edge (the short last line of
    // the paragraph below): movable too, top-down so a chain follows its first line.
    let mut rest: Vec<&Item> = overlapping
        .iter()
        .chain(outside.iter())
        .copied()
        .filter(|it| it.line.is_some())
        .collect();
    rest.sort_by(|a, b| {
        let (a, b) = (a.line.map_or(0.0, |l| l.baseline), b.line.map_or(0.0, |l| l.baseline));
        b.total_cmp(&a)
    });
    let mut joined: HashSet<usize> = HashSet::new();
    // Movable text lines as (baseline, size, left), by baseline, for the continuation test.
    let mut starts: Vec<(f32, f32, f32)> = candidates
        .iter()
        .filter_map(|c| c.line.map(|l| (l.baseline, l.size, c.bounds.l)))
        .collect();
    starts.sort_by(|a, b| a.0.total_cmp(&b.0));
    let reach = CONTINUE_STEP * starts.iter().map(|s| s.1).fold(g.size, f32::max);
    for it in rest {
        let Some(line) = it.line else { continue };
        let from = starts.partition_point(|s| s.0 <= line.baseline);
        let to = starts.partition_point(|s| s.0 <= line.baseline + reach);
        let continues = starts[from..to].iter().any(|&(baseline, size, left)| {
            let step = baseline - line.baseline;
            step >= PITCH_MIN_STEP * size.min(line.size)
                && step <= CONTINUE_STEP * size
                && (it.bounds.l - left).abs() <= CONTINUE_EDGE * size
        });
        if continues && !reaches_across(&it.bounds, &others) {
            candidates.push(it);
            joined.insert(it.index);
            let at = starts.partition_point(|s| s.0 <= line.baseline);
            starts.insert(at, (line.baseline, line.size, it.bounds.l));
        }
    }
    // List markers: a narrow object just left of a movable line's text, on its baseline band,
    // that belongs to no other column — it goes with its item.
    let lines: Vec<&Item> = candidates.iter().copied().filter(|c| c.line.is_some()).collect();
    let largest = lines.iter().filter_map(|c| c.line).map(|l| l.size).fold(g.size, f32::max);
    // A movable line starts at most 15 % of the column width left of it (see `Column::contains`).
    let leftmost = column.l - IN_COLUMN_OVERHANG * column.width() - MARKER_GAP * largest;
    for it in &outside {
        let m = &it.bounds;
        if joined.contains(&it.index)
            || m.r > column.l
            || m.r < leftmost
            || m.width() > MARKER_WIDTH * largest
        {
            continue;
        }
        let centre = (m.b + m.t) / 2.0;
        let marks = lines.iter().any(|c| {
            let size = c.line.map_or(g.size, |l| l.size);
            centre >= c.bounds.b
                && centre <= c.bounds.t
                && m.r <= c.bounds.l
                && c.bounds.l - m.r <= MARKER_GAP * size
                && m.width() <= MARKER_WIDTH * size
        });
        if marks && !others.iter().any(|(i, y)| *i != it.index && y.l < m.r && y.r > m.l) {
            candidates.push(it);
            joined.insert(it.index);
        }
    }
    for it in &overlapping {
        if !joined.contains(&it.index) {
            raise(&mut obstacle, it.bounds.t);
        }
    }
    anything_below |= !candidates.is_empty() || overlapping.iter().any(|it| !joined.contains(&it.index));
    for a in annots {
        if a.subtype == consts::FPDF_ANNOT_WIDGET
            && valid(&a.rect)
            && below(&a.rect)
            && column.overlaps(&a.rect)
        {
            raise(&mut obstacle, a.rect.t);
            anything_below = true;
        }
    }
    // An in-column object that reaches below the first obstacle's top cannot move; when it also
    // reaches above it (a line wrapped beside a figure), its own top is what is in the way.
    while let Some(top) = obstacle {
        let straddling = candidates
            .iter()
            .filter(|it| it.bounds.b < top - EDGE_TOL && it.bounds.t > top)
            .map(|it| it.bounds.t)
            .fold(top, f32::max);
        if straddling <= top {
            break;
        }
        obstacle = Some(straddling);
    }

    let movable: Vec<&Item> = candidates
        .iter()
        .copied()
        .filter(|it| obstacle.is_none_or(|top| it.bounds.b >= top - EDGE_TOL))
        .collect();
    // Nothing moves: the paragraph's own line grid is what grows toward the obstacle or floor.
    let stack_bottom = movable
        .iter()
        .map(|it| it.bounds.b)
        .reduce(f32::min)
        .unwrap_or(g.grid_bottom);
    let floor = floor(items, &footer_set, g);
    let room_floor = stack_bottom - floor;
    let (room, limit) = match obstacle {
        Some(top) if stack_bottom - top <= room_floor => (stack_bottom - top, FlowBlocked::Obstacle),
        _ => (room_floor, FlowBlocked::PageBottom),
    };

    // Distances from the paragraph's line grid, keeping a line's clearance above content.
    let clear = (g.leading - g.size).max(0.0);
    let content_top = candidates.iter().map(|it| it.bounds.t).chain(obstacle).reduce(f32::max);
    let to_floor = (g.grid_bottom - floor).max(0.0);
    let free = content_top.map_or(to_floor, |t| (g.grid_bottom - t - clear).clamp(0.0, to_floor));
    let stack_top = movable.iter().map(|it| it.bounds.t).reduce(f32::max);
    let gap = stack_top.map_or(0.0, |top| (g.grid_bottom - top - clear).max(0.0));

    let band = (!movable.is_empty())
        .then(|| Rect::new(column.l, stack_bottom - tol, column.r, bottom + tol));
    // Annotations in the column below the paragraph whose vertical centre lies in the band move
    // with the content: a box drawn around the last moved line reaches below the band.
    let annots_moving: Vec<usize> = match band {
        None => Vec::new(),
        Some(band) => annots
            .iter()
            .filter(|a| {
                a.subtype != consts::FPDF_ANNOT_WIDGET
                    && a.subtype != consts::FPDF_ANNOT_POPUP
                    && valid(&a.rect)
                    && below(&a.rect)
                    && (a.rect.b + a.rect.t) / 2.0 >= band.b
                    && column.contains(&a.rect)
            })
            .map(|a| a.index)
            .collect(),
    };

    let mut movable: Vec<usize> = movable.iter().map(|it| it.index).collect();
    movable.sort_unstable();
    Plan {
        movable,
        annots: annots_moving,
        room: room.max(0.0),
        limit,
        free,
        gap,
        stack_top,
        nothing_below: !anything_below,
        footer,
        band,
    }
}

impl Plan {
    /// The same plan with nothing movable: what follows stays where it is (the content streams
    /// holding it cannot be rewritten), so it is in the way of the paragraph's growth.
    pub fn frozen(&self) -> Plan {
        Plan {
            movable: Vec::new(),
            annots: Vec::new(),
            room: 0.0,
            limit: FlowBlocked::Obstacle,
            gap: 0.0,
            stack_top: None,
            band: None,
            ..self.clone()
        }
    }
}

/// The shift for `flow`, given how much the paragraph's line grid grew (`growth` > 0) or
/// shrank, and how far the new text's ink reaches below the original grid bottom (`reach`,
/// ≥ `growth`: a descender deeper than the nominal descent reaches further).
pub fn decide(plan: &Plan, flow: ParagraphFlow, growth: f32, reach: f32) -> Decision {
    let none = Decision {
        shift: 0.0,
        blocked: None,
        overflow: 0.0,
        past_bottom: 0.0,
    };
    let reach = reach.max(growth);
    match flow {
        ParagraphFlow::Push if growth > MIN_SHIFT && plan.nothing_below => {
            // Only the floor (or a frame's bottom edge): the paragraph grows into the margin,
            // nothing is overlapped.
            let past = reach - plan.free;
            if past > MIN_SHIFT {
                Decision {
                    blocked: Some(plan.limit),
                    past_bottom: past,
                    ..none
                }
            } else {
                none
            }
        }
        ParagraphFlow::Push if growth > MIN_SHIFT && plan.movable.is_empty() => {
            // Something is in the way and nothing can move: the free space takes what it can.
            let over = reach - plan.free;
            if over > MIN_SHIFT {
                Decision {
                    blocked: Some(plan.limit),
                    overflow: over,
                    ..none
                }
            } else {
                none
            }
        }
        ParagraphFlow::Push if growth > MIN_SHIFT => {
            // The stack moves by the growth (the spacing below the paragraph stays as it was);
            // when the room runs out first, the paragraph spacing beyond a line's clearance
            // absorbs what it can. `need` is the least shift that keeps the new ink clear.
            let need = (reach - plan.gap).max(0.0);
            let shift = growth.max(need).min(plan.room);
            let remaining = need - shift;
            let blocked = remaining > MIN_SHIFT;
            Decision {
                shift: if shift >= MIN_SHIFT { shift } else { 0.0 },
                blocked: blocked.then_some(plan.limit),
                overflow: if blocked { remaining } else { 0.0 },
                past_bottom: 0.0,
            }
        }
        ParagraphFlow::Push if growth < -MIN_SHIFT && !plan.movable.is_empty() => {
            // A shorter paragraph pulls what follows up with it.
            Decision {
                shift: growth,
                ..none
            }
        }
        ParagraphFlow::Push => none,
        ParagraphFlow::Overlap | ParagraphFlow::Fit => {
            let over = (reach - plan.free).max(0.0);
            if plan.nothing_below {
                Decision {
                    past_bottom: over,
                    ..none
                }
            } else {
                Decision {
                    overflow: over,
                    ..none
                }
            }
        }
    }
}

/// Every annotation of the page, for [`plan`].
pub fn read_annots(bindings: &'static dyn PdfiumLibraryBindings, page: &PdfPage<'_>) -> Vec<AnnotItem> {
    (0..raw::annot::count(bindings, page))
        .filter_map(|index| {
            let a = raw::annot::get(bindings, page, index).ok()?;
            Some(AnnotItem {
                index,
                subtype: a.subtype(),
                rect: a.rect()?,
            })
        })
        .collect()
}

/// Moves the annotations at `indices` by `dy`. Returns how many moved.
///
/// Per kind:
/// * **every kind**: `/Rect` is translated. PDFium draws an existing appearance stream
///   through the `/Rect`↔`/BBox` matrix, so the appearance follows (`FPDFAnnot_SetRect` only
///   rewrites the `BBox` when the new rect *contains* it — never for a pure translation of a
///   rect the size of its appearance);
/// * **text markup** (highlight, underline, squiggly, strikeout): the `/QuadPoints` move too.
///   PDFium draws its own markup appearances at the quads' bounds and grows the `BBox` while
///   quads are half-moved, so the appearance is cleared first and regenerated on the next
///   render (the same policy as `update_annotation`; `OpenDoc::touched` makes the save
///   render it);
/// * **links**: their optional `/QuadPoints` move too;
/// * **ink** (SeePDF lines / arrows included): the `/InkList` strokes move too; the
///   appearance is kept and follows the rect.
///
/// Geometry keys PDFium cannot write (`/L` of a Line, `/Vertices` of a Polygon, `/CL` of a
/// FreeText callout) keep their old values; the appearance, which is what every viewer draws,
/// moves.
pub fn move_annots(
    bindings: &'static dyn PdfiumLibraryBindings,
    page: &PdfPage<'_>,
    indices: &[usize],
    dy: f32,
) -> Result<u32, EngineError> {
    let mut moved = 0u32;
    for &index in indices {
        let mut a = raw::annot::get(bindings, page, index)?;
        let Some(rect) = a.rect() else {
            continue;
        };
        match a.subtype() {
            consts::FPDF_ANNOT_HIGHLIGHT
            | consts::FPDF_ANNOT_UNDERLINE
            | consts::FPDF_ANNOT_SQUIGGLY
            | consts::FPDF_ANNOT_STRIKEOUT => {
                a.clear_ap();
                a.translate_quads(0.0, dy);
            }
            consts::FPDF_ANNOT_LINK => {
                a.translate_quads(0.0, dy);
            }
            consts::FPDF_ANNOT_INK => {
                let strokes = a.ink_paths();
                if !strokes.is_empty() {
                    a.remove_ink_list();
                    for stroke in strokes {
                        let moved_stroke: Vec<f32> = stroke
                            .chunks_exact(2)
                            .flat_map(|p| [p[0], p[1] + dy])
                            .collect();
                        if moved_stroke.len() >= 4 {
                            a.add_ink_stroke(&moved_stroke)?;
                        }
                    }
                }
            }
            _ => {}
        }
        if a.set_rect(Rect::new(rect.l, rect.b + dy, rect.r, rect.t + dy)) {
            moved += 1;
        }
    }
    Ok(moved)
}

#[cfg(test)]
mod tests {
    use super::*;

    const CROP: Rect = Rect {
        l: 0.0,
        b: 0.0,
        r: 612.0,
        t: 792.0,
    };

    fn item(index: usize, l: f32, b: f32, r: f32, t: f32) -> Item {
        Item {
            index,
            bounds: Rect::new(l, b, r, t),
            kind: ItemKind::Content,
            line: None,
        }
    }

    /// A text line of `size` on `baseline` (ink from 0.2 × size below to 0.7 × size above).
    fn text(index: usize, l: f32, r: f32, baseline: f32, size: f32) -> Item {
        Item {
            index,
            bounds: Rect::new(l, baseline - 0.2 * size, r, baseline + 0.7 * size),
            kind: ItemKind::Content,
            line: Some(TextLine { baseline, size }),
        }
    }

    /// 10 pt text on a 12 pt leading (a 2 pt clearance), the grid bottom at the ink bottom.
    fn geometry(original: Rect, new_rect: Rect) -> Geometry {
        Geometry {
            crop: CROP,
            original,
            new_rect,
            size: 10.0,
            leading: 12.0,
            grid_bottom: original.b,
        }
    }

    /// `decide` for new text whose ink stays on its grid.
    fn decide(plan: &Plan, flow: ParagraphFlow, growth: f32) -> Decision {
        super::decide(plan, flow, growth, growth)
    }

    fn decision(shift: f32, blocked: Option<FlowBlocked>, overflow: f32) -> Decision {
        Decision {
            shift,
            blocked,
            overflow,
            past_bottom: 0.0,
        }
    }

    /// Two columns: a right-column paragraph moves only right-column content.
    #[test]
    fn two_columns_stay_apart() {
        let para = Rect::new(320.0, 600.0, 540.0, 650.0);
        let items = vec![
            Item {
                kind: ItemKind::Paragraph,
                ..item(0, 320.0, 600.0, 540.0, 650.0)
            },
            item(1, 72.0, 500.0, 290.0, 590.0), // left column, below
            item(2, 320.0, 520.0, 540.0, 590.0), // right column, below
            item(3, 330.0, 400.0, 400.0, 500.0), // right column image, narrower
            item(4, 72.0, 700.0, 540.0, 720.0), // full-width title above
        ];
        let p = plan(&items, &[], &geometry(para, Rect::new(320.0, 576.0, 540.0, 650.0)));
        assert_eq!(p.movable, vec![2, 3]);
        assert_eq!(p.limit, FlowBlocked::PageBottom);
        // floor = min(lowest body bottom 400, 18) = 18
        assert!((p.room - (400.0 - 18.0)).abs() < 1e-3, "{p:?}");
        // 10 pt between the grid bottom and B, less a line's clearance (12 − 10).
        assert!((p.free - 8.0).abs() < 1e-3);
        assert!((p.gap - 8.0).abs() < 1e-3);
        assert!(!p.nothing_below);
        assert_eq!(p.band, Some(Rect::new(320.0, 397.5, 540.0, 602.5)));
        let d = decide(&p, ParagraphFlow::Push, 24.0);
        assert_eq!(d, decision(24.0, None, 0.0));
        let d = decide(&p, ParagraphFlow::Push, -12.0);
        assert_eq!(d.shift, -12.0);
        let d = decide(&p, ParagraphFlow::Overlap, 24.0);
        assert_eq!(d, decision(0.0, None, 16.0));
    }

    /// A full-width figure stops the push; content below it stays.
    #[test]
    fn full_width_obstacle_limits_the_room() {
        let para = Rect::new(72.0, 600.0, 300.0, 650.0);
        let items = vec![
            item(1, 72.0, 560.0, 290.0, 590.0), // in column
            item(2, 72.0, 400.0, 540.0, 540.0), // full-width figure
            item(3, 72.0, 300.0, 290.0, 380.0), // in column below the figure
        ];
        let p = plan(&items, &[], &geometry(para, para));
        assert_eq!(p.movable, vec![1]);
        assert_eq!(p.limit, FlowBlocked::Obstacle);
        assert!((p.room - 20.0).abs() < 1e-3);
        assert!((p.gap - 8.0).abs() < 1e-3);
        // room 20 + the 8 pt of paragraph spacing beyond a line's clearance: 28 fits, 40
        // overlaps by 12.
        assert_eq!(decide(&p, ParagraphFlow::Push, 28.0), decision(20.0, None, 0.0));
        let d = decide(&p, ParagraphFlow::Push, 40.0);
        assert_eq!(d.shift, 20.0);
        assert_eq!(d.blocked, Some(FlowBlocked::Obstacle));
        assert!((d.overflow - 12.0).abs() < 1e-3);
    }

    /// The paragraph spacing above what follows absorbs a growth the push cannot make: no
    /// `blocked`, no overflow, although the room is only 1 pt.
    #[test]
    fn the_gap_absorbs_what_the_push_cannot_make() {
        let para = Rect::new(72.0, 600.0, 300.0, 650.0);
        let items = vec![
            item(1, 72.0, 569.0, 290.0, 578.0), // B, 22 pt below the paragraph
            item(2, 60.0, 368.0, 540.0, 568.0), // figure 1 pt under B
        ];
        let p = plan(&items, &[], &geometry(para, para));
        assert!((p.room - 1.0).abs() < 1e-3 && (p.gap - 20.0).abs() < 1e-3, "{p:?}");
        assert_eq!(decide(&p, ParagraphFlow::Push, 14.4), decision(1.0, None, 0.0));
        let d = decide(&p, ParagraphFlow::Push, 30.0);
        assert_eq!(d.blocked, Some(FlowBlocked::Obstacle));
        assert!((d.overflow - 9.0).abs() < 1e-3, "{d:?}");
        // The overlap flow agrees: nothing moves, 20 pt are free.
        assert_eq!(decide(&p, ParagraphFlow::Overlap, 14.4), decision(0.0, None, 0.0));
        assert!((decide(&p, ParagraphFlow::Overlap, 30.0).overflow - 10.0).abs() < 1e-3);
    }

    /// A page number far below the body is the running footer: never moved, and in the way.
    #[test]
    fn running_footer_is_an_obstacle() {
        let para = Rect::new(72.0, 600.0, 540.0, 650.0);
        let items = vec![
            item(1, 72.0, 200.0, 540.0, 590.0),
            item(2, 300.0, 36.0, 312.0, 46.0), // page number
        ];
        let p = plan(&items, &[], &geometry(para, para));
        assert_eq!(p.footer, vec![2]);
        assert_eq!(p.movable, vec![1]);
        assert_eq!(p.limit, FlowBlocked::Obstacle);
        assert!((p.room - (200.0 - 46.0)).abs() < 1e-3);
    }

    /// A body column of 10 pt lines on a 12 pt pitch down to `last`, as text items.
    fn column_lines(first_index: usize, l: f32, r: f32, top: f32, last: f32) -> Vec<Item> {
        let mut out = Vec::new();
        let mut y = top;
        let mut k = first_index;
        while y >= last - 0.01 {
            out.push(text(k, l, r, y, 10.0));
            k += 1;
            y -= 12.0;
        }
        out
    }

    /// Two columns: the left column's copyright block is its footer even though the right
    /// column's lines run down past it (the gap scan is column-local).
    #[test]
    fn the_footer_is_found_per_column() {
        let para = Rect::new(54.0, 158.0, 293.0, 200.0);
        let mut items = vec![Item {
            kind: ItemKind::Paragraph,
            ..item(0, 54.0, 158.0, 293.0, 200.0)
        }];
        items.extend(column_lines(1, 54.0, 293.0, 700.0, 212.0)); // left column above
        items.extend(column_lines(100, 318.0, 558.0, 700.0, 60.0)); // right column to 60
        // copyright block: 118 … 90, 34 pt below the paragraph
        items.push(text(200, 54.0, 293.0, 118.0, 8.0));
        items.push(text(201, 54.0, 293.0, 108.0, 8.0));
        items.push(text(202, 54.0, 250.0, 98.0, 8.0));
        let p = plan(&items, &[], &geometry(para, para));
        assert_eq!(p.footer, vec![200, 201, 202], "{p:?}");
        assert!(p.movable.is_empty(), "{p:?}");
        assert_eq!(p.limit, FlowBlocked::Obstacle);
        assert!((p.room - (158.0 - (118.0 + 5.6))).abs() < 1e-3, "{p:?}");
    }

    /// The footer gap is measured against the page's line pitch, not the edited paragraph's
    /// leading: editing a 24 pt heading does not turn the page number into body text.
    #[test]
    fn the_footer_gap_uses_the_page_pitch() {
        let heading = Rect::new(72.0, 700.0, 400.0, 740.0);
        let mut items = vec![Item {
            kind: ItemKind::Paragraph,
            line: Some(TextLine { baseline: 720.0, size: 24.0 }),
            ..item(0, 72.0, 700.0, 400.0, 740.0)
        }];
        items.extend(column_lines(1, 72.0, 540.0, 680.0, 92.0));
        // page number: 30 pt below the last body line's ink
        items.push(text(99, 300.0, 310.0, 62.0 - 7.0, 10.0));
        let g = Geometry {
            leading: 28.8,
            size: 24.0,
            ..geometry(heading, heading)
        };
        let p = plan(&items, &[], &g);
        assert_eq!(p.footer, vec![99], "{p:?}");
        assert!(!p.movable.contains(&99));
    }

    /// A short page number at LaTeX's \footskip (a clear gap under 2 × the pitch) is the
    /// footer too.
    #[test]
    fn a_short_page_number_needs_a_smaller_gap() {
        let para = Rect::new(72.0, 600.0, 540.0, 650.0);
        let mut items = column_lines(1, 72.0, 540.0, 586.0, 130.0);
        // last body ink bottom 128; page number baseline 30 below the last baseline: ink top 107
        items.push(text(99, 303.0, 308.0, 100.0, 10.0));
        let p = plan(&items, &[], &geometry(para, para));
        assert_eq!(p.footer, vec![99], "{p:?}");
        // A wide line with the same gap is body text.
        let mut items = column_lines(1, 72.0, 540.0, 586.0, 130.0);
        items.push(text(99, 72.0, 540.0, 100.0, 10.0));
        let p = plan(&items, &[], &geometry(para, para));
        assert!(p.footer.is_empty(), "{p:?}");
        assert!(p.movable.contains(&99));
    }

    /// A page number in the bottom margin needs less of a gap: under a page filled to the
    /// bottom inch it may sit less than two lines below the body (a heading's edit, which
    /// widens the column to the body, leaves it alone too).
    #[test]
    fn a_page_number_in_the_bottom_margin_is_the_footer() {
        let heading = Rect::new(72.0, 716.0, 190.0, 731.5);
        let mut items = vec![Item {
            kind: ItemKind::Paragraph,
            ..text(0, 72.0, 190.0, 720.0, 16.0)
        }];
        items.extend(column_lines(1, 72.0, 420.0, 690.0, 78.0));
        items.push(text(99, 300.0, 305.0, 60.0, 10.0)); // ink top 67: 9 pt under the body
        let g = Geometry {
            size: 16.0,
            leading: 19.2,
            ..geometry(heading, heading)
        };
        let p = plan(&items, &[], &g);
        assert_eq!(p.footer, vec![99], "{p:?}");
        assert!(p.movable.contains(&1) && !p.movable.contains(&99), "{p:?}");
        // The same line high on the page, with that gap, is body text.
        let mut items = column_lines(1, 72.0, 420.0, 690.0, 400.0);
        items.push(text(99, 300.0, 305.0, 382.0, 10.0));
        let p = plan(&items, &[], &geometry(Rect::new(72.0, 700.0, 420.0, 720.0), Rect::new(72.0, 700.0, 420.0, 720.0)));
        assert!(p.footer.is_empty() && p.movable.contains(&99), "{p:?}");
    }

    /// Header / footer stamps are obstacles; watermarks are not.
    #[test]
    fn stamps_widgets_and_annotations() {
        let para = Rect::new(72.0, 600.0, 540.0, 650.0);
        let items = vec![
            item(1, 72.0, 500.0, 540.0, 590.0),
            Item {
                kind: ItemKind::Stamp,
                ..item(2, 0.0, 300.0, 612.0, 500.0)
            },
        ];
        let annots = vec![
            AnnotItem {
                index: 0,
                subtype: consts::FPDF_ANNOT_HIGHLIGHT,
                rect: Rect::new(72.0, 570.0, 200.0, 585.0),
            },
            AnnotItem {
                index: 1,
                subtype: consts::FPDF_ANNOT_WIDGET,
                rect: Rect::new(72.0, 200.0, 300.0, 230.0),
            },
        ];
        let p = plan(&items, &annots, &geometry(para, para));
        assert_eq!(p.movable, vec![1]);
        assert_eq!(p.annots, vec![0]);
        assert_eq!(p.limit, FlowBlocked::Obstacle);
        assert!((p.room - (500.0 - 230.0)).abs() < 1e-3);

        let footer_stamp = vec![
            item(1, 72.0, 60.0, 540.0, 590.0),
            Item {
                kind: ItemKind::Fixed,
                ..item(2, 273.0, 36.0, 338.0, 47.0)
            },
        ];
        let p = plan(&footer_stamp, &[], &geometry(para, para));
        assert_eq!(p.movable, vec![1]);
        assert_eq!(p.limit, FlowBlocked::Obstacle);
        assert!((p.room - 13.0).abs() < 1e-3, "{p:?}");
    }

    /// Content inside a frame around the paragraph stops at the frame's bottom edge; what is
    /// below the frame stays.
    #[test]
    fn a_container_keeps_its_content_inside() {
        let para = Rect::new(80.0, 660.0, 460.0, 710.0);
        let items = vec![
            item(1, 68.5, 619.3, 541.5, 717.5), // the stroked box, one path
            item(2, 80.0, 630.0, 460.0, 650.0), // B, inside the box
            item(3, 72.0, 520.0, 460.0, 590.0), // C, below the box
        ];
        let p = plan(&items, &[], &geometry(para, para));
        assert_eq!(p.movable, vec![2], "{p:?}");
        assert_eq!(p.limit, FlowBlocked::Obstacle);
        assert!((p.room - (630.0 - 622.3)).abs() < 1e-3, "{p:?}");
        let d = decide(&p, ParagraphFlow::Push, 14.4 + 10.0 + 7.7);
        assert_eq!(d.blocked, Some(FlowBlocked::Obstacle));
    }

    /// A line wrapped beside a figure that sticks out of the column straddles the figure's top:
    /// it cannot move, so the push stops at its top.
    #[test]
    fn a_line_beside_a_figure_is_in_the_way() {
        let para = Rect::new(72.0, 656.6, 290.0, 710.0);
        let line = |index, l, b, r, t, baseline| Item {
            line: Some(TextLine { baseline, size: 12.0 }),
            ..item(index, l, b, r, t)
        };
        let items = vec![
            line(1, 72.0, 647.5, 230.0, 658.6, 650.0), // "Following", in the column
            item(2, 200.0, 560.0, 420.0, 630.0),       // figure, 41 % in the column: an obstacle
            line(3, 72.0, 625.5, 150.0, 636.7, 628.0), // "Beside the fig": straddles its top
        ];
        let p = plan(&items, &[], &geometry(para, para));
        assert_eq!(p.movable, vec![1], "{p:?}");
        assert_eq!(p.limit, FlowBlocked::Obstacle);
        assert!((p.room - (647.5 - 636.7)).abs() < 1e-3, "{p:?}");
        let d = decide(&p, ParagraphFlow::Push, 14.4);
        assert_eq!(d.blocked, Some(FlowBlocked::Obstacle), "{d:?}");
    }

    /// Nothing below: the paragraph itself grows into the page bottom margin; past it, nothing
    /// is overlapped — the text runs past the floor.
    #[test]
    fn nothing_below_grows_to_the_floor() {
        let para = Rect::new(72.0, 60.0, 540.0, 100.0);
        let items = vec![Item {
            kind: ItemKind::Paragraph,
            ..item(0, 72.0, 60.0, 540.0, 100.0)
        }];
        let p = plan(&items, &[], &geometry(para, para));
        assert!(p.movable.is_empty() && p.nothing_below && p.band.is_none());
        assert!((p.room - 42.0).abs() < 1e-3, "{p:?}");
        let d = decide(&p, ParagraphFlow::Push, 50.0);
        assert_eq!(d.shift, 0.0);
        assert_eq!(d.blocked, Some(FlowBlocked::PageBottom));
        assert_eq!(d.overflow, 0.0);
        assert!((d.past_bottom - 8.0).abs() < 1e-3);
        assert_eq!(decide(&p, ParagraphFlow::Push, 30.0), decision(0.0, None, 0.0));
        let d = decide(&p, ParagraphFlow::Overlap, 50.0);
        assert_eq!((d.overflow, d.blocked), (0.0, None));
        assert!((d.past_bottom - 8.0).abs() < 1e-3);
        // Content in the other column is not "below".
        let items = vec![item(1, 320.0, 30.0, 560.0, 50.0)];
        let left = Rect::new(72.0, 60.0, 290.0, 100.0);
        let p = plan(&items, &[], &geometry(left, left));
        assert!(p.nothing_below, "{p:?}");
    }

    /// The pitch is the median baseline step; superscripts fold into their line.
    #[test]
    fn line_pitch_is_the_median_step() {
        let column = Column { l: 72.0, r: 540.0 };
        let mut items = column_lines(0, 72.0, 540.0, 700.0, 600.0);
        items.push(text(50, 200.0, 210.0, 703.5, 7.0)); // a superscript
        assert!((line_pitch(&items, &CROP, &column, 99.0) - 12.0).abs() < 1e-3);
        assert_eq!(line_pitch(&items[..2], &CROP, &column, 99.0), 99.0, "too few lines");
    }

    /// The gap is measured from the paragraph's line grid, like the growth, and keeps a line's
    /// clearance: a new last line whose ink reaches below the grid (`reach` > `growth`) needs
    /// that much more (the verifier's descenders, which the ink-to-ink gap let overlap).
    #[test]
    fn the_gap_is_measured_from_the_grid_and_the_new_ink() {
        // The old last line had no descenders: its ink bottom (600) is 2.5 pt above the grid.
        let para = Rect::new(72.0, 600.0, 300.0, 650.0);
        let items = vec![
            item(1, 72.0, 569.0, 290.0, 578.0),  // B, 22 pt below the ink, 19.5 below the grid
            item(2, 60.0, 368.0, 540.0, 568.0),  // figure 1 pt under B
        ];
        let g = Geometry {
            grid_bottom: 597.5,
            ..geometry(para, para)
        };
        let p = plan(&items, &[], &g);
        assert!((p.gap - 17.5).abs() < 1e-3, "{p:?}"); // 597.5 − 578 − 2
        // 18.5 of growth fits (room 1 + gap 17.5)…
        assert_eq!(super::decide(&p, ParagraphFlow::Push, 18.5, 18.5), decision(1.0, None, 0.0));
        // …unless the new last line's descenders reach 1.5 pt further than the grid.
        let d = super::decide(&p, ParagraphFlow::Push, 18.5, 20.0);
        assert_eq!(d.blocked, Some(FlowBlocked::Obstacle), "{d:?}");
        assert!((d.overflow - 1.5).abs() < 1e-3, "{d:?}");
        // With room to spare, a deep descender pushes a little further instead.
        let roomy = vec![item(1, 72.0, 569.0, 290.0, 578.0)];
        let p = plan(&roomy, &[], &g);
        let d = super::decide(&p, ParagraphFlow::Push, 0.5, 20.0);
        assert!((d.shift - 2.5).abs() < 1e-3 && d.blocked.is_none(), "{d:?}");
    }

    /// Body text wider than a short or ragged paragraph (or around a block quote) follows it:
    /// the column widens to the text block instead of making every body line an obstacle.
    #[test]
    fn body_text_wider_than_the_paragraph_follows_it() {
        for (label, para) in [
            ("one short line", Rect::new(72.3, 697.4, 253.7, 708.6)),
            ("block quote", Rect::new(108.0, 683.0, 400.0, 708.6)),
        ] {
            let mut items = vec![Item {
                kind: ItemKind::Paragraph,
                ..text(0, para.l, para.r, 700.0, 10.0)
            }];
            items.extend(column_lines(1, 72.9, 452.5, 671.2, 600.0));
            items.push(text(20, 72.9, 130.0, 588.0, 10.0)); // a short last line
            let p = plan(&items, &[], &geometry(para, para));
            let want: Vec<usize> = (1..=6).chain([20]).collect();
            assert_eq!(p.movable, want, "{label}: {p:?}");
            assert!(p.band.is_some_and(|b| b.l <= 72.9 && b.r >= 452.5), "{label}: {p:?}");
            assert_eq!(decide(&p, ParagraphFlow::Push, 24.0).blocked, None, "{label}");
        }
    }

    /// A line that spans into another column's content is not widened into: on a two-column
    /// page a caption across both columns stays an obstacle for a short left-column paragraph.
    #[test]
    fn a_line_spanning_two_columns_is_not_the_text_block() {
        let para = Rect::new(54.0, 600.0, 200.0, 610.0);
        let mut items = vec![Item {
            kind: ItemKind::Paragraph,
            ..text(0, 54.0, 200.0, 602.0, 10.0)
        }];
        items.extend(column_lines(1, 54.0, 293.0, 590.0, 530.0)); // left column below
        items.extend(column_lines(20, 318.0, 558.0, 700.0, 530.0)); // right column
        items.push(text(40, 54.0, 558.0, 510.0, 10.0)); // caption across both columns
        let p = plan(&items, &[], &geometry(para, para));
        assert_eq!(p.movable, (1..=6).collect::<Vec<_>>(), "{p:?}");
        assert!(!p.movable.contains(&40) && p.movable.iter().all(|&i| i < 20), "{p:?}");
        assert_eq!(p.limit, FlowBlocked::Obstacle);
        assert!((p.room - (530.0 - 2.0 - 517.0)).abs() < 1e-3, "{p:?}");
    }

    /// A list: each following item's marker (in the hanging indent, left of the column) moves
    /// with its text, and the paragraph after the list moves whole — its short last line is
    /// not an obstacle inside it.
    #[test]
    fn list_markers_and_last_lines_follow_their_text() {
        let para = Rect::new(108.0, 663.0, 458.0, 688.0);
        let mut items = vec![Item {
            kind: ItemKind::Paragraph,
            ..text(0, 108.0, 458.0, 680.0, 11.0)
        }];
        let marker = |index, baseline: f32| text(index, 90.6, 93.3, baseline, 11.0);
        items.push(marker(1, 645.0));
        items.push(text(2, 108.0, 455.0, 645.0, 11.0));
        items.push(text(3, 108.0, 300.0, 631.6, 11.0));
        items.push(marker(4, 610.0));
        items.push(text(5, 108.0, 240.0, 610.0, 11.0));
        items.push(text(6, 72.0, 400.5, 580.0, 11.0)); // closing paragraph
        items.push(text(7, 72.4, 138.9, 566.6, 11.0)); // …its short last line
        let g = Geometry {
            size: 11.0,
            leading: 13.4,
            ..geometry(para, para)
        };
        let p = plan(&items, &[], &g);
        assert_eq!(p.movable, vec![1, 2, 3, 4, 5, 6, 7], "{p:?}");
        assert_eq!(p.limit, FlowBlocked::PageBottom, "{p:?}");
        // A marker-sized object that belongs to another column's text is not a marker.
        let mut two = items.clone();
        two.push(text(8, 20.0, 88.0, 600.0, 11.0)); // a sidebar line left of the list
        two.push(text(9, 80.0, 86.0, 590.0, 11.0)); // …and a word of it near the gutter
        let p = plan(&two, &[], &g);
        assert!(!p.movable.contains(&9), "{p:?}");
    }

    /// A block at the page bottom separated by a gap is a running footer only when it looks
    /// like one: a letter's signature block (left-aligned, body size, above the bottom inch)
    /// follows the text; the same block in smaller type is a footer.
    #[test]
    fn a_signature_block_is_not_a_footer() {
        let para = Rect::new(72.0, 684.0, 470.0, 708.6);
        let mut items = vec![Item {
            kind: ItemKind::Paragraph,
            ..text(0, 72.0, 470.0, 700.0, 12.0)
        }];
        let mut k = 1;
        let mut y = 656.8;
        while y > 380.0 {
            items.push(text(k, 72.0, 470.0, y, 12.0));
            k += 1;
            y -= 14.4;
        }
        items.push(text(50, 72.0, 130.0, 240.0, 12.0)); // Sincerely,
        let signature = [(51, 148.0, 120.0), (52, 133.6, 197.0), (53, 119.2, 170.0)];
        let mut letter = items.clone();
        for (index, baseline, r) in signature {
            letter.push(text(index, 72.0, r, baseline, 12.0));
        }
        let g = Geometry {
            size: 12.0,
            leading: 14.4,
            ..geometry(para, para)
        };
        let p = plan(&letter, &[], &g);
        assert!(p.footer.is_empty(), "{p:?}");
        assert!(p.movable.contains(&51) && p.movable.contains(&53), "{p:?}");
        assert_eq!(p.limit, FlowBlocked::PageBottom);
        // Set in 8 pt it is a footer (a copyright block), never moved.
        for (index, baseline, r) in signature {
            items.push(text(index, 72.0, r, baseline, 8.0));
        }
        let p = plan(&items, &[], &g);
        assert_eq!(p.footer, vec![51, 52, 53], "{p:?}");
    }

    /// A page background (a rect as large as the page) is not a container: the last paragraph
    /// with nothing below runs past the floor, it does not "overlap" the background.
    #[test]
    fn a_page_background_is_not_a_container() {
        let para = Rect::new(72.0, 60.0, 540.0, 100.0);
        let items = vec![
            item(1, 0.0, 0.0, 612.0, 792.0), // background
            Item {
                kind: ItemKind::Paragraph,
                ..item(0, 72.0, 60.0, 540.0, 100.0)
            },
        ];
        let p = plan(&items, &[], &geometry(para, para));
        assert!(p.nothing_below, "{p:?}");
        let d = decide(&p, ParagraphFlow::Push, 50.0);
        assert_eq!((d.overflow, d.blocked), (0.0, Some(FlowBlocked::PageBottom)), "{d:?}");
        assert!((d.past_bottom - 8.0).abs() < 1e-3, "{d:?}");
        // A frame around the paragraph still keeps it inside, and nothing is overlapped.
        let items = vec![item(1, 60.0, 40.0, 552.0, 110.0)];
        let p = plan(&items, &[], &geometry(para, para));
        assert!(p.nothing_below && p.limit == FlowBlocked::Obstacle, "{p:?}");
        let d = decide(&p, ParagraphFlow::Push, 50.0);
        assert_eq!((d.overflow, d.blocked), (0.0, Some(FlowBlocked::Obstacle)), "{d:?}");
    }

    /// An annotation whose centre lies in the moved band moves, although it reaches below the
    /// stack's ink (a box drawn around the last moved line).
    #[test]
    fn a_box_around_the_last_line_moves() {
        let para = Rect::new(72.0, 600.0, 540.0, 650.0);
        let items = vec![text(1, 72.0, 300.0, 540.0, 10.0)];
        let annots = vec![
            AnnotItem {
                index: 0,
                subtype: consts::FPDF_ANNOT_SQUARE,
                rect: Rect::new(70.0, 530.0, 305.0, 550.0),
            },
            AnnotItem {
                index: 1,
                subtype: consts::FPDF_ANNOT_SQUARE,
                rect: Rect::new(70.0, 480.0, 305.0, 530.0), // below the text: stays
            },
        ];
        let p = plan(&items, &annots, &geometry(para, para));
        assert_eq!(p.annots, vec![0], "{p:?}");
    }

    /// Content in a stream PDFium cannot rewrite stays: the frozen plan moves nothing and the
    /// growth is blocked by it, as far as the free space goes.
    #[test]
    fn a_frozen_plan_moves_nothing() {
        let para = Rect::new(72.0, 600.0, 300.0, 650.0);
        let items = vec![item(1, 72.0, 560.0, 290.0, 590.0)];
        let p = plan(&items, &[], &geometry(para, para)).frozen();
        assert!(p.movable.is_empty() && p.band.is_none() && !p.nothing_below, "{p:?}");
        let d = decide(&p, ParagraphFlow::Push, 20.0);
        assert_eq!((d.shift, d.blocked), (0.0, Some(FlowBlocked::Obstacle)), "{d:?}");
        assert!((d.overflow - 12.0).abs() < 1e-3, "{d:?}"); // 20 − (600 − 590 − 2)
        assert_eq!(decide(&p, ParagraphFlow::Push, -10.0).shift, 0.0);
    }
}
