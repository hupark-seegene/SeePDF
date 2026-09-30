//! 그룹 해제 → 편집 corpus (v0.3.1). Every case is a test of its own (`case_*`): the UI's call
//! sequence must end in an edited, saved and reopened page — or, where the case says so, in a
//! refusal that names the reason **before** anything was ungrouped, or in a 그룹 해제 that is
//! refused because the page would look different (and then changed nothing). The whole corpus
//! with a report:
//!
//! ```sh
//! cd src-tauri && cargo test --release --test ungroup_corpus -- --nocapture --test-threads=1
//! ```
//!
//! 1. `generate_corpus` writes grouped-text PDFs to `fixtures/out/ungroup-corpus/` (gitignored):
//!    what Korean office users produce — plain forms, Office/Hancom transparency groups, real
//!    transparency, nested forms, shared forms, /Matrix + /BBox, Hangul CID fonts, tagged / OCG
//!    content, whole-page forms, inline images / shadings, font resources that collide between
//!    the page and a form. PNGs of every look change land in `png/`, saved outputs in `saved/`.
//! 2. For every corpus file and every repo fixture page that has text inside a Form XObject,
//!    `run_case` drives the **same call sequence the UI takes** (`src/edit/actions.ts`
//!    `beginParagraphEdit` → `src/edit/ungroup.ts` `confirmUngroup` → re-probe → `commitParagraph`):
//!    `list_page_objects` → `probe_paragraph(at)` → refused `insideXObject`? →
//!    `ungroup_object(probe.objectIds[0], probe.docGeneration)` → probe again (the UI recurses) →
//!    `edit_paragraph` dry run `push` (→ `overlap` when unwritable) → real write → save → reopen →
//!    text extractable + render diff outside the edited paragraph, and the other pages sharing
//!    the form.
//! 3. `probe_after_a_queued_ungroup_sees_it` is the user's "expectGeneration 3 but the
//!    document is at 4" with the real engine thread: a probe sent after a queued 그룹 해제 used
//!    to overtake it (`Lane::Interactive` before `Lane::Edit`); on `Lane::Edit` it no longer can.
//! 4. `inspector_ungroup_on_every_case`: the Inspector's 그룹 해제 (no click point) on every case.
//!
//! The report goes to stdout and `fixtures/out/ungroup-corpus/report.txt`.

mod common;
use common::*;

use lopdf::{dictionary, Dictionary, Document, Object, ObjectId as LoId, Stream};
use seepdf_lib::engine::objects::{self, paragraph, ungroup};
use seepdf_lib::engine::redact::raw;
use seepdf_lib::engine::registry;
use seepdf_lib::engine::render::tiles;
use seepdf_lib::engine::text;
use seepdf_lib::engine::Lane;
use seepdf_lib::ipc::types::{
    DocGeneration, NotEditableReason, ObjectId, PageIndex, PageObjectType, ParagraphEdit,
    ParagraphEditResult, ParagraphFlow, ParagraphProbe, Rect, TextAlign, TextEditStrategy,
};
use seepdf_lib::ipc::EngineError;
use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const W: f32 = 595.0;
const H: f32 = 842.0;
const SCALE: f32 = 1.5;
/// The text appended in the edit, like a Korean user typing.
const MARK: &str = "수정";

fn corpus_dir() -> PathBuf {
    let dir = fixture("out").join("ungroup-corpus");
    std::fs::create_dir_all(&dir).expect("create fixtures/out/ungroup-corpus");
    dir
}

// ---------------------------------------------------------------------------------------
// PDF builder (lopdf)
// ---------------------------------------------------------------------------------------

struct Pdf {
    doc: Document,
    pages_id: LoId,
    kids: Vec<Object>,
    helv: LoId,
    ocgs: Vec<(LoId, bool)>,
}

impl Pdf {
    fn new() -> Self {
        let mut doc = Document::with_version("1.7");
        let pages_id = doc.new_object_id();
        let helv = doc.add_object(dictionary! {
            "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica",
            "Encoding" => "WinAnsiEncoding",
        });
        Pdf {
            doc,
            pages_id,
            kids: Vec::new(),
            helv,
            ocgs: Vec::new(),
        }
    }

    fn fonts(&self) -> Dictionary {
        dictionary! { "F1" => self.helv }
    }

    /// A Form XObject. `extra` may add /Group, /Matrix, /OC …
    fn form(
        &mut self,
        bbox: [f32; 4],
        resources: Dictionary,
        extra: Dictionary,
        body: &str,
    ) -> LoId {
        let mut dict = dictionary! {
            "Type" => "XObject",
            "Subtype" => "Form",
            "BBox" => bbox.iter().map(|&v| Object::Real(v)).collect::<Vec<_>>(),
            "Resources" => resources,
        };
        for (k, v) in extra.into_iter() {
            dict.set(k, v);
        }
        self.doc
            .add_object(Stream::new(dict, body.as_bytes().to_vec()))
    }

    fn page(&mut self, content: &str, resources: Dictionary) {
        let content_id = self
            .doc
            .add_object(Stream::new(dictionary! {}, content.as_bytes().to_vec()));
        let page_id = self.doc.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => self.pages_id,
            "MediaBox" => vec![0.into(), 0.into(), Object::Real(W), Object::Real(H)],
            "Contents" => content_id,
            "Resources" => resources,
        });
        self.kids.push(page_id.into());
    }

    fn ocg(&mut self, name: &str, on: bool) -> LoId {
        let id = self.doc.add_object(dictionary! {
            "Type" => "OCG",
            "Name" => Object::string_literal(name),
        });
        self.ocgs.push((id, on));
        id
    }

    fn finish(mut self) -> Vec<u8> {
        let count = self.kids.len() as i64;
        self.doc.objects.insert(
            self.pages_id,
            Object::Dictionary(dictionary! {
                "Type" => "Pages",
                "Kids" => self.kids.clone(),
                "Count" => count,
            }),
        );
        let mut catalog = dictionary! { "Type" => "Catalog", "Pages" => self.pages_id };
        if !self.ocgs.is_empty() {
            let all: Vec<Object> = self.ocgs.iter().map(|(id, _)| (*id).into()).collect();
            let on: Vec<Object> = self
                .ocgs
                .iter()
                .filter(|(_, o)| *o)
                .map(|(id, _)| (*id).into())
                .collect();
            let off: Vec<Object> = self
                .ocgs
                .iter()
                .filter(|(_, o)| !*o)
                .map(|(id, _)| (*id).into())
                .collect();
            catalog.set(
                "OCProperties",
                dictionary! {
                    "OCGs" => all.clone(),
                    "D" => dictionary! { "Order" => all, "ON" => on, "OFF" => off },
                },
            );
        }
        let catalog_id = self.doc.add_object(catalog);
        self.doc.trailer.set("Root", catalog_id);
        let mut out = Vec::new();
        self.doc.save_to(&mut out).expect("lopdf save");
        out
    }
}

fn group_dict(isolated: bool, knockout: bool) -> Dictionary {
    let mut g = dictionary! { "Type" => "Group", "S" => "Transparency", "CS" => "DeviceRGB" };
    if isolated {
        g.set("I", true);
    }
    if knockout {
        g.set("K", true);
    }
    g
}

fn extra_group(isolated: bool, knockout: bool) -> Dictionary {
    dictionary! { "Group" => group_dict(isolated, knockout) }
}

/// Wraps each page's whole content in one Form XObject (the "everything in one XObject" shape
/// Word / Hancom / print drivers produce). `group`: add /Group /S /Transparency to the form.
fn wrap_pages_in_form(bytes: &[u8], group: bool) -> Vec<u8> {
    let mut doc = Document::load_mem(bytes).expect("lopdf load");
    let pages: Vec<LoId> = doc.get_pages().values().copied().collect();
    for page_id in pages {
        let content = doc.get_page_content(page_id);
        let page = doc.get_dictionary(page_id).expect("page dict").clone();
        let media =
            page.get(b"MediaBox").ok().cloned().unwrap_or_else(|| {
                vec![0.into(), 0.into(), Object::Real(W), Object::Real(H)].into()
            });
        // resources: the page's own (inline or referenced), else inherited from /Pages
        let resources: Object = match page.get(b"Resources") {
            Ok(r) => r.clone(),
            Err(_) => {
                let (_, ids) = doc.get_page_resources(page_id).expect("resources");
                ids.first()
                    .map(|id| Object::Reference(*id))
                    .unwrap_or_else(|| Dictionary::new().into())
            }
        };
        let mut dict = dictionary! {
            "Type" => "XObject",
            "Subtype" => "Form",
            "BBox" => media,
            "Resources" => resources,
        };
        if group {
            dict.set("Group", group_dict(false, false));
        }
        let form_id = doc.add_object(Stream::new(dict, content));
        let new_content = doc.add_object(Stream::new(dictionary! {}, b"q /SeeWrap0 Do Q".to_vec()));
        let page_mut = doc
            .get_object_mut(page_id)
            .and_then(Object::as_dict_mut)
            .expect("page");
        page_mut.set("Contents", new_content);
        page_mut.set(
            "Resources",
            dictionary! { "XObject" => dictionary! { "SeeWrap0" => form_id } },
        );
    }
    let mut out = Vec::new();
    doc.save_to(&mut out).expect("lopdf save");
    out
}

// ---------------------------------------------------------------------------------------
// Corpus
// ---------------------------------------------------------------------------------------

struct Case {
    name: String,
    what: String,
    bytes: Vec<u8>,
    page: PageIndex,
    /// Where the user clicks: on the grouped text.
    at: [f32; 2],
    /// Pages that draw the same form (checked unchanged after save).
    shared: Vec<PageIndex>,
    source: String,
    expect: Expect,
}

/// How a case must end.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Expect {
    /// One 그룹 해제, then the paragraph is edited, saved and reopened intact.
    Edit,
    /// The same, with only the group's text taken out (`UngroupResult.partial`).
    EditPartial,
    /// Refused with this reason before anything is ungrouped.
    Refused(NotEditableReason),
    /// 그룹 해제 refused (`lookChanged`), the document untouched.
    LookChanged,
}

/// Two text lines of one paragraph at form-space (10, 150) / (10, 135.6), plus a filled rect.
const PARA: &str = "0.85 0.9 1 rg 0 0 480 200 re f \
    BT /F1 12 Tf 14.4 TL 0 0 0 rg 10 150 Td (Grouped paragraph line one of the form) Tj T* \
    (and its second line in the same paragraph) Tj ET \
    BT /F1 12 Tf 10 60 Td (Another paragraph lower in the group) Tj ET";
/// Click on the first line of `PARA` drawn at page (60, 600).
const PARA_AT: [f32; 2] = [60.0 + 10.0 + 40.0, 600.0 + 150.0 + 4.0];
const HEAD: &str = "BT /F1 16 Tf 60 790 Td (Page level heading outside any group) Tj ET ";

fn case(
    name: &str,
    what: &str,
    bytes: Vec<u8>,
    page: PageIndex,
    at: [f32; 2],
    shared: Vec<PageIndex>,
) -> Case {
    Case {
        name: name.into(),
        what: what.into(),
        bytes,
        page,
        at,
        shared,
        source: "corpus".into(),
        expect: Expect::Edit,
    }
}

