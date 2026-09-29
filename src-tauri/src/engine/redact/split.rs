//! R2 (v0.3): word-level redaction — a mark over one word no longer takes its whole run.
//!
//! PDFium cannot delete characters from a text object, but it can **rewrite** one: `set_text`
//! re-encodes the characters through the font's own `/ToUnicode` map (the same reverse lookup
//! the in-place text edit uses), and `FPDFText_SetPositions` pins every glyph where it was.
//! After `set_text` PDFium's content generator writes the object as a `TJ` whose kerning comes
//! from those positions, so the survivors keep their exact places through a save — kerning,
//! `Tc` / `Tw` justification and all (measured: `tests/redact.rs`).
//!
//! For each text object the marks cover only partly:
//!
//! 1. its characters are read from a text page (`FPDFText_GetTextObject` attributes each
//!    glyph to the object that drew it) and cut into **runs** of unmarked characters;
//! 2. the object itself is rewritten to the first run (matrix origin moved to the run's first
//!    glyph, positions pinned) — it keeps its place in the content stream, so copy / search /
//!    read-aloud order is unchanged for that part;
//! 3. every further run is a copy of the original object — moved out of a throw-away second
//!    parse of the page, so its clip path, colours, alpha and marked content come along —
//!    inserted right after it and rewritten the same way;
//! 4. a fresh text page must read every run back with the same visible characters at the
//!    same tight boxes (±[`TOLERANCE`] pt) before the content is regenerated; the redaction
//!    checks the regenerated page once more (`mod.rs`).
//!
//! Runs are trimmed of leading and trailing whitespace, and the read-back compares visible
//! characters only: after the rewrite PDFium's text page may drop a space glyph from the
//! object or add a generated one between words, neither of which changes what is drawn.
//! A **ligature** (one glyph PDFium reports as several characters at one origin and box, e.g.
//! "fi") is written back as its presentation-form code point (U+FB01), so the font's own
//! ligature glyph is drawn again; writing "f" + "i" at one origin would overprint them.
//!
//! **Fallback** to today's whole-run removal, with the collateral named in the preview: a
//! Type3 font (no font program — PDFium's generator cannot write Type3 text at all), a font
//! without a usable `/ToUnicode` (the characters read back as raw codes), vertical or
//! out-of-baseline runs, text whose characters come from `/ActualText` rather than its
//! glyphs, and any run that fails its read-back — for a glyph the font cannot re-encode from
//! its Unicode value, for instance (a TeX hyphen reported as U+0002). `mod.rs` runs the same
//! rewrite and read-back on a throw-away parse in the preview, so a fallback is named there
//! before the confirm, not discovered after the destructive step.

use super::raw::{self, HandleKey, RawChar, TextPage};
use crate::engine::raw::object::Matrix;
use crate::ipc::types::Rect;
use crate::ipc::EngineError;
use pdfium_render::prelude::{
    PdfPage, PdfPageObjectsCommon, PdfiumLibraryBindings, FPDF_PAGEOBJECT,
};

/// Survivors must read back within this many points of where they were.
pub const TOLERANCE: f32 = 0.5;

/// Is `c` under a mark? The rule every redaction step shares: its tight box overlaps one.
pub fn marked(c: &RawChar, rects: &[Rect]) -> bool {
    !c.generated && rects.iter().any(|r| r.intersects(&c.tight))
}

/// One run of characters that stays.
#[derive(Debug, Clone, PartialEq)]
pub struct Run {
    pub chars: Vec<RawChar>,
}

impl Run {
    pub fn text(&self) -> String {
        self.chars
            .iter()
            .map(|c| char::from_u32(c.unicode).unwrap_or('\u{fffd}'))
            .collect()
    }

