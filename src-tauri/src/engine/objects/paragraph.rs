//! Paragraph detection and the reflowing paragraph edit — Stage 7 (`IPC_CONTRACT.md` §7.4b).
//!
//! PDF has no paragraphs: a page is a bag of positioned text runs. [`probe`] rebuilds one
//! around a point — runs → lines (same baseline, small horizontal gaps) → the run of lines
//! that share size, leading and x-range — and [`edit`] replaces all of its objects with a
//! freshly laid-out block, inside one `registry::mutate` (one undo step `undo.paragraphEdit`).
//!
//! ## Measuring and the coverage check
//!
//! Both use one **trial object**: a fresh text object in the candidate font holding every
//! distinct non-space character of the text (plus `" X"` when the text has spaces), appended
//! to the scratch page, read back through a fresh `page.text()` and removed again. It answers
//! both questions at once:
//!
//! * *can the font draw this?* — the read-back must equal what we asked for and every char
//!   must have a positive loose (advance) width, exactly the rule of `probe_text_edit`
//!   (see the module docs of `engine::objects`);
//! * *how wide is each character?* — the loose box width is the font's advance width, which
//!   is what PDFium uses to place the next glyph of the objects we create (no kerning).
//!
//! Unlike `probe_text_edit`'s trial, the paragraph's own objects are never touched, and the
//! probe runs on a [`ScratchPage`] that is closed without regenerating the content, so the
//! document is left exactly as it was.
//!
//! A font that draws every character but has no usable space glyph (common for LaTeX subset
//! fonts, which position words instead of drawing spaces) is still `inPlace`: the new text is
//! then written one object per word, with a synthetic space. Justified lines are always one
//! object per word; everything else is one object per line.

use super::{
    check_font_size, check_generation, flow, not_editable, object_at, relist, rgb_of, ungroup,
};
use crate::engine::annot::ScratchPage;
use crate::engine::registry::{self, MutateOpts, OpenDoc};
use crate::engine::types::EngineState;
use crate::engine::{fonts, raw, stamp};
use crate::ipc::error::PdfiumResultExt;
use crate::ipc::types::{
    ChangeReason, DocGeneration, FlowBlocked, NotEditableReason, ObjectId, PageIndex,
    ParagraphAlign, ParagraphEdit, ParagraphEditResult, ParagraphFlow, ParagraphProbe, Point, Rect,
    Rgb, StampRole, TextEditStrategy,
};
use crate::ipc::{EngineError, ErrorCode};
use pdfium_render::prelude::*;
use std::collections::{HashMap, HashSet};

/// Baselines within this fraction of the font size are one line.
const BASELINE_TOL: f32 = 0.25;
/// A horizontal gap wider than this (× size) splits a baseline band into separate lines
/// (columns, table cells). Wider than the contract's 1.5 space widths so that the stretched
/// word gaps of a loose justified line do not split it.
const SPLIT_GAP: f32 = 1.0;
/// A gap wider than this (× size) between two runs of a line is a word space.
const WORD_GAP: f32 = 0.15;
/// Font sizes within ±8 % are the same paragraph.
const SIZE_TOL: f32 = 0.08;
/// Leading within ±20 % of the first leading.
const LEADING_TOL: f32 = 0.2;
/// Leading larger than this (× size) is a blank line.
const MAX_LEADING: f32 = 2.2;
/// A left-flush line ending before this fraction of the block width ends the paragraph.
const SHORT_LINE: f32 = 0.75;
/// A line starting this far (× size) right of the block is the first line of a new paragraph…
const INDENT: f32 = 0.5;
/// …unless it is flush right and inset further than this (right-aligned text).
const INDENT_MAX: f32 = 3.0;
/// A laid-out line may exceed the box by this much (× size): the probe rect is the union of
/// the *ink* bounds, so the original longest line measured by advance widths is a little
/// wider than the rect and would otherwise wrap its last word on an unchanged edit.
const FIT_SLACK: f32 = 0.15;
/// The space of a font without a usable space glyph, × size.
const SYNTHETIC_SPACE: f32 = 0.3;
/// A justified line never stretches its word gaps past this (× size): the probe splits a
/// baseline band at gaps wider than [`SPLIT_GAP`] × size, and a line SeePDF wrote must read
/// back as one line (the side bearings add a little to the advance gap). A line that would
/// need more stays short of the right edge, like TeX's underfull boxes.
const JUSTIFY_MAX_GAP: f32 = 0.75;
/// A paragraph is justified when at least this share of its lines (all but the last) end at
/// the right edge — so a line that could not be stretched (see [`JUSTIFY_MAX_GAP`]) does not
/// turn it into a left-aligned one on the next edit.
const JUSTIFY_FLUSH_SHARE: f32 = 0.75;
/// A line whose word gaps are all at least this wide (× size, ink to ink) was stretched by
/// justification but stopped short of the right edge at [`JUSTIFY_MAX_GAP`] (long words): it
/// counts as flush. A normal word space is about 0.25–0.35 × size.
const STRETCHED_GAP: f32 = 0.55;
/// Trial objects are created at this size so the loose widths keep their precision.
const TRIAL_SIZE: f32 = 100.0;
/// …and placed far off the page. PDFium's text page drops a character that repeats the same
/// char code in the same font within `0.07 × size` of one of the previous seven characters
/// (its fake-bold filter), so a trial object laid over the paragraph it is measuring loses
/// glyphs at random. Off-page objects are still part of the text page.
const TRIAL_AT: (f32, f32) = (-20000.0, -20000.0);

// ---------------------------------------------------------------------------------------
// Runs and lines
// ---------------------------------------------------------------------------------------

/// One upright, visible text object.
#[derive(Debug, Clone)]
struct Run {
    index: usize,
    text: String,
    /// Origin of the first glyph (matrix `e`, `f`): `y` is the baseline.
    x: f32,
    y: f32,
    /// Rendered size: font size × the matrix' vertical scale.
    size: f32,
    bounds: Rect,
    font: String,
    color: Rgb,
    render_mode: PdfPageTextRenderMode,
    /// Non-whitespace characters — the weight of this run's style.
    weight: usize,
    reason: Option<NotEditableReason>,
}

/// A text object the paragraph detector does not handle, kept so a click on it can say why.
#[derive(Debug, Clone)]
struct Obstacle {
    index: usize,
    bounds: Rect,
    reason: NotEditableReason,
    text: String,
    font: String,
    size: f32,
    color: Rgb,
}

#[derive(Debug, Clone)]
struct Line {
    /// Indices into the run list, left to right.
    runs: Vec<usize>,
    baseline: f32,
    size: f32,
    /// Origin x of the leftmost run (glyph origin, not ink).
    left: f32,
    /// Right edge of the ink.
    right: f32,
    bounds: Rect,
}

fn bounds_of(object: &PdfPageObject<'_>) -> Rect {
    object
        .bounds()
        .map(|q| {
            let r = q.to_rect();
            Rect::new(
                r.left().value,
                r.bottom().value,
                r.right().value,
                r.top().value,
            )
        })
        .unwrap_or(Rect::ZERO)
}

enum Classified {
    Run(Run),
    Obstacle(Obstacle),
    Skip,
}

/// v0.3 (R5): the frame a paragraph is detected, laid out and rewritten in. Upright text uses
/// the page itself; a paragraph whose runs are all rotated by θ (a 90° caption up the margin,
/// a landscape table on a portrait page) is handled in a frame rotated by θ, where it is
/// upright: runs, lines, the layout and the probe rect are frame coordinates there, and the
/// new objects are rotated back onto the page. The flow does not move anything for a rotated
/// paragraph (the content "below" it is not below it on the page).
#[derive(Debug, Clone, Copy, PartialEq)]
struct Rot {
    cos: f32,
    sin: f32,
}

impl Rot {
    const UPRIGHT: Rot = Rot { cos: 1.0, sin: 0.0 };

    /// The direction of a text object's baseline; [`Rot::UPRIGHT`] within 0.02 rad.
    fn of_matrix(m: [f32; 6]) -> Rot {
        let len = (m[0] * m[0] + m[1] * m[1]).sqrt();
        if len < 1e-6 {
            return Rot::UPRIGHT;
        }
        let (cos, sin) = (m[0] / len, m[1] / len);
        if cos > 0.0 && sin.abs() <= 0.02 {
            Rot::UPRIGHT
        } else {
            Rot { cos, sin }
        }
    }

    fn upright(&self) -> bool {
        *self == Rot::UPRIGHT
    }

    /// Page → frame.
    fn to_frame(self, (x, y): (f32, f32)) -> (f32, f32) {
        (x * self.cos + y * self.sin, -x * self.sin + y * self.cos)
    }

    /// Frame → page.
    fn to_page(self, (x, y): (f32, f32)) -> (f32, f32) {
        (x * self.cos - y * self.sin, x * self.sin + y * self.cos)
    }

    /// An object matrix as seen in the frame.
    fn matrix_to_frame(&self, m: [f32; 6]) -> [f32; 6] {
        let (c, s) = (self.cos, self.sin);
        let (e, f) = self.to_frame((m[4], m[5]));
        [
            m[0] * c + m[1] * s,
            -m[0] * s + m[1] * c,
            m[2] * c + m[3] * s,
            -m[2] * s + m[3] * c,
            e,
            f,
        ]
    }

    /// The frame-aligned box of four page points.
    fn points_to_frame(&self, points: [(f32, f32); 4]) -> Rect {
        let (mut l, mut b, mut r, mut t) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for p in points {
            let (x, y) = self.to_frame(p);
            l = l.min(x);
            b = b.min(y);
            r = r.max(x);
            t = t.max(y);
        }
        Rect::new(l, b, r, t)
    }

    /// A frame rect as the page-aligned box around it.
    fn rect_to_page(&self, r: Rect) -> Rect {
        if self.upright() {
            return r;
        }
        let back = Rot {
            cos: self.cos,
            sin: -self.sin,
        };
        back.points_to_frame([(r.l, r.b), (r.r, r.b), (r.r, r.t), (r.l, r.t)])
    }
}