fn one_form(extra: Dictionary, page_prefix: &str, page_gs: Dictionary, body: &str) -> Vec<u8> {
    let mut p = Pdf::new();
    let fonts = p.fonts();
    let fm = p.form(
        [0.0, 0.0, 480.0, 200.0],
        dictionary! { "Font" => fonts.clone() },
        extra,
        body,
    );
    let mut res = dictionary! { "Font" => fonts, "XObject" => dictionary! { "Fm0" => fm } };
    if !page_gs.is_empty() {
        res.set("ExtGState", page_gs);
    }
    p.page(
        &format!("{HEAD} q {page_prefix} 1 0 0 1 60 600 cm /Fm0 Do Q"),
        res,
    );
    p.finish()
}

fn smask_form(p: &mut Pdf) -> LoId {
    // luminosity mask: white left half, grey right half
    p.form(
        [0.0, 0.0, 595.0, 842.0],
        dictionary! {},
        extra_group(false, false),
        "1 g 0 0 300 842 re f 0.5 g 300 0 295 842 re f",
    )
}

/// A whole Word-like page: heading, red text, strikethrough, a 3×4 table, 30 body lines.
fn office_page_body() -> String {
    let mut s = String::new();
    s.push_str("BT /F1 18 Tf 0 0 0 rg 60 780 Td (SecureUploader spec - verification) Tj ET ");
    s.push_str("BT /F1 11 Tf 0.8 0 0 rg 60 750 Td (Changed requirement shown in red text) Tj ET ");
    // strikethrough line over a black run
    s.push_str("BT /F1 11 Tf 0 0 0 rg 60 730 Td (Removed requirement with strikethrough) Tj ET ");
    s.push_str("0 0 0 RG 0.8 w 60 733.5 m 262 733.5 l S ");
    // table
    s.push_str("0.5 w 0 0 0 RG ");
    for r in 0..5 {
        let y = 700.0 - r as f32 * 20.0;
        let _ = write!(s, "60 {y} m 535 {y} l S ");
    }
    for c in 0..4 {
        let x = 60.0 + c as f32 * 158.33;
        let _ = write!(s, "{x} 620 m {x} 700 l S ");
    }
    for r in 0..4 {
        for c in 0..3 {
            let x = 64.0 + c as f32 * 158.33;
            let y = 686.0 - r as f32 * 20.0;
            let _ = write!(
                s,
                "BT /F1 10 Tf 0 0 0 rg {x} {y} Td (Cell {r}-{c} value) Tj ET "
            );
        }
    }
    // body paragraphs: 3 × 10 lines
    let mut y = 590.0;
    for para in 0..3 {
        s.push_str("BT /F1 11 Tf 13.2 TL 0 0 0 rg ");
        let _ = write!(s, "60 {y} Td ");
        for line in 0..10 {
            let _ = write!(
                s,
                "(Body paragraph {para} line {line} lorem ipsum dolor sit amet consectetur) Tj T* "
            );
        }
        s.push_str("ET ");
        y -= 13.2 * 10.0 + 18.0;
    }
    s
}

/// A Hangul page written by the engine itself (the bundled CID font, Identity-H + ToUnicode,
/// embedded — what Hancom / Word produce for Korean), saved, returned as bytes.
fn hangul_engine_page() -> Vec<u8> {
    let mut p = Pdf::new();
    p.page(" ", dictionary! {});
    let blank = p.finish();
    let info = with_state(move |st| registry::open(st, None, blank, None)).expect("open blank");
    let doc_id = info.doc_id.clone();
    let _guard = TestDoc {
        info,
        doc_id: doc_id.clone(),
    };
    let d = doc_id.clone();
    with_state(move |st| {
        objects::add_text(
            st,
            &d,
            0,
            Rect::new(60.0, 640.0, 520.0, 760.0),
            "그룹 안의 한글 문단입니다 첫째 줄 내용이 이어집니다\n둘째 줄도 같은 문단에 속합니다",
            14.0,
            [0, 0, 0],
            TextAlign::Left,
        )
    })
    .expect("add hangul text");
    let d = doc_id.clone();
    with_state(move |st| {
        objects::add_text(
            st,
            &d,
            0,
            Rect::new(60.0, 500.0, 520.0, 560.0),
            "아래쪽 다른 문단 빨간색 아님",
            12.0,
            [200, 0, 0],
            TextAlign::Left,
        )
    })
    .expect("add hangul text 2");
    save_bytes(&doc_id)
}

