//! Page operations, extract, split and merge — `IPC_CONTRACT.md` §7.3, `ARCHITECTURE.md` §6.3.
//!
//! Three rules shape everything here:
//!
//! 1. **One `page_ops` call is one `registry::mutate(..).structural()`** = one generation =
//!    one undo step. `structural()` flushes the page LRU before the closure runs, which the
//!    raw `FPDF_MovePages` / `FPDF_ImportPagesByIndex` calls need because they bypass
//!    pdfium-render's `PdfPageIndexCache` (`STAGE1A_NOTES.md` §1).
//! 2. **Extract and split copy the original and delete the other pages.** `FPDF_ImportPages*`
//!    keeps the widget annotations but drops `/AcroForm`, the outline and the document
//!    metadata (pages spike §2), so a split of a form would produce a document whose fields
//!    render but cannot be filled. Deleting keeps the catalog intact.
//! 3. **Merge is the one place import is used**, because there is no original to copy — and it
//!    reports exactly what it could not carry over as `MergeWarning`s.
//!
//! Every page range the *user* types is 1-based (`"1-3,5"`); every index inside the engine is
//! 0-based. [`parse_range`] is the only converter and never mixes the two.

pub mod boxes;
// v0.3 pkg2-pages-structure-forms: images → PDF (D1) and the import carry-over (P1 / P3).
pub mod carry;
pub mod create;

use crate::engine::raw;
use crate::engine::registry::{self, MutateOpts, OpenDoc};
use crate::engine::render;
use crate::engine::types::EngineState;
use crate::ipc::error::PdfiumResultExt;
use crate::ipc::types::{
    BlankPageSize, ChangeReason, DocInfo, ExtractPagesResult, MergeInput, MergeResult,
    MergeWarning, NamedPageSize, OutlineNode, PageOp, SplitMode,
};
use crate::ipc::{EngineError, ErrorCode};
use pdfium_render::prelude::{
    PdfDocument, PdfPagePaperSize, PdfPageRenderRotation, PdfPoints, Pdfium,
};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------------------
// Range strings
// ---------------------------------------------------------------------------------------

/// Parses a **1-based** `"1-3,5,9-"` range string into ascending, deduplicated **0-based**
/// page indices.
///
/// `""` and `"all"` mean the whole document. An open-ended `"5-"` runs to the last page. Any
/// index outside `1..=page_count`, an inverted range or a non-number is `invalidArgument` —
/// silently clamping would hand the user a different document than the one they asked for.
pub fn parse_range(spec: &str, page_count: u16) -> Result<Vec<u16>, EngineError> {
    let spec = spec.trim();
    if spec.is_empty() || spec.eq_ignore_ascii_case("all") {
        return Ok((0..page_count).collect());
    }
    let mut out: BTreeSet<u16> = BTreeSet::new();
    for part in spec.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let (first, last) = match part.split_once('-') {
            Some((a, b)) => {
                let a = a.trim();
                let b = b.trim();
                let first = parse_one(a, page_count)?;
                let last = if b.is_empty() {
                    page_count
                } else {
                    parse_one(b, page_count)?
                };
                (first, last)
            }
            None => {
                let n = parse_one(part, page_count)?;
                (n, n)
            }
        };
        if first > last {
            return Err(EngineError::invalid(format!(
                "page range '{part}' runs backwards"
            )));
        }
        for p in first..=last {
            out.insert(p - 1);
        }
    }
    if out.is_empty() {
        return Err(EngineError::invalid(format!(
            "page range '{spec}' selects no page"
        )));
    }
    Ok(out.into_iter().collect())
}

fn parse_one(token: &str, page_count: u16) -> Result<u16, EngineError> {
    let n: u32 = token
        .parse()
        .map_err(|_| EngineError::invalid(format!("'{token}' is not a page number")))?;
    if n == 0 || n > page_count as u32 {
        return Err(EngineError::invalid(format!(
            "page {n} is outside 1..={page_count}"
        )));
    }
    Ok(n as u16)
}

/// Turns 0-based indices back into the 1-based string `copy_pages_from_document` wants.
fn range_string(pages: &[u16]) -> String {
    let mut parts: Vec<String> = Vec::new();
    let mut i = 0usize;
    while i < pages.len() {
        let start = pages[i];
        let mut end = start;
        i += 1;
        while i < pages.len() && pages[i] == end + 1 {
            end = pages[i];
            i += 1;
        }
        if start == end {
            parts.push((start + 1).to_string());
        } else {
            parts.push(format!("{}-{}", start + 1, end + 1));
        }
    }
    parts.join(",")
}

