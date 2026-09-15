//! Which font a text box (and the P2 free-text `/DA`) is drawn with.
//!
//! `/Helv` cannot render 한글 and PDFium's FreeText appearance generator only knows the
//! base-14 fonts, which is why a Korean text box is a **Stamp with a real text object**
//! (`ARCHITECTURE.md` §6.1) — and a text object needs a font that covers the string.
//!
//! Order of preference:
//! 1. Latin-1-only text → `helvetica()`, embedded nowhere, 0 bytes added to the document;
//! 2. `resources/fonts/SeePDF-Hangul.ttf` — the bundled OFL subset Stage 1 (b) produces;
//! 3. a host font that covers Hangul, as a **development fallback** so this module works
//!    before (b) lands. PDFium embeds the *whole* file (AppleGothic is 15 MB), so a document
//!    written this way is large and is not what ships; [`FontChoice::source`] says which one
//!    was used and the caller records it.

use crate::ipc::error::PdfiumResultExt;
use crate::ipc::{EngineError, ErrorCode};
use pdfium_render::prelude::{PdfDocument, PdfFontToken};
use std::path::{Path, PathBuf};

/// Where the glyphs came from, for diagnostics and for `STAGE1A_NOTES.md`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FontSource {
    /// One of the base-14 fonts; nothing is embedded.
    Helvetica,
    /// `resources/fonts/SeePDF-Hangul.ttf`.
    Bundled(PathBuf),
    /// A host font, because the bundled one is not there yet.
    System(PathBuf),
}

impl FontSource {
    pub fn label(&self) -> String {
        match self {
            FontSource::Helvetica => "Helvetica".to_string(),
            FontSource::Bundled(p) | FontSource::System(p) => p
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| "font".to_string()),
        }
    }
}

pub struct FontChoice {
    pub token: PdfFontToken,
    pub source: FontSource,
}

/// True when every character can be drawn by a base-14 font (WinAnsi coverage, near enough).
pub fn is_latin1(text: &str) -> bool {
    text.chars().all(|c| (c as u32) < 0x100)
}

/// Host fonts that cover Hangul, best first. Only used until the bundled subset exists.
fn system_candidates() -> Vec<PathBuf> {
    let paths: &[&str] = if cfg!(target_os = "macos") {
        &[
            "/System/Library/Fonts/Supplemental/AppleGothic.ttf",
            "/Library/Fonts/Arial Unicode.ttf",
            "/System/Library/Fonts/Supplemental/Arial Unicode.ttf",
        ]
    } else if cfg!(target_os = "windows") {
        &[
            "C:\\Windows\\Fonts\\malgun.ttf",
            "C:\\Windows\\Fonts\\gulim.ttc",
        ]
    } else {
        &[
            "/usr/share/fonts/truetype/nanum/NanumGothic.ttf",
            "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
        ]
    };
    paths.iter().map(PathBuf::from).collect()
}

/// `src-tauri/resources/fonts/SeePDF-Hangul.ttf`, wherever this build can see it.
pub fn bundled_hangul_path() -> Option<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            dirs.push(dir.join("resources").join("fonts"));
            // macOS bundle: <App>/Contents/MacOS/seepdf -> <App>/Contents/Resources
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
        .map(|d| d.join("SeePDF-Hangul.ttf"))
        .find(|p| p.is_file())
}

/// Picks and loads the font for `text`.
///
/// Must run **before** any `PdfPage` of the document is borrowed: `fonts_mut()` needs
/// `&mut PdfDocument` and `PdfFontToken` is `Copy`, so take the token first (annotations
/// spike §5.14).
pub fn resolve(doc: &mut PdfDocument<'_>, text: &str) -> Result<FontChoice, EngineError> {
    if is_latin1(text) {
        return Ok(FontChoice {
            token: doc.fonts_mut().helvetica(),
            source: FontSource::Helvetica,
        });
    }
    let bundled = bundled_hangul_path().map(|p| (p, true));
    let candidate = bundled.or_else(|| {
        system_candidates()
            .into_iter()
            .find(|p| p.is_file())
            .map(|p| (p, false))
    });
    let Some((path, is_bundled)) = candidate else {
        return Err(EngineError::new(
            ErrorCode::FontCoverage,
            "no font covering this text is available (resources/fonts/SeePDF-Hangul.ttf is missing \
             and no host CJK font was found)",
        ));
    };
    let token = load(doc, &path)?;
    Ok(FontChoice {
        token,
        source: if is_bundled {
            FontSource::Bundled(path)
        } else {
            FontSource::System(path)
        },
    })
}

fn load(doc: &mut PdfDocument<'_>, path: &Path) -> Result<PdfFontToken, EngineError> {
    let bytes = std::fs::read(path).map_err(|e| {
        EngineError::new(
            ErrorCode::Io,
            format!("read font {}: {e}", path.display()),
        )
    })?;
    // `is_cid_font = true`: Hangul needs the 16-bit glyph space.
    doc.fonts_mut()
        .load_true_type_from_bytes(&bytes, true)
        .ctx(&format!("load font {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn latin1_detection() {
        assert!(is_latin1("SeePDF text box"));
        assert!(is_latin1("caf\u{e9}"));
        assert!(!is_latin1("한글 텍스트"));
    }
}
