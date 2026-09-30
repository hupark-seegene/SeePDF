//! v0.3.1 그룹 해제 at the content-stream level: a Form XObject's `Do` on the page is replaced
//! by the form's own content, byte for byte.
//!
//! The v0.3.0 ungroup moved the form's children with PDFium's object API and let
//! `FPDFPage_GenerateContent` write them back. That generator cannot write everything it
//! reads (the corpus in `tests/ungroup_corpus.rs`): an inline image is dropped, `Tc` / `Tw` /
//! `Tz` are lost (justified lines collapse), optional-content marks point at `/Properties`
//! names the page does not have (hidden content shows), the clip around the `Do` is not
//! carried over, and two fonts with the same `BaseFont` are merged into one. Inlining the
//! content itself keeps all of that:
//!
//! * the form's operators are copied verbatim; only the **names** of its resources are
//!   rewritten, to fresh names in the page's `/Resources` that point at the same objects (a
//!   name the form takes from the page's resources, as PDFium does when the form's
//!   `/Resources` lacks that category, keeps its page name)
//!   (`/Font`, `/XObject`, `/ExtGState`, `/ColorSpace`, `/Pattern`, `/Shading`,
//!   `/Properties`, and an inline image's `/CS`). A pattern is re-anchored: its space is the
//!   form's space, which after inlining is the page's, so a copy gets `/Matrix` × the form's
//!   transform;
//! * the form's `/Matrix` becomes a `cm`, its `/BBox` a clip, its `/OC` a `/OC … BDC`;
//!   the graphics state in effect at the `Do` (the clip around it, a colour, an alpha) is
//!   simply still in effect;
//! * a nested form on the way to the clicked text is inlined the same way, in the same pass
//!   (one undo step).
//!
//! **Self-contained pieces.** A later 문단 편집 lets PDFium regenerate the content streams that
//! hold the paragraph (`raw::page::rewrite_set`). A regenerated stream is wrapped in `q … Q`
//! and writes each object whole, so it no longer leaves any state behind — a colour, font or
//! clip that the rest of the inlined content relied on would be gone. So the inlined content is
//! cut into pieces — one per text object (`BT … ET`), one per image, shading, nested group or
//! marked point, and one per run of consecutive paths (a table's borders; PDFium writes paths
//! back losslessly) — and **every piece restates the whole graphics state it starts with** (one
//! `q` per open level with the operators that level set: `cm`, clips, `gs`, colours, line
//! state, and — only in a piece that draws text — the text state; then the open marked-content
//! sequences) and closes everything it opened. No piece depends
//! on another, so regenerating one (the edited paragraph's) leaves the others exactly as they
//! were, including their `Tc` / `Tw`, inline images and marks. The page stream holding the
//! `Do` is split the same way: the part before closes every open level, the part after
//! reopens them. A leading `q` makes the page's own level 0 closable.
//!
//! **Text only.** For a group whose look depends on it staying a group (real transparency), the
//! caller may ask for [`Mode::TextOnly`]: the `Do` stays, pointing at a copy of the form without
//! its text, and only the text is inlined after it. Whether either result looks the same is
//! not decided here — the caller renders the page before and after (`ungroup.rs`).
//!
//! What this cannot keep: a transparency group's own compositing (an alpha or soft mask at the
//! `Do` is applied per object instead of to the group as a whole — the render check decides),
//! text used as a clip (`Tr` 4–7) across pieces, and structure (`/MCID`) links into the form's
//! stream.

use crate::engine::save::lopdf_error;
use crate::ipc::{EngineError, ErrorCode};
use lopdf::{Dictionary, Document, Object, ObjectId, Stream};
use std::collections::HashMap;
use std::ops::Range;

/// `[a b c d e f]`, PDF row-vector convention (`cm`).
pub type Mat = [f64; 6];

pub const IDENTITY: Mat = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];

/// `a` then `b`: the matrix of `x · a · b`.
pub fn mul(a: &Mat, b: &Mat) -> Mat {
    [
        a[0] * b[0] + a[1] * b[2],
        a[0] * b[1] + a[1] * b[3],
        a[2] * b[0] + a[3] * b[2],
        a[2] * b[1] + a[3] * b[3],
        a[4] * b[0] + a[5] * b[2] + b[4],
        a[4] * b[1] + a[5] * b[3] + b[5],
    ]
}

pub fn invert(m: &Mat) -> Option<Mat> {
    let det = m[0] * m[3] - m[1] * m[2];
    if det.abs() < 1e-12 {
        return None;
    }
    let (a, b, c, d) = (m[3] / det, -m[1] / det, -m[2] / det, m[0] / det);
    Some([a, b, c, d, -(m[4] * a + m[5] * c), -(m[4] * b + m[5] * d)])
}

/// Same matrix within PDFium's float precision.
pub fn close(a: &Mat, b: &Mat) -> bool {
    let near = |x: f64, y: f64, tol: f64| (x - y).abs() <= tol * x.abs().max(y.abs()).max(1.0);
    (0..4).all(|k| near(a[k], b[k], 2e-3)) && (4..6).all(|k| near(a[k], b[k], 2e-3))
}

fn is_identity(m: &Mat) -> bool {
    close(m, &IDENTITY)
}

// ---------------------------------------------------------------------------------------
// Tokenizer
// ---------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub enum OperandKind {
    Number(f64),
    Name(Vec<u8>),
    Other,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Operand {
    pub span: Range<usize>,
    pub kind: OperandKind,
}

impl Operand {
    fn name(&self) -> Option<&[u8]> {
        match &self.kind {
            OperandKind::Name(n) => Some(n),
            _ => None,
        }
    }
    fn number(&self) -> Option<f64> {
        match self.kind {
            OperandKind::Number(v) => Some(v),
            _ => None,
        }
    }
}

/// One operator with its operands. `span` covers the first operand to the end of the operator
/// (for `BI`, to the end of `EI`).
#[derive(Debug, Clone, PartialEq)]
pub struct Op {
    pub operator: String,
    pub operands: Vec<Operand>,
    pub span: Range<usize>,
    /// `BI`: the value of `/CS` (`/ColorSpace`) when it is a name.
    pub inline_cs: Option<Operand>,
}

fn is_ws(c: u8) -> bool {
    matches!(c, 0 | 9 | 10 | 12 | 13 | 32)
}

fn is_delim(c: u8) -> bool {
    matches!(
        c,
        b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%'
    )
}

fn unparseable(what: &str) -> EngineError {
    EngineError::new(
        ErrorCode::Unsupported,
        format!("the group's content could not be read ({what})"),
    )
    .with_detail("content")
}

struct Lexer<'a> {
    data: &'a [u8],
    i: usize,
}

impl<'a> Lexer<'a> {
    fn skip_ws(&mut self) {
        while self.i < self.data.len() {
            let c = self.data[self.i];
            if is_ws(c) {
                self.i += 1;
            } else if c == b'%' {
                while self.i < self.data.len() && !matches!(self.data[self.i], b'\r' | b'\n') {
                    self.i += 1;
                }
            } else {
                break;
            }
        }
    }