fn generate_corpus() -> Vec<Case> {
    let mut cases = Vec::new();

    // (1) plain form
    cases.push(case(
        "c01-plain",
        "plain form, Helvetica, no /Group",
        one_form(dictionary! {}, "", dictionary! {}, PARA),
        0,
        PARA_AT,
        vec![],
    ));

    // (2) Office / Hancom transparency group with opaque content
    cases.push(case(
        "c02a-office-group-opaque",
        "form /Group <</S /Transparency /CS /DeviceRGB>>, opaque content (Office/Hancom export)",
        one_form(extra_group(false, false), "", dictionary! {}, PARA),
        0,
        PARA_AT,
        vec![],
    ));
    cases.push(case(
        "c02b-office-group-isolated-opaque",
        "form /Group /I true (PowerPoint), opaque content",
        one_form(extra_group(true, false), "", dictionary! {}, PARA),
        0,
        PARA_AT,
        vec![],
    ));
    {
        // page-level /Group too (Word puts one on every page) — the form itself has none
        let mut doc =
            Document::load_mem(&one_form(dictionary! {}, "", dictionary! {}, PARA)).unwrap();
        let pid = *doc.get_pages().values().next().unwrap();
        doc.get_object_mut(pid)
            .and_then(Object::as_dict_mut)
            .unwrap()
            .set("Group", group_dict(false, false));
        let mut out = Vec::new();
        doc.save_to(&mut out).unwrap();
        cases.push(case(
            "c02c-page-group-plain-form",
            "page /Group transparency, plain form",
            out,
            0,
            PARA_AT,
            vec![],
        ));
    }

    // (3) real transparency
    cases.push(case(
        "c03a-alpha-ca-0.5",
        "form drawn under /ca 0.5 /CA 0.5",
        one_form(
            dictionary! {},
            "/GS0 gs",
            dictionary! { "GS0" => dictionary! { "ca" => 0.5, "CA" => 0.5 } },
            PARA,
        ),
        0,
        PARA_AT,
        vec![],
    ));
    cases.push(case(
        "c03b-blend-multiply",
        "form drawn under /BM /Multiply",
        one_form(
            dictionary! {},
            "/GS0 gs",
            dictionary! { "GS0" => dictionary! { "BM" => "Multiply" } },
            PARA,
        ),
        0,
        PARA_AT,
        vec![],
    ));
    {
        let mut p = Pdf::new();
        let fonts = p.fonts();
        let mask = smask_form(&mut p);
        let fm = p.form(
            [0.0, 0.0, 480.0, 200.0],
            dictionary! { "Font" => fonts.clone() },
            dictionary! {},
            PARA,
        );
        p.page(
            &format!("{HEAD} q /GS0 gs 1 0 0 1 60 600 cm /Fm0 Do Q"),
            dictionary! {
                "Font" => fonts,
                "XObject" => dictionary! { "Fm0" => fm },
                "ExtGState" => dictionary! { "GS0" => dictionary! {
                    "SMask" => dictionary! { "Type" => "Mask", "S" => "Luminosity", "G" => mask },
                } },
            },
        );
        // PDFium applies the soft mask to the form as a whole: only the text comes out
        let mut c = case(
            "c03c-smask",
            "form drawn under a luminosity /SMask",
            p.finish(),
            0,
            PARA_AT,
            vec![],
        );
        c.expect = Expect::EditPartial;
        cases.push(c);
    }
    cases.push(case(
        "c03d-isolated-knockout",
        "form /Group /I true /K true",
        one_form(extra_group(true, true), "", dictionary! {}, PARA),
        0,
        PARA_AT,
        vec![],
    ));
    {
        // transparency on a child inside the form (no group, no transparency on the form object)
        let mut p = Pdf::new();
        let fonts = p.fonts();
        let body = format!("/GSa gs {PARA}");
        let fm = p.form(
            [0.0, 0.0, 480.0, 200.0],
            dictionary! { "Font" => fonts.clone(), "ExtGState" => dictionary! { "GSa" => dictionary! { "ca" => 0.4 } } },
            dictionary! {},
            &body,
        );
        p.page(
            &format!("{HEAD} q 1 0 0 1 60 600 cm /Fm0 Do Q"),
            dictionary! { "Font" => fonts, "XObject" => dictionary! { "Fm0" => fm } },
        );
        cases.push(case(
            "c03e-child-alpha-inside-form",
            "children inside the form painted at /ca 0.4 (form object itself opaque)",
            p.finish(),
            0,
            PARA_AT,
            vec![],
        ));
    }

    {
        // real transparency that the group's compositing matters for: two overlapping
        // rectangles under /ca 0.5 (ungrouped, the overlap would be darker); the text is
        // beside them → only the text comes out
        let body = "0.2 0.4 0.8 rg 300 20 120 120 re f 0.8 0.3 0.2 rg 360 60 110 110 re f \
            BT /F1 12 Tf 14.4 TL 0 0 0 rg 10 150 Td (Grouped paragraph line one of the form) Tj T* \
            (and its second line in the same paragraph) Tj ET";
        let mut c = case(
            "c03f-alpha-overlap-text-apart",
            "transparency group under /ca 0.5 with two overlapping rectangles, the text beside them",
            one_form(
                extra_group(false, false),
                "/GS0 gs",
                dictionary! { "GS0" => dictionary! { "ca" => 0.5, "CA" => 0.5 } },
                body,
            ),
            0,
            PARA_AT,
            vec![],
        );
        c.expect = Expect::EditPartial;
        cases.push(c);
        // …and with the text over a dark rectangle: neither way keeps the look
        let body = "0.2 0.4 0.8 rg 0 100 300 80 re f 0.8 0.3 0.2 rg 150 80 200 80 re f \
            BT /F1 12 Tf 14.4 TL 0 0 0 rg 10 150 Td (Grouped paragraph line one of the form) Tj T* \
            (and its second line in the same paragraph) Tj ET";
        let mut c = case(
            "c03g-alpha-text-over-graphics",
            "transparency group under /ca 0.5, the text over overlapping dark rectangles",
            one_form(
                extra_group(false, false),
                "/GS0 gs",
                dictionary! { "GS0" => dictionary! { "ca" => 0.5, "CA" => 0.5 } },
                body,
            ),
            0,
            PARA_AT,
            vec![],
        );
        c.expect = Expect::LookChanged;
        cases.push(c);
    }

    // (4) nested forms
    {
        let mut p = Pdf::new();
        let fonts = p.fonts();
        let inner = p.form(
            [0.0, 0.0, 480.0, 200.0],
            dictionary! { "Font" => fonts.clone() },
            dictionary! {},
            PARA,
        );
        let outer = p.form(
            [0.0, 0.0, 595.0, 842.0],
            dictionary! { "XObject" => dictionary! { "Fm1" => inner } },
            dictionary! {},
            "0.9 g 40 580 520 240 re f q 1 0 0 1 60 600 cm /Fm1 Do Q",
        );
        p.page(
            &format!("{HEAD} q /Fm0 Do Q"),
            dictionary! { "Font" => fonts, "XObject" => dictionary! { "Fm0" => outer } },
        );
        cases.push(case(
            "c04a-nested-2",
            "outer form (no text of its own) → inner form with the text",
            p.finish(),
            0,
            PARA_AT,
            vec![],
        ));
    }
    {
        let mut p = Pdf::new();
        let fonts = p.fonts();
        let f2 = p.form(
            [0.0, 0.0, 480.0, 200.0],
            dictionary! { "Font" => fonts.clone() },
            dictionary! {},
            PARA,
        );
        let f1 = p.form(
            [0.0, 0.0, 595.0, 842.0],
            dictionary! { "XObject" => dictionary! { "Fm2" => f2 } },
            dictionary! {},
            "q 1 0 0 1 60 600 cm /Fm2 Do Q",
        );
        let f0 = p.form(
            [0.0, 0.0, 595.0, 842.0],
            dictionary! { "XObject" => dictionary! { "Fm1" => f1 } },
            extra_group(false, false),
            "q /Fm1 Do Q",
        );
        p.page(
            &format!("{HEAD} q /Fm0 Do Q"),
            dictionary! { "Font" => fonts, "XObject" => dictionary! { "Fm0" => f0 } },
        );
        cases.push(case(
            "c04b-nested-3-outer-group",
            "3 levels (outer has Office /Group), text only in the innermost",
            p.finish(),
            0,
            PARA_AT,
            vec![],
        ));
    }
    {
        let mut p = Pdf::new();
        let fonts = p.fonts();
        let inner = p.form(
            [0.0, 0.0, 480.0, 200.0],
            dictionary! { "Font" => fonts.clone() },
            dictionary! {},
            PARA,
        );
        let outer = p.form(
            [0.0, 0.0, 595.0, 842.0],
            dictionary! { "Font" => fonts.clone(), "XObject" => dictionary! { "Fm1" => inner } },
            dictionary! {},
            "BT /F1 12 Tf 60 400 Td (Outer form own text line) Tj ET q 1 0 0 1 60 600 cm /Fm1 Do Q",
        );
        p.page(
            &format!("{HEAD} q /Fm0 Do Q"),
            dictionary! { "Font" => fonts, "XObject" => dictionary! { "Fm0" => outer } },
        );
        cases.push(case(
            "c04c-nested-text-both-levels",
            "outer form has its own text; click on the inner form's text",
            p.finish(),
            0,
            PARA_AT,
            vec![],
        ));
    }

    // (5) the same form on several pages and twice on one page
    {
        let mut p = Pdf::new();
        let fonts = p.fonts();
        let fm = p.form(
            [0.0, 0.0, 480.0, 200.0],
            dictionary! { "Font" => fonts.clone() },
            dictionary! {},
            PARA,
        );
        let res = || dictionary! { "Font" => p_fonts_clone(&fonts), "XObject" => dictionary! { "Fm0" => fm } };
        p.page(
            &format!("{HEAD} q 1 0 0 1 60 600 cm /Fm0 Do Q q 1 0 0 1 60 250 cm /Fm0 Do Q"),
            res(),
        );
        p.page(&format!("{HEAD} q 1 0 0 1 60 600 cm /Fm0 Do Q"), res());
        p.page(&format!("{HEAD} q 1 0 0 1 60 300 cm /Fm0 Do Q"), res());
        cases.push(case(
            "c05-shared-form",
            "one form drawn twice on page 1 and once on pages 2 and 3",
            p.finish(),
            0,
            PARA_AT,
            vec![1, 2],
        ));
    }

    // (6) /Matrix, rotation, /BBox clip, page clip
    {
        let body = "BT /F1 12 Tf 0 0 0 rg 10 150 Td (This grouped line is longer than the BBox so its tail is clipped away) Tj ET";
        let bytes = one_form(
            dictionary! { "Matrix" => vec![0.75.into(), 0.into(), 0.into(), 0.75.into(), 0.into(), 0.into()], "BBox" => vec![0.into(), 0.into(), 250.into(), 200.into()] },
            "",
            dictionary! {},
            body,
        );
        cases.push(case(
            "c06a-matrix-scale-bbox-clip",
            "/Matrix 0.75 scale, /BBox clips the tail of the line",
            bytes,
            0,
            [60.0 + 0.75 * 40.0, 600.0 + 0.75 * 154.0],
            vec![],
        ));
    }
    {
        let bytes = {
            let mut p = Pdf::new();
            let fonts = p.fonts();
            let fm = p.form(
                [0.0, 0.0, 480.0, 200.0],
                dictionary! { "Font" => fonts.clone() },
                dictionary! {},
                PARA,
            );
            p.page(
                &format!("{HEAD} q 0 1 -1 0 400 200 cm /Fm0 Do Q"),
                dictionary! { "Font" => fonts, "XObject" => dictionary! { "Fm0" => fm } },
            );
            p.finish()
        };
        // form point (50, 154) → page (400 - 154, 200 + 50)
        cases.push(case(
            "c06b-rotated-90",
            "form drawn rotated 90° (cm 0 1 -1 0)",
            bytes,
            0,
            [400.0 - 154.0, 200.0 + 50.0],
            vec![],
        ));
    }
    {
        let (c, s) = (30f32.to_radians().cos(), 30f32.to_radians().sin());
        let bytes = one_form(
            dictionary! { "Matrix" => vec![Object::Real(c), Object::Real(s), Object::Real(-s), Object::Real(c), 0.into(), 0.into()] },
            "",
            dictionary! {},
            PARA,
        );
        let (fx, fy) = (50.0f32, 154.0f32);
        cases.push(case(
            "c06c-matrix-rotate-30",
            "form /Matrix rotated 30°",
            bytes,
            0,
            [60.0 + c * fx - s * fy, 600.0 + s * fx + c * fy],
            vec![],
        ));
    }
    cases.push(case(
        "c06d-page-clip-around-do",
        "page clips (re W n) the form to its top 30 pt before /Fm0 Do",
        one_form(dictionary! {}, "60 725 480 75 re W n", dictionary! {}, PARA),
        0,
        PARA_AT,
        vec![],
    ));

    // (7) Hangul CID fonts
    {
        let plain = hangul_engine_page();
        std::fs::write(corpus_dir().join("_hangul-source.pdf"), &plain).ok();
        cases.push(case(
            "c07a-hangul-embedded-cid-whole-page-form",
            "engine-written Hangul (embedded CID, Identity-H) wrapped in one form",
            wrap_pages_in_form(&plain, false),
            0,
            [120.0, 750.0],
            vec![],
        ));
        cases.push(case(
            "c07b-hangul-embedded-cid-office-group",
            "same, form with Office /Group transparency",
            wrap_pages_in_form(&plain, true),
            0,
            [120.0, 750.0],
            vec![],
        ));
    }
    {
        let mut p = Pdf::new();
        let desc = p.doc.add_object(dictionary! {
            "Type" => "Font", "Subtype" => "CIDFontType0", "BaseFont" => "HYGoThic-Medium",
            "CIDSystemInfo" => dictionary! { "Registry" => Object::string_literal("Adobe"), "Ordering" => Object::string_literal("Korea1"), "Supplement" => 2 },
            "FontDescriptor" => dictionary! { "Type" => "FontDescriptor", "FontName" => "HYGoThic-Medium", "Flags" => 6, "FontBBox" => vec![0.into(), (-200).into(), 1000.into(), 900.into()], "ItalicAngle" => 0, "Ascent" => 880, "Descent" => -120, "CapHeight" => 700, "StemV" => 80 },
            "DW" => 1000,
        });
        let f = p.doc.add_object(dictionary! {
            "Type" => "Font", "Subtype" => "Type0", "BaseFont" => "HYGoThic-Medium",
            "Encoding" => "UniKS-UCS2-H", "DescendantFonts" => vec![desc.into()],
        });
        // UCS2 big-endian hex of the Hangul text
        let hex = |s: &str| {
            s.encode_utf16()
                .map(|u| format!("{u:04X}"))
                .collect::<String>()
        };
        let body = format!(
            "BT /K1 14 Tf 16.8 TL 0 0 0 rg 10 150 Td <{}> Tj T* <{}> Tj ET",
            hex("비임베디드 한글 글꼴로 쓴 그룹 문단"),
            hex("두 번째 줄입니다")
        );
        let fm = p.form(
            [0.0, 0.0, 480.0, 200.0],
            dictionary! { "Font" => dictionary! { "K1" => f } },
            extra_group(false, false),
            &body,
        );
        let fonts = p.fonts();
        p.page(
            &format!("{HEAD} q 1 0 0 1 60 600 cm /Fm0 Do Q"),
            dictionary! { "Font" => fonts, "XObject" => dictionary! { "Fm0" => fm } },
        );
        cases.push(case(
            "c07c-hangul-nonembedded-uniks",
            "non-embedded Type0 HYGoThic-Medium /UniKS-UCS2-H inside an Office-group form",
            p.finish(),
            0,
            [60.0 + 10.0 + 40.0, 600.0 + 150.0 + 5.0],
            vec![],
        ));
    }

    // (8) tagged / optional content
    {
        let body = format!("/P <</MCID 0>> BDC {PARA} EMC");
        cases.push(case(
            "c08a-tagged-mcid",
            "form content inside /P <</MCID 0>> BDC … EMC",
            one_form(dictionary! {}, "", dictionary! {}, &body),
            0,
            PARA_AT,
            vec![],
        ));
    }
    for (name, on) in [
        ("c08b-ocg-on-inside-form", true),
        ("c08c-ocg-off-line-inside-form", false),
    ] {
        let mut p = Pdf::new();
        let oc = p.ocg("Layer", on);
        let fonts = p.fonts();
        let body = format!("{PARA} /OC /oc1 BDC BT /F1 12 Tf 1 0 0 rg 10 100 Td (HIDDEN-OR-SHOWN optional content line) Tj ET EMC");
        let fm = p.form(
            [0.0, 0.0, 480.0, 200.0],
            dictionary! { "Font" => fonts.clone(), "Properties" => dictionary! { "oc1" => oc } },
            dictionary! {},
            &body,
        );
        p.page(
            &format!("{HEAD} q 1 0 0 1 60 600 cm /Fm0 Do Q"),
            dictionary! { "Font" => fonts, "XObject" => dictionary! { "Fm0" => fm } },
        );
        cases.push(case(
            name,
            if on {
                "a line in /OC /oc1 BDC (layer ON) inside the form"
            } else {
                "a line in /OC /oc1 BDC (layer OFF, hidden) inside the form"
            },
            p.finish(),
            0,
            PARA_AT,
            vec![],
        ));
    }
    {
        // the form XObject itself carries /OC of an OFF layer (a hidden watermark); visible text elsewhere
        let mut p = Pdf::new();
        let oc = p.ocg("Watermark", false);
        let fonts = p.fonts();
        let fm = p.form(
            [0.0, 0.0, 480.0, 200.0],
            dictionary! { "Font" => fonts.clone() },
            dictionary! { "OC" => oc },
            "BT /F1 30 Tf 1 0 0 rg 10 100 Td (HIDDEN WATERMARK) Tj ET",
        );
        p.page(
            &format!("{HEAD} q 1 0 0 1 60 400 cm /Fm0 Do Q"),
            dictionary! { "Font" => fonts, "XObject" => dictionary! { "Fm0" => fm } },
        );
        let mut c = case("c08d-hidden-form-oc-off", "form XObject with /OC of an OFF layer (invisible); the user clicks where nothing is visible", p.finish(), 0, [60.0 + 10.0 + 60.0, 400.0 + 100.0 + 10.0], vec![]);
        c.expect = Expect::Refused(NotEditableReason::Invisible);
        cases.push(c);
    }

    // (9) whole page in one form
    {
        let mut p = Pdf::new();
        let fonts = p.fonts();
        p.page(&office_page_body(), dictionary! { "Font" => fonts });
        let plain = p.finish();
        cases.push(case(
            "c09a-whole-page-form",
            "Word-like page (table, red text, strikethrough, 30 body lines) all in one form",
            wrap_pages_in_form(&plain, false),
            0,
            [120.0, 590.0 + 4.0],
            vec![],
        ));
        cases.push(case(
            "c09b-whole-page-form-office-group",
            "same, with Office /Group transparency on the form",
            wrap_pages_in_form(&plain, true),
            0,
            [120.0, 590.0 + 4.0],
            vec![],
        ));
    }
    {
        let tm = std::fs::read(fixture("tracemonkey.pdf")).expect("tracemonkey");
        // only page 1 matters; wrapping all pages is fine
        cases.push(case(
            "c09c-tracemonkey-p1-wrapped",
            "tracemonkey.pdf (real Type1 fonts, 2 columns) with every page wrapped in one form",
            wrap_pages_in_form(&tm, false),
            0,
            [100.0, 600.0],
            vec![1],
        ));
    }

    // (10) inline image / shading inside the form
    {
        let body = format!("{PARA} q 60 0 0 40 400 20 cm BI /W 2 /H 2 /CS /RGB /BPC 8 /F /AHx ID 0000FFFF000000FF00FFFF00> EI Q");
        cases.push(case(
            "c10a-inline-image",
            "form with text + an inline image (BI … ID … EI)",
            one_form(dictionary! {}, "", dictionary! {}, &body),
            0,
            PARA_AT,
            vec![],
        ));
    }
    {
        let mut p = Pdf::new();
        let fonts = p.fonts();
        let sh = p.doc.add_object(dictionary! {
            "ShadingType" => 2, "ColorSpace" => "DeviceRGB",
            "Coords" => vec![0.into(), 0.into(), 480.into(), 0.into()],
            "Function" => dictionary! { "FunctionType" => 2, "Domain" => vec![0.into(), 1.into()], "C0" => vec![1.into(), 1.into(), 0.into()], "C1" => vec![0.into(), 0.5.into(), 1.into()], "N" => 1 },
            "Extend" => vec![true.into(), true.into()],
        });
        let body = format!("q 0 0 480 40 re W n /Sh0 sh Q {PARA}");
        let fm = p.form(
            [0.0, 0.0, 480.0, 200.0],
            dictionary! { "Font" => fonts.clone(), "Shading" => dictionary! { "Sh0" => sh } },
            dictionary! {},
            &body,
        );
        p.page(
            &format!("{HEAD} q 1 0 0 1 60 600 cm /Fm0 Do Q"),
            dictionary! { "Font" => fonts, "XObject" => dictionary! { "Fm0" => fm } },
        );
        cases.push(case(
            "c10b-shading-sh",
            "form with text + a clipped axial shading (sh)",
            p.finish(),
            0,
            PARA_AT,
            vec![],
        ));
    }
    {
        let body = "BT /F1 12 Tf 14.4 TL 0 0 0 rg 0.6 Tc 2 Tw 90 Tz 10 150 Td (Justified line with Tc Tw and Tz set in the group) Tj T* (second line keeps the same spacing state) Tj ET";
        cases.push(case(
            "c10c-tc-tw-tz",
            "text with Tc / Tw / Tz (justified Korean producers) inside the form",
            one_form(dictionary! {}, "", dictionary! {}, body),
            0,
            PARA_AT,
            vec![],
        ));
    }

    // (11) overlapping sibling forms: a full-page header/border form first, then the body form
    {
        let mut p = Pdf::new();
        let fonts = p.fonts();
        let bg = p.form(
            [0.0, 0.0, 595.0, 842.0],
            dictionary! { "Font" => fonts.clone() },
            dictionary! {},
            "0 0 0 RG 1 w 30 30 535 782 re S BT /F1 9 Tf 40 820 Td (Header: CONFIDENTIAL - page border form) Tj ET",
        );
        let body = p.form(
            [0.0, 0.0, 480.0, 200.0],
            dictionary! { "Font" => fonts.clone() },
            dictionary! {},
            PARA,
        );
        p.page(
            &format!("{HEAD} q /Bg Do Q q 1 0 0 1 60 600 cm /Fm0 Do Q"),
            dictionary! { "Font" => fonts, "XObject" => dictionary! { "Bg" => bg, "Fm0" => body } },
        );
        cases.push(case(
            "c11-overlapping-forms-border-first",
            "a full-page border/header form (with text) is drawn before the body form",
            p.finish(),
            0,
            PARA_AT,
            vec![],
        ));
    }

    // (12) font resource names that collide between the page and a form / between two forms
    {
        let mut p = Pdf::new();
        let courier = p.doc.add_object(dictionary! { "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Courier", "Encoding" => "WinAnsiEncoding" });
        let fm = p.form(
            [0.0, 0.0, 480.0, 200.0],
            dictionary! { "Font" => dictionary! { "F1" => courier } },
            dictionary! {},
            PARA,
        );
        let fonts = p.fonts();
        p.page(
            &format!("{HEAD} q 1 0 0 1 60 600 cm /Fm0 Do Q"),
            dictionary! { "Font" => fonts, "XObject" => dictionary! { "Fm0" => fm } },
        );
        cases.push(case(
            "c12a-font-name-collision-page-vs-form",
            "page /F1 = Helvetica, form /F1 = Courier",
            p.finish(),
            0,
            PARA_AT,
            vec![],
        ));
    }
    {
        let mut p = Pdf::new();
        let times = p.doc.add_object(dictionary! { "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Times-Roman", "Encoding" => "WinAnsiEncoding" });
        let courier = p.doc.add_object(dictionary! { "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Courier", "Encoding" => "WinAnsiEncoding" });
        let a = p.form([0.0, 0.0, 595.0, 842.0], dictionary! { "Font" => dictionary! { "F1" => times } }, dictionary! {}, "0 0 0 RG 30 30 535 782 re S BT /F1 14 Tf 60 450 Td (Times form, drawn first, full-page bounds) Tj ET");
        let b = p.form(
            [0.0, 0.0, 480.0, 200.0],
            dictionary! { "Font" => dictionary! { "F1" => courier } },
            dictionary! {},
            PARA,
        );
        let fonts = p.fonts();
        p.page(
            &format!("{HEAD} q /Fa Do Q q 1 0 0 1 60 600 cm /Fb Do Q"),
            dictionary! { "Font" => fonts, "XObject" => dictionary! { "Fa" => a, "Fb" => b } },
        );
        cases.push(case(
            "c12b-font-name-collision-two-forms",
            "two overlapping forms, each /F1 but Times vs Courier (both get ungrouped)",
            p.finish(),
            0,
            PARA_AT,
            vec![],
        ));
    }
    {
        let mut p = Pdf::new();
        let shifted = p.doc.add_object(dictionary! {
            "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica",
            "Encoding" => dictionary! { "Type" => "Encoding", "BaseEncoding" => "WinAnsiEncoding",
                "Differences" => vec![97.into(), "b".into(), "c".into(), "d".into(), "e".into(), "f".into()] },
        });
        let a = p.form([0.0, 0.0, 595.0, 842.0], dictionary! { "Font" => dictionary! { "F1" => shifted } }, dictionary! {}, "0 0 0 RG 30 30 535 782 re S BT /F1 14 Tf 60 450 Td (abcde drawn with a Differences encoding) Tj ET");
        let b = p.form(
            [0.0, 0.0, 480.0, 200.0],
            dictionary! { "Font" => p_fonts_clone(&dictionary! { "F1" => p.helv }) },
            dictionary! {},
            PARA,
        );
        let fonts = p.fonts();
        p.page(
            &format!("{HEAD} q /Fa Do Q q 1 0 0 1 60 600 cm /Fb Do Q"),
            dictionary! { "Font" => fonts, "XObject" => dictionary! { "Fa" => a, "Fb" => b } },
        );
        cases.push(case(
            "c12c-same-basefont-different-encoding",
            "two forms: Helvetica with /Differences vs plain Helvetica (same BaseFont)",
            p.finish(),
            0,
            PARA_AT,
            vec![],
        ));
    }

    for c in &cases {
        std::fs::write(corpus_dir().join(format!("{}.pdf", c.name)), &c.bytes)
            .expect("write corpus pdf");
    }
    cases
}

