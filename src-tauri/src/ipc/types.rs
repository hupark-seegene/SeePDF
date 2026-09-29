//! The Rust half of `IPC_CONTRACT.md`, field-for-field identical to `src/ipc/types.ts`.
//!
//! Rules (contract §1): every field crosses the wire as camelCase; tagged unions use
//! `#[serde(tag = …, rename_all_fields = "camelCase")]`; every rectangle, point and ink
//! path is in **PDF user space** (points, y-up, origin bottom-left, `/Rotate` *not*
//! applied). Device pixels appear only in tile URLs, OCR boxes and the
//! `X-Image-Width/Height` headers.
//!
//! This file is frozen at the end of Stage 0 and changed only through the integrator.

#![allow(dead_code)]

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------------------
// §3 Shared types
// ---------------------------------------------------------------------------------------

/// `"d1"`, `"d2"` — process-unique, never reused.
pub type DocId = String;
/// `u32`, +1 on every mutation of that document.
pub type DocGeneration = u32;
/// 0-based.
pub type PageIndex = u16;
/// The annotation's `/NM` (uuid v4); stable across generations.
pub type AnnotId = String;
/// Page-object index; valid only within one `docGeneration`.
pub type ObjectId = u32;
pub type JobId = u64;
/// 0 | 90 | 180 | 270.
pub type Rotation = u16;
/// `[r, g, b]`, 0..255.
pub type Rgb = [u8; 3];
/// `[a, b, c, d, e, f]`.
pub type Mat6 = [f32; 6];
/// `[x, y]`.
pub type Point = [f32; 2];

/// PDF points, y-up.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Rect {
    pub l: f32,
    pub b: f32,
    pub r: f32,
    pub t: f32,
}

impl Rect {
    pub const ZERO: Rect = Rect {
        l: 0.0,
        b: 0.0,
        r: 0.0,
        t: 0.0,
    };

    pub fn new(l: f32, b: f32, r: f32, t: f32) -> Self {
        Self { l, b, r, t }
    }

    pub fn width(&self) -> f32 {
        self.r - self.l
    }

    pub fn height(&self) -> f32 {
        self.t - self.b
    }

    pub fn union(&self, other: &Rect) -> Rect {
        Rect {
            l: self.l.min(other.l),
            b: self.b.min(other.b),
            r: self.r.max(other.r),
            t: self.t.max(other.t),
        }
    }

    pub fn intersects(&self, other: &Rect) -> bool {
        self.l < other.r && other.l < self.r && self.b < other.t && other.b < self.t
    }
}

// ---------------------------------------------------------------------------------------
// §4 Documents
// ---------------------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PageGeom {
    pub index: PageIndex,
    /// Display size: the page's `/Rotate` is applied.
    pub width_pt: f32,
    /// Display size: the page's `/Rotate` is applied.
    pub height_pt: f32,
    pub rotation: Rotation,
    /// Unrotated user space — what the page→device matrix is built from.
    pub crop: Rect,
    /// From `/PageLabels` (`FPDF_GetPageLabel`); written by `set_page_labels` (P2).
    pub label: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SecurityRevision {
    Unprotected,
    R2,
    R3,
    R4,
    /// AES-256, ISO 32000-1 Adobe extension level 3 (deprecated, still in the wild).
    R5,
    /// AES-256, PDF 2.0 — what `set_password` writes.
    R6,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Permissions {
    pub print: bool,
    pub modify: bool,
    pub extract_text: bool,
    pub annotate: bool,
    pub fill_forms: bool,
    pub assemble: bool,
    pub revision: SecurityRevision,
}

impl Default for Permissions {
    fn default() -> Self {
        Self {
            print: true,
            modify: true,
            extract_text: true,
            annotate: true,
            fill_forms: true,
            assemble: true,
            revision: SecurityRevision::Unprotected,
        }
    }
}

/// `set_password`'s `permissions: Partial<Permissions>` (contract §7.5). Every flag the
/// frontend leaves out is **allowed**, and `revision` (read-only, it describes the file) is
/// ignored if sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PermissionsRequest {
    pub print: bool,
    pub modify: bool,
    pub extract_text: bool,
    pub annotate: bool,
    pub fill_forms: bool,
    pub assemble: bool,
}

impl Default for PermissionsRequest {
    fn default() -> Self {
        Self {
            print: true,
            modify: true,
            extract_text: true,
            annotate: true,
            fill_forms: true,
            assemble: true,
        }
    }
}

/// Raw `"D:YYYYMMDD…"` strings; the frontend parses them.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DocMeta {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub author: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub subject: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub keywords: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub creator: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub producer: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub created: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub modified: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DocInfo {
    pub doc_id: DocId,
    pub path: Option<String>,
    pub name: String,
    pub bytes: u64,
    pub page_count: u16,
    pub pages: Vec<PageGeom>,
    pub doc_generation: DocGeneration,
    pub dirty: bool,
    pub can_undo: bool,
    pub can_redo: bool,
    /// i18n key, e.g. `"undo.annotCreate"`.
    pub undo_label: Option<String>,
    pub redo_label: Option<String>,
    pub encrypted: bool,
    pub permissions: Permissions,
    pub has_form: bool,
    pub xfa: bool,
    pub has_outline: bool,
    pub meta: DocMeta,
    pub pdf_version: String,
    pub tagged: bool,
    /// P2 page labels: every page's `/PageLabels` label as `FPDF_GetPageLabel` reads it (`""`
    /// for a page the number tree gives no label), **absent** when no page has a label — the UI
    /// then shows plain page numbers. The same values as `pages[i].label`, as one array.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub page_labels: Option<Vec<String>>,
    // --- v0.3 pkg3-security-save-integrity ---
    /// S1: the document's digital signatures (`FPDF_GetSignatureCount`), in file order. Only
    /// **detected** — no cryptographic validation is performed. Absent when there are none.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub signatures: Vec<SignatureInfo>,
    /// S1: on a signed document, whether the next `save_document` appends an incremental
    /// update (the signed revisions stay byte-identical) instead of a full rewrite that
    /// invalidates every signature. Absent on an unsigned document.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub incremental_save: Option<bool>,
    /// S4: how many document-level attachments (`/EmbeddedFiles`) there are; absent when 0.
    #[serde(skip_serializing_if = "is_zero", default)]
    pub attachment_count: u32,
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OutlineNode {
    pub title: String,
    #[serde(default)]
    pub page: Option<PageIndex>,
    /// Stage 2 (`STAGE1C_NOTES.md` §7.1): where on the page the heading is, in PDF user space,
    /// so 목차 scrolls to the heading rather than to the top of the page. `None` when the
    /// destination is a plain page reference or a fit-to-window view, which is the common case.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub dest: Option<OutlineDest>,
    /// P2: a web link (`/A << /S /URI >>`) instead of a page; `page` is then `None`.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub url: Option<String>,
    /// P2: whether the node's children start expanded (`/Count` > 0). Read back only for a
    /// node with children; on `set_outline` a missing flag on such a node means open.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub open: Option<bool>,
    #[serde(default)]
    pub children: Vec<OutlineNode>,
}

/// A `/Dest` reduced to what a scroller can use. All three are optional because the PDF
/// "retain the current value" convention writes them as null.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OutlineDest {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub x: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub y: Option<f32>,
    /// Zoom **factor** (1.0 == 100 %), as the PDF stores it; 0 means "retain".
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub zoom: Option<f32>,
}

/// P2 page labels (`set_page_labels` / `get_page_labels`): one `/PageLabels` number-tree
/// entry. `start` is the first page of the range (0-based); it runs to the next range's start.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PageLabelRange {
    pub start: PageIndex,
    pub style: PageLabelStyle,
    /// `/P`, written before the number (`"App-"` → `App-1`).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub prefix: Option<String>,
    /// `/St`, the number of the range's first page (≥ 1; 1 when absent).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub first: Option<u32>,
}

/// `/S`: `D` decimal, `r` / `R` roman, `a` / `A` letters; `none` = no `/S` (the prefix alone).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PageLabelStyle {
    Decimal,
    Roman,
    RomanUpper,
    Alpha,
    AlphaUpper,
    #[serde(rename = "none")]
    NoNumber,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum OpenSource {
    Argv,
    MacosOpened,
    Drop,
    Dialog,
    Recent,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenRequest {
    pub path: String,
    pub source: OpenSource,
}

// ---------------------------------------------------------------------------------------
// §5 View, render control, statistics
// ---------------------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ViewportHint {
    pub doc_id: DocId,
    /// `round(zoomPercent * devicePixelRatio)`.
    pub scale_key: u32,
    pub rotation: Rotation,
    pub centre_page: PageIndex,
    pub first_page: PageIndex,
    pub last_page: PageIndex,
    pub velocity_px_per_ms: f32,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueueDepth {
    pub interactive: u32,
    pub edit: u32,
    pub prefetch: u32,
    pub thumb: u32,
    pub background: u32,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EngineStats {
    pub docs: u32,
    pub queue_depth: QueueDepth,
    pub dropped_stale: u64,
    pub tile_p50_ms: f64,
    pub tile_p95_ms: f64,
    pub encode_p50_ms: f64,
    pub tile_cache_bytes: u64,
    pub tile_cache_hit_rate: f64,
    pub page_lru_len: u32,
    pub rss_bytes: u64,
}

// ---------------------------------------------------------------------------------------
// §6 Text, selection, search
// ---------------------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchHit {
    pub page: PageIndex,
    pub char_start: u32,
    pub char_length: u32,
    /// One rect per line the hit spans.
    pub rects: Vec<Rect>,
    /// ±40 chars for the results list.
    pub context: String,
    /// `[start, length]` of the match inside `context`.
    pub context_match: [u32; 2],
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum SearchEvent {
    Page {
        page: PageIndex,
        hits: Vec<SearchHit>,
        scanned: u32,
        total: u32,
    },
    Done {
        total: u32,
        pages_scanned: u32,
        elapsed_ms: f64,
    },
    Cancelled {
        scanned: u32,
    },
    Error {
        error: super::error::EngineError,
    },
}

// ---------------------------------------------------------------------------------------
// §7.1 Annotations
// ---------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AnnotKind {
    Highlight,
    Underline,
    Strikeout,
    Squiggly,
    Ink,
    Square,
    Circle,
    Note,
    Textbox,
    Stamp,
    Signature,
    /// Stored as Ink + `/Subj "SeePDF:Line"` (PDFium cannot create `/Line`).
    Line,
    /// Stored as Ink + `/Subj "SeePDF:Arrow"`.
    Arrow,
    Link,
    // v0.3 pkg4-annotations-stamps-objects: real `/Polygon`, `/PolyLine` and `FreeText`
    // callout annotations, written with lopdf (`engine::annot::lopdf_annots`).
    Polygon,
    Polyline,
    Callout,
    Widget,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Editability {
    Full,
    /// Third-party appearance stream we would have to regenerate.
    MoveOnly,
    ReadOnly,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Annot {
    pub id: AnnotId,
    pub page: PageIndex,
    pub kind: AnnotKind,
    /// Raw PDF subtype, e.g. `"Ink"`.
    pub subtype: String,
    pub rect: Rect,
    /// Markup: one rect per line run (the engine converts to TL,TR,BL,BR quads).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub quads: Option<Vec<Rect>>,
    /// Ink / line / arrow: `[x0,y0,x1,y1,…]` per stroke.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub ink_paths: Option<Vec<Vec<f32>>>,
    /// Line / arrow convenience: `x1, y1, x2, y2`.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub line_points: Option<[f32; 4]>,
    pub color: Rgb,
    pub fill_color: Option<Rgb>,
    /// 0..1, i.e. `/CA`.
    pub opacity: f32,
    pub border_width: f32,
    pub contents: String,
    pub author: Option<String>,
    pub created: Option<String>,
    pub modified: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub font_size: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub stamp_kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub image_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub uri: Option<String>,
    /// P2 link: the go-to-page target of a Link annotation (`/Dest`, or a GoTo `/A`).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub dest: Option<LinkDest>,
    /// P2 threads: the `/NM` of the annotation this one replies to (`/IRT`, `/RT /R` or no
    /// `/RT`). Absent for a top-level annotation and for a `/RT /Group` member.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub in_reply_to: Option<AnnotId>,
    pub hidden: bool,
    pub printed: bool,
    pub locked: bool,
    pub editable: Editability,
    // v0.3 pkg4-annotations-stamps-objects ------------------------------------------
    /// Text box / callout alignment (the `SeePDFQ` mirror, or a foreign FreeText's `/Q`).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub align: Option<TextAlign>,
    /// Line / arrow heads `[start, end]`.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub heads: Option<[bool; 2]>,
    /// Polygon / polyline: `[x0,y0,x1,y1,…]` (`/Vertices`).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub vertices: Option<Vec<f32>>,
    /// Polygon: cloudy border (`/BE << /S /C >>`).
    #[serde(skip_serializing_if = "std::ops::Not::not", default)]
    pub cloudy: bool,
    /// Callout: the leader line (`/CL`), 4 or 6 numbers, the first point at the arrow tip.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub callout: Option<Vec<f32>>,
    /// Dashed border (`/BS << /S /D >>`).
    #[serde(skip_serializing_if = "std::ops::Not::not", default)]
    pub dashed: bool,
    /// Line / polygon / polyline drawn with a length or area label.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub measure: Option<MeasureUnit>,
}

/// v0.3 pkg4: the unit a measuring line / polygon labels itself in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MeasureUnit {
    Mm,
    Pt,
}

/// A go-to-page destination: the page plus the optional `/XYZ` left / top / zoom (PDF user
/// space; zoom is a factor, 1.0 = 100 %). All three absent = the whole page (`/Fit`).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LinkDest {
    pub page: PageIndex,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub x: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub y: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub zoom: Option<f32>,
}

/// A web address target (`/A << /S /URI /URI (…) >>`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LinkUrl {
    pub url: String,
}

