//! v0.3 pkg4 (A2 / A6 / A8): the annotations PDFium cannot write — real `/Line`, `/Polygon`,
//! `/PolyLine` and `FreeText /FreeTextCallout` — written with `lopdf` on the serialised file,
//! through `registry::mutate_bytes` (one undo step, PDFium reopens the result before it
//! replaces the document; `ARCHITECTURE.md` §6.1).
//!
//! `FPDFPage_CreateAnnot` refuses these subtypes and PDFium has no setter for `/L`,
//! `/Vertices`, `/CL`, `/LE`, `/BE` or `/BS`. PDFium also never generates an appearance for
//! them, so every write here carries **its own `/AP`**: a Form XObject whose `/BBox` is the
//! `/Rect` (identity matrix, page coordinates), with an `ExtGState` for the opacity and, for a
//! measuring annotation, Helvetica (WinAnsi) for the label — digits, a dot and `mm` / `pt`
//! only, so no embedded font is needed.
//!
//! What PDFium cannot read back is mirrored in SeePDF's private keys like the colours
//! (`annot::KEY_*`): heads, dash, cloud, measuring unit and the callout line. `/L` and
//! `/Vertices` it reads itself (`FPDFAnnot_GetLine` / `FPDFAnnot_GetVertices`).
//!
//! Every function here is pure — bytes in, bytes out — so the unit tests run on any PDF.

use crate::engine::annot::{self, create, KEY_COLOR, KEY_FILL};
use crate::engine::save;
use crate::ipc::types::{MeasureUnit, PageIndex, Rect, Rgb};
use crate::ipc::EngineError;
use lopdf::{Dictionary, Object, ObjectId, Stream};

/// The geometry of one lopdf-written annotation.
#[derive(Debug, Clone, PartialEq)]
pub enum Shape {
    Line {
        p1: [f32; 2],
        p2: [f32; 2],
        heads: [bool; 2],
    },
    Polygon {
        vertices: Vec<f32>,
        cloudy: bool,
    },
    Polyline {
        vertices: Vec<f32>,
    },
}

/// Everything [`write_new`] / [`rewrite`] draw.
#[derive(Debug, Clone, PartialEq)]
pub struct Drawn {
    pub shape: Shape,
    pub color: Rgb,
    pub fill: Option<Rgb>,
    pub width: f32,
    pub opacity: f32,
    pub dashed: bool,
    pub measure: Option<MeasureUnit>,
}

/// The identity of a new annotation.
#[derive(Debug, Clone)]
pub struct Identity {
    pub id: String,
    pub author: Option<String>,
    pub contents: Option<String>,
    /// `D:…` — `/CreationDate` and `/M`.
    pub date: String,
}

/// The dash pattern of a dashed border, in points.
const DASH: [f32; 2] = [4.0, 3.0];
/// Measuring label size, points.
const LABEL_SIZE: f32 = 9.0;
/// Print flag.
const FLAG_PRINT: i64 = 4;

// ---------------------------------------------------------------------------------------
// entry points
// ---------------------------------------------------------------------------------------

/// Adds a new annotation to `page` of `bytes`.
pub fn write_new(
    bytes: &[u8],
    page: PageIndex,
    drawn: &Drawn,
    ident: &Identity,
) -> Result<Vec<u8>, EngineError> {
    validate(drawn)?;
    let mut doc = load(bytes)?;
    let page_id = page_id(&doc, page)?;
    let mut dict = Dictionary::new();
    dict.set("Type", Object::Name(b"Annot".to_vec()));
    dict.set("NM", save::pdf_text_string(&ident.id));
    if let Some(author) = ident.author.as_deref().filter(|a| !a.trim().is_empty()) {
        dict.set("T", save::pdf_text_string(author.trim()));
    }
    if let Some(contents) = ident.contents.as_deref().filter(|c| !c.is_empty()) {
        dict.set("Contents", save::pdf_text_string(contents));
    }
    dict.set("CreationDate", save::pdf_text_string(&ident.date));
    dict.set("F", Object::Integer(FLAG_PRINT));
    dict.set("P", Object::Reference(page_id));
    apply(&mut doc, &mut dict, drawn, &ident.date);
    let annot_id = doc.add_object(Object::Dictionary(dict));
    let mut annots = annots_of(&doc, page_id)?;
    annots.push(Object::Reference(annot_id));
    store_annots(&mut doc, page_id, annots)?;
    save_doc(&mut doc, bytes.len())
}

/// Redraws the annotation `id` on `page` as `drawn`, **in place**: the dictionary keeps its
/// object number (so a reply's `/IRT` and a popup's `/Parent` still resolve), its `/NM`,
/// `/T`, `/CreationDate`, `/F` and `/Popup`; geometry, colours, `/BS`, `/AP` and the mirrors
/// are replaced, `/M` is now.
pub fn rewrite(
    bytes: &[u8],
    page: PageIndex,
    id: &str,
    drawn: &Drawn,
    edits: &DictEdits,
) -> Result<Vec<u8>, EngineError> {
    validate(drawn)?;
    let mut doc = load(bytes)?;
    let page_id = page_id(&doc, page)?;
    let (slot, mut dict) = find(&doc, page_id, id)?;
    let date = annot::pdf_date_now();
    // What another subtype left behind must not survive a Line ↔ Polygon change.
    for key in [
        "L",
        "LE",
        "Vertices",
        "BE",
        "IT",
        "IC",
        "InkList",
        "QuadPoints",
    ] {
        dict.remove(key.as_bytes());
    }
    apply(&mut doc, &mut dict, drawn, &date);
    edits.apply(&mut dict);
    put(&mut doc, page_id, slot, dict)?;
    save_doc(&mut doc, bytes.len())
}

