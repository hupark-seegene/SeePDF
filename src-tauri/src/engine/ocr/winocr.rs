//! Native Windows OCR — `Windows.Media.Ocr` (v0.3 O3, pkg7-ocr), the Windows twin of
//! [`super::vision`] behind the same `ocr_recognize_native`.
//!
//! Korean Windows ships the Korean recogniser (and every Windows the one of its display
//! language); it is faster than tesseract.js and needs no 8 MB of WASM. Three halves, like
//! Vision's:
//!
//! * **render** — [`super::render_page_gray`], the `/ocr` image, on the engine thread;
//! * **recognise** ([`win::recognize`], Windows only) — `OcrEngine::TryCreateFromLanguage` +
//!   `RecognizeAsync(..).get()` over a `SoftwareBitmap`, on a blocking thread (WinRT is
//!   free-threaded; the MTA is joined implicitly by `windows-core`);
//! * **normalise** ([`observations`], everywhere) — Windows' lines and word rectangles (image
//!   pixels, origin top-left) into [`super::vision::VisionObservation`]s, so
//!   [`super::vision::normalize`] builds the `OcrPage` exactly as it does for Vision: reading
//!   order, synthetic baseline and row height, the same confidence filter.
//!
//! Windows reports **no confidence**; every word gets [`WINDOWS_CONFIDENCE`]. It also recognises
//! **one language per engine**: the first requested language this machine has a recogniser for
//! wins (Korean first for `kor+eng` — the Korean recogniser reads Latin too).
//!
//! CJK recognisers report every ideograph / kana as its own "word" with a space between them in
//! the line text; the text layer would then read `日 本 語`. Adjacent CJK words are merged back
//! into one run before normalisation.

use super::vision::{VisionObservation, VisionRect, VisionWord};

/// The confidence (0..1) given to every Windows word: above every filter, below a certainty.
pub const WINDOWS_CONFIDENCE: f32 = 0.9;