/// An object's bounds in the frame `rot` (its tight rotated quad for text and images).
fn bounds_in(object: &PdfPageObject<'_>, rot: Rot) -> Rect {
    if rot.upright() {
        return bounds_of(object);
    }
    object
        .bounds()
        .map(|q| {
            rot.points_to_frame([
                (q.x1().value, q.y1().value),
                (q.x2().value, q.y2().value),
                (q.x3().value, q.y3().value),
                (q.x4().value, q.y4().value),
            ])
        })
        .unwrap_or(Rect::ZERO)
}

fn matrix_of(object: &PdfPageObject<'_>) -> Option<[f32; 6]> {
    object
        .matrix()
        .ok()
        .map(|m| [m.a(), m.b(), m.c(), m.d(), m.e(), m.f()])
}

/// What kind of text object this is for the paragraph editor, seen in the frame `rot`.
fn classify(
    index: usize,
    object: &PdfPageObject<'_>,
    text_page: &PdfPageText<'_>,
    rot: Rot,
) -> Classified {
    let bounds = bounds_in(object, rot);
    // v0.3.1: text inside a group is found by the text under the point, not by the group's
    // bounds (`grouped_probe`).
    let Some(t) = object.as_text_object() else {
        return Classified::Skip;
    };
    let text = text_page.for_object(t);
    if text.trim().is_empty() {
        return Classified::Skip;
    }
    let (a, b, d, e, f) = matrix_of(object)
        .map(|m| rot.matrix_to_frame(m))
        .map(|m| (m[0], m[1], m[3], m[4], m[5]))
        .unwrap_or((1.0, 0.0, 1.0, bounds.l, bounds.b));
    let size = t.unscaled_font_size().value * d.abs();
    let font = t.font().name();
    let color = object.fill_color().ok().map(rgb_of).unwrap_or([0, 0, 0]);
    let (_, reason) = super::text_editability(t, &text);
    let upright = a > 0.0 && d > 0.0 && b.abs() <= 0.02 * a;
    let obstacle = match reason {
        Some(NotEditableReason::Invisible) => Some(NotEditableReason::Invisible),
        _ if !upright => Some(NotEditableReason::RotatedText),
        _ => None,
    };
    if let Some(reason) = obstacle {
        return Classified::Obstacle(Obstacle {
            index,
            bounds,
            reason,
            text,
            font,
            size,
            color,
        });
    }
    if size.is_nan() || size <= 0.0 {
        return Classified::Skip;
    }
    let weight = text.chars().filter(|c| !c.is_whitespace()).count();
    Classified::Run(Run {
        index,
        text,
        x: e,
        y: f,
        size,
        bounds,
        font,
        color,
        render_mode: t.render_mode(),
        weight,
        reason,
    })
}

/// Groups runs into lines: baseline bands first (small-print bands — superscripts,
/// subscripts — folded into the line they sit on), then each band split at wide gaps.
fn build_lines(runs: &[Run], split: bool) -> Vec<Line> {
    let mut order: Vec<usize> = (0..runs.len()).collect();
    order.sort_by(|&a, &b| runs[b].y.total_cmp(&runs[a].y));

    // Baseline bands, top to bottom.
    let mut bands: Vec<Vec<usize>> = Vec::new();
    let mut anchors: Vec<(f32, f32)> = Vec::new();
    for i in order {
        let r = &runs[i];
        if let (Some(band), Some(&(ay, asz))) = (bands.last_mut(), anchors.last()) {
            if (r.y - ay).abs() <= BASELINE_TOL * asz.max(r.size) {
                band.push(i);
                continue;
            }
        }
        bands.push(vec![i]);
        anchors.push((r.y, r.size));
    }

    // Fold small-print bands into the neighbouring band they belong to.
    let band_size = |band: &[usize]| band.iter().map(|&i| runs[i].size).fold(0.0f32, f32::max);
    let band_x = |band: &[usize]| {
        band.iter().fold((f32::MAX, f32::MIN), |(l, r), &i| {
            (l.min(runs[i].bounds.l), r.max(runs[i].bounds.r))
        })
    };
    let mut k = 0;
    while k < bands.len() {
        let small = band_size(&bands[k]);
        let (sl, sr) = band_x(&bands[k]);
        let mut target = None;
        for n in [k.wrapping_sub(1), k + 1] {
            let Some(other) = bands.get(n) else { continue };
            let big = band_size(other);
            let (ol, or) = band_x(other);
            let dy = (anchors[k].0 - anchors[n].0).abs();
            if small < 0.85 * big && dy <= 0.6 * big && sl >= ol - big && sr <= or + big {
                target = Some(n);
                break;
            }
        }
        if let Some(n) = target {
            let moved = bands.remove(k);
            anchors.remove(k);
            let n = if n > k { n - 1 } else { n };
            bands[n].extend(moved);
            // Re-examine the band that now sits at `k`.
            continue;
        }
        k += 1;
    }

    let mut lines = Vec::new();
    for band in bands {
        let mut members = band;
        members.sort_by(|&a, &b| runs[a].x.total_cmp(&runs[b].x));
        let size = members.iter().map(|&i| runs[i].size).fold(0.0f32, f32::max);
        let mut segment: Vec<usize> = Vec::new();
        let mut right = f32::MIN;
        for i in members {
            if split && !segment.is_empty() && runs[i].bounds.l - right > SPLIT_GAP * size {
                lines.push(make_line(runs, std::mem::take(&mut segment)));
                right = f32::MIN;
            }
            right = right.max(runs[i].bounds.r);
            segment.push(i);
        }
        if !segment.is_empty() {
            lines.push(make_line(runs, segment));
        }
    }
    lines
}

fn make_line(runs: &[Run], members: Vec<usize>) -> Line {
    // The heaviest run carries the line's baseline and size (not a superscript).
    let main = *members
        .iter()
        .max_by(|&&a, &&b| {
            runs[a]
                .weight
                .cmp(&runs[b].weight)
                .then(runs[a].size.total_cmp(&runs[b].size))
        })
        .expect("non-empty line");
    let mut bounds = runs[members[0]].bounds;
    let mut left = f32::MAX;
    for &i in &members {
        bounds = bounds.union(&runs[i].bounds);
        left = left.min(runs[i].x);
    }
    Line {
        baseline: runs[main].y,
        size: runs[main].size,
        left,
        right: bounds.r,
        bounds,
        runs: members,
    }
}

// ---------------------------------------------------------------------------------------
// Paragraph grouping
// ---------------------------------------------------------------------------------------

fn align_tol(size: f32) -> f32 {
    (0.3 * size).max(1.0)
}

/// Symmetric insets on both sides — a centred line, whose left edge says nothing about
/// paragraph starts.
fn centred(line: &Line, lo: f32, hi: f32, tol: f32) -> bool {
    let inset_l = line.left - lo;
    let inset_r = hi - line.right;
    inset_l > tol && inset_r > tol && (inset_l - inset_r).abs() <= tol
}

fn extent(lines: &[Line], para: &[usize]) -> (f32, f32) {
    para.iter().fold((f32::MAX, f32::MIN), |(l, r), &i| {
        (l.min(lines[i].left), r.max(lines[i].right))
    })
}

/// The nearest line above (`up`) or below `from` that overlaps `[lo, hi]` horizontally.
fn neighbour(
    lines: &[Line],
    para: &[usize],
    from: &Line,
    lo: f32,
    hi: f32,
    up: bool,
) -> Option<usize> {
    let min_step = 0.3 * from.size;
    lines
        .iter()
        .enumerate()
        .filter(|(i, l)| {
            !para.contains(i)
                && l.right.min(hi) - l.left.max(lo) > 0.0
                && if up {
                    l.baseline > from.baseline + min_step
                } else {
                    l.baseline < from.baseline - min_step
                }
        })
        .min_by(|(_, a), (_, b)| {
            (a.baseline - from.baseline)
                .abs()
                .total_cmp(&(b.baseline - from.baseline).abs())
        })
        .map(|(i, _)| i)
}

/// Approximate ink width of the first word of `line` (its share of the line's characters).
fn first_word_width(runs: &[Run], line: &Line) -> f32 {
    let text = line_text(runs, line);
    let total = text.chars().count().max(1);
    let first = text
        .split_whitespace()
        .next()
        .map_or(0, |w| w.chars().count());
    (line.right - line.left) * first as f32 / total as f32
}

/// Is `line` the last line of a left-aligned/justified paragraph? It is left-flush, ends
/// before [`SHORT_LINE`] of the block, and the first word of `following` would have fitted
/// after it (so the break was not a wrap).
fn ends_paragraph(line: &Line, following_word: f32, left: f32, right: f32, tol: f32) -> bool {
    (line.left - left).abs() <= tol
        && line.right < left + SHORT_LINE * (right - left)
        && line.right + SYNTHETIC_SPACE * line.size + following_word < right - tol
}

/// Is `line` indented relative to `left` in a way that starts a paragraph? Not when it is
/// centred, and not when it is flush right with a clearly larger inset than an indent
/// (right-aligned text — `INDENT_MAX`) or the paragraph so far is already ragged-left.
fn starts_paragraph(line: &Line, lo: f32, hi: f32, ragged_left: bool, tol: f32) -> bool {
    let size = line.size;
    if line.left <= lo + INDENT * size || centred(line, lo, hi, tol) {
        return false;
    }
    let flush_right = (line.right - hi).abs() <= tol;
    !(flush_right && (ragged_left || line.left - lo > INDENT_MAX * size))
}