/// The dictionary strings and flags an update sets beside the drawing.
#[derive(Debug, Clone, Default)]
pub struct DictEdits {
    pub contents: Option<String>,
    pub author: Option<String>,
    pub locked: Option<bool>,
    pub printed: Option<bool>,
}

impl DictEdits {
    fn apply(&self, dict: &mut Dictionary) {
        if let Some(c) = &self.contents {
            if c.is_empty() {
                dict.remove(b"Contents");
            } else {
                dict.set("Contents", save::pdf_text_string(c));
            }
        }
        if let Some(t) = &self.author {
            dict.set("T", save::pdf_text_string(t));
        }
        let mut flags = dict
            .get(b"F")
            .ok()
            .and_then(|o| o.as_i64().ok())
            .unwrap_or(FLAG_PRINT);
        if let Some(printed) = self.printed {
            flags = if printed { flags | 4 } else { flags & !4 };
        }
        if let Some(locked) = self.locked {
            flags = if locked { flags | 128 } else { flags & !128 };
        }
        dict.set("F", Object::Integer(flags));
    }
}

/// v0.3 A2: turns the text box `id` (a SeePDF Stamp PDFium just built, its glyphs baked into
/// the appearance) into a `FreeText /IT /FreeTextCallout` with the leader line `callout`
/// (4 or 6 numbers, the first point at the tip): `/CL`, `/LE /OpenArrow`, `/RD` (where the
/// box sits inside the grown `/Rect`), the appearance's `/BBox` grown to the new `/Rect` and
/// the leader drawn after the box.
pub fn to_callout(
    bytes: &[u8],
    page: PageIndex,
    id: &str,
    callout: &[f32],
    color: Rgb,
    width: f32,
) -> Result<Vec<u8>, EngineError> {
    if !(callout.len() == 4 || callout.len() == 6) || callout.iter().any(|v| !v.is_finite()) {
        return Err(EngineError::invalid(
            "a callout line has 2 or 3 points (4 or 6 numbers)",
        ));
    }
    let mut doc = load(bytes)?;
    let page_id = page_id(&doc, page)?;
    let (slot, mut dict) = find(&doc, page_id, id)?;
    let boxed = rect_of(&dict).ok_or_else(|| EngineError::invalid("the text box has no /Rect"))?;
    let tip = [callout[0], callout[1]];
    let from = [callout[2], callout[3]];
    let heads = create::arrow_head(tip, from, width);
    let mut bounds = boxed;
    for p in callout.chunks_exact(2) {
        bounds = bounds.union(&Rect::new(p[0], p[1], p[0], p[1]));
    }
    for stroke in &heads {
        for p in stroke.chunks_exact(2) {
            bounds = bounds.union(&Rect::new(p[0], p[1], p[0], p[1]));
        }
    }
    let pad = (width / 2.0).max(1.0);
    let rect = Rect::new(
        bounds.l - pad,
        bounds.b - pad,
        bounds.r + pad,
        bounds.t + pad,
    );

    // The leader, after the box's own content.
    let mut ops = String::from("q\n");
    ops += &format!("{} w 1 J 1 j\n", num(width));
    ops += &format!("{} RG\n", rgb_ops(color));
    ops += &polyline_ops(callout);
    ops += "S\n";
    for stroke in &heads {
        // `arrow_head` strokes start at the tip: tip → barb.
        ops += &format!(
            "{} {} m {} {} l S\n",
            num(stroke[2]),
            num(stroke[3]),
            num(stroke[0]),
            num(stroke[1])
        );
    }
    ops += "Q\n";
    let ap = normal_ap(&dict).ok_or_else(|| EngineError::invalid("the text box has no /AP /N"))?;
    {
        let stream = doc
            .get_object_mut(ap)
            .and_then(Object::as_stream_mut)
            .map_err(|e| save::lopdf_error("callout appearance", e))?;
        let mut content = stream
            .decompressed_content()
            .unwrap_or_else(|_| stream.content.clone());
        content.extend_from_slice(b"\n");
        content.extend_from_slice(ops.as_bytes());
        stream.set_plain_content(content);
        stream
            .dict
            .set("BBox", reals(&[rect.l, rect.b, rect.r, rect.t]));
        stream.dict.remove(b"Matrix");
    }

    dict.set("Subtype", Object::Name(b"FreeText".to_vec()));
    dict.set("IT", Object::Name(b"FreeTextCallout".to_vec()));
    dict.set("CL", reals(callout));
    dict.set("LE", Object::Name(b"OpenArrow".to_vec()));
    dict.set("Rect", reals(&[rect.l, rect.b, rect.r, rect.t]));
    dict.set(
        "RD",
        reals(&[
            boxed.l - rect.l,
            boxed.b - rect.b,
            rect.r - boxed.r,
            rect.t - boxed.t,
        ]),
    );
    dict.set("C", color_array(color));
    dict.set("Subj", save::pdf_text_string(annot::SUBJ_CALLOUT));
    dict.set(annot::KEY_CALLOUT, save::pdf_text_string(&numbers(callout)));
    // The text box inside the grown `/Rect` (what `/RD` says), for SeePDF's own reader.
    dict.set(
        annot::KEY_BOX,
        save::pdf_text_string(&numbers(&[boxed.l, boxed.b, boxed.r, boxed.t])),
    );
    put(&mut doc, page_id, slot, dict)?;
    save_doc(&mut doc, bytes.len())
}