fn p_fonts_clone(d: &Dictionary) -> Dictionary {
    d.clone()
}

// ---------------------------------------------------------------------------------------
// Repo fixtures: pages with text inside a Form XObject
// ---------------------------------------------------------------------------------------

/// For each page: the first non-space character drawn by a text object inside a form (its box
/// centre), and the index of the top-level form that holds it.
fn grouped_points(doc_id: &str, max_pages: u16) -> Vec<(PageIndex, [f32; 2], u32, String)> {
    let doc_id = doc_id.to_string();
    with_doc(&doc_id, move |d| {
        let bindings = d.bindings();
        let count = d.info().page_count.min(max_pages);
        let mut out = Vec::new();
        for p in 0..count {
            let page = d.page(p)?;
            let n = raw::object_count(bindings, page);
            let mut owner: HashMap<raw::HandleKey, u32> = HashMap::new();
            for i in 0..n {
                let h = raw::object_at(bindings, page, i)?;
                if raw::object_type(bindings, h) != raw::OBJ_FORM {
                    continue;
                }
                let mut stack = vec![h];
                while let Some(f) = stack.pop() {
                    for c in raw::form_children(bindings, f) {
                        owner.insert(raw::key(c), i as u32);
                        if raw::object_type(bindings, c) == raw::OBJ_FORM {
                            stack.push(c);
                        }
                    }
                }
            }
            if owner.is_empty() {
                continue;
            }
            let tp = raw::TextPage::load(bindings, page)?;
            let chars = tp.chars();
            // the first grouped character (often a running header / page number) and the one in
            // the middle of the grouped text (body text)
            let grouped: Vec<usize> = chars
                .iter()
                .enumerate()
                .filter(|(_, c)| {
                    owner.contains_key(&c.object)
                        && char::from_u32(c.unicode).is_some_and(|ch| ch.is_alphanumeric())
                        && c.tight.r > c.tight.l
                })
                .map(|(k, _)| k)
                .collect();
            let mut picks = Vec::new();
            if let Some(&k) = grouped.first() {
                picks.push(k);
            }
            if grouped.len() > 2 {
                picks.push(grouped[grouped.len() / 2]);
            }
            for k in picks {
                let c = &chars[k];
                let snippet: String = chars[k..]
                    .iter()
                    .take(24)
                    .filter_map(|c| char::from_u32(c.unicode))
                    .collect();
                out.push((
                    p,
                    [(c.tight.l + c.tight.r) / 2.0, (c.tight.b + c.tight.t) / 2.0],
                    owner[&c.object],
                    snippet,
                ));
            }
        }
        Ok(out)
    })
    .expect("scan grouped text")
}