/// The lines of the paragraph around `hit`, top to bottom.
fn grow(runs: &[Run], lines: &[Line], hit: usize) -> Vec<usize> {
    let size = lines[hit].size;
    let tol = align_tol(size);
    let same_size = |l: &Line| (l.size - size).abs() <= SIZE_TOL * size;
    let leading_ok = |gap: f32, leading: Option<f32>| {
        gap <= MAX_LEADING * size && leading.is_none_or(|l| (gap - l).abs() <= LEADING_TOL * l)
    };
    // Two or more lines, all flush right, not all flush left.
    let ragged_left = |para: &[usize], lo: f32, hi: f32| {
        para.len() >= 2
            && para.iter().all(|&i| (lines[i].right - hi).abs() <= tol)
            && para.iter().any(|&i| (lines[i].left - lo).abs() > tol)
    };
    let mut para = vec![hit];
    let mut leading: Option<f32> = None;

    // Down.
    loop {
        let last = &lines[*para.last().expect("non-empty")];
        let (lo, hi) = extent(lines, &para);
        let Some(n) = neighbour(lines, &para, last, lo, hi, false) else {
            break;
        };
        let next = &lines[n];
        let gap = last.baseline - next.baseline;
        if !same_size(next) || !leading_ok(gap, leading) {
            break;
        }
        let (left, right) = (lo.min(next.left), hi.max(next.right));
        if ends_paragraph(last, first_word_width(runs, next), left, right, tol)
            || starts_paragraph(next, lo, hi, ragged_left(&para, lo, hi), tol)
        {
            break;
        }
        para.push(n);
        leading.get_or_insert(gap);
    }

    // Up.
    loop {
        let top = &lines[para[0]];
        let (lo, hi) = extent(lines, &para);
        let Some(p) = neighbour(lines, &para, top, lo, hi, true) else {
            break;
        };
        let prev = &lines[p];
        let gap = prev.baseline - top.baseline;
        if !same_size(prev) || !leading_ok(gap, leading) {
            break;
        }
        let (left, right) = (lo.min(prev.left), hi.max(prev.right));
        // The top line is indented relative to the line above: it is a first line.
        let mut with_prev = para.clone();
        with_prev.push(p);
        if starts_paragraph(top, left, right, ragged_left(&with_prev, left, right), tol)
            && top.left > prev.left + INDENT * size
        {
            break;
        }
        // The line above is the short last line of the paragraph before.
        if ends_paragraph(prev, first_word_width(runs, top), left, right, tol) {
            break;
        }
        para.insert(0, p);
        leading.get_or_insert(gap);
    }
    para
}

// ---------------------------------------------------------------------------------------
// Paragraph analysis
// ---------------------------------------------------------------------------------------

/// Everything the probe reports and the edit needs, for a set of lines (top to bottom).
#[derive(Debug, Clone)]
struct Info {
    ids: Vec<ObjectId>,
    rect: Rect,
    text: String,
    /// Run index of the dominant style.
    dominant: usize,
    mixed: bool,
    size: f32,
    line_height: f32,
    align: ParagraphAlign,
    indent: f32,
    /// Left edge of the body lines (glyph origins).
    body_left: f32,
    first_baseline: f32,
    lines: u32,
    reason: Option<NotEditableReason>,
}

fn collapse_ws(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut space = false;
    // PDFium reports a hyphen at the end of a line as U+0002.
    for ch in text.chars().map(|c| if c == '\u{2}' { '-' } else { c }) {
        if ch.is_whitespace() {
            space = true;
        } else {
            if space && !out.is_empty() {
                out.push(' ');
            }
            space = false;
            out.push(ch);
        }
    }
    if space && !out.is_empty() {
        out.push(' ');
    }
    // Leading whitespace is kept as one space so the caller can tell a word boundary.
    if text.starts_with(char::is_whitespace) {
        out.insert(0, ' ');
    }
    out
}

fn line_text(runs: &[Run], line: &Line) -> String {
    let mut out = String::new();
    let mut right: Option<f32> = None;
    for &i in &line.runs {
        let r = &runs[i];
        let piece = collapse_ws(&r.text);
        if let Some(prev) = right {
            if r.bounds.l - prev > WORD_GAP * line.size
                && !out.ends_with(' ')
                && !piece.starts_with(' ')
            {
                out.push(' ');
            }
        }
        out.push_str(&piece);
        right = Some(right.map_or(r.bounds.r, |p| p.max(r.bounds.r)));
    }
    out.trim().to_string()
}

fn join_lines(texts: &[String]) -> String {
    let mut out = String::new();
    for t in texts.iter().filter(|t| !t.is_empty()) {
        if out.is_empty() {
            out.push_str(t);
            continue;
        }
        let continues_lower = t.chars().next().is_some_and(char::is_lowercase);
        let hyphenated = out.ends_with('-')
            && out[..out.len() - 1]
                .chars()
                .last()
                .is_some_and(char::is_alphabetic);
        if hyphenated && continues_lower {
            out.pop();
        } else {
            out.push(' ');
        }
        out.push_str(t);
    }
    out
}

/// A justified line that could not reach the right edge: separate words, every gap between them
/// stretched well past a word space (see [`STRETCHED_GAP`]).
fn stretched(runs: &[Run], line: &Line) -> bool {
    let size = line.size;
    let mut gaps = line
        .runs
        .windows(2)
        .map(|w| runs[w[1]].bounds.l - runs[w[0]].bounds.r)
        .filter(|&gap| gap > WORD_GAP * size)
        .peekable();
    gaps.peek().is_some() && gaps.all(|gap| gap >= STRETCHED_GAP * size)
}

fn analyze(runs: &[Run], lines: &[&Line]) -> Info {
    let n = lines.len();
    let mut ids = Vec::new();
    let mut rect = lines[0].bounds;
    let mut styles: HashMap<(String, i32, Rgb), (usize, usize)> = HashMap::new();
    let mut reason = None;
    for line in lines {
        rect = rect.union(&line.bounds);
        for &i in &line.runs {
            let r = &runs[i];
            ids.push(r.index as ObjectId);
            if reason.is_none() {
                reason = r.reason;
            }
            let key = (r.font.clone(), (r.size * 2.0).round() as i32, r.color);
            let entry = styles.entry(key).or_insert((0, i));
            entry.0 += r.weight.max(1);
        }
    }
    let dominant = styles
        .values()
        .max_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(&a.1)))
        .map(|&(_, i)| i)
        .unwrap_or(lines[0].runs[0]);
    let size = runs[dominant].size;
    let texts: Vec<String> = lines.iter().map(|l| line_text(runs, l)).collect();

    let line_height = if n >= 2 {
        (lines[0].baseline - lines[n - 1].baseline) / (n - 1) as f32
    } else {
        size * super::LINE_HEIGHT
    };

    // Alignment from the edges; the first line is left out of the left-edge test (indent)
    // and the last line out of the right-edge test (it is short in a justified paragraph).
    let tol = align_tol(size);
    let body_left = if n >= 2 {
        lines[1..].iter().map(|l| l.left).fold(f32::MAX, f32::min)
    } else {
        lines[0].left
    };
    let block_right = lines.iter().map(|l| l.right).fold(f32::MIN, f32::max);
    let left_flush = n >= 2 && lines[1..].iter().all(|l| (l.left - body_left).abs() <= tol);
    let right_of_body = lines[..n - 1]
        .iter()
        .map(|l| l.right)
        .fold(f32::MIN, f32::max);
    let flush_right = lines[..n - 1]
        .iter()
        .filter(|l| (l.right - right_of_body).abs() <= tol || stretched(runs, l))
        .count();
    let align = if n >= 3
        && left_flush
        && flush_right >= 2
        && flush_right as f32 >= JUSTIFY_FLUSH_SHARE * (n - 1) as f32
    {
        ParagraphAlign::Justify
    } else if n < 2 || left_flush {
        ParagraphAlign::Left
    } else if lines.iter().all(|l| (l.right - block_right).abs() <= tol) {
        ParagraphAlign::Right
    } else {
        let lo = lines.iter().map(|l| l.left).fold(f32::MAX, f32::min);
        let mid = (lo + block_right) / 2.0;
        if lines
            .iter()
            .all(|l| ((l.left + l.right) / 2.0 - mid).abs() <= tol)
        {
            ParagraphAlign::Center
        } else {
            ParagraphAlign::Left
        }
    };
    let (indent, body_left) = match align {
        ParagraphAlign::Left | ParagraphAlign::Justify if n >= 2 => {
            (lines[0].left - body_left, body_left)
        }
        ParagraphAlign::Left | ParagraphAlign::Justify => (0.0, lines[0].left),
        _ => (0.0, lines.iter().map(|l| l.left).fold(f32::MAX, f32::min)),
    };

    Info {
        ids,
        rect,
        text: join_lines(&texts),
        dominant,
        mixed: styles.len() > 1,
        size,
        line_height,
        align,
        indent,
        body_left,
        first_baseline: lines[0].baseline,
        lines: n as u32,
        reason,
    }
}

// ---------------------------------------------------------------------------------------
// Trial object: coverage + advance widths
// ---------------------------------------------------------------------------------------

/// Advance widths per 1 pt of font size.
#[derive(Debug, Clone)]
struct Metrics {
    widths: HashMap<char, f32>,
    /// `None`: the font has no usable space glyph; words become separate objects.
    space: Option<f32>,
}

impl Metrics {
    fn char_width(&self, ch: char) -> f32 {
        self.widths.get(&ch).copied().unwrap_or(0.5)
    }

    fn width(&self, s: &str) -> f32 {
        s.chars().map(|c| self.char_width(c)).sum()
    }
}

/// Every distinct non-whitespace character of `text`, in order of appearance.
fn distinct_glyphs(text: &str) -> String {
    let mut seen = std::collections::HashSet::new();
    text.chars()
        .filter(|c| !c.is_whitespace() && seen.insert(*c))
        .collect()
}

/// Appends a trial object in `font` holding `sample` at `at`, reads it back, removes it.
/// `Some(widths)` (per 1 pt, one per char of `sample`) when the font draws all of it.
fn trial<'p>(
    page: &mut PdfPage<'p>,
    document: &PdfDocument<'p>,
    font: PdfFontToken,
    sample: &str,
) -> Result<Option<Vec<f32>>, EngineError> {
    let mut object = PdfPageTextObject::new(document, sample, font, PdfPoints::new(TRIAL_SIZE))
        .ctx("create trial text object")?;
    object
        .translate(PdfPoints::new(TRIAL_AT.0), PdfPoints::new(TRIAL_AT.1))
        .ctx("place trial text object")?;
    page.objects_mut()
        .add_text_object(object)
        .ctx("add trial text object")?;
    let index = page.objects().len() - 1;
    // No `?` until the trial object is gone again.
    let out = read_trial(page, index, sample);
    page.objects_mut()
        .remove_object_at_index(index)
        .ctx("remove trial text object")?;
    Ok(out)
}

