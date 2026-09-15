//! The bundled Hangul font: where it lives, how it is loaded, and what it covers.
//!
//! `ARCHITECTURE.md` §6.2/§6.6, `WORKPLAN.md` §5 assets. PDFium's `FPDFText_LoadFont` embeds
//! the **whole** font file with no subsetting (text spike §5), so SeePDF never embeds a host
//! font: it ships one 487 KB OFL subset, `resources/fonts/SeePDF-Hangul.ttf`, built by
//! `examples/build_hangul_font.rs` and committed.
//!
//! Two things this module owns:
//!
//! * [`load`] — read the file once per process, load it into a document once per document
//!   (a second `load_true_type_from_bytes` would embed a second copy of the font);
//! * [`covers`] / [`Coverage`] — the offline `ttf-parser` cmap probe. PDFium checks nothing:
//!   a glyph it cannot find is silently dropped, which is how `한글 테스트` becomes
//!   `  Hangul ABC` with the wrong font (text spike §5). Checking before we embed is the only
//!   way to give the UI an honest `fontCoverage` error.

pub mod ksx1001;

use crate::ipc::error::PdfiumResultExt;
use crate::ipc::{EngineError, ErrorCode};
use pdfium_render::prelude::{PdfDocument, PdfFontToken};
use std::path::PathBuf;
use std::sync::OnceLock;

/// The family name `examples/build_hangul_font.rs` writes into the subset. It is what
/// `PdfFont::name()` reports for our own text objects, which is how `list_page_objects` marks
/// them `ours`, and what `TextEditProbe.substituteFont` promises the UI.
pub const BUNDLED_FAMILY: &str = "SeePDF Hangul";
/// `/BaseFont` of the embedded subset (the PostScript name).
pub const BUNDLED_POSTSCRIPT: &str = "SeePDF-Hangul";
pub const BUNDLED_FILE: &str = "SeePDF-Hangul.ttf";

/// `resources/fonts/SeePDF-Hangul.ttf`, wherever this build can see it.
///
/// Same search order as `app::pdfium_path`: the executable's `resources/` directory (which is
/// what `bundle.resources` produces, and what `tauri-build` copies into `target/<profile>`),
/// the macOS `.app` `Contents/Resources`, then `CARGO_MANIFEST_DIR/resources` for `cargo run`
/// and `cargo test`.
pub fn bundled_path() -> Option<PathBuf> {
    static PATH: OnceLock<Option<PathBuf>> = OnceLock::new();
    PATH.get_or_init(|| {
        let mut dirs: Vec<PathBuf> = Vec::new();
        if let Ok(exe) = std::env::current_exe() {
            for up in [1usize, 2, 3] {
                if let Some(dir) = exe.ancestors().nth(up) {
                    dirs.push(dir.join("resources").join("fonts"));
                }
            }
            if let Some(dir) = exe.parent() {
                dirs.push(
                    dir.join("..")
                        .join("Resources")
                        .join("resources")
                        .join("fonts"),
                );
            }
        }
        dirs.push(
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("resources")
                .join("fonts"),
        );
        dirs.into_iter()
            .map(|d| d.join(BUNDLED_FILE))
            .find(|p| p.is_file())
    })
    .clone()
}

/// The font file's bytes, read once per process (487 KB).
pub fn bundled_bytes() -> Result<&'static [u8], EngineError> {
    static BYTES: OnceLock<Option<Vec<u8>>> = OnceLock::new();
    BYTES
        .get_or_init(|| bundled_path().and_then(|p| std::fs::read(p).ok()))
        .as_deref()
        .ok_or_else(|| {
            EngineError::new(
                ErrorCode::FontCoverage,
                format!(
                    "resources/fonts/{BUNDLED_FILE} is missing; rebuild it with \
                     `cargo run --release --example build_hangul_font -- <OFL Korean TTF>`"
                ),
            )
        })
}

/// Loads the bundled font into `doc` as a CID font and returns its token.
///
/// **Call this at most once per document**: `FPDFText_LoadFont` appends a fresh copy of the
/// whole file to the document every time. The callers that add many objects (`add_text_object`
/// with several lines, `ocr_apply` with hundreds of words) take one token and reuse it — a
/// `PdfFontToken` is `Copy`.
///
/// Must run **before** a `PdfPage` of the same document is borrowed: `fonts_mut()` needs
/// `&mut PdfDocument`.
pub fn load(doc: &mut PdfDocument<'_>) -> Result<PdfFontToken, EngineError> {
    let bytes = bundled_bytes()?;
    doc.fonts_mut()
        .load_true_type_from_bytes(bytes, true)
        .ctx("load SeePDF-Hangul.ttf")
}

/// A `PdfFontToken` for a copy of the bundled font **already embedded in this page**, if there
/// is one.
///
/// `FPDFText_LoadFont` appends a fresh copy of the whole ~487 KB file every time it is called,
/// and PDFium does not deduplicate. `PdfPage::fonts()` walks the page's text objects and hands
/// back non-owning `PdfFont` wrappers (it does **not** close their `FPDF_FONT` on drop), and a
/// `PdfFontToken` is just that handle — so a second Korean text box on a page that already has
/// one costs nothing.
///
/// Limitation: this only sees the page it is given. Adding Korean text to several *different*
/// pages of one document still embeds one copy per page; a document-level token cache needs a
/// field on `OpenDoc` (`STAGE1B_NOTES.md` §5).
pub fn embedded_token(page: &pdfium_render::prelude::PdfPage<'_>) -> Option<PdfFontToken> {
    use pdfium_render::prelude::ToPdfFontToken;
    page.fonts()
        .into_iter()
        .find(|f| {
            let name = f.name();
            name == BUNDLED_POSTSCRIPT || name == BUNDLED_FAMILY
        })
        .map(|f| f.token())
}