/// Validates a 0-based page list against the document.
fn check_pages(pages: &[u16], page_count: u16) -> Result<Vec<u16>, EngineError> {
    if pages.is_empty() {
        return Err(EngineError::invalid("no page was selected"));
    }
    let mut seen: BTreeSet<u16> = BTreeSet::new();
    for &p in pages {
        if p >= page_count {
            return Err(
                EngineError::invalid(format!("page {p} is outside 0..{page_count}")).with_page(p),
            );
        }
        seen.insert(p);
    }
    Ok(seen.into_iter().collect())
}

// ---------------------------------------------------------------------------------------
// page_ops
// ---------------------------------------------------------------------------------------

/// The i18n key the Edit menu shows for this batch.
fn undo_label(ops: &[PageOp]) -> &'static str {
    if ops.len() != 1 {
        return "undo.pageOps";
    }
    match ops[0] {
        PageOp::Move { .. } => "undo.pageMove",
        PageOp::Delete { .. } => "undo.pageDelete",
        PageOp::Rotate { .. } => "undo.pageRotate",
        PageOp::InsertBlank { .. } => "undo.pageInsert",
        PageOp::Duplicate { .. } => "undo.pageDuplicate",
        PageOp::InsertFrom { .. } => "undo.pageInsertFrom",
        PageOp::Reverse => "undo.pageReverse",
    }
}

/// `page_ops` — the whole batch in one `mutate`, so the organizer's drag is one undo step.
pub fn apply_ops<'p>(
    st: &mut EngineState<'p>,
    doc_id: &str,
    ops: Vec<PageOp>,
) -> Result<DocInfo, EngineError> {
    if ops.is_empty() {
        return Ok(st.doc(doc_id)?.info());
    }
    // v0.3 P1: 파일에서 페이지 삽입 (always a one-op batch from the UI) keeps the source's
    // outline, page labels and form fields — a byte-level rewrite, still one undo step.
    if let [PageOp::InsertFrom {
        at,
        path,
        range,
        password,
    }] = ops.as_slice()
    {
        if !st.doc(doc_id)?.encrypted {
            if let Some(info) = insert_from_carrying(
                st,
                doc_id,
                *at,
                Path::new(path),
                range.as_deref(),
                password.as_deref(),
            )? {
                return Ok(info);
            }
        }
    }
    let pdfium: &'p Pdfium = st.pdfium;
    let label = undo_label(&ops);
    registry::mutate(
        st,
        doc_id,
        MutateOpts::new(label, ChangeReason::Pages).structural(),
        |doc| {
            for op in &ops {
                apply_one(pdfium, doc, op)?;
            }
            Ok(())
        },
    )?;
    Ok(st.doc(doc_id)?.info())
}

/// The page count **right now**, from PDFium.
///
/// `OpenDoc::page_count()` reads `pages_meta`, which `registry::mutate` only refreshes after
/// the closure returns — so inside a multi-op batch it still describes the document as it was
/// before the first op. Every validation here has to see what the previous ops did.
fn live_count(doc: &OpenDoc<'_>) -> u16 {
    raw::page::page_count(doc.bindings(), doc.pdf())
}