fn fixture_cases(summary: &mut String) -> Vec<Case> {
    let mut cases = Vec::new();
    for path in all_fixtures() {
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let bytes = match std::fs::read(&path) {
            Ok(b) => b,
            Err(_) => continue,
        };
        if bytes.len() > 30_000_000 {
            continue;
        }
        let b2 = bytes.clone();
        let Ok(info) = with_state(move |st| registry::open(st, None, b2, None)) else {
            continue;
        };
        let doc = TestDoc {
            doc_id: info.doc_id.clone(),
            info,
        };
        let points = grouped_points(&doc.doc_id, 2000);
        let _ = writeln!(
            summary,
            "fixture scan: {name}: {} pages, {} with text inside a Form XObject {:?}",
            doc.info.page_count,
            points.len(),
            points.iter().map(|p| p.0 + 1).take(20).collect::<Vec<_>>()
        );
        for (k, (page, at, form, snippet)) in points.into_iter().take(6).enumerate() {
            cases.push(Case {
                name: format!(
                    "fx-{}-p{}-{}",
                    name.trim_end_matches(".pdf"),
                    page + 1,
                    if k % 2 == 0 { "first" } else { "mid" }
                ),
                what: format!("repo fixture; grouped text {snippet:?} in top-level form #{form}"),
                bytes: bytes.clone(),
                page,
                at,
                shared: Vec::new(),
                source: format!(
                    "fixtures/{}",
                    path.strip_prefix(fixture("")).unwrap().display()
                ),
                // decided by what the engine finds: see `fixture_cases_edit_or_refuse_first`
                expect: Expect::Edit,
            });
        }
    }
    cases
}

// ---------------------------------------------------------------------------------------
// Engine helpers
// ---------------------------------------------------------------------------------------

fn save_bytes(doc_id: &str) -> Vec<u8> {
    let doc_id = doc_id.to_string();
    with_state(move |st| {
        tiles::generate_appearances(st, &doc_id)?;
        Ok(st.doc(&doc_id)?.to_bytes()?.to_vec())
    })
    .expect("save to bytes")
}

fn open_bytes(bytes: Vec<u8>) -> Result<TestDoc, EngineError> {
    let info = with_state(move |st| registry::open(st, None, bytes, None))?;
    Ok(TestDoc {
        doc_id: info.doc_id.clone(),
        info,
    })
}

fn render(doc_id: &str, page: PageIndex) -> Vec<u8> {
    let doc_id = doc_id.to_string();
    with_state(move |st| tiles::render_raw_buffer(st, &doc_id, page, SCALE, None)).expect("render")
}

fn dims(buf: &[u8]) -> (u32, u32) {
    let w = u32::from_le_bytes(buf[8..12].try_into().unwrap());
    let h = u32::from_le_bytes(buf[12..16].try_into().unwrap());
    (w, h)
}

/// (mean abs channel diff, share of pixels off by > 24) outside `masks` (page points; the
/// page's crop origin is `origin`).
fn diff(a: &[u8], b: &[u8], masks: &[Rect], crop: Rect) -> (f64, f64) {
    if a.len() != b.len() {
        return (255.0, 1.0);
    }
    let (w, _h) = dims(a);
    let masked = |x: u32, y: u32| {
        let px = crop.l + x as f32 / SCALE;
        let py = crop.t - y as f32 / SCALE;
        masks
            .iter()
            .any(|m| px >= m.l && px <= m.r && py >= m.b && py <= m.t)
    };
    let (mut sum, mut off, mut n) = (0u64, 0u64, 0u64);
    for (i, (p, q)) in a[32..]
        .chunks_exact(4)
        .zip(b[32..].chunks_exact(4))
        .enumerate()
    {
        let (x, y) = (i as u32 % w, i as u32 / w);
        if !masks.is_empty() && masked(x, y) {
            continue;
        }
        let d: u32 = (0..3)
            .map(|k| (p[k] as i32 - q[k] as i32).unsigned_abs())
            .sum();
        sum += d as u64;
        if d > 24 {
            off += 1;
        }
        n += 1;
    }
    if n == 0 {
        return (0.0, 0.0);
    }
    (sum as f64 / (3 * n) as f64, off as f64 / n as f64)
}

fn generation(doc_id: &str) -> DocGeneration {
    let doc_id = doc_id.to_string();
    with_state(move |st| Ok(st.doc(&doc_id)?.generation)).expect("generation")
}

fn page_text(doc_id: &str, page: PageIndex) -> String {
    let doc_id = doc_id.to_string();
    with_doc(&doc_id, move |d| {
        Ok(text::layer::page_text(d, page)?.text.clone())
    })
    .unwrap_or_default()
}

fn crop_of(doc: &TestDoc, page: PageIndex) -> Rect {
    doc.info.pages[page as usize].crop
}

fn probe(
    doc_id: &str,
    page: PageIndex,
    at: [f32; 2],
) -> Result<Option<ParagraphProbe>, EngineError> {
    let doc_id = doc_id.to_string();
    with_doc(&doc_id, move |d| paragraph::probe(d, page, at))
}

fn err_str(e: &EngineError) -> String {
    format!(
        "{:?}{} \"{}\"",
        e.code,
        e.detail
            .as_deref()
            .map(|d| format!("/{d}"))
            .unwrap_or_default(),
        e.message
    )
}

fn probe_str(p: &Option<ParagraphProbe>) -> String {
    match p {
        None => "None (no text there → the UI does nothing, silently)".into(),
        Some(p) => format!(
            "{:?}{} ids={:?} gen={} text={:?}",
            p.strategy,
            p.reason.map(|r| format!("/{r:?}")).unwrap_or_default(),
            &p.object_ids[..p.object_ids.len().min(6)],
            p.doc_generation,
            p.text.chars().take(40).collect::<String>()
        ),
    }
}

fn tokens(s: &str) -> Vec<String> {
    s.split_whitespace().map(|t| t.to_string()).collect()
}

fn write_png(buf: &[u8], path: &std::path::Path) {
    let (w, h) = dims(buf);
    if let Some(img) = image::RgbaImage::from_raw(w, h, buf[32..].to_vec()) {
        let _ = img.save(path);
    }
}

/// Right after the (first) ungroup: in-memory render vs the original, then save → reopen →
/// render + text vs the original (what a user who only ungroups and saves gets).
fn check_after_ungroup(
    o: &mut Outcome,
    c: &Case,
    id: &str,
    before: &[u8],
    text_before: &str,
    crop: Rect,
    round: usize,
) {
    let page = c.page;
    let after = render(id, page);
    let (m, s) = diff(before, &after, &[], crop);
    let png = corpus_dir().join("png");
    std::fs::create_dir_all(&png).ok();
    if s > 0.002 {
        o.fail(format!("ungroup #{round} changed the page's look (in memory): mean {m:.3} share {s:.4} (png/{}-{{before,ungrouped{round}}}.png)", c.name));
        write_png(before, &png.join(format!("{}-before.png", c.name)));
        write_png(
            &after,
            &png.join(format!("{}-ungrouped{round}.png", c.name)),
        );
    } else {
        o.log(format!(
            "  render after ungroup vs original (whole page, in memory): mean {m:.3} share {s:.4}"
        ));
    }
    let saved = save_bytes(id);
    let re = match open_bytes(saved) {
        Ok(d) => d,
        Err(e) => {
            o.fail(format!("reopen after ungroup + save: {}", err_str(&e)));
            return;
        }
    };
    let after_saved = render(&re.doc_id, page);
    let (m, s) = diff(before, &after_saved, &[], crop);
    if s > 0.002 {
        o.fail(format!(
            "ungroup #{round} + save + reopen changed the page's look: mean {m:.3} share {s:.4}"
        ));
        write_png(
            &after_saved,
            &png.join(format!("{}-ungrouped{round}-saved.png", c.name)),
        );
    } else {
        o.log(format!(
            "  ungroup + save + reopen: mean {m:.3} share {s:.4}"
        ));
    }
    let squash = |s: &str| s.chars().filter(|c| !c.is_whitespace()).collect::<String>();
    let t_after = page_text(&re.doc_id, page);
    if squash(&t_after) != squash(text_before) {
        let a = squash(text_before);
        let b = squash(&t_after);
        let common = a.chars().zip(b.chars()).take_while(|(x, y)| x == y).count();
        o.fail(format!(
            "ungroup #{round} + save changed the extracted text ({} → {} chars, first difference at char {common}: {:?} vs {:?})",
            a.chars().count(),
            b.chars().count(),
            a.chars().skip(common).take(20).collect::<String>(),
            b.chars().skip(common).take(20).collect::<String>()
        ));
    } else {
        o.log("  ungroup + save: extracted page text identical");
    }
}

/// Top-level forms whose bounds hold `at`: (index, direct text children, nested text children).
fn forms_at(doc_id: &str, page: PageIndex, at: [f32; 2]) -> Vec<(usize, usize, usize)> {
    let doc_id = doc_id.to_string();
    with_doc(&doc_id, move |d| {
        let bindings = d.bindings();
        let pg = d.page(page)?;
        let mut out = Vec::new();
        for i in 0..raw::object_count(bindings, pg) {
            let h = raw::object_at(bindings, pg, i)?;
            if raw::object_type(bindings, h) != raw::OBJ_FORM {
                continue;
            }
            let Some(b) = raw::bounds(bindings, h) else {
                continue;
            };
            if !(at[0] >= b.l && at[0] <= b.r && at[1] >= b.b && at[1] <= b.t) {
                continue;
            }
            let direct = raw::form_children(bindings, h)
                .into_iter()
                .filter(|&c| raw::object_type(bindings, c) == 1)
                .count();
            let mut nested = 0;
            let mut stack: Vec<_> = raw::form_children(bindings, h)
                .into_iter()
                .filter(|&c| raw::object_type(bindings, c) == raw::OBJ_FORM)
                .collect();
            while let Some(f) = stack.pop() {
                for c in raw::form_children(bindings, f) {
                    match raw::object_type(bindings, c) {
                        1 => nested += 1,
                        t if t == raw::OBJ_FORM => stack.push(c),
                        _ => {}
                    }
                }
            }
            out.push((i, direct, nested));
        }
        Ok(out)
    })
    .unwrap_or_default()
}