/// `create_link` / `update_link`'s `target`: `{ page, x?, y?, zoom? }` or `{ url }`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum LinkTarget {
    Page(LinkDest),
    Url(LinkUrl),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnnotList {
    pub doc_id: DocId,
    pub page: PageIndex,
    pub doc_generation: DocGeneration,
    pub annots: Vec<Annot>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnnotResult {
    pub list: AnnotList,
    pub annot: Option<Annot>,
    pub previous: Option<Annot>,
}

// v0.3 pkg4-annotations-stamps-objects (A3 / A4): `annotation_batch` — several annotation
// edits of one gesture (a partial-eraser scrub, a pressure-banded pen stroke) as ONE undo step.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "camelCase")]
pub enum AnnotOp {
    Create {
        spec: AnnotSpec,
        #[serde(default)]
        id: Option<AnnotId>,
    },
    Update {
        id: AnnotId,
        patch: AnnotPatch,
    },
    Delete {
        ids: Vec<AnnotId>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnnotBatchResult {
    pub list: AnnotList,
    /// The `/NM` of each `create` op, in order.
    pub created: Vec<AnnotId>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MarkupSpec {
    pub rects: Vec<Rect>,
    pub color: Rgb,
    pub opacity: f32,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub contents: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NoteSpec {
    pub at: Point,
    pub color: Rgb,
    pub contents: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InkSpec {
    pub paths: Vec<Vec<f32>>,
    pub color: Rgb,
    pub width: f32,
    pub opacity: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShapeSpec {
    pub rect: Rect,
    pub color: Rgb,
    pub fill_color: Option<Rgb>,
    pub width: f32,
    pub opacity: f32,
    /// v0.3 pkg4: a dashed border (`/BS /S /D`) — a copy of a dashed shape stays dashed.
    #[serde(skip_serializing_if = "std::ops::Not::not", default)]
    pub dashed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LineSpec {
    pub p1: Point,
    pub p2: Point,
    pub color: Rgb,
    pub width: f32,
    pub opacity: f32,
    /// `[start, end]` arrow heads.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub heads: Option<[bool; 2]>,
    /// v0.3 pkg4: label the line with its length.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub measure: Option<MeasureUnit>,
    /// v0.3 pkg4: a dashed line (`/BS /S /D`) — a copy of a dashed line stays dashed.
    #[serde(skip_serializing_if = "std::ops::Not::not", default)]
    pub dashed: bool,
}

/// v0.3 pkg4: `/Polygon` and `/PolyLine` (lopdf, own appearance stream).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PolySpec {
    /// `[x0,y0,x1,y1,…]`, at least 2 points for a polyline, 3 for a polygon.
    pub vertices: Vec<f32>,
    pub color: Rgb,
    #[serde(default)]
    pub fill_color: Option<Rgb>,
    pub width: f32,
    pub opacity: f32,
    /// Polygon only: a cloudy border (`/BE << /S /C /I 1 >>`).
    #[serde(skip_serializing_if = "std::ops::Not::not", default)]
    pub cloudy: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not", default)]
    pub dashed: bool,
    /// Label the polyline's length / the polygon's area.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub measure: Option<MeasureUnit>,
}

/// v0.3 pkg4: a text box with a leader line (`FreeText /IT /FreeTextCallout /CL`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CalloutSpec {
    pub rect: Rect,
    pub text: String,
    pub font_size: f32,
    pub color: Rgb,
    pub align: TextAlign,
    pub fill_color: Option<Rgb>,
    /// `/CL`: 4 numbers (tip → box) or 6 (tip → knee → box), PDF user space.
    pub callout: Vec<f32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TextAlign {
    Left,
    Center,
    Right,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextBoxSpec {
    pub rect: Rect,
    pub text: String,
    pub font_size: f32,
    pub color: Rgb,
    pub align: TextAlign,
    pub fill_color: Option<Rgb>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", untagged)]
pub enum StampImage {
    Path {
        path: String,
    },
    Builtin {
        builtin: String,
    },
    /// v0.3 pkg4: a custom text stamp (내 도장) or a quick date mark. `{{date}}` (yyyy.MM.dd,
    /// local) and `{{author}}` (설정 ▸ 작성자) are expanded at placement.
    Text {
        text: String,
        color: Rgb,
        #[serde(default)]
        shape: StampShape,
    },
}

/// v0.3 pkg4: the border of a text stamp. `None` draws the label alone (오늘 날짜).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum StampShape {
    #[default]
    Rect,
    Round,
    None,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StampSpec {
    pub rect: Rect,
    pub image: StampImage,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub rotate: Option<f32>,
    /// Stage 8: a typed / image signature. Written with `/Subj "SeePDF:Signature"` and read
    /// back as `kind: 'signature'` (서명, not 도장). Absent = `false`.
    #[serde(skip_serializing_if = "std::ops::Not::not", default)]
    pub signature: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum AnnotSpec {
    Highlight(MarkupSpec),
    Underline(MarkupSpec),
    Strikeout(MarkupSpec),
    Squiggly(MarkupSpec),
    Note(NoteSpec),
    Ink(InkSpec),
    Signature(InkSpec),
    Square(ShapeSpec),
    Circle(ShapeSpec),
    Line(LineSpec),
    Arrow(LineSpec),
    Textbox(TextBoxSpec),
    Stamp(StampSpec),
    // v0.3 pkg4-annotations-stamps-objects
    Polygon(PolySpec),
    Polyline(PolySpec),
    Callout(CalloutSpec),
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnnotPatch {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub rect: Option<Rect>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub rects: Option<Vec<Rect>>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub paths: Option<Vec<Vec<f32>>>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub p1: Option<Point>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub p2: Option<Point>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub color: Option<Rgb>,
    /// `Some(None)` clears the interior colour; absent leaves it unchanged.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub fill_color: Option<Option<Rgb>>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub opacity: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub border_width: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub contents: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub author: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub font_size: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub locked: Option<bool>,
    // v0.3 pkg4-annotations-stamps-objects ------------------------------------------
    /// Text box / callout alignment.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub align: Option<TextAlign>,
    /// Line / arrow heads `[start, end]`; any head makes it an arrow.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub heads: Option<[bool; 2]>,
    /// The `/F` Print bit (인쇄).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub printed: Option<bool>,
    /// Square / circle / line / polygon: dashed (`/BS /D`) or solid border.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub dashed: Option<bool>,
    /// Polygon / polyline geometry.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub vertices: Option<Vec<f32>>,
    /// Callout leader line.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub callout: Option<Vec<f32>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum AnnotScanEvent {
    Page { page: PageIndex, annots: Vec<Annot> },
    Done { total: u32 },
    Cancelled,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ViewNonce {
    pub view_nonce: u64,
}

// ---------------------------------------------------------------------------------------
// §7.2 Forms
// ---------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FieldType {
    Text,
    Checkbox,
    Radio,
    Combo,
    List,
    Button,
    Signature,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FormOption {
    pub label: String,
    pub selected: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FormField {
    pub page: PageIndex,
    pub index: u32,
    pub name: String,
    #[serde(rename = "type")]
    pub field_type: FieldType,
    pub rect: Rect,
    pub value: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub checked: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub options: Option<Vec<FormOption>>,
    pub read_only: bool,
    pub required: bool,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub multiline: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub comb: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub max_len: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub font_size_pt: Option<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum FieldValue {
    Text { text: String },
    Checked { checked: bool },
    Selected { selected: Vec<u32> },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetFormFieldResult {
    pub field: FormField,
    pub previous: Option<String>,
    pub doc_generation: DocGeneration,
}

// ---------------------------------------------------------------------------------------
// §7.3 Pages
// ---------------------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PageSizePt {
    pub width_pt: f32,
    pub height_pt: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged, rename_all = "camelCase")]
pub enum BlankPageSize {
    Explicit(PageSizePt),
    Named(NamedPageSize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum NamedPageSize {
    A4,
    Letter,
    SameAs,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum PageOp {
    /// `FPDF_MovePages` ordered-list semantics.
    Move {
        pages: Vec<PageIndex>,
        to: PageIndex,
    },
    Delete {
        pages: Vec<PageIndex>,
    },
    Rotate {
        pages: Vec<PageIndex>,
        delta: u16,
    },
    InsertBlank {
        at: PageIndex,
        size: BlankPageSize,
    },
    Duplicate {
        pages: Vec<PageIndex>,
    },
    /// `range` is a 1-based `"1-3,5"` string.
    InsertFrom {
        at: PageIndex,
        path: String,
        #[serde(skip_serializing_if = "Option::is_none", default)]
        range: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none", default)]
        password: Option<String>,
    },
    Reverse,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtractPagesResult {
    pub bytes: u64,
    pub doc_generation: DocGeneration,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged, rename_all = "camelCase")]
pub enum SplitMode {
    EveryN {
        #[serde(rename = "everyN")]
        every_n: u32,
    },
    Ranges {
        ranges: Vec<String>,
    },
    /// v0.3 P4: one file per outline node of `level` (1 = top level), named after its title.
    ByOutline {
        #[serde(rename = "byOutline")]
        by_outline: OutlineSplit,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MergeInput {
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub range: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub password: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MergeWarning {
    FormsDropped,
    OutlineDropped,
    MetadataDropped,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MergeResult {
    pub info: DocInfo,
    pub warnings: Vec<MergeWarning>,
}

// ---------------------------------------------------------------------------------------
// §7.3a Page boxes and page size (P2)
// ---------------------------------------------------------------------------------------

/// Inset from each page's current crop box, in points, **as the page is seen** (`/Rotate`
/// applied): `top` is the displayed top edge whatever the rotation.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Margins {
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
    pub left: f32,
}

/// `set_page_boxes.crop`: an absolute rectangle in unrotated user space (the same for every
/// page), or margins inset from each page's own crop box.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum CropSpec {
    Rect(Rect),
    Margins { margins: Margins },
}

/// `set_page_boxes(args)`. `crop` / `media`: absent = unchanged, `null` = reset (crop only:
/// the crop box becomes the media box; a `null` media box is `invalidArgument`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PageBoxesArgs {
    pub doc_id: DocId,
    pub pages: PageSelection,
    #[serde(
        default,
        deserialize_with = "double_option",
        skip_serializing_if = "Option::is_none"
    )]
    pub crop: Option<Option<CropSpec>>,
    #[serde(
        default,
        deserialize_with = "double_option",
        skip_serializing_if = "Option::is_none"
    )]
    pub media: Option<Option<Rect>>,
}

/// Absent → `None`, `null` → `Some(None)`, a value → `Some(Some(v))` (with `#[serde(default)]`).
fn double_option<'de, T, D>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    T: Deserialize<'de>,
    D: serde::Deserializer<'de>,
{
    Option::<T>::deserialize(deserializer).map(Some)
}

/// A named paper size for `resize_pages` (portrait dimensions; the page's own orientation
/// is kept).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PaperName {
    A4,
    Letter,
    A3,
}

impl PaperName {
    /// Portrait width × height in points.
    pub fn size_pt(self) -> (f32, f32) {
        match self {
            PaperName::A4 => (595.28, 841.89),
            PaperName::Letter => (612.0, 792.0),
            PaperName::A3 => (841.89, 1190.55),
        }
    }
}

/// `'A4' | 'Letter' | 'A3' | { w, h }` — `{ w, h }` is the size **as seen**, in points.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ResizeTarget {
    Named(PaperName),
    Size { w: f32, h: f32 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ResizeMode {
    /// Scale the page content uniformly to fit the new size, centred.
    ScaleContent,
    /// Keep the content at 100 % and centre it (a smaller page clips it).
    CenterContent,
}

// ---------------------------------------------------------------------------------------
// §7.4 Page objects
// ---------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PageObjectType {
    Text,
    Image,
    Path,
    Shading,
    Form,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum NotEditableReason {
    InsideXObject,
    NoUnicode,
    Type3,
    Invisible,
    Permissions,
    GlyphsMissing,
    /// Stage 7 `probe_paragraph`: the text is rotated relative to the page.
    RotatedText,
    /// Stage 9 `probe_paragraph` / `edit_paragraph`: rewriting the page would drop content
    /// PDFium's content generator cannot write back (an inline image, a shading `sh`).
    UnwritableContent,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PageObject {
    pub object_id: ObjectId,
    #[serde(rename = "type")]
    pub object_type: PageObjectType,
    pub rect: Rect,
    pub matrix: Mat6,
    /// True when SeePDF created the object.
    pub ours: bool,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub font_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub font_size_pt: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub color: Option<Rgb>,
    pub editable: Editability,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub reason: Option<NotEditableReason>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PageObjectList {
    pub doc_generation: DocGeneration,
    pub objects: Vec<PageObject>,
}

/// Stage 8 `duplicate_objects`: `ObjectsResult & { newObjectIds }`. `objects` and
/// `newObjectIds` belong to `targetPage ?? page`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DuplicateObjectsResult {
    pub doc_generation: DocGeneration,
    pub objects: Vec<PageObject>,
    pub new_object_ids: Vec<ObjectId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TextEditStrategy {
    InPlace,
    ReplaceFont,
    Refused,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextEditProbe {
    pub strategy: TextEditStrategy,
    /// e.g. `"SeePDF Hangul"`; the UI must confirm before committing.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub substitute_font: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub reason: Option<NotEditableReason>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextObjectPatch {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub font_size_pt: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub color: Option<Rgb>,
}

// Stage 7 — paragraph probe + reflowing edit (`IPC_CONTRACT.md` §7.4b)

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ParagraphAlign {
    Left,
    Center,
    Right,
    Justify,
}

/// `probe_paragraph` — the paragraph under a point, as the reflowing editor sees it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ParagraphProbe {
    /// Every text object of the paragraph, in reading order.
    pub object_ids: Vec<ObjectId>,
    /// Union of their bounds.
    pub rect: Rect,
    /// Soft wraps joined with `' '`; a trailing `-` + lowercase continuation is de-hyphenated.
    pub text: String,
    pub font_name: String,
    /// The rendered size (font size × the object matrix' vertical scale).
    pub font_size_pt: f32,
    pub color: Rgb,
    pub mixed_styles: bool,
    /// Baseline to baseline; `fontSize × 1.2` for a single line.
    pub line_height_pt: f32,
    pub align: ParagraphAlign,
    pub first_line_indent_pt: f32,
    pub lines: u32,
    pub strategy: TextEditStrategy,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub substitute_font: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub reason: Option<NotEditableReason>,
    /// Additive to the Stage 7 contract: the generation the `objectIds` belong to.
    pub doc_generation: DocGeneration,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ParagraphEdit {
    pub object_ids: Vec<ObjectId>,
    /// `'\n'` = hard line break. Empty (or whitespace-only) deletes the paragraph; with flow
    /// `push`, what follows moves up into its place.
    pub text: String,
    /// New box width in pt (default: the probe rect's width).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub width: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub font_size_pt: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub color: Option<Rgb>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub align: Option<ParagraphAlign>,
    /// Stage 9: what happens to the content below the paragraph (default `push`).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub flow: Option<ParagraphFlow>,
    /// Stage 9: compute the layout + flow plan and change nothing (no generation bump, no
    /// undo entry, nothing embedded).
    #[serde(skip_serializing_if = "std::ops::Not::not", default)]
    pub dry_run: bool,
}

/// Stage 9 — what the content below an edited paragraph does (`IPC_CONTRACT.md` §7.4b).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ParagraphFlow {
    /// The content that follows the paragraph in its column moves with its bottom edge:
    /// down when it grew (as far as the page bottom margin / the first obstacle allow), up
    /// when it shrank.
    #[default]
    Push,
    /// Nothing else moves (the Stage 7 behaviour); a longer paragraph may cover what follows.
    Overlap,
    /// Nothing else moves; font size and leading shrink (0.7 … 1) until the paragraph fits
    /// its original height.
    Fit,
}

/// Stage 9 — why a `push` could not make all the room it needed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FlowBlocked {
    /// The content below would leave the page's bottom margin — or, with nothing below the
    /// paragraph (`pastBottomPt` > 0, `overflowPt` 0), the paragraph itself would.
    PageBottom,
    /// Something that does not follow the paragraph is in the way: a figure or table wider
    /// than the column, a form field, a SeePDF header / footer stamp, the running footer, the
    /// bottom edge of a box around the paragraph.
    Obstacle,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ParagraphEditResult {
    /// The page objects after the edit (`{ docGeneration, objects }`); for a dry run the
    /// page did not change, so `objects` is empty (`docGeneration` is the current one).
    pub objects: PageObjectList,
    /// The box the new text actually occupies.
    pub rect: Rect,
    pub lines: u32,
    /// How far the new text still extends over the content below after the flow was applied
    /// (0 when a push made all the room it needed — the paragraph spacing counts — or when
    /// nothing is below).
    pub overflow_pt: f32,
    /// Stage 9: how far the content below moved — > 0 pushed down, < 0 pulled up, 0 none.
    #[serde(default)]
    pub shifted_pt: f32,
    /// Stage 9: page objects moved by the flow.
    #[serde(default)]
    pub moved_objects: u32,
    /// Stage 9: annotations moved with them.
    #[serde(default)]
    pub moved_annotations: u32,
    /// Stage 9: how far the content below could move down before it reaches the page bottom
    /// margin or an obstacle.
    #[serde(default)]
    pub room_pt: f32,
    /// Stage 9 (`push` only): the push needed more than `roomPt`; `overflowPt` is what is
    /// left after pushing `roomPt`.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub blocked: Option<FlowBlocked>,
    /// Stage 9 (`fit` only): the factor applied to font size and leading, 0.7 … 1.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub fit_scale: Option<f32>,
    /// Stage 9: nothing is below the paragraph and the new text runs this far past the page's
    /// bottom margin (nothing is overlapped, so `overflowPt` is 0). 0 otherwise.
    #[serde(default)]
    pub past_bottom_pt: f32,
    /// Stage 9: when content moved (`shiftedPt` ≠ 0), the region it came from — the
    /// paragraph's column, from its original bottom down to the moved stack's bottom (plus a
    /// quarter of the font size on both ends). Annotations inside it moved by `−shiftedPt`;
    /// the UI moves pending 영역 표시 marks inside it the same way.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub moved_band: Option<Rect>,
}

// ---------------------------------------------------------------------------------------
// §7.5 Redaction and security
// ---------------------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RedactTextObject {
    pub object_id: ObjectId,
    pub text: String,
    pub rect: Rect,
    pub fully_inside: bool,
    /// v0.3 (pkg1, R2): the object is only partly marked and will be **split** — the marked
    /// characters go, the rest is re-emitted in place. `false` for a partly marked object
    /// that falls back to whole-run removal (its text is then listed in `collateral`).
    #[serde(default)]
    pub split: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RedactImageObject {
    pub object_id: ObjectId,
    pub rect: Rect,
    pub fully_inside: bool,
    /// v0.3 (pkg1, R1): the image is partly marked and the pixels under the marks will be
    /// **blanked** (the rest of the image stays). `false` with `fully_inside == false` means
    /// the image cannot be re-encoded faithfully (transparency, a palette, a stencil mask) and
    /// is removed whole.
    #[serde(default)]
    pub blank: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RedactPreview {
    pub page: PageIndex,
    pub text_objects: Vec<RedactTextObject>,
    pub image_objects: Vec<RedactImageObject>,
    pub annotations: Vec<AnnotId>,
    /// Non-empty ⇒ the apply will refuse.
    pub form_fields: Vec<String>,
    /// Text that will be removed although it is outside the marks.
    pub collateral: Vec<String>,
    /// v0.3 (pkg1, R4): top-level Form XObjects (groups) holding marked text or images. The
    /// apply refuses (`verifyFailed`) unless `RedactOptions.ungroup` is set, in which case
    /// they are ungrouped first (same undo step).
    #[serde(default)]
    pub groups: Vec<ObjectId>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RedactOptions {
    pub fill: Rgb,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub overlay_text: Option<String>,
    /// v0.3 (pkg1, R4): ungroup the Form XObjects the marks reach (`RedactPreview.groups`)
    /// before removing anything. The UI sets it after one confirm.
    #[serde(default)]
    pub ungroup: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RedactResult {
    pub removed_objects: u32,
    pub verified: bool,
    pub doc_generation: DocGeneration,
}

/// Stage 8 `apply_redactions_batch`: one page's marks.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RedactBatchMark {
    pub page: PageIndex,
    pub rects: Vec<Rect>,
}

/// Stage 8 `apply_redactions_batch`: every marked page in one undo step (`undo.redact`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RedactBatchResult {
    pub removed_objects: u32,
    pub verified: bool,
    pub doc_generation: DocGeneration,
    /// The pages that were redacted, ascending and deduplicated.
    pub pages: Vec<PageIndex>,
    /// v0.3 (pkg1, R2): text that was removed although it is outside the marks — runs that
    /// could not be split after all (the preview promised a split, the re-emission failed
    /// its check). Empty in the normal case (and then not serialised).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub collateral: Vec<String>,
}

// v0.3 pkg1-redaction-and-text-objects (R4) ------------------------------------------------

/// `ungroup_object`: the page after the Form XObject `objectId` was replaced by its children.
/// `newObjectIds` are the children's ids, in drawing order.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UngroupResult {
    pub doc_generation: DocGeneration,
    pub objects: Vec<PageObject>,
    pub new_object_ids: Vec<ObjectId>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BytesWritten {
    pub bytes: u64,
}

// ---------------------------------------------------------------------------------------
// §7.6 Save
// ---------------------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveResult {
    pub doc_id: DocId,
    pub path: String,
    pub bytes: u64,
    pub doc_generation: DocGeneration,
    pub elapsed_ms: f64,
}

// ---------------------------------------------------------------------------------------
// §7.7 Export and print
// ---------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ImageFormat {
    Png,
    Jpeg,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportImagesArgs {
    pub doc_id: DocId,
    pub pages: Vec<PageIndex>,
    pub format: ImageFormat,
    pub dpi: u32,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub quality: Option<u8>,
    pub out_dir: String,
    pub base_name: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub transparent_background: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportEstimate {
    pub bytes: u64,
    pub sampled_pages: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportTextResult {
    pub chars: u64,
}

/// `export_annotation_summary` output format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SummaryFormat {
    Txt,
    Csv,
    Md,
}

/// `export_annotation_summary` (P2): rows written and file size.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnnotationSummaryResult {
    pub count: u32,
    pub bytes: u64,
}

/// Which OS voice `tts_speak` drives (P2 read aloud).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TtsEngine {
    /// macOS `/usr/bin/say`.
    Say,
    /// Windows PowerShell + `System.Speech.Synthesis`.
    Sapi,
}

/// `tts_speak` / `tts_stop` / `tts_status`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TtsStatus {
    /// An offline OS voice exists on this platform.
    pub supported: bool,
    /// An utterance is playing right now.
    pub speaking: bool,
    pub engine: Option<TtsEngine>,
    /// The voice of the current (or last) utterance, when one was picked explicitly.
    pub voice: Option<String>,
    /// v0.3 (V4): the sentence being read when `tts_speak` was given `sentences`.
    #[serde(default)]
    pub sentence_index: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrintPrepareResult {
    pub temp_path: String,
}

// ---------------------------------------------------------------------------------------
// §7.9 OCR
// ---------------------------------------------------------------------------------------

/// `[x0, y0, x1, y1]` in **image pixels**, origin top-left.
pub type OcrBox = [f32; 4];

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OcrWord {
    pub text: String,
    pub bbox: OcrBox,
    pub confidence: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OcrLine {
    pub text: String,
    pub bbox: OcrBox,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub baseline: Option<[f32; 4]>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub row_height_px: Option<f32>,
    pub words: Vec<OcrWord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OcrPage {
    pub page: PageIndex,
    pub dpi: u32,
    pub width_px: u32,
    pub height_px: u32,
    pub rotation: Rotation,
    pub lines: Vec<OcrLine>,
}

/// One element of `ocr_apply.pages`: the `OcrPage` itself, or the Stage 8 `{ page, ocr }` form.
/// Either way the whole array is one `registry::mutate` = one undo step.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum OcrApplyPage {
    Wrapped {
        page: PageIndex,
        ocr: OcrPage,
        /// v0.3 pkg7-ocr (O2 페이지 회전 자동 감지): set the page's `/Rotate` to this before the
        /// layer is written, in the same undo step. `ocr` must have been recognised at it.
        #[serde(
            default,
            rename = "setRotation",
            skip_serializing_if = "Option::is_none"
        )]
        set_rotation: Option<Rotation>,
    },
    Plain(OcrPage),
}

impl OcrApplyPage {
    /// The page to apply; a wrapped page whose `page` disagrees with its `ocr.page` is refused
    /// rather than guessed at (the boxes belong to one of the two).
    pub fn into_page(self) -> Result<OcrPage, crate::ipc::EngineError> {
        self.into_page_and_rotation().map(|(ocr, _)| ocr)
    }

    /// [`OcrApplyPage::into_page`] plus the wrapped form's `setRotation` (v0.3 O2).
    pub fn into_page_and_rotation(
        self,
    ) -> Result<(OcrPage, Option<Rotation>), crate::ipc::EngineError> {
        match self {
            OcrApplyPage::Plain(ocr) => Ok((ocr, None)),
            OcrApplyPage::Wrapped {
                page,
                ocr,
                set_rotation,
            } if page == ocr.page => Ok((ocr, set_rotation)),
            OcrApplyPage::Wrapped { page, ocr, .. } => {
                Err(crate::ipc::EngineError::invalid(format!(
                    "ocr_apply: page {page} carries the OCR result of page {}",
                    ocr.page
                ))
                .with_page(page))
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum OcrEngine {
    Tesseract,
    Vision,
    Windows,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OcrCapabilities {
    pub engines: Vec<OcrEngine>,
    pub languages: Vec<String>,
    /// v0.3 pkg7-ocr (O1): the languages (`kor`, `eng`, `jpn`, `chi_sim`) each listed engine
    /// reads. `languages` stays the tesseract baseline for older callers.
    #[serde(default)]
    pub engine_languages: OcrEngineLanguages,
}

/// v0.3 pkg7-ocr (O1): `OcrCapabilities.engineLanguages`. An engine that is not listed in
/// `engines` has an empty list. The tesseract list is what the backend guarantees (`kor`,
/// `eng`); extra traineddata staged by `prepare-ocr --langs` is announced by the frontend's
/// asset manifest, which only it can see.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OcrEngineLanguages {
    pub tesseract: Vec<String>,
    pub vision: Vec<String>,
    pub windows: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OcrPageStatus {
    pub page: PageIndex,
    pub has_text: bool,
    pub char_count: u32,
}

// ---------------------------------------------------------------------------------------
// Stage 4: page stamps (P1-4) and compression (P1-5)
// ---------------------------------------------------------------------------------------

/// Where a page stamp sits, as the user sees the page: `t`/`m`/`b` row, `l`/`c`/`r` column.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum StampAnchor {
    Tl,
    Tc,
    Tr,
    Ml,
    Mc,
    Mr,
    Bl,
    Bc,
    Br,
}

/// Only picks the undo label (`undo.watermark` vs `undo.headerFooter`); geometry comes from
/// the anchor and margin.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum StampRole {
    Watermark,
    Header,
    Footer,
}

impl StampRole {
    /// The wire name, which is also what Stage 8 stores as the `SeePDF:Stamp` mark's `role`
    /// parameter so `remove_stamps` can filter by role.
    pub fn as_str(self) -> &'static str {
        match self {
            StampRole::Watermark => "watermark",
            StampRole::Header => "header",
            StampRole::Footer => "footer",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "watermark" => Some(StampRole::Watermark),
            "header" => Some(StampRole::Header),
            "footer" => Some(StampRole::Footer),
            _ => None,
        }
    }
}

/// Contract `StampSource`. `{{page}}`, `{{total}}`, `{{date}}` and `{{filename}}` are
/// replaced per page in `text`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum PageStampSource {
    Text {
        text: String,
        font_size_pt: f32,
        color: Rgb,
    },
    /// Height follows the image's aspect ratio.
    Image { path: String, width_pt: f32 },
    /// v0.3 T4: a solid background colour filling the page's crop box, always behind the
    /// page content (anchor, margin and rotation do not apply).
    Background { color: Rgb },
}

/// `PageIndex[] | 'all'`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PageSelection {
    All(AllPages),
    List(Vec<PageIndex>),
}

/// Contract `StampSpec` (`add_stamp`). Named `PageStampSpec` in Rust because `StampSpec` is
/// already the stamp **annotation** spec (`AnnotSpec::Stamp`); the wire shape is the
/// contract's exactly.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PageStampSpec {
    pub role: StampRole,
    pub source: PageStampSource,
    pub anchor: StampAnchor,
    /// Distance from the page edge for non-centre anchors, in points (>= 0).
    pub margin_pt: f32,
    /// -180..180, counter-clockwise as the user sees the page, about the stamp's centre.
    pub rotate_deg: f32,
    /// 0..1, fill and stroke alpha.
    pub opacity: f32,
    pub pages: PageSelection,
    /// P2 Bates numbering: what `{{bates}}` expands to. Flattened, so the wire fields are
    /// `batesStart` / `batesDigits` / `batesPrefix` / `batesSuffix`, each optional.
    #[serde(flatten)]
    pub bates: BatesOptions,
    /// v0.3 T4 (뒤에 배치): insert the stamp at the start of the page content (painted first,
    /// under the text) instead of on top. Absent = `false`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub behind: bool,
}

/// `{{bates}}` = `batesPrefix` + (`batesStart` + n, zero-padded to `batesDigits`) +
/// `batesSuffix`, where n counts the **stamped** pages in ascending order (0 for the first).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BatesOptions {
    #[serde(
        default = "BatesOptions::default_start",
        skip_serializing_if = "BatesOptions::is_default_start"
    )]
    pub bates_start: u64,
    #[serde(
        default = "BatesOptions::default_digits",
        skip_serializing_if = "BatesOptions::is_default_digits"
    )]
    pub bates_digits: u8,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub bates_prefix: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub bates_suffix: String,
}

impl BatesOptions {
    fn default_start() -> u64 {
        1
    }

    fn default_digits() -> u8 {
        6
    }

    fn is_default_start(v: &u64) -> bool {
        *v == Self::default_start()
    }

    fn is_default_digits(v: &u8) -> bool {
        *v == Self::default_digits()
    }
}

impl Default for BatesOptions {
    fn default() -> Self {
        Self {
            bates_start: Self::default_start(),
            bates_digits: Self::default_digits(),
            bates_prefix: String::new(),
            bates_suffix: String::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StampResult {
    pub info: DocInfo,
    pub pages_stamped: u32,
}

/// Stage 8 `remove_stamps`: `removed == 0` is not an error.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoveStampsResult {
    pub info: DocInfo,
    /// Page objects removed, over every page.
    pub removed: u32,
}

/// `compress_estimate`'s options. `target_dpi` is one of the presets 300 / 150 / 96.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompressOptions {
    pub target_dpi: u32,
    /// Default: every page.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub pages: Option<Vec<PageIndex>>,
    /// v0.3 pkg8 (X5) 구조 최적화, default `false`: drop unused page resources and unreferenced
    /// objects, empty content streams, and write object streams (unencrypted documents only).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub optimize: Option<bool>,
}

/// Rides on the `done` job event of `compress_estimate`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompressReport {
    /// Pending-result handle for `compress_apply` / `compress_discard`.
    pub token: u64,
    pub before_bytes: u64,
    pub after_bytes: u64,
    pub images_total: u32,
    pub images_downsampled: u32,
    pub elapsed_ms: f64,
}

// ---------------------------------------------------------------------------------------
// Stage 5 — compare two documents (P1-6), autosave / crash recovery (P1-8)
// ---------------------------------------------------------------------------------------

/// `compare_documents`' options. Page lists pair by position; the longer list's extra pages
/// become rows with the other side `null`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompareOptions {
    /// Default: every page of A.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub pages_a: Option<Vec<PageIndex>>,
    /// Default: every page of B.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub pages_b: Option<Vec<PageIndex>>,
    #[serde(default)]
    pub ignore_case: bool,
    /// Stage 8, default `true`: pair pages by text similarity (sequence alignment over per-page
    /// word sets) so an inserted / deleted page becomes a null-sided row instead of shifting
    /// every later pair. `false` = the Stage 5 positional pairing.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub align_pages: Option<bool>,
    /// v0.3 pkg8 (X4), default `true`: a 50 DPI pixel diff of every pair outside the text,
    /// reported as `visual` ops.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub visual: Option<bool>,
}

impl CompareOptions {
    pub fn align(&self) -> bool {
        self.align_pages.unwrap_or(true)
    }

    /// v0.3 pkg8 (X4) `visual`, default `true`: also diff the pixels outside the text.
    pub fn visual(&self) -> bool {
        self.visual.unwrap_or(true)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DiffKind {
    Equal,
    Insert,
    Delete,
    Replace,
    /// v0.3 pkg8 (X4): pixels changed outside the text (a figure, a scan, a drawing): `words`
    /// is 0, no text, `rectsA` / `rectsB` are the changed regions on each page.
    Visual,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiffOp {
    pub kind: DiffKind,
    /// Words on the A side for equal / delete / replace, on the B side for insert.
    pub words: u32,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub text_a: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub text_b: Option<String>,
    /// PDF points on page A, one rect per line fragment.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub rects_a: Option<Vec<Rect>>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub rects_b: Option<Vec<Rect>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ComparePage {
    pub page_a: Option<PageIndex>,
    pub page_b: Option<PageIndex>,
    pub changed: bool,
    pub words_a: u32,
    pub words_b: u32,
    pub ops: Vec<DiffOp>,
}

/// Rides on the `done` job event of `compare_documents`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompareReport {
    pub doc_a: DocId,
    pub doc_b: DocId,
    pub pages: Vec<ComparePage>,
    pub changed_pages: u32,
    /// Word counts; a replace counts on both.
    pub inserted: u32,
    pub deleted: u32,
    pub elapsed_ms: f64,
}

/// One autosaved copy in the recovery directory. The `<id>.json` sidecar is this, verbatim.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecoveryEntry {
    /// uuid v4; also the file stem in the recovery directory.
    pub id: String,
    /// `None` for a never-saved document.
    pub original_path: Option<String>,
    pub name: String,
    /// ISO-8601 (RFC 3339, UTC).
    pub saved_at: String,
    pub bytes: u64,
    pub pages: u16,
    /// Absolute path of the `.pdf` copy.
    pub recovery_path: String,
}

// ---------------------------------------------------------------------------------------
// §8 Events and progress
// ---------------------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum JobEvent {
    Started {
        job_id: JobId,
        total: u32,
    },
    Progress {
        job_id: JobId,
        done: u32,
        total: u32,
        #[serde(skip_serializing_if = "Option::is_none", default)]
        page: Option<PageIndex>,
        #[serde(skip_serializing_if = "Option::is_none", default)]
        note: Option<String>,
    },
    Done {
        job_id: JobId,
        elapsed_ms: f64,
        #[serde(skip_serializing_if = "Option::is_none", default)]
        outputs: Option<Vec<String>>,
        /// `compress_estimate` only.
        #[serde(skip_serializing_if = "Option::is_none", default)]
        report: Option<CompressReport>,
        /// `compare_documents` only.
        #[serde(skip_serializing_if = "Option::is_none", default)]
        compare: Option<CompareReport>,
    },
    Cancelled {
        job_id: JobId,
        done: u32,
    },
    Error {
        job_id: JobId,
        error: super::error::EngineError,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenFilePayload {
    pub path: String,
    pub source: OpenSource,
}

/// `doc-changed.reason`. There is no `save`: a save changes nothing a cache is keyed on, so
/// it keeps the generation and emits only `doc-saved` (`IPC_CONTRACT.md` §8).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ChangeReason {
    Edit,
    Undo,
    Redo,
    Pages,
    Ocr,
    Redact,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ChangedPages {
    All(AllPages),
    Some(Vec<PageIndex>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AllPages {
    All,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DocChangedPayload {
    pub doc_id: DocId,
    pub doc_generation: DocGeneration,
    pub changed_pages: ChangedPages,
    pub structure: bool,
    pub dirty: bool,
    pub reason: ChangeReason,
    /// Stage 2: the history state travels with the event so the title bar's ↶ / ↷ stay honest
    /// without a `get_document` round trip per edit (`STAGE1D_NOTES.md` §7.4).
    pub can_undo: bool,
    pub can_redo: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DocSavedPayload {
    pub doc_id: DocId,
    pub path: String,
    pub doc_generation: DocGeneration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PressureLevel {
    Normal,
    High,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnginePressurePayload {
    pub level: PressureLevel,
}

// ---------------------------------------------------------------------------------------
// §11 App, settings, recents
// ---------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Layout {
    Single,
    Continuous,
    Two,
    /// v0.3 (pkg6, V5): 두 쪽 with the cover alone — page 1, then 2|3, 4|5, …
    TwoCover,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecentEntry {
    pub path: String,
    pub name: String,
    pub dir: String,
    pub pages: u16,
    pub bytes: u64,
    /// ISO-8601.
    pub last_opened: String,
    pub last_page: PageIndex,
    pub zoom_percent: f32,
    pub layout: Layout,
    pub pinned: bool,
    pub thumb_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Locale {
    Ko,
    En,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Theme {
    System,
    Light,
    Dark,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RenderQuality {
    Balanced,
    High,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(untagged, rename_all = "kebab-case")]
pub enum DefaultZoom {
    Named(ZoomPreset),
    Percent(f32),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ZoomPreset {
    FitWidth,
    FitPage,
    Actual,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum OcrDpi {
    Auto(OcrDpiAuto),
    Fixed(u32),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum OcrDpiAuto {
    Auto,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    pub locale: Locale,
    pub theme: Theme,
    pub default_layout: Layout,
    pub default_zoom: DefaultZoom,
    pub restore_position: bool,
    pub author: String,
    pub render_quality: RenderQuality,
    pub tile_cache_mb: u32,
    /// How many entries the welcome screen and ⇧⌘O show (0 = do not keep recents).
    /// Stage 2: a first-class field, so it no longer rides in `tool_defaults`
    /// (`STAGE1E_NOTES.md` §5.3). `serde(default)` keeps settings written before it readable.
    #[serde(default = "default_recents_count")]
    pub recents_count: u32,
    pub backups_enabled: bool,
    pub ocr_languages: Vec<String>,
    pub ocr_dpi: OcrDpi,
    pub tool_defaults: serde_json::Map<String, serde_json::Value>,
    /// Autosave (crash recovery) interval in seconds; 0 = off. Stage 5, `serde(default)` so
    /// settings files written before it still load.
    #[serde(default = "default_autosave_sec")]
    pub autosave_sec: u32,
    /// 서명 보관함 (P1-9): at most [`MAX_SAVED_SIGNATURES`], newest last. Stage 6b,
    /// `serde(default)` so older settings files still load; an entry that does not parse is
    /// dropped on its own instead of resetting every setting to the default.
    #[serde(default, deserialize_with = "lenient_signatures")]
    pub signatures: Vec<SavedSignature>,
    /// 야간 모드 (P1-10), persisted since Stage 8. `serde(default)` = `off` for settings files
    /// written before it; an unknown value also reads as `off` rather than resetting everything.
    #[serde(default, deserialize_with = "lenient_night")]
    pub night: NightMode,
    /// 시작할 때 업데이트 확인 (v0.2.0). `true` for settings files written before it.
    #[serde(default = "default_true")]
    pub check_updates: bool,
    /// v0.3 pkg4: 내 도장 — custom image / text stamps, at most [`MAX_CUSTOM_STAMPS`], newest
    /// last. Lenient like `signatures`: a malformed entry is dropped on its own.
    #[serde(default, deserialize_with = "lenient_stamps")]
    pub stamps: Vec<CustomStamp>,
}

/// v0.3 pkg4: the 내 도장 cap.
pub const MAX_CUSTOM_STAMPS: usize = 30;

/// v0.3 pkg4: one 내 도장 entry. An image stamp's file was copied under
/// `$APPDATA/SeePDF/stamps/` (`write_stamp_image`); a text stamp is drawn by the built-in stamp
/// generator, `{{date}}` / `{{author}}` expanded at placement.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum CustomStamp {
    #[serde(rename_all = "camelCase")]
    Image {
        id: String,
        path: String,
        /// width ÷ height of the image.
        aspect: f32,
        #[serde(default)]
        created_at: String,
    },
    #[serde(rename_all = "camelCase")]
    Text {
        id: String,
        text: String,
        color: Rgb,
        #[serde(default)]
        shape: StampShape,
        #[serde(default)]
        created_at: String,
    },
}

fn lenient_stamps<'de, D>(deserializer: D) -> Result<Vec<CustomStamp>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let items = match serde_json::Value::deserialize(deserializer)? {
        serde_json::Value::Array(items) => items,
        _ => Vec::new(),
    };
    Ok(items
        .into_iter()
        .filter_map(|v| serde_json::from_value::<CustomStamp>(v).ok())
        .take(MAX_CUSTOM_STAMPS)
        .collect())
}

fn default_true() -> bool {
    true
}

/// `Settings.night`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum NightMode {
    #[default]
    Off,
    Dark,
    Sepia,
}

fn lenient_night<'de, D>(deserializer: D) -> Result<NightMode, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    Ok(serde_json::from_value(value).unwrap_or_default())
}

fn default_autosave_sec() -> u32 {
    60
}

/// The 서명 보관함 cap (P1-9).
pub const MAX_SAVED_SIGNATURES: usize = 10;

/// One saved signature. A drawn one keeps its unit-space strokes (0…1 of the drawn box,
/// y-down, as `SignatureDialog` produces them); a typed one keeps the text and the style id,
/// and is re-rendered to a PNG each time it is placed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum SavedSignature {
    #[serde(rename_all = "camelCase")]
    Drawn {
        id: String,
        paths: Vec<Vec<f32>>,
        aspect: f32,
        #[serde(default)]
        created_at: String,
    },
    #[serde(rename_all = "camelCase")]
    Typed {
        id: String,
        text: String,
        style: String,
        #[serde(default)]
        created_at: String,
    },
    /// v0.3 pkg4: a picked image (scanned signature, 도장), copied under
    /// `$APPDATA/SeePDF/signatures/` by `write_signature_image` (`keep: true`, never pruned).
    #[serde(rename_all = "camelCase")]
    Image {
        id: String,
        path: String,
        aspect: f32,
        #[serde(default)]
        created_at: String,
    },
}

/// `signatures` is user data inside a file that also holds every other setting: a malformed
/// entry must not make `get_settings` fall back to `Settings::default()` wholesale.
fn lenient_signatures<'de, D>(deserializer: D) -> Result<Vec<SavedSignature>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    // Through `Value` first: a wrong shape is then a value we ignore, not a parse error.
    let items = match serde_json::Value::deserialize(deserializer)? {
        serde_json::Value::Array(items) => items,
        _ => Vec::new(),
    };
    Ok(items
        .into_iter()
        .filter_map(|v| serde_json::from_value::<SavedSignature>(v).ok())
        .take(MAX_SAVED_SIGNATURES)
        .collect())
}

fn default_recents_count() -> u32 {
    20
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            locale: Locale::Ko,
            theme: Theme::System,
            default_layout: Layout::Continuous,
            default_zoom: DefaultZoom::Named(ZoomPreset::FitWidth),
            restore_position: true,
            author: String::new(),
            render_quality: RenderQuality::Balanced,
            tile_cache_mb: 64,
            recents_count: default_recents_count(),
            backups_enabled: true,
            ocr_languages: vec!["kor".into(), "eng".into()],
            ocr_dpi: OcrDpi::Auto(OcrDpiAuto::Auto),
            tool_defaults: serde_json::Map::new(),
            autosave_sec: default_autosave_sec(),
            signatures: Vec::new(),
            night: NightMode::Off,
            check_updates: true,
            stamps: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThumbId {
    pub thumb_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    pub version: String,
    pub os: String,
    pub arch: String,
    pub debug: bool,
    pub pdfium_version: String,
    pub pdfium_dir: String,
    pub locale: Locale,
    pub theme: Theme,
}

// --- v0.3 pkg5-app-shell-release-diagnostics ---

/// `problem_report` (H10): the text 문제 보고 copies, and the file it was also written to.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProblemReport {
    pub text: String,
    pub path: String,
}

// ---------------------------------------------------------------------------------------
// v0.3 pkg6-viewer-accessibility-settings: web links, reading order, read-aloud progress
// ---------------------------------------------------------------------------------------

/// `get_web_links` (V5): a URL PDFium found in the page **text** (`FPDFLink_LoadWebLinks`) —
/// not a Link annotation. `rects` are one per line the address spans (PDF user space);
/// `charStart` / `charCount` index the page's text layer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebLink {
    pub url: String,
    pub rects: Vec<Rect>,
    pub char_start: u32,
    pub char_count: u32,
}

/// `get_reading_order` (V6): the page's text-layer char ranges `[start, end)` in reading order.
/// `tagged` = the page has a structure tree with marked content, and `runs` follow its MCID
/// sequence; otherwise one run covering the page in content order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadingOrder {
    pub tagged: bool,
    pub runs: Vec<[u32; 2]>,
}

/// `tts-progress` (V4): the sentence the voice has started (`null` = the queue finished).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TtsProgressPayload {
    pub sentence_index: Option<u32>,
}

// ---------------------------------------------------------------------------------------
// v0.3 pkg3-security-save-integrity: signatures (S1), sanitize (S3), attachments (S4)
// ---------------------------------------------------------------------------------------

/// S1: one digital signature as PDFium reports it (`FPDFSignatureObj_*`) plus the signature
/// field's `/T`. Every value is optional because a signature dictionary may omit it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SignatureInfo {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub field_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub reason: Option<String>,
    /// `/M`, the raw PDF date string (`D:YYYYMMDDHHmmSS…`), passed through unparsed.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub time: Option<String>,
    /// `/SubFilter`, e.g. `adbe.pkcs7.detached`, `ETSI.CAdES.detached`.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub sub_filter: Option<String>,
}

/// S3 `sanitize_document`: what to remove. Every flag the frontend leaves out is `true`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SanitizeOptions {
    /// Document JavaScript (`/Names /JavaScript`) and every JavaScript action.
    pub javascript: bool,
    /// Embedded files (`/Names /EmbeddedFiles`) and FileAttachment annotations.
    pub attachments: bool,
    /// Automatic actions: `/OpenAction` and every `/AA` (catalog, page, annotation, field).
    pub actions: bool,
    /// `/Info`, every `/Metadata` stream (XMP) and `/PieceInfo`.
    pub metadata: bool,
    /// Optional-content groups that are hidden by default: `/OCProperties` is removed, so
    /// every layer shows and nothing stays hidden in the file.
    pub hidden_layers: bool,
}

impl Default for SanitizeOptions {
    fn default() -> Self {
        Self {
            javascript: true,
            attachments: true,
            actions: true,
            metadata: true,
            hidden_layers: true,
        }
    }
}

/// S3: how many entries `sanitize_document` removed per category (0 = none found).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SanitizeCounts {
    pub javascript: u32,
    pub attachments: u32,
    pub actions: u32,
    pub metadata: u32,
    pub hidden_layers: u32,
}

impl SanitizeCounts {
    pub fn total(&self) -> u32 {
        self.javascript + self.attachments + self.actions + self.metadata + self.hidden_layers
    }
}

/// S3 result: the report and the document afterwards. When nothing was found no undo step is
/// pushed and the generation is unchanged.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SanitizeResult {
    pub removed: SanitizeCounts,
    pub info: DocInfo,
}

/// S4: one document-level attachment (`/Names /EmbeddedFiles`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AttachmentInfo {
    /// Position in PDFium's attachment list; valid for the generation it was listed in.
    pub index: u32,
    pub name: String,
    /// Uncompressed size in bytes.
    pub size: u64,
}

// ---------------------------------------------------------------------------------------
// v0.3 pkg2-pages-structure-forms — images → PDF, split by outline, form authoring, form
// data, link borders
// ---------------------------------------------------------------------------------------

/// D1 `create_from_images`: the page each image goes on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ImagePageSize {
    /// The image's own size at its resolution (plus the margin).
    Original,
    A4,
    Letter,
}

/// D1: how an image sits on an A4 / Letter page.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ImageFit {
    /// Scaled (up or down) to fill the page less its margins, aspect kept, centred.
    #[default]
    Contain,
    /// Its own size at its resolution, scaled down only when it does not fit, centred.
    Actual,
}

/// P4 `SplitMode::ByOutline`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OutlineSplit {
    /// 1 = the top-level nodes, 2 = their children, …
    pub level: u8,
}

/// F1 `create_form_field`: the kinds of field that can be authored.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum NewFieldType {
    Text,
    Checkbox,
    Radio,
    Combo,
    Signature,
}

/// F1 `create_form_field`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FormFieldSpec {
    pub page: PageIndex,
    pub rect: Rect,
    #[serde(rename = "type")]
    pub field_type: NewFieldType,
    /// `/T`. A radio button whose name is taken by a radio group joins that group.
    pub name: String,
    /// Combo: the choices (`/Opt`). Radio: the export value of this button (first entry).
    #[serde(default)]
    pub options: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub max_len: Option<u32>,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub multiline: bool,
}