fn apply_one<'p>(
    pdfium: &'p Pdfium,
    doc: &mut OpenDoc<'p>,
    op: &PageOp,
) -> Result<(), EngineError> {
    match op {
        PageOp::Move { pages, to } => {
            // Ordered-list semantics: the named pages are lifted out in the order given and
            // re-inserted as one block starting at `to`. Validation lives in the raw wrapper.
            raw::page::move_pages(doc.bindings(), doc.pdf(), pages, *to)
        }
        PageOp::Delete { pages } => {
            let count = live_count(doc);
            let pages = check_pages(pages, count)?;
            if pages.len() as u16 >= count {
                return Err(EngineError::invalid(
                    "a document must keep at least one page",
                ));
            }
            // Descending: FPDFPage_Delete shifts everything after the deleted index.
            for &index in pages.iter().rev() {
                let page = doc
                    .pdf()
                    .pages()
                    .get(index as i32)
                    .ctx(&format!("load page {index}"))?;
                page.delete().ctx(&format!("delete page {index}"))?;
            }
            Ok(())
        }
        PageOp::Rotate { pages, delta } => {
            let pages = check_pages(pages, live_count(doc))?;
            if !matches!(delta % 360, 0 | 90 | 180 | 270) {
                return Err(EngineError::invalid(format!(
                    "rotation delta {delta} is not a multiple of 90"
                )));
            }
            for &index in &pages {
                let mut page = doc
                    .pdf()
                    .pages()
                    .get(index as i32)
                    .ctx(&format!("load page {index}"))?;
                // `/Rotate` is a page attribute, not content: no regeneration needed, which is
                // where the 2.8 ms of the default strategy went (pages spike §1).
                page.set_content_regeneration_strategy(
                    pdfium_render::prelude::PdfPageContentRegenerationStrategy::Manual,
                );
                let current = page.rotation().map(|r| r.as_degrees() as i32).unwrap_or(0);
                let next = (current + *delta as i32).rem_euclid(360);
                page.set_rotation(rotation_from_degrees(next));
            }
            Ok(())
        }
        PageOp::InsertBlank { at, size } => {
            let count = live_count(doc);
            if *at > count {
                return Err(EngineError::invalid(format!(
                    "cannot insert at {at}: the document has {count} pages"
                )));
            }
            let paper = paper_size(doc, *at, count, size)?;
            let page = doc
                .pdf_mut()
                .pages_mut()
                .create_page_at_index(paper, *at as i32)
                .ctx("create blank page")?;
            drop(page);
            Ok(())
        }
        PageOp::Duplicate { pages } => {
            let pages = check_pages(pages, live_count(doc))?;
            let bindings = doc.bindings();
            // Descending, so the indices of the pages still to copy do not move.
            for &index in pages.iter().rev() {
                raw::page::import_pages_by_index(
                    bindings,
                    doc.pdf(),
                    doc.pdf(),
                    &[index],
                    index + 1,
                )?;
            }
            Ok(())
        }
        PageOp::InsertFrom {
            at,
            path,
            range,
            password,
        } => {
            let count = live_count(doc);
            if *at > count {
                return Err(EngineError::invalid(format!(
                    "cannot insert at {at}: the document has {count} pages"
                )));
            }
            let source = load_source(pdfium, Path::new(path), password.as_deref())?;
            let source_count = source.pages().len() as u16;
            let selected = parse_range(range.as_deref().unwrap_or(""), source_count)?;
            let spec = range_string(&selected);
            doc.pdf_mut()
                .pages_mut()
                .copy_pages_from_document(&source, &spec, *at as i32)
                .ctx(&format!("import pages {spec} from {path}"))?;
            Ok(())
        }
        PageOp::Reverse => {
            let count = live_count(doc);
            if count < 2 {
                return Ok(());
            }
            let order: Vec<u16> = (0..count).rev().collect();
            raw::page::move_pages(doc.bindings(), doc.pdf(), &order, 0)
        }
    }
}

fn rotation_from_degrees(deg: i32) -> PdfPageRenderRotation {
    match deg.rem_euclid(360) {
        90 => PdfPageRenderRotation::Degrees90,
        180 => PdfPageRenderRotation::Degrees180,
        270 => PdfPageRenderRotation::Degrees270,
        _ => PdfPageRenderRotation::None,
    }
}

fn paper_size(
    doc: &OpenDoc<'_>,
    at: u16,
    count: u16,
    size: &BlankPageSize,
) -> Result<PdfPagePaperSize, EngineError> {
    let explicit = match size {
        BlankPageSize::Explicit(s) => Some((s.width_pt, s.height_pt)),
        BlankPageSize::Named(NamedPageSize::A4) => Some((595.28, 841.89)),
        BlankPageSize::Named(NamedPageSize::Letter) => Some((612.0, 792.0)),
        BlankPageSize::Named(NamedPageSize::SameAs) => None,
    };
    let (w, h) = match explicit {
        Some(size) => size,
        None => {
            // The page the new one lands in front of, or the last page when appending. Read
            // live rather than from `pages_meta`, which is one batch behind.
            let neighbour = at.min(count.saturating_sub(1));
            let rect = doc
                .pdf()
                .pages()
                .page_size(neighbour as i32)
                .ctx("read the neighbouring page size")?;
            (rect.width().value, rect.height().value)
        }
    };
    if !(w.is_finite() && h.is_finite()) || w <= 1.0 || h <= 1.0 || w > 14400.0 || h > 14400.0 {
        return Err(EngineError::invalid(format!(
            "page size {w} x {h} pt is out of range"
        )));
    }
    Ok(PdfPagePaperSize::from_points(
        PdfPoints::new(w),
        PdfPoints::new(h),
    ))
}

fn load_source<'p>(
    pdfium: &'p Pdfium,
    path: &Path,
    password: Option<&str>,
) -> Result<PdfDocument<'p>, EngineError> {
    if !path.is_file() {
        return Err(EngineError::not_found(format!("{}", path.display())));
    }
    let bytes = std::fs::read(path).map_err(EngineError::from)?;
    pdfium
        .load_pdf_from_byte_vec(bytes, password)
        .map_err(|e| EngineError::pdfium(&format!("open {}", path.display()), e))
}

// ---------------------------------------------------------------------------------------
// v0.3 P1 / P3: imports that keep the source's structure
// ---------------------------------------------------------------------------------------

