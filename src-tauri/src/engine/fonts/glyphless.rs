//! The glyphless CID font of the invisible OCR text layer (v0.3 O5, pkg7-ocr).
//!
//! Before this, every non-Latin OCR word was written in the bundled Hangul subset, which embeds
//! the whole ~487 KB file into the document the first time a Korean scan was recognised — and a
//! render mode 3 (`3 Tr`) run never paints a glyph, so none of those bytes were ever used for
//! anything but the character → Unicode mapping and the advance widths. It also meant a
//! Japanese or Chinese character outside KS X 1001 had no glyph and was silently dropped.
//!
//! This is what Tesseract's own PDF renderer does instead: a **tiny TrueType font with no real
//! glyphs** (built here, ~0.6 KB), loaded with `FPDFText_LoadCidType2Font`
//! as a `Type0` / `Identity-H` font whose `/ToUnicode` and `/CIDToGIDMap` we write ourselves:
//!
//! ```text
//! the characters of one `ocr_apply` batch, sorted     → CID 1, 2, 3, …   (CID 0 is never used:
//!                                                        PDFium's reverse lookup answers 0 for
//!                                                        "not found")
//! /ToUnicode   <0001> <0020> · <0002> <AC80> · …       → one bfchar per used character
//! /CIDToGIDMap CID → GID 1 (1 em) or GID 2 (½ em)      → PDFium derives /W from it, so a Latin
//!                                                        letter inside a Korean word is ½ em
//! ```
//!
//! `FPDFText_SetText` finds each character's code through that `/ToUnicode` (`CPDF_Font::
//! CharCodeFromUnicode` → `ReverseLookup`), and every extractor — PDFium's, Preview's, Acrobat's
//! — reads it back through the same map. The glyphs themselves are plain rectangles filling the
//! advance box: never painted in mode 3, but a valid bbox gives every character a real box for
//! selection and search highlights (an outline-less glyph has an empty one).
//!
//! **One font per call.** The `/ToUnicode` is fixed when the font is loaded, so the batch's
//! characters must be known first; `ocr_apply` collects them before it writes a word. PDFium's
//! reverse lookup is a linear scan of the map, which is why the map holds only the characters
//! actually used (a few hundred for a dense Korean page) rather than a fixed repertoire.
//! Characters outside the Basic Multilingual Plane become U+FFFD: `FPDFText_SetText` splits them
//! into surrogates on Windows (16-bit `wchar_t`) and the map could not be found either way.
//!
//! **`unsafe`.** pdfium-render 0.9.4 has no wrapper for `FPDFText_LoadCidType2Font`, and a
//! `PdfFontToken` cannot be made from a raw `FPDF_FONT` outside the crate, so the text objects of
//! this font are created through the raw bindings too. The rules of `engine::raw` apply: handles
//! come from live Rust values on the engine thread, are never stored beyond the call that uses
//! them, and every `unsafe` block is a single FFI call.

use crate::ipc::{EngineError, ErrorCode};
use pdfium_render::prelude::{
    PdfDocument, PdfPage, PdfiumLibraryBindings, FPDF_FONT, FPDF_PAGEOBJECT, FPDF_WIDESTRING,
};
use std::collections::BTreeMap;
use std::os::raw::c_float;
use std::sync::OnceLock;

/// The PostScript name written into the font (PDFium derives `/BaseFont` from it).
pub const GLYPHLESS_POSTSCRIPT: &str = "SeePDF-Glyphless";
const FAMILY: &str = "SeePDF Glyphless";

/// Design units per em; the advances below are in these.
const UPM: u16 = 1000;
const ASCENT: i16 = 880;
const DESCENT: i16 = -120;
/// GID 1: full-width (Hangul, kana, CJK ideographs, fullwidth forms).
const GID_FULL: u16 = 1;
/// GID 2: half-width (Latin, digits, punctuation, the space, halfwidth forms).
const GID_HALF: u16 = 2;
const ADVANCE_FULL: u16 = 1000;
const ADVANCE_HALF: u16 = 500;

/// `FPDF_TEXTRENDERMODE_INVISIBLE`.
const RENDER_MODE_INVISIBLE: i32 = 3;

// ---------------------------------------------------------------------------------------
// The font file
// ---------------------------------------------------------------------------------------

