//! The 64 MiB encoded-PNG LRU — `ARCHITECTURE.md` §3.3.
//!
//! Looked up **on the protocol thread** before anything is submitted to the engine, so a
//! cache hit never touches pdfium. Hand-rolled `HashMap + VecDeque`; the `lru` crate buys
//! nothing here (`WORKPLAN.md` §5).
//!
//! v0.3 (pkg6): the budget is live — `set_budget` (설정 › 고급 › 캐시 크기) evicts down to the
//! new size at once, no restart — and the module emits `engine-pressure` (`IPC_CONTRACT.md` §8)
//! through [`TileCache::start_pressure_monitor`]. No pdfium anywhere in this file.

use crate::ipc::types::PressureLevel;
use parking_lot::Mutex;
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

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
}

impl RenderKind {
    pub fn as_str(self) -> &'static str {
        match self {
            RenderKind::Tile => "tile",
            RenderKind::Page => "page",
            RenderKind::Thumb => "thumb",
            RenderKind::Ocr => "ocr",
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
    /// `(when, bytes)` of every LRU eviction in the last [`CHURN_WINDOW`] — the cache half of
    /// the pressure signal.
    evictions: VecDeque<(Instant, usize)>,
}

// ---------------------------------------------------------------------------------------
// engine-pressure (v0.3, H5)
// ---------------------------------------------------------------------------------------

/// `high` above this share of a budget …
pub const PRESSURE_HIGH: f64 = 0.85;
/// … and `normal` again only below this one (hysteresis: no flapping around one threshold).
pub const PRESSURE_NORMAL: f64 = 0.70;
/// The process RSS budget the RSS half of the signal is measured against: the 400 MB idle
/// budget of `ARCHITECTURE.md` §13 plus the 256 MiB undo-snapshot RAM budget (§7), rounded up.
pub const RSS_BUDGET_BYTES: u64 = 1 << 30;
/// How far back evictions count. An LRU sits at ~100 % of its budget in steady state, so the
/// cache's share of the signal is **churn** — bytes evicted in this window against the budget:
/// a working set that no longer fits (a fling through a scan at 400 %) evicts a budget's worth
/// within seconds, a document that fits evicts nothing.
pub const CHURN_WINDOW: Duration = Duration::from_secs(10);
/// How often the monitor thread re-evaluates (and samples RSS) when nothing is inserted.
pub const MONITOR_INTERVAL: Duration = Duration::from_secs(3);

/// The next level for a measured load — pure, so the thresholds are unit-tested.
/// `cache` and `rss` are fractions of their budgets; the worse of the two decides.
pub fn next_pressure(current: PressureLevel, cache: f64, rss: f64) -> PressureLevel {
    let load = cache.max(rss);
    match current {
        PressureLevel::Normal if load > PRESSURE_HIGH => PressureLevel::High,
        PressureLevel::High if load < PRESSURE_NORMAL => PressureLevel::Normal,
        level => level,
    }
}

type PressureListener = Arc<dyn Fn(PressureLevel) + Send + Sync>;

struct Pressure {
    level: PressureLevel,
    /// Last RSS sample, as a fraction of [`RSS_BUDGET_BYTES`] (0 until the monitor samples).
    rss: f64,
    listener: Option<PressureListener>,
}

impl Default for Pressure {
    fn default() -> Self {
        Self {
            level: PressureLevel::Normal,
            rss: 0.0,
            listener: None,
        }
    }
}

pub struct TileCache {
    inner: Mutex<Inner>,
    /// Bytes; live (`set_budget`).
    capacity: AtomicUsize,
    pressure: Mutex<Pressure>,
}

impl TileCache {
    pub fn new(capacity: usize) -> Self {
        Self {
            inner: Mutex::new(Inner::default()),
            capacity: AtomicUsize::new(capacity),
            pressure: Mutex::new(Pressure::default()),
        }
    }

    pub fn capacity(&self) -> usize {
        self.capacity.load(Ordering::Relaxed)
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
        let capacity = self.capacity();
        if size > capacity {
            return; // A single image larger than the whole budget is never cached.
        }
        {
            let mut inner = self.inner.lock();
            if let Some(old) = inner.map.insert(key.clone(), image) {
                inner.bytes = inner.bytes.saturating_sub(old.bytes());
                if let Some(pos) = inner.order.iter().position(|k| k == &key) {
                    inner.order.remove(pos);
                }
            }
            inner.bytes += size;
            inner.order.push_back(key);
            Self::evict_to(&mut inner, capacity, true);
        }
        self.evaluate_pressure();
    }

