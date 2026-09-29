//! 페이지 회전 자동 감지 (v0.3 O2, pkg7-ocr): which way up is a scan?
//!
//! Sideways and upside-down scans are recognised as garbage, so with the option on each page is
//! first read at a low resolution (~100 DPI) four times — as rendered, and turned 90°, 180° and
//! 270° clockwise — and the turn whose words the recogniser is most confident about wins, if it
//! wins clearly (a margin of at least 15 % over the runner-up and over the page as it is).
//! `ocr_apply` then sets the page's `/Rotate` in the same undo step as the text layer
//! (`OcrApplyPage`'s `setRotation`), so the scan reads upright in the viewer too.
//!
//! [`pick`] is the Rust twin of `src/ocr/orientation.ts` `pickOrientation` (the tesseract path
//! runs in the frontend); [`rotate_gray`] turns a rendered page for the native recognisers, which
//! is exactly what PDFium would render with the extra `/Rotate` (the page box turns with it).

use super::GrayPage;
use crate::ipc::types::{OcrPage, Rotation};
use serde::{Deserialize, Serialize};

/// The resolution the four trial reads use: enough for a recogniser to tell text from noise,
/// a ninth of the pixels of the 300 DPI read.
pub const DETECT_DPI: u32 = 100;

/// The winner must beat the runner-up (and the page as it is) by this factor.
pub const MARGIN: f32 = 1.15;

/// Fewer words than this at the best turn: not enough evidence to turn anything.
pub const MIN_WORDS: u32 = 3;

/// One trial read.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrientationScore {
    /// Clockwise turn applied to the rendered page before the read: 0, 90, 180 or 270.
    pub rotation: Rotation,
    /// Mean word confidence, 0..100.
    pub confidence: f32,
    pub words: u32,
}

/// `ocr_detect_orientation`'s answer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrientationResult {
    /// The clockwise turn to apply: 0 = leave the page as it is.
    pub rotation: Rotation,
    pub scores: Vec<OrientationScore>,
}

/// The score of one read: mean confidence over the words it kept.
pub fn score(rotation: Rotation, page: &OcrPage) -> OrientationScore {
    let words: Vec<f32> = page
        .lines
        .iter()
        .flat_map(|l| l.words.iter())
        .filter(|w| !w.text.trim().is_empty())
        .map(|w| w.confidence)
        .collect();
    let confidence = if words.is_empty() {
        0.0
    } else {
        words.iter().sum::<f32>() / words.len() as f32
    };
    OrientationScore {
        rotation,
        confidence,
        words: words.len() as u32,
    }
}

/// Vision's reading directions ([`super::vision::TextDirection`]) as one score per turn: the
/// share of the page's characters (0..100) on lines whose baseline points that way, and their
/// words. Vision reads sideways text as confidently as upright text, so its *direction* is the
/// signal, not its confidence.
pub fn scores_from_directions(dirs: &[super::vision::TextDirection]) -> Vec<OrientationScore> {
    let total: u32 = dirs.iter().map(|d| d.chars).sum();
    [0u16, 90, 180, 270]
        .into_iter()
        .map(|rotation| {
            let (chars, words) = dirs
                .iter()
                .filter(|d| d.angle.is_finite() && quarter(d.angle) == rotation)
                .fold((0u32, 0u32), |(c, w), d| (c + d.chars, w + d.words));
            OrientationScore {
                rotation,
                confidence: if total == 0 {
                    0.0
                } else {
                    chars as f32 * 100.0 / total as f32
                },
                words,
            }
        })
        .collect()
}

/// An angle in degrees, counter-clockwise, as the nearest quarter turn 0/90/180/270.
pub fn quarter(angle: f64) -> Rotation {
    (((angle / 90.0).round() as i64).rem_euclid(4) * 90) as Rotation
}

/// The clockwise turn that makes the page upright, or 0 when no turn wins clearly.
///
/// A turn wins when it has the highest mean confidence, at least [`MIN_WORDS`] words, and beats
/// both the runner-up and the unturned page by [`MARGIN`]. Anything less is noise, and turning a
/// correct page is worse than leaving a sideways one.
pub fn pick(scores: &[OrientationScore]) -> Rotation {
    let usable = |s: &&OrientationScore| s.words >= MIN_WORDS && s.confidence.is_finite();
    let mut ranked: Vec<&OrientationScore> = scores.iter().filter(usable).collect();
    ranked.sort_by(|a, b| b.confidence.total_cmp(&a.confidence));
    let Some(best) = ranked.first() else {
        return 0;
    };
    if best.rotation % 360 == 0 {
        return 0;
    }
    let runner_up = ranked.get(1).map(|s| s.confidence).unwrap_or(0.0);
    let as_is = scores
        .iter()
        .find(|s| s.rotation % 360 == 0)
        .map(|s| if s.words == 0 { 0.0 } else { s.confidence })
        .unwrap_or(0.0);
    if best.confidence >= runner_up * MARGIN && best.confidence >= as_is * MARGIN {
        best.rotation % 360
    } else {
        0
    }
}

