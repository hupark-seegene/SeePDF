//! macOS Vision OCR — P1-11, `ocr_recognize_native` (`IPC_CONTRACT.md` §7.9).
//!
//! Three halves, only one of them platform-specific:
//!
//! * **render** ([`super::render_page_gray`]) — the same grayscale image the `/ocr` route hands
//!   tesseract.js, rendered on the engine thread (PDFium is not thread-safe);
//! * **recognise** ([`mac::recognize`], macOS only) — `VNRecognizeTextRequest` at the accurate
//!   level, `ko-KR` + `en-US`, language correction on. It runs **off** the engine thread (Vision
//!   is thread-safe and takes 0.1–3 s a page; holding the engine for that would stall every tile);
//! * **normalise** ([`normalize`]) — Vision's observations → the contract's `OcrPage`, the Rust
//!   twin of `src/ocr/normalize.ts` `normalizeVision`: normalised bottom-left rects become image
//!   pixels with the origin top-left, exactly like tesseract's, so `ocr_apply` cannot tell the
//!   two engines apart.
//!
//! Vision gives one observation per **line**. Word boxes come from `boundingBoxForRange` on the
//! top candidate; when Vision answers with the whole line box for every word (it does for some
//! scripts and revisions) the line is split by character count instead, as the TS normaliser
//! does for an observation without words.

use crate::ipc::types::{OcrBox, OcrLine, OcrPage, OcrWord, PageIndex, Rotation};

/// Words below this confidence (0..100) are dropped, as in `normalizeVision`
/// (`DEFAULT_MIN_CONFIDENCE`). Vision's accurate level reports 0.3 / 0.5 / 1.0 for most lines,
/// so 30 keeps everything Vision itself was willing to return.
pub const MIN_CONFIDENCE: f32 = 30.0;

/// A normalised Vision rect: 0..1, origin **bottom-left** (Vision's own space).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VisionRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct VisionWord {
    pub text: String,
    /// 0..1
    pub confidence: f32,
    pub bbox: VisionRect,
}

/// One `VNRecognizedTextObservation`: its top candidate, the line box and (when Vision could
/// place them) per-word boxes. `words` empty means "split the line for me".
#[derive(Debug, Clone, PartialEq)]
pub struct VisionObservation {
    pub text: String,
    /// 0..1
    pub confidence: f32,
    pub bbox: VisionRect,
    pub words: Vec<VisionWord>,
}

/// What the image was: the same fields `normalizeVision`'s `NormalizeContext` carries.
#[derive(Debug, Clone, Copy)]
pub struct VisionContext {
    pub page: PageIndex,
    pub dpi: u32,
    pub width_px: u32,
    pub height_px: u32,
    pub rotation: Rotation,
    pub min_confidence: f32,
}

// ---------------------------------------------------------------------------------------
// Languages
// ---------------------------------------------------------------------------------------

/// The request's languages as Vision spells them (BCP 47), from whatever the caller sent:
/// tesseract codes (`kor`, `eng`), bare ISO 639-1 (`ko`) or Vision's own (`ko-KR`).
///
/// Order is priority (the first language wins ties), duplicates go, and Korean always brings
/// English along — a Korean scan carries Latin, the same rule as `kor+eng` for tesseract.
/// Nothing requested means Korean + English.
pub fn vision_languages(requested: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for raw in requested
        .iter()
        .flat_map(|s| s.split(['+', ',']))
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        let lang = match raw.to_ascii_lowercase().replace('_', "-").as_str() {
            "kor" | "ko" | "ko-kr" => "ko-KR".to_string(),
            "eng" | "en" | "en-us" => "en-US".to_string(),
            "jpn" | "ja" | "ja-jp" => "ja-JP".to_string(),
            "chi-sim" | "zh" | "zh-hans" | "zh-cn" => "zh-Hans".to_string(),
            "chi-tra" | "zh-hant" | "zh-tw" => "zh-Hant".to_string(),
            _ => raw.to_string(),
        };
        if !out.contains(&lang) {
            out.push(lang);
        }
    }
    if out.is_empty() {
        out = vec!["ko-KR".to_string(), "en-US".to_string()];
    }
    if out.iter().any(|l| l == "ko-KR") && !out.iter().any(|l| l == "en-US") {
        out.push("en-US".to_string());
    }
    out
}