/// F1 `update_form_field`: every member optional; absent = unchanged.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FormFieldPatch {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub options: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub required: Option<bool>,
    /// 0 removes `/MaxLen`.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub max_len: Option<u32>,
}

/// F1: the document and its fields after an authoring edit.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FormEditResult {
    pub info: DocInfo,
    pub fields: Vec<FormField>,
    /// The field the edit created or changed, when there is one.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub field: Option<FormField>,
}

/// F1 form data export / import.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FormDataFormat {
    /// `name,value` rows, UTF-8 with a BOM.
    Csv,
    /// ISO 19444-1 XFDF.
    Xfdf,
}

/// F1 `import_form_data` / `export_form_data`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FormDataResult {
    /// Fields written (export) or set (import).
    pub fields: u32,
    /// Import: names in the file that match no field of the document.
    #[serde(default)]
    pub unknown: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub doc_generation: Option<DocGeneration>,
}

/// P5: a visible link border (`/Border [0 0 w]` + `/C`). `width: 0` = no border.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LinkBorder {
    pub width: f32,
    #[serde(default)]
    pub color: Rgb,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// v0.3 V5: `twoCover` is a layout the settings file and the recents accept.
    #[test]
    fn two_cover_layout_round_trips() {
        let layout: Layout = serde_json::from_value(json!("twoCover")).unwrap();
        assert_eq!(layout, Layout::TwoCover);
        assert_eq!(
            serde_json::to_value(Layout::TwoCover).unwrap(),
            json!("twoCover")
        );
    }

    /// P2: `set_page_boxes` tells "leave the crop alone" (absent) from "reset it" (`null`),
    /// and `crop` takes a rect or `{ margins }`.
    #[test]
    fn page_boxes_args_keep_null_apart_from_absent() {
        let absent: PageBoxesArgs =
            serde_json::from_value(json!({ "docId": "d1", "pages": "all" })).unwrap();
        assert_eq!(absent.crop, None);
        assert_eq!(absent.media, None);
        let reset: PageBoxesArgs =
            serde_json::from_value(json!({ "docId": "d1", "pages": [0], "crop": null })).unwrap();
        assert_eq!(reset.crop, Some(None));
        let rect: PageBoxesArgs = serde_json::from_value(json!({
            "docId": "d1", "pages": [0, 2],
            "crop": { "l": 10, "b": 20, "r": 300, "t": 400 },
            "media": { "l": 0, "b": 0, "r": 612, "t": 792 }
        }))
        .unwrap();
        assert_eq!(
            rect.crop,
            Some(Some(CropSpec::Rect(Rect::new(10.0, 20.0, 300.0, 400.0))))
        );
        assert_eq!(rect.media, Some(Some(Rect::new(0.0, 0.0, 612.0, 792.0))));
        let margins: PageBoxesArgs = serde_json::from_value(json!({
            "docId": "d1", "pages": "all",
            "crop": { "margins": { "top": 1, "right": 2, "bottom": 3, "left": 4 } }
        }))
        .unwrap();
        assert_eq!(
            margins.crop,
            Some(Some(CropSpec::Margins {
                margins: Margins {
                    top: 1.0,
                    right: 2.0,
                    bottom: 3.0,
                    left: 4.0
                }
            }))
        );
        let named: ResizeTarget = serde_json::from_value(json!("Letter")).unwrap();
        assert_eq!(named, ResizeTarget::Named(PaperName::Letter));
        let sized: ResizeTarget = serde_json::from_value(json!({ "w": 300, "h": 400 })).unwrap();
        assert_eq!(sized, ResizeTarget::Size { w: 300.0, h: 400.0 });
        assert_eq!(
            serde_json::from_value::<ResizeMode>(json!("scaleContent")).unwrap(),
            ResizeMode::ScaleContent
        );
    }

    /// P2 Bates fields are optional on the wire and default to 1 / 6 digits / no affixes.
    #[test]
    fn stamp_spec_bates_fields_are_optional() {
        let base = json!({
            "role": "footer",
            "source": { "kind": "text", "text": "{{bates}}", "fontSizePt": 9, "color": [0, 0, 0] },
            "anchor": "br", "marginPt": 24, "rotateDeg": 0, "opacity": 1, "pages": "all"
        });
        let plain: PageStampSpec = serde_json::from_value(base.clone()).unwrap();
        assert_eq!(plain.bates, BatesOptions::default());
        let mut with = base;
        with["batesStart"] = json!(101);
        with["batesDigits"] = json!(6);
        with["batesPrefix"] = json!("ABC");
        let spec: PageStampSpec = serde_json::from_value(with).unwrap();
        assert_eq!(spec.bates.bates_start, 101);
        assert_eq!(spec.bates.bates_prefix, "ABC");
        assert_eq!(spec.bates.bates_suffix, "");
    }

    /// The error shape JS receives on rejection: `{ code, message }`, with `page` / `detail`
    /// present only when they are (`IPC_CONTRACT.md` §2).
    #[test]
    fn engine_error_is_code_and_message() {
        let error = crate::ipc::EngineError::new(
            crate::ipc::ErrorCode::PasswordRequired,
            "document is encrypted",
        );
        assert_eq!(
            serde_json::to_value(&error).unwrap(),
            json!({ "code": "passwordRequired", "message": "document is encrypted" })
        );
        let detailed = error.clone().with_page(3).with_detail("more");
        assert_eq!(
            serde_json::to_value(&detailed).unwrap(),
            json!({
                "code": "passwordRequired",
                "message": "document is encrypted",
                "page": 3,
                "detail": "more"
            })
        );
    }

    #[test]
    fn document_types_are_camel_case() {
        let geom = PageGeom {
            index: 1,
            width_pt: 612.0,
            height_pt: 792.0,
            rotation: 90,
            crop: Rect::new(0.0, 0.0, 612.0, 792.0),
            label: Some("ii".into()),
        };
        assert_eq!(
            serde_json::to_value(&geom).unwrap(),
            json!({
                "index": 1,
                "widthPt": 612.0,
                "heightPt": 792.0,
                "rotation": 90,
                "crop": { "l": 0.0, "b": 0.0, "r": 612.0, "t": 792.0 },
                "label": "ii"
            })
        );

        let permissions = serde_json::to_value(Permissions::default()).unwrap();
        assert_eq!(permissions["extractText"], json!(true));
        assert_eq!(permissions["fillForms"], json!(true));
        assert_eq!(permissions["revision"], json!("unprotected"));

        assert_eq!(
            serde_json::to_value(OpenRequest {
                path: "/a.pdf".into(),
                source: OpenSource::MacosOpened,
            })
            .unwrap(),
            json!({ "path": "/a.pdf", "source": "macos-opened" })
        );
    }

    /// Tagged unions must match the TypeScript discriminants exactly, including
    /// `rename_all_fields = "camelCase"` — the tauri serde gotcha of §1.
    #[test]
    fn tagged_unions_match_the_typescript() {
        assert_eq!(
            serde_json::to_value(SearchEvent::Done {
                total: 62,
                pages_scanned: 14,
                elapsed_ms: 1.5,
            })
            .unwrap(),
            json!({ "type": "done", "total": 62, "pagesScanned": 14, "elapsedMs": 1.5 })
        );
        assert_eq!(
            serde_json::to_value(JobEvent::Progress {
                job_id: 7,
                done: 3,
                total: 14,
                page: Some(2),
                note: None,
            })
            .unwrap(),
            json!({ "type": "progress", "jobId": 7, "done": 3, "total": 14, "page": 2 })
        );

        let spec: AnnotSpec = serde_json::from_value(json!({
            "kind": "highlight",
            "rects": [{ "l": 1.0, "b": 2.0, "r": 3.0, "t": 4.0 }],
            "color": [255, 216, 0],
            "opacity": 0.6
        }))
        .expect("AnnotSpec::Highlight");
        assert!(matches!(spec, AnnotSpec::Highlight(_)));
        let spec: AnnotSpec = serde_json::from_value(json!({
            "kind": "textbox",
            "rect": { "l": 1.0, "b": 2.0, "r": 3.0, "t": 4.0 },
            "text": "한글",
            "fontSize": 12.0,
            "color": [0, 0, 0],
            "align": "left",
            "fillColor": null
        }))
        .expect("AnnotSpec::Textbox");
        assert!(matches!(spec, AnnotSpec::Textbox(_)));

        let ops: Vec<PageOp> = serde_json::from_value(json!([
            { "kind": "move", "pages": [3, 2], "to": 1 },
            { "kind": "insertBlank", "at": 0, "size": "a4" },
            { "kind": "insertBlank", "at": 0, "size": { "widthPt": 200.0, "heightPt": 400.0 } },
            { "kind": "insertFrom", "at": 2, "path": "/b.pdf", "range": "1-3,5" },
            { "kind": "reverse" }
        ]))
        .expect("PageOp list");
        assert_eq!(ops.len(), 5);
        assert!(matches!(ops[4], PageOp::Reverse));

        let value: FieldValue = serde_json::from_value(json!({ "checked": true })).unwrap();
        assert!(matches!(value, FieldValue::Checked { checked: true }));
        let value: FieldValue = serde_json::from_value(json!({ "selected": [0, 2] })).unwrap();
        assert!(matches!(value, FieldValue::Selected { .. }));
    }

    #[test]
    fn settings_round_trip_through_their_defaults() {
        let value = serde_json::to_value(Settings::default()).unwrap();
        assert_eq!(value["locale"], json!("ko"));
        assert_eq!(value["defaultZoom"], json!("fit-width"));
        assert_eq!(value["ocrDpi"], json!("auto"));
        assert_eq!(value["tileCacheMb"], json!(64));
        let back: Settings = serde_json::from_value(value).unwrap();
        assert_eq!(back.locale, Locale::Ko);

        // The two union-typed settings also accept their numeric forms.
        let mut numeric = serde_json::to_value(Settings::default()).unwrap();
        numeric["defaultZoom"] = json!(125.0);
        numeric["ocrDpi"] = json!(400);
        let numeric: Settings = serde_json::from_value(numeric).unwrap();
        assert!(matches!(numeric.default_zoom, DefaultZoom::Percent(z) if z == 125.0));
        assert!(matches!(numeric.ocr_dpi, OcrDpi::Fixed(400)));
    }

    #[test]
    fn stage4_stamp_and_compress_shapes() {
        let spec: PageStampSpec = serde_json::from_value(json!({
            "role": "watermark",
            "source": { "kind": "text", "text": "DRAFT {{page}}", "fontSizePt": 48.0, "color": [200, 0, 0] },
            "anchor": "mc",
            "marginPt": 36.0,
            "rotateDeg": 45.0,
            "opacity": 0.5,
            "pages": "all"
        }))
        .expect("text StampSpec");
        assert!(matches!(spec.pages, PageSelection::All(AllPages::All)));
        assert!(
            matches!(spec.source, PageStampSource::Text { font_size_pt, .. } if font_size_pt == 48.0)
        );
        assert_eq!(
            serde_json::to_value(&spec).unwrap(),
            json!({
                "role": "watermark",
                "source": { "kind": "text", "text": "DRAFT {{page}}", "fontSizePt": 48.0, "color": [200, 0, 0] },
                "anchor": "mc", "marginPt": 36.0, "rotateDeg": 45.0, "opacity": 0.5,
                "pages": "all"
            })
        );

        let spec: PageStampSpec = serde_json::from_value(json!({
            "role": "footer",
            "source": { "kind": "image", "path": "/tmp/logo.png", "widthPt": 120.0 },
            "anchor": "br", "marginPt": 24.0, "rotateDeg": 0.0, "opacity": 1.0,
            "pages": [0, 2]
        }))
        .expect("image StampSpec");
        assert_eq!(spec.role, StampRole::Footer);
        assert_eq!(spec.anchor, StampAnchor::Br);
        assert!(matches!(&spec.pages, PageSelection::List(p) if p == &vec![0, 2]));
        assert!(
            matches!(spec.source, PageStampSource::Image { width_pt, .. } if width_pt == 120.0)
        );
        for (anchor, wire) in [
            (StampAnchor::Tl, "tl"),
            (StampAnchor::Mr, "mr"),
            (StampAnchor::Bc, "bc"),
        ] {
            assert_eq!(serde_json::to_value(anchor).unwrap(), json!(wire));
        }

        let options: CompressOptions =
            serde_json::from_value(json!({ "targetDpi": 150 })).expect("CompressOptions");
        assert_eq!((options.target_dpi, options.pages.is_none()), (150, true));
        let options: CompressOptions =
            serde_json::from_value(json!({ "targetDpi": 96, "pages": [1] })).unwrap();
        assert_eq!(options.pages, Some(vec![1]));

        let report = CompressReport {
            token: 3,
            before_bytes: 1000,
            after_bytes: 800,
            images_total: 4,
            images_downsampled: 2,
            elapsed_ms: 12.5,
        };
        assert_eq!(
            serde_json::to_value(JobEvent::Done {
                job_id: 9,
                elapsed_ms: 12.5,
                outputs: None,
                report: Some(report),
                compare: None,
            })
            .unwrap(),
            json!({
                "type": "done", "jobId": 9, "elapsedMs": 12.5,
                "report": {
                    "token": 3, "beforeBytes": 1000, "afterBytes": 800,
                    "imagesTotal": 4, "imagesDownsampled": 2, "elapsedMs": 12.5
                }
            })
        );
        // Other jobs' `done` has no `report` key at all.
        let done = serde_json::to_value(JobEvent::Done {
            job_id: 1,
            elapsed_ms: 1.0,
            outputs: Some(vec!["/a.png".into()]),
            report: None,
            compare: None,
        })
        .unwrap();
        assert!(done.get("report").is_none());

        assert_eq!(
            serde_json::to_value(SecurityRevision::R5).unwrap(),
            json!("r5")
        );
        assert_eq!(
            serde_json::to_value(SecurityRevision::R6).unwrap(),
            json!("r6")
        );
    }

    #[test]
    fn stage7_paragraph_shapes() {
        let probe = ParagraphProbe {
            object_ids: vec![3, 4],
            rect: Rect::new(72.0, 640.0, 540.0, 712.0),
            text: "Hello world".into(),
            font_name: "Helvetica".into(),
            font_size_pt: 12.0,
            color: [0, 0, 0],
            mixed_styles: false,
            line_height_pt: 14.4,
            align: ParagraphAlign::Justify,
            first_line_indent_pt: -3.0,
            lines: 2,
            strategy: TextEditStrategy::Refused,
            substitute_font: None,
            reason: Some(NotEditableReason::RotatedText),
            doc_generation: 7,
        };
        let v = serde_json::to_value(&probe).unwrap();
        assert_eq!(v["objectIds"], json!([3, 4]));
        assert_eq!(
            v["rect"],
            json!({ "l": 72.0, "b": 640.0, "r": 540.0, "t": 712.0 })
        );
        assert_eq!(v["fontName"], json!("Helvetica"));
        assert_eq!(v["fontSizePt"], json!(12.0));
        assert_eq!(v["mixedStyles"], json!(false));
        assert!(v["lineHeightPt"].is_number());
        assert_eq!(v["align"], json!("justify"));
        assert_eq!(v["firstLineIndentPt"], json!(-3.0));
        assert_eq!(v["lines"], json!(2));
        assert_eq!(v["strategy"], json!("refused"));
        assert_eq!(v["reason"], json!("rotatedText"));
        assert_eq!(v["docGeneration"], json!(7));
        assert!(v.get("substituteFont").is_none());

        let edit: ParagraphEdit = serde_json::from_value(json!({
            "objectIds": [3, 4], "text": "a\nb", "align": "center", "fontSizePt": 10.5,
            "color": [255, 0, 0], "width": 300
        }))
        .unwrap();
        assert_eq!(edit.object_ids, vec![3, 4]);
        assert_eq!(edit.text, "a\nb");
        assert_eq!(edit.align, Some(ParagraphAlign::Center));
        assert_eq!(edit.width, Some(300.0));
        let minimal: ParagraphEdit =
            serde_json::from_value(json!({ "objectIds": [], "text": "x" })).unwrap();
        assert!(minimal.width.is_none() && minimal.align.is_none() && minimal.color.is_none());
        // Stage 9: `flow` defaults to push (absent on the wire), `dryRun` to false.
        assert!(minimal.flow.is_none() && !minimal.dry_run);
        assert_eq!(minimal.flow.unwrap_or_default(), ParagraphFlow::Push);
        let wire = serde_json::to_value(&minimal).unwrap();
        assert!(wire.get("flow").is_none() && wire.get("dryRun").is_none());
        for (name, flow) in [
            ("push", ParagraphFlow::Push),
            ("overlap", ParagraphFlow::Overlap),
            ("fit", ParagraphFlow::Fit),
        ] {
            let e: ParagraphEdit = serde_json::from_value(
                json!({ "objectIds": [1], "text": "x", "flow": name, "dryRun": true }),
            )
            .unwrap();
            assert_eq!(e.flow, Some(flow));
            assert!(e.dry_run);
            let back = serde_json::to_value(&e).unwrap();
            assert_eq!(back["flow"], json!(name));
            assert_eq!(back["dryRun"], json!(true));
        }

        let result = ParagraphEditResult {
            objects: PageObjectList {
                doc_generation: 8,
                objects: vec![],
            },
            rect: Rect::new(1.0, 2.0, 3.0, 4.0),
            lines: 3,
            overflow_pt: 12.5,
            shifted_pt: 28.8,
            moved_objects: 4,
            moved_annotations: 1,
            room_pt: 40.0,
            blocked: None,
            fit_scale: None,
            past_bottom_pt: 0.0,
            moved_band: Some(Rect::new(72.0, 500.0, 300.0, 600.0)),
        };
        assert_eq!(
            serde_json::to_value(&result).unwrap(),
            json!({
                "objects": { "docGeneration": 8, "objects": [] },
                "rect": { "l": 1.0, "b": 2.0, "r": 3.0, "t": 4.0 },
                "lines": 3,
                "overflowPt": 12.5,
                "shiftedPt": 28.799999237060547,
                "movedObjects": 4,
                "movedAnnotations": 1,
                "roomPt": 40.0,
                "pastBottomPt": 0.0,
                "movedBand": { "l": 72.0, "b": 500.0, "r": 300.0, "t": 600.0 }
            })
        );
        let blocked = ParagraphEditResult {
            blocked: Some(FlowBlocked::PageBottom),
            fit_scale: Some(0.86),
            past_bottom_pt: 8.5,
            moved_band: None,
            ..result.clone()
        };
        let v = serde_json::to_value(&blocked).unwrap();
        assert_eq!(v["blocked"], json!("pageBottom"));
        assert!((v["fitScale"].as_f64().unwrap() - 0.86).abs() < 1e-6);
        assert_eq!(v["pastBottomPt"], json!(8.5));
        assert!(v.get("movedBand").is_none());
        assert_eq!(
            serde_json::to_value(NotEditableReason::UnwritableContent).unwrap(),
            json!("unwritableContent")
        );
        assert_eq!(
            serde_json::to_value(FlowBlocked::Obstacle).unwrap(),
            json!("obstacle")
        );
        // A Stage 7 result (no Stage 9 fields) still parses.
        let old: ParagraphEditResult = serde_json::from_value(json!({
            "objects": { "docGeneration": 8, "objects": [] },
            "rect": { "l": 1.0, "b": 2.0, "r": 3.0, "t": 4.0 },
            "lines": 3, "overflowPt": 0.0
        }))
        .unwrap();
        assert!(old.blocked.is_none() && old.moved_objects == 0 && old.shifted_pt == 0.0);
        assert!(old.past_bottom_pt == 0.0 && old.moved_band.is_none());
    }

    #[test]
    fn stage5_compare_and_recovery_shapes() {
        let options: CompareOptions = serde_json::from_value(json!({})).expect("empty options");
        assert!(options.pages_a.is_none() && options.pages_b.is_none() && !options.ignore_case);
        let options: CompareOptions =
            serde_json::from_value(json!({ "pagesA": [0, 2], "pagesB": [1], "ignoreCase": true }))
                .unwrap();
        assert_eq!(options.pages_a, Some(vec![0, 2]));
        assert_eq!(options.pages_b, Some(vec![1]));
        assert!(options.ignore_case);

        let report = CompareReport {
            doc_a: "d1".into(),
            doc_b: "d2".into(),
            pages: vec![
                ComparePage {
                    page_a: Some(0),
                    page_b: Some(0),
                    changed: true,
                    words_a: 3,
                    words_b: 4,
                    ops: vec![
                        DiffOp {
                            kind: DiffKind::Equal,
                            words: 2,
                            text_a: None,
                            text_b: None,
                            rects_a: None,
                            rects_b: None,
                        },
                        DiffOp {
                            kind: DiffKind::Replace,
                            words: 1,
                            text_a: Some("old".into()),
                            text_b: Some("new text".into()),
                            rects_a: Some(vec![Rect::new(1.0, 2.0, 3.0, 4.0)]),
                            rects_b: Some(vec![Rect::new(5.0, 6.0, 7.0, 8.0)]),
                        },
                    ],
                },
                ComparePage {
                    page_a: None,
                    page_b: Some(1),
                    changed: false,
                    words_a: 0,
                    words_b: 0,
                    ops: vec![],
                },
            ],
            changed_pages: 1,
            inserted: 2,
            deleted: 1,
            elapsed_ms: 3.5,
        };
        assert_eq!(
            serde_json::to_value(JobEvent::Done {
                job_id: 4,
                elapsed_ms: 3.5,
                outputs: None,
                report: None,
                compare: Some(report.clone()),
            })
            .unwrap(),
            json!({
                "type": "done", "jobId": 4, "elapsedMs": 3.5,
                "compare": {
                    "docA": "d1", "docB": "d2",
                    "pages": [
                        { "pageA": 0, "pageB": 0, "changed": true, "wordsA": 3, "wordsB": 4, "ops": [
                            { "kind": "equal", "words": 2 },
                            { "kind": "replace", "words": 1, "textA": "old", "textB": "new text",
                              "rectsA": [{ "l": 1.0, "b": 2.0, "r": 3.0, "t": 4.0 }],
                              "rectsB": [{ "l": 5.0, "b": 6.0, "r": 7.0, "t": 8.0 }] }
                        ] },
                        { "pageA": null, "pageB": 1, "changed": false, "wordsA": 0, "wordsB": 0, "ops": [] }
                    ],
                    "changedPages": 1, "inserted": 2, "deleted": 1, "elapsedMs": 3.5
                }
            })
        );
        for (kind, wire) in [
            (DiffKind::Equal, "equal"),
            (DiffKind::Insert, "insert"),
            (DiffKind::Delete, "delete"),
            (DiffKind::Replace, "replace"),
        ] {
            assert_eq!(serde_json::to_value(kind).unwrap(), json!(wire));
        }

        let entry = RecoveryEntry {
            id: "0f8fad5b-d9cb-469f-a165-70867728950e".into(),
            original_path: None,
            name: "제목 없음".into(),
            saved_at: "2026-09-28T01:02:03.000Z".into(),
            bytes: 1234,
            pages: 3,
            recovery_path: "/r/0f8fad5b-d9cb-469f-a165-70867728950e.pdf".into(),
        };
        let wire = json!({
            "id": "0f8fad5b-d9cb-469f-a165-70867728950e", "originalPath": null, "name": "제목 없음",
            "savedAt": "2026-09-28T01:02:03.000Z", "bytes": 1234, "pages": 3,
            "recoveryPath": "/r/0f8fad5b-d9cb-469f-a165-70867728950e.pdf"
        });
        assert_eq!(serde_json::to_value(&entry).unwrap(), wire);
        assert_eq!(
            serde_json::from_value::<RecoveryEntry>(wire).unwrap(),
            entry
        );

        // Settings written before Stage 5 have no autosaveSec: it defaults to 60.
        let mut old = serde_json::to_value(Settings::default()).unwrap();
        assert_eq!(old["autosaveSec"], json!(60));
        old.as_object_mut().unwrap().remove("autosaveSec");
        let back: Settings = serde_json::from_value(old).unwrap();
        assert_eq!(back.autosave_sec, 60);
    }

    /// Stage 8: `night` round-trips, defaults to `off` for settings written before it, and an
    /// unknown value reads as `off` without resetting the other settings.
    #[test]
    fn settings_night_round_trip_and_default() {
        for (mode, wire) in [
            (NightMode::Off, "off"),
            (NightMode::Dark, "dark"),
            (NightMode::Sepia, "sepia"),
        ] {
            let settings = Settings {
                night: mode,
                ..Settings::default()
            };
            let value = serde_json::to_value(&settings).unwrap();
            assert_eq!(value["night"], json!(wire));
            let back: Settings = serde_json::from_value(value).unwrap();
            assert_eq!(back.night, mode);
        }
        // Written before Stage 8: no `night` key.
        let mut old = serde_json::to_value(Settings::default()).unwrap();
        old.as_object_mut().unwrap().remove("night");
        old["author"] = json!("박현우");
        let back: Settings = serde_json::from_value(old.clone()).unwrap();
        assert_eq!(back.night, NightMode::Off);
        assert_eq!(back.author, "박현우");
        // A value from the future is `off`, the rest survives.
        old["night"] = json!("amber");
        let back: Settings = serde_json::from_value(old).unwrap();
        assert_eq!(
            (back.night, back.author.as_str()),
            (NightMode::Off, "박현우")
        );

        // v0.2.0: `checkUpdates` is on by default and for settings written before it.
        let value = serde_json::to_value(Settings::default()).unwrap();
        assert_eq!(value["checkUpdates"], json!(true));
        let mut old = value.clone();
        old.as_object_mut().unwrap().remove("checkUpdates");
        assert!(
            serde_json::from_value::<Settings>(old)
                .unwrap()
                .check_updates
        );
        let mut off = value;
        off["checkUpdates"] = json!(false);
        assert!(
            !serde_json::from_value::<Settings>(off)
                .unwrap()
                .check_updates
        );

        // Stage 8 shapes the frontend mocks: remove_stamps / duplicate / redact batch results.
        let dup = DuplicateObjectsResult {
            doc_generation: 3,
            objects: Vec::new(),
            new_object_ids: vec![7, 8],
        };
        assert_eq!(
            serde_json::to_value(&dup).unwrap(),
            json!({ "docGeneration": 3, "objects": [], "newObjectIds": [7, 8] })
        );
        let batch = RedactBatchResult {
            removed_objects: 4,
            verified: true,
            doc_generation: 9,
            pages: vec![0, 2],
            collateral: vec![],
        };
        assert_eq!(
            serde_json::to_value(&batch).unwrap(),
            json!({ "removedObjects": 4, "verified": true, "docGeneration": 9, "pages": [0, 2] })
        );
        let mark: RedactBatchMark = serde_json::from_value(
            json!({ "page": 1, "rects": [{ "l": 0.0, "b": 0.0, "r": 1.0, "t": 1.0 }] }),
        )
        .unwrap();
        assert_eq!((mark.page, mark.rects.len()), (1, 1));
        let options: CompareOptions = serde_json::from_value(json!({})).unwrap();
        assert!(options.align());
        let options: CompareOptions =
            serde_json::from_value(json!({ "alignPages": false })).unwrap();
        assert!(!options.align());
        assert_eq!(
            StampRole::from_name(StampRole::Header.as_str()),
            Some(StampRole::Header)
        );
    }

    /// Stage 6b (P1-9): the 서명 보관함 wire shape, the default for settings written before it,
    /// and the leniency that keeps one bad entry from resetting every setting.
    #[test]
    fn saved_signatures_shape_default_and_leniency() {
        let drawn = SavedSignature::Drawn {
            id: "s1".into(),
            paths: vec![vec![0.0, 0.0, 1.0, 1.0]],
            aspect: 2.5,
            created_at: "2026-09-28T00:00:00.000Z".into(),
        };
        let typed = SavedSignature::Typed {
            id: "s2".into(),
            text: "박현우".into(),
            style: "script".into(),
            created_at: String::new(),
        };
        assert_eq!(
            serde_json::to_value(&drawn).unwrap(),
            json!({ "kind": "drawn", "id": "s1", "paths": [[0.0, 0.0, 1.0, 1.0]], "aspect": 2.5,
                    "createdAt": "2026-09-28T00:00:00.000Z" })
        );
        assert_eq!(
            serde_json::to_value(&typed).unwrap(),
            json!({ "kind": "typed", "id": "s2", "text": "박현우", "style": "script", "createdAt": "" })
        );

        // Written before Stage 6b: no `signatures` key at all.
        let mut old = serde_json::to_value(Settings::default()).unwrap();
        assert_eq!(old["signatures"], json!([]));
        old.as_object_mut().unwrap().remove("signatures");
        let back: Settings = serde_json::from_value(old.clone()).unwrap();
        assert!(back.signatures.is_empty());

        // A malformed entry is dropped on its own; the rest of the settings survive.
        old["author"] = json!("박현우");
        old["signatures"] = json!([
            { "kind": "typed", "id": "ok", "text": "Kim", "style": "formal" },
            { "kind": "drawn", "id": "bad" },
            42,
        ]);
        let back: Settings = serde_json::from_value(old.clone()).unwrap();
        assert_eq!(back.author, "박현우");
        assert_eq!(back.signatures.len(), 1);

        // Not an array: ignored, not an error.
        old["signatures"] = json!("nope");
        let back: Settings = serde_json::from_value(old.clone()).unwrap();
        assert!(back.signatures.is_empty());

        // Capped at MAX_SAVED_SIGNATURES.
        old["signatures"] = serde_json::Value::Array(
            (0..15)
                .map(|i| json!({ "kind": "typed", "id": format!("t{i}"), "text": "x", "style": "script" }))
                .collect(),
        );
        let back: Settings = serde_json::from_value(old).unwrap();
        assert_eq!(back.signatures.len(), MAX_SAVED_SIGNATURES);
    }

    /// `ocr_apply.pages` takes the `OcrPage[]` form and the Stage 8 `{ page, ocr }[]` form,
    /// mixed if need be; a wrapper that disagrees with its own page is refused.
    #[test]
    fn ocr_apply_pages_take_both_forms() {
        let ocr = |page: u16| {
            json!({
                "page": page, "dpi": 300, "widthPx": 2480, "heightPx": 3508, "rotation": 0,
                "lines": [{ "text": "가", "bbox": [0, 0, 10, 10], "words": [
                    { "text": "가", "bbox": [0, 0, 10, 10], "confidence": 90 }
                ] }]
            })
        };
        let pages: Vec<OcrApplyPage> =
            serde_json::from_value(json!([ocr(0), { "page": 1, "ocr": ocr(1) }])).unwrap();
        let pages: Vec<OcrPage> = pages
            .into_iter()
            .map(OcrApplyPage::into_page)
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(pages.iter().map(|p| p.page).collect::<Vec<_>>(), [0, 1]);
        assert_eq!(pages[1].lines[0].words[0].text, "가");

        let bad: OcrApplyPage =
            serde_json::from_value(json!({ "page": 2, "ocr": ocr(1) })).unwrap();
        let err = bad.into_page().unwrap_err();
        assert_eq!(err.code, crate::ipc::ErrorCode::InvalidArgument);
        assert_eq!(err.page, Some(2));
    }
}