fn read_trial(page: &PdfPage<'_>, index: usize, sample: &str) -> Option<Vec<f32>> {
    let text_page = page.text().ok()?;
    let object = page.objects().get(index).ok()?;
    let t = object.as_text_object()?;
    if text_page.for_object(t) != sample {
        return None;
    }
    let chars = t.chars(&text_page).ok()?;
    let mut widths = Vec::with_capacity(sample.len());
    for ch in chars.iter() {
        if ch.is_generated().unwrap_or(false) {
            continue;
        }
        let w = ch.loose_bounds().ok()?.width().value;
        if w.is_nan() || w <= 0.0 {
            return None;
        }
        widths.push(w / TRIAL_SIZE);
    }
    (widths.len() == sample.chars().count()).then_some(widths)
}

/// Coverage + metrics of `font` for `text`. `None` when the font cannot draw some glyph.
fn measure_font<'p>(
    page: &mut PdfPage<'p>,
    document: &PdfDocument<'p>,
    font: PdfFontToken,
    text: &str,
) -> Result<Option<Metrics>, EngineError> {
    let glyphs = distinct_glyphs(text);
    let Some(first) = glyphs.chars().next() else {
        return Ok(None);
    };
    let has_space = text
        .split('\n')
        .any(|seg| seg.split_whitespace().nth(1).is_some());
    let collect = |widths: &[f32]| -> HashMap<char, f32> {
        glyphs.chars().zip(widths.iter().copied()).collect()
    };
    if has_space {
        let sample = format!("{glyphs} {first}");
        if let Some(widths) = trial(page, document, font, &sample)? {
            let n = glyphs.chars().count();
            return Ok(Some(Metrics {
                widths: collect(&widths[..n]),
                space: Some(widths[n]),
            }));
        }
    }
    Ok(trial(page, document, font, &glyphs)?.map(|widths| Metrics {
        widths: collect(&widths),
        space: None,
    }))
}

fn font_token_of(page: &PdfPage<'_>, index: usize) -> Result<PdfFontToken, EngineError> {
    let object = object_at(page, index as ObjectId)?;
    let t = object
        .as_text_object()
        .ok_or_else(|| EngineError::invalid(format!("object {index} is not a text object")))?;
    let token = t.font().token();
    Ok(token)
}

// ---------------------------------------------------------------------------------------
// probe_paragraph
// ---------------------------------------------------------------------------------------

/// `probe_paragraph` — the paragraph under `at` (PDF points on the page), or `None` when
/// there is no text there. Never mutates: the trial object lives on a scratch page that is
/// closed without regenerating its content.
pub fn probe(
    doc: &mut OpenDoc<'_>,
    page_index: PageIndex,
    at: Point,
) -> Result<Option<ParagraphProbe>, EngineError> {
    let generation = doc.generation;
    let mut scratch = ScratchPage::open(doc, page_index)?;
    // SeePDF's own header / footer / watermark stamps are edited in their dialog; they never
    // join a paragraph (a header 4 pt above the first line would fold into it and be deleted).
    let stamps: HashSet<usize> = stamp::stamp_indices(doc.bindings(), &scratch.page, None)
        .into_iter()
        .collect();
    let collect =
        |page: &PdfPage<'_>, rot: Rot| -> Result<(Vec<Run>, Vec<Obstacle>), EngineError> {
            let text_page = page.text().ctx("load text page")?;
            let mut runs = Vec::new();
            let mut obstacles = Vec::new();
            for (index, object) in page.objects().iter().enumerate() {
                if stamps.contains(&index) {
                    continue;
                }
                match classify(index, &object, &text_page, rot) {
                    Classified::Run(r) => runs.push(r),
                    Classified::Obstacle(o) => obstacles.push(o),
                    Classified::Skip => {}
                }
            }
            Ok((runs, obstacles))
        };
    let hit_line = |lines: &[Line], (px, py): (f32, f32)| {
        lines
            .iter()
            .enumerate()
            .filter(|(_, l)| {
                let pad = 0.25 * l.size;
                let in_band = py >= l.baseline - 0.35 * l.size && py <= l.baseline + 0.95 * l.size;
                let in_box = py >= l.bounds.b - 1.0 && py <= l.bounds.t + 1.0;
                (in_band || in_box) && px >= l.left - pad && px <= l.right + pad
            })
            .min_by(|(_, a), (_, b)| {
                let da = (py - (a.baseline + 0.3 * a.size)).abs();
                let db = (py - (b.baseline + 0.3 * b.size)).abs();
                da.total_cmp(&db)
            })
            .map(|(i, _)| i)
    };
    let (mut runs, obstacles) = collect(&scratch.page, Rot::UPRIGHT)?;
    let [px, py] = at;
    let mut rot = Rot::UPRIGHT;
    let mut lines = build_lines(&runs, true);
    let mut hit = hit_line(&lines, (px, py));

    let inside =
        |r: &Rect| px >= r.l - 1.0 && px <= r.r + 1.0 && py >= r.b - 1.0 && py <= r.t + 1.0;
    if hit.is_none() {
        // v0.3 (R5): a click on rotated text probes again in that text's own frame.
        let rotated = obstacles
            .iter()
            .find(|o| o.reason == NotEditableReason::RotatedText && inside(&o.bounds))
            .and_then(|o| scratch.page.objects().get(o.index).ok())
            .and_then(|object| matrix_of(&object))
            .map(Rot::of_matrix)
            .filter(|r| !r.upright());
        if let Some(frame) = rotated {
            let (frame_runs, _) = collect(&scratch.page, frame)?;
            let frame_lines = build_lines(&frame_runs, true);
            if let Some(h) = hit_line(&frame_lines, frame.to_frame((px, py))) {
                rot = frame;
                runs = frame_runs;
                lines = frame_lines;
                hit = Some(h);
            }
        }
    }
    let Some(hit) = hit else {
        // v0.3.1: text inside a group (nested groups too) under the point.
        if let Some(p) = grouped_probe(&mut scratch, doc.bindings(), at, generation)? {
            return Ok(Some(p));
        }
        return Ok(obstacles
            .iter()
            .find(|o| inside(&o.bounds))
            .map(|o| obstacle_probe(o, generation)));
    };

    let para = grow(&runs, &lines, hit);
    let para_lines: Vec<&Line> = para.iter().map(|&i| &lines[i]).collect();
    let info = analyze(&runs, &para_lines);
    let dominant = &runs[info.dominant];

    // The edit rewrites the content streams that hold the paragraph (see `run`): refuse up
    // front, before anything is typed, when that would lose an inline image or a shading.
    let own: Vec<usize> = info.ids.iter().map(|&i| i as usize).collect();
    let unwritable = info.reason.is_none()
        && raw::page::rewrite_set(doc.bindings(), doc.pdf(), page_index, &own)?.is_none();
    let (strategy, substitute_font, reason) = if let Some(reason) = info.reason {
        (TextEditStrategy::Refused, None, Some(reason))
    } else if unwritable {
        (
            TextEditStrategy::Refused,
            None,
            Some(NotEditableReason::UnwritableContent),
        )
    } else {
        let token = font_token_of(&scratch.page, dominant.index)?;
        let covered = measure_font(&mut scratch.page, doc.pdf(), token, &info.text)?.is_some();
        if covered {
            (TextEditStrategy::InPlace, None, None)
        } else {
            let pick = fonts::pick(&info.text);
            let substitutable = pick == fonts::Pick::Helvetica
                || fonts::covers(&info.text).is_ok_and(|c| c.is_complete());
            if substitutable {
                (
                    TextEditStrategy::ReplaceFont,
                    Some(pick.name().to_string()),
                    Some(NotEditableReason::GlyphsMissing),
                )
            } else {
                (
                    TextEditStrategy::Refused,
                    None,
                    Some(NotEditableReason::GlyphsMissing),
                )
            }
        }
    };
    // Dropping the scratch page without `regenerate_content()` discards the trial objects.
    Ok(Some(ParagraphProbe {
        object_ids: info.ids,
        // v0.3: the page-aligned box around a rotated paragraph.
        rect: rot.rect_to_page(info.rect),
        text: info.text,
        font_name: dominant.font.clone(),
        font_size_pt: info.size,
        color: dominant.color,
        mixed_styles: info.mixed,
        line_height_pt: info.line_height,
        align: info.align,
        first_line_indent_pt: info.indent,
        lines: info.lines,
        strategy,
        substitute_font,
        reason,
        doc_generation: generation,
        group_object_id: None,
        group_depth: None,
    }))
}

/// v0.3.1: the text inside a group under `at` ([`ungroup::group_hit`]), as a refused probe:
/// `insideXObject` with `groupObjectId` when 그룹 해제 would make it editable — or the reason it
/// could not be edited even then (`noUnicode`, `invisible`: a run nobody can see, like an
/// optional-content layer that is off), so the UI never offers a 그룹 해제 that cannot lead to
/// an edit. `None` when no grouped text is there.
fn grouped_probe(
    scratch: &mut ScratchPage<'_>,
    bindings: &'static dyn PdfiumLibraryBindings,
    at: Point,
    generation: DocGeneration,
) -> Result<Option<ParagraphProbe>, EngineError> {
    if !ungroup::has_grouped_text(bindings, &scratch.page) {
        return Ok(None);
    }
    let Some(hit) = ungroup::group_hit(bindings, &scratch.page, at, None)? else {
        return Ok(None);
    };
    let (mut size, mut r, mut g, mut b, mut a) = (0f32, 0u32, 0u32, 0u32, 0u32);
    // SAFETY: `hit.text` is a live text object of the scratch page; out-params outlive the calls.
    let (mode, sized, coloured) = unsafe {
        (
            bindings.FPDFTextObj_GetTextRenderMode(hit.text) as i64,
            bindings.is_true(bindings.FPDFTextObj_GetFontSize(hit.text, &mut size)),
            bindings.is_true(
                bindings.FPDFPageObj_GetFillColor(hit.text, &mut r, &mut g, &mut b, &mut a),
            ),
        )
    };
    let height = (hit.rect.t - hit.rect.b).abs();
    let size = if sized && size > 0.0 { size } else { height };
    let mut reason = if matches!(mode, 3 | 7) {
        Some(NotEditableReason::Invisible)
    } else if !super::has_usable_unicode(&hit.chars) {
        Some(NotEditableReason::NoUnicode)
    } else {
        None
    };
    if reason.is_none() && !visible(scratch, bindings, &hit)? {
        reason = Some(NotEditableReason::Invisible);
    }
    let editable = reason.is_none();
    Ok(Some(ParagraphProbe {
        object_ids: vec![hit.top as ObjectId],
        rect: hit.rect,
        text: collapse_ws(&hit.chars).trim().to_string(),
        font_name: String::new(),
        font_size_pt: size,
        color: if coloured {
            [r as u8, g as u8, b as u8]
        } else {
            [0, 0, 0]
        },
        mixed_styles: false,
        line_height_pt: size * super::LINE_HEIGHT,
        align: ParagraphAlign::Left,
        first_line_indent_pt: 0.0,
        lines: 1,
        strategy: TextEditStrategy::Refused,
        substitute_font: None,
        reason: Some(reason.unwrap_or(NotEditableReason::InsideXObject)),
        doc_generation: generation,
        group_object_id: editable.then_some(hit.top as ObjectId),
        group_depth: editable.then_some(hit.chain.len() as u32 + 1),
    }))
}