/// Imports `selected` (source indices, in this order) of `source` into `doc_id` at `at` as a
/// byte-level rewrite — PDFium imports into a scratch copy, [`carry::apply`] puts back what
/// the import leaves behind (fields, outline, labels as `carry` says; popups whenever the source
/// was parsed) — then the open document is replaced: one undo step (`label`), rolled back on any
/// failure. When the carry-over itself cannot be done (lopdf cannot parse the bytes) PDFium's
/// plain import is kept, which is what every import did before v0.3.
///
/// `Ok(None)`: nothing to carry (no fields, outline or labels in the source, no labels in the
/// destination) — the caller does PDFium's plain import in a `registry::mutate` instead, which
/// costs no serialisation. Refuses an encrypted destination (the rewrite cannot re-encrypt);
/// callers check first.
#[allow(clippy::too_many_arguments)] // one private helper, two callers; a struct would only rename them
fn import_carrying<'p>(
    st: &mut EngineState<'p>,
    doc_id: &str,
    source: &PdfDocument<'p>,
    name: &str,
    selected: Vec<u16>,
    at: u16,
    carry_what: carry::Carry,
    label: &'static str,
) -> Result<Option<DocInfo>, EngineError> {
    let count = st.doc(doc_id)?.page_count();
    if at > count {
        return Err(EngineError::invalid(format!(
            "cannot insert at {at}: the document has {count} pages"
        )));
    }
    if selected.is_empty() {
        return Err(EngineError::invalid("no page was selected"));
    }
    let expected = u16::try_from(count as usize + selected.len()).map_err(|_| {
        EngineError::new(
            ErrorCode::Unsupported,
            format!(
                "the document would have more than {} pages",
                registry::MAX_PAGES
            ),
        )
    })?;
    let pdfium = st.pdfium;
    let bindings = raw::bindings(pdfium);
    let src = carry::read_source(bindings, source, name, selected.clone(), carry_what);
    // Nothing to carry and no labels of our own to keep in step: PDFium's plain import is
    // enough, and much cheaper than a byte-level rewrite — the caller takes that path.
    if !src.carries_anything() && st.doc(doc_id)?.page_labels().is_none() {
        return Ok(None);
    }
    registry::mutate_bytes_resized(
        st,
        doc_id,
        MutateOpts::new(label, ChangeReason::Pages).structural(),
        expected,
        move |bytes, doc| {
            let scratch = pdfium
                .load_pdf_from_byte_vec(bytes.to_vec(), doc.password.as_deref())
                .map_err(|e| EngineError::pdfium("reload for import", e))?;
            raw::page::import_pages_by_index(bindings, &scratch, source, &selected, at)?;
            let imported =
                raw::save::save_as_copy(bindings, &scratch, raw::save::SaveFlags::NoIncremental)?;
            drop(scratch);
            let placed = [carry::Placed { source: src, at }];
            match carry::apply(&imported, &placed, carry::OutlineMode::OnlyOutlined) {
                Ok(rewritten) => Ok(rewritten),
                Err(e) => {
                    tracing::warn!(error = %e, "import carry-over skipped");
                    Ok(imported)
                }
            }
        },
        |_, _| Ok(()),
    )
    .map(Some)
}

/// `page_ops [insertFrom]` on an unencrypted document (v0.3 P1): the source's outline (under a
/// node named after the file), page labels and form fields come along.
fn insert_from_carrying(
    st: &mut EngineState<'_>,
    doc_id: &str,
    at: u16,
    path: &Path,
    range: Option<&str>,
    password: Option<&str>,
) -> Result<Option<DocInfo>, EngineError> {
    let pdfium = st.pdfium;
    let source = load_source(pdfium, path, password)?;
    let selected = parse_range(range.unwrap_or(""), source.pages().len() as u16)?;
    let name = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "Untitled".to_string());
    let done = import_carrying(
        st,
        doc_id,
        &source,
        &name,
        selected,
        at,
        carry::Carry::ALL,
        "undo.pageInsertFrom",
    )?;
    Ok(match done {
        Some(_) => Some(st.doc(doc_id)?.info()),
        None => None,
    })
}