/// v0.3 A6: a solid or dashed border (`/BS << /W /S /D >>`) on a PDFium-drawn kind (square,
/// circle, ink). The appearance is dropped so PDFium regenerates it — its generator reads
/// `/BS /D` — and `annot::KEY_DASH` mirrors the choice.
pub fn set_border_style(
    bytes: &[u8],
    page: PageIndex,
    id: &str,
    width: f32,
    dashed: bool,
) -> Result<Vec<u8>, EngineError> {
    let mut doc = load(bytes)?;
    let page_id = page_id(&doc, page)?;
    let (slot, mut dict) = find(&doc, page_id, id)?;
    let mut bs = Dictionary::new();
    bs.set("W", Object::Real(width.max(0.0)));
    if dashed {
        bs.set("S", Object::Name(b"D".to_vec()));
        bs.set("D", reals(&DASH));
        dict.set(annot::KEY_DASH, save::pdf_text_string("1"));
    } else {
        bs.set("S", Object::Name(b"S".to_vec()));
        dict.remove(annot::KEY_DASH.as_bytes());
    }
    dict.set("BS", Object::Dictionary(bs));
    dict.set("Border", reals(&[0.0, 0.0, width.max(0.0)]));
    dict.remove(b"AP");
    put(&mut doc, page_id, slot, dict)?;
    save_doc(&mut doc, bytes.len())
}

/// Moves / scales another application's line, polygon or polyline by `m`: `/Rect`, `/L`,
/// `/Vertices` and `/CL` follow, the appearance stream is kept (it is mapped onto the new
/// `/Rect` through its `/BBox`), plus the dictionary edits.
pub fn move_foreign(
    bytes: &[u8],
    page: PageIndex,
    id: &str,
    m: [f32; 6],
    edits: &DictEdits,
) -> Result<Vec<u8>, EngineError> {
    let mut doc = load(bytes)?;
    let page_id = page_id(&doc, page)?;
    let (slot, mut dict) = find(&doc, page_id, id)?;
    for key in ["L", "Vertices", "CL"] {
        if let Some(v) = numbers_of(&dict, key.as_bytes()) {
            dict.set(key, reals(&map_points(&v, &m)));
        }
    }
    if let Some(r) = rect_of(&dict) {
        let mapped = bounds_of(&map_points(&[r.l, r.b, r.r, r.t], &m));
        dict.set("Rect", reals(&[mapped.l, mapped.b, mapped.r, mapped.t]));
    }
    dict.set("M", save::pdf_text_string(&annot::pdf_date_now()));
    edits.apply(&mut dict);
    put(&mut doc, page_id, slot, dict)?;
    save_doc(&mut doc, bytes.len())
}

/// v0.3 A8: maps `/L`, `/Vertices` and `/CL` (and SeePDF's callout mirrors) of every
/// annotation on `pages` whose `/Rect` PDFium already transformed by `m` — the same matrix the
/// page transform used (`[a b c d e f]`, PDF user space). Annotations without any of these
/// keys are left alone; popups are the caller's (PDFium maps them).
pub fn transform_geometry(
    bytes: &[u8],
    pages: &[(PageIndex, [f32; 6])],
) -> Result<Vec<u8>, EngineError> {
    let mut doc = load(bytes)?;
    let mut changed = false;
    for (page, m) in pages {
        let page_id = page_id(&doc, *page)?;
        let annots = annots_of(&doc, page_id)?;
        for (slot, entry) in annots.iter().enumerate() {
            let Some(dict) = resolve(&doc, entry).cloned() else {
                continue;
            };
            let subtype = dict
                .get(b"Subtype")
                .and_then(Object::as_name)
                .unwrap_or_default()
                .to_vec();
            let mut next = dict.clone();
            let mut touched = false;
            for key in ["L", "Vertices", "CL"] {
                if let Some(v) = numbers_of(&dict, key.as_bytes()) {
                    next.set(key, reals(&map_points(&v, m)));
                    touched = true;
                }
            }
            // The mirror of `/CL` follows the real key.
            if let Some(v) = dict
                .get(annot::KEY_CALLOUT.as_bytes())
                .ok()
                .and_then(|o| lopdf::decode_text_string(o).ok())
                .map(|s| crate::engine::annot::read::parse_numbers(&s))
                .filter(|v| v.len() >= 4)
            {
                next.set(
                    annot::KEY_CALLOUT,
                    save::pdf_text_string(&numbers(&map_points(&v, m))),
                );
                touched = true;
            }
            if let Some(v) = dict
                .get(annot::KEY_BOX.as_bytes())
                .ok()
                .and_then(|o| lopdf::decode_text_string(o).ok())
                .map(|s| crate::engine::annot::read::parse_numbers(&s))
                .filter(|v| v.len() == 4)
            {
                let b = bounds_of(&map_points(
                    &[v[0], v[1], v[2], v[3], v[0], v[3], v[2], v[1]],
                    m,
                ));
                next.set(
                    annot::KEY_BOX,
                    save::pdf_text_string(&numbers(&[b.l, b.b, b.r, b.t])),
                );
                touched = true;
            }
            if touched && subtype != b"Popup" {
                put(&mut doc, page_id, slot, next)?;
                changed = true;
            }
        }
    }
    if !changed {
        return Ok(bytes.to_vec());
    }
    save_doc(&mut doc, bytes.len())
}