/// Does the grouped run show on the page at all? Rendered with it and without it (taken out of
/// its form on the scratch page, which is never written back): no pixel changes when it is
/// hidden — an optional-content layer that is off, white on white, covered by an image.
fn visible(
    scratch: &mut ScratchPage<'_>,
    bindings: &'static dyn PdfiumLibraryBindings,
    hit: &ungroup::GroupHit,
) -> Result<bool, EngineError> {
    let scale = (1.5e6 / (scratch.page.width().value * scratch.page.height().value).max(1.0))
        .sqrt()
        .min(1.5);
    let with = ungroup::render_look(&scratch.page, scale)?;
    // SAFETY: both handles belong to the live scratch page; the removed object is ours to free.
    let removed = unsafe { bindings.FPDFFormObj_RemoveObject(hit.parent, hit.text) };
    if !bindings.is_true(removed) {
        return Ok(true);
    }
    // SAFETY: removed above, so unowned.
    unsafe { bindings.FPDFPageObj_Destroy(hit.text) };
    let without = ungroup::render_look(&scratch.page, scale)?;
    let (mean, share) = ungroup::look_diff(&with, &without);
    Ok(mean > 0.0 || share > 0.0)
}

fn obstacle_probe(o: &Obstacle, generation: DocGeneration) -> ParagraphProbe {
    ParagraphProbe {
        object_ids: vec![o.index as ObjectId],
        rect: o.bounds,
        text: collapse_ws(&o.text).trim().to_string(),
        font_name: o.font.clone(),
        font_size_pt: o.size,
        color: o.color,
        mixed_styles: false,
        line_height_pt: o.size * super::LINE_HEIGHT,
        align: ParagraphAlign::Left,
        first_line_indent_pt: 0.0,
        lines: 1,
        strategy: TextEditStrategy::Refused,
        substitute_font: None,
        reason: Some(o.reason),
        doc_generation: generation,
        group_object_id: None,
        group_depth: None,
    }
}

// ---------------------------------------------------------------------------------------
// edit_paragraph
// ---------------------------------------------------------------------------------------

/// One text object of the new layout.
#[derive(Debug, Clone, PartialEq)]
struct Placed {
    text: String,
    x: f32,
    baseline: f32,
}

#[derive(Debug, Clone, Copy)]
struct Frame {
    size: f32,
    box_l: f32,
    box_r: f32,
    indent: f32,
    align: ParagraphAlign,
    first_baseline: f32,
    line_height: f32,
}

/// Greedy line breaking at spaces (by character only when a word alone overflows), then
/// placement. Returns the objects and the number of lines (blank lines included).
fn layout(text: &str, m: &Metrics, f: Frame) -> (Vec<Placed>, u32) {
    let size = f.size;
    let width = (f.box_r - f.box_l).max(size);
    let space = m.space.unwrap_or(SYNTHETIC_SPACE) * size;
    let slack = FIT_SLACK * size;
    let indent = match f.align {
        ParagraphAlign::Left | ParagraphAlign::Justify => f.indent,
        _ => 0.0,
    };
    let avail_for = |row: usize| width - if row == 0 { indent } else { 0.0 };

    // (words with widths, last line of its hard-break segment)
    let mut rows: Vec<(Vec<(String, f32)>, bool)> = Vec::new();
    for segment in text.split('\n') {
        let mut current: Vec<(String, f32)> = Vec::new();
        let mut used = 0.0f32;
        for word in segment.split_whitespace() {
            let ww = m.width(word) * size;
            let needed = if current.is_empty() {
                ww
            } else {
                used + space + ww
            };
            if needed <= avail_for(rows.len()) + slack {
                current.push((word.to_string(), ww));
                used = needed;
                continue;
            }
            if !current.is_empty() {
                rows.push((std::mem::take(&mut current), false));
            }
            if ww <= avail_for(rows.len()) + slack {
                current.push((word.to_string(), ww));
                used = ww;
                continue;
            }
            // A word wider than the box: break it by character.
            let mut chunk = String::new();
            let mut cw = 0.0f32;
            for ch in word.chars() {
                let w = m.char_width(ch) * size;
                if !chunk.is_empty() && cw + w > avail_for(rows.len()) + slack {
                    rows.push((vec![(std::mem::take(&mut chunk), cw)], false));
                    cw = 0.0;
                }
                chunk.push(ch);
                cw += w;
            }
            current.push((chunk, cw));
            used = cw;
        }
        rows.push((current, true));
    }

    let per_word = m.space.is_none();
    let mut placed = Vec::new();
    for (row, (words, last)) in rows.iter().enumerate() {
        if words.is_empty() {
            continue;
        }
        let baseline = f.first_baseline - row as f32 * f.line_height;
        let ind = if row == 0 { indent } else { 0.0 };
        let avail = width - ind;
        let sum: f32 = words.iter().map(|w| w.1).sum();
        let gaps = (words.len() - 1) as f32;
        let natural = sum + space * gaps;
        let x0 = match f.align {
            ParagraphAlign::Left | ParagraphAlign::Justify => f.box_l + ind,
            ParagraphAlign::Right => f.box_r - natural,
            ParagraphAlign::Center => f.box_l + (width - natural) / 2.0,
        };
        let justify = f.align == ParagraphAlign::Justify && !last && words.len() > 1;
        if justify || per_word {
            let gap = if justify {
                ((avail - sum) / gaps).min((JUSTIFY_MAX_GAP * size).max(space))
            } else {
                space
            };
            let mut x = x0;
            for (word, w) in words {
                placed.push(Placed {
                    text: word.clone(),
                    x,
                    baseline,
                });
                x += w + gap;
            }
        } else {
            let joined: Vec<&str> = words.iter().map(|w| w.0.as_str()).collect();
            placed.push(Placed {
                text: joined.join(" "),
                x: x0,
                baseline,
            });
        }
    }
    (placed, rows.len() as u32)
}

/// Reads the paragraph's runs back from explicit object ids (the probe's), checking each.
fn runs_for(
    bindings: &'static dyn PdfiumLibraryBindings,
    page: &PdfPage<'_>,
    ids: &[ObjectId],
    rot: Rot,
) -> Result<Vec<Run>, EngineError> {
    let text_page = page.text().ctx("load text page")?;
    let stamps: HashSet<usize> = stamp::stamp_indices(bindings, page, None)
        .into_iter()
        .collect();
    let mut runs = Vec::with_capacity(ids.len());
    for &id in ids {
        if stamps.contains(&(id as usize)) {
            return Err(EngineError::invalid(format!(
                "object {id} is a SeePDF stamp, not paragraph text"
            )));
        }
        let object = object_at(page, id)?;
        match classify(id as usize, &object, &text_page, rot) {
            Classified::Run(r) => {
                if let Some(reason) = r.reason {
                    return Err(not_editable(reason).with_detail(format!("object {id}")));
                }
                runs.push(r);
            }
            Classified::Obstacle(o) => {
                return Err(not_editable(o.reason).with_detail(format!("object {id}")));
            }
            Classified::Skip => {
                if object.object_type() != PdfPageObjectType::Text {
                    return Err(EngineError::invalid(format!(
                        "object {id} is not a text object"
                    )));
                }
                // A whitespace-only run inside the paragraph: deleted with the rest.
            }
        }
    }
    if runs.is_empty() {
        return Err(EngineError::invalid("the paragraph has no visible text"));
    }
    Ok(runs)
}

/// Nominal descent below the last baseline, × size. The flow compares **line grids**, not
/// ink: a descender typed into (or deleted from) the last line must not nudge everything
/// below by 2 pt, and an unchanged line count must mean "nothing moves".
const NOMINAL_DESCENT: f32 = 0.25;
/// `fit`: the smallest factor tried, the step, and how close to the original bottom counts
/// as fitting (the grid arithmetic is exact, so this only absorbs float noise).
const FIT_MIN: f32 = 0.7;
const FIT_STEP: f32 = 0.02;
const FIT_TOL: f32 = 0.05;

/// What one edit did (or, for a dry run, would do).
struct Outcome {
    rect: Rect,
    lines: u32,
    overflow: f32,
    shifted: f32,
    moved_objects: u32,
    moved_annotations: u32,
    room: f32,
    blocked: Option<FlowBlocked>,
    fit_scale: Option<f32>,
    past_bottom: f32,
    moved_band: Option<Rect>,
    /// v0.3 (R5): an order anchor was left for `restore_order`.
    anchored: bool,
    /// v0.3 A8 (pkg4): moved annotations whose `/L`, `/Vertices` or `/CL` still needs the
    /// lopdf pass ([`flow::lopdf_geometry`]), and the distance they moved.
    lopdf_annots: Vec<usize>,
    dy: f32,
}

impl Outcome {
    fn into_result(self, objects: crate::ipc::types::PageObjectList) -> ParagraphEditResult {
        ParagraphEditResult {
            objects,
            rect: self.rect,
            lines: self.lines,
            overflow_pt: round2(self.overflow.max(0.0)),
            shifted_pt: round2(self.shifted),
            moved_objects: self.moved_objects,
            moved_annotations: self.moved_annotations,
            room_pt: round2(self.room.max(0.0)),
            blocked: self.blocked,
            fit_scale: self.fit_scale,
            past_bottom_pt: round2(self.past_bottom.max(0.0)),
            moved_band: self.moved_band,
        }
    }
}