/// One recognised word: text and its box in image pixels, origin top-left.
#[derive(Debug, Clone, PartialEq)]
pub struct WinWord {
    pub text: String,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// One `OcrLine`.
#[derive(Debug, Clone, PartialEq)]
pub struct WinLine {
    pub text: String,
    pub words: Vec<WinWord>,
}

/// Han ideographs, kana and CJK punctuation — scripts written without spaces between words.
fn is_unspaced_script(c: char) -> bool {
    matches!(c as u32,
        0x3000..=0x303F   // CJK symbols and punctuation
        | 0x3040..=0x30FF // hiragana, katakana
        | 0x31F0..=0x31FF // katakana phonetic extensions
        | 0x3400..=0x4DBF // CJK extension A
        | 0x4E00..=0x9FFF // CJK unified ideographs
        | 0xF900..=0xFAFF // compatibility ideographs
        | 0xFF00..=0xFF60 // fullwidth forms
        | 0x20000..=0x3FFFF)
}

fn unspaced(text: &str) -> bool {
    !text.is_empty() && text.chars().all(is_unspaced_script)
}

/// Merges runs of adjacent ideograph/kana words into one word (the union of their boxes).
pub fn merge_unspaced(words: &[WinWord]) -> Vec<WinWord> {
    let mut out: Vec<WinWord> = Vec::with_capacity(words.len());
    for word in words {
        let text = word.text.trim();
        if text.is_empty() {
            continue;
        }
        if let Some(last) = out.last_mut() {
            if unspaced(&last.text) && unspaced(text) {
                let x0 = last.x.min(word.x);
                let y0 = last.y.min(word.y);
                let x1 = (last.x + last.width).max(word.x + word.width);
                let y1 = (last.y + last.height).max(word.y + word.height);
                last.text.push_str(text);
                last.x = x0;
                last.y = y0;
                last.width = x1 - x0;
                last.height = y1 - y0;
                continue;
            }
        }
        out.push(WinWord {
            text: text.to_string(),
            ..word.clone()
        });
    }
    out
}

/// Image-pixel rectangle (origin top-left) → Vision's normalised rect (origin bottom-left).
fn to_vision_rect(x: f64, y: f64, w: f64, h: f64, width_px: u32, height_px: u32) -> VisionRect {
    let (iw, ih) = (width_px.max(1) as f64, height_px.max(1) as f64);
    VisionRect {
        x: x / iw,
        y: 1.0 - (y + h) / ih,
        width: w / iw,
        height: h / ih,
    }
}

/// Windows' lines → the observation shape [`super::vision::normalize`] takes. `scale` maps the
/// recognised image back to the page image (a downscaled read has `scale` > 1).
pub fn observations(
    lines: &[WinLine],
    width_px: u32,
    height_px: u32,
    scale: f64,
) -> Vec<VisionObservation> {
    let mut out = Vec::with_capacity(lines.len());
    for line in lines {
        let words: Vec<WinWord> = merge_unspaced(&line.words)
            .into_iter()
            .map(|w| WinWord {
                x: w.x * scale,
                y: w.y * scale,
                width: w.width * scale,
                height: w.height * scale,
                ..w
            })
            .filter(|w| w.width > 0.0 && w.height > 0.0)
            .collect();
        if words.is_empty() {
            continue;
        }
        let x0 = words.iter().map(|w| w.x).fold(f64::INFINITY, f64::min);
        let y0 = words.iter().map(|w| w.y).fold(f64::INFINITY, f64::min);
        let x1 = words
            .iter()
            .map(|w| w.x + w.width)
            .fold(f64::NEG_INFINITY, f64::max);
        let y1 = words
            .iter()
            .map(|w| w.y + w.height)
            .fold(f64::NEG_INFINITY, f64::max);
        let text = words
            .iter()
            .map(|w| w.text.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        out.push(VisionObservation {
            text,
            confidence: WINDOWS_CONFIDENCE,
            bbox: to_vision_rect(x0, y0, x1 - x0, y1 - y0, width_px, height_px),
            words: words
                .iter()
                .map(|w| VisionWord {
                    text: w.text.clone(),
                    confidence: WINDOWS_CONFIDENCE,
                    bbox: to_vision_rect(w.x, w.y, w.width, w.height, width_px, height_px),
                })
                .collect(),
        });
    }
    out
}

/// The app's language code (`kor`, `eng`, `jpn`, `chi_sim`) of a Windows recogniser tag
/// (`ko`, `en-US`, `ja`, `zh-Hans-CN`…), or `None` for a language SeePDF does not offer.
pub fn app_code(tag: &str) -> Option<&'static str> {
    let tag = tag.to_ascii_lowercase().replace('_', "-");
    let primary = tag.split('-').next().unwrap_or("");
    match primary {
        "ko" => Some("kor"),
        "en" => Some("eng"),
        "ja" => Some("jpn"),
        "zh" if tag.contains("hant") || tag.ends_with("-tw") || tag.ends_with("-hk") => None,
        "zh" => Some("chi_sim"),
        _ => None,
    }
}

/// The recogniser tag to use for `requested` (app codes, tesseract strings like `kor+eng`, or
/// BCP 47 tags as the frontend sends Vision): the first requested language `available` has.
pub fn pick_language(requested: &[String], available: &[String]) -> Option<String> {
    let wanted: Vec<&'static str> = requested
        .iter()
        .flat_map(|s| s.split(['+', ',']))
        .map(str::trim)
        .filter_map(|code| match code {
            "kor" | "eng" | "jpn" | "chi_sim" => super::app_language(code),
            other => app_code(other),
        })
        .collect();
    let wanted = if wanted.is_empty() {
        vec!["kor", "eng"]
    } else {
        wanted
    };
    wanted.iter().find_map(|code| {
        available
            .iter()
            .find(|tag| app_code(tag) == Some(code))
            .cloned()
    })
}

/// The app codes a list of recogniser tags covers, in the app's order, without duplicates.
pub fn app_languages(tags: &[String]) -> Vec<String> {
    super::APP_LANGUAGES
        .iter()
        .filter(|code| tags.iter().any(|t| app_code(t) == Some(**code)))
        .map(|c| c.to_string())
        .collect()
}

/// A box-filtered copy of `gray` scaled down by the integer `factor` (for pages larger than
/// `OcrEngine::MaxImageDimension`). Returns the pixels and their size.
pub fn downscale(gray: &[u8], width: u32, height: u32, factor: u32) -> (Vec<u8>, u32, u32) {
    let factor = factor.max(1);
    if factor == 1 {
        return (gray.to_vec(), width, height);
    }
    let (w, h) = (width as usize, height as usize);
    let (nw, nh) = (w.div_ceil(factor as usize), h.div_ceil(factor as usize));
    let f = factor as usize;
    let mut out = vec![0u8; nw * nh];
    for ny in 0..nh {
        for nx in 0..nw {
            let (mut sum, mut n) = (0u32, 0u32);
            for y in ny * f..((ny + 1) * f).min(h) {
                for x in nx * f..((nx + 1) * f).min(w) {
                    sum += gray[y * w + x] as u32;
                    n += 1;
                }
            }
            out[ny * nw + nx] = (sum / n.max(1)) as u8;
        }
    }
    (out, nw as u32, nh as u32)
}

// ---------------------------------------------------------------------------------------
// Windows.Media.Ocr itself (Windows only)
// ---------------------------------------------------------------------------------------

#[cfg(windows)]
pub mod win {
    use super::{WinLine, WinWord};
    use crate::ipc::{EngineError, ErrorCode};
    use std::sync::OnceLock;
    use windows::core::HSTRING;
    use windows::Globalization::Language;
    use windows::Graphics::Imaging::{BitmapAlphaMode, BitmapPixelFormat, SoftwareBitmap};
    use windows::Media::Ocr::OcrEngine;
    use windows::Storage::Streams::DataWriter;

