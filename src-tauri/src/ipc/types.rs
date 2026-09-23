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
}