/// `import_pages_from_doc` (v0.3 P3): pages of one open document copied into another at `at`
/// — a drag between windows. `pages` are source indices in the order they land (duplicates
/// dropped); the source's unsaved edits are included (it is serialised first). One undo step
/// `undo.pageImport` on the target; the source is untouched. The imported widgets keep their
/// fields (renamed on a clash) and popups their parent.
pub fn import_pages_from_doc(
    st: &mut EngineState<'_>,
    src_doc_id: &str,
    pages: &[u16],
    dst_doc_id: &str,
    at: u16,
) -> Result<DocInfo, EngineError> {
    if src_doc_id == dst_doc_id {
        return Err(EngineError::invalid(
            "source and target are the same document: use page_ops move",
        ));
    }
    st.doc(dst_doc_id)?;
    let src_count = st.doc(src_doc_id)?.page_count();
    let mut seen: BTreeSet<u16> = BTreeSet::new();
    let mut ordered: Vec<u16> = Vec::with_capacity(pages.len());
    for &p in pages {
        if p >= src_count {
            return Err(
                EngineError::invalid(format!("page {p} is outside 0..{src_count}")).with_page(p),
            );
        }
        if seen.insert(p) {
            ordered.push(p);
        }
    }
    if ordered.is_empty() {
        return Err(EngineError::invalid("no page was selected"));
    }
    render::tiles::generate_appearances(st, src_doc_id)?;
    let (bytes, password, name) = {
        let src = st.doc(src_doc_id)?;
        (src.to_bytes()?, src.password.clone(), src.name_stem())
    };
    let scratch = st
        .pdfium
        .load_pdf_from_byte_vec(bytes.to_vec(), password.as_deref())
        .map_err(|e| EngineError::pdfium("reopen the source", e))?;
    if !st.doc(dst_doc_id)?.encrypted {
        if let Some(info) = import_carrying(
            st,
            dst_doc_id,
            &scratch,
            &name,
            ordered.clone(),
            at,
            carry::Carry::PAGES_ONLY,
            "undo.pageImport",
        )? {
            return Ok(info);
        }
    }
    // An encrypted target, or nothing to carry: PDFium's plain import.
    let count = st.doc(dst_doc_id)?.page_count();
    if at > count {
        return Err(EngineError::invalid(format!(
            "cannot insert at {at}: the document has {count} pages"
        )));
    }
    registry::mutate(
        st,
        dst_doc_id,
        MutateOpts::new("undo.pageImport", ChangeReason::Pages).structural(),
        |doc| raw::page::import_pages_by_index(doc.bindings(), doc.pdf(), &scratch, &ordered, at),
    )?;
    Ok(st.doc(dst_doc_id)?.info())
}

// ---------------------------------------------------------------------------------------
// extract / split — copy the original, delete the rest
// ---------------------------------------------------------------------------------------

/// Serialises the document with only `keep` (0-based, ascending) still in it.
///
/// This is the "copy and delete" strategy: a scratch `PdfDocument` is loaded from the current
/// bytes (so unsaved edits are included), the unwanted pages are deleted from it descending
/// and the result is serialised. `/AcroForm`, the outline, `/PageLabels` and the metadata all
/// survive, which is exactly what `FPDF_ImportPages*` would have destroyed.
pub fn subset_bytes(
    st: &mut EngineState<'_>,
    doc_id: &str,
    keep: &[u16],
) -> Result<Vec<u8>, EngineError> {
    // Markup annotations without an `/AP` are invisible in other viewers; the pre-flight pass
    // is the same one `save` runs.
    render::tiles::generate_appearances(st, doc_id)?;
    let doc = st.doc(doc_id)?;
    let keep = check_pages(keep, doc.page_count())?;
    let password = doc.password.clone();
    let source = doc.to_bytes()?;
    let bindings = doc.bindings();

    let scratch = st
        .pdfium
        .load_pdf_from_byte_vec(source.to_vec(), password.as_deref())
        .map_err(|e| EngineError::pdfium("reopen for extract", e))?;
    let total = raw::page::page_count(bindings, &scratch);
    let keep_set: BTreeSet<u16> = keep.iter().copied().collect();
    for index in (0..total).rev() {
        if keep_set.contains(&index) {
            continue;
        }
        let page = scratch
            .pages()
            .get(index as i32)
            .ctx(&format!("load page {index}"))?;
        page.delete().ctx(&format!("delete page {index}"))?;
    }
    raw::save::save_as_copy(bindings, &scratch, raw::save::SaveFlags::NoIncremental)
}

/// `extract_pages` (§7.3).
pub fn extract(
    st: &mut EngineState<'_>,
    doc_id: &str,
    pages: &[u16],
    out_path: &str,
    remove_after: bool,
) -> Result<ExtractPagesResult, EngineError> {
    let bytes = subset_bytes(st, doc_id, pages)?;
    let written = write_atomic(Path::new(out_path), &bytes)?;
    if remove_after {
        apply_ops(
            st,
            doc_id,
            vec![PageOp::Delete {
                pages: pages.to_vec(),
            }],
        )?;
    }
    Ok(ExtractPagesResult {
        bytes: written,
        doc_generation: st.doc(doc_id)?.generation,
    })
}