/// The glyphless TrueType, built once per process. Three glyphs: `.notdef` (empty), a 1 em box
/// and a ½ em box; no `cmap` entries beyond the space, because the CID font never consults it.
pub fn font_bytes() -> &'static [u8] {
    static BYTES: OnceLock<Vec<u8>> = OnceLock::new();
    BYTES.get_or_init(build_font)
}

fn be16(out: &mut Vec<u8>, v: u16) {
    out.extend_from_slice(&v.to_be_bytes());
}

fn be16i(out: &mut Vec<u8>, v: i16) {
    out.extend_from_slice(&v.to_be_bytes());
}

fn be32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_be_bytes());
}

fn checksum(data: &[u8]) -> u32 {
    data.chunks(4).fold(0u32, |sum, chunk| {
        let mut word = [0u8; 4];
        word[..chunk.len()].copy_from_slice(chunk);
        sum.wrapping_add(u32::from_be_bytes(word))
    })
}

/// A rectangle glyph `0..advance × DESCENT..ASCENT`, one contour of four on-curve points.
fn box_glyph(advance: u16) -> Vec<u8> {
    let mut g = Vec::new();
    be16i(&mut g, 1); // numberOfContours
    be16i(&mut g, 0); // xMin
    be16i(&mut g, DESCENT);
    be16i(&mut g, advance as i16); // xMax
    be16i(&mut g, ASCENT);
    be16(&mut g, 3); // endPtsOfContours[0]
    be16(&mut g, 0); // instructionLength
    g.extend_from_slice(&[0x01; 4]); // on-curve, 16-bit signed deltas
    let w = advance as i16;
    let h = ASCENT - DESCENT;
    for dx in [0, w, 0, -w] {
        be16i(&mut g, dx);
    }
    for dy in [DESCENT, 0, h, 0] {
        be16i(&mut g, dy);
    }
    g
}

fn utf16be(s: &str) -> Vec<u8> {
    s.encode_utf16().flat_map(|u| u.to_be_bytes()).collect()
}

fn name_table() -> Vec<u8> {
    let names: [(u16, &str); 5] = [
        (1, FAMILY),
        (2, "Regular"),
        (3, GLYPHLESS_POSTSCRIPT),
        (4, FAMILY),
        (6, GLYPHLESS_POSTSCRIPT),
    ];
    let mut records = Vec::new();
    let mut strings = Vec::new();
    // Windows Unicode BMP (3, 1, 0x409) and Macintosh Roman (1, 0, 0) — sorted by platform.
    let mut entries: Vec<(u16, u16, u16, u16, Vec<u8>)> = Vec::new();
    for (id, s) in names {
        entries.push((1, 0, 0, id, s.as_bytes().to_vec()));
    }
    for (id, s) in names {
        entries.push((3, 1, 0x409, id, utf16be(s)));
    }
    for (platform, encoding, language, id, bytes) in &entries {
        be16(&mut records, *platform);
        be16(&mut records, *encoding);
        be16(&mut records, *language);
        be16(&mut records, *id);
        be16(&mut records, bytes.len() as u16);
        be16(&mut records, strings.len() as u16);
        strings.extend_from_slice(bytes);
    }
    let mut t = Vec::new();
    be16(&mut t, 0); // format
    be16(&mut t, entries.len() as u16);
    be16(&mut t, (6 + records.len()) as u16); // stringOffset
    t.extend_from_slice(&records);
    t.extend_from_slice(&strings);
    t
}