// ---------------------------------------------------------------------------------------
// The UI's call sequence
// ---------------------------------------------------------------------------------------

#[derive(Default)]
struct Outcome {
    lines: Vec<String>,
    failures: Vec<String>,
    ungroups: usize,
    /// Refused before any 그룹 해제, with this reason.
    refused: Option<NotEditableReason>,
    /// 그룹 해제 refused because the page would look different.
    look_changed: bool,
    /// Only the text came out.
    partial: bool,
    edited: bool,
}

impl Outcome {
    fn log(&mut self, s: impl Into<String>) {
        self.lines.push(s.into());
    }
    fn fail(&mut self, s: impl Into<String>) {
        let s = s.into();
        self.lines.push(format!("FAIL: {s}"));
        self.failures.push(s);
    }
}

fn run_case(c: &Case) -> Outcome {
    let mut o = Outcome::default();
    let doc = match open_bytes(c.bytes.clone()) {
        Ok(d) => d,
        Err(e) => {
            o.fail(format!("open: {}", err_str(&e)));
            return o;
        }
    };
    let id = doc.doc_id.clone();
    let page = c.page;
    let crop = crop_of(&doc, page);
    let before = render(&id, page);
    let before_shared: Vec<(PageIndex, Vec<u8>)> =
        c.shared.iter().map(|&p| (p, render(&id, p))).collect();
    let text_before = page_text(&id, page);

    // list_page_objects (what EditLayer holds: badge / hit test / double-click)
    let listed = with_doc(&id, move |doc| objects::list(doc, page));
    let listed = match listed {
        Ok(l) => l,
        Err(e) => {
            o.fail(format!("list_page_objects: {}", err_str(&e)));
            return o;
        }
    };
    let forms: Vec<ObjectId> = listed
        .objects
        .iter()
        .filter(|x| x.object_type == PageObjectType::Form)
        .map(|x| x.object_id)
        .collect();
    let hit = listed.objects.iter().rev().find(|x| {
        let r = &x.rect;
        c.at[0] >= r.l.min(r.r)
            && c.at[0] <= r.l.max(r.r)
            && c.at[1] >= r.b.min(r.t)
            && c.at[1] <= r.b.max(r.t)
    });
    o.log(format!(
        "list: gen={} objects={} forms={:?}; topmost object under the click (선택 hit test): {}",
        listed.doc_generation,
        listed.objects.len(),
        &forms[..forms.len().min(8)],
        hit.map(|h| format!(
            "#{} {:?} {:?}/{:?}",
            h.object_id, h.object_type, h.editable, h.reason
        ))
        .unwrap_or_else(|| "none".into())
    ));
    if let Some(h) = hit {
        if h.object_type == PageObjectType::Form {
            o.log("  선택 double-click: EditLayer needs hit.type === \"text\"; this is \"form\" → beginParagraphEdit is never called (nothing happens)");
        }
    }

    // beginParagraphEdit → (refused insideXObject → confirmUngroup → beginParagraphEdit)…
    let mut ungroups = 0;
    let mut final_probe: Option<ParagraphProbe> = None;
    for round in 0..4 {
        let p = match probe(&id, page, c.at) {
            Ok(p) => p,
            Err(e) => {
                o.fail(format!("probe #{round}: {}", err_str(&e)));
                return o;
            }
        };
        o.log(format!("probe #{round}: {}", probe_str(&p)));
        let Some(p) = p else {
            o.log(format!("  forms whose bounds hold the click (index, direct text children, nested text children): {:?}", forms_at(&id, page, c.at)));
            if let Ok(l) = with_doc(&id, move |doc| objects::list(doc, page)) {
                let under: Vec<String> = l
                    .objects
                    .iter()
                    .filter(|x| {
                        c.at[0] >= x.rect.l - 2.0
                            && c.at[0] <= x.rect.r + 2.0
                            && c.at[1] >= x.rect.b - 2.0
                            && c.at[1] <= x.rect.t + 2.0
                    })
                    .map(|x| {
                        format!(
                            "#{} {:?} {:?}/{:?} text={:?} m={:?}",
                            x.object_id,
                            x.object_type,
                            x.editable,
                            x.reason,
                            x.text
                                .as_deref()
                                .map(|t| t.chars().take(20).collect::<String>()),
                            x.matrix
                        )
                    })
                    .take(6)
                    .collect();
                o.log(format!("  objects under the click now: {under:?}"));
            }
            if round == 0 {
                o.fail("probe on grouped text returned None: the click does nothing and no 그룹 해제 is offered");
            } else {
                o.fail(format!("after {ungroups} ungroup(s) the probe at the same point finds no text: the editor never opens (silent)"));
            }
            return o;
        };
        if p.strategy != TextEditStrategy::Refused {
            final_probe = Some(p);
            break;
        }
        if p.reason != Some(NotEditableReason::InsideXObject) {
            if ungroups == 0 {
                o.refused = p.reason;
                o.log(format!(
                    "  refused {:?} before any 그룹 해제 → the UI says why; nothing changed",
                    p.reason
                ));
            } else {
                o.fail(format!(
                    "refused {:?} after {ungroups} ungroup(s) → the user paid for a 그룹 해제 that leads to no edit",
                    p.reason
                ));
            }
            o.ungroups = ungroups;
            return o;
        }
        if p.group_object_id.is_none() {
            o.fail("insideXObject probe without groupObjectId");
        }
        if round == 3 {
            o.fail("still insideXObject after 3 ungroups");
            return o;
        }
        // confirmUngroup: objectId = probe.objectIds[0], expectGeneration = probe.docGeneration
        let obj = p.group_object_id.or(p.object_ids.first().copied());
        let Some(obj) = obj else {
            o.fail("refused probe has no objectIds → confirmUngroup returns false silently");
            return o;
        };
        let cur = {
            let d = id.clone();
            with_doc(&d, move |doc| objects::list(doc, page)).ok()
        };
        let what = cur
            .as_ref()
            .and_then(|l| l.objects.get(obj as usize))
            .map(|x| {
                format!(
                    "{:?} rect=({:.0},{:.0},{:.0},{:.0}) text={:?}",
                    x.object_type,
                    x.rect.l,
                    x.rect.b,
                    x.rect.r,
                    x.rect.t,
                    x.text
                        .as_deref()
                        .map(|t| t.chars().take(30).collect::<String>())
                )
            })
            .unwrap_or_else(|| "<no such object>".into());
        o.log(format!("  probe.objectIds[0] = #{obj}: {what}"));
        let gen = p.doc_generation;
        let d = id.clone();
        let at = c.at;
        let r = with_state(move |st| ungroup::ungroup(st, &d, page, obj, gen, Some(at)));
        match r {
            Ok(u) => {
                ungroups += 1;
                o.partial |= u.partial;
                o.log(format!(
                    "  ungroup_object(#{obj}, expect {gen}) → gen {} moved {} objects{}",
                    u.doc_generation,
                    u.new_object_ids.len(),
                    if u.partial { " (text only)" } else { "" }
                ));
                check_after_ungroup(&mut o, c, &id, &before, &text_before, crop, ungroups);
                undo_redo_check(&mut o, &id, page, &before, crop);
            }
            Err(e) if e.detail.as_deref() == Some("lookChanged") => {
                o.look_changed = true;
                o.ungroups = ungroups;
                o.log(format!(
                    "  ungroup_object → {} (the UI says why)",
                    err_str(&e)
                ));
                if generation(&id) != gen {
                    o.fail("a refused 그룹 해제 changed the generation");
                }
                let (m, s) = diff(&before, &render(&id, page), &[], crop);
                if s > 0.0 || m > 0.001 {
                    o.fail(format!(
                        "a refused 그룹 해제 changed the page: mean {m:.3} share {s:.4}"
                    ));
                }
                return o;
            }
            Err(e) => {
                o.fail(format!("ungroup_object → {}", err_str(&e)));
                return o;
            }
        }
    }
    o.ungroups = ungroups;
    if ungroups > 1 {
        o.fail(format!("the user had to confirm 그룹 해제 {ungroups} times for one click (first objectIds[0] was not the form holding the clicked text, or nesting)"));
    }
    let p = final_probe.expect("editable probe");

    // commitParagraph: dry run push → (unwritable → overlap) → blocked → overlap → write
    let text = format!("{}{MARK}", p.text);
    let mut allow = p.strategy == TextEditStrategy::ReplaceFont;
    let gen = p.doc_generation;
    let edit_of = |flow: ParagraphFlow, dry: bool| ParagraphEdit {
        object_ids: p.object_ids.clone(),
        text: text.clone(),
        width: None,
        font_size_pt: None,
        color: None,
        align: None,
        flow: Some(flow),
        dry_run: dry,
    };
    let run =
        |flow: ParagraphFlow, dry: bool, allow: bool| -> Result<ParagraphEditResult, EngineError> {
            let d = id.clone();
            let e = edit_of(flow, dry);
            with_state(move |st| paragraph::edit(st, &d, page, gen, e, allow))
        };
    let mut flow = ParagraphFlow::Push;
    let plan = match run(flow, true, allow) {
        Ok(r) => Ok(r),
        Err(e) if e.code == seepdf_lib::ipc::ErrorCode::FontCoverage && !allow => {
            allow = true;
            o.log("  fontCoverage → user consents to 글꼴 바꾸기");
            run(flow, true, allow)
        }
        Err(e) => Err(e),
    };
    let plan = match plan {
        Ok(r) => r,
        Err(e)
            if e.detail
                .as_deref()
                .is_some_and(|d| d.starts_with("unwritableContent")) =>
        {
            flow = ParagraphFlow::Overlap;
            o.log(format!("  push unwritable ({}) → overlap", err_str(&e)));
            match run(flow, true, allow) {
                Ok(r) => r,
                Err(e) => {
                    o.fail(format!("edit_paragraph dry run overlap: {}", err_str(&e)));
                    return o;
                }
            }
        }
        Err(e) => {
            o.fail(format!("edit_paragraph dry run: {}", err_str(&e)));
            return o;
        }
    };
    if plan.blocked.is_some() {
        o.log(format!("  blocked {:?} → user picks overlap", plan.blocked));
        flow = ParagraphFlow::Overlap;
    }
    let result = match run(flow, false, allow) {
        Ok(r) => r,
        Err(e) => {
            o.fail(format!("edit_paragraph write: {}", err_str(&e)));
            return o;
        }
    };
    o.log(format!(
        "  edit_paragraph({flow:?}) ok → gen {} shifted {:.1} rect ({:.0},{:.0},{:.0},{:.0})",
        result.objects.doc_generation,
        result.shifted_pt,
        result.rect.l,
        result.rect.b,
        result.rect.r,
        result.rect.t
    ));

    // save → reopen
    let saved = save_bytes(&id);
    let out = corpus_dir().join("saved").join(format!("{}.pdf", c.name));
    std::fs::create_dir_all(out.parent().unwrap()).ok();
    std::fs::write(&out, &saved).ok();
    let re = match open_bytes(saved) {
        Ok(d) => d,
        Err(e) => {
            o.fail(format!("reopen saved: {}", err_str(&e)));
            return o;
        }
    };
    let text_after = page_text(&re.doc_id, page);
    let squash = |s: &str| s.chars().filter(|c| !c.is_whitespace()).collect::<String>();
    if !squash(&text_after).contains(&squash(&text)) {
        o.fail(format!(
            "saved page text does not contain the edited paragraph {:?}",
            text.chars().take(50).collect::<String>()
        ));
    }
    let para_tokens: HashSet<String> = tokens(&p.text).into_iter().collect();
    let after_tokens: HashSet<String> = tokens(&text_after).into_iter().collect();
    let missing: Vec<String> = tokens(&text_before)
        .into_iter()
        .filter(|t| {
            !para_tokens.contains(t)
                && !p.text.contains(t.as_str())
                && !after_tokens.contains(t)
                && !after_tokens.iter().any(|a| a.contains(t.as_str()))
        })
        .collect();
    if !missing.is_empty() {
        o.fail(format!(
            "{} words of the page outside the paragraph are gone from the saved text: {:?}",
            missing.len(),
            &missing[..missing.len().min(8)]
        ));
    }
    let pad = 4.0;
    let mut masks = vec![
        Rect::new(
            p.rect.l - pad,
            p.rect.b - pad,
            p.rect.r + pad,
            p.rect.t + pad,
        ),
        Rect::new(
            result.rect.l - pad,
            result.rect.b - pad,
            result.rect.r + pad,
            result.rect.t + pad,
        ),
    ];
    if let Some(b) = result.moved_band {
        let s = result.shifted_pt.abs() + pad;
        masks.push(Rect::new(b.l - pad, b.b - s, b.r + pad, b.t + s));
    }
    let after = render(&re.doc_id, page);
    let (m, s) = diff(&before, &after, &masks, crop);
    if s > 0.003 {
        o.fail(format!(
            "saved page differs OUTSIDE the edited paragraph: mean {m:.3} share {s:.4} masks {masks:?}"
        ));
        let png = corpus_dir().join("png");
        std::fs::create_dir_all(&png).ok();
        write_png(&before, &png.join(format!("{}-before.png", c.name)));
        write_png(&after, &png.join(format!("{}-edited-saved.png", c.name)));
    } else {
        o.log(format!(
            "  saved render outside the paragraph: mean {m:.3} share {s:.4}"
        ));
    }
    for (sp, b) in &before_shared {
        let a = render(&re.doc_id, *sp);
        let (m, s) = diff(b, &a, &[], crop_of(&re, *sp));
        if s > 0.002 {
            o.fail(format!(
                "page {} (shares the form) changed: mean {m:.3} share {s:.4}",
                sp + 1
            ));
        } else {
            o.log(format!("  shared page {}: unchanged (mean {m:.3})", sp + 1));
        }
    }
    let _ = generation(&id);
    o.edited = true;
    o
}