    /// The run as the glyphs that drew it: consecutive characters at one origin with one box
    /// are the pieces of one glyph (a ligature PDFium decomposed). `(text, origin)` each.
    pub fn glyphs(&self) -> Vec<(String, (f32, f32))> {
        let mut out: Vec<(String, (f32, f32), Rect)> = Vec::new();
        for c in &self.chars {
            let ch = char::from_u32(c.unicode).unwrap_or('\u{fffd}');
            match out.last_mut() {
                Some((text, origin, tight))
                    if same_point(*origin, c.origin) && *tight == c.tight && !is_space(c) =>
                {
                    text.push(ch)
                }
                _ => out.push((ch.to_string(), c.origin, c.tight)),
            }
        }
        out.into_iter().map(|(t, o, _)| (t, o)).collect()
    }

    /// What `set_text` gets (one character per glyph) and each glyph's origin; `None` when a
    /// multi-character glyph has no single code point to write it back with.
    pub fn encoded(&self) -> Option<(String, Vec<(f32, f32)>)> {
        let mut text = String::new();
        let mut origins = Vec::new();
        for (glyph, origin) in self.glyphs() {
            let mut it = glyph.chars();
            let ch = match (it.next(), it.next()) {
                // PDFium's text page reports a hyphen that ends a line as U+0002.
                (Some(LINE_END_HYPHEN), None) => '-',
                (Some(c), None) => c,
                _ => ligature(&glyph)?,
            };
            text.push(ch);
            origins.push(origin);
        }
        Some((text, origins))
    }
}

/// What PDFium's text page reports for a hyphen at the end of a line (`CPDF_TextPage`
/// marks it so extraction can join the word).
const LINE_END_HYPHEN: char = '\u{2}';

/// The same character for the read-back: a hyphen written back may or may not end its line
/// any more, so U+0002, U+002D and the soft hyphen U+00AD all read as one.
fn same_char(a: u32, b: u32) -> bool {
    let norm = |c: u32| if c == 0x02 || c == 0xAD { 0x2D } else { c };
    norm(a) == norm(b)
}

/// The presentation form (U+FB00–U+FB06) PDFium's text page decomposes back into `pieces`.
fn ligature(pieces: &str) -> Option<char> {
    Some(match pieces {
        "ff" => '\u{fb00}',
        "fi" => '\u{fb01}',
        "fl" => '\u{fb02}',
        "ffi" => '\u{fb03}',
        "ffl" => '\u{fb04}',
        "\u{17f}t" => '\u{fb05}',
        "st" => '\u{fb06}',
        _ => return None,
    })
}

fn same_point(a: (f32, f32), b: (f32, f32)) -> bool {
    (a.0 - b.0).abs() < 0.01 && (a.1 - b.1).abs() < 0.01
}

fn is_space(c: &RawChar) -> bool {
    char::from_u32(c.unicode).is_some_and(char::is_whitespace)
}

/// A character the read-back compares: drawn by the object and not whitespace.
fn visible(c: &RawChar) -> bool {
    !c.generated && !is_space(c)
}

/// The runs of `chars` (one object's characters, in order) that the marks leave. `None`
/// when no character is marked.
pub fn runs(chars: &[RawChar], rects: &[Rect]) -> Option<Vec<Run>> {
    let mut any = false;
    let mut out: Vec<Run> = Vec::new();
    let mut current: Vec<RawChar> = Vec::new();
    for c in chars.iter().filter(|c| !c.generated) {
        if marked(c, rects) {
            any = true;
            if !current.is_empty() {
                out.push(Run {
                    chars: std::mem::take(&mut current),
                });
            }
        } else {
            current.push(*c);
        }
    }
    if !current.is_empty() {
        out.push(Run { chars: current });
    }
    if !any {
        return None;
    }
    // Leading and trailing spaces carry nothing, and PDFium's text page does not always
    // attribute a rewritten object's leading space to it; a run of spaces is no run at all.
    for r in &mut out {
        let start = r
            .chars
            .iter()
            .position(|c| !is_space(c))
            .unwrap_or(r.chars.len());
        let end = r
            .chars
            .iter()
            .rposition(|c| !is_space(c))
            .map_or(start, |e| e + 1);
        r.chars = r.chars[start..end].to_vec();
    }
    out.retain(|r| !r.chars.is_empty());
    Some(out)
}