// ---------------------------------------------------------------------------------------
// the drawing
// ---------------------------------------------------------------------------------------

fn validate(d: &Drawn) -> Result<(), EngineError> {
    let finite = |v: &[f32]| v.iter().all(|x| x.is_finite());
    match &d.shape {
        Shape::Line { p1, p2, .. } => {
            if !finite(p1) || !finite(p2) {
                return Err(EngineError::invalid("line points must be finite"));
            }
        }
        Shape::Polygon { vertices, .. } => {
            if vertices.len() < 6 || !vertices.len().is_multiple_of(2) || !finite(vertices) {
                return Err(EngineError::invalid("a polygon needs at least 3 points"));
            }
        }
        Shape::Polyline { vertices } => {
            if vertices.len() < 4 || !vertices.len().is_multiple_of(2) || !finite(vertices) {
                return Err(EngineError::invalid("a polyline needs at least 2 points"));
            }
        }
    }
    if !(d.width.is_finite() && d.width >= 0.0) {
        return Err(EngineError::invalid("border width must be ≥ 0"));
    }
    Ok(())
}

/// Sets every key the drawing owns on `dict` and writes a fresh appearance stream.
fn apply(doc: &mut lopdf::Document, dict: &mut Dictionary, d: &Drawn, date: &str) {
    let width = d.width.max(0.0);
    let opacity = d.opacity.clamp(0.0, 1.0);
    let label = d.measure.map(|unit| measure_label(&d.shape, unit));
    let (subtype, subj) = match &d.shape {
        Shape::Line { heads, .. } => (
            "Line",
            if heads[0] || heads[1] {
                annot::SUBJ_ARROW
            } else {
                annot::SUBJ_LINE
            },
        ),
        Shape::Polygon { .. } => ("Polygon", annot::SUBJ_POLYGON),
        Shape::Polyline { .. } => ("PolyLine", annot::SUBJ_POLYLINE),
    };
    dict.set("Subtype", Object::Name(subtype.as_bytes().to_vec()));
    dict.set("Subj", save::pdf_text_string(subj));
    dict.set("M", save::pdf_text_string(date));
    dict.set("C", color_array(d.color));
    dict.set("CA", Object::Real(opacity));
    let mut bs = Dictionary::new();
    bs.set("W", Object::Real(width));
    if d.dashed {
        bs.set("S", Object::Name(b"D".to_vec()));
        bs.set("D", reals(&DASH));
    } else {
        bs.set("S", Object::Name(b"S".to_vec()));
    }
    dict.set("BS", Object::Dictionary(bs));
    match &d.shape {
        Shape::Line { p1, p2, heads } => {
            dict.set("L", reals(&[p1[0], p1[1], p2[0], p2[1]]));
            let le = |on: bool| {
                Object::Name(if on {
                    b"OpenArrow".to_vec()
                } else {
                    b"None".to_vec()
                })
            };
            dict.set("LE", Object::Array(vec![le(heads[0]), le(heads[1])]));
            dict.set(
                annot::KEY_HEADS,
                save::pdf_text_string(&create::heads_code(*heads)),
            );
            if d.measure.is_some() {
                dict.set("IT", Object::Name(b"LineDimension".to_vec()));
            }
        }
        Shape::Polygon { vertices, cloudy } => {
            dict.set("Vertices", reals(vertices));
            if *cloudy {
                let mut be = Dictionary::new();
                be.set("S", Object::Name(b"C".to_vec()));
                be.set("I", Object::Integer(1));
                dict.set("BE", Object::Dictionary(be));
                dict.set(annot::KEY_CLOUD, save::pdf_text_string("1"));
            } else {
                dict.remove(annot::KEY_CLOUD.as_bytes());
            }
            if d.measure.is_some() {
                dict.set("IT", Object::Name(b"PolygonDimension".to_vec()));
            }
        }
        Shape::Polyline { vertices } => {
            dict.set("Vertices", reals(vertices));
            if d.measure.is_some() {
                dict.set("IT", Object::Name(b"PolyLineDimension".to_vec()));
            }
        }
    }
    match d.fill {
        Some(fill) if !matches!(d.shape, Shape::Line { .. } | Shape::Polyline { .. }) => {
            dict.set("IC", color_array(fill));
        }
        _ => {
            dict.remove(b"IC");
        }
    }
    // Mirrors (`annot::KEY_*`): what PDFium's readers cannot see.
    dict.set(KEY_COLOR, save::pdf_text_string(&rgb_code(d.color)));
    dict.set(
        KEY_FILL,
        save::pdf_text_string(&d.fill.map(rgb_code).unwrap_or_default()),
    );
    if d.dashed {
        dict.set(annot::KEY_DASH, save::pdf_text_string("1"));
    } else {
        dict.remove(annot::KEY_DASH.as_bytes());
    }
    match d.measure {
        Some(unit) => dict.set(
            annot::KEY_MEASURE,
            save::pdf_text_string(match unit {
                MeasureUnit::Mm => "mm",
                MeasureUnit::Pt => "pt",
            }),
        ),
        None => {
            dict.remove(annot::KEY_MEASURE.as_bytes());
        }
    }

    let (content, rect) = appearance(d, label.as_ref());
    dict.set("Rect", reals(&[rect.l, rect.b, rect.r, rect.t]));
    let ap = appearance_stream(doc, &content, rect, opacity, label.is_some());
    let mut ap_dict = Dictionary::new();
    ap_dict.set("N", Object::Reference(ap));
    dict.set("AP", Object::Dictionary(ap_dict));
}