    fn regular(&mut self) -> &'a [u8] {
        let start = self.i;
        while self.i < self.data.len() && !is_ws(self.data[self.i]) && !is_delim(self.data[self.i])
        {
            self.i += 1;
        }
        &self.data[start..self.i]
    }

    fn literal_string(&mut self) -> Result<(), EngineError> {
        // at '('
        self.i += 1;
        let mut depth = 1usize;
        while self.i < self.data.len() {
            match self.data[self.i] {
                b'\\' => self.i += 2,
                b'(' => {
                    depth += 1;
                    self.i += 1;
                }
                b')' => {
                    depth -= 1;
                    self.i += 1;
                    if depth == 0 {
                        return Ok(());
                    }
                }
                _ => self.i += 1,
            }
        }
        Err(unparseable("unterminated string"))
    }

    fn name(&mut self) -> Vec<u8> {
        // at '/'
        self.i += 1;
        let raw = self.regular();
        let mut out = Vec::with_capacity(raw.len());
        let mut k = 0;
        while k < raw.len() {
            if raw[k] == b'#' && k + 2 < raw.len() {
                let hex = std::str::from_utf8(&raw[k + 1..k + 3])
                    .ok()
                    .and_then(|h| u8::from_str_radix(h, 16).ok());
                if let Some(v) = hex {
                    out.push(v);
                    k += 3;
                    continue;
                }
            }
            out.push(raw[k]);
            k += 1;
        }
        out
    }

    /// Skips one object (any kind) starting at the current position.
    fn object(&mut self) -> Result<OperandKind, EngineError> {
        self.skip_ws();
        if self.i >= self.data.len() {
            return Err(unparseable("truncated object"));
        }
        match self.data[self.i] {
            b'(' => {
                self.literal_string()?;
                Ok(OperandKind::Other)
            }
            b'<' if self.data.get(self.i + 1) == Some(&b'<') => {
                self.i += 2;
                loop {
                    self.skip_ws();
                    if self.i >= self.data.len() {
                        return Err(unparseable("unterminated dictionary"));
                    }
                    if self.data[self.i] == b'>' && self.data.get(self.i + 1) == Some(&b'>') {
                        self.i += 2;
                        return Ok(OperandKind::Other);
                    }
                    self.object()?;
                }
            }
            b'<' => {
                while self.i < self.data.len() && self.data[self.i] != b'>' {
                    self.i += 1;
                }
                if self.i >= self.data.len() {
                    return Err(unparseable("unterminated hex string"));
                }
                self.i += 1;
                Ok(OperandKind::Other)
            }
            b'[' => {
                self.i += 1;
                loop {
                    self.skip_ws();
                    if self.i >= self.data.len() {
                        return Err(unparseable("unterminated array"));
                    }
                    if self.data[self.i] == b']' {
                        self.i += 1;
                        return Ok(OperandKind::Other);
                    }
                    self.object()?;
                }
            }
            b'/' => Ok(OperandKind::Name(self.name())),
            c if is_delim(c) => Err(unparseable("stray delimiter")),
            _ => {
                let tok = self.regular();
                Ok(number(tok).map_or(OperandKind::Other, OperandKind::Number))
            }
        }
    }
}

fn number(tok: &[u8]) -> Option<f64> {
    if tok.is_empty()
        || !tok
            .iter()
            .all(|c| c.is_ascii_digit() || matches!(c, b'+' | b'-' | b'.'))
    {
        return None;
    }
    // "4." / ".5" / "+.5" parse; "-" alone and "1.2.3" do not
    std::str::from_utf8(tok).ok()?.parse::<f64>().ok()
}

/// Components per sample of an inline image colour space, when known.
fn inline_components(cs: Option<&[u8]>) -> Option<usize> {
    match cs {
        None => None,
        Some(b"G" | b"DeviceGray" | b"I" | b"Indexed") => Some(1),
        Some(b"RGB" | b"DeviceRGB") => Some(3),
        Some(b"CMYK" | b"DeviceCMYK") => Some(4),
        _ => None,
    }
}

/// `BI … ID <data> EI`: at the first byte after `BI`. Returns the `/CS` operand.
fn inline_image(lx: &mut Lexer<'_>) -> Result<Option<Operand>, EngineError> {
    let mut cs = None;
    let (mut w, mut h, mut bpc) = (None, None, None);
    let mut filtered = false;
    let mut mask = false;
    let mut cs_name: Option<Vec<u8>> = None;
    loop {
        lx.skip_ws();
        if lx.i >= lx.data.len() {
            return Err(unparseable("unterminated inline image"));
        }
        if lx.data[lx.i] != b'/' {
            let tok = lx.regular();
            if tok == b"ID" {
                break;
            }
            if tok.is_empty() {
                return Err(unparseable("inline image dictionary"));
            }
            continue;
        }
        let key = lx.name();
        lx.skip_ws();
        let vstart = lx.i;
        let value = lx.object()?;
        let span = vstart..lx.i;
        match key.as_slice() {
            b"CS" | b"ColorSpace" => {
                if let OperandKind::Name(n) = &value {
                    cs_name = Some(n.clone());
                    cs = Some(Operand {
                        span,
                        kind: value.clone(),
                    });
                }
            }
            b"W" | b"Width" => {
                w = if let OperandKind::Number(v) = value {
                    Some(v)
                } else {
                    None
                }
            }
            b"H" | b"Height" => {
                h = if let OperandKind::Number(v) = value {
                    Some(v)
                } else {
                    None
                }
            }
            b"BPC" | b"BitsPerComponent" => {
                bpc = if let OperandKind::Number(v) = value {
                    Some(v)
                } else {
                    None
                }
            }
            b"F" | b"Filter" => filtered = true,
            b"IM" | b"ImageMask" => mask = lx.data[span].starts_with(b"true"),
            _ => {}
        }
    }
    // one white-space byte after ID
    if lx.i < lx.data.len() && is_ws(lx.data[lx.i]) {
        lx.i += 1;
    }
    let data_start = lx.i;
    let ends_at = |p: usize| -> Option<usize> {
        let mut q = p;
        while q < lx.data.len() && is_ws(lx.data[q]) {
            q += 1;
        }
        (lx.data.get(q..q + 2) == Some(b"EI")
            && lx.data.get(q + 2).is_none_or(|&c| is_ws(c) || is_delim(c)))
        .then_some(q + 2)
    };
    let comps = if mask {
        Some(1)
    } else {
        inline_components(cs_name.as_deref())
    };
    let bpc = if mask { Some(1.0) } else { bpc };
    if let (false, Some(w), Some(h), Some(bpc), Some(n)) = (filtered, w, h, bpc, comps) {
        let row = ((w as usize) * (bpc as usize) * n).div_ceil(8);
        let len = row * (h as usize);
        if let Some(end) = ends_at(data_start + len) {
            lx.i = end;
            return Ok(cs);
        }
    }
    let mut p = data_start;
    while p + 2 <= lx.data.len() {
        if lx.data[p] == b'E'
            && lx.data[p + 1] == b'I'
            && p > data_start
            && is_ws(lx.data[p - 1])
            && lx.data.get(p + 2).is_none_or(|&c| is_ws(c) || is_delim(c))
        {
            lx.i = p + 2;
            return Ok(cs);
        }
        p += 1;
    }
    Err(unparseable("inline image without EI"))
}

/// Splits a (decoded) content stream into operators.
pub fn tokenize(data: &[u8]) -> Result<Vec<Op>, EngineError> {
    let mut lx = Lexer { data, i: 0 };
    let mut ops = Vec::new();
    let mut operands: Vec<Operand> = Vec::new();
    loop {
        lx.skip_ws();
        if lx.i >= data.len() {
            break;
        }
        let start = lx.i;
        let c = data[start];
        if c == b'/' || c == b'(' || c == b'<' || c == b'[' {
            let kind = lx.object()?;
            operands.push(Operand {
                span: start..lx.i,
                kind,
            });
            continue;
        }
        if is_delim(c) {
            return Err(unparseable("stray delimiter"));
        }
        let tok = lx.regular();
        if let Some(v) = number(tok) {
            operands.push(Operand {
                span: start..lx.i,
                kind: OperandKind::Number(v),
            });
            continue;
        }
        if matches!(tok, b"true" | b"false" | b"null") {
            operands.push(Operand {
                span: start..lx.i,
                kind: OperandKind::Other,
            });
            continue;
        }
        let operator = String::from_utf8_lossy(tok).into_owned();
        let op_start = operands.first().map_or(start, |o| o.span.start);
        if operator == "BI" {
            let inline_cs = inline_image(&mut lx)?;
            operands.clear();
            ops.push(Op {
                operator,
                operands: Vec::new(),
                span: start..lx.i,
                inline_cs,
            });
            continue;
        }
        ops.push(Op {
            operator,
            operands: std::mem::take(&mut operands),
            span: op_start..lx.i,
            inline_cs: None,
        });
    }
    Ok(ops)
}