/// One output file of a split.
#[derive(Debug, Clone)]
pub struct SplitPart {
    pub pages: Vec<u16>,
    /// File name, without a directory.
    pub name: String,
}

/// Turns a [`SplitMode`] into the list of files to write. Pure; unit-tested.
pub fn split_plan(
    mode: &SplitMode,
    page_count: u16,
    stem: &str,
) -> Result<Vec<SplitPart>, EngineError> {
    split_plan_with_outline(mode, page_count, stem, &[])
}

/// [`split_plan`] with the document's outline, which `SplitMode::ByOutline` (v0.3 P4) needs.
pub fn split_plan_with_outline(
    mode: &SplitMode,
    page_count: u16,
    stem: &str,
    outline: &[OutlineNode],
) -> Result<Vec<SplitPart>, EngineError> {
    let mut parts: Vec<SplitPart> = Vec::new();
    match mode {
        SplitMode::ByOutline { by_outline } => {
            parts = outline_parts(outline, by_outline.level, page_count, stem)?;
        }
        SplitMode::EveryN { every_n } => {
            if *every_n == 0 {
                return Err(EngineError::invalid("everyN must be at least 1"));
            }
            let step = (*every_n).min(u16::MAX as u32) as u16;
            let mut first = 0u16;
            while first < page_count {
                let last = (first as u32 + step as u32 - 1).min(page_count as u32 - 1) as u16;
                parts.push(SplitPart {
                    pages: (first..=last).collect(),
                    name: format!("{stem}-{:03}-{:03}.pdf", first + 1, last + 1),
                });
                first = last + 1;
            }
        }
        SplitMode::Ranges { ranges } => {
            if ranges.is_empty() {
                return Err(EngineError::invalid("no range was given"));
            }
            for (i, spec) in ranges.iter().enumerate() {
                let pages = parse_range(spec, page_count)?;
                parts.push(SplitPart {
                    pages,
                    name: format!("{stem}-{:02}.pdf", i + 1),
                });
            }
        }
    }
    if parts.is_empty() {
        return Err(EngineError::invalid("the split produced no file"));
    }
    Ok(parts)
}

/// v0.3 P4 — 책갈피로 분할: one part per outline node at `level` (1 = top level) that points at
/// a page, from its page to the page before the next such node (the last runs to the end).
/// Pages before the first node form a part of their own, named after the document. Nodes on
/// the same page as an earlier one fold into it. Files are `NN 제목.pdf`, the title made safe
/// for every file system (`\ / : * ? " < > |` and control characters become `_`, at most 80
/// characters), numbered so they sort in reading order.
fn outline_parts(
    outline: &[OutlineNode],
    level: u8,
    page_count: u16,
    stem: &str,
) -> Result<Vec<SplitPart>, EngineError> {
    if level == 0 {
        return Err(EngineError::invalid("the outline level starts at 1"));
    }
    fn collect<'a>(nodes: &'a [OutlineNode], depth: u8, level: u8, out: &mut Vec<&'a OutlineNode>) {
        for n in nodes {
            if depth == level {
                out.push(n);
            } else {
                collect(&n.children, depth + 1, level, out);
            }
        }
    }
    let mut nodes: Vec<&OutlineNode> = Vec::new();
    collect(outline, 1, level, &mut nodes);
    let mut starts: Vec<(u16, &str)> = Vec::new();
    for n in nodes {
        let Some(page) = n.page.filter(|p| *p < page_count) else {
            continue;
        };
        if !starts.iter().any(|(p, _)| *p == page) {
            starts.push((page, n.title.as_str()));
        }
    }
    starts.sort_by_key(|(p, _)| *p);
    if starts.is_empty() {
        return Err(EngineError::invalid(format!(
            "the outline has no bookmark with a page at level {level}"
        )));
    }
    let mut parts: Vec<(u16, u16, String)> = Vec::new();
    if starts[0].0 > 0 {
        parts.push((0, starts[0].0 - 1, stem.to_string()));
    }
    for (i, (first, title)) in starts.iter().enumerate() {
        let last = starts
            .get(i + 1)
            .map(|(next, _)| next - 1)
            .unwrap_or(page_count - 1);
        parts.push((*first, last, title.to_string()));
    }
    let width = if parts.len() >= 100 { 3 } else { 2 };
    let mut used: BTreeSet<String> = BTreeSet::new();
    Ok(parts
        .into_iter()
        .enumerate()
        .map(|(i, (first, last, title))| {
            let mut name = format!("{:0width$} {}", i + 1, safe_file_title(&title, stem));
            let base = name.clone();
            let mut n = 2;
            while !used.insert(name.to_lowercase()) {
                name = format!("{base} ({n})");
                n += 1;
            }
            SplitPart {
                pages: (first..=last).collect(),
                name: format!("{name}.pdf"),
            }
        })
        .collect())
}

