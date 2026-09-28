//! Link annotations with a target (P2): `create_link`, `update_link`, `delete_link`.
//!
//! Which writer each change takes:
//!
//! | change | writer | encrypted document |
//! |---|---|---|
//! | new link to a web address | PDFium: `FPDFPage_CreateAnnot(LINK)` + `SetRect` + `SetURI` + `/Border [0 0 0]` | works |
//! | new link to a page | lopdf: a `/Link` dictionary with `/Dest` appended to the page's `/Annots` | `unsupported` |
//! | move / resize only | PDFium: `FPDFAnnot_SetRect` | works |
//! | web address → web address (no `/Dest` present) | PDFium: `FPDFAnnot_SetURI` replaces `/A` | works |
//! | anything → page, page → web address | lopdf: `/Dest` set and `/A` removed, or the reverse | `unsupported` |
//! | delete | PDFium: `FPDFPage_RemoveAnnot` | works |
//!
//! PDFium has no setter for `/Dest`, and `FPDFAnnot_SetURI` leaves an existing `/Dest` in place
//! (a link with both is invalid PDF), which is why those cases are rewrites. Every lopdf rewrite
//! is checked by reopening with PDFium and reading the link back by its `/NM`.
//!
//! Every command returns the page's fresh `AnnotResult` (like `create_annotation`), announces
//! only that page and keeps the page text — a link has no appearance and touches no content.

use super::{
    dest_array, encode_uri, finite, normalize_view, page_id, page_ids, refuse_encrypted, same_f32,
    same_view, uri_action, verify_failed,
};
use crate::engine::annot::{self, ScratchPage};
use crate::engine::raw;
use crate::engine::registry::{self, MutateOpts};
use crate::engine::types::EngineState;
use crate::ipc::types::{
    Annot, AnnotId, AnnotKind, AnnotList, AnnotResult, ChangeReason, LinkDest, LinkTarget,
    OutlineDest, PageIndex, Rect,
};
use crate::ipc::{EngineError, ErrorCode};
use lopdf::{Dictionary, Document, Object, ObjectId};
use pdfium_render::prelude::{PdfDocument, PdfPageIndex, PdfiumLibraryBindings};

/// Smallest side of a link's click area, in points.
const MIN_SIDE: f32 = 1.0;

/// A target after validation, exactly as it will read back.
#[derive(Debug, Clone, PartialEq)]
enum Target {
    Page(LinkDest),
    Url(String),
}

fn opts(label: &'static str, page: PageIndex) -> MutateOpts {
    MutateOpts::new(label, ChangeReason::Edit)
        .page(page)
        .keeps_text()
}

/// `create_link` — a Link annotation on `page` covering `rect`, going to `target`.
pub fn create_link(
    st: &mut EngineState<'_>,
    doc_id: &str,
    page: PageIndex,
    rect: Rect,
    target: &LinkTarget,
) -> Result<AnnotResult, EngineError> {
    let page_count = st.doc(doc_id)?.page_count();
    check_page(page, page_count)?;
    let rect = normalize_rect(rect)?;
    let id = match resolve(target, page_count)? {
        Target::Url(url) => registry::mutate(st, doc_id, opts("undo.linkCreate", page), |doc| {
            annot::create::create_link(doc, page, rect, &[], &url)
        })?,
        Target::Page(dest) => {
            refuse_encrypted(st, doc_id)?;
            let id = annot::new_id();
            let want = Target::Page(dest);
            let check = check_link(page, id.clone(), Some(rect), want);
            let new_id = id.clone();
            registry::mutate_bytes_checked(
                st,
                doc_id,
                opts("undo.linkCreate", page),
                move |bytes, _| add_link(bytes, page, rect, &new_id, dest),
                check,
            )?;
            id
        }
    };
    result_for(st, doc_id, page, Some(id), None)
}

