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

use super::{check_font_size, check_generation, not_editable, object_at, relist, rgb_of};
use crate::engine::annot::ScratchPage;
use crate::engine::fonts;
use crate::engine::registry::{self, MutateOpts, OpenDoc};
use crate::engine::types::EngineState;
use crate::ipc::error::PdfiumResultExt;
use crate::ipc::types::{
    ChangeReason, DocGeneration, NotEditableReason, ObjectId, PageIndex, ParagraphAlign,
    ParagraphEdit, ParagraphEditResult, ParagraphProbe, Point, Rect, Rgb, TextEditStrategy,
};
use crate::ipc::{EngineError, ErrorCode};
use pdfium_render::prelude::*;
use std::collections::HashMap;

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

/// What kind of text object this is for the paragraph editor.
fn classify(index: usize, object: &PdfPageObject<'_>, text_page: &PdfPageText<'_>) -> Classified {
    let bounds = bounds_of(object);
    if object.object_type() == PdfPageObjectType::XObjectForm {
        let has_text = object
            .as_x_object_form_object()
            .map(|f| f.iter().any(|c| c.object_type() == PdfPageObjectType::Text))
            .unwrap_or(false);
        if !has_text {
            return Classified::Skip;
        }
        return Classified::Obstacle(Obstacle {
            index,
            bounds,
            reason: NotEditableReason::InsideXObject,
            text: String::new(),
            font: String::new(),
            size: 0.0,
            color: [0, 0, 0],
        });
    }
    let Some(t) = object.as_text_object() else {
        return Classified::Skip;
    };
    let text = text_page.for_object(t);
    if text.trim().is_empty() {
        return Classified::Skip;
    }
    let (a, b, d, e, f) = object
        .matrix()
        .map(|m| (m.a(), m.b(), m.d(), m.e(), m.f()))
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
    if !(size > 0.0) {
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
fn neighbour(lines: &[Line], para: &[usize], from: &Line, lo: f32, hi: f32, up: bool) -> Option<usize> {
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
    let first = text.split_whitespace().next().map_or(0, |w| w.chars().count());
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
        gap <= MAX_LEADING * size
            && leading.is_none_or(|l| (gap - l).abs() <= LEADING_TOL * l)
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
    let right_of_body = lines[..n - 1].iter().map(|l| l.right).fold(f32::MIN, f32::max);
    let align = if n >= 3
        && left_flush
        && lines[..n - 1]
            .iter()
            .all(|l| (l.right - right_of_body).abs() <= tol)
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
        if !(w > 0.0) {
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
    let has_space = text.split('\n').any(|seg| seg.split_whitespace().nth(1).is_some());
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
    let (runs, obstacles) = {
        let text_page = scratch.page.text().ctx("load text page")?;
        let mut runs = Vec::new();
        let mut obstacles = Vec::new();
        for (index, object) in scratch.page.objects().iter().enumerate() {
            match classify(index, &object, &text_page) {
                Classified::Run(r) => runs.push(r),
                Classified::Obstacle(o) => obstacles.push(o),
                Classified::Skip => {}
            }
        }
        (runs, obstacles)
    };
    let [px, py] = at;
    let lines = build_lines(&runs, true);
    let hit = lines
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
        .map(|(i, _)| i);

    let Some(hit) = hit else {
        let inside = |r: &Rect| px >= r.l - 1.0 && px <= r.r + 1.0 && py >= r.b - 1.0 && py <= r.t + 1.0;
        return Ok(obstacles
            .iter()
            .find(|o| inside(&o.bounds))
            .map(|o| obstacle_probe(o, generation)));
    };

    let para = grow(&runs, &lines, hit);
    let para_lines: Vec<&Line> = para.iter().map(|&i| &lines[i]).collect();
    let info = analyze(&runs, &para_lines);
    let dominant = &runs[info.dominant];

    let (strategy, substitute_font, reason) = if let Some(reason) = info.reason {
        (TextEditStrategy::Refused, None, Some(reason))
    } else {
        let token = font_token_of(&scratch.page, dominant.index)?;
        let covered = measure_font(
            &mut scratch.page,
            doc.pdf(),
            token,
            &info.text,
        )?
        .is_some();
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
        rect: info.rect,
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
    }))
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
            let needed = if current.is_empty() { ww } else { used + space + ww };
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
            let gap = if justify { (avail - sum) / gaps } else { space };
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
fn runs_for(page: &PdfPage<'_>, ids: &[ObjectId]) -> Result<Vec<Run>, EngineError> {
    let text_page = page.text().ctx("load text page")?;
    let mut runs = Vec::with_capacity(ids.len());
    for &id in ids {
        let object = object_at(page, id)?;
        match classify(id as usize, &object, &text_page) {
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

/// `edit_paragraph` — replaces the paragraph's objects with the new text, reflowed inside
/// the paragraph's box. One `registry::mutate` = one undo step (`undo.paragraphEdit`).
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
    if text.trim().is_empty() {
        return Err(EngineError::invalid(
            "the text is empty; delete the objects instead",
        ));
    }
    if let Some(size) = edit.font_size_pt {
        check_font_size(size)?;
    }
    if let Some(w) = edit.width {
        if !w.is_finite() || w < 1.0 {
            return Err(EngineError::invalid(format!("width {w} pt is out of range")));
        }
    }
    let mut ids = edit.object_ids.clone();
    ids.sort_unstable();
    ids.dedup();

    let (original, rect, lines) = registry::mutate(
        st,
        doc_id,
        MutateOpts::new("undo.paragraphEdit", ChangeReason::Edit).page(page_index),
        |doc| {
            let mut scratch = ScratchPage::open(doc, page_index)?;
            let runs = runs_for(&scratch.page, &ids)?;
            let mut lines = build_lines(&runs, false);
            lines.sort_by(|a, b| b.baseline.total_cmp(&a.baseline));
            let line_refs: Vec<&Line> = lines.iter().collect();
            let info = analyze(&runs, &line_refs);
            let dominant = runs[info.dominant].clone();

            // Font: the paragraph's own when it draws every glyph, else the substitute.
            let own = font_token_of(&scratch.page, dominant.index)?;
            let (token, metrics) =
                match measure_font(&mut scratch.page, doc.pdf(), own, &text)? {
                    Some(m) => (own, m),
                    None => {
                        let pick = fonts::pick(&text);
                        if !allow_font_substitution {
                            return Err(EngineError::new(
                                ErrorCode::FontCoverage,
                                format!(
                                    "the paragraph's font cannot draw this text; it would have \
                                     to be replaced with {}",
                                    pick.name()
                                ),
                            )
                            .with_page(page_index));
                        }
                        doc.adopt_hangul_token(&scratch.page);
                        let flat: String = text.replace('\n', " ");
                        let (token, _) = doc.hangul_token_for(&flat)?;
                        let m = measure_font(&mut scratch.page, doc.pdf(), token, &text)?
                            .ok_or_else(|| {
                                EngineError::new(
                                    ErrorCode::FontCoverage,
                                    format!("{} cannot draw this text either", pick.name()),
                                )
                                .with_page(page_index)
                            })?;
                        (token, m)
                    }
                };

            let size = edit.font_size_pt.unwrap_or(info.size);
            let scale = if info.size > 0.0 { size / info.size } else { 1.0 };
            let box_r = match edit.width {
                Some(w) => info.rect.l + w,
                None => info.rect.r,
            };
            let frame = Frame {
                size,
                box_l: info.body_left,
                box_r,
                indent: info.indent,
                align: edit.align.unwrap_or(info.align),
                first_baseline: info.first_baseline,
                line_height: info.line_height * scale,
            };
            let (placed, lines) = layout(&text, &metrics, frame);
            let color = edit.color.unwrap_or(dominant.color);
            let render_mode = dominant.render_mode;

            // Add first, remove afterwards: the new objects hold the font alive (an embedded
            // font is only referenced by its text objects), and the old indices stay valid.
            let document = doc.pdf();
            let mut rect: Option<Rect> = None;
            for p in &placed {
                let mut object =
                    PdfPageTextObject::new(document, &p.text, token, PdfPoints::new(size))
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
                object
                    .translate(PdfPoints::new(p.x), PdfPoints::new(p.baseline))
                    .ctx("place text object")?;
                let b = object.bounds().ctx("measure text object")?.to_rect();
                let r = Rect::new(b.left().value, b.bottom().value, b.right().value, b.top().value);
                rect = Some(rect.map_or(r, |acc| acc.union(&r)));
                scratch
                    .page
                    .objects_mut()
                    .add_text_object(object)
                    .ctx("add text object")?;
            }
            for &id in ids.iter().rev() {
                scratch
                    .page
                    .objects_mut()
                    .remove_object_at_index(id as usize)
                    .ctx(&format!("remove object {id}"))?;
            }
            scratch
                .page
                .regenerate_content()
                .ctx("regenerate page content")?;
            Ok((info.rect, rect.unwrap_or(Rect::ZERO), lines))
        },
    )?;
    let objects = relist(st, doc_id, page_index)?;
    Ok(ParagraphEditResult {
        objects,
        rect,
        lines,
        overflow_pt: (original.b - rect.b).max(0.0),
    })
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
        let (placed, lines) = layout("aaaa bbbb cccc dddd eeee ffff", &m, frame(ParagraphAlign::Left));
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

        let (placed, lines) =
            layout("aaaa bbbb cccc dddd eeee", &m, frame(ParagraphAlign::Justify));
        assert_eq!(lines, 2);
        // First line: 4 words, one object each, last word flush right.
        let first: Vec<&Placed> = placed.iter().filter(|p| p.baseline == 700.0).collect();
        assert_eq!(first.len(), 4);
        assert!((first[3].x + 20.0 - 200.0).abs() < 1e-3);
        // Last line left-aligned.
        assert_eq!(placed.last().unwrap().x, 100.0);
    }

    #[test]
    fn dehyphenates_lowercase_continuations_only() {
        let lines = vec!["an exam-".to_string(), "ple of".into(), "Semi-".into(), "Final".into()];
        assert_eq!(join_lines(&lines), "an example of Semi- Final");
    }
}