/// A measuring label: the text and where its baseline starts.
#[derive(Debug, Clone, PartialEq)]
pub struct Label {
    pub text: String,
    pub at: [f32; 2],
}

/// Points → millimetres.
const MM_PER_PT: f32 = 25.4 / 72.0;

/// The label text of a measuring annotation: a line's / polyline's length, a polygon's area.
pub fn measure_text(shape: &Shape, unit: MeasureUnit) -> String {
    let length = |v: &[f32]| -> f32 {
        v.chunks_exact(2)
            .zip(v.chunks_exact(2).skip(1))
            .map(|(a, b)| ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2)).sqrt())
            .sum()
    };
    let (value, square) = match shape {
        Shape::Line { p1, p2, .. } => (length(&[p1[0], p1[1], p2[0], p2[1]]), false),
        Shape::Polyline { vertices } => (length(vertices), false),
        Shape::Polygon { vertices, .. } => (polygon_area(vertices), true),
    };
    let (value, suffix) = match (unit, square) {
        (MeasureUnit::Mm, false) => (value * MM_PER_PT, "mm"),
        (MeasureUnit::Mm, true) => (value * MM_PER_PT * MM_PER_PT, "mm\u{b2}"),
        (MeasureUnit::Pt, false) => (value, "pt"),
        (MeasureUnit::Pt, true) => (value, "pt\u{b2}"),
    };
    format!("{value:.1} {suffix}")
}

/// Shoelace, absolute.
pub fn polygon_area(v: &[f32]) -> f32 {
    let n = v.len() / 2;
    if n < 3 {
        return 0.0;
    }
    let mut sum = 0.0f32;
    for i in 0..n {
        let j = (i + 1) % n;
        sum += v[2 * i] * v[2 * j + 1] - v[2 * j] * v[2 * i + 1];
    }
    (sum / 2.0).abs()
}

fn measure_label(shape: &Shape, unit: MeasureUnit) -> Label {
    let text = measure_text(shape, unit);
    let width = label_width(&text);
    let at = match shape {
        Shape::Line { p1, p2, .. } => {
            // Above the middle of the line (perpendicular, towards +y where possible).
            let (mx, my) = ((p1[0] + p2[0]) / 2.0, (p1[1] + p2[1]) / 2.0);
            let (dx, dy) = (p2[0] - p1[0], p2[1] - p1[1]);
            let len = (dx * dx + dy * dy).sqrt().max(1e-3);
            let (mut nx, mut ny) = (-dy / len, dx / len);
            if ny < 0.0 {
                nx = -nx;
                ny = -ny;
            }
            [mx + nx * 4.0 - width / 2.0, my + ny * 4.0]
        }
        Shape::Polyline { vertices } => {
            let mid = (vertices.len() / 4) * 2;
            [vertices[mid] + 4.0, vertices[mid + 1] + 4.0]
        }
        Shape::Polygon { vertices, .. } => {
            let b = bounds_of(vertices);
            [
                (b.l + b.r) / 2.0 - width / 2.0,
                (b.b + b.t) / 2.0 - LABEL_SIZE / 3.0,
            ]
        }
    };
    Label { text, at }
}

/// Helvetica's widths are ~0.556 em for digits; close enough to size the `/Rect`.
fn label_width(text: &str) -> f32 {
    text.chars().count() as f32 * LABEL_SIZE * 0.56
}