/// `update_link` — a new click area and / or a new target for the link `id` on `page`.
/// `previous` in the result is the link before the edit.
pub fn update_link(
    st: &mut EngineState<'_>,
    doc_id: &str,
    page: PageIndex,
    id: &str,
    rect: Option<Rect>,
    target: Option<&LinkTarget>,
) -> Result<AnnotResult, EngineError> {
    let page_count = st.doc(doc_id)?.page_count();
    check_page(page, page_count)?;
    let previous = find_link(st, doc_id, page, id)?;
    let rect = rect.map(normalize_rect).transpose()?;
    let target = target.map(|t| resolve(t, page_count)).transpose()?;
    if rect.is_none() && target.is_none() {
        return Err(EngineError::invalid("update_link needs a rect or a target"));
    }

    let has_dest = {
        let doc = st.doc_mut(doc_id)?;
        let bindings = doc.bindings();
        let pdf_page = doc.page(page)?;
        let index = annot::index_of(bindings, pdf_page, id)?;
        raw::annot::get(bindings, pdf_page, index)?.has_key("Dest")
    };
    let pdfium = match &target {
        None => true,
        Some(Target::Url(_)) => !has_dest,
        Some(Target::Page(_)) => false,
    };

    if pdfium {
        let url = match &target {
            Some(Target::Url(url)) => Some(url.clone()),
            _ => None,
        };
        let id_owned = id.to_string();
        registry::mutate(st, doc_id, opts("undo.linkEdit", page), move |doc| {
            let bindings = doc.bindings();
            let scratch = ScratchPage::open(doc, page)?;
            let index = annot::index_of(bindings, &scratch.page, &id_owned)?;
            let mut a = raw::annot::get(bindings, &scratch.page, index)?;
            if let Some(r) = rect {
                if !a.set_rect(r) {
                    return Err(EngineError::new(
                        ErrorCode::Pdfium,
                        "FPDFAnnot_SetRect failed",
                    ));
                }
            }
            if let Some(url) = &url {
                if !a.set_uri(url) {
                    return Err(EngineError::new(
                        ErrorCode::Pdfium,
                        "FPDFAnnot_SetURI failed",
                    ));
                }
            }
            a.set_string("M", &annot::pdf_date_now());
            Ok(())
        })?;
    } else {
        refuse_encrypted(st, doc_id)?;
        let target = target.expect("the lopdf path always has a target");
        let check = check_link(page, id.to_string(), rect, target.clone());
        let id_owned = id.to_string();
        registry::mutate_bytes_checked(
            st,
            doc_id,
            opts("undo.linkEdit", page),
            move |bytes, _| rewrite_link(bytes, page, &id_owned, rect, &target),
            check,
        )?;
    }
    result_for(st, doc_id, page, Some(id.to_string()), Some(previous))
}

/// `delete_link` — removes the link `id` from `page` (PDFium, works on encrypted documents).
pub fn delete_link(
    st: &mut EngineState<'_>,
    doc_id: &str,
    page: PageIndex,
    id: &str,
) -> Result<AnnotResult, EngineError> {
    let page_count = st.doc(doc_id)?.page_count();
    check_page(page, page_count)?;
    let previous = find_link(st, doc_id, page, id)?;
    let ids = vec![id.to_string()];
    registry::mutate(st, doc_id, opts("undo.linkDelete", page), |doc| {
        annot::delete(doc, page, &ids)
    })?;
    result_for(st, doc_id, page, None, Some(previous))
}

/// The page's annotations after an edit plus the one that was touched — the `AnnotResult`
/// every annotation command returns.
pub fn result_for(
    st: &mut EngineState<'_>,
    doc_id: &str,
    page: PageIndex,
    id: Option<AnnotId>,
    previous: Option<Annot>,
) -> Result<AnnotResult, EngineError> {
    let doc = st.doc_mut(doc_id)?;
    let annots = annot::list(doc, page)?;
    let annot = id
        .as_ref()
        .and_then(|id| annots.iter().find(|a| &a.id == id).cloned());
    Ok(AnnotResult {
        list: AnnotList {
            doc_id: doc.doc_id.clone(),
            page,
            doc_generation: doc.generation,
            annots,
        },
        annot,
        previous,
    })
}