/// One undo puts the page back exactly as it was; redo brings the 그룹 해제 back.
fn undo_redo_check(o: &mut Outcome, id: &str, page: PageIndex, before: &[u8], crop: Rect) {
    let ungrouped = render(id, page);
    let d = id.to_string();
    let info = with_state(move |st| registry::undo(st, &d, false));
    match info {
        Ok(_) => {
            let (m, s) = diff(before, &render(id, page), &[], crop);
            if m > 0.0 || s > 0.0 {
                o.fail(format!(
                    "undo of 그룹 해제 does not restore the page: mean {m:.3} share {s:.4}"
                ));
            } else {
                o.log("  undo: page identical to the original; redo");
            }
        }
        Err(e) => o.fail(format!("undo: {}", err_str(&e))),
    }
    let d = id.to_string();
    if let Err(e) = with_state(move |st| registry::undo(st, &d, true)) {
        o.fail(format!("redo: {}", err_str(&e)));
    }
    let (m, s) = diff(&ungrouped, &render(id, page), &[], crop);
    if m > 0.0 || s > 0.0 {
        o.fail(format!(
            "redo does not bring the 그룹 해제 back: mean {m:.3} share {s:.4}"
        ));
    }
}

/// Checks an outcome against the case's expectation.
fn judge(c: &Case, o: &Outcome) -> Vec<String> {
    let mut problems = o.failures.clone();
    // a repo fixture may be refused with its reason, as long as nothing was ungrouped first
    if c.source != "corpus" && o.refused.is_some() && o.ungroups == 0 {
        return problems;
    }
    match c.expect {
        Expect::Edit | Expect::EditPartial => {
            if !o.edited {
                problems.push(format!(
                    "expected an edit, got {:?} / lookChanged {}",
                    o.refused, o.look_changed
                ));
            }
            if o.ungroups != 1 {
                problems.push(format!(
                    "expected exactly one 그룹 해제, got {}",
                    o.ungroups
                ));
            }
            let partial = c.expect == Expect::EditPartial;
            if o.partial != partial {
                problems.push(format!("expected partial = {partial}, got {}", o.partial));
            }
        }
        Expect::Refused(reason) => {
            if o.refused != Some(reason) || o.ungroups != 0 {
                problems.push(format!(
                    "expected a refusal {reason:?} before any 그룹 해제, got {:?} after {}",
                    o.refused, o.ungroups
                ));
            }
        }
        Expect::LookChanged => {
            if !o.look_changed || o.ungroups != 0 {
                problems.push(format!(
                    "expected 그룹 해제 refused (lookChanged), got lookChanged {} after {} ungroups",
                    o.look_changed, o.ungroups
                ));
            }
        }
    }
    problems
}

fn corpus_case(name: &str) {
    let cases = generate_corpus();
    let c = cases
        .iter()
        .find(|c| c.name == name)
        .unwrap_or_else(|| panic!("no corpus case {name}"));
    let o = run_case(c);
    let problems = judge(c, &o);
    assert!(
        problems.is_empty(),
        "{name}: {problems:#?}\n{}",
        o.lines.join("\n")
    );
}

macro_rules! corpus_cases {
    ($($test:ident => $name:literal,)*) => {
        $(
            #[test]
            fn $test() {
                corpus_case($name);
            }
        )*
        const CASE_NAMES: &[&str] = &[$($name),*];
    };
}

corpus_cases! {
    case_c01_plain => "c01-plain",
    case_c02a_office_group_opaque => "c02a-office-group-opaque",
    case_c02b_office_group_isolated_opaque => "c02b-office-group-isolated-opaque",
    case_c02c_page_group_plain_form => "c02c-page-group-plain-form",
    case_c03a_alpha => "c03a-alpha-ca-0.5",
    case_c03b_blend_multiply => "c03b-blend-multiply",
    case_c03c_smask => "c03c-smask",
    case_c03d_isolated_knockout => "c03d-isolated-knockout",
    case_c03e_child_alpha_inside_form => "c03e-child-alpha-inside-form",
    case_c03f_alpha_overlap_text_apart => "c03f-alpha-overlap-text-apart",
    case_c03g_alpha_text_over_graphics => "c03g-alpha-text-over-graphics",
    case_c04a_nested_2 => "c04a-nested-2",
    case_c04b_nested_3_outer_group => "c04b-nested-3-outer-group",
    case_c04c_nested_text_both_levels => "c04c-nested-text-both-levels",
    case_c05_shared_form => "c05-shared-form",
    case_c06a_matrix_scale_bbox_clip => "c06a-matrix-scale-bbox-clip",
    case_c06b_rotated_90 => "c06b-rotated-90",
    case_c06c_matrix_rotate_30 => "c06c-matrix-rotate-30",
    case_c06d_page_clip_around_do => "c06d-page-clip-around-do",
    case_c07a_hangul_cid_whole_page_form => "c07a-hangul-embedded-cid-whole-page-form",
    case_c07b_hangul_cid_office_group => "c07b-hangul-embedded-cid-office-group",
    case_c07c_hangul_nonembedded_uniks => "c07c-hangul-nonembedded-uniks",
    case_c08a_tagged_mcid => "c08a-tagged-mcid",
    case_c08b_ocg_on_inside_form => "c08b-ocg-on-inside-form",
    case_c08c_ocg_off_line_inside_form => "c08c-ocg-off-line-inside-form",
    case_c08d_hidden_form_oc_off => "c08d-hidden-form-oc-off",
    case_c09a_whole_page_form => "c09a-whole-page-form",
    case_c09b_whole_page_form_office_group => "c09b-whole-page-form-office-group",
    case_c09c_tracemonkey_p1_wrapped => "c09c-tracemonkey-p1-wrapped",
    case_c10a_inline_image => "c10a-inline-image",
    case_c10b_shading_sh => "c10b-shading-sh",
    case_c10c_tc_tw_tz => "c10c-tc-tw-tz",
    case_c11_overlapping_forms_border_first => "c11-overlapping-forms-border-first",
    case_c12a_font_name_collision_page_vs_form => "c12a-font-name-collision-page-vs-form",
    case_c12b_font_name_collision_two_forms => "c12b-font-name-collision-two-forms",
    case_c12c_same_basefont_different_encoding => "c12c-same-basefont-different-encoding",
}

#[test]
fn every_corpus_case_has_a_test() {
    let names: Vec<String> = generate_corpus().into_iter().map(|c| c.name).collect();
    for n in &names {
        assert!(
            CASE_NAMES.contains(&n.as_str()),
            "corpus case {n} has no test"
        );
    }
    assert_eq!(names.len(), CASE_NAMES.len());
}

/// The repo fixtures with text inside a group (TAMReview.pdf, tracemonkey.pdf): the click is
/// edited after one 그룹 해제, or refused — with the reason — before anything is ungrouped
/// (TAMReview's text has no `/ToUnicode`: it could never be edited, so 그룹 해제 is not offered).
#[test]
fn fixture_cases_edit_or_refuse_first() {
    let mut summary = String::new();
    let cases = fixture_cases(&mut summary);
    assert!(cases.len() >= 10, "{summary}");
    let mut edited = 0;
    for c in &cases {
        let o = run_case(c);
        assert!(
            o.failures.is_empty() && !o.look_changed,
            "{}: {:#?}\n{}",
            c.name,
            o.failures,
            o.lines.join("\n")
        );
        if o.edited {
            assert_eq!(o.ungroups, 1, "{}: one 그룹 해제 per click", c.name);
            edited += 1;
        } else {
            assert_eq!(o.ungroups, 0, "{}", c.name);
            assert!(o.refused.is_some(), "{}", c.name);
        }
        if c.name.contains("tracemonkey") {
            assert!(
                o.edited,
                "{}: tracemonkey's grouped text is editable",
                c.name
            );
        }
    }
    assert!(edited >= 6, "{edited} edited");
}

