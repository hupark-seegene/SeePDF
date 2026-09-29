//! v0.3 (pkg1, verification round 2): **everything the marks do not cover stays where it was.**
//!
//! Two things PDFium's content generator does to a page it regenerates, found on real files:
//!
//! * **Text state is not written.** `CPDF_PageContentGenerator` writes each text object as
//!   `BT <Tm> /F <size> Tf <mode> Tr [...] TJ ET` and computes the `TJ` adjustments from the
//!   glyph positions *minus* the object's character and word spacing (`Tc`, `Tw`) — but never
//!   writes `Tc` or `Tw`. Every run of justified text in a regenerated stream therefore moves
//!   (6 pt on `TAMReview.pdf` p.0), untouched runs included, and a split run no longer reads
//!   back where it was.
//! * **Streams that break mid-object** (`contents.rs`) leave a dangling fragment that breaks
//!   the parse of the rest of the page.
//!
//! So the apply (`mod.rs`):
//!
//! 1. takes a [`staged`] snapshot — every top-level text object, in drawing order, with the
//!    characters it draws in memory (where `Tc` / `Tw` still apply) — right before it
//!    regenerates;
//! 2. regenerates, re-parses and [`repair`]s: each text object that no longer draws its
//!    characters at the staged boxes is rewritten with the staged positions pinned (the
//!    re-parsed object carries no `Tc` / `Tw` any more, so the generator's adjustments now
//!    come out exact), then regenerates once more;
//! 3. checks the page-wide post-condition with [`missing`]: every visible character that was
//!    on the page before, is not under a mark and is not drawn by an object removed whole
//!    (collateral the preview named), must still be extractable at its box (±[`TOLERANCE`]
//!    pt). Anything else is `verifyFailed`, and the whole batch rolls back.

use super::raw::{self, HandleKey, RawChar, TextPage};
use super::split::{self, Run, TOLERANCE};
use crate::ipc::types::Rect;
use crate::ipc::{EngineError, ErrorCode};
use pdfium_render::prelude::{PdfPage, PdfiumLibraryBindings};
use std::collections::HashMap;

/// How far ahead [`repair`] looks for the re-parsed twin of a staged object (PDFium drops a
/// text object it cannot write — a Type3 run — so the two lists need not line up 1:1).
const LOOKAHEAD: usize = 16;

/// Every top-level text object of `page`, in drawing order, with the characters it draws
/// (generated ones left out). Read from the page's in-memory objects.
pub fn staged(
    bindings: &dyn PdfiumLibraryBindings,
    page: &PdfPage<'_>,
) -> Result<Vec<Vec<RawChar>>, EngineError> {
    Ok(text_objects(bindings, page)?
        .into_iter()
        .map(|(_, chars)| chars)
        .collect())
}

/// The visible characters of `chars`, as a string (the key [`repair`] pairs objects by).
fn spelled(chars: &[RawChar]) -> String {
    chars
        .iter()
        .filter(|c| split::visible(c))
        .map(|c| char::from_u32(c.unicode).unwrap_or('\u{fffd}'))
        .collect()
}

/// Does `now` draw the visible characters of `was` at other boxes?
fn drifted(was: &[RawChar], now: &[RawChar]) -> bool {
    was.iter()
        .filter(|c| split::visible(c))
        .zip(now.iter().filter(|c| split::visible(c)))
        .any(|(a, b)| !split::same_box(&a.tight, &b.tight))
}

/// Pairs each staged object with its re-parsed twin (`post`: `(index, chars)` of every text
/// object of the regenerated page, in order): same visible text, in the same order.
fn pair(staged: &[Vec<RawChar>], post: &[(usize, Vec<RawChar>)]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut next = 0;
    for (s, was) in staged.iter().enumerate() {
        let key = spelled(was);
        if key.is_empty() {
            continue;
        }
        let end = (next + LOOKAHEAD).min(post.len());
        if let Some(k) = (next..end).find(|&k| spelled(&post[k].1) == key) {
            out.push((s, k));
            next = k + 1;
        }
    }
    out
}

/// How a drifted object is put back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Fix {
    /// Its own char codes, glyph positions pinned ([`split::pin`]).
    Pin,
    /// Re-encoded from Unicode, positions pinned ([`split::rewrite`]) — for an object whose
    /// glyph count PDFium sees differently.
    Rewrite,
}