fn round2(v: f32) -> f32 {
    let r = (v * 100.0).round() / 100.0;
    if r == 0.0 {
        0.0 // no "-0"
    } else {
        r
    }
}

/// `edit_paragraph` — replaces the paragraph's objects with the new text, reflowed inside
/// the paragraph's box, and applies the Stage 9 flow to what follows it (see
/// [`super::flow`]): `push` (default) moves the content below by the growth, `overlap` moves
/// nothing, `fit` shrinks font size and leading until the text fits its original height.
/// One `registry::mutate` = one undo step (`undo.paragraphEdit`) for the text **and** every
/// move.
///
/// `dryRun` runs the same layout and plan without mutating: no generation bump, no undo
/// entry, nothing embedded (a substitute font is measured in a throwaway document), and the
/// page's trial objects are dropped with a scratch page that is never regenerated. Its
/// `objects` list is empty (the page is as it was).
///
/// An empty (or whitespace-only) `text` deletes the paragraph: nothing is laid out (`lines` 0,
/// `rect` a zero-height box at the paragraph's top), and `push` pulls what follows up into its
/// place — the top of the first thing below moves to the paragraph's top — so emptying a
/// paragraph does not leave a hole. `overlap` and `fit` only delete.
///
/// Refused with `unsupported` / `unwritableContent` when rewriting the paragraph's content
/// streams would lose an inline image or a shading object (see `raw::page::rewrite_set`):
/// the dry run and the write rehearse the rewrite, and the write re-parses the page and rolls
/// back. When only the streams of the content *below* cannot be rewritten, that content stays
/// where it is and is treated as an obstacle (`blocked: obstacle`), so `overlap` and `fit`
/// remain possible.
pub fn edit(
    st: &mut EngineState<'_>,
    doc_id: &str,
    page_index: PageIndex,
    expect_generation: DocGeneration,
    edit: ParagraphEdit,
    allow_font_substitution: bool,
) -> Result<ParagraphEditResult, EngineError> {
    check_generation(st.doc(doc_id)?, expect_generation)?;
    if edit.object_ids.is_empty() {
        return Err(EngineError::invalid("objectIds is empty"));
    }
    let text = edit.text.replace("\r\n", "\n").replace('\r', "\n");
    // No text at all: the paragraph is deleted (and, with `push`, what follows takes its place).
    let text = if text.trim().is_empty() {
        String::new()
    } else {
        text
    };
    if let Some(size) = edit.font_size_pt {
        check_font_size(size)?;
    }
    if let Some(w) = edit.width {
        if !w.is_finite() || w < 1.0 {
            return Err(EngineError::invalid(format!(
                "width {w} pt is out of range"
            )));
        }
    }
    let mut ids = edit.object_ids.clone();
    ids.sort_unstable();
    ids.dedup();
    let flow = edit.flow.unwrap_or_default();
    let pdfium = st.pdfium;
    // v0.3 (R5): the new text keeps the old paragraph's place in the reading order — a
    // `lopdf` pass after the write (on an encrypted file, over its decrypted serialisation,
    // re-encrypted with its own key: `security::lopdf_pass`). v0.3 integration (R5 × S1): it
    // is not run on a signed file that can still be saved incrementally (`pristine`): the
    // rewrite would force a full save, which invalidates the signatures; there the edit keeps
    // them and the paragraph reads last instead. Nor where an encryption cannot be rewritten
    // (checked first, so no order anchor is left behind for a pass that cannot run).
    let keep_order = !edit.dry_run
        && !st.doc(doc_id)?.pristine
        && crate::engine::security::can_rewrite(st, doc_id)?;
    let job = Job {
        page_index,
        ids: &ids,
        text: &text,
        edit: &edit,
        flow,
        allow_font_substitution,
        keep_order,
    };

    let outcome = if edit.dry_run {
        let doc = st.doc_mut(doc_id)?;
        run(doc, pdfium, &job, false)?
    } else {
        registry::mutate(
            st,
            doc_id,
            MutateOpts::new("undo.paragraphEdit", ChangeReason::Edit).page(page_index),
            |doc| run(doc, pdfium, &job, true),
        )?
    };
    // Nothing is anchored for an emptied paragraph: there is nothing to move.
    if keep_order && outcome.anchored {
        // Best effort: the edit itself is done and verified; a failure here only leaves the
        // paragraph last in the reading order (and an empty placeholder where it was).
        if let Err(e) = restore_order(st, doc_id, page_index) {
            tracing::warn!(doc_id, page = page_index, error = %e, "paragraph reading order not restored");
        }
    }
    // v0.3 A8 (pkg4): a moved Line / Polygon / callout keeps its geometry keys in step with
    // its `/Rect` — a lopdf pass coalesced into the same undo step (as a page resize does).
    // An encrypted document is rewritten through S2's re-encryption; where its encryption
    // cannot be rewritten the pass fails and only the appearance moves.
    // v0.3 integration (A8 × S1): nor a signed file that still saves incrementally — the
    // rewrite would force a full save and invalidate its signatures (as R5's reorder above).
    // It runs after R5's reorder, which leaves the annotations (and their slots) as they are.
    if !edit.dry_run && !outcome.lopdf_annots.is_empty() {
        let rewritable = st.doc(doc_id)?.quiet_rewrite_ok();
        if rewritable {
            let opts = MutateOpts::new("undo.paragraphEdit", ChangeReason::Edit)
                .page(page_index)
                .coalesced();
            st.doc_mut(doc_id)?.history.refresh_last();
            let (slots, dy) = (outcome.lopdf_annots.clone(), outcome.dy);
            // The edit itself is done: a failed pass (rolled back by `mutate_bytes`) leaves the
            // keys stale, as before, rather than reporting the applied edit as failed.
            if let Err(e) = registry::mutate_bytes(st, doc_id, opts, move |bytes, _| {
                crate::engine::annot::lopdf_annots::translate_annots(bytes, page_index, &slots, dy)
            }) {
                tracing::warn!(doc_id, error = %e.message, "paragraph flow: annotation geometry pass failed");
            }
        }
    }
    // A dry run changed nothing: no need to list the page again (on a dense page the listing
    // costs more than the plan).
    let objects = if edit.dry_run {
        crate::ipc::types::PageObjectList {
            doc_generation: st.doc(doc_id)?.generation,
            objects: Vec::new(),
        }
    } else {
        relist(st, doc_id, page_index)?
    };
    Ok(outcome.into_result(objects))
}

/// The arguments of one [`edit`], validated.
struct Job<'a> {
    page_index: PageIndex,
    /// Sorted, deduplicated.
    ids: &'a [ObjectId],
    /// Line breaks normalised to `\n`.
    text: &'a str,
    edit: &'a ParagraphEdit,
    flow: ParagraphFlow,
    allow_font_substitution: bool,
    /// v0.3 (R5): leave an order anchor and mark the new objects (see [`restore_order`]).
    keep_order: bool,
}

/// v0.3 (R5): moves the new paragraph back to where the old one was in the page's content
/// stream (`flow::restore_reading_order`), inside the same undo step: the bytes are replaced
/// without a history entry of their own, and the page renders exactly as before.
fn restore_order(
    st: &mut EngineState<'_>,
    doc_id: &str,
    page_index: PageIndex,
) -> Result<bool, EngineError> {
    let Some(out) = crate::engine::security::lopdf_pass(st, doc_id, |bytes| {
        flow::restore_reading_order(bytes, page_index)
    })?
    else {
        return Ok(false);
    };
    let (pages, password) = {
        let doc = st.doc(doc_id)?;
        (doc.page_count(), doc.password.clone())
    };
    crate::engine::save::verify_bytes(st, &out, pages, password)?;
    registry::replace(st, doc_id, std::sync::Arc::from(out.into_boxed_slice()))?;
    Ok(true)
}

/// The placeholder [`flow::restore_reading_order`] looks for: a copy of the object at `at`,
/// moved out of a throw-away second parse (so it keeps that object's content stream), inserted
/// at `at`, emptied and marked. On any failure nothing is left behind.
fn order_anchor(
    bindings: &'static dyn PdfiumLibraryBindings,
    document: &PdfDocument<'_>,
    page: &PdfPage<'_>,
    page_index: PageIndex,
    at: usize,
) -> Result<(), EngineError> {
    let mut source = document
        .pages()
        .get(page_index as PdfPageIndex)
        .ctx(&format!("load page {page_index}"))?;
    source.set_content_regeneration_strategy(PdfPageContentRegenerationStrategy::Manual);
    let handle = crate::engine::redact::raw::transplant_at(bindings, &source, at, page, at)?;
    drop(source);
    let emptied = (|| {
        let mut object = page.objects().get(at).ctx("load the anchor")?;
        object
            .as_text_object_mut()
            .ok_or_else(|| EngineError::invalid("the anchor is not a text object"))?
            .set_text("")
            .ctx("empty the anchor")?;
        raw::object::add_mark(
            bindings,
            document,
            page,
            at,
            raw::object::Mark::plain(flow::ORDER_ANCHOR),
        )
    })();
    if let Err(e) = emptied {
        let _ = crate::engine::redact::raw::remove_and_destroy(bindings, page, handle);
        return Err(e);
    }
    Ok(())
}