fn build_font() -> Vec<u8> {
    let glyphs: [Vec<u8>; 3] = [Vec::new(), box_glyph(ADVANCE_FULL), box_glyph(ADVANCE_HALF)];
    let advances = [ADVANCE_FULL, ADVANCE_FULL, ADVANCE_HALF];

    let mut glyf = Vec::new();
    let mut loca = Vec::new();
    for g in &glyphs {
        be16(&mut loca, (glyf.len() / 2) as u16); // short offsets: bytes / 2
        glyf.extend_from_slice(g);
    }
    be16(&mut loca, (glyf.len() / 2) as u16);

    let mut head = Vec::new();
    be32(&mut head, 0x0001_0000); // version
    be32(&mut head, 0x0001_0000); // fontRevision
    be32(&mut head, 0); // checkSumAdjustment, patched below
    be32(&mut head, 0x5F0F_3CF5); // magicNumber
    be16(&mut head, 0x000B); // flags: baseline at y=0, lsb at x=0, integer ppem
    be16(&mut head, UPM);
    head.extend_from_slice(&[0u8; 16]); // created, modified
    be16i(&mut head, 0); // xMin
    be16i(&mut head, DESCENT);
    be16i(&mut head, ADVANCE_FULL as i16);
    be16i(&mut head, ASCENT);
    be16(&mut head, 0); // macStyle
    be16(&mut head, 8); // lowestRecPPEM
    be16i(&mut head, 2); // fontDirectionHint
    be16i(&mut head, 0); // indexToLocFormat: short
    be16i(&mut head, 0); // glyphDataFormat

    let mut hhea = Vec::new();
    be32(&mut hhea, 0x0001_0000);
    be16i(&mut hhea, ASCENT);
    be16i(&mut hhea, DESCENT);
    be16i(&mut hhea, 0); // lineGap
    be16(&mut hhea, ADVANCE_FULL); // advanceWidthMax
    be16i(&mut hhea, 0); // minLeftSideBearing
    be16i(&mut hhea, 0); // minRightSideBearing
    be16i(&mut hhea, ADVANCE_FULL as i16); // xMaxExtent
    be16i(&mut hhea, 1); // caretSlopeRise
    be16i(&mut hhea, 0); // caretSlopeRun
    be16i(&mut hhea, 0); // caretOffset
    hhea.extend_from_slice(&[0u8; 8]); // reserved
    be16i(&mut hhea, 0); // metricDataFormat
    be16(&mut hhea, glyphs.len() as u16); // numberOfHMetrics

    let mut hmtx = Vec::new();
    for advance in advances {
        be16(&mut hmtx, advance);
        be16i(&mut hmtx, 0);
    }

    let mut maxp = Vec::new();
    be32(&mut maxp, 0x0001_0000);
    be16(&mut maxp, glyphs.len() as u16); // numGlyphs
    be16(&mut maxp, 4); // maxPoints
    be16(&mut maxp, 1); // maxContours
    be16(&mut maxp, 0); // maxCompositePoints
    be16(&mut maxp, 0); // maxCompositeContours
    be16(&mut maxp, 2); // maxZones
    for _ in 0..8 {
        be16(&mut maxp, 0); // twilight points … component depth
    }

    // cmap: one format-4 subtable (3, 1) mapping only U+0020 → GID 2, plus the 0xFFFF sentinel.
    let mut cmap = Vec::new();
    be16(&mut cmap, 0); // version
    be16(&mut cmap, 1); // numTables
    be16(&mut cmap, 3);
    be16(&mut cmap, 1);
    be32(&mut cmap, 12); // offset of the subtable
    let seg_count: u16 = 2;
    be16(&mut cmap, 4); // format
    be16(&mut cmap, 16 + 8 * seg_count); // length
    be16(&mut cmap, 0); // language
    be16(&mut cmap, seg_count * 2);
    be16(&mut cmap, 4); // searchRange = 2 * 2^floor(log2(segCount))
    be16(&mut cmap, 1); // entrySelector = floor(log2(segCount))
    be16(&mut cmap, 0); // rangeShift = 2 * segCount - searchRange
    be16(&mut cmap, 0x0020); // endCode[0]
    be16(&mut cmap, 0xFFFF); // endCode[1]
    be16(&mut cmap, 0); // reservedPad
    be16(&mut cmap, 0x0020); // startCode[0]
    be16(&mut cmap, 0xFFFF); // startCode[1]
    be16i(&mut cmap, (GID_HALF as i32 - 0x20) as i16); // idDelta[0]
    be16i(&mut cmap, 1); // idDelta[1]
    be16(&mut cmap, 0); // idRangeOffset[0]
    be16(&mut cmap, 0); // idRangeOffset[1]

    let mut post = Vec::new();
    be32(&mut post, 0x0003_0000); // version 3: no glyph names
    be32(&mut post, 0); // italicAngle
    be16i(&mut post, -100); // underlinePosition
    be16i(&mut post, 50); // underlineThickness
    post.extend_from_slice(&[0u8; 20]); // isFixedPitch + the four memory fields

    let mut os2 = Vec::new();
    be16(&mut os2, 4); // version
    be16i(&mut os2, ((ADVANCE_FULL + ADVANCE_HALF) / 2) as i16); // xAvgCharWidth
    be16(&mut os2, 400); // usWeightClass
    be16(&mut os2, 5); // usWidthClass
    be16(&mut os2, 0); // fsType: installable (nothing to protect)
    for v in [650i16, 600, 0, 75, 650, 600, 0, 350, 50, 250] {
        be16i(&mut os2, v); // sub/superscript sizes and offsets, strikeout size/position
    }
    be16i(&mut os2, 0); // sFamilyClass
    os2.extend_from_slice(&[0u8; 10]); // panose
    os2.extend_from_slice(&[0u8; 16]); // ulUnicodeRange1-4
    os2.extend_from_slice(b"SPDF"); // achVendID
    be16(&mut os2, 0x0040); // fsSelection: REGULAR
    be16(&mut os2, 0x0020); // usFirstCharIndex
    be16(&mut os2, 0xFFFF); // usLastCharIndex
    be16i(&mut os2, ASCENT); // sTypoAscender
    be16i(&mut os2, DESCENT); // sTypoDescender
    be16i(&mut os2, 0); // sTypoLineGap
    be16(&mut os2, ASCENT as u16); // usWinAscent
    be16(&mut os2, (-DESCENT) as u16); // usWinDescent
    os2.extend_from_slice(&[0u8; 8]); // ulCodePageRange1-2
    be16i(&mut os2, 500); // sxHeight
    be16i(&mut os2, 700); // sCapHeight
    be16(&mut os2, 0); // usDefaultChar
    be16(&mut os2, 0x20); // usBreakChar
    be16(&mut os2, 1); // usMaxContext

    let name = name_table();

    // Tables sorted by tag, as the directory requires.
    let tables: Vec<(&[u8; 4], Vec<u8>)> = vec![
        (b"OS/2", os2),
        (b"cmap", cmap),
        (b"glyf", glyf),
        (b"head", head),
        (b"hhea", hhea),
        (b"hmtx", hmtx),
        (b"loca", loca),
        (b"maxp", maxp),
        (b"name", name),
        (b"post", post),
    ];
    let num_tables = tables.len() as u16;
    let entry_selector = 15 - num_tables.leading_zeros() as u16; // floor(log2)
    let search_range = (1u16 << entry_selector) * 16;

    let mut out = Vec::new();
    be32(&mut out, 0x0001_0000); // sfntVersion: TrueType
    be16(&mut out, num_tables);
    be16(&mut out, search_range);
    be16(&mut out, entry_selector);
    be16(&mut out, num_tables * 16 - search_range);

    let mut offset = 12 + 16 * tables.len();
    let mut body = Vec::new();
    let mut head_offset = 0usize;
    for (tag, data) in &tables {
        out.extend_from_slice(*tag);
        be32(&mut out, checksum(data));
        be32(&mut out, offset as u32);
        be32(&mut out, data.len() as u32);
        if *tag == b"head" {
            head_offset = offset;
        }
        let mut padded = data.clone();
        while padded.len() % 4 != 0 {
            padded.push(0);
        }
        offset += padded.len();
        body.extend_from_slice(&padded);
    }
    out.extend_from_slice(&body);
    let adjustment = 0xB1B0_AFBAu32.wrapping_sub(checksum(&out));
    out[head_offset + 8..head_offset + 12].copy_from_slice(&adjustment.to_be_bytes());
    out
}

