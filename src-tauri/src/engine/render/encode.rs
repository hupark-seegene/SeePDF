//! The PNG encode pool — `ARCHITECTURE.md` §1.1.
//!
//! Encoding is the only parallelisable part of the render pipeline (0.2 ms per 512² tile,
//! 2.29 ms per 2× page with `Compression::Fast` + `Filter::Sub`, render spike §4) and the
//! engine thread must never run an encoder. Workers encode, insert into the tile cache and
//! invoke the sink — which for a `seepdf://` request is the `UriSchemeResponder` itself, so
//! the webview is answered with no hop back to the engine or main thread.

use crate::engine::render::cache::{EncodedImage, TileCache, TileKey};
use crate::ipc::{EngineError, ErrorCode};
use crossbeam_channel::{unbounded, Sender};
use std::sync::Arc;
use std::time::Instant;

/// Raw pixels straight out of pdfium: RGBA8, top-left origin, tightly packed
/// (`set_reverse_byte_order(true)` + `as_raw_bytes()`, one copy, no swizzle).
pub struct RawImage {
    pub pixels: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub render_ms: f64,
    /// Encode as 8-bit grayscale (the `/ocr` route) instead of RGBA.
    pub grayscale: bool,
}

type Sink = Box<dyn FnOnce(Result<Arc<EncodedImage>, EngineError>) + Send + 'static>;

struct EncodeJob {
    key: Option<TileKey>,
    image: RawImage,
    cache: Option<Arc<TileCache>>,
    stats: Option<Arc<crate::engine::stats::Stats>>,
    sink: Sink,
}

/// `seepdf-encode-0..N`.
#[derive(Clone)]
pub struct EncodePool {
    tx: Sender<EncodeJob>,
}

impl EncodePool {
    pub fn spawn(workers: usize) -> Self {
        let (tx, rx) = unbounded::<EncodeJob>();
        for i in 0..workers.max(1) {
            let rx = rx.clone();
            std::thread::Builder::new()
                .name(format!("seepdf-encode-{i}"))
                .spawn(move || {
                    for job in rx {
                        let t0 = Instant::now();
                        let encoded = encode_png(&job.image).map(|png| {
                            Arc::new(EncodedImage {
                                png,
                                width: job.image.width,
                                height: job.image.height,
                                render_ms: job.image.render_ms,
                                encode_ms: t0.elapsed().as_secs_f64() * 1000.0,
                            })
                        });
                        if let Ok(image) = &encoded {
                            if let Some(stats) = &job.stats {
                                stats.record_encode_ms(image.encode_ms);
                            }
                            if let (Some(cache), Some(key)) = (&job.cache, &job.key) {
                                cache.insert(key.clone(), image.clone());
                            }
                        }
                        (job.sink)(encoded);
                    }
                })
                .expect("spawn encode worker");
        }
        Self { tx }
    }

    /// Encodes `image`, optionally caching it under `key`, then calls `sink` on the worker.
    pub fn submit(
        &self,
        key: Option<TileKey>,
        image: RawImage,
        cache: Option<Arc<TileCache>>,
        stats: Option<Arc<crate::engine::stats::Stats>>,
        sink: impl FnOnce(Result<Arc<EncodedImage>, EngineError>) + Send + 'static,
    ) {
        let job = EncodeJob {
            key,
            image,
            cache,
            stats,
            sink: Box::new(sink),
        };
        if let Err(e) = self.tx.send(job) {
            // The pool is gone; the sink went with the job, so nothing can be answered.
            tracing::error!("encode pool is gone: {e}");
        }
    }
}

/// PNG, `Compression::Fast` + `Filter::Sub`: 1.4 MB / 2.29 ms for a 2× letter page, versus
/// 431 kB / 30.4 ms for `Balanced` + `Adaptive` (render spike §4).
pub fn encode_png(image: &RawImage) -> Result<Vec<u8>, EngineError> {
    let expected = (image.width as usize) * (image.height as usize) * 4;
    if image.pixels.len() < expected {
        return Err(EngineError::new(
            ErrorCode::Pdfium,
            format!(
                "bitmap is {} bytes, expected {expected} for {}x{}",
                image.pixels.len(),
                image.width,
                image.height
            ),
        ));
    }
    let mut out = Vec::with_capacity(expected / 4);
    {
        let mut encoder = png::Encoder::new(&mut out, image.width, image.height);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_compression(png::Compression::Fast);
        encoder.set_filter(png::Filter::Sub);
        if image.grayscale {
            encoder.set_color(png::ColorType::Grayscale);
        } else {
            encoder.set_color(png::ColorType::Rgba);
        }
        let mut writer = encoder
            .write_header()
            .map_err(|e| EngineError::new(ErrorCode::Io, format!("png header: {e}")))?;
        if image.grayscale {
            // pdfium rendered with `use_grayscale_rendering`, so R == G == B; take one byte
            // per pixel rather than re-running a colour transform.
            let gray: Vec<u8> = image.pixels[..expected]
                .chunks_exact(4)
                .map(|px| px[0])
                .collect();
            writer
                .write_image_data(&gray)
                .map_err(|e| EngineError::new(ErrorCode::Io, format!("png data: {e}")))?;
        } else {
            writer
                .write_image_data(&image.pixels[..expected])
                .map_err(|e| EngineError::new(ErrorCode::Io, format!("png data: {e}")))?;
        }
        writer
            .finish()
            .map_err(|e| EngineError::new(ErrorCode::Io, format!("png finish: {e}")))?;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_rgba_and_grayscale() {
        let image = RawImage {
            pixels: vec![0x40; 4 * 4 * 4],
            width: 4,
            height: 4,
            render_ms: 0.0,
            grayscale: false,
        };
        let rgba = encode_png(&image).expect("rgba");
        assert_eq!(&rgba[1..4], b"PNG");

        let gray = encode_png(&RawImage {
            grayscale: true,
            ..image
        })
        .expect("gray");
        assert!(gray.len() < rgba.len() + 64);
    }

    #[test]
    fn refuses_a_short_buffer() {
        let err = encode_png(&RawImage {
            pixels: vec![0; 3],
            width: 4,
            height: 4,
            render_ms: 0.0,
            grayscale: false,
        })
        .unwrap_err();
        assert_eq!(err.code, ErrorCode::Pdfium);
    }
}