/// A bookmark title as a file name: path and reserved characters out, trimmed, ≤ 80 chars.
pub fn safe_file_title(title: &str, fallback: &str) -> String {
    let cleaned: String = title
        .chars()
        .map(|c| {
            if c.is_control() || matches!(c, '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|') {
                '_'
            } else {
                c
            }
        })
        .collect();
    let trimmed: String = cleaned
        .trim()
        .trim_end_matches('.')
        .chars()
        .take(80)
        .collect();
    let trimmed = trimmed.trim().to_string();
    if trimmed.is_empty() {
        fallback.to_string()
    } else {
        trimmed
    }
}

/// Writes one part of a split. Called once per part on `Lane::Background` so tiles interleave.
pub fn write_split_part(
    st: &mut EngineState<'_>,
    doc_id: &str,
    part: &SplitPart,
    out_dir: &Path,
) -> Result<PathBuf, EngineError> {
    let bytes = subset_bytes(st, doc_id, &part.pages)?;
    std::fs::create_dir_all(out_dir).map_err(EngineError::from)?;
    let path = out_dir.join(&part.name);
    write_atomic(&path, &bytes)?;
    Ok(path)
}

/// The file stem a split's outputs are named after.
pub fn output_stem(doc: &OpenDoc<'_>) -> String {
    doc.path
        .as_ref()
        .and_then(|p| p.file_stem())
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "Untitled".to_string())
}

// ---------------------------------------------------------------------------------------
// merge
// ---------------------------------------------------------------------------------------

/// `merge_documents` — the **only** importer in the engine.
///
/// There is no original to copy pages out of, so `FPDF_ImportPages` is unavoidable and the
/// losses it causes are reported rather than hidden: a source with an AcroForm yields
/// `formsDropped`, one with bookmarks `outlineDropped`, one with `/Info` entries
/// `metadataDropped`. The merged document is opened untitled (`path: None`) and dirty, so the
/// first ⌘S goes through Save As and nothing discards it unasked.
pub fn merge(st: &mut EngineState<'_>, inputs: &[MergeInput]) -> Result<MergeResult, EngineError> {
    if inputs.is_empty() {
        return Err(EngineError::invalid("merge needs at least one input"));
    }
    let pdfium = st.pdfium;
    // `MergeWarning` is a frozen contract type without `Ord`, so this is a small ordered
    // `Vec` rather than a set.
    let mut warnings: Vec<MergeWarning> = Vec::new();
    let warn = |w: MergeWarning, warnings: &mut Vec<MergeWarning>| {
        if !warnings.contains(&w) {
            warnings.push(w);
        }
    };
    // v0.3 P1: what PDFium's import leaves behind is put back by `carry::apply`; these are
    // only reported when that fails.
    let mut lost: Vec<MergeWarning> = Vec::new();
    let mut placed: Vec<carry::Placed> = Vec::new();
    let bytes = {
        let mut out = pdfium
            .create_new_pdf()
            .map_err(|e| EngineError::pdfium("create merged document", e))?;
        let mut at: u16 = 0;
        for input in inputs {
            let source = load_source(pdfium, Path::new(&input.path), input.password.as_deref())?;
            let source_count = source.pages().len() as u16;
            if source_count == 0 {
                return Err(EngineError::invalid(format!("{} has no pages", input.path)));
            }
            let selected = parse_range(input.range.as_deref().unwrap_or(""), source_count)?;
            if source.form().is_some() {
                warn(MergeWarning::FormsDropped, &mut lost);
            }
            if source.bookmarks().root().is_some() {
                warn(MergeWarning::OutlineDropped, &mut lost);
            }
            if !source.metadata().is_empty() {
                warn(MergeWarning::MetadataDropped, &mut warnings);
            }
            let stem = Path::new(&input.path)
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| format!("{}", placed.len() + 1));
            placed.push(carry::Placed {
                source: carry::read_source(
                    raw::bindings(pdfium),
                    &source,
                    &stem,
                    selected.clone(),
                    carry::Carry::ALL,
                ),
                at,
            });
            let spec = range_string(&selected);
            // Page indices are u16: a merge past 65,535 pages is refused, never wrapped.
            let next = u16::try_from(at as usize + selected.len()).map_err(|_| {
                EngineError::new(
                    crate::ipc::ErrorCode::Unsupported,
                    format!(
                        "the merge would have more than {} pages",
                        registry::MAX_PAGES
                    ),
                )
            })?;
            out.pages_mut()
                .copy_pages_from_document(&source, &spec, at as i32)
                .ctx(&format!("merge {} pages {spec}", input.path))?;
            at = next;
        }
        if at == 0 {
            return Err(EngineError::invalid("the merge selected no page"));
        }
        raw::save::save_as_copy(
            raw::bindings(pdfium),
            &out,
            raw::save::SaveFlags::NoIncremental,
        )?
    };
    let carried = carry::apply(&bytes, &placed, carry::OutlineMode::PerSource)
        .map_err(|e| tracing::warn!(error = %e, "merge carry-over skipped"))
        .ok();
    let info = match carried.map(|b| registry::open(st, None, b, None)) {
        Some(Ok(info)) => info,
        _ => {
            for w in lost {
                warn(w, &mut warnings);
            }
            registry::open(st, None, bytes, None)?
        }
    };
    // The merge has never been saved: it opens **dirty**, so closing the window, quitting or
    // opening another file asks first, and autosave keeps a recovery copy of it.
    let doc = st.doc_mut(&info.doc_id)?;
    doc.saved_generation = 0;
    let info = doc.info();
    Ok(MergeResult { info, warnings })
}