// ---------------------------------------------------------------------------------------
// Graphics state tracking
// ---------------------------------------------------------------------------------------

/// What an operator does to the state a later piece has to restate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Cat {
    /// `cm`, clips and `gs`: order matters, kept in order.
    Ordered,
    FillSpace,
    FillColor,
    StrokeSpace,
    StrokeColor,
    /// `Tf` `Tc` `Tw` `Tz` `TL` `Ts` `Tr` and `w` `J` `j` `M` `d` `ri` `i`: the last one wins.
    Single(&'static str),
}

#[derive(Debug, Clone, Default)]
struct Frame {
    ops: Vec<(Cat, Vec<u8>)>,
}

impl Frame {
    fn set(&mut self, cat: Cat, bytes: Vec<u8>) {
        match cat {
            Cat::Ordered => {}
            Cat::FillSpace => self
                .ops
                .retain(|(c, _)| !matches!(c, Cat::FillSpace | Cat::FillColor)),
            Cat::StrokeSpace => self
                .ops
                .retain(|(c, _)| !matches!(c, Cat::StrokeSpace | Cat::StrokeColor)),
            Cat::FillColor | Cat::StrokeColor | Cat::Single(_) => {
                self.ops.retain(|(c, _)| *c != cat)
            }
        }
        self.ops.push((cat, bytes));
    }
}

const SINGLES: [&str; 14] = [
    "Tf", "Tc", "Tw", "Tz", "TL", "Ts", "Tr", "w", "J", "j", "M", "d", "ri", "i",
];
/// The text state: restated only in a piece that holds a text object.
const TEXT_STATE: [&str; 7] = ["Tf", "Tc", "Tw", "Tz", "TL", "Ts", "Tr"];
/// At most this many painted paths share one piece.
const PIECE_PATHS: usize = 256;
const PATH_CONSTRUCTION: [&str; 7] = ["m", "l", "c", "v", "y", "h", "re"];
const PATH_PAINT: [&str; 10] = ["S", "s", "f", "F", "f*", "B", "B*", "b", "b*", "n"];
const TEXT_SHOW: [&str; 4] = ["Tj", "TJ", "'", "\""];

/// Does this operator put ink on the page (or mark a point)?
fn paints(op: &str) -> bool {
    (PATH_PAINT.contains(&op) && op != "n")
        || TEXT_SHOW.contains(&op)
        || matches!(op, "sh" | "Do" | "BI" | "MP" | "DP")
}

/// One operator as it goes out: its bytes (resource names already rewritten), its name and
/// its numeric operands (for `cm` and `"`).
#[derive(Debug, Clone)]
pub struct VOp {
    pub bytes: Vec<u8>,
    pub op: String,
    pub nums: Vec<f64>,
}

impl VOp {
    fn synth(text: &str) -> Self {
        let op = text.rsplit(' ').next().unwrap_or(text).to_string();
        let nums = text
            .split(' ')
            .filter_map(|t| t.parse::<f64>().ok())
            .collect();
        Self {
            bytes: text.as_bytes().to_vec(),
            op,
            nums,
        }
    }
}

fn fmt_num(v: f64) -> String {
    if v.fract() == 0.0 && v.abs() < 1e15 {
        format!("{}", v as i64)
    } else {
        let s = format!("{v:.6}");
        let s = s.trim_end_matches('0').trim_end_matches('.');
        if s == "-0" {
            "0".into()
        } else {
            s.to_string()
        }
    }
}

fn cm_text(m: &Mat) -> String {
    format!(
        "{} {} {} {} {} {} cm",
        fmt_num(m[0]),
        fmt_num(m[1]),
        fmt_num(m[2]),
        fmt_num(m[3]),
        fmt_num(m[4]),
        fmt_num(m[5])
    )
}

/// The graphics state along the content, as far as a piece must restate it.
#[derive(Debug, Clone)]
struct State {
    frames: Vec<(Frame, Mat)>,
    marks: Vec<Vec<u8>>,
    path: Vec<u8>,
    clip: Option<&'static str>,
    in_text: bool,
}

impl State {
    fn new() -> Self {
        Self {
            frames: vec![(Frame::default(), IDENTITY)],
            marks: Vec::new(),
            path: Vec::new(),
            clip: None,
            in_text: false,
        }
    }

    fn ctm(&self) -> Mat {
        self.frames.last().map_or(IDENTITY, |f| f.1)
    }

    fn top(&mut self) -> &mut Frame {
        &mut self.frames.last_mut().expect("level 0 is never popped").0
    }

    fn feed(&mut self, v: &VOp) {
        let op = v.op.as_str();
        match op {
            "q" => {
                let ctm = self.ctm();
                self.frames.push((Frame::default(), ctm));
            }
            "Q" => {
                if self.frames.len() > 1 {
                    self.frames.pop();
                }
            }
            "cm" if v.nums.len() >= 6 => {
                let n = &v.nums[v.nums.len() - 6..];
                let m = [n[0], n[1], n[2], n[3], n[4], n[5]];
                let ctm = mul(&m, &self.ctm());
                self.frames.last_mut().expect("level 0").1 = ctm;
                self.top().set(Cat::Ordered, v.bytes.clone());
            }
            "gs" => self.top().set(Cat::Ordered, v.bytes.clone()),
            "BT" => self.in_text = true,
            "ET" => self.in_text = false,
            "W" | "W*" => self.clip = Some(if op == "W" { "W" } else { "W*" }),
            "BDC" | "BMC" => self.marks.push(v.bytes.clone()),
            "EMC" => {
                self.marks.pop();
            }
            "cs" => self.top().set(Cat::FillSpace, v.bytes.clone()),
            "CS" => self.top().set(Cat::StrokeSpace, v.bytes.clone()),
            "sc" | "scn" => self.top().set(Cat::FillColor, v.bytes.clone()),
            "SC" | "SCN" => self.top().set(Cat::StrokeColor, v.bytes.clone()),
            "g" | "rg" | "k" => {
                self.top().set(Cat::FillSpace, Vec::new());
                self.top().set(Cat::FillColor, v.bytes.clone());
            }
            "G" | "RG" | "K" => {
                self.top().set(Cat::StrokeSpace, Vec::new());
                self.top().set(Cat::StrokeColor, v.bytes.clone());
            }
            "\"" => {
                if v.nums.len() >= 2 {
                    let (aw, ac) = (v.nums[0], v.nums[1]);
                    self.top().set(
                        Cat::Single("Tw"),
                        format!("{} Tw", fmt_num(aw)).into_bytes(),
                    );
                    self.top().set(
                        Cat::Single("Tc"),
                        format!("{} Tc", fmt_num(ac)).into_bytes(),
                    );
                }
            }
            _ if PATH_CONSTRUCTION.contains(&op) => {
                self.path.extend_from_slice(&v.bytes);
                self.path.push(b'\n');
            }
            _ if PATH_PAINT.contains(&op) => {
                if let Some(w) = self.clip.take() {
                    let mut entry = std::mem::take(&mut self.path);
                    entry.extend_from_slice(w.as_bytes());
                    entry.extend_from_slice(b" n");
                    self.top().set(Cat::Ordered, entry);
                }
                self.path.clear();
            }
            _ => {
                if let Some(&name) = SINGLES.iter().find(|s| **s == op) {
                    self.top().set(Cat::Single(name), v.bytes.clone());
                }
            }
        }
    }

    /// Reopens every level and every marked-content sequence from the initial state. `text`:
    /// with the text state (`Tf` …) — only content that draws text gets it: a paragraph edit
    /// makes PDFium write the text with its own font name and drop the old one from the page's
    /// `/Font`, so a `Tf` restated in a piece without text would name an undefined font.
    fn restate(&self, text: bool) -> Vec<u8> {
        let mut out = Vec::new();
        for (frame, _) in &self.frames {
            out.extend_from_slice(b"q\n");
            for (cat, bytes) in &frame.ops {
                let text_state = matches!(cat, Cat::Single(n) if TEXT_STATE.contains(n));
                if !bytes.is_empty() && (text || !text_state) {
                    out.extend_from_slice(bytes);
                    out.push(b'\n');
                }
            }
        }
        for m in &self.marks {
            out.extend_from_slice(m);
            out.push(b'\n');
        }
        out
    }