// ---------------------------------------------------------------------------------------
// validation
// ---------------------------------------------------------------------------------------

fn check_page(page: PageIndex, page_count: u16) -> Result<(), EngineError> {
    if page >= page_count {
        return Err(EngineError::not_found(format!("page {page} of {page_count}")).with_page(page));
    }
    Ok(())
}

/// Ordered corners, finite, at least [`MIN_SIDE`] on each side.
fn normalize_rect(r: Rect) -> Result<Rect, EngineError> {
    for v in [r.l, r.b, r.r, r.t] {
        finite(Some(v), "rect")?;
    }
    let out = Rect::new(r.l.min(r.r), r.b.min(r.t), r.l.max(r.r), r.b.max(r.t));
    if out.width() < MIN_SIDE || out.height() < MIN_SIDE {
        return Err(EngineError::invalid(
            "a link needs a click area of at least 1 pt × 1 pt",
        ));
    }
    Ok(out)
}

fn resolve(target: &LinkTarget, page_count: u16) -> Result<Target, EngineError> {
    match target {
        LinkTarget::Url(u) => {
            let url = encode_uri(&u.url);
            if url.is_empty() {
                return Err(EngineError::invalid("a web link needs an address"));
            }
            Ok(Target::Url(url))
        }
        LinkTarget::Page(d) => {
            if d.page >= page_count {
                return Err(EngineError::invalid(format!(
                    "the link target page {} is not in this {page_count}-page document",
                    d.page
                ))
                .with_page(d.page));
            }
            let view = normalize_view(Some(OutlineDest {
                x: d.x,
                y: d.y,
                zoom: d.zoom,
            }))?
            .unwrap_or_default();
            Ok(Target::Page(LinkDest {
                page: d.page,
                x: view.x,
                y: view.y,
                zoom: view.zoom,
            }))
        }
    }
}

/// The link `id` on `page`, or `notFound`; `invalidArgument` when `id` is another annotation.
fn find_link(
    st: &mut EngineState<'_>,
    doc_id: &str,
    page: PageIndex,
    id: &str,
) -> Result<Annot, EngineError> {
    let doc = st.doc_mut(doc_id)?;
    // Listing also stamps a `/NM` on annotations that lack one, so the lopdf rewrite can find
    // this one in the serialised bytes.
    let found = annot::list(doc, page)?
        .into_iter()
        .find(|a| a.id == id)
        .ok_or_else(|| EngineError::not_found(format!("annotation '{id}'")).with_page(page))?;
    if found.kind != AnnotKind::Link {
        return Err(EngineError::invalid(format!(
            "annotation '{id}' is not a link"
        )));
    }
    Ok(found)
}

// ---------------------------------------------------------------------------------------
// lopdf
// ---------------------------------------------------------------------------------------

fn rect_object(r: Rect) -> Object {
    Object::Array(vec![
        Object::Real(r.l),
        Object::Real(r.b),
        Object::Real(r.r),
        Object::Real(r.t),
    ])
}

fn view_of(d: &LinkDest) -> Option<OutlineDest> {
    Some(OutlineDest {
        x: d.x,
        y: d.y,
        zoom: d.zoom,
    })
}