// ---------------------------------------------------------------------------------------
// Shared file writing
// ---------------------------------------------------------------------------------------

/// Writes `bytes` to `path` through a temp file in the same directory, fsynced and renamed.
///
/// Same guarantee as `engine::save`: a crash mid-write leaves either the old file or nothing,
/// never a half-written PDF. Returns the number of bytes written.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<u64, EngineError> {
    use std::io::Write;

    let dir = path.parent().unwrap_or(Path::new("."));
    if !dir.exists() {
        std::fs::create_dir_all(dir).map_err(EngineError::from)?;
    }
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .ok_or_else(|| EngineError::invalid(format!("{} is not a file path", path.display())))?;
    let tmp = dir.join(format!(".{name}.seepdf-{}.tmp", std::process::id()));
    let result = (|| -> Result<(), EngineError> {
        let mut file = std::fs::File::create(&tmp).map_err(EngineError::from)?;
        file.write_all(bytes).map_err(EngineError::from)?;
        file.sync_all().map_err(EngineError::from)?;
        drop(file);
        rename_with_retry(&tmp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result?;
    Ok(bytes.len() as u64)
}

/// `fs::rename` with the Windows sharing-violation retry (`ARCHITECTURE.md` §8).
pub fn rename_with_retry(from: &Path, to: &Path) -> Result<(), EngineError> {
    let mut last = None;
    for attempt in 0..5 {
        match std::fs::rename(from, to) {
            Ok(()) => return Ok(()),
            Err(e) => {
                last = Some(e);
                if attempt < 4 {
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
            }
        }
    }
    let e = last.expect("at least one attempt");
    let code = match e.kind() {
        std::io::ErrorKind::PermissionDenied | std::io::ErrorKind::ReadOnlyFilesystem => {
            ErrorCode::ReadOnly
        }
        _ => ErrorCode::Io,
    };
    Err(EngineError::new(
        code,
        format!("rename to {}: {e}", to.display()),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn range_parsing_is_one_based() {
        assert_eq!(parse_range("1-3,5", 10).unwrap(), vec![0, 1, 2, 4]);
        assert_eq!(parse_range("2", 10).unwrap(), vec![1]);
        assert_eq!(parse_range(" 5 - ", 7).unwrap(), vec![4, 5, 6]);
        assert_eq!(parse_range("", 3).unwrap(), vec![0, 1, 2]);
        assert_eq!(parse_range("3,1,2", 3).unwrap(), vec![0, 1, 2]);
        assert!(parse_range("0", 3).is_err());
        assert!(parse_range("4", 3).is_err());
        assert!(parse_range("3-1", 3).is_err());
        assert!(parse_range("a", 3).is_err());
    }

    #[test]
    fn range_string_round_trips() {
        assert_eq!(range_string(&[0, 1, 2, 4]), "1-3,5");
        assert_eq!(range_string(&[3]), "4");
        assert_eq!(range_string(&[]), "");
    }

    #[test]
    fn split_plans() {
        let parts = split_plan(&SplitMode::EveryN { every_n: 5 }, 12, "doc").unwrap();
        assert_eq!(parts.len(), 3);
        assert_eq!(parts[0].pages, (0..5).collect::<Vec<_>>());
        assert_eq!(parts[2].pages, vec![10, 11]);
        assert_eq!(parts[2].name, "doc-011-012.pdf");

        let parts = split_plan(
            &SplitMode::Ranges {
                ranges: vec!["1-2".into(), "4".into()],
            },
            6,
            "doc",
        )
        .unwrap();
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[1].pages, vec![3]);
        assert!(split_plan(&SplitMode::EveryN { every_n: 0 }, 6, "d").is_err());
    }
}