    /// Closes every marked-content sequence and every level, back to the initial state.
    fn closers(&self) -> Vec<u8> {
        let mut out = Vec::new();
        if self.in_text {
            out.extend_from_slice(b"ET\n");
        }
        for _ in &self.marks {
            out.extend_from_slice(b"EMC\n");
        }
        for _ in &self.frames {
            out.extend_from_slice(b"Q\n");
        }
        out
    }
}

/// Cuts the inlined content into self-contained pieces (module docs): one per text object,
/// one per image / shading / nested group / marked point, and one per run of consecutive paths
/// (at most [`PIECE_PATHS`]) — a table of thousands of cells would otherwise become thousands
/// of content streams.
struct Pieces {
    state: State,
    done: Vec<Vec<u8>>,
    /// The state the current piece starts from (restated at its head).
    start: State,
    body: Vec<u8>,
    painted: bool,
    /// The current piece holds a text object.
    text: bool,
    paths: usize,
    /// Right after the last path painted in the current piece: the body length and the state
    /// then (where the piece ends when something else than a path follows).
    split: Option<(usize, State)>,
}

impl Pieces {
    fn new(state: State) -> Self {
        Self {
            start: state.clone(),
            state,
            done: Vec::new(),
            body: Vec::new(),
            painted: false,
            text: false,
            paths: 0,
            split: None,
        }
    }

    fn piece(&self, len: usize, end: &State) -> Vec<u8> {
        let mut piece = self.start.restate(self.text);
        piece.extend_from_slice(&self.body[..len]);
        piece.extend_from_slice(&end.closers());
        piece
    }

    fn reset(&mut self, start: State) {
        self.start = start;
        self.painted = false;
        self.text = false;
        self.paths = 0;
        self.split = None;
    }

    fn cut(&mut self) {
        if self.painted {
            let piece = self.piece(self.body.len(), &self.state);
            self.done.push(piece);
        }
        self.body.clear();
        self.reset(self.state.clone());
    }

    /// Ends the current run of paths where its last path was painted; what came after it (the
    /// set-up of the next object) starts the next piece.
    fn end_paths(&mut self) {
        let Some((len, at)) = self.split.take() else {
            return;
        };
        let piece = self.piece(len, &at);
        self.done.push(piece);
        self.body.drain(..len);
        self.reset(at);
    }

    fn emit(&mut self, v: VOp) {
        let op = v.op.as_str();
        let in_text = self.state.in_text;
        let graphic = paints(op) && !in_text;
        let path = graphic && PATH_PAINT.contains(&op);
        if op == "BT" && !in_text {
            self.end_paths();
            self.cut();
        } else if graphic && !path {
            self.end_paths();
        }
        if !in_text && TEXT_STATE.contains(&op) {
            // outside a text object the text state only matters to the next one, whose piece
            // restates it
            self.state.feed(&v);
            return;
        }
        if paints(op) {
            self.painted = true;
        }
        if op == "BT" {
            self.text = true;
        }
        self.body.extend_from_slice(&v.bytes);
        self.body.push(b'\n');
        self.state.feed(&v);
        // Whatever a later edit makes PDFium regenerate, it regenerates nothing else with it
        // that PDFium could not write back (paths it can).
        if op == "ET" || (graphic && !path) {
            self.cut();
        } else if path {
            self.paths += 1;
            if self.paths >= PIECE_PATHS {
                self.cut();
            } else {
                self.split = Some((self.body.len(), self.state.clone()));
            }
        }
    }

    fn finish(mut self) -> Vec<Vec<u8>> {
        self.cut();
        self.done
    }
}

// ---------------------------------------------------------------------------------------
// Resources
// ---------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Res {
    Font,
    XObject,
    ExtGState,
    ColorSpace,
    Pattern,
    Shading,
    Properties,
}

impl Res {
    fn key(self) -> &'static [u8] {
        match self {
            Res::Font => b"Font",
            Res::XObject => b"XObject",
            Res::ExtGState => b"ExtGState",
            Res::ColorSpace => b"ColorSpace",
            Res::Pattern => b"Pattern",
            Res::Shading => b"Shading",
            Res::Properties => b"Properties",
        }
    }
    fn prefix(self) -> &'static str {
        match self {
            Res::Font => "SPf",
            Res::XObject => "SPx",
            Res::ExtGState => "SPg",
            Res::ColorSpace => "SPc",
            Res::Pattern => "SPp",
            Res::Shading => "SPs",
            Res::Properties => "SPo",
        }
    }
}

fn deref<'a>(doc: &'a Document, o: &'a Object) -> &'a Object {
    doc.dereference(o).map(|(_, o)| o).unwrap_or(o)
}

fn dict_of<'a>(doc: &'a Document, o: &'a Object) -> Option<&'a Dictionary> {
    match deref(doc, o) {
        Object::Dictionary(d) => Some(d),
        Object::Stream(s) => Some(&s.dict),
        _ => None,
    }
}

/// The page's resources as a direct dictionary it owns: its own `/Resources` or the nearest
/// inherited one, with every category we may add to copied (a shared sub-dictionary is never
/// changed).
struct PageRes {
    dict: Dictionary,
    /// The page's resources as they were before any name was added: what PDFium looks a name
    /// up in when a form's `/Resources` lacks that whole category.
    orig: Dictionary,
    /// Numbering of the fresh names.
    counter: usize,
}

impl PageRes {
    fn load(doc: &Document, page_id: ObjectId) -> Self {
        let mut node = doc.get_dictionary(page_id).ok();
        let mut dict = Dictionary::new();
        let mut guard = 0;
        while let Some(n) = node {
            if let Ok(r) = n.get(b"Resources") {
                if let Some(d) = dict_of(doc, r) {
                    dict = d.clone();
                }
                break;
            }
            guard += 1;
            node = n
                .get(b"Parent")
                .ok()
                .and_then(|p| p.as_reference().ok())
                .and_then(|id| doc.get_dictionary(id).ok())
                .filter(|_| guard < 64);
        }
        // Categories as direct copies, so adding to them never touches another page.
        for cat in [
            Res::Font,
            Res::XObject,
            Res::ExtGState,
            Res::ColorSpace,
            Res::Pattern,
            Res::Shading,
            Res::Properties,
        ] {
            if let Ok(o) = dict.get(cat.key()) {
                let copy = dict_of(doc, o).cloned().unwrap_or_default();
                dict.set(cat.key(), Object::Dictionary(copy));
            }
        }
        Self {
            orig: dict.clone(),
            dict,
            counter: 0,
        }
    }

    /// `name` in the page's own (original) resources.
    fn lookup(&self, cat: Res, name: &[u8]) -> Option<&Object> {
        match self.orig.get(cat.key()) {
            Ok(Object::Dictionary(d)) => d.get(name).ok(),
            _ => None,
        }
    }

    fn add(&mut self, cat: Res, value: Object) -> Vec<u8> {
        if !matches!(self.dict.get(cat.key()), Ok(Object::Dictionary(_))) {
            self.dict
                .set(cat.key(), Object::Dictionary(Dictionary::new()));
        }
        let Ok(Object::Dictionary(sub)) = self.dict.get_mut(cat.key()) else {
            unreachable!("just set");
        };
        loop {
            self.counter += 1;
            let name = format!("{}{}", cat.prefix(), self.counter).into_bytes();
            if !sub.has(&name) {
                sub.set(name.clone(), value);
                return name;
            }
        }
    }
}