// ---------------------------------------------------------------------------------------
// The character map of one batch
// ---------------------------------------------------------------------------------------

/// Half-width characters get the ½ em glyph: everything before the Hangul Jamo block (Latin,
/// Greek, Cyrillic, general punctuation is further up but narrow too) and the halfwidth forms.
fn is_half_width(c: char) -> bool {
    let cp = c as u32;
    cp < 0x1100
        || (0x2000..=0x206F).contains(&cp) // general punctuation
        || (0x20A0..=0x20CF).contains(&cp) // currency symbols
        || (0xFF61..=0xFFDC).contains(&cp) // halfwidth katakana / Hangul
        || (0xFFE8..=0xFFEE).contains(&cp)
}

/// What the text layer writes instead of a character PDFium cannot round-trip.
fn representable(c: char) -> char {
    if (c as u32) > 0xFFFF || c == '\0' {
        '\u{FFFD}'
    } else {
        c
    }
}

/// The CIDs of one batch: each distinct character used, in code-point order, from CID 1.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CodeMap {
    codes: BTreeMap<char, u16>,
}

impl CodeMap {
    /// Every character of `texts`, plus the space (the word-space separator and PDFium's own
    /// word-break threshold both look it up).
    pub fn new<'a>(texts: impl IntoIterator<Item = &'a str>) -> Result<Self, EngineError> {
        let mut chars: std::collections::BTreeSet<char> = std::collections::BTreeSet::new();
        chars.insert(' ');
        for text in texts {
            chars.extend(text.chars().map(representable));
        }
        if chars.len() >= u16::MAX as usize {
            return Err(EngineError::invalid(format!(
                "the OCR batch uses {} distinct characters; a CID font holds 65 534",
                chars.len()
            )));
        }
        let codes = chars
            .into_iter()
            .enumerate()
            .map(|(i, c)| (c, (i + 1) as u16))
            .collect();
        Ok(Self { codes })
    }

    pub fn len(&self) -> usize {
        self.codes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.codes.is_empty()
    }

    /// The CID of `c`, if the batch uses it.
    pub fn cid(&self, c: char) -> Option<u16> {
        self.codes.get(&representable(c)).copied()
    }

    /// `text` as the font can write it (non-BMP characters → U+FFFD).
    pub fn encodable(text: &str) -> String {
        text.chars().map(representable).collect()
    }

    /// The `/ToUnicode` CMap: one `bfchar` per used CID, in blocks of at most 100.
    pub fn to_unicode_cmap(&self) -> String {
        let mut s = String::from(
            "/CIDInit /ProcSet findresource begin\n12 dict begin\nbegincmap\n\
             /CIDSystemInfo << /Registry (Adobe) /Ordering (UCS) /Supplement 0 >> def\n\
             /CMapName /SeePDF-Glyphless-UCS def\n/CMapType 2 def\n\
             1 begincodespacerange\n<0000> <FFFF>\nendcodespacerange\n",
        );
        let entries: Vec<(&char, &u16)> = self.codes.iter().collect();
        for block in entries.chunks(100) {
            s.push_str(&format!("{} beginbfchar\n", block.len()));
            for (c, cid) in block {
                s.push_str(&format!("<{:04X}> <{:04X}>\n", cid, **c as u32));
            }
            s.push_str("endbfchar\n");
        }
        s.push_str("endcmap\nCMapName currentdict /CMap defineresource pop\nend\nend\n");
        s
    }

    /// The `/CIDToGIDMap` stream: two big-endian bytes per CID from 0 to the last one used.
    /// PDFium builds `/W` from it and the font's advances.
    pub fn cid_to_gid_map(&self) -> Vec<u8> {
        let last = self.codes.values().copied().max().unwrap_or(0) as usize;
        let mut map = vec![0u8; (last + 1) * 2];
        for (c, cid) in &self.codes {
            let gid = if is_half_width(*c) {
                GID_HALF
            } else {
                GID_FULL
            };
            let at = *cid as usize * 2;
            map[at..at + 2].copy_from_slice(&gid.to_be_bytes());
        }
        map
    }
}