/// Layout, flow plan and — when `apply` — the page rewrite. The caller is inside
/// `registry::mutate` exactly when `apply` is true.
fn run<'p>(
    doc: &mut OpenDoc<'p>,
    pdfium: &'p Pdfium,
    job: &Job<'_>,
    apply: bool,
) -> Result<Outcome, EngineError> {
    let page_index = job.page_index;
    let text = job.text;
    let edit = job.edit;
    let bindings = doc.bindings();
    let crop = doc.geom(page_index)?.crop;
    let mut scratch = ScratchPage::open(doc, page_index)?;
    // v0.3 (R5): the paragraph's frame, from its first run (upright for upright text).
    let rot = scratch
        .page
        .objects()
        .get(job.ids[0] as usize)
        .ok()
        .and_then(|o| matrix_of(&o))
        .map(Rot::of_matrix)
        .unwrap_or(Rot::UPRIGHT);
    let runs = runs_for(bindings, &scratch.page, job.ids, rot)?;
    let mut lines = build_lines(&runs, false);
    lines.sort_by(|a, b| b.baseline.total_cmp(&a.baseline));
    let line_refs: Vec<&Line> = lines.iter().collect();
    let info = analyze(&runs, &line_refs);
    let last_baseline = lines.last().map_or(info.first_baseline, |l| l.baseline);
    let dominant = runs[info.dominant].clone();

    // Font: the paragraph's own when it draws every glyph, else the substitute.
    let own = font_token_of(&scratch.page, dominant.index)?;
    // Nothing to lay out: the paragraph is deleted (see `edit`).
    let emptied = text.is_empty();
    // A dry run measures the substitute in a throwaway document (see `edit`).
    let mut measuring_doc: Option<PdfDocument<'p>> = None;
    let measured = if emptied {
        Some(Metrics {
            widths: HashMap::new(),
            space: None,
        })
    } else {
        measure_font(&mut scratch.page, doc.pdf(), own, text)?
    };
    let (token, metrics) = match measured {
        Some(m) => (own, m),
        None => {
            let pick = fonts::pick(text);
            if !job.allow_font_substitution {
                return Err(EngineError::new(
                    ErrorCode::FontCoverage,
                    format!(
                        "the paragraph's font cannot draw this text; it would have to be \
                         replaced with {}",
                        pick.name()
                    ),
                )
                .with_page(page_index));
            }
            let cannot = || {
                EngineError::new(
                    ErrorCode::FontCoverage,
                    format!("{} cannot draw this text either", pick.name()),
                )
                .with_page(page_index)
            };
            let flat: String = text.replace('\n', " ");
            if apply {
                doc.adopt_hangul_token(&scratch.page);
                let (token, _) = doc.hangul_token_for(&flat)?;
                let m =
                    measure_font(&mut scratch.page, doc.pdf(), token, text)?.ok_or_else(cannot)?;
                (token, m)
            } else {
                let mut sub = pdfium.create_new_pdf().ctx("create measuring document")?;
                let (token, _) = fonts::token_for(&mut sub, &flat, &mut None)?;
                let mut page = sub
                    .pages_mut()
                    .create_page_at_end(PdfPagePaperSize::a4())
                    .ctx("create measuring page")?;
                let m = measure_font(&mut page, &sub, token, text)?.ok_or_else(cannot)?;
                drop(page);
                measuring_doc = Some(sub);
                (token, m)
            }
        }
    };

    // Layout — for `fit`, the largest factor in [0.7, 1] whose grid fits the original height.
    let base_size = edit.font_size_pt.unwrap_or(info.size);
    let scale = if info.size > 0.0 {
        base_size / info.size
    } else {
        1.0
    };
    let base_leading = info.line_height * scale;
    let box_r = match edit.width {
        Some(w) => info.rect.l + w,
        None => info.rect.r,
    };
    let align = edit.align.unwrap_or(info.align);
    let frame_at = |factor: f32| Frame {
        size: base_size * factor,
        box_l: info.body_left,
        box_r,
        indent: info.indent,
        align,
        first_baseline: info.first_baseline,
        line_height: base_leading * factor,
    };
    let original_bottom = last_baseline - NOMINAL_DESCENT * info.size;
    // > 0: the new grid ends lower than the old one.
    let growth_of = |rows: u32, factor: f32| {
        let last = info.first_baseline - rows.saturating_sub(1) as f32 * base_leading * factor;
        original_bottom - (last - NOMINAL_DESCENT * base_size * factor)
    };
    let (placed, rows, factor) = match job.flow {
        _ if emptied => (Vec::new(), 0, 1.0),
        ParagraphFlow::Fit => {
            let steps = ((1.0 - FIT_MIN) / FIT_STEP).round() as u32;
            let mut chosen = None;
            for k in 0..=steps {
                let factor = (1.0 - k as f32 * FIT_STEP).max(FIT_MIN);
                let (placed, rows) = layout(text, &metrics, frame_at(factor));
                if growth_of(rows, factor) <= FIT_TOL || k == steps {
                    chosen = Some((placed, rows, factor));
                    break;
                }
            }
            chosen.expect("the last step always chooses")
        }
        _ => {
            let (placed, rows) = layout(text, &metrics, frame_at(1.0));
            (placed, rows, 1.0)
        }
    };
    let size = base_size * factor;
    // An emptied paragraph's growth is known once the plan says what follows it (below).
    let mut growth = if emptied {
        0.0
    } else {
        growth_of(rows, factor)
    };
    let color = edit.color.unwrap_or(dominant.color);
    let render_mode = dominant.render_mode;

    // The new objects, measured before anything on the page changes.
    let built = {
        let document = measuring_doc.as_ref().unwrap_or_else(|| doc.pdf());
        build_objects(document, token, size, color, render_mode, &placed, rot)?
    };
    let new_rect = built
        .iter()
        .map(|(_, r)| *r)
        .reduce(|a, b| a.union(&b))
        // Deleted: a zero-height box at the paragraph's top (its column stays the column).
        .unwrap_or(rot.rect_to_page(Rect::new(
            info.rect.l,
            info.rect.t,
            info.rect.r,
            info.rect.t,
        )));

    // What follows the paragraph, from the page as it is (indices still the listed ones).
    let own_ids: HashSet<usize> = job.ids.iter().map(|&i| i as usize).collect();
    let (items, risky) = flow_items(bindings, &scratch.page, &own_ids);
    let annots = flow::read_annots(bindings, &scratch.page);
    let mut plan = flow::plan(
        &items,
        &annots,
        &flow::Geometry {
            crop,
            original: info.rect,
            new_rect,
            size: info.size,
            leading: info.line_height,
            grid_bottom: original_bottom,
        },
    );
    if emptied {
        // What follows moves up into the paragraph's place: its top to the paragraph's top.
        growth = plan.stack_top.map_or(0.0, |top| top - info.rect.t).min(0.0);
    }
    // How far the new ink reaches below the old grid bottom: a descender deeper than the
    // nominal descent reaches further than the grid says.
    let reach = if emptied {
        growth
    } else {
        growth.max(original_bottom - new_rect.b)
    };
    if !rot.upright() {
        // A rotated paragraph reflows in its own frame; nothing on the page moves for it.
        plan = plan.frozen();
    }
    let mut decision = if rot.upright() {
        flow::decide(&plan, job.flow, growth, reach)
    } else {
        flow::decide(&plan, ParagraphFlow::Overlap, 0.0, 0.0)
    };
    let is_moving = |d: &flow::Decision, p: &flow::Plan| {
        d.shift.abs() >= flow::MIN_SHIFT && !p.movable.is_empty()
    };
    // The content streams a write regenerates: those of the paragraph's objects (removed) and
    // of the objects the flow moves; the new text goes into a new stream of its own. Rehearse
    // it — the dry run and the write alike — and regenerate whatever else it would disturb.
    let own: Vec<usize> = job.ids.iter().map(|&i| i as usize).collect();
    let mut dirty = own.clone();
    if is_moving(&decision, &plan) {
        dirty.extend(plan.movable.iter().copied());
    }
    let rewrite = match raw::page::rewrite_set(bindings, doc.pdf(), page_index, &dirty)? {
        Some(rewrite) => rewrite,
        None if dirty.len() > own.len() => {
            // What follows sits in a stream PDFium cannot rewrite (an inline image or a shading
            // shares it): it stays where it is and is in the paragraph's way.
            let Some(rewrite) = raw::page::rewrite_set(bindings, doc.pdf(), page_index, &own)?
            else {
                return Err(unwritable(page_index));
            };
            plan = plan.frozen();
            decision = flow::decide(&plan, job.flow, growth, reach);
            rewrite
        }
        None => return Err(unwritable(page_index)),
    };
    let extra = rewrite.extra;
    let moving = is_moving(&decision, &plan);
    let dy = -decision.shift;
    let moved: &[usize] = if moving { &plan.movable } else { &[] };

    let mut moved_annotations = if moving { plan.annots.len() as u32 } else { 0 };
    let mut anchored = false;
    let mut lopdf_annots = Vec::new();
    if apply {
        let count = scratch.page.objects().len();
        // Moves first, while the listed indices are still valid; then add first, remove
        // afterwards: the new objects hold the font alive (an embedded font is only
        // referenced by its text objects), and the old indices stay valid.
        for &index in moved {
            raw::object::translate(bindings, &scratch.page, index, 0.0, dy)?;
        }
        // An identity transform marks an object dirty: its stream is regenerated with it whole.
        for &index in &extra {
            raw::object::translate(bindings, &scratch.page, index, 0.0, 0.0)?;
        }
        // v0.3.1: an ICC / calibrated / separation colour of an object the write regenerates
        // would come back black (an ungrouped figure's colours) — re-set it as RGB first.
        for &index in &rewrite.recolour {
            raw::page::normalise_colours(bindings, &scratch.page, index);
        }
        let added = built.len();
        // v0.3 (R5): a placeholder where the paragraph starts, in its content stream.
        anchored = job.keep_order
            && added > 0
            && order_anchor(
                bindings,
                doc.pdf(),
                &scratch.page,
                page_index,
                job.ids[0] as usize,
            )
            .is_ok();
        let shift = usize::from(anchored);
        let first_new = scratch.page.objects().len();
        for (object, _) in built {
            scratch
                .page
                .objects_mut()
                .add_text_object(object)
                .ctx("add text object")?;
        }
        if anchored {
            for index in first_new..first_new + added {
                raw::object::add_mark(
                    bindings,
                    doc.pdf(),
                    &scratch.page,
                    index,
                    raw::object::Mark::plain(flow::ORDER_PARA),
                )?;
            }
        }
        for &id in job.ids.iter().rev() {
            let index = id as usize + shift;
            scratch
                .page
                .objects_mut()
                .remove_object_at_index(index)
                .ctx(&format!("remove object {id}"))?;
        }
        scratch
            .page
            .regenerate_content()
            .ctx("regenerate page content")?;
        if moving && !plan.annots.is_empty() {
            moved_annotations = flow::move_annots(bindings, &scratch.page, &plan.annots, dy)?;
            lopdf_annots = flow::lopdf_geometry(bindings, &scratch.page, &plan.annots);
            // Cleared markup appearances are regenerated by the pre-save render.
            doc.touched.insert(page_index);
        }
        drop(scratch);
        // PDFium's content generator skips an inline image or a shading object in a stream it
        // rewrites (text, paths and forms are always written): re-parse, and refuse (the
        // mutation rolls back) rather than lose it.
        if !risky.is_empty() {
            let expected = count - job.ids.len() + added + usize::from(anchored);
            let reparsed = ScratchPage::open(doc, page_index)?.page.objects().len();
            if reparsed < expected {
                return Err(unwritable(page_index).with_detail(format!(
                    "unwritableContent: {} object(s) would be lost",
                    expected - reparsed
                )));
            }
        }
    }
    // A dry run drops `built` and the scratch page (never regenerated) here.
    Ok(Outcome {
        rect: new_rect,
        lines: rows,
        overflow: decision.overflow,
        shifted: if moving { decision.shift } else { 0.0 },
        moved_objects: if moving { plan.movable.len() as u32 } else { 0 },
        moved_annotations,
        room: plan.room,
        blocked: decision.blocked,
        fit_scale: (job.flow == ParagraphFlow::Fit).then(|| round2(factor)),
        past_bottom: decision.past_bottom,
        moved_band: if moving { plan.band } else { None },
        anchored,
        lopdf_annots,
        dy,
    })
}