/// The resource names of one content stream: the page's (kept as they are) or a form's (each
/// mapped to a fresh page name on first use).
///
/// A name is looked up the way PDFium does (`CPDF_StreamContentParser::FindResourceObj`): in the
/// form's `/Resources` when it has that category's dictionary (a name missing there is
/// undefined); in the **page's** resources when the form's `/Resources` lacks the whole
/// category (Word / PowerPoint forms with `/ProcSet` only, fonts or icons that live on the
/// page); a form without `/Resources` uses its parent's. What the page defines keeps its name.
struct Scope {
    /// `None`: the page itself, or a form without `/Resources` (its names are its parent's).
    res: Option<Dictionary>,
    parent: Option<Box<Scope>>,
    /// Form space → page default space (for patterns).
    space: Mat,
    names: HashMap<(Res, Vec<u8>), Vec<u8>>,
}

impl Scope {
    fn page() -> Self {
        Self {
            res: None,
            parent: None,
            space: IDENTITY,
            names: HashMap::new(),
        }
    }

    /// Where `name` is defined for PDFium: in the form's own category dictionary (`Some(Some)`),
    /// nowhere although the form has that category (`Some(None)`: undefined), or — the form's
    /// `/Resources` lacks the category — in the page's resources (`None`).
    fn own<'d>(
        res: &'d Dictionary,
        doc: &'d Document,
        cat: Res,
        name: &[u8],
    ) -> Option<Option<&'d Object>> {
        let category = res.get(cat.key()).ok().and_then(|o| dict_of(doc, o))?;
        Some(category.get(name).ok())
    }

    /// The object a name refers to in this scope (for `Do`: which XObject).
    fn resolve<'d>(
        &self,
        doc: &'d Document,
        page: &'d PageRes,
        cat: Res,
        name: &[u8],
    ) -> Option<Object> {
        match &self.res {
            Some(res) => match Self::own(res, doc, cat, name) {
                Some(found) => found.cloned(),
                None => page.lookup(cat, name).cloned(),
            },
            None => match &self.parent {
                Some(p) => p.resolve(doc, page, cat, name),
                None => page.lookup(cat, name).cloned(),
            },
        }
    }

    /// The page-level name for `name`.
    fn rename(&mut self, doc: &mut Document, page: &mut PageRes, cat: Res, name: &[u8]) -> Vec<u8> {
        if self.res.is_none() {
            return match &mut self.parent {
                Some(p) => p.rename(doc, page, cat, name),
                None => name.to_vec(),
            };
        }
        if let Some(n) = self.names.get(&(cat, name.to_vec())) {
            return n.clone();
        }
        let (value, from_page) = match self.res.as_ref().and_then(|r| Self::own(r, doc, cat, name))
        {
            Some(found) => (found.cloned(), false),
            None => (page.lookup(cat, name).cloned(), true),
        };
        let new = match value {
            // A name PDFium does not find either stays undefined (never falls through to a
            // page resource of the same name when the form has that category).
            None => format!("SPmissing{}", page.counter + 1).into_bytes(),
            Some(v) if cat == Res::Pattern && !is_identity(&self.space) => {
                // a pattern's space is the space of the content that uses it, wherever the
                // resource itself lives
                let moved = reanchor_pattern(doc, &v, &self.space).unwrap_or(v);
                page.add(cat, moved)
            }
            // the page's own resource: after inlining, its name means the same object
            Some(_) if from_page => name.to_vec(),
            Some(v) => page.add(cat, v),
        };
        if new.starts_with(b"SPmissing") {
            page.counter += 1;
        }
        self.names.insert((cat, name.to_vec()), new.clone());
        new
    }
}

/// A copy of a pattern whose `/Matrix` maps into the page's space instead of the form's.
fn reanchor_pattern(doc: &mut Document, value: &Object, space: &Mat) -> Option<Object> {
    let obj = deref(doc, value).clone();
    let matrix = |d: &Dictionary| -> Mat {
        d.get(b"Matrix")
            .ok()
            .and_then(|m| m.as_array().ok())
            .filter(|a| a.len() == 6)
            .map(|a| {
                let mut m = [0.0; 6];
                for (k, v) in a.iter().enumerate() {
                    m[k] = v.as_float().map(f64::from).unwrap_or(0.0);
                }
                m
            })
            .unwrap_or(IDENTITY)
    };
    let to_array = |m: Mat| Object::Array(m.iter().map(|&v| Object::Real(v as f32)).collect());
    let copy = match obj {
        Object::Stream(mut s) => {
            let m = mul(&matrix(&s.dict), space);
            s.dict.set("Matrix", to_array(m));
            Object::Stream(s)
        }
        Object::Dictionary(mut d) => {
            let m = mul(&matrix(&d), space);
            d.set("Matrix", to_array(m));
            Object::Dictionary(d)
        }
        _ => return None,
    };
    Some(Object::Reference(doc.add_object(copy)))
}

/// Encodes a name for a content stream.
fn name_bytes(name: &[u8]) -> Vec<u8> {
    let mut out = vec![b'/'];
    for &c in name {
        if c.is_ascii_graphic() && !is_delim(c) && c != b'#' {
            out.push(c);
        } else {
            out.extend_from_slice(format!("#{c:02X}").as_bytes());
        }
    }
    out
}

/// A form XObject as read from the file.
struct Form {
    dict: Dictionary,
    content: Vec<u8>,
}

/// What a `Do` draws, as far as counting PDFium's form objects goes.
enum Drawn {
    /// Not a form (an image, a missing name): PDFium makes no form object of it.
    Other,
    /// A form whose content lopdf cannot decode (an abbreviated `/Filter /Fl`, a filter lopdf
    /// lacks) — PDFium may well draw it, so it still counts as a form object.
    Opaque,
    Form(Form),
}

fn drawn(doc: &Document, value: &Object) -> Drawn {
    let Object::Stream(s) = deref(doc, value) else {
        return Drawn::Other;
    };
    if s.dict.get(b"Subtype").and_then(Object::as_name).ok() != Some(b"Form".as_slice()) {
        return Drawn::Other;
    }
    let content = s
        .decompressed_content()
        .ok()
        .or_else(|| s.dict.get(b"Filter").is_err().then(|| s.content.clone()));
    match content {
        Some(content) => Drawn::Form(Form {
            dict: s.dict.clone(),
            content,
        }),
        None => Drawn::Opaque,
    }
}

fn form_of(doc: &Document, value: &Object) -> Option<Form> {
    match drawn(doc, value) {
        Drawn::Form(f) => Some(f),
        _ => None,
    }
}

fn form_matrix(d: &Dictionary) -> Mat {
    d.get(b"Matrix")
        .ok()
        .and_then(|m| m.as_array().ok())
        .filter(|a| a.len() == 6)
        .map(|a| {
            let mut m = [0.0; 6];
            for (k, v) in a.iter().enumerate() {
                m[k] = v.as_float().map(f64::from).unwrap_or(0.0);
            }
            m
        })
        .unwrap_or(IDENTITY)
}

fn form_bbox(d: &Dictionary) -> Option<[f64; 4]> {
    let a = d.get(b"BBox").ok()?.as_array().ok()?;
    if a.len() != 4 {
        return None;
    }
    let v: Vec<f64> = a
        .iter()
        .map(|o| o.as_float().map(f64::from).unwrap_or(0.0))
        .collect();
    Some([
        v[0].min(v[2]),
        v[1].min(v[3]),
        v[0].max(v[2]),
        v[1].max(v[3]),
    ])
}

/// Does a form draw text, itself or through a form it draws (bounded depth)? `page`: the page's
/// resources, where a form without an `/XObject` dictionary finds its forms (see [`Scope`]).
fn form_has_text(doc: &Document, page: &PageRes, form: &Form, depth: usize) -> bool {
    let Ok(ops) = tokenize(&form.content) else {
        return false;
    };
    if ops.iter().any(|o| TEXT_SHOW.contains(&o.operator.as_str())) {
        return true;
    }
    if depth > 8 {
        return false;
    }
    let res = form
        .dict
        .get(b"Resources")
        .ok()
        .and_then(|r| dict_of(doc, r));
    let own = res
        .and_then(|r| r.get(b"XObject").ok())
        .and_then(|x| dict_of(doc, x));
    let lookup = |n: &[u8]| -> Option<Object> {
        match own {
            Some(x) => x.get(n).ok().cloned(),
            None => page.lookup(Res::XObject, n).cloned(),
        }
    };
    ops.iter()
        .filter(|o| o.operator == "Do")
        .filter_map(|o| o.operands.last().and_then(|n| n.name()))
        .filter_map(lookup)
        .filter_map(|v| form_of(doc, &v))
        .any(|f| form_has_text(doc, page, &f, depth + 1))
}