/// The appearance content (page coordinates) and the `/Rect` that holds it.
fn appearance(d: &Drawn, label: Option<&Label>) -> (String, Rect) {
    let width = d.width.max(0.0);
    let mut ops = String::new();
    let mut pts: Vec<f32> = Vec::new();
    let mut extra_pad = 0.0f32;
    ops += "q\n/GS0 gs\n";
    ops += &format!("{} w 1 J 1 j\n", num(width));
    ops += &format!("{} RG\n", rgb_ops(d.color));
    if d.dashed {
        ops += &format!("[{} {}] 0 d\n", num(DASH[0]), num(DASH[1]));
    }
    match &d.shape {
        Shape::Line { p1, p2, heads } => {
            let v = [p1[0], p1[1], p2[0], p2[1]];
            pts.extend_from_slice(&v);
            ops += &polyline_ops(&v);
            ops += "S\n";
            // Heads are drawn solid even on a dashed line.
            if d.dashed {
                ops += "[] 0 d\n";
            }
            let mut barbs: Vec<Vec<f32>> = Vec::new();
            if heads[1] {
                barbs.push(head_path(*p2, *p1, width));
            }
            if heads[0] {
                barbs.push(head_path(*p1, *p2, width));
            }
            for b in barbs.iter().filter(|b| !b.is_empty()) {
                pts.extend_from_slice(b);
                ops += &polyline_ops(b);
                ops += "S\n";
            }
        }
        Shape::Polyline { vertices } => {
            pts.extend_from_slice(vertices);
            ops += &polyline_ops(vertices);
            ops += "S\n";
        }
        Shape::Polygon { vertices, cloudy } => {
            pts.extend_from_slice(vertices);
            if let Some(fill) = d.fill {
                ops += &format!("{} rg\n", rgb_ops(fill));
            }
            if *cloudy {
                let (path, bulge) = cloud_ops(vertices, width);
                ops += &path;
                extra_pad = bulge;
            } else {
                ops += &polyline_ops(vertices);
                ops += "h\n";
            }
            ops += if d.fill.is_some() { "B\n" } else { "S\n" };
        }
    }
    let mut rect = bounds_of(&pts);
    let pad = (width / 2.0).max(1.0) + extra_pad;
    rect = Rect::new(rect.l - pad, rect.b - pad, rect.r + pad, rect.t + pad);
    if let Some(label) = label {
        ops += &format!(
            "BT /Helv {} Tf {} rg {} {} Td {} Tj ET\n",
            num(LABEL_SIZE),
            rgb_ops(d.color),
            num(label.at[0]),
            num(label.at[1]),
            pdf_string_literal(&label.text)
        );
        let w = label_width(&label.text);
        let label_rect = Rect::new(
            label.at[0] - 1.0,
            label.at[1] - LABEL_SIZE * 0.3,
            label.at[0] + w + 1.0,
            label.at[1] + LABEL_SIZE,
        );
        rect = rect.union(&label_rect);
    }
    ops += "Q\n";
    (ops, rect)
}

/// An open arrow head at `tip`, opening towards `from`: barb → tip → barb.
fn head_path(tip: [f32; 2], from: [f32; 2], width: f32) -> Vec<f32> {
    let strokes = create::arrow_head(tip, from, width);
    if strokes.len() != 2 {
        return Vec::new();
    }
    vec![
        strokes[0][2],
        strokes[0][3],
        tip[0],
        tip[1],
        strokes[1][2],
        strokes[1][3],
    ]
}

/// A cloudy border: every edge becomes a row of outward half-circle bumps (one cubic each).
/// Returns the path and how far the bumps reach outside the polygon.
fn cloud_ops(v: &[f32], width: f32) -> (String, f32) {
    let n = v.len() / 2;
    // Orientation: bumps go outward, i.e. to the right of a counter-clockwise edge.
    let mut signed = 0.0f32;
    for i in 0..n {
        let j = (i + 1) % n;
        signed += v[2 * i] * v[2 * j + 1] - v[2 * j] * v[2 * i + 1];
    }
    let ccw = signed >= 0.0;
    let arc = (width * 4.0).max(8.0);
    let mut ops = format!("{} {} m\n", num(v[0]), num(v[1]));
    let mut max_bulge = 0.0f32;
    for i in 0..n {
        let j = (i + 1) % n;
        let (x0, y0, x1, y1) = (v[2 * i], v[2 * i + 1], v[2 * j], v[2 * j + 1]);
        let (dx, dy) = (x1 - x0, y1 - y0);
        let len = (dx * dx + dy * dy).sqrt();
        if len < 1e-3 {
            continue;
        }
        let bumps = (len / arc).ceil().max(1.0) as usize;
        let (ux, uy) = (dx / len, dy / len);
        let (nx, ny) = if ccw { (uy, -ux) } else { (-uy, ux) };
        let step = len / bumps as f32;
        let k = step / 2.0 * 4.0 / 3.0;
        max_bulge = max_bulge.max(step / 2.0 * 1.0);
        for b in 0..bumps {
            let (ax, ay) = (x0 + ux * step * b as f32, y0 + uy * step * b as f32);
            let (bx, by) = (ax + ux * step, ay + uy * step);
            ops += &format!(
                "{} {} {} {} {} {} c\n",
                num(ax + nx * k),
                num(ay + ny * k),
                num(bx + nx * k),
                num(by + ny * k),
                num(bx),
                num(by)
            );
        }
    }
    ops += "h\n";
    (ops, max_bulge)
}