// ---------------------------------------------------------------------------------------
// PDFium
// ---------------------------------------------------------------------------------------

fn pdfium_error(what: &str) -> EngineError {
    EngineError::new(ErrorCode::Pdfium, format!("{what} failed"))
}

/// The font loaded into one document for one batch. Dropping it releases our reference
/// (`FPDFFont_Close`); the text objects created from it keep the font alive in the document.
pub struct GlyphlessFont {
    handle: FPDF_FONT,
    bindings: &'static dyn PdfiumLibraryBindings,
    map: CodeMap,
}

impl GlyphlessFont {
    /// `FPDFText_LoadCidType2Font` with this batch's `/ToUnicode` and `/CIDToGIDMap`.
    pub fn load(
        bindings: &'static dyn PdfiumLibraryBindings,
        doc: &PdfDocument<'_>,
        map: CodeMap,
    ) -> Result<Self, EngineError> {
        let font = font_bytes();
        let cmap = map.to_unicode_cmap();
        let cid_to_gid = map.cid_to_gid_map();
        // SAFETY: the document is live on the engine thread; PDFium copies all three buffers.
        let handle = unsafe {
            bindings.FPDFText_LoadCidType2Font(
                doc.raw_handle(),
                font.as_ptr(),
                font.len() as u32,
                &cmap,
                cid_to_gid.as_ptr(),
                cid_to_gid.len() as u32,
            )
        };
        if handle.is_null() {
            return Err(pdfium_error("FPDFText_LoadCidType2Font"));
        }
        Ok(Self {
            handle,
            bindings,
            map,
        })
    }