/// Static checks the preview can make without touching anything: can this object be split
/// at all? (The read-back decides the rest.)
pub fn splittable(
    bindings: &dyn PdfiumLibraryBindings,
    handle: FPDF_PAGEOBJECT,
    chars: &[RawChar],
) -> bool {
    if raw::object_type(bindings, handle) != raw::OBJ_TEXT {
        return false;
    }
    // Type3: no font program, and PDFium's generator drops Type3 text.
    if raw::font_data_len(bindings, handle) == 0 {
        return false;
    }
    // `/ActualText`: the text page reports the replacement text, not the glyphs, so neither
    // the runs nor their read-back describe what is drawn.
    if raw::marks(bindings, handle).into_iter().any(|m| {
        raw::mark_strings(bindings, m)
            .iter()
            .any(|(k, _)| k == "ActualText")
    }) {
        return false;
    }
    let text: String = chars
        .iter()
        .filter(|c| !c.generated)
        .map(|c| char::from_u32(c.unicode).unwrap_or('\u{fffd}'))
        .collect();
    if !usable_unicode(&text) {
        return false;
    }
    // Horizontal writing only: every glyph on the object's own baseline.
    let m = raw::matrix(bindings, handle);
    let Some(first) = chars.iter().find(|c| !c.generated) else {
        return false;
    };
    chars.iter().filter(|c| !c.generated).all(|c| {
        let (_, ty) = text_space_delta(m, first.origin, c.origin).unwrap_or((0.0, f32::MAX));
        ty.abs() * scale_of(m) <= TOLERANCE
    })
}

/// The same rule as `objects::has_usable_unicode`: raw char codes leaking through a missing
/// `/ToUnicode` show up as control characters or U+FFFD.
fn usable_unicode(text: &str) -> bool {
    let mut total = 0usize;
    let mut junk = 0usize;
    for ch in text.chars() {
        total += 1;
        let cp = ch as u32;
        if (cp < 0x20 && !matches!(ch, '\n' | '\r' | '\t' | '\u{2}')) || cp == 0xFFFD {
            junk += 1;
        }
    }
    total > 0 && junk * 4 < total
}

/// The page-space vector `from → to`, in the object's text space (the inverse of the linear
/// part of `m`).
fn text_space_delta(m: Matrix, from: (f32, f32), to: (f32, f32)) -> Option<(f32, f32)> {
    let det = m[0] * m[3] - m[1] * m[2];
    if det.abs() < 1e-9 {
        return None;
    }
    let (dx, dy) = (to.0 - from.0, to.1 - from.1);
    Some((
        (m[3] * dx - m[2] * dy) / det,
        (-m[1] * dx + m[0] * dy) / det,
    ))
}

/// Rough text-space → page scale of `m` (for tolerances).
fn scale_of(m: Matrix) -> f32 {
    ((m[0] * m[3] - m[1] * m[2]).abs()).sqrt().max(1e-6)
}

/// Rewrites the object at `index` of `page` to hold exactly `run`, at the run's own place.
pub fn rewrite(
    bindings: &dyn PdfiumLibraryBindings,
    page: &PdfPage<'_>,
    index: usize,
    run: &Run,
) -> Result<(), EngineError> {
    let handle = raw::object_at(bindings, page, index)?;
    let m = raw::matrix(bindings, handle);
    let (text, origins) = run
        .encoded()
        .ok_or_else(|| EngineError::invalid("a multi-character glyph cannot be re-encoded"))?;
    let first = *origins
        .first()
        .ok_or_else(|| EngineError::invalid("empty run"))?;
    let mut positions = Vec::with_capacity(origins.len().saturating_sub(1));
    for &origin in &origins[1..] {
        let (tx, _) = text_space_delta(m, first, origin)
            .ok_or_else(|| EngineError::invalid("singular text matrix"))?;
        positions.push(tx);
    }
    {
        let mut object = page
            .objects()
            .get(index)
            .map_err(|e| EngineError::pdfium("load text object", e))?;
        let Some(object) = object.as_text_object_mut() else {
            return Err(EngineError::invalid("not a text object"));
        };
        object
            .set_text(&text)
            .map_err(|e| EngineError::pdfium("set_text", e))?;
    }
    let handle = raw::object_at(bindings, page, index)?;
    raw::set_matrix(bindings, handle, [m[0], m[1], m[2], m[3], first.0, first.1])?;
    raw::set_positions(bindings, handle, &positions)
}

