//! Per-page text layer: chars with geometry, words, lines — `ARCHITECTURE.md` §5.
//!
//! Built from `page.text()?.chars()` in ~5.6 ms/page (text spike §2) and cached per
//! generation. The grouping rules are the spike's, verified to give 89 lines / 754 words on
//! `tracemonkey.pdf` page 1.
//!
//! Two caches live side by side:
//! * [`PageText`] — just the code points (one FFI call per char, ~0.5 ms/page). Kept for the
//!   document's lifetime (≈ 5 kB/page) so repeated searches are pure memory.
//! * [`TextLayer`] — the full geometry, in a 32 MiB LRU.

use crate::engine::registry::OpenDoc;
use crate::ipc::error::PdfiumResultExt;
use crate::ipc::types::{PageGeom, Rect};
use crate::ipc::EngineError;
use pdfium_render::prelude::*;
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

/// 32 MiB of `TextLayer`s.
pub const DEFAULT_LAYER_BUDGET: usize = 32 * 1024 * 1024;

pub const FLAG_GENERATED: u8 = 1;
pub const FLAG_HYPHEN: u8 = 2;
pub const FLAG_SPACE: u8 = 4;

/// One extracted character.
#[derive(Debug, Clone, Copy)]
pub struct CharEntry {
    pub codepoint: u32,
    /// Full advance × ascent/descent — what selection highlights use.
    pub loose: Rect,
    /// Glyph ink box — what redaction hit-tests use.
    pub tight: Rect,
    pub baseline_y: f32,
    pub font_size: f32,
    pub flags: u8,
    /// Index of the owning text object, or `u32::MAX` when unknown.
    pub object_id: u32,
}