/// The Inspector's 그룹 해제 (no click point): the group and its nested groups that draw text come
/// out, the page looks the same, and no group with text is left where the group was.
#[test]
fn inspector_ungroup_on_every_case() {
    for c in generate_corpus() {
        let doc = open_bytes(c.bytes.clone()).expect("open");
        let id = doc.doc_id.clone();
        let page = c.page;
        let crop = crop_of(&doc, page);
        let before = render(&id, page);
        let top = {
            let (d, at) = (id.clone(), c.at);
            let bindings_hit = with_doc(&d, move |doc| {
                let bindings = doc.bindings();
                let pg = doc.page(page)?;
                ungroup::group_hit(bindings, pg, at, None).map(|h| h.map(|h| h.top))
            })
            .expect("hit");
            match bindings_hit {
                Some(t) => t as ObjectId,
                None => continue, // c08d: a hidden group; nothing to click
            }
        };
        let gen = generation(&id);
        let d = id.clone();
        let r = with_state(move |st| ungroup::ungroup(st, &d, page, top, gen, None));
        match (c.expect, r) {
            (Expect::LookChanged, Err(e)) => {
                assert_eq!(e.detail.as_deref(), Some("lookChanged"), "{}", c.name);
                assert_eq!(generation(&id), gen, "{}", c.name);
            }
            (_, Ok(u)) => {
                let (m, s) = diff(&before, &render(&id, page), &[], crop);
                assert!(
                    m <= 0.35 && s <= 0.0005,
                    "{}: mean {m:.3} share {s:.4}",
                    c.name
                );
                for &n in &u.new_object_ids {
                    let o = &u.objects[n as usize];
                    assert!(
                        !(o.object_type == PageObjectType::Form && o.text.is_some()) || u.partial,
                        "{}: object {n} is still a group with text",
                        c.name
                    );
                }
            }
            (e, r) => panic!(
                "{}: expected {e:?}, got {:?}",
                c.name,
                r.map(|u| u.partial).map_err(|e| err_str(&e))
            ),
        }
    }
}

// ---------------------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------------------

#[test]
fn ungroup_corpus_report() {
    let mut cases = generate_corpus();
    let mut report = String::new();
    if std::env::var("UNGROUP_CORPUS_NO_FIXTURES").is_err() {
        cases.extend(fixture_cases(&mut report));
    }
    let mut failed = 0;
    for c in &cases {
        let t0 = Instant::now();
        let o = run_case(c);
        let problems = judge(c, &o);
        let status = if problems.is_empty() { "PASS" } else { "FAIL" };
        if !problems.is_empty() {
            failed += 1;
        }
        let _ = writeln!(
            report,
            "=== {status} {} [{}] ({} ms)\n    {}\n    click page {} at ({:.1}, {:.1})",
            c.name,
            c.source,
            t0.elapsed().as_millis(),
            c.what,
            c.page + 1,
            c.at[0],
            c.at[1]
        );
        for l in &o.lines {
            let _ = writeln!(report, "    {l}");
        }
        for p in problems.iter().filter(|p| !o.failures.contains(p)) {
            let _ = writeln!(report, "    FAIL: {p}");
        }
    }
    let _ = writeln!(report, "\n{failed} of {} cases failed", cases.len());
    println!("{report}");
    std::fs::write(corpus_dir().join("report.txt"), &report).expect("write report");
}

/// The user's screenshot: "expectGeneration 3 but the document is at 4; re-list the page
/// objects" after a 그룹 해제 went through. The engine used to run `probe_paragraph` on
/// `Lane::Interactive`, before the queued `ungroup_object` (`Lane::Edit`): a second click while
/// the first 그룹 해제 was still queued was probed at the OLD generation, refused `insideXObject`
/// again, and its 그룹 해제 was pinned to that old generation. v0.3.1 runs the probe on
/// `Lane::Edit` (`commands::objects::probe_paragraph`), FIFO with the mutations: the second
/// probe sees the ungrouped page and the paragraph is simply editable.
#[test]
fn probe_after_a_queued_ungroup_sees_it() {
    let bytes = one_form(dictionary! {}, "", dictionary! {}, PARA);
    let doc = open_bytes(bytes).expect("open");
    let id = doc.doc_id.clone();
    let g0 = generation(&id);
    let p1 = probe(&id, 0, PARA_AT).unwrap().expect("probe 1");
    assert_eq!(p1.reason, Some(NotEditableReason::InsideXObject));
    assert_eq!(p1.group_object_id, Some(1));
    let order = Arc::new(Mutex::new(Vec::<String>::new()));

    // The engine is busy (a render in flight — the page repaints when the confirm dialog closes).
    let busy = std::thread::spawn({
        let order = order.clone();
        move || {
            engine()
                .call_blocking(Lane::Interactive, "test/busy", move |_st| {
                    std::thread::sleep(Duration::from_millis(400));
                    Ok(())
                })
                .unwrap();
            order.lock().unwrap().push("busy render done".into());
        }
    });
    std::thread::sleep(Duration::from_millis(50));
    // 1st click's 그룹 해제: queued on Lane::Edit
    let first = std::thread::spawn({
        let (id, order, obj, gen) = (
            id.clone(),
            order.clone(),
            p1.group_object_id.unwrap(),
            p1.doc_generation,
        );
        move || {
            let d = id.clone();
            let r = engine().call_blocking(Lane::Edit, "ungroup_object", move |st| {
                ungroup::ungroup(st, &d, 0, obj, gen, Some(PARA_AT))
            });
            order.lock().unwrap().push(format!(
                "ungroup #1 → {:?}",
                r.as_ref().map(|u| u.doc_generation).map_err(err_str)
            ));
            r
        }
    });
    std::thread::sleep(Duration::from_millis(50));
    // 2nd click: probe_paragraph, on the lane the command now uses
    let second = std::thread::spawn({
        let (id, order) = (id.clone(), order.clone());
        move || {
            let d = id.clone();
            let r = engine()
                .call_blocking(Lane::Edit, "probe_paragraph", move |st| {
                    let doc = st.doc_mut(&d)?;
                    paragraph::probe(doc, 0, PARA_AT)
                })
                .unwrap();
            order
                .lock()
                .unwrap()
                .push(format!("probe #2 → {}", probe_str(&r)));
            r
        }
    });
    busy.join().unwrap();
    let u1 = first.join().unwrap().expect("ungroup #1 succeeds");
    let p2 = second.join().unwrap().expect("probe #2");
    let order = order.lock().unwrap().clone();
    println!("order: {order:#?}");
    assert!(
        order.iter().position(|l| l.starts_with("ungroup #1"))
            < order.iter().position(|l| l.starts_with("probe #2")),
        "{order:?}"
    );
    assert_eq!(u1.doc_generation, g0 + 1);
    assert_eq!(p2.doc_generation, g0 + 1, "the probe ran after the ungroup");
    assert_eq!(p2.strategy, TextEditStrategy::InPlace);
    assert_eq!(p2.reason, None);
}

/// Evidence for the font mix-up after a second ungroup on tracemonkey.pdf p.6: every font
/// reachable from the page (through nested forms) with its resource name, object id and BaseFont.
#[test]
#[ignore = "diagnostic: cargo test --release --test ungroup_corpus fonts_of_tracemonkey_p6 -- --ignored --nocapture"]
fn fonts_of_tracemonkey_p6() {
    let doc = Document::load_mem(&std::fs::read(fixture("tracemonkey.pdf")).unwrap()).unwrap();
    let page_id = doc.get_pages()[&6];
    fn walk(doc: &Document, res: &Dictionary, path: &str, depth: usize) {
        let get = |o: &Object| -> Option<Dictionary> {
            match o {
                Object::Reference(id) => doc.get_dictionary(*id).ok().cloned(),
                Object::Dictionary(d) => Some(d.clone()),
                _ => None,
            }
        };
        if let Some(fonts) = res.get(b"Font").ok().and_then(get) {
            for (name, f) in fonts.iter() {
                let id = f
                    .as_reference()
                    .map(|r| format!("{} {} R", r.0, r.1))
                    .unwrap_or_else(|_| "direct".into());
                let base = get(f)
                    .and_then(|d| {
                        d.get(b"BaseFont")
                            .ok()
                            .and_then(|b| b.as_name().ok())
                            .map(|b| String::from_utf8_lossy(b).to_string())
                    })
                    .unwrap_or_default();
                println!(
                    "{path} /{} = {id} BaseFont {base}",
                    String::from_utf8_lossy(name)
                );
            }
        }
        if depth > 4 {
            return;
        }
        if let Some(xo) = res.get(b"XObject").ok().and_then(get) {
            for (name, x) in xo.iter() {
                if let Ok(id) = x.as_reference() {
                    if let Ok(s) = doc.get_object(id).and_then(Object::as_stream) {
                        if s.dict.get(b"Subtype").and_then(Object::as_name).ok()
                            == Some(b"Form".as_slice())
                        {
                            if let Some(r) = s.dict.get(b"Resources").ok().and_then(get) {
                                walk(
                                    doc,
                                    &r,
                                    &format!(
                                        "{path}/{}({} {})",
                                        String::from_utf8_lossy(name),
                                        id.0,
                                        id.1
                                    ),
                                    depth + 1,
                                );
                            }
                        }
                    }
                }
            }
        }
    }
    let (inline, ids) = doc.get_page_resources(page_id).unwrap();
    if let Some(r) = inline {
        walk(&doc, r, "page", 0);
    }
    for id in ids {
        walk(&doc, doc.get_dictionary(id).unwrap(), "page", 0);
    }
}

/// A page whose content lopdf-level inlining cannot read (a stray `}` PDFium skips) is
/// ungrouped the v0.3.0 way — PDFium moves the objects — under the same render check, and
/// the paragraph is editable afterwards.
#[test]
fn fallback_when_the_content_cannot_be_read() {
    let bytes = one_form(dictionary! {}, "", dictionary! {}, PARA);
    let mut doc = Document::load_mem(&bytes).unwrap();
    let page_id = *doc.get_pages().values().next().unwrap();
    let content_id = doc.get_page_contents(page_id)[0];
    let stream = doc
        .get_object_mut(content_id)
        .and_then(Object::as_stream_mut)
        .unwrap();
    let mut data = b"} ".to_vec();
    data.extend_from_slice(&stream.content);
    stream.set_plain_content(data);
    let mut out = Vec::new();
    doc.save_to(&mut out).unwrap();
    let doc = open_bytes(out).expect("open");
    let id = doc.doc_id.clone();
    let before = render(&id, 0);
    let crop = crop_of(&doc, 0);
    let p = probe(&id, 0, PARA_AT).unwrap().expect("probe");
    assert_eq!(p.group_object_id, Some(1));
    let (d, gen) = (id.clone(), p.doc_generation);
    let u =
        with_state(move |st| ungroup::ungroup(st, &d, 0, 1, gen, Some(PARA_AT))).expect("ungroup");
    assert!(!u.partial);
    assert!(u
        .objects
        .iter()
        .all(|o| o.object_type != PageObjectType::Form));
    let (m, s) = diff(&before, &render(&id, 0), &[], crop);
    assert!(m < 0.35 && s < 0.0005, "mean {m:.3} share {s:.4}");
    let p = probe(&id, 0, PARA_AT).unwrap().expect("probe after");
    assert_eq!(p.strategy, TextEditStrategy::InPlace);
}