    pub fn map(&self) -> &CodeMap {
        &self.map
    }

    /// A new, unplaced invisible (`3 Tr`) text object of `text` at `size` points.
    pub fn text_object(
        &self,
        doc: &PdfDocument<'_>,
        text: &str,
        size: f32,
    ) -> Result<InvisibleText, EngineError> {
        let b = self.bindings;
        // SAFETY: document and font are live; the object is ours until inserted.
        let handle = unsafe { b.FPDFPageObj_CreateTextObj(doc.raw_handle(), self.handle, size) };
        if handle.is_null() {
            return Err(pdfium_error("FPDFPageObj_CreateTextObj"));
        }
        let object = InvisibleText {
            handle,
            bindings: b,
        };
        object.set_text(text)?;
        // SAFETY: a live, unowned text object.
        let ok = unsafe { b.FPDFTextObj_SetTextRenderMode(handle, RENDER_MODE_INVISIBLE) };
        if !b.is_true(ok) {
            return Err(pdfium_error("FPDFTextObj_SetTextRenderMode"));
        }
        Ok(object)
    }
}

impl Drop for GlyphlessFont {
    fn drop(&mut self) {
        // SAFETY: our own reference from `FPDFText_LoadCidType2Font`; objects hold their own.
        unsafe { self.bindings.FPDFFont_Close(self.handle) };
    }
}

/// A text object not yet on a page; destroyed on drop unless [`InvisibleText::insert`] moved
/// it into one.
pub struct InvisibleText {
    handle: FPDF_PAGEOBJECT,
    bindings: &'static dyn PdfiumLibraryBindings,
}

impl InvisibleText {
    /// `FPDFText_SetText` (UTF-16LE, NUL-terminated).
    pub fn set_text(&self, text: &str) -> Result<(), EngineError> {
        let wide: Vec<u16> = CodeMap::encodable(text)
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        // SAFETY: `wide` is NUL-terminated and outlives the call; PDFium copies it.
        let ok = unsafe {
            self.bindings
                .FPDFText_SetText(self.handle, wide.as_ptr() as FPDF_WIDESTRING)
        };
        if self.bindings.is_true(ok) {
            Ok(())
        } else {
            Err(pdfium_error("FPDFText_SetText"))
        }
    }

    /// The object's width in points (`FPDFPageObj_GetBounds`), before any transform.
    pub fn width(&self) -> Result<f32, EngineError> {
        let (mut l, mut b, mut r, mut t): (c_float, c_float, c_float, c_float) =
            (0.0, 0.0, 0.0, 0.0);
        // SAFETY: a live object and four locals.
        let ok = unsafe {
            self.bindings
                .FPDFPageObj_GetBounds(self.handle, &mut l, &mut b, &mut r, &mut t)
        };
        if self.bindings.is_true(ok) {
            Ok((r - l).abs())
        } else {
            Err(pdfium_error("FPDFPageObj_GetBounds"))
        }
    }

    /// Post-multiplies `[a b c d e f]` (`FPDFPageObj_Transform`).
    pub fn transform(&self, m: [f64; 6]) {
        // SAFETY: a live object.
        unsafe {
            self.bindings
                .FPDFPageObj_Transform(self.handle, m[0], m[1], m[2], m[3], m[4], m[5])
        };
    }

    /// Moves the object onto `page` (`FPDFPage_InsertObject`). The caller regenerates the
    /// page content once at the end, as for every other object of the layer.
    pub fn insert(self, page: &PdfPage<'_>) -> Result<(), EngineError> {
        let handle = self.handle;
        let bindings = self.bindings;
        // Ownership passes to the page (PDFium frees it on failure): no destroy on drop.
        std::mem::forget(self);
        // SAFETY: a live page on the engine thread and an unowned object.
        let ok = unsafe { bindings.FPDFPage_InsertObject(page.raw_handle(), handle) };
        if bindings.is_true(ok) {
            Ok(())
        } else {
            Err(pdfium_error("FPDFPage_InsertObject"))
        }
    }
}