impl CharEntry {
    pub fn is_generated(&self) -> bool {
        self.flags & FLAG_GENERATED != 0
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Word {
    pub first_char: u32,
    pub char_count: u32,
    pub line_index: u32,
    pub rect: Rect,
    pub baseline_y: f32,
    pub font_size: f32,
}

#[derive(Debug, Clone, Copy)]
pub struct Line {
    pub first_word: u32,
    pub word_count: u32,
    pub baseline_y: f32,
    pub rect: Rect,
    pub first_char: u32,
    pub char_count: u32,
}

#[derive(Debug)]
pub struct TextLayer {
    pub page: u16,
    pub chars: Vec<CharEntry>,
    pub words: Vec<Word>,
    pub lines: Vec<Line>,
    /// Page text, built from `chars` so code-point indices line up exactly. Includes
    /// pdfium's generated `\r\n`.
    pub text: String,
    /// Page user space → device px at `s = 1` (crop box + `/Rotate`).
    pub matrix: [f32; 6],
    pub crop: Rect,
    pub has_object_ids: bool,
}

impl TextLayer {
    /// Rough in-memory size, for the LRU budget.
    pub fn bytes(&self) -> usize {
        self.chars.len() * std::mem::size_of::<CharEntry>()
            + self.words.len() * std::mem::size_of::<Word>()
            + self.lines.len() * std::mem::size_of::<Line>()
            + self.text.len()
            + 128
    }

    /// One rect per line for the code-point range `start .. start + len`.
    pub fn range_rects(&self, start: u32, len: u32) -> Vec<Rect> {
        let end = start.saturating_add(len);
        let mut out: Vec<Rect> = Vec::new();
        let mut current: Option<(f32, Rect)> = None;
        for i in start..end.min(self.chars.len() as u32) {
            let c = &self.chars[i as usize];
            if c.is_generated() || c.loose.width() <= 0.0 {
                continue;
            }
            let em = c.font_size.max(1.0);
            match &mut current {
                Some((baseline, rect)) if (*baseline - c.baseline_y).abs() <= 0.5 * em => {
                    *rect = rect.union(&c.loose);
                }
                _ => {
                    if let Some((_, rect)) = current.take() {
                        out.push(rect);
                    }
                    current = Some((c.baseline_y, c.loose));
                }
            }
        }
        if let Some((_, rect)) = current {
            out.push(rect);
        }
        out
    }
}

/// The cheap per-page cache used by search.
#[derive(Debug)]
pub struct PageText {
    pub chars: Vec<char>,
    /// `chars` case-folded 1:1, so a match index is a char index.
    pub folded: Vec<char>,
    pub text: String,
}

impl PageText {
    fn from_chars(chars: Vec<char>) -> Self {
        let folded = chars.iter().map(|c| fold(*c)).collect();
        let text = chars.iter().collect();
        Self {
            chars,
            folded,
            text,
        }
    }
}

/// Single-code-point case folding, so folding never changes indices.
pub fn fold(c: char) -> char {
    c.to_lowercase().next().unwrap_or(c)
}

#[derive(Default)]
pub struct TextCache {
    layers: HashMap<u16, Arc<TextLayer>>,
    order: VecDeque<u16>,
    bytes: usize,
    texts: HashMap<u16, Arc<PageText>>,
}

impl TextCache {
    pub fn layer(&self, page: u16) -> Option<Arc<TextLayer>> {
        self.layers.get(&page).cloned()
    }

    pub fn text(&self, page: u16) -> Option<Arc<PageText>> {
        self.texts.get(&page).cloned()
    }

    pub fn put_layer(&mut self, page: u16, layer: Arc<TextLayer>) {
        let size = layer.bytes();
        if let Some(old) = self.layers.insert(page, layer) {
            self.bytes = self.bytes.saturating_sub(old.bytes());
            if let Some(pos) = self.order.iter().position(|&p| p == page) {
                self.order.remove(pos);
            }
        }
        self.bytes += size;
        self.order.push_back(page);
        while self.bytes > DEFAULT_LAYER_BUDGET {
            let Some(victim) = self.order.pop_front() else {
                break;
            };
            if let Some(old) = self.layers.remove(&victim) {
                self.bytes = self.bytes.saturating_sub(old.bytes());
            }
        }
    }

    pub fn put_text(&mut self, page: u16, text: Arc<PageText>) {
        self.texts.insert(page, text);
    }

    pub fn invalidate_page(&mut self, page: u16) {
        if let Some(old) = self.layers.remove(&page) {
            self.bytes = self.bytes.saturating_sub(old.bytes());
        }
        if let Some(pos) = self.order.iter().position(|&p| p == page) {
            self.order.remove(pos);
        }
        self.texts.remove(&page);
    }

    pub fn clear(&mut self) {
        self.layers.clear();
        self.order.clear();
        self.texts.clear();
        self.bytes = 0;
    }

    pub fn bytes(&self) -> usize {
        self.bytes
    }
}

/// Builds (or returns the cached) full text layer for one page.
pub fn layer(doc: &mut OpenDoc<'_>, page_index: u16) -> Result<Arc<TextLayer>, EngineError> {
    if let Some(cached) = doc.text.layer(page_index) {
        return Ok(cached);
    }
    let geom: PageGeom = doc.geom(page_index)?.clone();
    let built = {
        let page = doc.page(page_index)?;
        build_layer(page, page_index, &geom)?
    };
    let built = Arc::new(built);
    doc.text.put_layer(page_index, built.clone());
    let text = Arc::new(PageText::from_chars(
        built
            .chars
            .iter()
            .map(|c| char::from_u32(c.codepoint).unwrap_or('\u{fffd}'))
            .collect(),
    ));
    doc.text.put_text(page_index, text);
    Ok(built)
}

/// Code points only — the cheap path search uses.
pub fn page_text(doc: &mut OpenDoc<'_>, page_index: u16) -> Result<Arc<PageText>, EngineError> {
    if let Some(cached) = doc.text.text(page_index) {
        return Ok(cached);
    }
    let chars = {
        let page = doc.page(page_index)?;
        let text = page.text().ctx("load text page")?;
        let chars = text.chars();
        let mut out = Vec::with_capacity(chars.len());
        for c in chars.iter() {
            out.push(char::from_u32(c.unicode_value()).unwrap_or('\u{fffd}'));
        }
        out
    };
    let text = Arc::new(PageText::from_chars(chars));
    doc.text.put_text(page_index, text.clone());
    Ok(text)
}

fn rect_of(r: &PdfRect) -> Rect {
    Rect::new(
        r.left().value,
        r.bottom().value,
        r.right().value,
        r.top().value,
    )
}

fn build_layer(
    page: &PdfPage<'_>,
    page_index: u16,
    geom: &PageGeom,
) -> Result<TextLayer, EngineError> {
    let text_page = page.text().ctx("load text page")?;
    let chars_handle = text_page.chars();
    let object_ids = object_index_map(page);

    let mut chars: Vec<CharEntry> = Vec::with_capacity(chars_handle.len());
    let mut text = String::with_capacity(chars_handle.len());
    for c in chars_handle.iter() {
        let loose = c.loose_bounds().map(|r| rect_of(&r)).unwrap_or(Rect::ZERO);
        let tight = c.tight_bounds().map(|r| rect_of(&r)).unwrap_or(loose);
        let ch = char::from_u32(c.unicode_value()).unwrap_or('\u{fffd}');
        let generated = c.is_generated().unwrap_or(false);
        let mut flags = 0u8;
        if generated {
            flags |= FLAG_GENERATED;
        }
        if c.is_hyphen().unwrap_or(false) {
            flags |= FLAG_HYPHEN;
        }
        if ch.is_whitespace() {
            flags |= FLAG_SPACE;
        }
        let object_id = object_ids
            .as_ref()
            .and_then(|map| object_id_for(map, &c))
            .unwrap_or(u32::MAX);
        chars.push(CharEntry {
            codepoint: c.unicode_value(),
            loose,
            tight,
            baseline_y: c.origin().map(|(_, y)| y.value).unwrap_or(loose.b),
            font_size: c.scaled_font_size().value,
            flags,
            object_id,
        });
        text.push(ch);
    }

    let (words, lines) = group(&chars);
    Ok(TextLayer {
        page: page_index,
        chars,
        words,
        lines,
        text,
        matrix: crate::engine::render::geometry::page_to_device(geom, 0, 1.0),
        crop: geom.crop,
        has_object_ids: object_ids.is_some(),
    })
}

/// Words break on whitespace / generated chars, on a baseline jump > 0.5 em, on a horizontal
/// gap > 0.25 em, or when x goes backwards. Lines join consecutive words whose baselines are
/// within 0.5 em.
pub(crate) fn group(chars: &[CharEntry]) -> (Vec<Word>, Vec<Line>) {
    let mut words: Vec<Word> = Vec::new();
    let mut current: Option<Word> = None;

    for (index, c) in chars.iter().enumerate() {
        let index = index as u32;
        let ch = char::from_u32(c.codepoint).unwrap_or('\u{fffd}');
        let separator =
            c.is_generated() || ch.is_whitespace() || matches!(ch, '\u{fffe}' | '\u{ffff}');
        if separator {
            if let Some(w) = current.take() {
                words.push(w);
            }
            continue;
        }
        let em = c.font_size.max(1.0);
        let start_new = match &current {
            None => true,
            Some(w) => {
                let dy = (w.baseline_y - c.baseline_y).abs();
                let gap = c.loose.l - w.rect.r;
                dy > 0.5 * em || gap > 0.25 * em || gap < -0.5 * em
            }
        };
        if start_new {
            if let Some(w) = current.take() {
                words.push(w);
            }
            current = Some(Word {
                first_char: index,
                char_count: 0,
                line_index: 0,
                rect: c.loose,
                baseline_y: c.baseline_y,
                font_size: c.font_size,
            });
        }
        let w = current.as_mut().expect("word was just started");
        w.rect = w.rect.union(&c.loose);
        w.char_count = index - w.first_char + 1;
    }
    if let Some(w) = current.take() {
        words.push(w);
    }

    let mut lines: Vec<Line> = Vec::new();
    for (i, word) in words.iter_mut().enumerate() {
        let em = word.font_size.max(1.0);
        let join = lines
            .last()
            .map(|l| (l.baseline_y - word.baseline_y).abs() <= 0.5 * em)
            .unwrap_or(false);
        if join {
            let line = lines.last_mut().expect("join implies a last line");
            line.rect = line.rect.union(&word.rect);
            line.word_count += 1;
            line.char_count = word.first_char + word.char_count - line.first_char;
        } else {
            lines.push(Line {
                first_word: i as u32,
                word_count: 1,
                baseline_y: word.baseline_y,
                rect: word.rect,
                first_char: word.first_char,
                char_count: word.char_count,
            });
        }
        word.line_index = (lines.len() - 1) as u32;
    }
    (words, lines)
}

/// Maps a text object's `(matrix, font size)` fingerprint to its page-object index, so each
/// char can be attributed to the object that drew it (text spike §5, method B).
fn object_index_map(page: &PdfPage<'_>) -> Option<HashMap<[i64; 7], u32>> {
    let objects = page.objects();
    let mut map: HashMap<[i64; 7], u32> = HashMap::new();
    for i in 0..objects.len() {
        let Ok(object) = objects.get(i) else { continue };
        let Some(text_object) = object.as_text_object() else {
            continue;
        };
        if let Some(key) = fingerprint(text_object) {
            map.entry(key).or_insert(i as u32);
        }
    }
    if map.is_empty() {
        None
    } else {
        Some(map)
    }
}

fn fingerprint(object: &PdfPageTextObject<'_>) -> Option<[i64; 7]> {
    let m = object.matrix().ok()?;
    let size = object.unscaled_font_size().value;
    Some([
        q(m.a()),
        q(m.b()),
        q(m.c()),
        q(m.d()),
        q(m.e()),
        q(m.f()),
        q(size),
    ])
}

fn q(v: f32) -> i64 {
    (v as f64 * 100.0).round() as i64
}

fn object_id_for(map: &HashMap<[i64; 7], u32>, c: &PdfPageTextChar<'_>) -> Option<u32> {
    let object = c.text_object().ok()?;
    map.get(&fingerprint(&object)?).copied()
}
