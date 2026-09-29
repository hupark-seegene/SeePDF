//! v0.3 (pkg1, verification round 2): `Tc` / `Tw` written back after a redaction.
//!
//! PDFium's content generator never writes character or word spacing (`survivors.rs`), so a
//! redaction pins the glyphs of every run it had to put back with `TJ` adjustments instead.
//! The glyphs are where they were, but PDFium's text extraction reads a large adjustment as a
//! word gap: letter-spaced text (`2 Tc`) extracts as "A n d r e a s" and cannot be found.
//!
//! This `lopdf` pass over the serialised file ([`restore_spacing`]) finds, in every `TJ` the
//! generator wrote (`q … BT … TJ ET Q`), the adjustment that repeats between glyphs, and
//! writes it back as what it was — `Tc` (between every two glyphs) and `Tw` (after a
//! single-byte space) — leaving only the true kerning in the array. The page draws exactly
//! the same glyphs at exactly the same places (`mod.rs` checks that on the reopened file and
//! keeps the pinned version when anything differs).

use crate::ipc::types::PageIndex;
use crate::ipc::{EngineError, ErrorCode};
use lopdf::content::{Content, Operation};
use lopdf::{Document, Object, StringFormat};
use std::collections::HashMap;

/// A repeating adjustment smaller than this (thousandths of an em) is left alone.
const MIN_SPACING: f32 = 0.5;
/// An adjustment left smaller than this after the spacing is taken out is dropped.
const ZERO: f32 = 1e-3;

fn lopdf_error(what: &str, e: lopdf::Error) -> EngineError {
    EngineError::new(ErrorCode::Pdfium, format!("text spacing: {what}: {e}"))
}

fn number(o: &Object) -> Option<f32> {
    match o {
        Object::Integer(i) => Some(*i as f32),
        Object::Real(r) => Some(*r),
        _ => None,
    }
}

fn median(mut v: Vec<f32>) -> Option<f32> {
    if v.is_empty() {
        return None;
    }
    v.sort_by(f32::total_cmp);
    Some(v[v.len() / 2])
}

/// One `TJ` array drawn at font size `size` with `width`-byte codes: `(Tc, Tw, array)` with
/// the repeating spacing taken out of the adjustments, or `None` when there is none.
pub fn factor(array: &[Object], size: f32, width: usize) -> Option<(f32, f32, Vec<Object>)> {
    if width == 0 || size == 0.0 {
        return None;
    }
    // Every glyph with the adjustment before it.
    let mut lead = 0.0f32;
    let mut glyphs: Vec<(&[u8], f32)> = Vec::new();
    let mut pending = 0.0f32;
    for item in array {
        match item {
            Object::String(bytes, _) => {
                if bytes.len() % width != 0 {
                    return None;
                }
                for code in bytes.chunks(width) {
                    if glyphs.is_empty() {
                        lead = pending;
                    }
                    glyphs.push((code, if glyphs.is_empty() { 0.0 } else { pending }));
                    pending = 0.0;
                }
            }
            other => pending += number(other)?,
        }
    }
    let trail = pending;
    if glyphs.len() < 3 {
        return None;
    }
    let space = |code: &[u8]| width == 1 && code == b" ";
    let pairs = || glyphs.windows(2).map(|w| (w[0].0, w[1].1));
    let c = median(pairs().filter(|(p, _)| !space(p)).map(|(_, k)| k).collect()).unwrap_or(0.0);
    let w = median(
        pairs()
            .filter(|(p, _)| space(p))
            .map(|(_, k)| k - c)
            .collect(),
    )
    .unwrap_or(0.0);
    let c = if c.abs() < MIN_SPACING { 0.0 } else { c };
    let w = if w.abs() < MIN_SPACING { 0.0 } else { w };
    if c == 0.0 && w == 0.0 {
        return None;
    }

    let mut out: Vec<Object> = Vec::new();
    if lead.abs() >= ZERO {
        out.push(Object::Real(lead));
    }
    let mut run: Vec<u8> = Vec::new();
    for (i, (code, k)) in glyphs.iter().enumerate() {
        if i > 0 {
            let k = k - c - if space(glyphs[i - 1].0) { w } else { 0.0 };
            if k.abs() >= ZERO {
                out.push(Object::String(
                    std::mem::take(&mut run),
                    StringFormat::Hexadecimal,
                ));
                out.push(Object::Real(k));
            }
        }
        run.extend_from_slice(code);
    }
    out.push(Object::String(run, StringFormat::Hexadecimal));
    if trail.abs() >= ZERO {
        out.push(Object::Real(trail));
    }
    Some((-c * size / 1000.0, -w * size / 1000.0, out))
}