// ---------------------------------------------------------------------------------------
// The rewrite
// ---------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Every object of the form comes out.
    Full,
    /// Only the text comes out; the rest stays a group (a copy of the form without its text).
    TextOnly,
}

/// Which `Do` to inline and how deep.
#[derive(Debug, Clone)]
pub struct Plan {
    pub page_index: u16,
    /// The form's place among the page's form `Do`s (PDFium's form objects, in order).
    pub ordinal: usize,
    /// PDFium's matrix of that form object — the CTM at its `Do` (PDFium applies the form's
    /// own `/Matrix` inside the form, to its children); the `Do` must match it.
    pub matrix: Mat,
    /// Nested forms down to the one holding the clicked text: each form's place among its
    /// parent's form `Do`s (every `/Subtype /Form`, readable or not) and PDFium's matrix of it —
    /// the CTM at its `Do` in the parent's space *with* the parent's `/Matrix`, without its own.
    pub chain: Vec<(usize, Mat)>,
    pub mode: Mode,
    /// Also inline every nested form that draws text (the Inspector's 그룹 해제).
    pub deep: bool,
}

/// What the rewrite did (for tests and the result).
#[derive(Debug, Clone, Default)]
pub struct Rewritten {
    pub bytes: Vec<u8>,
    pub pieces: usize,
    pub forms_inlined: usize,
}

fn not_located(what: &str) -> EngineError {
    EngineError::new(
        ErrorCode::Unsupported,
        format!("the group could not be located in the page content ({what})"),
    )
    .with_detail("groupNotFound")
}

struct Ctx<'a> {
    doc: &'a mut Document,
    page: PageRes,
    inlined: usize,
    mode: Mode,
    deep: bool,
}

/// The operator as it goes out, with its resource names rewritten through `scope`.
fn renamed(ctx: &mut Ctx<'_>, scope: &mut Scope, data: &[u8], op: &Op) -> VOp {
    let target: Option<(Res, &Operand)> = match op.operator.as_str() {
        "Tf" => op.operands.first().map(|o| (Res::Font, o)),
        "Do" => op.operands.last().map(|o| (Res::XObject, o)),
        "gs" => op.operands.last().map(|o| (Res::ExtGState, o)),
        "sh" => op.operands.last().map(|o| (Res::Shading, o)),
        "cs" | "CS" => op
            .operands
            .last()
            .filter(|o| {
                !matches!(
                    o.name(),
                    Some(b"DeviceGray" | b"DeviceRGB" | b"DeviceCMYK" | b"Pattern")
                )
            })
            .map(|o| (Res::ColorSpace, o)),
        "scn" | "SCN" => op
            .operands
            .last()
            .filter(|o| o.name().is_some())
            .map(|o| (Res::Pattern, o)),
        "BDC" | "DP" => op
            .operands
            .get(1)
            .filter(|o| o.name().is_some())
            .map(|o| (Res::Properties, o)),
        "BI" => op
            .inline_cs
            .as_ref()
            .filter(|o| {
                !matches!(
                    o.name(),
                    Some(
                        b"G" | b"RGB"
                            | b"CMYK"
                            | b"I"
                            | b"DeviceGray"
                            | b"DeviceRGB"
                            | b"DeviceCMYK"
                            | b"Indexed"
                    )
                )
            })
            .map(|o| (Res::ColorSpace, o)),
        _ => None,
    };
    let mut bytes = data[op.span.clone()].to_vec();
    if let Some((cat, operand)) = target {
        if let Some(name) = operand.name() {
            let new = scope.rename(ctx.doc, &mut ctx.page, cat, name);
            if new != name {
                let (s, e) = (
                    operand.span.start - op.span.start,
                    operand.span.end - op.span.start,
                );
                bytes.splice(s..e, name_bytes(&new));
            }
        }
    }
    VOp {
        bytes,
        op: op.operator.clone(),
        nums: op.operands.iter().filter_map(Operand::number).collect(),
    }
}

/// Inlines `form` (drawn by a `Do` in the state `out.state`) into `out`.
fn expand(
    ctx: &mut Ctx<'_>,
    out: &mut Pieces,
    parent: Scope,
    form: &Form,
    chain: &[(usize, Mat)],
) -> Result<Scope, EngineError> {
    ctx.inlined += 1;
    let last = chain.is_empty();
    let text_only = last && ctx.mode == Mode::TextOnly;
    if text_only {
        // The group stays, without its text: a new form (the original may be drawn elsewhere).
        let stripped = strip_text(&form.content)?;
        let mut dict = form.dict.clone();
        for k in [b"Length".as_slice(), b"Filter", b"DecodeParms"] {
            dict.remove(k);
        }
        let mut stream = Stream::new(dict, stripped);
        let _ = stream.compress();
        let id = ctx.doc.add_object(stream);
        let name = ctx.page.add(Res::XObject, Object::Reference(id));
        let mut do_op = name_bytes(&name);
        do_op.extend_from_slice(b" Do");
        out.emit(VOp {
            bytes: do_op,
            op: "Do".into(),
            nums: Vec::new(),
        });
    }

    // The CTM at this form's `Do`, before its `/Matrix`: PDFium applies a form's `/Matrix`
    // inside the form's own parse, so the matrix it reports for a nested form is relative to
    // this space (it includes this form's `/Matrix`, and not the nested form's own).
    let do_ctm = out.state.ctm();
    out.emit(VOp::synth("q"));
    let m = form_matrix(&form.dict);
    if !is_identity(&m) {
        out.emit(VOp::synth(&cm_text(&m)));
    }
    if let Some([l, b, r, t]) = form_bbox(&form.dict) {
        out.emit(VOp::synth(&format!(
            "{} {} {} {} re",
            fmt_num(l),
            fmt_num(b),
            fmt_num(r - l),
            fmt_num(t - b)
        )));
        out.emit(VOp::synth("W"));
        out.emit(VOp::synth("n"));
    }
    let oc = form.dict.get(b"OC").ok().cloned();
    if let Some(oc) = &oc {
        let name = ctx.page.add(Res::Properties, oc.clone());
        let mut b = b"/OC ".to_vec();
        b.extend_from_slice(&name_bytes(&name));
        b.extend_from_slice(b" BDC");
        out.emit(VOp {
            bytes: b,
            op: "BDC".into(),
            nums: Vec::new(),
        });
    }
    let base_frames = out.state.frames.len();
    let base_marks = out.state.marks.len();
    let res = form
        .dict
        .get(b"Resources")
        .ok()
        .and_then(|r| dict_of(ctx.doc, r))
        .cloned();
    let mut scope = Scope {
        res,
        parent: Some(Box::new(parent)),
        space: out.state.ctm(),
        names: HashMap::new(),
    };
    let ops = tokenize(&form.content)?;
    let mut ordinal = 0usize;
    for op in &ops {
        let name = op.operator.as_str();
        if name == "Do" {
            let child = op
                .operands
                .last()
                .and_then(|o| o.name())
                .and_then(|n| scope.resolve(ctx.doc, &ctx.page, Res::XObject, n))
                .map_or(Drawn::Other, |v| drawn(ctx.doc, &v));
            // every form counts toward the ordinal, readable or not (PDFium's form objects)
            let here = ordinal;
            if !matches!(child, Drawn::Other) {
                ordinal += 1;
            }
            let on_chain =
                !matches!(child, Drawn::Other) && chain.first().is_some_and(|(k, _)| *k == here);
            if on_chain && matches!(child, Drawn::Opaque) {
                return Err(not_located("nested form content cannot be decoded"));
            }
            if let Drawn::Form(child) = child {
                if on_chain {
                    // PDFium's matrix of a nested form: the CTM at its `Do` relative to the
                    // parent's space before the parent's `/Matrix` (see `do_ctm`)
                    let rel = invert(&do_ctm)
                        .map(|inv| mul(&out.state.ctm(), &inv))
                        .unwrap_or(IDENTITY);
                    if !close(&rel, &chain[0].1) {
                        return Err(not_located("nested form matrix"));
                    }
                }
                if on_chain
                    || (!text_only && ctx.deep && form_has_text(ctx.doc, &ctx.page, &child, 0))
                {
                    let rest = if on_chain { &chain[1..] } else { &[][..] };
                    scope = expand(ctx, out, scope, &child, rest)?;
                    continue;
                }
            }
        }
        if name == "Q" && out.state.frames.len() <= base_frames {
            continue; // an unbalanced Q never reaches outside the form
        }
        if name == "EMC" && out.state.marks.len() <= base_marks {
            continue;
        }
        if text_only {
            if PATH_PAINT.contains(&name) {
                out.emit(VOp::synth("n"));
                continue;
            }
            if matches!(name, "sh" | "Do" | "BI" | "MP" | "DP") {
                continue;
            }
        }
        let v = renamed(ctx, &mut scope, &form.content, op);
        out.emit(v);
    }
    if out.state.in_text {
        out.emit(VOp::synth("ET"));
    }
    while out.state.marks.len() > base_marks {
        out.emit(VOp::synth("EMC"));
    }
    while out.state.frames.len() > base_frames {
        out.emit(VOp::synth("Q"));
    }
    if oc.is_some() {
        out.emit(VOp::synth("EMC"));
    }
    out.emit(VOp::synth("Q"));
    Ok(*scope.parent.take().expect("set above"))
}

