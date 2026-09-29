//! R4 (v0.3): 그룹 해제 — replacing a Form XObject on the page by the objects inside it.
//!
//! Office exports and many PDF producers wrap the whole page (or its body) in one Form XObject.
//! PDFium reads the text inside, but `FPDFPage_GenerateContent` only rewrites the page stream
//! and re-emits `/Form Do`, so an edit of a child is lost on save (text spike §8.7) and a
//! redaction cannot delete it. Ungrouping moves every child out:
//!
//! 1. `FPDFFormObj_RemoveObject` detaches the child (ownership passes to us);
//! 2. `FPDFPageObj_Transform` + `FPDFPageObj_TransformClipPath` apply the form matrix, so the
//!    child lands where it was drawn (form ∘ child);
//! 3. fill and stroke colours are re-set to their RGB value — PDFium's content generator only
//!    writes `rg` / `RG` for DeviceRGB and DeviceGray colours and silently drops an ICC,
//!    `/CalRGB` or `/Separation` colour, which would paint the child black;
//! 4. `FPDFPage_InsertObjectAtIndex` puts it at the form's index (children keep their order);
//! 5. the emptied form object is removed and the page content regenerated.
//!
//! What PDFium cannot carry over, and therefore refuses in the explicit `ungroup_object`
//! command (`unsupported`, detail `groupTransparency`): a form object drawn with transparency
//! (a group alpha, blend mode or soft mask) — its children would come out opaque. The
//! redaction ungroups such a form anyway: removing the marked content matters more there than
//! the group's opacity.
//!
//! Also lost, by PDFium's content generator for any text it rewrites (not specific to
//! ungrouping): the text-state operators `Tc` / `Tw` / `Tz` of text it did not create, so a
//! line justified by character spacing tightens (`TAMReview.pdf` p.2 has three). The form's
//! own clip path (set on the page before `/Form Do`) is not re-applied to the children; the
//! form `/BBox` clip is (it is part of every child's clip path).
//!
//! One `registry::mutate` → one undo step `undo.ungroup`.

use super::{check_generation, relist};
use crate::engine::annot::ScratchPage;
use crate::engine::raw::object::Matrix;
use crate::engine::redact::raw;
use crate::engine::registry::{self, MutateOpts};
use crate::engine::types::EngineState;
use crate::ipc::error::PdfiumResultExt;
use crate::ipc::types::{ChangeReason, DocGeneration, ObjectId, PageIndex, UngroupResult};
use crate::ipc::{EngineError, ErrorCode};
use pdfium_render::prelude::{PdfPage, PdfiumLibraryBindings, FPDF_PAGEOBJECT};

/// How the colours of a moved child are carried over (step 3 of the module docs).
fn normalise_colours(bindings: &dyn PdfiumLibraryBindings, child: FPDF_PAGEOBJECT) {
    let (mut r, mut g, mut b, mut a) = (0u32, 0u32, 0u32, 0u32);
    // SAFETY: `child` is a live, unowned page object; the out-params outlive the calls.
    unsafe {
        if bindings
            .is_true(bindings.FPDFPageObj_GetFillColor(child, &mut r, &mut g, &mut b, &mut a))
        {
            bindings.FPDFPageObj_SetFillColor(child, r, g, b, a);
        }
        if bindings
            .is_true(bindings.FPDFPageObj_GetStrokeColor(child, &mut r, &mut g, &mut b, &mut a))
        {
            bindings.FPDFPageObj_SetStrokeColor(child, r, g, b, a);
        }
    }
}

/// Moves the children of the form object at `index` of `page` onto the page, in place, and
/// removes the form. Returns the number of children (now at `index..index + n`).
///
/// The caller regenerates the page content. `lossy` = go ahead even when the form carries
/// transparency (module docs).
pub fn ungroup_at(
    bindings: &dyn PdfiumLibraryBindings,
    page: &PdfPage<'_>,
    index: usize,
    lossy: bool,
) -> Result<usize, EngineError> {
    let form = raw::object_at(bindings, page, index)?;
    if raw::object_type(bindings, form) != raw::OBJ_FORM {
        return Err(
            EngineError::invalid(format!("object {index} is not a group (Form XObject)"))
                .with_detail("notAGroup"),
        );
    }
    if !lossy && raw::has_transparency(bindings, form) {
        return Err(EngineError::new(
            ErrorCode::Unsupported,
            "this group is drawn with transparency, which its objects would lose",
        )
        .with_detail("groupTransparency"));
    }
    let m: Matrix = raw::matrix(bindings, form);
    let children = raw::form_children(bindings, form);
    for (k, &child) in children.iter().enumerate() {
        // SAFETY: `child` belongs to the live `form`; ownership passes to us.
        let ok = unsafe { bindings.FPDFFormObj_RemoveObject(form, child) };
        if !bindings.is_true(ok) {
            return Err(EngineError::new(
                ErrorCode::Pdfium,
                "FPDFFormObj_RemoveObject failed",
            ));
        }
        normalise_colours(bindings, child);
        let [a, b, c, d, e, f] = m.map(f64::from);
        // SAFETY: `child` is live and unowned; both calls take plain numbers.
        unsafe {
            bindings.FPDFPageObj_Transform(child, a, b, c, d, e, f);
            bindings.FPDFPageObj_TransformClipPath(child, a, b, c, d, e, f);
        }
        raw::insert_at(bindings, page, child, index + k)?;
    }
    let form = raw::object_at(bindings, page, index + children.len())?;
    raw::remove_and_destroy(bindings, page, form)?;
    Ok(children.len())
}

/// `ungroup_object` — the explicit 그룹 해제 (text edit on a run inside a group).
pub fn ungroup(
    st: &mut EngineState<'_>,
    doc_id: &str,
    page_index: PageIndex,
    object_id: ObjectId,
    expect_generation: DocGeneration,
) -> Result<UngroupResult, EngineError> {
    check_generation(st.doc(doc_id)?, expect_generation)?;
    st.doc(doc_id)?.geom(page_index)?;
    let moved = registry::mutate(
        st,
        doc_id,
        MutateOpts::new("undo.ungroup", ChangeReason::Edit).page(page_index),
        |doc| {
            let bindings = doc.bindings();
            let mut scratch = ScratchPage::open(doc, page_index)?;
            let count = raw::object_count(bindings, &scratch.page);
            if object_id as usize >= count {
                return Err(
                    EngineError::not_found(format!("object {object_id} of {count}"))
                        .with_page(page_index),
                );
            }
            let n = ungroup_at(bindings, &scratch.page, object_id as usize, false)?;
            scratch
                .page
                .regenerate_content()
                .ctx("regenerate page content")?;
            Ok(n)
        },
    )?;
    let list = relist(st, doc_id, page_index)?;
    Ok(UngroupResult {
        doc_generation: list.doc_generation,
        new_object_ids: (object_id..object_id + moved as ObjectId).collect(),
        objects: list.objects,
    })
}