/// Bytes per character code of each font of a page: 1 for simple fonts, 2 for a Type0 font
/// with an Identity CMap, 0 (unknown: left alone) for any other Type0 font.
fn code_widths(doc: &Document, page_id: lopdf::ObjectId) -> HashMap<Vec<u8>, usize> {
    let Ok(fonts) = doc.get_page_fonts(page_id) else {
        return HashMap::new();
    };
    fonts
        .into_iter()
        .map(|(name, font)| {
            let type0 = font.get(b"Subtype").and_then(Object::as_name).ok() == Some(b"Type0");
            let identity = matches!(
                font.get(b"Encoding").and_then(Object::as_name).ok(),
                Some(b"Identity-H" | b"Identity-V")
            );
            let width = match (type0, identity) {
                (false, _) => 1,
                (true, true) => 2,
                (true, false) => 0,
            };
            (name, width)
        })
        .collect()
}

/// Rewrites one content stream's operations; `true` when anything changed.
fn respace_ops(ops: &mut Vec<Operation>, widths: &HashMap<Vec<u8>, usize>) -> bool {
    let mut out: Vec<Operation> = Vec::with_capacity(ops.len());
    let mut changed = false;
    let (mut font, mut size) = (Vec::new(), 0.0f32);
    for (i, op) in ops.iter().enumerate() {
        if op.operator == "Tf" && op.operands.len() == 2 {
            font = op.operands[0]
                .as_name()
                .map(<[u8]>::to_vec)
                .unwrap_or_default();
            size = number(&op.operands[1]).unwrap_or(0.0);
        }
        // Only the generator's own shape: the TJ is the last show of its BT, and a `Q` right
        // after the `ET` restores the text state for whatever follows.
        let confined = op.operator == "TJ"
            && ops.get(i + 1).is_some_and(|o| o.operator == "ET")
            && ops.get(i + 2).is_some_and(|o| o.operator == "Q");
        let factored = confined
            .then(|| op.operands.first().and_then(|a| a.as_array().ok()))
            .flatten()
            .and_then(|array| factor(array, size, widths.get(&font).copied().unwrap_or(0)));
        match factored {
            Some((tc, tw, array)) => {
                if tc != 0.0 {
                    out.push(Operation::new("Tc", vec![Object::Real(tc)]));
                }
                if tw != 0.0 {
                    out.push(Operation::new("Tw", vec![Object::Real(tw)]));
                }
                out.push(Operation::new("TJ", vec![Object::Array(array)]));
                changed = true;
            }
            None => out.push(op.clone()),
        }
    }
    if changed {
        *ops = out;
    }
    changed
}