/// The refusal for a page PDFium cannot rewrite without losing something.
fn unwritable(page_index: PageIndex) -> EngineError {
    not_editable(NotEditableReason::UnwritableContent)
        .with_page(page_index)
        .with_detail("unwritableContent")
}

/// Every top-level object of `page` as the flow sees it, and the indices of its image and
/// shading objects: only those can be lost for good when a content stream is rewritten, so a
/// page without one needs no re-parse after the write (`raw::page::rewrite_set`).
fn flow_items(
    bindings: &'static dyn PdfiumLibraryBindings,
    page: &PdfPage<'_>,
    own_ids: &HashSet<usize>,
) -> (Vec<flow::Item>, Vec<usize>) {
    let stamps: HashMap<usize, Option<StampRole>> =
        stamp::stamp_roles(bindings, page).into_iter().collect();
    let mut risky = Vec::new();
    let items = page
        .objects()
        .iter()
        .enumerate()
        .map(|(index, object)| {
            if matches!(
                object.object_type(),
                PdfPageObjectType::Image | PdfPageObjectType::Shading
            ) {
                risky.push(index);
            }
            let text = object.as_text_object();
            // An invisible OCR layer belongs to the scan under it, not to the text flow.
            let invisible = text.as_ref().is_some_and(|t| {
                matches!(
                    t.render_mode(),
                    PdfPageTextRenderMode::Invisible | PdfPageTextRenderMode::InvisibleClipping
                )
            });
            let kind = if own_ids.contains(&index) {
                flow::ItemKind::Paragraph
            } else if let Some(role) = stamps.get(&index) {
                match role {
                    Some(StampRole::Header | StampRole::Footer) => flow::ItemKind::Fixed,
                    _ => flow::ItemKind::Stamp,
                }
            } else if invisible {
                flow::ItemKind::Ignored
            } else if matches!(
                object.object_type(),
                PdfPageObjectType::Text
                    | PdfPageObjectType::Image
                    | PdfPageObjectType::Path
                    | PdfPageObjectType::Shading
                    | PdfPageObjectType::XObjectForm
            ) {
                flow::ItemKind::Content
            } else {
                flow::ItemKind::Ignored
            };
            // Upright text: its baseline and rendered size (the page's line pitch).
            let line = match (&text, object.matrix()) {
                (Some(t), Ok(m))
                    if !invisible && m.a() > 0.0 && m.d() > 0.0 && m.b().abs() <= 0.02 * m.a() =>
                {
                    let size = t.unscaled_font_size().value * m.d();
                    (size > 0.0).then_some(flow::TextLine {
                        baseline: m.f(),
                        size,
                    })
                }
                _ => None,
            };
            flow::Item {
                index,
                bounds: bounds_of(&object),
                kind,
                line,
            }
        })
        .collect();
    (items, risky)
}

/// The laid-out text as detached text objects, each with its bounds.
fn build_objects<'p>(
    document: &PdfDocument<'p>,
    token: PdfFontToken,
    size: f32,
    color: Rgb,
    render_mode: PdfPageTextRenderMode,
    placed: &[Placed],
    rot: Rot,
) -> Result<Vec<(PdfPageTextObject<'p>, Rect)>, EngineError> {
    let mut out = Vec::with_capacity(placed.len());
    for p in placed {
        let mut object = PdfPageTextObject::new(document, &p.text, token, PdfPoints::new(size))
            .ctx("create text object")?;
        object
            .set_fill_color(PdfColor::new(color[0], color[1], color[2], 255))
            .ctx("set_fill_color")?;
        if !matches!(
            render_mode,
            PdfPageTextRenderMode::Unknown | PdfPageTextRenderMode::FilledUnstroked
        ) {
            object.set_render_mode(render_mode).ctx("set_render_mode")?;
        }
        if rot.upright() {
            object
                .translate(PdfPoints::new(p.x), PdfPoints::new(p.baseline))
                .ctx("place text object")?;
        } else {
            // v0.3 (R5): laid out in the rotated frame, drawn rotated back onto the page.
            let (x, y) = rot.to_page((p.x, p.baseline));
            object
                .apply_matrix(PdfMatrix::new(rot.cos, rot.sin, -rot.sin, rot.cos, x, y))
                .ctx("place rotated text object")?;
        }
        let q = object.bounds().ctx("measure text object")?;
        let r = if rot.upright() {
            let b = q.to_rect();
            Rect::new(
                b.left().value,
                b.bottom().value,
                b.right().value,
                b.top().value,
            )
        } else {
            Rot::UPRIGHT.points_to_frame([
                (q.x1().value, q.y1().value),
                (q.x2().value, q.y2().value),
                (q.x3().value, q.y3().value),
                (q.x4().value, q.y4().value),
            ])
        };
        out.push((object, r));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metrics(space: Option<f32>) -> Metrics {
        // Every glyph 0.5 em wide.
        Metrics {
            widths: HashMap::new(),
            space,
        }
    }

    fn frame(align: ParagraphAlign) -> Frame {
        Frame {
            size: 10.0,
            box_l: 100.0,
            box_r: 200.0,
            indent: 0.0,
            align,
            first_baseline: 700.0,
            line_height: 12.0,
        }
    }

    #[test]
    fn breaks_at_spaces_and_by_char_on_overflow() {
        // 5 px per char, 3 px space, 100 px box.
        let m = metrics(Some(0.3));
        let (placed, lines) = layout(
            "aaaa bbbb cccc dddd eeee ffff",
            &m,
            frame(ParagraphAlign::Left),
        );
        // "aaaa bbbb cccc dddd" = 4*20 + 3*3 = 89 fits; + " eeee" = 112 does not.
        assert_eq!(lines, 2);
        assert_eq!(placed[0].text, "aaaa bbbb cccc dddd");
        assert_eq!(placed[1].text, "eeee ffff");
        assert_eq!(placed[1].baseline, 688.0);

        let long = "x".repeat(45); // 225 px: three chunks of <= 100 (+1.5 slack) px
        let (placed, lines) = layout(&long, &m, frame(ParagraphAlign::Left));
        assert_eq!(lines, 3);
        assert_eq!(placed.iter().map(|p| p.text.len()).sum::<usize>(), 45);
    }

    #[test]
    fn hard_breaks_align_and_justify() {
        let m = metrics(Some(0.3));
        let (placed, lines) = layout("ab\n\ncd", &m, frame(ParagraphAlign::Right));
        assert_eq!(lines, 3, "a blank line counts");
        assert_eq!(placed.len(), 2);
        assert_eq!(placed[0].x, 190.0);
        assert_eq!(placed[1].baseline, 676.0);

        let (placed, _) = layout("ab", &m, frame(ParagraphAlign::Center));
        assert_eq!(placed[0].x, 145.0);

        let (placed, lines) = layout(
            "aaaa bbbb cccc dddd eeee",
            &m,
            frame(ParagraphAlign::Justify),
        );
        assert_eq!(lines, 2);
        // First line: 4 words, one object each, last word flush right.
        let first: Vec<&Placed> = placed.iter().filter(|p| p.baseline == 700.0).collect();
        assert_eq!(first.len(), 4);
        assert!((first[3].x + 20.0 - 200.0).abs() < 1e-3);
        // Last line left-aligned.
        assert_eq!(placed.last().unwrap().x, 100.0);
    }

    /// A justified line whose next word did not fit is not stretched past the probe's split
    /// gap: it has to read back as one line (the verifier's Hangul re-probe).
    #[test]
    fn justified_gaps_are_capped_below_the_probe_split() {
        let m = metrics(Some(0.3));
        // "aa bb" (20 px) then an 18-char word (90 px) that does not fit: one 80 px gap
        // unless capped at 0.75 × 10.
        let (placed, lines) = layout(
            "aa bb cccccccccccccccccc dd",
            &m,
            frame(ParagraphAlign::Justify),
        );
        assert_eq!(lines, 3);
        let first: Vec<&Placed> = placed.iter().filter(|p| p.baseline == 700.0).collect();
        assert_eq!(first.len(), 2);
        let gap = first[1].x - (first[0].x + 10.0);
        assert!((gap - 7.5).abs() < 1e-3, "{gap}");
        assert!(gap < SPLIT_GAP * 10.0);
        // An ordinary line is still flush right.
        let (placed, _) = layout(
            "aaaa bbbb cccc dddd eeee",
            &m,
            frame(ParagraphAlign::Justify),
        );
        let last_on_first = placed.iter().rfind(|p| p.baseline == 700.0).unwrap();
        assert!((last_on_first.x + 20.0 - 200.0).abs() < 1e-3);
    }

    #[test]
    fn dehyphenates_lowercase_continuations_only() {
        let lines = vec![
            "an exam-".to_string(),
            "ple of".into(),
            "Semi-".into(),
            "Final".into(),
        ];
        assert_eq!(join_lines(&lines), "an example of Semi- Final");
    }
}