/// The form's content without its text-showing operators (the rest stays byte for byte).
fn strip_text(content: &[u8]) -> Result<Vec<u8>, EngineError> {
    let ops = tokenize(content)?;
    let mut out = Vec::with_capacity(content.len());
    for op in &ops {
        if TEXT_SHOW.contains(&op.operator.as_str()) {
            continue;
        }
        out.extend_from_slice(&content[op.span.clone()]);
        out.push(b'\n');
    }
    Ok(out)
}

/// The spans of the `Q` operators PDFium ignores: those with no `q` to restore (the graphics
/// state stack is empty — PDFium reads all of a page's streams as one).
fn stray_restores(ops: &[Op]) -> Vec<Range<usize>> {
    let mut depth = 0usize;
    let mut out = Vec::new();
    for op in ops {
        match op.operator.as_str() {
            "q" => depth += 1,
            "Q" if depth == 0 => out.push(op.span.clone()),
            "Q" => depth -= 1,
            _ => {}
        }
    }
    out
}

/// `data[range]` without the `skip` spans inside it.
fn without(data: &[u8], range: Range<usize>, skip: &[Range<usize>]) -> Vec<u8> {
    let mut out = Vec::with_capacity(range.len());
    let mut at = range.start;
    for r in skip
        .iter()
        .filter(|r| r.start >= range.start && r.end <= range.end)
    {
        out.extend_from_slice(&data[at..r.start]);
        out.push(b' ');
        at = r.end;
    }
    out.extend_from_slice(&data[at..range.end]);
    out
}