    fn ocr_error(what: &str, e: windows::core::Error) -> EngineError {
        EngineError::new(ErrorCode::Pdfium, format!("Windows OCR: {what}: {e}"))
    }

    /// The recogniser languages installed on this machine (`ko`, `en-US`, …), once per process.
    pub fn available_languages() -> &'static [String] {
        static TAGS: OnceLock<Vec<String>> = OnceLock::new();
        TAGS.get_or_init(|| {
            let Ok(list) = OcrEngine::AvailableRecognizerLanguages() else {
                return Vec::new();
            };
            let mut out = Vec::new();
            for language in list {
                if let Ok(tag) = language.LanguageTag() {
                    out.push(tag.to_string());
                }
            }
            out
        })
    }

    /// `OcrEngine::MaxImageDimension` (10 000 px on current Windows).
    pub fn max_dimension() -> u32 {
        OcrEngine::MaxImageDimension().unwrap_or(10_000).max(1)
    }

    /// Recognises an 8-bit grayscale image with the recogniser of `tag`. Blocking — call it
    /// from a blocking thread, never the engine thread.
    pub fn recognize(
        gray: &[u8],
        width: u32,
        height: u32,
        tag: &str,
    ) -> Result<Vec<WinLine>, EngineError> {
        let (w, h) = (width as usize, height as usize);
        if w == 0 || h == 0 || gray.len() < w * h {
            return Err(EngineError::invalid(format!(
                "a {width}x{height} image needs {} bytes, got {}",
                w * h,
                gray.len()
            )));
        }
        let language = Language::CreateLanguage(&HSTRING::from(tag))
            .map_err(|e| ocr_error("CreateLanguage", e))?;
        let engine = OcrEngine::TryCreateFromLanguage(&language).map_err(|e| {
            EngineError::unsupported(&format!(
                "ocr_recognize_native: no Windows recogniser for {tag}: {e}"
            ))
        })?;
        // Bgra8 premultiplied is the format every OcrEngine revision accepts.
        let mut bgra = Vec::with_capacity(w * h * 4);
        for &g in &gray[..w * h] {
            bgra.extend_from_slice(&[g, g, g, 255]);
        }
        let writer = DataWriter::new().map_err(|e| ocr_error("DataWriter", e))?;
        writer
            .WriteBytes(&bgra)
            .map_err(|e| ocr_error("WriteBytes", e))?;
        let buffer = writer
            .DetachBuffer()
            .map_err(|e| ocr_error("DetachBuffer", e))?;
        let bitmap = SoftwareBitmap::CreateCopyWithAlphaFromBuffer(
            &buffer,
            BitmapPixelFormat::Bgra8,
            width as i32,
            height as i32,
            BitmapAlphaMode::Premultiplied,
        )
        .map_err(|e| ocr_error("SoftwareBitmap", e))?;
        let result = engine
            .RecognizeAsync(&bitmap)
            .map_err(|e| ocr_error("RecognizeAsync", e))?
            .get()
            .map_err(|e| ocr_error("RecognizeAsync", e))?;
        let mut out = Vec::new();
        for line in result.Lines().map_err(|e| ocr_error("Lines", e))? {
            let text = line.Text().map(|t| t.to_string()).unwrap_or_default();
            let mut words = Vec::new();
            for word in line.Words().map_err(|e| ocr_error("Words", e))? {
                let rect = word
                    .BoundingRect()
                    .map_err(|e| ocr_error("BoundingRect", e))?;
                words.push(WinWord {
                    text: word.Text().map(|t| t.to_string()).unwrap_or_default(),
                    x: rect.X as f64,
                    y: rect.Y as f64,
                    width: rect.Width as f64,
                    height: rect.Height as f64,
                });
            }
            out.push(WinLine { text, words });
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::super::vision::{self, VisionContext};
    use super::*;

    fn w(text: &str, x: f64, y: f64, width: f64, height: f64) -> WinWord {
        WinWord {
            text: text.into(),
            x,
            y,
            width,
            height,
        }
    }

    /// The cross-platform mapping test of O3: mock word rects → the contract's `OcrPage`.
    #[test]
    fn word_rects_map_to_an_ocr_page() {
        let lines = vec![
            WinLine {
                text: "검색 가능한 문서".into(),
                words: vec![
                    w("검색", 100.0, 300.0, 200.0, 100.0),
                    w("가능한", 320.0, 300.0, 280.0, 100.0),
                    w("문서", 620.0, 300.0, 180.0, 100.0),
                ],
            },
            WinLine {
                text: "Mixed script 2026".into(),
                words: vec![
                    w("Mixed", 100.0, 500.0, 200.0, 60.0),
                    w("script", 320.0, 500.0, 220.0, 60.0),
                    w("2026", 560.0, 500.0, 160.0, 60.0),
                ],
            },
        ];
        let obs = observations(&lines, 1000, 2000, 1.0);
        let page = vision::normalize(
            &obs,
            &VisionContext {
                page: 4,
                dpi: 300,
                width_px: 1000,
                height_px: 2000,
                rotation: 0,
                min_confidence: vision::MIN_CONFIDENCE,
            },
        );
        assert_eq!(page.page, 4);
        assert_eq!(page.lines.len(), 2);
        let first = &page.lines[0];
        assert_eq!(first.text, "검색 가능한 문서");
        assert_eq!(first.bbox, [100.0, 300.0, 800.0, 400.0]);
        assert_eq!(first.words[1].text, "가능한");
        assert_eq!(first.words[1].bbox, [320.0, 300.0, 600.0, 400.0]);
        assert_eq!(first.words[1].confidence, 90.0);
        assert_eq!(page.lines[1].words[2].bbox, [560.0, 500.0, 720.0, 560.0]);
        assert_eq!(page.lines[1].text, "Mixed script 2026");
    }

    #[test]
    fn a_downscaled_read_maps_back_to_the_page_image() {
        let lines = vec![WinLine {
            text: "big".into(),
            words: vec![w("big", 10.0, 20.0, 30.0, 5.0)],
        }];
        let obs = observations(&lines, 1000, 1000, 2.0);
        let page = vision::normalize(
            &obs,
            &VisionContext {
                page: 0,
                dpi: 300,
                width_px: 1000,
                height_px: 1000,
                rotation: 0,
                min_confidence: 0.0,
            },
        );
        assert_eq!(page.lines[0].words[0].bbox, [20.0, 40.0, 80.0, 50.0]);
    }

    #[test]
    fn cjk_characters_merge_into_one_run() {
        let merged = merge_unspaced(&[
            w("日", 0.0, 0.0, 10.0, 10.0),
            w("本", 11.0, 0.0, 10.0, 10.0),
            w("語", 22.0, 1.0, 10.0, 10.0),
            w("OCR", 40.0, 0.0, 30.0, 10.0),
            w("テ", 75.0, 0.0, 10.0, 10.0),
        ]);
        let texts: Vec<&str> = merged.iter().map(|w| w.text.as_str()).collect();
        assert_eq!(texts, ["日本語", "OCR", "テ"]);
        assert_eq!(
            (merged[0].x, merged[0].y, merged[0].width, merged[0].height),
            (0.0, 0.0, 32.0, 11.0)
        );
        // Hangul words keep their spaces.
        let hangul = merge_unspaced(&[
            w("검색", 0.0, 0.0, 10.0, 10.0),
            w("문서", 12.0, 0.0, 10.0, 10.0),
        ]);
        assert_eq!(hangul.len(), 2);
    }

    #[test]
    fn languages_map_between_windows_tags_and_app_codes() {
        let available: Vec<String> = ["en-US", "ko", "ja", "zh-Hans-CN", "zh-Hant-TW"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(app_languages(&available), ["kor", "eng", "jpn", "chi_sim"]);
        let pick = |xs: &[&str]| {
            pick_language(
                &xs.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
                &available,
            )
        };
        assert_eq!(pick(&["kor+eng"]).as_deref(), Some("ko"));
        assert_eq!(pick(&["ko-KR", "en-US"]).as_deref(), Some("ko"));
        assert_eq!(pick(&["eng"]).as_deref(), Some("en-US"));
        assert_eq!(pick(&["zh-Hans"]).as_deref(), Some("zh-Hans-CN"));
        assert_eq!(pick(&["jpn"]).as_deref(), Some("ja"));
        assert_eq!(pick(&[]).as_deref(), Some("ko"));
        let english_only = vec!["en-US".to_string()];
        assert_eq!(
            pick_language(&["kor".to_string(), "eng".to_string()], &english_only).as_deref(),
            Some("en-US"),
            "Korean missing: the next requested language"
        );
        assert_eq!(pick_language(&["jpn".to_string()], &english_only), None);
    }

    #[test]
    fn downscale_averages_blocks() {
        let (px, w, h) = downscale(&[0, 100, 200, 250, 10, 20], 3, 2, 2);
        assert_eq!((w, h), (2, 1));
        assert_eq!(px, vec![90, 110], "(0+100+250+10)/4 and (200+20)/2");
    }
}
