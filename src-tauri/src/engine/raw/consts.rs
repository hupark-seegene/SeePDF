//! PDFium constants the 7881 bindgen does not re-export through the prelude.
//!
//! Values are copied from `vendor/pdfium-render/src/bindgen/pdfium_7881.rs`; the unit test
//! at the bottom re-checks the handful that are also visible through the crate so a bindgen
//! bump cannot silently move them.

#![allow(dead_code)]

use std::os::raw::c_int;

// --- annotation subtypes (FPDF_ANNOTATION_SUBTYPE) ---
pub const FPDF_ANNOT_UNKNOWN: c_int = 0;
pub const FPDF_ANNOT_TEXT: c_int = 1;
pub const FPDF_ANNOT_LINK: c_int = 2;
pub const FPDF_ANNOT_FREETEXT: c_int = 3;
pub const FPDF_ANNOT_LINE: c_int = 4;
pub const FPDF_ANNOT_SQUARE: c_int = 5;
pub const FPDF_ANNOT_CIRCLE: c_int = 6;
pub const FPDF_ANNOT_POLYGON: c_int = 7;
pub const FPDF_ANNOT_POLYLINE: c_int = 8;
pub const FPDF_ANNOT_HIGHLIGHT: c_int = 9;
pub const FPDF_ANNOT_UNDERLINE: c_int = 10;
pub const FPDF_ANNOT_SQUIGGLY: c_int = 11;
pub const FPDF_ANNOT_STRIKEOUT: c_int = 12;
pub const FPDF_ANNOT_STAMP: c_int = 13;
pub const FPDF_ANNOT_CARET: c_int = 14;
pub const FPDF_ANNOT_INK: c_int = 15;
pub const FPDF_ANNOT_POPUP: c_int = 16;
pub const FPDF_ANNOT_FILEATTACHMENT: c_int = 17;
pub const FPDF_ANNOT_WIDGET: c_int = 20;
pub const FPDF_ANNOT_REDACT: c_int = 26;

// --- annotation colour types (FPDFANNOT_COLORTYPE) ---
/// `/C` — stroke / markup colour.
pub const FPDFANNOT_COLORTYPE_COLOR: c_int = 0;
/// `/IC` — interior colour.
pub const FPDFANNOT_COLORTYPE_INTERIORCOLOR: c_int = 1;

// --- appearance modes (FPDF_ANNOT_APPEARANCEMODE) ---
pub const FPDF_ANNOT_APPEARANCEMODE_NORMAL: c_int = 0;
pub const FPDF_ANNOT_APPEARANCEMODE_ROLLOVER: c_int = 1;
pub const FPDF_ANNOT_APPEARANCEMODE_DOWN: c_int = 2;

// --- annotation flags (/F) ---
pub const FPDF_ANNOT_FLAG_NONE: c_int = 0;
pub const FPDF_ANNOT_FLAG_INVISIBLE: c_int = 1;
pub const FPDF_ANNOT_FLAG_HIDDEN: c_int = 2;
pub const FPDF_ANNOT_FLAG_PRINT: c_int = 4;
pub const FPDF_ANNOT_FLAG_NOZOOM: c_int = 8;
pub const FPDF_ANNOT_FLAG_NOROTATE: c_int = 16;
pub const FPDF_ANNOT_FLAG_NOVIEW: c_int = 32;
pub const FPDF_ANNOT_FLAG_READONLY: c_int = 64;
pub const FPDF_ANNOT_FLAG_LOCKED: c_int = 128;
pub const FPDF_ANNOT_FLAG_TOGGLENOVIEW: c_int = 256;

// --- render flags ---
pub const FPDF_ANNOT: c_int = 0x01;
pub const FPDF_LCD_TEXT: c_int = 0x02;
pub const FPDF_GRAYSCALE: c_int = 0x08;
pub const FPDF_PRINTING: c_int = 0x800;
pub const FPDF_REVERSE_BYTE_ORDER: c_int = 0x10;

// --- FPDFPage_Flatten ---
/// Flatten for display: keeps every annotation that is visible on screen.
pub const FLAT_NORMALDISPLAY: c_int = 0;
/// Flatten for print: **deletes** annotations without the Print flag (annotations spike §3.3).
pub const FLAT_PRINT: c_int = 1;
pub const FLATTEN_FAIL: c_int = 0;
pub const FLATTEN_SUCCESS: c_int = 1;
pub const FLATTEN_NOTHINGTODO: c_int = 2;

// --- FPDF_SaveAsCopy flags ---
pub const FPDF_INCREMENTAL: u32 = 1;
pub const FPDF_NO_INCREMENTAL: u32 = 2;
/// Deprecated value — **not** the one to use.
pub const FPDF_REMOVE_SECURITY_DEPRECATED: u32 = 3;
/// The value that actually removes security in build 8057 (pages spike §8).
pub const FPDF_REMOVE_SECURITY: u32 = 4;

// --- bitmap formats ---
pub const FPDFBITMAP_GRAY: c_int = 1;
pub const FPDFBITMAP_BGR: c_int = 2;
pub const FPDFBITMAP_BGRX: c_int = 3;
pub const FPDFBITMAP_BGRA: c_int = 4;

#[cfg(test)]
mod tests {
    use super::*;

    /// The prelude does not re-export PDFium's constants, so these values are copied from
    /// `bindgen/pdfium_7881.rs`. Pin them here: a bindgen bump that renumbers anything has
    /// to fail a test rather than silently corrupt a document.
    #[test]
    fn constants_are_the_7881_values() {
        assert_eq!(FPDF_ANNOT_CIRCLE, 6);
        assert_eq!(FPDF_ANNOT_INK, 15);
        assert_eq!(FPDF_ANNOT_WIDGET, 20);
        assert_eq!(FPDF_ANNOT_REDACT, 26);
        assert_eq!(FPDFANNOT_COLORTYPE_INTERIORCOLOR, 1);
        assert_eq!(FPDF_ANNOT_APPEARANCEMODE_NORMAL, 0);
        assert_eq!(FPDF_ANNOT_FLAG_PRINT, 4);
        assert_eq!(FLAT_NORMALDISPLAY, 0);
        assert_eq!(FLAT_PRINT, 1);
        assert_eq!(FPDFBITMAP_BGRA, 4);
        // 3 is FPDF_REMOVE_SECURITY_DEPRECATED; 4 is the one that works on build 8057.
        assert_eq!(FPDF_REMOVE_SECURITY, 4);
    }
}