impl Drop for InvisibleText {
    fn drop(&mut self) {
        // SAFETY: never inserted (see `insert`), so it is ours to destroy.
        unsafe { self.bindings.FPDFPageObj_Destroy(self.handle) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_font_parses_and_is_tiny() {
        let bytes = font_bytes();
        assert!(bytes.len() < 2048, "{} bytes", bytes.len());
        let face = ttf_parser::Face::parse(bytes, 0).expect("a valid TrueType");
        assert_eq!(face.number_of_glyphs(), 3);
        assert_eq!(face.units_per_em(), UPM);
        assert_eq!(
            face.glyph_hor_advance(ttf_parser::GlyphId(GID_FULL)),
            Some(ADVANCE_FULL)
        );
        assert_eq!(
            face.glyph_hor_advance(ttf_parser::GlyphId(GID_HALF)),
            Some(ADVANCE_HALF)
        );
        let bbox = face
            .glyph_bounding_box(ttf_parser::GlyphId(GID_FULL))
            .expect("the box glyph has an outline");
        assert_eq!(
            (bbox.x_max, bbox.y_min, bbox.y_max),
            (1000, DESCENT, ASCENT)
        );
        assert_eq!(face.glyph_index(' '), Some(ttf_parser::GlyphId(GID_HALF)));
        let ps = face
            .names()
            .into_iter()
            .find(|n| n.name_id == ttf_parser::name_id::POST_SCRIPT_NAME && n.is_unicode())
            .and_then(|n| n.to_string());
        assert_eq!(ps.as_deref(), Some(GLYPHLESS_POSTSCRIPT));
        // The whole-font checksum is the magic number once the adjustment is in.
        assert_eq!(checksum(bytes), 0xB1B0_AFBA);
    }

    #[test]
    fn codes_start_at_one_and_include_the_space() {
        let map = CodeMap::new(["검색 가능한", "日本語"]).unwrap();
        assert_eq!(map.cid(' '), Some(1), "the space sorts first");
        assert!(map.cid('검').unwrap() > 1);
        assert_eq!(map.len(), 1 + 5 + 3);
        assert!(map.cid('x').is_none());
        let cmap = map.to_unicode_cmap();
        assert!(cmap.contains("<0001> <0020>"));
        assert!(cmap.contains(&format!("<{:04X}> <AC80>", map.cid('검').unwrap())));
        assert!(cmap.contains("9 beginbfchar"));
    }

    #[test]
    fn cid_to_gid_gives_latin_half_and_hangul_full_width() {
        let map = CodeMap::new(["A한"]).unwrap();
        let bytes = map.cid_to_gid_map();
        assert_eq!(bytes.len(), (map.len() + 1) * 2);
        let gid = |c: char| {
            let at = map.cid(c).unwrap() as usize * 2;
            u16::from_be_bytes([bytes[at], bytes[at + 1]])
        };
        assert_eq!(gid(' '), GID_HALF);
        assert_eq!(gid('A'), GID_HALF);
        assert_eq!(gid('한'), GID_FULL);
        assert_eq!(&bytes[..2], &[0, 0], "CID 0 stays .notdef");
    }

    #[test]
    fn large_batches_split_into_blocks_of_100() {
        let text: String = (0xAC00u32..0xAC00 + 250)
            .map(|c| char::from_u32(c).unwrap())
            .collect();
        let map = CodeMap::new([text.as_str()]).unwrap();
        let cmap = map.to_unicode_cmap();
        assert_eq!(cmap.matches("beginbfchar").count(), 3);
        assert!(cmap.contains("51 beginbfchar"));
    }

    #[test]
    fn non_bmp_becomes_replacement() {
        let map = CodeMap::new(["a😀"]).unwrap();
        assert!(map.cid('\u{FFFD}').is_some());
        assert_eq!(map.cid('😀'), map.cid('\u{FFFD}'));
        assert_eq!(CodeMap::encodable("a😀"), "a\u{FFFD}");
    }
}