/// Does `page` (in memory) draw every visible character of `run` through the object
/// `handle`, in order, at the same tight boxes? `chars` = a fresh text page's characters.
/// Whitespace is not compared (module docs).
pub fn reads_back(chars: &[RawChar], handle: HandleKey, run: &Run) -> bool {
    let got: Vec<&RawChar> = chars
        .iter()
        .filter(|c| c.object == handle && visible(c))
        .collect();
    let want: Vec<&RawChar> = run.chars.iter().filter(|c| visible(c)).collect();
    got.len() == want.len()
        && got
            .iter()
            .zip(&want)
            .all(|(g, w)| same_char(g.unicode, w.unicode) && same_box(&g.tight, &w.tight))
}

/// Is every visible survivor of `runs` somewhere on the (regenerated, re-parsed) page, same
/// character at the same tight box?
pub fn survives(chars: &[RawChar], runs: &[Run]) -> bool {
    runs.iter()
        .flat_map(|r| &r.chars)
        .filter(|c| visible(c))
        .all(|want| {
            chars.iter().any(|c| {
                !c.generated
                    && same_char(c.unicode, want.unicode)
                    && same_box(&c.tight, &want.tight)
            })
        })
}

fn same_box(a: &Rect, b: &Rect) -> bool {
    (a.l - b.l).abs() <= TOLERANCE
        && (a.b - b.b).abs() <= TOLERANCE
        && (a.r - b.r).abs() <= TOLERANCE
        && (a.t - b.t).abs() <= TOLERANCE
}