// ---------------------------------------------------------------------------------------
// Normalisation (platform independent, unit-tested everywhere)
// ---------------------------------------------------------------------------------------

/// Sub-pixel precision is noise in an OCR box; two decimals, as in `normalize.ts`.
fn px(n: f64) -> f32 {
    ((n * 100.0).round() / 100.0) as f32
}

/// Normalised Vision rect (0..1, origin bottom-left) → image pixels, origin top-left.
pub fn rect_to_pixels(r: VisionRect, width_px: u32, height_px: u32) -> OcrBox {
    let (w, h) = (width_px as f64, height_px as f64);
    let x0 = r.x * w;
    let x1 = (r.x + r.width) * w;
    let y0 = (1.0 - (r.y + r.height)) * h;
    let y1 = (1.0 - r.y) * h;
    [px(x0.min(x1)), px(y0.min(y1)), px(x0.max(x1)), px(y0.max(y1))]
}

/// Vision's 0..1 → the contract's 0..100, one decimal (Tesseract's scale).
fn percent(confidence: f32) -> f32 {
    let c = if confidence.is_finite() { confidence.clamp(0.0, 1.0) } else { 0.0 };
    ((c as f64 * 1000.0).round() / 10.0) as f32
}

/// The same Hangul ranges as `normalize.ts` `HANGUL`.
pub fn has_hangul(s: &str) -> bool {
    s.chars().any(|c| {
        matches!(c as u32,
            0x1100..=0x11FF | 0x3130..=0x318F | 0xA960..=0xA97F | 0xAC00..=0xD7AF | 0xD7B0..=0xD7FF)
    })
}

/// `splitObservation`: a line without per-word boxes, cut into its whitespace-separated words
/// with the line box distributed by character count (one character's width per gap).
fn split_line(text: &str, bbox: OcrBox, confidence: f32) -> Vec<OcrWord> {
    let tokens: Vec<&str> = text.split_whitespace().collect();
    let chars: usize = tokens.iter().map(|t| t.chars().count()).sum();
    if tokens.is_empty() || chars == 0 {
        return Vec::new();
    }
    let width = (bbox[2] - bbox[0]) as f64;
    let per = width / (chars + tokens.len().saturating_sub(1)) as f64;
    let mut x = bbox[0] as f64;
    let mut out = Vec::with_capacity(tokens.len());
    for token in tokens {
        let w = token.chars().count() as f64 * per;
        out.push(OcrWord {
            text: token.to_string(),
            bbox: [px(x), bbox[1], px(x + w), bbox[3]],
            confidence,
        });
        x += w + per;
    }
    out
}

/// Reading order: top → bottom, and left → right among lines on the same row.
///
/// `normalize.ts` sorts with a tolerance comparator; that is not a total order, and Rust's sort
/// may panic on one, so the lines are sorted by top edge and then **grouped into rows** (a line
/// whose top is within half a line height of the row's first line joins it) before each row is
/// sorted by its left edge. Same result on real pages, and deterministic.
fn reading_order(mut lines: Vec<(OcrBox, usize)>) -> Vec<usize> {
    lines.sort_by(|a, b| a.0[1].total_cmp(&b.0[1]).then(a.0[0].total_cmp(&b.0[0])));
    let mut out = Vec::with_capacity(lines.len());
    let mut row: Vec<(OcrBox, usize)> = Vec::new();
    let flush = |row: &mut Vec<(OcrBox, usize)>, out: &mut Vec<usize>| {
        row.sort_by(|a, b| a.0[0].total_cmp(&b.0[0]));
        out.extend(row.drain(..).map(|(_, i)| i));
    };
    for line in lines {
        if let Some(first) = row.first() {
            let h_first = first.0[3] - first.0[1];
            let h_line = line.0[3] - line.0[1];
            let tolerance = (h_first.min(h_line) * 0.5).max(4.0);
            if (line.0[1] - first.0[1]).abs() > tolerance {
                flush(&mut row, &mut out);
            }
        }
        row.push(line);
    }
    flush(&mut row, &mut out);
    out
}