/// The normal appearance: a Form XObject in page coordinates (`/BBox` = `/Rect`, no matrix).
fn appearance_stream(
    doc: &mut lopdf::Document,
    content: &str,
    rect: Rect,
    opacity: f32,
    with_font: bool,
) -> ObjectId {
    let mut gs = Dictionary::new();
    gs.set("Type", Object::Name(b"ExtGState".to_vec()));
    gs.set("CA", Object::Real(opacity));
    gs.set("ca", Object::Real(opacity));
    let mut ext = Dictionary::new();
    ext.set("GS0", Object::Dictionary(gs));
    let mut resources = Dictionary::new();
    resources.set("ExtGState", Object::Dictionary(ext));
    if with_font {
        let mut helv = Dictionary::new();
        helv.set("Type", Object::Name(b"Font".to_vec()));
        helv.set("Subtype", Object::Name(b"Type1".to_vec()));
        helv.set("BaseFont", Object::Name(b"Helvetica".to_vec()));
        helv.set("Encoding", Object::Name(b"WinAnsiEncoding".to_vec()));
        let mut fonts = Dictionary::new();
        fonts.set("Helv", Object::Dictionary(helv));
        resources.set("Font", Object::Dictionary(fonts));
    }
    let mut dict = Dictionary::new();
    dict.set("Type", Object::Name(b"XObject".to_vec()));
    dict.set("Subtype", Object::Name(b"Form".to_vec()));
    dict.set("BBox", reals(&[rect.l, rect.b, rect.r, rect.t]));
    dict.set("Resources", Object::Dictionary(resources));
    doc.add_object(Object::Stream(Stream::new(
        dict,
        content.as_bytes().to_vec(),
    )))
}

// ---------------------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------------------

fn load(bytes: &[u8]) -> Result<lopdf::Document, EngineError> {
    lopdf::Document::load_mem(bytes).map_err(|e| save::lopdf_error("parse", e))
}

fn save_doc(doc: &mut lopdf::Document, hint: usize) -> Result<Vec<u8>, EngineError> {
    let mut out = Vec::with_capacity(hint + 4096);
    doc.save_to(&mut out)
        .map_err(|e| save::lopdf_error("write", e))?;
    Ok(out)
}

fn page_id(doc: &lopdf::Document, page: PageIndex) -> Result<ObjectId, EngineError> {
    doc.get_pages()
        .get(&(page as u32 + 1))
        .copied()
        .ok_or_else(|| EngineError::not_found(format!("page {page}")).with_page(page))
}

/// The page's `/Annots` entries (inline array or indirect object).
fn annots_of(doc: &lopdf::Document, page_id: ObjectId) -> Result<Vec<Object>, EngineError> {
    let page = doc
        .get_dictionary(page_id)
        .map_err(|e| save::lopdf_error("page", e))?;
    match page.get(b"Annots") {
        Ok(Object::Array(items)) => Ok(items.clone()),
        Ok(Object::Reference(id)) => Ok(doc
            .get_object(*id)
            .and_then(Object::as_array)
            .cloned()
            .unwrap_or_default()),
        _ => Ok(Vec::new()),
    }
}

fn store_annots(
    doc: &mut lopdf::Document,
    page_id: ObjectId,
    annots: Vec<Object>,
) -> Result<(), EngineError> {
    let indirect = doc
        .get_dictionary(page_id)
        .ok()
        .and_then(|p| p.get(b"Annots").ok())
        .and_then(|o| o.as_reference().ok());
    if let Some(id) = indirect {
        if let Ok(target) = doc.get_object_mut(id) {
            if let Ok(array) = target.as_array_mut() {
                *array = annots;
                return Ok(());
            }
        }
    }
    doc.get_dictionary_mut(page_id)
        .map_err(|e| save::lopdf_error("page", e))?
        .set("Annots", Object::Array(annots));
    Ok(())
}

fn resolve<'d>(doc: &'d lopdf::Document, entry: &'d Object) -> Option<&'d Dictionary> {
    match entry {
        Object::Reference(id) => doc.get_dictionary(*id).ok(),
        Object::Dictionary(d) => Some(d),
        _ => None,
    }
}

/// The `/Annots` slot and a copy of the dictionary of the annotation named `id`.
fn find(
    doc: &lopdf::Document,
    page_id: ObjectId,
    id: &str,
) -> Result<(usize, Dictionary), EngineError> {
    annots_of(doc, page_id)?
        .iter()
        .enumerate()
        .find_map(|(i, entry)| {
            let dict = resolve(doc, entry)?;
            let nm = dict
                .get(b"NM")
                .ok()
                .and_then(|o| lopdf::decode_text_string(o).ok());
            let popup =
                dict.get(b"Subtype").and_then(Object::as_name).ok() == Some(b"Popup".as_slice());
            (nm.as_deref() == Some(id) && !popup).then(|| (i, dict.clone()))
        })
        .ok_or_else(|| {
            EngineError::not_found(format!("annotation '{id}' is not in the saved page"))
        })
}

/// Writes `dict` back into slot `slot` (its own object when it has one).
fn put(
    doc: &mut lopdf::Document,
    page_id: ObjectId,
    slot: usize,
    dict: Dictionary,
) -> Result<(), EngineError> {
    let mut annots = annots_of(doc, page_id)?;
    match annots.get(slot) {
        Some(Object::Reference(id)) => {
            let id = *id;
            doc.objects.insert(id, Object::Dictionary(dict));
        }
        Some(_) => {
            annots[slot] = Object::Dictionary(dict);
            store_annots(doc, page_id, annots)?;
        }
        None => return Err(EngineError::not_found("annotation slot vanished")),
    }
    Ok(())
}