/// Characters of `page`, freshly extracted from its in-memory objects.
pub fn page_chars(
    bindings: &dyn PdfiumLibraryBindings,
    page: &PdfPage<'_>,
) -> Result<Vec<RawChar>, EngineError> {
    Ok(TextPage::load(bindings, page)?.chars())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ch(c: char, x: f32) -> RawChar {
        RawChar {
            unicode: c as u32,
            generated: false,
            origin: (x, 0.0),
            tight: Rect::new(x, 0.0, x + 5.0, 7.0),
            object: 1,
        }
    }

    #[test]
    fn runs_split_at_the_marked_characters() {
        let chars: Vec<RawChar> = "Andreas Gal"
            .chars()
            .enumerate()
            .map(|(i, c)| ch(c, i as f32 * 6.0))
            .collect();
        // Mark "Gal" (x 48..66).
        let r = runs(&chars, &[Rect::new(47.0, -1.0, 70.0, 8.0)]).unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].text(), "Andreas", "the trailing space is trimmed");
        // Mark "dre": two runs, "An" and "as Gal".
        let r = runs(&chars, &[Rect::new(13.0, -1.0, 28.0, 8.0)]).unwrap();
        assert_eq!(
            r.iter().map(Run::text).collect::<Vec<_>>(),
            ["An", "as Gal"]
        );
        // Mark "rea" of "Andreas Gal wrote": the runs are trimmed at both ends.
        let spaced: Vec<RawChar> = "Andreas Gal wrote"
            .chars()
            .enumerate()
            .map(|(i, c)| ch(c, i as f32 * 6.0))
            .collect();
        let r = runs(&spaced, &[Rect::new(47.0, -1.0, 70.0, 8.0)]).unwrap();
        assert_eq!(
            r.iter().map(Run::text).collect::<Vec<_>>(),
            ["Andreas", "wrote"]
        );
        // Nothing marked.
        assert!(runs(&chars, &[Rect::new(100.0, 0.0, 110.0, 5.0)]).is_none());
        // Everything marked: no survivors.
        assert!(runs(&chars, &[Rect::new(-1.0, -1.0, 100.0, 8.0)])
            .unwrap()
            .is_empty());
    }

    /// A ligature: PDFium reports "fi" as two characters at one origin with one box.
    fn ligature_run() -> Run {
        let mut chars = vec![ch('d', 0.0), ch('i', 6.0)];
        let mut f = ch('f', 12.0);
        f.tight = Rect::new(12.0, 0.0, 16.0, 7.0);
        let mut i = ch('i', 12.0);
        i.tight = f.tight;
        chars.extend([f, i, ch('c', 18.0)]);
        Run { chars }
    }

    #[test]
    fn ligature_pieces_are_one_glyph() {
        let run = ligature_run();
        let glyphs: Vec<String> = run.glyphs().into_iter().map(|(t, _)| t).collect();
        assert_eq!(glyphs, ["d", "i", "fi", "c"]);
        let (text, origins) = run.encoded().unwrap();
        assert_eq!(text, "di\u{fb01}c");
        assert_eq!(origins.len(), 4);
        assert_eq!(origins[2], (12.0, 0.0));
        // Two pieces with no presentation form: not re-encodable.
        let mut odd = run.clone();
        odd.chars[3].unicode = 'q' as u32;
        assert!(odd.encoded().is_none());
    }

    #[test]
    fn read_back_ignores_whitespace_but_not_glyphs() {
        let want = Run {
            chars: "wrote this"
                .chars()
                .enumerate()
                .map(|(i, c)| ch(c, i as f32 * 6.0))
                .collect(),
        };
        // The space is gone and a generated one sits elsewhere: still the same run.
        let mut got: Vec<RawChar> = want
            .chars
            .iter()
            .filter(|c| c.unicode != 32)
            .copied()
            .collect();
        let mut gen = ch(' ', 31.0);
        gen.generated = true;
        gen.tight = Rect::new(31.0, 0.0, 31.0, 0.0);
        got.insert(5, gen);
        assert!(reads_back(&got, 1, &want));
        assert!(survives(&got, std::slice::from_ref(&want)));
        // A glyph that moved is not.
        got[0].tight = Rect::new(3.0, 0.0, 8.0, 7.0);
        assert!(!reads_back(&got, 1, &want));
        // Nor one that changed.
        got[0] = want.chars[0];
        got[1].unicode = 0xff;
        assert!(!reads_back(&got, 1, &want));
        assert!(!survives(&got, &[want]));
    }

    #[test]
    fn raw_codes_are_not_usable_text() {
        assert!(usable_unicode("Andreas Gal"));
        assert!(usable_unicode("홍길동 님"));
        assert!(!usable_unicode("\u{3}\u{13}\u{e}\u{4}"));
        assert!(!usable_unicode(""));
    }

    #[test]
    fn deltas_are_taken_in_text_space() {
        // Scaled ×11 (Tf 1 with an 11 pt text matrix): 22 pt on the page is 2 text units.
        let m = [11.0, 0.0, 0.0, 11.0, 100.0, 100.0];
        assert_eq!(
            text_space_delta(m, (100.0, 100.0), (122.0, 100.0)),
            Some((2.0, 0.0))
        );
        // Rotated 90°: up the page is along the baseline.
        let m = [0.0, 1.0, -1.0, 0.0, 0.0, 0.0];
        let (tx, ty) = text_space_delta(m, (0.0, 0.0), (0.0, 10.0)).unwrap();
        assert!((tx - 10.0).abs() < 1e-5 && ty.abs() < 1e-5);
    }
}