    /// 설정 › 캐시 크기 (v0.3, U3): the new budget applies at once — the least recently used
    /// entries go until the cache fits. No pdfium; callable from any thread.
    ///
    /// A budget change is a settings action, not memory pressure: the evictions it causes are
    /// not recorded as churn, and the churn window restarts — churn is a fraction of the
    /// budget, so evictions measured against the old budget would be misread against the new
    /// one (e.g. 500 B churned on 1000 B reads as 1.67 after shrinking to 300 B). Real pressure
    /// shows up again within a few inserts, and the RSS signal is untouched.
    pub fn set_budget(&self, bytes: usize) {
        self.capacity.store(bytes, Ordering::Relaxed);
        {
            let mut inner = self.inner.lock();
            Self::evict_to(&mut inner, bytes, false);
            inner.evictions.clear();
        }
        self.evaluate_pressure();
    }

    /// Evicts from the LRU end until `inner.bytes <= capacity`; `record` logs each eviction
    /// as churn (the engine-pressure signal) — true for working-set evictions on insert only.
    fn evict_to(inner: &mut Inner, capacity: usize, record: bool) {
        let now = Instant::now();
        while inner.bytes > capacity {
            let Some(victim) = inner.order.pop_front() else {
                break;
            };
            if let Some(old) = inner.map.remove(&victim) {
                inner.bytes = inner.bytes.saturating_sub(old.bytes());
                if record {
                    inner.evictions.push_back((now, old.bytes()));
                }
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

    /// 설정 › 고급 › 캐시 비우기 (v0.3, U3) and tests.
    pub fn clear(&self) {
        {
            let mut inner = self.inner.lock();
            inner.map.clear();
            inner.order.clear();
            inner.bytes = 0;
            inner.evictions.clear();
        }
        self.evaluate_pressure();
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

    // --- engine-pressure -----------------------------------------------------------------

    /// The current pressure level.
    pub fn pressure(&self) -> PressureLevel {
        self.pressure.lock().level
    }

    /// Bytes evicted within [`CHURN_WINDOW`] as a fraction of the budget (older entries are
    /// dropped as a side effect).
    pub fn churn(&self) -> f64 {
        let capacity = self.capacity().max(1) as f64;
        let mut inner = self.inner.lock();
        if let Some(cutoff) = Instant::now().checked_sub(CHURN_WINDOW) {
            while inner.evictions.front().is_some_and(|&(at, _)| at < cutoff) {
                inner.evictions.pop_front();
            }
        }
        inner.evictions.iter().map(|&(_, b)| b as f64).sum::<f64>() / capacity
    }

    /// Records an RSS sample (bytes) and re-evaluates.
    pub fn note_rss(&self, rss_bytes: u64) {
        self.pressure.lock().rss = rss_bytes as f64 / RSS_BUDGET_BYTES as f64;
        self.evaluate_pressure();
    }

    /// Re-evaluates the level; a change calls the listener (outside the lock).
    pub fn evaluate_pressure(&self) -> PressureLevel {
        let churn = self.churn();
        let (changed, level, listener) = {
            let mut p = self.pressure.lock();
            let next = next_pressure(p.level, churn, p.rss);
            let changed = next != p.level;
            p.level = next;
            (changed, next, p.listener.clone())
        };
        if changed {
            tracing::debug!(?level, churn, "engine-pressure");
            if let Some(listener) = listener {
                listener(level);
            }
        }
        level
    }

    /// Installs the `engine-pressure` listener (the app emits the event from it).
    pub fn set_pressure_listener(&self, listener: impl Fn(PressureLevel) + Send + Sync + 'static) {
        self.pressure.lock().listener = Some(Arc::new(listener));
    }

    /// Installs the listener and starts the monitor thread: every [`MONITOR_INTERVAL`] it
    /// samples the process RSS and re-evaluates, so the level also drops back to `normal`
    /// while nothing is being rendered. The thread holds a `Weak` and ends with the cache.
    pub fn start_pressure_monitor(
        self: &Arc<Self>,
        listener: impl Fn(PressureLevel) + Send + Sync + 'static,
    ) {
        self.set_pressure_listener(listener);
        let weak: Weak<Self> = Arc::downgrade(self);
        let spawned = std::thread::Builder::new()
            .name("seepdf-pressure".into())
            .spawn(move || loop {
                std::thread::sleep(MONITOR_INTERVAL);
                let Some(cache) = weak.upgrade() else {
                    break;
                };
                cache.note_rss(crate::engine::stats::rss_bytes());
            });
        if let Err(e) = spawned {
            tracing::warn!("engine-pressure monitor not started: {e}");
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
    fn set_budget_evicts_to_the_new_limit() {
        let cache = TileCache::new(1000);
        for page in 0..8 {
            cache.insert(key(page, 1), image(100));
        }
        assert_eq!(cache.bytes(), 800);
        assert!(cache.get(&key(7, 1)).is_some()); // most recent
        cache.set_budget(300);
        assert_eq!(cache.capacity(), 300);
        assert_eq!(cache.bytes(), 300);
        assert_eq!(cache.len(), 3);
        // the least recently used went first: 5, 6 and the touched 7 remain
        assert!(cache.get(&key(7, 1)).is_some());
        assert!(cache.get(&key(6, 1)).is_some());
        assert!(cache.get(&key(5, 1)).is_some());
        assert!(cache.get(&key(0, 1)).is_none());
        // a larger budget keeps what is there and admits more
        cache.set_budget(1000);
        cache.insert(key(9, 1), image(100));
        assert_eq!(cache.bytes(), 400);
        // an image larger than the (new) budget is never cached
        cache.set_budget(50);
        assert_eq!(cache.bytes(), 0);
        cache.insert(key(10, 1), image(100));
        assert!(cache.is_empty());
    }

    #[test]
    fn pressure_thresholds_have_hysteresis() {
        use PressureLevel::{High, Normal};
        assert_eq!(next_pressure(Normal, 0.5, 0.0), Normal);
        assert_eq!(next_pressure(Normal, 0.85, 0.0), Normal); // not above 85 %
        assert_eq!(next_pressure(Normal, 0.86, 0.0), High);
        assert_eq!(next_pressure(Normal, 0.0, 0.9), High); // RSS alone is enough
        assert_eq!(next_pressure(High, 0.75, 0.0), High); // between the thresholds: stays
        assert_eq!(next_pressure(High, 0.69, 0.1), Normal);
        assert_eq!(next_pressure(High, 0.1, 0.72), High); // the worse of the two decides
    }

    #[test]
    fn eviction_churn_raises_and_clearing_lowers_pressure() {
        let cache = TileCache::new(1000);
        let seen = Arc::new(Mutex::new(Vec::new()));
        {
            let seen = seen.clone();
            cache.set_pressure_listener(move |l| seen.lock().push(l));
        }
        // filling the budget evicts nothing: an LRU at 100 % is not under pressure
        for page in 0..10 {
            cache.insert(key(page, 1), image(100));
        }
        assert_eq!(cache.pressure(), PressureLevel::Normal);
        // a working set that does not fit: 900 bytes evicted within the window
        for page in 10..19 {
            cache.insert(key(page, 1), image(100));
        }
        assert_eq!(cache.pressure(), PressureLevel::High);
        assert_eq!(*seen.lock(), vec![PressureLevel::High]);
        // RSS alone keeps it high once the churn is forgotten (clear drops the window)
        cache.note_rss((RSS_BUDGET_BYTES as f64 * 0.8) as u64);
        cache.clear();
        assert_eq!(cache.pressure(), PressureLevel::High);
        cache.note_rss(RSS_BUDGET_BYTES / 10);
        assert_eq!(cache.pressure(), PressureLevel::Normal);
        assert_eq!(
            *seen.lock(),
            vec![PressureLevel::High, PressureLevel::Normal]
        );
    }

    #[test]
    fn shrinking_the_budget_is_not_memory_pressure() {
        // verification round 1 (H5): 설정 › 캐시 크기 256 → 64 raised engine-pressure 'high'
        let cache = TileCache::new(256_000);
        let seen = Arc::new(Mutex::new(Vec::new()));
        {
            let seen = seen.clone();
            cache.set_pressure_listener(move |l| seen.lock().push(l));
        }
        for page in 0..256 {
            cache.insert(key(page, 1), image(1000));
        }
        assert_eq!(cache.pressure(), PressureLevel::Normal);
        cache.set_budget(64_000);
        assert_eq!(cache.bytes(), 64_000);
        assert_eq!(cache.churn(), 0.0);
        assert_eq!(cache.pressure(), PressureLevel::Normal);
        assert!(
            seen.lock().is_empty(),
            "no engine-pressure event for a settings change"
        );

        // churn measured against the old budget is not re-read against the smaller one:
        // 500 B churned on a 1000 B budget (0.5) would be 1.67 of a 300 B budget
        let cache = TileCache::new(1000);
        for page in 0..15 {
            cache.insert(key(page, 1), image(100));
        }
        assert!((cache.churn() - 0.5).abs() < 1e-9);
        assert_eq!(cache.pressure(), PressureLevel::Normal);
        cache.set_budget(300);
        assert_eq!(cache.pressure(), PressureLevel::Normal);
        // real churn afterwards still counts against the new budget
        for page in 20..23 {
            cache.insert(key(page, 1), image(100));
        }
        assert!((cache.churn() - 1.0).abs() < 1e-9);
        assert_eq!(cache.pressure(), PressureLevel::High);
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