/// A new `/Link` with a `/Dest`, appended to the page's `/Annots`.
fn add_link(
    bytes: &[u8],
    page: PageIndex,
    rect: Rect,
    id: &str,
    dest: LinkDest,
) -> Result<Vec<u8>, EngineError> {
    let mut doc = super::load(bytes)?;
    let pages = page_ids(&doc);
    let page_obj = page_id(&pages, page)?;
    let target = page_id(&pages, dest.page)?;
    let now = annot::pdf_date_now();

    let mut dict = Dictionary::new();
    dict.set("Type", Object::Name(b"Annot".to_vec()));
    dict.set("Subtype", Object::Name(b"Link".to_vec()));
    dict.set("Rect", rect_object(rect));
    dict.set(
        "Border",
        Object::Array(vec![
            Object::Integer(0),
            Object::Integer(0),
            Object::Integer(0),
        ]),
    );
    // /F 4: Print (FLAT_PRINT and printing keep it).
    dict.set(
        "F",
        Object::Integer(raw::consts::FPDF_ANNOT_FLAG_PRINT as i64),
    );
    dict.set("NM", Object::string_literal(id.as_bytes().to_vec()));
    dict.set("P", Object::Reference(page_obj));
    dict.set("Dest", dest_array(target, view_of(&dest)));
    dict.set(
        "CreationDate",
        Object::string_literal(now.as_bytes().to_vec()),
    );
    dict.set("M", Object::string_literal(now.as_bytes().to_vec()));
    let annot_id = doc.add_object(Object::Dictionary(dict));

    let existing = doc
        .get_dictionary(page_obj)
        .map_err(|e| crate::engine::save::lopdf_error("page", e))?
        .get(b"Annots")
        .ok()
        .cloned();
    let appended = match existing {
        Some(Object::Reference(array)) => match doc.get_object_mut(array) {
            Ok(Object::Array(items)) => {
                items.push(Object::Reference(annot_id));
                true
            }
            _ => false,
        },
        Some(Object::Array(mut items)) => {
            items.push(Object::Reference(annot_id));
            page_dict_mut(&mut doc, page_obj)?.set("Annots", Object::Array(items));
            true
        }
        _ => false,
    };
    if !appended {
        page_dict_mut(&mut doc, page_obj)?
            .set("Annots", Object::Array(vec![Object::Reference(annot_id)]));
    }
    super::write(doc)
}

fn page_dict_mut(doc: &mut Document, page: ObjectId) -> Result<&mut Dictionary, EngineError> {
    doc.get_dictionary_mut(page)
        .map_err(|e| crate::engine::save::lopdf_error("page", e))
}

/// New target (and click area) for an existing link: `/Dest` set and `/A` removed, or `/A`
/// set and `/Dest` removed — never both, which is invalid.
fn rewrite_link(
    bytes: &[u8],
    page: PageIndex,
    id: &str,
    rect: Option<Rect>,
    target: &Target,
) -> Result<Vec<u8>, EngineError> {
    let mut doc = super::load(bytes)?;
    let pages = page_ids(&doc);
    let page_obj = page_id(&pages, page)?;
    let dest = match target {
        Target::Page(d) => Some(dest_array(page_id(&pages, d.page)?, view_of(d))),
        Target::Url(_) => None,
    };
    let now = annot::pdf_date_now();
    let edit = |dict: &mut Dictionary| {
        if let Some(r) = rect {
            dict.set("Rect", rect_object(r));
        }
        match (target, &dest) {
            (Target::Page(_), Some(dest)) => {
                dict.set("Dest", dest.clone());
                dict.remove(b"A");
            }
            (Target::Url(url), _) => {
                dict.set("A", uri_action(url));
                dict.remove(b"Dest");
            }
            _ => {}
        }
        dict.set("M", Object::string_literal(now.as_bytes().to_vec()));
    };

    let items: Vec<Object> = match doc
        .get_dictionary(page_obj)
        .map_err(|e| crate::engine::save::lopdf_error("page", e))?
        .get(b"Annots")
    {
        Ok(Object::Reference(array)) => doc
            .get_object(*array)
            .and_then(Object::as_array)
            .cloned()
            .unwrap_or_default(),
        Ok(Object::Array(items)) => items.clone(),
        _ => Vec::new(),
    };
    for (index, item) in items.iter().enumerate() {
        let (indirect, dict) = match item {
            Object::Reference(r) => match doc.get_dictionary(*r) {
                Ok(d) => (Some(*r), d),
                Err(_) => continue,
            },
            Object::Dictionary(d) => (None, d),
            _ => continue,
        };
        let name = dict
            .get(b"NM")
            .ok()
            .and_then(|o| lopdf::decode_text_string(o).ok());
        if name.as_deref() != Some(id) {
            continue;
        }
        match indirect {
            Some(r) => edit(
                doc.get_dictionary_mut(r)
                    .map_err(|e| crate::engine::save::lopdf_error("link", e))?,
            ),
            None => {
                let annots = inline_annots_mut(&mut doc, page_obj)?;
                if let Some(Object::Dictionary(d)) = annots.get_mut(index) {
                    edit(d);
                }
            }
        }
        return super::write(doc);
    }
    Err(
        EngineError::not_found(format!("link '{id}' is not in page {page}'s /Annots"))
            .with_page(page),
    )
}