/// Rewrites page `plan.page_index` of `bytes` with the planned `Do` inlined (module docs).
pub fn rewrite(bytes: &[u8], plan: &Plan) -> Result<Rewritten, EngineError> {
    let mut doc = Document::load_mem(bytes).map_err(|e| lopdf_error("parse", e))?;
    let page_id = *doc
        .get_pages()
        .get(&(plan.page_index as u32 + 1))
        .ok_or_else(|| EngineError::not_found(format!("page {}", plan.page_index)))?;
    let stream_ids = doc.get_page_contents(page_id);
    if stream_ids.is_empty() {
        return Err(not_located("the page has no content stream"));
    }
    // The page's streams, decoded, one after the other (PDFium reads them as one).
    let mut data = Vec::new();
    let mut bounds = Vec::with_capacity(stream_ids.len());
    for id in &stream_ids {
        let stream = doc
            .get_object(*id)
            .and_then(Object::as_stream)
            .map_err(|e| lopdf_error("content stream", e))?;
        let decoded = stream
            .decompressed_content()
            .map_err(|_| unparseable("a page content stream cannot be decoded"))?;
        let start = data.len();
        data.extend_from_slice(&decoded);
        bounds.push(start..data.len());
        data.push(b'\n');
    }
    let ops = tokenize(&data)?;
    let strays = stray_restores(&ops);
    let page = PageRes::load(&doc, page_id);
    let mut ctx = Ctx {
        doc: &mut doc,
        page,
        inlined: 0,
        mode: plan.mode,
        deep: plan.deep,
    };

    // Walk the page content to the planned `Do`, tracking the state.
    let page_scope = Scope::page();
    let mut state = State::new();
    let mut ordinal = 0usize;
    let mut target: Option<(usize, Form)> = None;
    for (k, op) in ops.iter().enumerate() {
        if op.operator == "Do" {
            let what = op
                .operands
                .last()
                .and_then(|o| o.name())
                .and_then(|n| page_scope.resolve(ctx.doc, &ctx.page, Res::XObject, n))
                .map_or(Drawn::Other, |v| drawn(ctx.doc, &v));
            // Every form `Do` is one of PDFium's form objects, whether lopdf can read its
            // content or not — skipping one would shift the ordinal onto the next group.
            match what {
                Drawn::Other => {}
                _ if ordinal != plan.ordinal => ordinal += 1,
                Drawn::Opaque => return Err(not_located("the group's content cannot be decoded")),
                Drawn::Form(form) => {
                    // PDFium's matrix of a form object is the CTM at its `Do`; the form's
                    // `/Matrix` is applied inside the form (to its children).
                    if !close(&state.ctm(), &plan.matrix) {
                        return Err(not_located("form matrix"));
                    }
                    target = Some((k, form));
                    break;
                }
            }
        }
        state.feed(&VOp {
            bytes: data[op.span.clone()].to_vec(),
            op: op.operator.clone(),
            nums: op.operands.iter().filter_map(Operand::number).collect(),
        });
    }
    let Some((k, form)) = target else {
        return Err(not_located("no such Do"));
    };
    let do_span = ops[k].span.clone();
    let Some(si) = bounds.iter().position(|b| b.contains(&do_span.start)) else {
        return Err(not_located("Do outside the streams"));
    };
    if do_span.end > bounds[si].end {
        return Err(not_located("Do split across streams"));
    }

    let mut pieces = Pieces::new(state.clone());
    expand(&mut ctx, &mut pieces, page_scope, &form, &plan.chain)?;
    let inlined = ctx.inlined;
    let inner = pieces.finish();
    let page_res = ctx.page;

    // The stream holding the `Do`: before (closing every level) | pieces | after (reopening).
    // A `Q` with nothing to restore (more `Q` than `q` so far) is ignored by PDFium but would
    // pop the leading `q` or a level the pieces restate, so it is left out wherever it is.
    let mut before = Vec::new();
    if si == 0 {
        before.extend_from_slice(b"q\n");
    }
    before.extend_from_slice(&without(&data, bounds[si].start..do_span.start, &strays));
    before.push(b'\n');
    before.extend_from_slice(&state.closers());
    let text_after = ops[k + 1..].iter().any(|o| o.operator == "BT");
    let mut after = state.restate(text_after);
    after.extend_from_slice(&without(&data, do_span.end..bounds[si].end, &strays));
    after.push(b'\n');

    let mut new_stream = |content: Vec<u8>| -> ObjectId {
        let mut s = Stream::new(Dictionary::new(), content);
        let _ = s.compress();
        doc.add_object(s)
    };
    let mut contents: Vec<Object> = Vec::with_capacity(stream_ids.len() + inner.len() + 3);
    if si > 0 {
        contents.push(Object::Reference(new_stream(b"q\n".to_vec())));
    }
    for (i, id) in stream_ids.iter().enumerate() {
        if i != si {
            let b = &bounds[i];
            if strays.iter().any(|r| b.contains(&r.start)) {
                contents.push(Object::Reference(new_stream(without(
                    &data,
                    b.clone(),
                    &strays,
                ))));
            } else {
                contents.push(Object::Reference(*id));
            }
            continue;
        }
        contents.push(Object::Reference(new_stream(before.clone())));
        for piece in &inner {
            contents.push(Object::Reference(new_stream(piece.clone())));
        }
        contents.push(Object::Reference(new_stream(after.clone())));
    }
    let pieces_count = inner.len();
    let page_dict = doc
        .get_object_mut(page_id)
        .and_then(Object::as_dict_mut)
        .map_err(|e| lopdf_error("page", e))?;
    page_dict.set("Contents", Object::Array(contents));
    page_dict.set("Resources", Object::Dictionary(page_res.dict));
    let mut out = Vec::new();
    doc.save_to(&mut out).map_err(|e| lopdf_error("save", e))?;
    Ok(Rewritten {
        bytes: out,
        pieces: pieces_count,
        forms_inlined: inlined,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ops_of(s: &str) -> Vec<String> {
        tokenize(s.as_bytes())
            .unwrap()
            .into_iter()
            .map(|o| o.operator)
            .collect()
    }

    #[test]
    fn tokenizes_strings_arrays_dicts_and_inline_images() {
        let s = "q 1 0 0 1 60 600 cm BT /F1 12 Tf (a (nested) \\) paren) Tj [(x) -20 (y)] TJ ET \
                 /P <</MCID 0 /Alt (z)>> BDC EMC BI /W 2 /H 1 /CS /RGB /BPC 8 ID \x00\x01\x02EI\x03\x04\x05 EI Q";
        assert_eq!(
            ops_of(s),
            ["cm", "BT", "Tf", "Tj", "TJ", "ET", "BDC", "EMC", "BI", "Q"]
                .iter()
                .map(|s| s.to_string())
                .collect::<Vec<_>>()
                .into_iter()
                .fold(vec!["q".to_string()], |mut v, s| {
                    v.push(s);
                    v
                })
        );
        let ops = tokenize(s.as_bytes()).unwrap();
        let bi = ops.iter().find(|o| o.operator == "BI").unwrap();
        assert!(s.as_bytes()[bi.span.clone()].ends_with(b" EI"));
        assert_eq!(
            bi.inline_cs.as_ref().unwrap().name(),
            Some(b"RGB".as_slice())
        );
        let tf = ops.iter().find(|o| o.operator == "Tf").unwrap();
        assert_eq!(tf.operands[0].name(), Some(b"F1".as_slice()));
        assert_eq!(&s.as_bytes()[tf.span.clone()], b"/F1 12 Tf");
    }

    #[test]
    fn names_decode_hex_escapes() {
        let ops = tokenize(b"/F#231 1 Tf").unwrap();
        assert_eq!(ops[0].operands[0].name(), Some(b"F#1".as_slice()));
        assert_eq!(name_bytes(b"F#1"), b"/F#231".to_vec());
    }

    #[test]
    fn restate_reopens_levels_and_marks() {
        let mut st = State::new();
        for t in [
            "1 0 0 rg",
            "q",
            "2 0 0 2 10 10 cm",
            "0 0 10 10 re",
            "W",
            "n",
            "/F1 12 Tf",
            "0.5 Tc",
        ] {
            st.feed(&VOp::synth(t));
        }
        st.feed(&VOp {
            bytes: b"/OC /oc1 BDC".to_vec(),
            op: "BDC".into(),
            nums: vec![],
        });
        let r = String::from_utf8(st.restate(true)).unwrap();
        assert_eq!(
            r,
            "q\n1 0 0 rg\nq\n2 0 0 2 10 10 cm\n0 0 10 10 re\nW n\n/F1 12 Tf\n0.5 Tc\n/OC /oc1 BDC\n"
        );
        assert_eq!(String::from_utf8(st.closers()).unwrap(), "EMC\nQ\nQ\n");
        assert_eq!(st.ctm(), [2.0, 0.0, 0.0, 2.0, 10.0, 10.0]);
        // a later colour of the same kind replaces the earlier one
        st.feed(&VOp::synth("/F2 9 Tf"));
        st.feed(&VOp::synth("0 g"));
        let r = String::from_utf8(st.restate(true)).unwrap();
        assert!(!r.contains("/F1 12 Tf") && r.contains("/F2 9 Tf") && r.contains("0 g"));
    }

    #[test]
    fn pieces_cut_at_text_objects_and_drop_state_only_runs() {
        let mut p = Pieces::new(State::new());
        for t in [
            "q",
            "1 0 0 1 5 5 cm",
            "BT",
            "/F1 9 Tf",
            "(a) Tj",
            "ET",
            "BT",
            "(b) Tj",
            "ET",
            "0 0 1 1 re",
            "f",
            "Q",
        ] {
            p.emit(VOp::synth(t));
        }
        let pieces: Vec<String> = p
            .finish()
            .into_iter()
            .map(|b| String::from_utf8(b).unwrap())
            .collect();
        assert_eq!(pieces.len(), 3, "{pieces:#?}");
        // the second text object restates the font set in the first
        assert!(pieces[1].contains("/F1 9 Tf") && pieces[1].contains("1 0 0 1 5 5 cm"));
        for p in &pieces {
            let q = p.matches("q\n").count();
            let big_q = p.matches("Q\n").count();
            assert_eq!(q, big_q, "balanced: {p}");
        }
    }

    fn pieces_of(ops: &[&str]) -> Vec<String> {
        let mut p = Pieces::new(State::new());
        for t in ops {
            p.emit(VOp::synth(t));
        }
        p.finish()
            .into_iter()
            .map(|b| String::from_utf8(b).unwrap())
            .collect()
    }

    #[test]
    fn consecutive_paths_share_a_piece_and_only_text_gets_the_text_state() {
        let pieces = pieces_of(&[
            "/F1 9 Tf",
            "0 0 1 1 re",
            "f",
            "1 1 1 1 re",
            "f",
            "BT",
            "(a) Tj",
            "ET",
            "2 2 1 1 re",
            "f",
            "q",
            "10 0 0 10 0 0 cm",
            "/Im0 Do",
            "Q",
            "3 3 1 1 re",
            "S",
        ]);
        assert_eq!(pieces.len(), 5, "{pieces:#?}");
        assert!(pieces[0].contains("0 0 1 1 re") && pieces[0].contains("1 1 1 1 re"));
        assert!(pieces[1].contains("/F1 9 Tf") && pieces[1].contains("(a) Tj"));
        assert!(pieces[2].contains("2 2 1 1 re") && !pieces[2].contains("cm"));
        assert!(pieces[3].contains("10 0 0 10 0 0 cm\n/Im0 Do"));
        assert!(pieces[4].contains("3 3 1 1 re"));
        for (k, p) in pieces.iter().enumerate() {
            assert_eq!(p.contains("Tf"), k == 1, "text state only with text: {p}");
            assert_eq!(
                p.matches("q\n").count(),
                p.matches("Q\n").count(),
                "balanced: {p}"
            );
        }
        // a long run of paths is cut every PIECE_PATHS paths
        let many: Vec<String> = (0..PIECE_PATHS * 2 + 1)
            .flat_map(|k| [format!("{k} 0 1 1 re"), "f".to_string()])
            .collect();
        let refs: Vec<&str> = many.iter().map(String::as_str).collect();
        assert_eq!(pieces_of(&refs).len(), 3);
    }

    #[test]
    fn matrices() {
        let a = [2.0, 0.0, 0.0, 2.0, 10.0, 20.0];
        let inv = invert(&a).unwrap();
        assert!(close(&mul(&a, &inv), &IDENTITY));
        assert_eq!(fmt_num(0.75), "0.75");
        assert_eq!(fmt_num(-0.0), "0");
        assert_eq!(fmt_num(600.0), "600");
    }
}