/// The text objects of `page`, in drawing order, as `(index, characters)`.
fn text_objects(
    bindings: &dyn PdfiumLibraryBindings,
    page: &PdfPage<'_>,
) -> Result<Vec<(usize, Vec<RawChar>)>, EngineError> {
    let chars = TextPage::load(bindings, page)?.chars();
    let mut by_object: HashMap<HandleKey, Vec<RawChar>> = HashMap::new();
    for c in chars.into_iter().filter(|c| !c.generated) {
        by_object.entry(c.object).or_default().push(c);
    }
    let mut out = Vec::new();
    for i in 0..raw::object_count(bindings, page) {
        let h = raw::object_at(bindings, page, i)?;
        if raw::object_type(bindings, h) == raw::OBJ_TEXT {
            out.push((i, by_object.remove(&raw::key(h)).unwrap_or_default()));
        }
    }
    Ok(out)
}

/// Applies `fix` to the object at `index`; `false` when PDFium refused.
fn put_back(
    bindings: &dyn PdfiumLibraryBindings,
    page: &PdfPage<'_>,
    index: usize,
    run: &Run,
    fix: Fix,
) -> bool {
    match fix {
        Fix::Pin => split::pin(bindings, page, index, run).is_ok(),
        Fix::Rewrite => split::rewrite(bindings, page, index, run).is_ok(),
    }
}

/// Does every object of `plan` read back its run on `page`? Returns those that do not.
fn unread(
    bindings: &dyn PdfiumLibraryBindings,
    page: &PdfPage<'_>,
    plan: &[(usize, Run, Fix)],
) -> Result<Vec<usize>, EngineError> {
    let chars = split::page_chars(bindings, page)?;
    let mut out = Vec::new();
    for (index, run, _) in plan {
        let h = raw::key(raw::object_at(bindings, page, *index)?);
        if !split::reads_back(&chars, h, run) {
            out.push(*index);
        }
    }
    Ok(out)
}

/// Puts back every text object of `page` (a fresh parse of the regenerated content, `Manual`
/// regeneration) that no longer draws its [`staged`] characters where they were, and returns
/// how many it changed; the caller regenerates when that is not zero.
///
/// `trial` is a second, throw-away parse of the same content: each fix is tried there first
/// — its own char codes pinned, else (fonts `split::splittable` accepts) re-encoded — and
/// only the fixes that read back in place are made on `page`. An object no fix puts back is
/// left as it is; [`missing`] then names what moved.
pub fn repair(
    bindings: &dyn PdfiumLibraryBindings,
    trial: &PdfPage<'_>,
    page: &PdfPage<'_>,
    staged: &[Vec<RawChar>],
) -> Result<usize, EngineError> {
    let post = text_objects(bindings, trial)?;
    let mut plan: Vec<(usize, Run, Fix)> = Vec::new();
    for (s, k) in pair(staged, &post) {
        let (index, now) = &post[k];
        if !drifted(&staged[s], now) {
            continue;
        }
        let run = Run {
            chars: staged[s].clone(),
        };
        let reencodable = run.encoded().is_some()
            && split::splittable(bindings, raw::object_at(bindings, trial, *index)?, now);
        if put_back(bindings, trial, *index, &run, Fix::Pin) {
            plan.push((*index, run, Fix::Pin));
        } else if reencodable && put_back(bindings, trial, *index, &run, Fix::Rewrite) {
            plan.push((*index, run, Fix::Rewrite));
        }
    }
    if plan.is_empty() {
        return Ok(0);
    }
    // Pinned glyphs that still do not read back get one more chance, re-encoded.
    let failed = unread(bindings, trial, &plan)?;
    if !failed.is_empty() {
        let post: HashMap<usize, Vec<RawChar>> = post.into_iter().collect();
        for (index, run, fix) in plan.iter_mut().filter(|(i, _, _)| failed.contains(i)) {
            let now = post.get(index).map(Vec::as_slice).unwrap_or_default();
            let reencodable = *fix == Fix::Pin
                && run.encoded().is_some()
                && split::splittable(bindings, raw::object_at(bindings, trial, *index)?, now);
            if reencodable && put_back(bindings, trial, *index, run, Fix::Rewrite) {
                *fix = Fix::Rewrite;
            }
        }
        let still = unread(bindings, trial, &plan)?;
        plan.retain(|(i, _, _)| !still.contains(i));
    }

    for (index, run, fix) in &plan {
        if !put_back(bindings, page, *index, run, *fix) {
            return Err(put_back_failed(run));
        }
    }
    if let Some(index) = unread(bindings, page, &plan)?.first() {
        let run = &plan.iter().find(|(i, _, _)| i == index).expect("planned").1;
        return Err(put_back_failed(run));
    }
    Ok(plan.len())
}