fn normal_ap(dict: &Dictionary) -> Option<ObjectId> {
    dict.get(b"AP")
        .ok()?
        .as_dict()
        .ok()?
        .get(b"N")
        .ok()?
        .as_reference()
        .ok()
}

fn rect_of(dict: &Dictionary) -> Option<Rect> {
    let v = numbers_of(dict, b"Rect")?;
    (v.len() == 4).then(|| {
        Rect::new(
            v[0].min(v[2]),
            v[1].min(v[3]),
            v[0].max(v[2]),
            v[1].max(v[3]),
        )
    })
}

fn numbers_of(dict: &Dictionary, key: &[u8]) -> Option<Vec<f32>> {
    let arr = dict.get(key).ok()?.as_array().ok()?;
    let v: Vec<f32> = arr
        .iter()
        .filter_map(|o| match o {
            Object::Integer(i) => Some(*i as f32),
            Object::Real(r) => Some(*r),
            _ => None,
        })
        .collect();
    (v.len() == arr.len() && !v.is_empty()).then_some(v)
}

fn map_points(v: &[f32], m: &[f32; 6]) -> Vec<f32> {
    v.chunks_exact(2)
        .flat_map(|p| {
            [
                m[0] * p[0] + m[2] * p[1] + m[4],
                m[1] * p[0] + m[3] * p[1] + m[5],
            ]
        })
        .collect()
}

pub(crate) fn bounds_of(v: &[f32]) -> Rect {
    let mut r: Option<Rect> = None;
    for p in v.chunks_exact(2) {
        let point = Rect::new(p[0], p[1], p[0], p[1]);
        r = Some(match r {
            Some(acc) => acc.union(&point),
            None => point,
        });
    }
    r.unwrap_or(Rect::ZERO)
}

fn polyline_ops(v: &[f32]) -> String {
    let mut out = String::new();
    for (i, p) in v.chunks_exact(2).enumerate() {
        out += &format!(
            "{} {} {}\n",
            num(p[0]),
            num(p[1]),
            if i == 0 { "m" } else { "l" }
        );
    }
    out
}

fn num(v: f32) -> String {
    let s = format!("{v:.3}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s.is_empty() || s == "-" {
        "0".to_string()
    } else {
        s.to_string()
    }
}

fn numbers(v: &[f32]) -> String {
    v.iter().map(|x| num(*x)).collect::<Vec<_>>().join(" ")
}

fn rgb_ops(c: Rgb) -> String {
    format!(
        "{} {} {}",
        num(c[0] as f32 / 255.0),
        num(c[1] as f32 / 255.0),
        num(c[2] as f32 / 255.0)
    )
}

fn rgb_code(c: Rgb) -> String {
    format!("{} {} {}", c[0], c[1], c[2])
}

fn color_array(c: Rgb) -> Object {
    reals(&[
        c[0] as f32 / 255.0,
        c[1] as f32 / 255.0,
        c[2] as f32 / 255.0,
    ])
}

fn reals(values: &[f32]) -> Object {
    Object::Array(values.iter().map(|v| Object::Real(*v)).collect())
}

/// A `( … )` literal in WinAnsi: the label is ASCII plus `²` (0xB2).
fn pdf_string_literal(text: &str) -> String {
    let mut out = String::from("(");
    for ch in text.chars() {
        match ch {
            '(' | ')' | '\\' => {
                out.push('\\');
                out.push(ch);
            }
            '\u{b2}' => out += "\\262",
            c if c.is_ascii() && !c.is_ascii_control() => out.push(c),
            _ => out.push('?'),
        }
    }
    out.push(')');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn measure_texts() {
        let line = Shape::Line {
            p1: [0.0, 0.0],
            p2: [72.0, 0.0],
            heads: [false, false],
        };
        assert_eq!(measure_text(&line, MeasureUnit::Mm), "25.4 mm");
        assert_eq!(measure_text(&line, MeasureUnit::Pt), "72.0 pt");
        let square = Shape::Polygon {
            vertices: vec![0.0, 0.0, 72.0, 0.0, 72.0, 72.0, 0.0, 72.0],
            cloudy: false,
        };
        assert_eq!(measure_text(&square, MeasureUnit::Mm), "645.2 mm\u{b2}");
        assert_eq!(polygon_area(&[0.0, 0.0, 10.0, 0.0, 0.0, 10.0]), 50.0);
    }

    #[test]
    fn numbers_are_compact() {
        assert_eq!(num(1.0), "1");
        assert_eq!(num(-0.25), "-0.25");
        assert_eq!(num(0.0), "0");
        assert_eq!(pdf_string_literal("1 mm\u{b2} (x)"), "(1 mm\\262 \\(x\\))");
    }

    #[test]
    fn cloud_bumps_reach_outside() {
        let (ops, bulge) = cloud_ops(&[0.0, 0.0, 100.0, 0.0, 100.0, 100.0], 1.0);
        assert!(ops.matches(" c\n").count() >= 3);
        assert!(bulge > 0.0);
    }
}
