//! The 64 MiB encoded-PNG LRU — `ARCHITECTURE.md` §3.3.
//!
//! Looked up **on the protocol thread** before anything is submitted to the engine, so a
//! cache hit never touches pdfium. Hand-rolled `HashMap + VecDeque`; the `lru` crate buys
//! nothing here (`WORKPLAN.md` §5).

use parking_lot::Mutex;
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

/// 64 MiB of encoded PNG.
pub const DEFAULT_CAPACITY_BYTES: usize = 64 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RenderKind {
    /// 512×512 device-pixel tile.
    Tile,
    /// Whole page (also the low-`sk` placeholder).
    Page,
    /// Sidebar / organiser thumbnail; `scale_key` carries the target width in px.
    Thumb,
    /// Gray8 page image for the OCR workers; `scale_key` carries the DPI.
    Ocr,
    /// v0.3 pkg8 (X7): a whole page for the print-only DOM — `/page?…&print=all|none|stamps`.
    /// Rendered with `FPDF_PRINTING`, so annotation `/F` Print / NoView flags are honoured the
    /// way a printer sees them, and never dropped as stale when the viewport moves.
    Print(PrintAnnots),
}

/// v0.3 pkg8 (X7): which markup reaches paper (인쇄 ▸ 주석).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, Default, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "camelCase")]
pub enum PrintAnnots {
    /// 문서와 주석 (default): every annotation whose flags say it prints.
    #[default]
    All,
    /// 문서만: page content and form field values, no markup.
    None,
    /// 문서와 도장·서명: only stamp and signature annotations (plus form fields).
    Stamps,
}

impl PrintAnnots {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "1" | "all" | "true" => Some(PrintAnnots::All),
            "none" => Some(PrintAnnots::None),
            "stamps" => Some(PrintAnnots::Stamps),
            _ => None,
        }
    }
}