fn put_back_failed(run: &Run) -> EngineError {
    EngineError::new(
        ErrorCode::VerifyFailed,
        format!(
            "{:?} could not be put back where it was after the page was rewritten; the page \
             was restored",
            run.text().trim()
        ),
    )
}

/// The characters of `expected` that `after` (a fresh parse) no longer draws: same
/// character (a line-end hyphen and "-" are one) at the same tight box, ±[`TOLERANCE`] pt.
/// Whitespace and generated characters are not compared.
pub fn missing<'a>(expected: &'a [RawChar], after: &[RawChar]) -> Vec<&'a RawChar> {
    let norm = |c: u32| if c == 0x02 || c == 0xAD { 0x2D } else { c };
    let mut at: HashMap<u32, Vec<Rect>> = HashMap::new();
    for c in after.iter().filter(|c| !c.generated) {
        at.entry(norm(c.unicode)).or_default().push(c.tight);
    }
    expected
        .iter()
        .filter(|c| split::visible(c))
        .filter(|want| {
            !at.get(&norm(want.unicode))
                .is_some_and(|boxes| boxes.iter().any(|b| split::same_box(b, &want.tight)))
        })
        .collect()
}

/// A short sample of `lost` for an error message: the characters in page order, runs of
/// neighbours joined, at most `max` characters.
pub fn sample(lost: &[&RawChar], max: usize) -> String {
    let mut out = String::new();
    let mut last: Option<&RawChar> = None;
    for c in lost.iter().take(max) {
        if let Some(prev) = last {
            let gap = (c.tight.l - prev.tight.r).abs() > prev.tight.height().max(1.0)
                || (c.origin.1 - prev.origin.1).abs() > TOLERANCE;
            if gap {
                out.push_str(" … ");
            }
        }
        out.push(char::from_u32(c.unicode).unwrap_or('\u{fffd}'));
        last = Some(c);
    }
    if lost.len() > max {
        out.push_str(" …");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ch(c: char, x: f32, object: HandleKey) -> RawChar {
        RawChar {
            unicode: c as u32,
            generated: false,
            origin: (x, 0.0),
            tight: Rect::new(x, 0.0, x + 5.0, 7.0),
            object,
        }
    }

    fn word(s: &str, x: f32, object: HandleKey) -> Vec<RawChar> {
        s.chars()
            .enumerate()
            .map(|(i, c)| ch(c, x + i as f32 * 6.0, object))
            .collect()
    }

    #[test]
    fn missing_is_by_character_and_box() {
        let expected = word("ab c", 0.0, 1);
        // Same characters, same boxes, other object, the space gone: nothing missing.
        let mut after = word("abc", 0.0, 9);
        after[2] = ch('c', 18.0, 9);
        assert!(missing(&expected, &after).is_empty());
        // "c" moved by a point: missing.
        after[2] = ch('c', 19.0, 9);
        let lost = missing(&expected, &after);
        assert_eq!(lost.len(), 1);
        assert_eq!(lost[0].unicode, 'c' as u32);
        // A line-end hyphen reads back as "-".
        let hyphen = [ch('\u{2}', 0.0, 1)];
        assert!(missing(&hyphen, &[ch('-', 0.0, 2)]).is_empty());
    }

    #[test]
    fn staged_objects_pair_by_text_in_order() {
        let staged = vec![
            word("one", 0.0, 1),
            Vec::new(),
            word("two", 0.0, 2),
            word("x", 0.0, 3),
        ];
        // The re-parse lost "one" (a Type3 run PDFium does not write) and kept the rest.
        let post = vec![(4, word("two", 1.0, 7)), (5, word("x", 0.0, 8))];
        assert_eq!(pair(&staged, &post), vec![(2, 0), (3, 1)]);
        assert!(drifted(&staged[2], &post[0].1));
        assert!(!drifted(&staged[3], &post[1].1));
    }

    #[test]
    fn a_sample_breaks_between_distant_characters() {
        let chars = [ch('a', 0.0, 1), ch('b', 6.0, 1), ch('z', 100.0, 1)];
        let refs: Vec<&RawChar> = chars.iter().collect();
        assert_eq!(sample(&refs, 10), "ab … z");
        assert_eq!(sample(&refs, 2), "ab …");
    }
}
