//! The `get_text_layer` binary payload — `IPC_CONTRACT.md` §10.1.
//!
//! All little-endian; every section starts at a 4-byte aligned offset. ≈ 32 B per char
//! (~163 kB for a 5,087-char page). `src/viewer/text/TextLayer.ts` wraps the buffer in
//! `Uint32Array` / `Float32Array` views — no DOM node per character.

use crate::engine::text::layer::TextLayer;

pub const MAGIC: &[u8; 4] = b"STXL";
pub const VERSION: u16 = 1;
/// bit0 of `flags`: per-char object ids are present.
pub const FLAG_OBJECT_IDS: u16 = 1;

pub const HEADER_LEN: usize = 64;
pub const CHAR_LEN: usize = 32;
pub const WORD_LEN: usize = 12;
pub const LINE_LEN: usize = 36;

pub fn serialize(layer: &TextLayer) -> Vec<u8> {
    let text_bytes = layer.text.as_bytes();
    let text_pad = (4 - (text_bytes.len() % 4)) % 4;
    let mut out = Vec::with_capacity(
        HEADER_LEN
            + layer.chars.len() * CHAR_LEN
            + layer.words.len() * WORD_LEN
            + layer.lines.len() * LINE_LEN
            + 4
            + text_bytes.len()
            + text_pad,
    );

    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&VERSION.to_le_bytes());
    let flags = if layer.has_object_ids {
        FLAG_OBJECT_IDS
    } else {
        0
    };
    out.extend_from_slice(&flags.to_le_bytes());
    out.extend_from_slice(&(layer.page as u32).to_le_bytes());
    out.extend_from_slice(&(layer.chars.len() as u32).to_le_bytes());
    out.extend_from_slice(&(layer.words.len() as u32).to_le_bytes());
    out.extend_from_slice(&(layer.lines.len() as u32).to_le_bytes());
    for v in layer.matrix {
        out.extend_from_slice(&v.to_le_bytes());
    }
    for v in [layer.crop.l, layer.crop.b, layer.crop.r, layer.crop.t] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    debug_assert_eq!(out.len(), HEADER_LEN);

    for c in &layer.chars {
        out.extend_from_slice(&c.codepoint.to_le_bytes());
        for v in [c.loose.l, c.loose.b, c.loose.r, c.loose.t] {
            out.extend_from_slice(&v.to_le_bytes());
        }
        out.extend_from_slice(&c.baseline_y.to_le_bytes());
        let font_size_x10 = (c.font_size * 10.0).round().clamp(0.0, u16::MAX as f32) as u16;
        out.extend_from_slice(&font_size_x10.to_le_bytes());
        out.push(c.flags);
        out.push(0); // pad
        out.extend_from_slice(&c.object_id.to_le_bytes());
    }

    for w in &layer.words {
        out.extend_from_slice(&w.first_char.to_le_bytes());
        out.extend_from_slice(&w.char_count.to_le_bytes());
        out.extend_from_slice(&w.line_index.to_le_bytes());
    }

    for l in &layer.lines {
        out.extend_from_slice(&l.first_word.to_le_bytes());
        out.extend_from_slice(&l.word_count.to_le_bytes());
        out.extend_from_slice(&l.baseline_y.to_le_bytes());
        for v in [l.rect.l, l.rect.b, l.rect.r, l.rect.t] {
            out.extend_from_slice(&v.to_le_bytes());
        }
        out.extend_from_slice(&l.first_char.to_le_bytes());
        out.extend_from_slice(&l.char_count.to_le_bytes());
    }

    out.extend_from_slice(&(text_bytes.len() as u32).to_le_bytes());
    out.extend_from_slice(text_bytes);
    out.extend(std::iter::repeat_n(0u8, text_pad));
    out
}

/// Header fields, for tests and for the frontend's cross-check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    pub version: u16,
    pub flags: u16,
    pub page: u32,
    pub char_count: u32,
    pub word_count: u32,
    pub line_count: u32,
}

pub fn parse_header(buffer: &[u8]) -> Option<Header> {
    if buffer.len() < HEADER_LEN || &buffer[0..4] != MAGIC {
        return None;
    }
    let u16_at = |o: usize| u16::from_le_bytes([buffer[o], buffer[o + 1]]);
    let u32_at =
        |o: usize| u32::from_le_bytes([buffer[o], buffer[o + 1], buffer[o + 2], buffer[o + 3]]);
    Some(Header {
        version: u16_at(4),
        flags: u16_at(6),
        page: u32_at(8),
        char_count: u32_at(12),
        word_count: u32_at(16),
        line_count: u32_at(20),
    })
}

/// Byte offset of each section for a given header — mirrors the reader in `TextLayer.ts`.
pub fn section_offsets(header: &Header) -> (usize, usize, usize, usize) {
    let chars = HEADER_LEN;
    let words = chars + header.char_count as usize * CHAR_LEN;
    let lines = words + header.word_count as usize * WORD_LEN;
    let text = lines + header.line_count as usize * LINE_LEN;
    (chars, words, lines, text)
}

/// Reads the page text back out of a serialized buffer (used by the round-trip test).
pub fn parse_text(buffer: &[u8]) -> Option<String> {
    let header = parse_header(buffer)?;
    let (_, _, _, text_at) = section_offsets(&header);
    if buffer.len() < text_at + 4 {
        return None;
    }
    let len = u32::from_le_bytes([
        buffer[text_at],
        buffer[text_at + 1],
        buffer[text_at + 2],
        buffer[text_at + 3],
    ]) as usize;
    let start = text_at + 4;
    let end = start.checked_add(len)?;
    if buffer.len() < end {
        return None;
    }
    String::from_utf8(buffer[start..end].to_vec()).ok()
}