impl RenderKind {
    pub fn as_str(self) -> &'static str {
        match self {
            RenderKind::Tile => "tile",
            RenderKind::Page => "page",
            RenderKind::Thumb => "thumb",
            RenderKind::Ocr => "ocr",
            RenderKind::Print(PrintAnnots::All) => "print",
            RenderKind::Print(PrintAnnots::None) => "print-none",
            RenderKind::Print(PrintAnnots::Stamps) => "print-stamps",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Night {
    #[default]
    Off,
    Dark,
    Sepia,
}

impl Night {
    pub fn parse(value: Option<&str>) -> Self {
        match value.unwrap_or("0") {
            "1" | "dark" | "true" => Night::Dark,
            "sepia" | "2" => Night::Sepia,
            _ => Night::Off,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Night::Off => "0",
            Night::Dark => "1",
            Night::Sepia => "sepia",
        }
    }

    pub fn is_on(self) -> bool {
        !matches!(self, Night::Off)
    }
}

/// `{doc, gen, page, kind, scaleKey, rot, tx, ty, night, hl, forms}`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TileKey {
    pub doc: String,
    pub generation: u32,
    pub page: u16,
    pub kind: RenderKind,
    /// Device `scaleKey` for tile/page, target width for thumb, DPI for ocr.
    pub scale_key: u32,
    pub rotation: u16,
    pub tx: u32,
    pub ty: u32,
    pub night: Night,
    /// Form-field highlighting ("필드 강조") is part of the key.
    pub hl: bool,
    /// Draw the AcroForm widgets (`FPDF_FFLDraw`). `false` is the `forms=0` request the
    /// 양식 overlay makes so the field value is rendered exactly once — by the HTML input
    /// on top, not by PDFium underneath as well (F-20).
    pub forms: bool,
}

impl TileKey {
    /// The `ETag` value from `IPC_CONTRACT.md` §9, quotes included.
    pub fn etag(&self) -> String {
        format!(
            "\"{}:{}:{}:{}:{}:{}:{}:{}:{}:{}:{}\"",
            self.doc,
            self.generation,
            self.page,
            self.kind.as_str(),
            self.scale_key,
            self.rotation,
            self.tx,
            self.ty,
            self.night.as_str(),
            u8::from(self.hl),
            u8::from(self.forms)
        )
    }
}

#[derive(Debug)]
pub struct EncodedImage {
    pub png: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub render_ms: f64,
    pub encode_ms: f64,
}

impl EncodedImage {
    pub fn bytes(&self) -> usize {
        self.png.len()
    }
}

#[derive(Default)]
struct Inner {
    map: HashMap<TileKey, Arc<EncodedImage>>,
    /// Least-recently used at the front.
    order: VecDeque<TileKey>,
    bytes: usize,
}

pub struct TileCache {
    inner: Mutex<Inner>,
    capacity: usize,
}

impl TileCache {
    pub fn new(capacity: usize) -> Self {
        Self {
            inner: Mutex::new(Inner::default()),
            capacity,
        }
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    pub fn bytes(&self) -> usize {
        self.inner.lock().bytes
    }

    pub fn len(&self) -> usize {
        self.inner.lock().map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn get(&self, key: &TileKey) -> Option<Arc<EncodedImage>> {
        let mut inner = self.inner.lock();
        let hit = inner.map.get(key).cloned()?;
        if let Some(pos) = inner.order.iter().position(|k| k == key) {
            let k = inner.order.remove(pos).expect("position was just found");
            inner.order.push_back(k);
        }
        Some(hit)
    }

    pub fn insert(&self, key: TileKey, image: Arc<EncodedImage>) {
        let size = image.bytes();
        if size > self.capacity {
            return; // A single image larger than the whole budget is never cached.
        }
        let mut inner = self.inner.lock();
        if let Some(old) = inner.map.insert(key.clone(), image) {
            inner.bytes = inner.bytes.saturating_sub(old.bytes());
            if let Some(pos) = inner.order.iter().position(|k| k == &key) {
                inner.order.remove(pos);
            }
        }
        inner.bytes += size;
        inner.order.push_back(key);
        while inner.bytes > self.capacity {
            let Some(victim) = inner.order.pop_front() else {
                break;
            };
            if let Some(old) = inner.map.remove(&victim) {
                inner.bytes = inner.bytes.saturating_sub(old.bytes());
            }
        }
    }

    /// Sweeps a document's dead entries after a generation bump (n ≤ ~500, O(n)).
    pub fn drop_older_generations(&self, doc: &str, keep: u32) {
        self.retain(|k| k.doc != doc || k.generation >= keep);
    }

    pub fn drop_document(&self, doc: &str) {
        self.retain(|k| k.doc != doc);
    }

    pub fn clear(&self) {
        let mut inner = self.inner.lock();
        inner.map.clear();
        inner.order.clear();
        inner.bytes = 0;
    }

    fn retain(&self, keep: impl Fn(&TileKey) -> bool) {
        let mut inner = self.inner.lock();
        let doomed: Vec<TileKey> = inner.map.keys().filter(|k| !keep(k)).cloned().collect();
        for key in doomed {
            if let Some(old) = inner.map.remove(&key) {
                inner.bytes = inner.bytes.saturating_sub(old.bytes());
            }
            if let Some(pos) = inner.order.iter().position(|k| k == &key) {
                inner.order.remove(pos);
            }
        }
    }
}

impl Default for TileCache {
    fn default() -> Self {
        Self::new(DEFAULT_CAPACITY_BYTES)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(page: u16, generation: u32) -> TileKey {
        TileKey {
            doc: "d1".into(),
            generation,
            page,
            kind: RenderKind::Tile,
            scale_key: 100,
            rotation: 0,
            tx: 0,
            ty: 0,
            night: Night::Off,
            hl: false,
            forms: true,
        }
    }

    fn image(bytes: usize) -> Arc<EncodedImage> {
        Arc::new(EncodedImage {
            png: vec![0u8; bytes],
            width: 1,
            height: 1,
            render_ms: 0.0,
            encode_ms: 0.0,
        })
    }

    #[test]
    fn evicts_least_recently_used_first() {
        let cache = TileCache::new(300);
        cache.insert(key(0, 1), image(100));
        cache.insert(key(1, 1), image(100));
        cache.insert(key(2, 1), image(100));
        assert!(cache.get(&key(0, 1)).is_some()); // page 0 becomes most-recent
        cache.insert(key(3, 1), image(100));
        assert!(cache.get(&key(1, 1)).is_none());
        assert!(cache.get(&key(0, 1)).is_some());
        assert_eq!(cache.bytes(), 300);
    }

    #[test]
    fn generation_bump_sweeps_dead_entries() {
        let cache = TileCache::new(1000);
        cache.insert(key(0, 1), image(10));
        cache.insert(key(0, 2), image(10));
        cache.drop_older_generations("d1", 2);
        assert!(cache.get(&key(0, 1)).is_none());
        assert!(cache.get(&key(0, 2)).is_some());
        assert_eq!(cache.bytes(), 10);
    }

    #[test]
    fn etag_is_the_contract_shape() {
        // …:night:hl:forms — `forms` is 1 by default (F-20 only turns it off in 양식 mode).
        assert_eq!(key(3, 7).etag(), "\"d1:7:3:tile:100:0:0:0:0:0:1\"");
        let no_forms = TileKey {
            forms: false,
            ..key(3, 7)
        };
        assert_eq!(no_forms.etag(), "\"d1:7:3:tile:100:0:0:0:0:0:0\"");
    }
}