/// Vision observations → `OcrPage`, the Rust twin of `normalizeVision` (`src/ocr/normalize.ts`).
///
/// Boxes are image pixels with the origin top-left, confidences 0..100, words below
/// `ctx.min_confidence` are dropped, and every line gets the synthetic baseline and row height
/// the TS normaliser gives it (Vision has neither): the baseline sits 10 % of the line height
/// above the bottom for Hangul, 20 % for Latin.
pub fn normalize(observations: &[VisionObservation], ctx: &VisionContext) -> OcrPage {
    let boxes: Vec<(OcrBox, usize)> = observations
        .iter()
        .enumerate()
        .map(|(i, o)| (rect_to_pixels(o.bbox, ctx.width_px, ctx.height_px), i))
        .collect();
    let mut lines: Vec<OcrLine> = Vec::new();
    for index in reading_order(boxes) {
        let o = &observations[index];
        let text = o.text.trim();
        if text.is_empty() {
            continue;
        }
        let bbox = rect_to_pixels(o.bbox, ctx.width_px, ctx.height_px);
        let confidence = percent(o.confidence);
        let words: Vec<OcrWord> = if o.words.is_empty() {
            split_line(text, bbox, confidence)
        } else {
            o.words
                .iter()
                .filter(|w| !w.text.trim().is_empty())
                .map(|w| OcrWord {
                    text: w.text.trim().to_string(),
                    bbox: rect_to_pixels(w.bbox, ctx.width_px, ctx.height_px),
                    confidence: percent(w.confidence),
                })
                .collect()
        };
        let kept: Vec<OcrWord> = words
            .into_iter()
            .filter(|w| w.confidence >= ctx.min_confidence)
            .collect();
        if kept.is_empty() {
            continue;
        }
        let height = bbox[3] - bbox[1];
        let baseline_y = bbox[3] - height * if has_hangul(text) { 0.1 } else { 0.2 };
        lines.push(OcrLine {
            text: kept
                .iter()
                .map(|w| w.text.as_str())
                .collect::<Vec<_>>()
                .join(" "),
            bbox,
            baseline: Some([bbox[0], baseline_y, bbox[2], baseline_y]),
            row_height_px: Some(height),
            words: kept,
        });
    }
    OcrPage {
        page: ctx.page,
        dpi: ctx.dpi,
        width_px: ctx.width_px,
        height_px: ctx.height_px,
        rotation: ctx.rotation,
        lines,
    }
}

/// The whitespace-separated words of `text` with their `NSRange` (UTF-16 offset, UTF-16
/// length) — what `boundingBoxForRange` wants.
pub fn word_ranges(text: &str) -> Vec<(String, usize, usize)> {
    let mut out = Vec::new();
    let mut offset = 0usize;
    let mut current: Option<(String, usize)> = None;
    for c in text.chars() {
        if c.is_whitespace() {
            if let Some((word, start)) = current.take() {
                out.push((word, start, offset - start));
            }
        } else {
            match current.as_mut() {
                Some((word, _)) => word.push(c),
                None => current = Some((c.to_string(), offset)),
            }
        }
        offset += c.len_utf16();
    }
    if let Some((word, start)) = current {
        out.push((word, start, offset - start));
    }
    out
}