/// The page's `/Annots` array, wherever it lives, mutably.
fn inline_annots_mut(doc: &mut Document, page: ObjectId) -> Result<&mut Vec<Object>, EngineError> {
    let array_ref = page_dict_mut(doc, page)?
        .get(b"Annots")
        .ok()
        .and_then(|o| o.as_reference().ok());
    let err = |e: lopdf::Error| crate::engine::save::lopdf_error("/Annots", e);
    match array_ref {
        Some(r) => doc
            .get_object_mut(r)
            .and_then(Object::as_array_mut)
            .map_err(err),
        None => page_dict_mut(doc, page)?
            .get_mut(b"Annots")
            .and_then(Object::as_array_mut)
            .map_err(err),
    }
}

// ---------------------------------------------------------------------------------------
// PDFium check of a rewrite
// ---------------------------------------------------------------------------------------

/// Reopened by PDFium, the link `id` on `page` is a Link with the wanted target (and area).
fn check_link(
    page: PageIndex,
    id: String,
    rect: Option<Rect>,
    want: Target,
) -> impl FnOnce(&'static dyn PdfiumLibraryBindings, &PdfDocument<'_>) -> Result<(), EngineError> {
    move |bindings, reopened| {
        let document = reopened.raw_handle();
        let pdf_page = reopened
            .pages()
            .get(page as PdfPageIndex)
            .map_err(|e| verify_failed(format!("page {page} of the rewrite: {e:?}")))?;
        for i in 0..raw::annot::count(bindings, &pdf_page) {
            let a = raw::annot::get(bindings, &pdf_page, i)?;
            if a.string("NM").as_deref() != Some(id.as_str()) {
                continue;
            }
            if a.subtype() != raw::consts::FPDF_ANNOT_LINK {
                return Err(verify_failed(format!("'{id}' is no longer a Link")));
            }
            if let (Some(want), Some(got)) = (rect, a.rect()) {
                let same = [
                    (want.l, got.l),
                    (want.b, got.b),
                    (want.r, got.r),
                    (want.t, got.t),
                ]
                .iter()
                .all(|(w, g)| same_f32(Some(*w), Some(*g)));
                if !same {
                    return Err(verify_failed(format!(
                        "the link's /Rect reads back as {got:?}"
                    )));
                }
            }
            let ok = match &want {
                Target::Page(d) => a.link_dest(document).is_some_and(|got| {
                    got.page == d.page
                        && same_view(
                            Some(OutlineDest {
                                x: got.x,
                                y: got.y,
                                zoom: got.zoom,
                            }),
                            Some(OutlineDest {
                                x: d.x,
                                y: d.y,
                                zoom: d.zoom,
                            }),
                        )
                }),
                Target::Url(url) => {
                    a.link_dest(document).is_none()
                        && a.uri(document).as_deref() == Some(url.as_str())
                }
            };
            return if ok {
                Ok(())
            } else {
                Err(verify_failed(format!(
                    "PDFium reads link '{id}' with another target"
                )))
            };
        }
        Err(verify_failed(format!(
            "link '{id}' is missing from page {page} of the rewrite"
        )))
    }
}