/// What [`covers`] found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Coverage {
    /// Every character the font cannot draw, deduplicated, in the order they appear.
    pub missing: Vec<char>,
}

impl Coverage {
    pub fn is_complete(&self) -> bool {
        self.missing.is_empty()
    }

    /// A message the UI can put behind 자세히.
    pub fn describe(&self) -> String {
        let sample: String = self.missing.iter().take(12).collect();
        format!(
            "{} character(s) are not in the bundled font: {sample}",
            self.missing.len()
        )
    }
}

/// Does the bundled font have a glyph for every character of `text`?
///
/// The check is `ttf-parser`'s `cmap` lookup on the real file, not a guess from the code
/// point ranges: the font is rebuilt by a dev tool and this is the only thing that stays true
/// if the repertoire ever changes. Whitespace that PDFium never draws (`\n`, `\r`, `\t`) is
/// ignored.
pub fn covers(text: &str) -> Result<Coverage, EngineError> {
    let bytes = bundled_bytes()?;
    let face = ttf_parser::Face::parse(bytes, 0).map_err(|e| {
        EngineError::new(
            ErrorCode::FontCoverage,
            format!("{BUNDLED_FILE} is not a readable TrueType font: {e}"),
        )
    })?;
    let mut missing = Vec::new();
    for ch in text.chars() {
        if matches!(ch, '\n' | '\r' | '\t') || missing.contains(&ch) {
            continue;
        }
        match face.glyph_index(ch) {
            Some(gid) if gid.0 != 0 => {}
            _ => missing.push(ch),
        }
    }
    Ok(Coverage { missing })
}

/// True when every character can be drawn by a base-14 font, so nothing has to be embedded.
///
/// WinAnsi covers Latin-1 closely enough for the text SeePDF writes; anything above U+00FF
/// goes to the bundled font.
pub fn is_latin1(text: &str) -> bool {
    text.chars().all(|c| (c as u32) < 0x100)
}

/// Which font a piece of text needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pick {
    /// Base-14 Helvetica; nothing is embedded and the document does not grow.
    Helvetica,
    /// `resources/fonts/SeePDF-Hangul.ttf`.
    Bundled,
}

impl Pick {
    /// The name the contract reports (`PageObject.fontName`, `TextEditProbe.substituteFont`).
    pub fn name(self) -> &'static str {
        match self {
            Pick::Helvetica => "Helvetica",
            Pick::Bundled => BUNDLED_FAMILY,
        }
    }
}

/// Helvetica for Latin-1, the bundled subset for everything else.
pub fn pick(text: &str) -> Pick {
    if is_latin1(text) {
        Pick::Helvetica
    } else {
        Pick::Bundled
    }
}

/// [`pick`] plus a loaded token. `hangul` is the already-loaded bundled token, so a caller
/// that writes many objects embeds the file once.
pub fn token_for(
    doc: &mut PdfDocument<'_>,
    text: &str,
    hangul: &mut Option<PdfFontToken>,
) -> Result<(PdfFontToken, Pick), EngineError> {
    match pick(text) {
        Pick::Helvetica => Ok((doc.fonts_mut().helvetica(), Pick::Helvetica)),
        Pick::Bundled => {
            let coverage = covers(text)?;
            if !coverage.is_complete() {
                return Err(EngineError::new(
                    ErrorCode::FontCoverage,
                    coverage.describe(),
                ));
            }
            let token = match hangul {
                Some(token) => *token,
                None => {
                    let token = load(doc)?;
                    *hangul = Some(token);
                    token
                }
            };
            Ok((token, Pick::Bundled))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn latin1_and_pick() {
        assert!(is_latin1("SeePDF text box"));
        assert!(is_latin1("caf\u{e9}"));
        assert!(!is_latin1("한글 텍스트"));
        assert_eq!(pick("plain ascii"), Pick::Helvetica);
        assert_eq!(pick("한글"), Pick::Bundled);
    }

    #[test]
    fn bundled_font_covers_the_ksx1001_repertoire() {
        // Skipped rather than failed when the font has not been built in this checkout, so a
        // fresh clone without the asset still gets a green `cargo test`.
        let Ok(bytes) = bundled_bytes() else {
            eprintln!("skipped: resources/fonts/{BUNDLED_FILE} is not present");
            return;
        };
        let face = ttf_parser::Face::parse(bytes, 0).expect("bundled font parses");
        for cp in ksx1001::syllables() {
            let ch = char::from_u32(cp).expect("syllable");
            let gid = face.glyph_index(ch).expect("syllable has a glyph");
            assert_ne!(gid.0, 0, "U+{cp:04X} maps to .notdef");
        }
        // The one that broke AppleGothic: the space must be its own glyph with a real advance.
        let space = face.glyph_index(' ').expect("space glyph");
        assert!(face.glyph_hor_advance(space).unwrap_or(0) > 0);
        assert!(covers("한글 테스트 ABC 123").unwrap().is_complete());
    }
}