/// `true` when Vision gave every word of a multi-word line (nearly) the whole line box — it
/// could not place them, and the character-count split is the better guess.
pub fn words_are_degenerate(line: VisionRect, words: &[VisionWord]) -> bool {
    words.len() > 1
        && line.width > 0.0
        && words.iter().all(|w| w.bbox.width >= line.width * 0.9)
}

// ---------------------------------------------------------------------------------------
// Vision itself (macOS only)
// ---------------------------------------------------------------------------------------

#[cfg(target_os = "macos")]
pub mod mac {
    //! The only `unsafe` Objective-C in the project. Every call happens inside an autorelease
    //! pool: this runs on a tokio blocking thread, which has none of its own.

    use super::{words_are_degenerate, word_ranges, VisionObservation, VisionRect, VisionWord};
    use crate::ipc::{EngineError, ErrorCode};
    use objc2::rc::{autoreleasepool, Retained};
    use objc2::runtime::AnyObject;
    use objc2::AnyThread;
    use objc2_core_foundation::{CFData, CFRetained, CGRect};
    use objc2_core_graphics::{
        CGBitmapInfo, CGColorRenderingIntent, CGColorSpace, CGDataProvider, CGImage,
        CGImageAlphaInfo,
    };
    use objc2_foundation::{
        NSArray, NSDictionary, NSOperatingSystemVersion, NSProcessInfo, NSRange, NSString,
    };
    use objc2_vision::{
        VNImageOption, VNImageRequestHandler, VNRecognizeTextRequest, VNRecognizedText,
        VNRequest, VNRequestTextRecognitionLevel,
    };
    use std::sync::OnceLock;