/// `image` turned `delta` degrees clockwise (0/90/180/270), with its `rotation` advanced to match:
/// the pixels PDFium renders once the page's `/Rotate` has grown by `delta`.
pub fn rotate_gray(image: &GrayPage, delta: Rotation) -> GrayPage {
    let delta = delta % 360;
    let (w, h) = (image.width as usize, image.height as usize);
    let src = &image.pixels;
    let (nw, nh, pixels) = match delta {
        90 => {
            // (x, y) → (h - 1 - y, x)
            let mut out = vec![0u8; w * h];
            for y in 0..h {
                for x in 0..w {
                    out[x * h + (h - 1 - y)] = src[y * w + x];
                }
            }
            (h, w, out)
        }
        180 => (w, h, src.iter().rev().copied().collect()),
        270 => {
            // (x, y) → (y, w - 1 - x)
            let mut out = vec![0u8; w * h];
            for y in 0..h {
                for x in 0..w {
                    out[(w - 1 - x) * h + y] = src[y * w + x];
                }
            }
            (h, w, out)
        }
        _ => (w, h, src.clone()),
    };
    GrayPage {
        page: image.page,
        dpi: image.dpi,
        pixels,
        width: nw as u32,
        height: nh as u32,
        rotation: (image.rotation + delta) % 360,
        render_ms: image.render_ms,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(rotation: Rotation, confidence: f32, words: u32) -> OrientationScore {
        OrientationScore {
            rotation,
            confidence,
            words,
        }
    }

    #[test]
    fn a_clear_winner_turns_the_page() {
        let scores = [
            s(0, 40.0, 12),
            s(90, 91.0, 60),
            s(180, 35.0, 9),
            s(270, 42.0, 14),
        ];
        assert_eq!(pick(&scores), 90);
    }

    #[test]
    fn an_upright_page_stays() {
        let scores = [
            s(0, 92.0, 80),
            s(90, 30.0, 5),
            s(180, 45.0, 20),
            s(270, 33.0, 6),
        ];
        assert_eq!(pick(&scores), 0);
    }

    #[test]
    fn a_close_call_leaves_the_page_alone() {
        // 180 is best but within 15 % of the page as it is.
        let scores = [
            s(0, 80.0, 40),
            s(90, 20.0, 4),
            s(180, 88.0, 40),
            s(270, 10.0, 3),
        ];
        assert_eq!(pick(&scores), 0);
        // …and within 15 % of the runner-up, even when the page as it is read nothing.
        let scores = [
            s(0, 0.0, 0),
            s(90, 80.0, 10),
            s(180, 20.0, 5),
            s(270, 85.0, 10),
        ];
        assert_eq!(pick(&scores), 0);
    }

    #[test]
    fn too_few_words_is_no_evidence() {
        let scores = [s(0, 0.0, 0), s(90, 95.0, 2), s(180, 0.0, 0), s(270, 0.0, 0)];
        assert_eq!(pick(&scores), 0);
        assert_eq!(pick(&[]), 0);
    }

    #[test]
    fn directions_vote_by_characters() {
        use super::super::vision::TextDirection;
        let d = |angle: f64, chars: u32| TextDirection {
            angle,
            words: chars / 4,
            chars,
        };
        assert_eq!(quarter(1.5), 0);
        assert_eq!(quarter(89.0), 90);
        assert_eq!(quarter(-179.0), 180);
        assert_eq!(quarter(-91.0), 270);
        // A sideways page: most lines read bottom to top, a stray label reads upright.
        let scores = scores_from_directions(&[d(90.2, 40), d(89.7, 36), d(91.0, 20), d(0.3, 4)]);
        assert_eq!(scores[1].rotation, 90);
        assert!((scores[1].confidence - 96.0).abs() < 1e-3);
        assert_eq!(scores[1].words, 10 + 9 + 5);
        assert_eq!(pick(&scores), 90);
        assert_eq!(pick(&scores_from_directions(&[d(0.0, 40), d(-0.5, 30)])), 0);
        assert_eq!(pick(&scores_from_directions(&[])), 0);
    }

    #[test]
    fn rotation_moves_pixels_clockwise() {
        // 3×2:  a b c
        //       d e f
        let image = GrayPage {
            page: 0,
            dpi: 100,
            pixels: vec![1, 2, 3, 4, 5, 6],
            width: 3,
            height: 2,
            rotation: 90,
            render_ms: 0.0,
        };
        let r90 = rotate_gray(&image, 90);
        // clockwise: d a / e b / f c
        assert_eq!((r90.width, r90.height), (2, 3));
        assert_eq!(r90.pixels, vec![4, 1, 5, 2, 6, 3]);
        assert_eq!(r90.rotation, 180);
        let r180 = rotate_gray(&image, 180);
        assert_eq!(r180.pixels, vec![6, 5, 4, 3, 2, 1]);
        assert_eq!(r180.rotation, 270);
        let r270 = rotate_gray(&image, 270);
        // counter-clockwise: c f / b e / a d
        assert_eq!(r270.pixels, vec![3, 6, 2, 5, 1, 4]);
        assert_eq!(r270.rotation, 0);
        assert_eq!(rotate_gray(&r90, 270).pixels, image.pixels);
    }
}
