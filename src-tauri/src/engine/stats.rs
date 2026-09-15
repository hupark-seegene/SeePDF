//! Cheap counters behind `engine_stats` (`IPC_CONTRACT.md` §5).

use crate::engine::types::Lane;
use crate::ipc::types::{EngineStats, QueueDepth};
use parking_lot::Mutex;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

const WINDOW: usize = 256;

/// A fixed-size ring of recent samples; percentiles are computed by sorting a copy.
#[derive(Default)]
struct Window {
    samples: Vec<f64>,
    next: usize,
}

impl Window {
    fn push(&mut self, v: f64) {
        if self.samples.len() < WINDOW {
            self.samples.push(v);
        } else {
            self.samples[self.next] = v;
            self.next = (self.next + 1) % WINDOW;
        }
    }

    fn percentile(&self, p: f64) -> f64 {
        if self.samples.is_empty() {
            return 0.0;
        }
        let mut v = self.samples.clone();
        v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let idx = (((v.len() - 1) as f64) * p).round() as usize;
        v[idx]
    }
}

#[derive(Default)]
pub struct Stats {
    pub dropped_stale: AtomicU64,
    pub cache_hits: AtomicU64,
    pub cache_misses: AtomicU64,
    pub page_lru_len: AtomicU32,
    queue_depth: [AtomicU32; 5],
    tile_ms: Mutex<Window>,
    encode_ms: Mutex<Window>,
}

impl Stats {
    pub fn record_tile_ms(&self, ms: f64) {
        self.tile_ms.lock().push(ms);
    }

    pub fn record_encode_ms(&self, ms: f64) {
        self.encode_ms.lock().push(ms);
    }

    pub fn set_queue_depth(&self, lane: Lane, depth: u32) {
        self.queue_depth[lane.index()].store(depth, Ordering::Relaxed);
    }

    pub fn note_cache(&self, hit: bool) {
        if hit {
            self.cache_hits.fetch_add(1, Ordering::Relaxed);
        } else {
            self.cache_misses.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub fn snapshot(&self, docs: u32, tile_cache_bytes: u64) -> EngineStats {
        let hits = self.cache_hits.load(Ordering::Relaxed);
        let misses = self.cache_misses.load(Ordering::Relaxed);
        let tile = self.tile_ms.lock();
        let encode = self.encode_ms.lock();
        EngineStats {
            docs,
            queue_depth: QueueDepth {
                interactive: self.queue_depth[0].load(Ordering::Relaxed),
                edit: self.queue_depth[1].load(Ordering::Relaxed),
                prefetch: self.queue_depth[2].load(Ordering::Relaxed),
                thumb: self.queue_depth[3].load(Ordering::Relaxed),
                background: self.queue_depth[4].load(Ordering::Relaxed),
            },
            dropped_stale: self.dropped_stale.load(Ordering::Relaxed),
            tile_p50_ms: tile.percentile(0.50),
            tile_p95_ms: tile.percentile(0.95),
            encode_p50_ms: encode.percentile(0.50),
            tile_cache_bytes,
            tile_cache_hit_rate: if hits + misses == 0 {
                0.0
            } else {
                hits as f64 / (hits + misses) as f64
            },
            page_lru_len: self.page_lru_len.load(Ordering::Relaxed),
            rss_bytes: rss_bytes(),
        }
    }
}

/// Resident set size of this process, best effort (0 when unavailable).
#[cfg(target_os = "macos")]
pub fn rss_bytes() -> u64 {
    // `ps` avoids a libproc dependency and is only called from `engine_stats`.
    std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &std::process::id().to_string()])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|s| s.trim().parse::<u64>().ok())
        .map(|kb| kb * 1024)
        .unwrap_or(0)
}

#[cfg(not(target_os = "macos"))]
pub fn rss_bytes() -> u64 {
    0
}