    /// The languages the accurate recogniser supports on this Mac, or `None` when Vision
    /// cannot read Korean here at all (macOS < 13, where `ko-KR` was added).
    fn supported_languages() -> Option<&'static [String]> {
        static SUPPORTED: OnceLock<Option<Vec<String>>> = OnceLock::new();
        SUPPORTED
            .get_or_init(|| {
                autoreleasepool(|_| {
                    let v13 = NSOperatingSystemVersion {
                        majorVersion: 13,
                        minorVersion: 0,
                        patchVersion: 0,
                    };
                    if !NSProcessInfo::processInfo().isOperatingSystemAtLeastVersion(v13) {
                        return None;
                    }
                    let request = VNRecognizeTextRequest::new();
                    request.setRecognitionLevel(VNRequestTextRecognitionLevel::Accurate);
                    // SAFETY: an instance method available from macOS 12; gated on 13 above.
                    let langs = unsafe { request.supportedRecognitionLanguagesAndReturnError() }
                        .ok()?;
                    let langs: Vec<String> = langs.iter().map(|s| s.to_string()).collect();
                    langs.iter().any(|l| l == "ko-KR").then_some(langs)
                })
            })
            .as_deref()
    }

    /// `ocr_capabilities`: Vision is offered only where it reads Korean.
    pub fn available() -> bool {
        supported_languages().is_some()
    }

    fn vision_error(what: &str, detail: impl std::fmt::Display) -> EngineError {
        EngineError::new(ErrorCode::Pdfium, format!("Vision: {what}: {detail}"))
    }

    fn nfc(s: &NSString) -> String {
        s.precomposedStringWithCanonicalMapping().to_string()
    }

    fn rect(r: CGRect) -> VisionRect {
        VisionRect {
            x: r.origin.x,
            y: r.origin.y,
            width: r.size.width,
            height: r.size.height,
        }
    }

    /// An 8-bit grayscale `CGImage` over a copy of `gray` (row-major, `width` bytes a row).
    fn gray_image(
        gray: &[u8],
        width: u32,
        height: u32,
    ) -> Result<CFRetained<CGImage>, EngineError> {
        let (w, h) = (width as usize, height as usize);
        if w == 0 || h == 0 || gray.len() < w * h {
            return Err(EngineError::invalid(format!(
                "a {width}x{height} image needs {} bytes, got {}",
                w * h,
                gray.len()
            )));
        }
        let data = CFData::from_bytes(&gray[..w * h]);
        let provider = CGDataProvider::with_cf_data(Some(&data))
            .ok_or_else(|| vision_error("CGDataProviderCreateWithCFData", "null"))?;
        let space = CGColorSpace::new_device_gray()
            .ok_or_else(|| vision_error("CGColorSpaceCreateDeviceGray", "null"))?;
        // SAFETY: the provider holds exactly w*h bytes of 8-bit gray, one byte a pixel.
        unsafe {
            CGImage::new(
                w,
                h,
                8,
                8,
                w,
                Some(&space),
                CGBitmapInfo(CGImageAlphaInfo::None.0),
                Some(&provider),
                std::ptr::null(),
                false,
                CGColorRenderingIntent::RenderingIntentDefault,
            )
        }
        .ok_or_else(|| vision_error("CGImageCreate", "null"))
    }

    /// Per-word boxes of one candidate; empty when Vision cannot place the words.
    fn words_of(
        candidate: &VNRecognizedText,
        text: &str,
        line: VisionRect,
        confidence: f32,
    ) -> Vec<VisionWord> {
        let mut words = Vec::new();
        for (word, start, len) in word_ranges(text) {
            let range = NSRange::new(start, len);
            // SAFETY: `range` lies inside the candidate's own string (UTF-16 units).
            let Ok(observation) = (unsafe { candidate.boundingBoxForRange_error(range) }) else {
                return Vec::new();
            };
            // SAFETY: a plain property read.
            let bbox = rect(unsafe { observation.boundingBox() });
            words.push(VisionWord {
                text: nfc(&NSString::from_str(&word)),
                confidence,
                bbox,
            });
        }
        if words_are_degenerate(line, &words) {
            return Vec::new();
        }
        words
    }

    /// Runs `VNRecognizeTextRequest` (accurate, language correction on) over an 8-bit
    /// grayscale image. Blocking — call it from a blocking thread, never the engine thread.
    pub fn recognize(
        gray: &[u8],
        width: u32,
        height: u32,
        languages: &[String],
    ) -> Result<Vec<VisionObservation>, EngineError> {
        let Some(supported) = supported_languages() else {
            return Err(EngineError::unsupported(
                "ocr_recognize_native: Vision cannot read Korean before macOS 13",
            ));
        };
        let wanted: Vec<&String> = languages
            .iter()
            .filter(|l| supported.iter().any(|s| s == *l))
            .collect();
        if wanted.is_empty() {
            return Err(EngineError::unsupported(&format!(
                "ocr_recognize_native: none of {languages:?} is a Vision language here"
            )));
        }
        autoreleasepool(|_| {
            let image = gray_image(gray, width, height)?;
            let options = NSDictionary::<VNImageOption, AnyObject>::new();
            // SAFETY: an empty options dictionary is always valid.
            let handler = unsafe {
                VNImageRequestHandler::initWithCGImage_options(
                    VNImageRequestHandler::alloc(),
                    &image,
                    &options,
                )
            };

            let request = VNRecognizeTextRequest::new();
            request.setRecognitionLevel(VNRequestTextRecognitionLevel::Accurate);
            request.setUsesLanguageCorrection(true);
            let names: Vec<Retained<NSString>> =
                wanted.iter().map(|l| NSString::from_str(l)).collect();
            request.setRecognitionLanguages(&NSArray::from_retained_slice(&names));

            let as_request: &VNRequest = &request;
            let requests = NSArray::from_slice(&[as_request]);
            handler
                .performRequests_error(&requests)
                .map_err(|e| vision_error("performRequests", e.localizedDescription()))?;

            let mut out = Vec::new();
            let Some(results) = request.results() else {
                return Ok(out);
            };
            for observation in results.iter() {
                let candidates = observation.topCandidates(1);
                let Some(candidate) = candidates.firstObject() else {
                    continue;
                };
                let raw = candidate.string();
                let text = raw.to_string();
                if text.trim().is_empty() {
                    continue;
                }
                let confidence = candidate.confidence();
                // SAFETY: a plain property read.
                let line = rect(unsafe { observation.boundingBox() });
                let words = words_of(&candidate, &text, line, confidence);
                out.push(VisionObservation {
                    text: nfc(&raw),
                    confidence,
                    bbox: line,
                    words,
                });
            }
            Ok(out)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> VisionContext {
        VisionContext {
            page: 2,
            dpi: 300,
            width_px: 1000,
            height_px: 2000,
            rotation: 0,
            min_confidence: MIN_CONFIDENCE,
        }
    }

    fn r(x: f64, y: f64, w: f64, h: f64) -> VisionRect {
        VisionRect { x, y, width: w, height: h }
    }

    fn obs(text: &str, confidence: f32, bbox: VisionRect, words: Vec<VisionWord>) -> VisionObservation {
        VisionObservation {
            text: text.into(),
            confidence,
            bbox,
            words,
        }
    }

    fn word(text: &str, confidence: f32, bbox: VisionRect) -> VisionWord {
        VisionWord {
            text: text.into(),
            confidence,
            bbox,
        }
    }

    #[test]
    fn boxes_flip_to_top_left_pixels() {
        // A line hugging the top-left corner: y is measured from the bottom in Vision.
        let b = rect_to_pixels(r(0.1, 0.9, 0.5, 0.05), 1000, 2000);
        assert_eq!(b, [100.0, 100.0, 600.0, 200.0]);
    }

    /// The fixtures of `normalize.test.ts` "normalize.vision", with the same expectations: the
    /// Rust normaliser must agree with the TS one it replaces.
    #[test]
    fn maps_boxes_like_normalize_vision() {
        let page = normalize(
            &[obs(
                "검색가능한 Searchable",
                0.95,
                r(0.1, 0.8, 0.5, 0.05),
                vec![
                    word("검색가능한", 0.96, r(0.1, 0.8, 0.3, 0.05)),
                    word("Searchable", 0.94, r(0.42, 0.8, 0.18, 0.05)),
                ],
            )],
            &ctx(),
        );
        assert_eq!(
            (page.page, page.dpi, page.width_px, page.height_px, page.rotation),
            (2, 300, 1000, 2000, 0)
        );
        assert_eq!(page.lines.len(), 1);
        let line = &page.lines[0];
        // y: 1 - (0.8 + 0.05) = 0.15 → 300 px top, 1 - 0.8 = 0.2 → 400 px bottom.
        assert_eq!(line.bbox, [100.0, 300.0, 600.0, 400.0]);
        assert_eq!(line.words[0].text, "검색가능한");
        assert_eq!(line.words[0].bbox, [100.0, 300.0, 400.0, 400.0]);
        assert_eq!(line.words[0].confidence, 96.0);
        assert_eq!(line.text, "검색가능한 Searchable");
        // Hangul sits lower in the box than Latin (spike §5).
        assert!((line.baseline.unwrap()[1] - 390.0).abs() < 1e-3);
        assert_eq!(line.row_height_px, Some(100.0));
    }

    #[test]
    fn splits_a_line_without_word_boxes_like_normalize_vision() {
        let page = normalize(&[obs("one two", 0.9, r(0.0, 0.5, 1.0, 0.05), vec![])], &ctx());
        let words = &page.lines[0].words;
        assert_eq!(words.iter().map(|w| w.text.as_str()).collect::<Vec<_>>(), ["one", "two"]);
        assert_eq!(words[0].bbox[0], 0.0);
        assert!((words[1].bbox[2] - 1000.0).abs() < 0.01);
        assert_eq!(words[0].confidence, 90.0);
        // 1000 px over 3 + 3 chars + 1 gap: "one" ends at 3/7 of the line.
        assert!((words[0].bbox[2] - 428.57).abs() < 0.01);
        // Latin: baseline 20 % of the line height above the bottom.
        let [_, top, _, bottom] = page.lines[0].bbox;
        let baseline = page.lines[0].baseline.unwrap()[1];
        assert!((baseline - (bottom - (bottom - top) * 0.2)).abs() < 0.01);
    }

    #[test]
    fn orders_lines_like_normalize_vision() {
        let page = normalize(
            &[
                obs("b", 0.9, r(0.5, 0.9, 0.1, 0.02), vec![]),
                obs("a", 0.9, r(0.1, 0.9, 0.1, 0.02), vec![]),
                obs("c", 0.9, r(0.1, 0.2, 0.1, 0.02), vec![]),
            ],
            &ctx(),
        );
        let order: Vec<&str> = page.lines.iter().map(|l| l.text.as_str()).collect();
        assert_eq!(order, ["a", "b", "c"]);
    }

    #[test]
    fn same_row_reads_left_to_right() {
        let page = normalize(
            &[
                obs("right", 1.0, r(0.6, 0.5, 0.2, 0.02), vec![]),
                // A few pixels higher, but on the same row.
                obs("left", 1.0, r(0.1, 0.501, 0.2, 0.02), vec![]),
                obs("below", 1.0, r(0.1, 0.4, 0.2, 0.02), vec![]),
            ],
            &ctx(),
        );
        let order: Vec<&str> = page.lines.iter().map(|l| l.text.as_str()).collect();
        assert_eq!(order, ["left", "right", "below"]);
    }

    #[test]
    fn drops_low_confidence_and_empty_lines() {
        let page = normalize(
            &[
                obs("  ", 1.0, r(0.1, 0.9, 0.2, 0.02), vec![]),
                obs("noise", 0.1, r(0.1, 0.8, 0.2, 0.02), vec![]),
                obs("kept", 0.3, r(0.1, 0.7, 0.2, 0.02), vec![]),
            ],
            &ctx(),
        );
        assert_eq!(page.lines.len(), 1);
        assert_eq!(page.lines[0].words[0].confidence, 30.0);
    }

    #[test]
    fn word_ranges_are_utf16() {
        let words = word_ranges("가 😀x  end");
        assert_eq!(
            words,
            vec![("가".to_string(), 0, 1), ("😀x".to_string(), 2, 3), ("end".to_string(), 7, 3)]
        );
    }

    #[test]
    fn degenerate_word_boxes_fall_back_to_splitting() {
        let line = r(0.1, 0.5, 0.4, 0.02);
        let whole = |t: &str| word(t, 1.0, line);
        assert!(words_are_degenerate(line, &[whole("a"), whole("b")]));
        assert!(!words_are_degenerate(line, &[whole("a")]));
        assert!(!words_are_degenerate(
            line,
            &[whole("a"), word("b", 1.0, r(0.3, 0.5, 0.2, 0.02))]
        ));
    }

    #[test]
    fn languages_map_to_vision_codes() {
        let v = |xs: &[&str]| {
            vision_languages(&xs.iter().map(|s| s.to_string()).collect::<Vec<_>>())
        };
        assert_eq!(v(&["kor", "eng"]), ["ko-KR", "en-US"]);
        assert_eq!(v(&["kor+eng"]), ["ko-KR", "en-US"]);
        assert_eq!(v(&["ko-KR"]), ["ko-KR", "en-US"], "Korean never goes alone");
        assert_eq!(v(&["eng"]), ["en-US"]);
        assert_eq!(v(&[]), ["ko-KR", "en-US"]);
        assert_eq!(v(&["en-US", "en", "ja"]), ["en-US", "ja-JP"]);
    }
}