/// Writes `Tc` / `Tw` back into the `TJ`s of `pages` (module docs). `None` when nothing
/// changed (or on an encrypted input: an encrypted document comes through
/// `security::lopdf_pass`, decrypted).
pub fn restore_spacing(bytes: &[u8], pages: &[PageIndex]) -> Result<Option<Vec<u8>>, EngineError> {
    let mut doc = Document::load_mem(bytes).map_err(|e| lopdf_error("parse", e))?;
    if doc.is_encrypted() {
        return Ok(None);
    }
    let page_ids = doc.get_pages();
    let mut changed = false;
    for &page in pages {
        let Some(&page_id) = page_ids.get(&(page as u32 + 1)) else {
            continue;
        };
        let widths = code_widths(&doc, page_id);
        for id in doc.get_page_contents(page_id) {
            let Ok(stream) = doc.get_object(id).and_then(Object::as_stream) else {
                continue;
            };
            let Ok(data) = stream.decompressed_content() else {
                continue;
            };
            // Only streams that parse whole, and none with an inline image (lopdf does not
            // write those back byte for byte).
            let Ok(mut content) = Content::decode_strict(&data) else {
                continue;
            };
            if content.operations.iter().any(|o| o.operator == "BI") {
                continue;
            }
            if !respace_ops(&mut content.operations, &widths) {
                continue;
            }
            let encoded = content
                .encode()
                .map_err(|e| lopdf_error("encode content", e))?;
            let stream = doc
                .get_object_mut(id)
                .and_then(Object::as_stream_mut)
                .map_err(|e| lopdf_error("content stream", e))?;
            stream.dict.remove(b"DecodeParms");
            stream.set_plain_content(encoded);
            let _ = stream.compress();
            changed = true;
        }
    }
    if !changed {
        return Ok(None);
    }
    let mut out = Vec::new();
    doc.save_to(&mut out).map_err(|e| {
        EngineError::new(
            ErrorCode::Pdfium,
            format!("text spacing: write the document: {e}"),
        )
    })?;
    Ok(Some(out))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(s: &str) -> Object {
        Object::String(s.as_bytes().to_vec(), StringFormat::Hexadecimal)
    }

    /// "ab c" at 10 pt with `2 Tc 5 Tw`, as the generator writes it once the spacing is gone:
    /// −200 between every two glyphs, −700 after the space.
    #[test]
    fn repeating_adjustments_become_tc_and_tw() {
        let array = vec![
            hex("a"),
            Object::Real(-200.0),
            hex("b"),
            Object::Real(-200.0),
            hex(" "),
            Object::Real(-700.0),
            hex("c"),
        ];
        let (tc, tw, out) = factor(&array, 10.0, 1).expect("factored");
        assert!(
            (tc - 2.0).abs() < 1e-4 && (tw - 5.0).abs() < 1e-4,
            "{tc} {tw}"
        );
        assert_eq!(out, vec![hex("ab c")]);
    }

    #[test]
    fn true_kerning_stays_in_the_array() {
        // Letter-spaced by 30 units, with a real −50 kern between "b" and "c".
        let array = vec![
            hex("a"),
            Object::Real(-30.0),
            hex("b"),
            Object::Real(-80.0),
            hex("c"),
            Object::Real(-30.0),
            hex("d"),
        ];
        let (tc, tw, out) = factor(&array, 10.0, 1).expect("factored");
        assert!((tc - 0.3).abs() < 1e-4 && tw == 0.0);
        assert_eq!(out, vec![hex("ab"), Object::Real(-50.0), hex("cd")]);
    }

    #[test]
    fn plain_kerning_is_left_alone() {
        let array = vec![hex("Wa"), Object::Real(80.0), hex("ter")];
        assert!(factor(&array, 10.0, 1).is_none());
        // Two-byte codes: an odd string is not ours to split.
        assert!(factor(&[hex("abc"), Object::Real(-200.0), hex("de")], 10.0, 2).is_none());
    }

    #[test]
    fn only_the_generators_confined_tj_is_rewritten() {
        let widths: HashMap<Vec<u8>, usize> = [(b"F1".to_vec(), 1)].into();
        let array = Object::Array(vec![
            hex("a"),
            Object::Real(-200.0),
            hex("b"),
            Object::Real(-200.0),
            hex("c"),
        ]);
        let tf = Operation::new("Tf", vec![Object::Name(b"F1".to_vec()), Object::Real(10.0)]);
        let tj = Operation::new("TJ", vec![array]);
        let mut ops = vec![
            Operation::new("q", vec![]),
            Operation::new("BT", vec![]),
            tf.clone(),
            tj.clone(),
            Operation::new("ET", vec![]),
            Operation::new("Q", vec![]),
        ];
        assert!(respace_ops(&mut ops, &widths));
        let names: Vec<&str> = ops.iter().map(|o| o.operator.as_str()).collect();
        assert_eq!(names, ["q", "BT", "Tf", "Tc", "TJ", "ET", "Q"]);
        // No `Q` after the `ET`: the spacing would leak into what follows.
        let mut open = vec![
            Operation::new("BT", vec![]),
            tf,
            tj,
            Operation::new("ET", vec![]),
        ];
        assert!(!respace_ops(&mut open, &widths));
    }
}
