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
    /// From `/PageLabels`; read-only in v1.
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

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OutlineNode {
    pub title: String,
    pub page: Option<PageIndex>,
    /// Stage 2 (`STAGE1C_NOTES.md` §7.1): where on the page the heading is, in PDF user space,
    /// so 목차 scrolls to the heading rather than to the top of the page. `None` when the
    /// destination is a plain page reference or a fit-to-window view, which is the common case.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub dest: Option<OutlineDest>,
    pub children: Vec<OutlineNode>,
}

/// A `/Dest` reduced to what a scroller can use. All three are optional because the PDF
/// "retain the current value" convention writes them as null.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
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
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
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
    pub hidden: bool,
    pub printed: bool,
    pub locked: bool,
    pub editable: Editability,
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
    Path { path: String },
    Builtin { builtin: String },
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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
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
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
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
    /// `'\n'` = hard line break.
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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ParagraphEditResult {
    /// The page objects after the edit (`{ docGeneration, objects }`).
    pub objects: PageObjectList,
    /// The box the new text actually occupies.
    pub rect: Rect,
    pub lines: u32,
    /// How far the new text extends below the original rect's bottom (0 if it does not).
    pub overflow_pt: f32,
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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RedactImageObject {
    pub object_id: ObjectId,
    pub rect: Rect,
    pub fully_inside: bool,
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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RedactOptions {
    pub fill: Rgb,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub overlay_text: Option<String>,
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
    Wrapped { page: PageIndex, ocr: OcrPage },
    Plain(OcrPage),
}

impl OcrApplyPage {
    /// The page to apply; a wrapped page whose `page` disagrees with its `ocr.page` is refused
    /// rather than guessed at (the boxes belong to one of the two).
    pub fn into_page(self) -> Result<OcrPage, crate::ipc::EngineError> {
        match self {
            OcrApplyPage::Plain(ocr) => Ok(ocr),
            OcrApplyPage::Wrapped { page, ocr } if page == ocr.page => Ok(ocr),
            OcrApplyPage::Wrapped { page, ocr } => Err(crate::ipc::EngineError::invalid(format!(
                "ocr_apply: page {page} carries the OCR result of page {}",
                ocr.page
            ))
            .with_page(page)),
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
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum PageStampSource {
    Text {
        text: String,
        font_size_pt: f32,
        color: Rgb,
    },
    /// Height follows the image's aspect ratio.
    Image { path: String, width_pt: f32 },
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
}

impl CompareOptions {
    pub fn align(&self) -> bool {
        self.align_pages.unwrap_or(true)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DiffKind {
    Equal,
    Insert,
    Delete,
    Replace,
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
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ChangeReason {
    Edit,
    Undo,
    Redo,
    Save,
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

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
        assert!(matches!(spec.source, PageStampSource::Text { font_size_pt, .. } if font_size_pt == 48.0));
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
        assert!(matches!(spec.source, PageStampSource::Image { width_pt, .. } if width_pt == 120.0));
        for (anchor, wire) in [(StampAnchor::Tl, "tl"), (StampAnchor::Mr, "mr"), (StampAnchor::Bc, "bc")] {
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

        assert_eq!(serde_json::to_value(SecurityRevision::R5).unwrap(), json!("r5"));
        assert_eq!(serde_json::to_value(SecurityRevision::R6).unwrap(), json!("r6"));
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
        assert_eq!(v["rect"], json!({ "l": 72.0, "b": 640.0, "r": 540.0, "t": 712.0 }));
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

        let result = ParagraphEditResult {
            objects: PageObjectList {
                doc_generation: 8,
                objects: vec![],
            },
            rect: Rect::new(1.0, 2.0, 3.0, 4.0),
            lines: 3,
            overflow_pt: 12.5,
        };
        assert_eq!(
            serde_json::to_value(&result).unwrap(),
            json!({
                "objects": { "docGeneration": 8, "objects": [] },
                "rect": { "l": 1.0, "b": 2.0, "r": 3.0, "t": 4.0 },
                "lines": 3,
                "overflowPt": 12.5
            })
        );
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
        assert_eq!(serde_json::from_value::<RecoveryEntry>(wire).unwrap(), entry);

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
        assert_eq!((back.night, back.author.as_str()), (NightMode::Off, "박현우"));

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
        };
        assert_eq!(
            serde_json::to_value(&batch).unwrap(),
            json!({ "removedObjects": 4, "verified": true, "docGeneration": 9, "pages": [0, 2] })
        );
        let mark: RedactBatchMark =
            serde_json::from_value(json!({ "page": 1, "rects": [{ "l": 0.0, "b": 0.0, "r": 1.0, "t": 1.0 }] }))
                .unwrap();
        assert_eq!((mark.page, mark.rects.len()), (1, 1));
        let options: CompareOptions = serde_json::from_value(json!({})).unwrap();
        assert!(options.align());
        let options: CompareOptions = serde_json::from_value(json!({ "alignPages": false })).unwrap();
        assert!(!options.align());
        assert_eq!(StampRole::from_name(StampRole::Header.as_str()), Some(StampRole::Header));
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

        let bad: OcrApplyPage = serde_json::from_value(json!({ "page": 2, "ocr": ocr(1) })).unwrap();
        let err = bad.into_page().unwrap_err();
        assert_eq!(err.code, crate::ipc::ErrorCode::InvalidArgument);
        assert_eq!(err.page, Some(2));
    }
}
