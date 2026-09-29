//! Redaction — `IPC_CONTRACT.md` §7.5, `ARCHITECTURE.md` §6.5.
//!
//! **The content is removed, not covered.** A black rectangle over text is not a redaction:
//! the characters are still in the content stream and every copy/paste, text extraction and
//! search finds them. So [`apply`] deletes the content and only then draws the box.
//!
//! What PDFium can and cannot do here (annotations spike §3.5):
//!
//! * `FPDFPage_CreateAnnot(FPDF_ANNOT_REDACT)` returns NULL — redaction annotations cannot be
//!   created, so there is no "mark now, apply later" object. The marks live in the UI.
//! * **Text** (v0.3, R2 — [`split`]): a text object the marks cover only partly is *split*: the
//!   marked characters go and the rest is re-emitted at the same positions ("Gal" no longer
//!   takes "Andreas" with it). Type3 fonts, fonts without a usable `/ToUnicode`, vertical runs
//!   and runs whose re-emission fails its check fall back to whole-run removal, which
//!   [`preview`] reports as `collateral` *before* the destructive step.
//! * **Images** (v0.3, R1 — [`image`]): an image entirely inside a mark is removed; a partly
//!   covered one has the pixels under the marks set to black in its own bitmap (scans), unless
//!   it cannot be re-encoded faithfully (transparency, palette, stencil), in which case it is
//!   removed whole.
//! * **Paths**: removed when entirely inside a mark; a partly covered path is removed when it
//!   is small (at least a quarter of its box under the marks, or no side longer than
//!   [`SMALL_PATH_PT`]) or has curves (outlined glyphs, signatures, logos). A large path of
//!   straight segments only — a table grid, a frame, a background box — stays under the fill
//!   box: PDFium has no per-object clip API to cut it, and a rule carries no content.
//! * **Groups** (v0.3, R4): content inside a Form XObject cannot be deleted through PDFium.
//!   [`preview`] lists such groups; with `RedactOptions.ungroup` they are ungrouped first
//!   (`objects::ungroup`, same undo step), otherwise the apply refuses with `verifyFailed`.
//!
//! Finally the page text is re-extracted and every marked string is counted again. If one
//! survives, the whole command fails with `verifyFailed` and `registry::mutate` rolls the
//! document back to the snapshot — a fake redaction is never written to disk.

pub mod image;
// Same rules and the same allowance as `engine::raw`: the wrappers take PDFium handles as plain
// values, always from a live page on the engine thread.
#[allow(clippy::not_unsafe_ptr_arg_deref)]
pub mod raw;
pub mod split;

use crate::engine::annot::{self, ScratchPage};
use crate::engine::objects::ungroup;
use crate::engine::registry::{self, MutateOpts, OpenDoc};
use crate::engine::text;
use crate::engine::types::EngineState;
use crate::ipc::error::PdfiumResultExt;
use crate::ipc::types::{
    ChangeReason, PageIndex, Rect, RedactBatchMark, RedactBatchResult, RedactImageObject,
    RedactOptions, RedactPreview, RedactTextObject,
};
use crate::ipc::{EngineError, ErrorCode};
use pdfium_render::prelude::{
    PdfColor, PdfPageContentRegenerationStrategy, PdfPageIndex, PdfPageObjectCommon,
    PdfPageObjectType, PdfPageObjectsCommon, PdfPoints, PdfRect, FPDF_PAGEOBJECT,
};
use raw::{HandleKey, RawChar};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

/// A partly covered path whose box has no side longer than this is "small" (removed).
pub const SMALL_PATH_PT: f32 = 36.0;
/// How deep nested groups are ungrouped for a redaction.
const MAX_GROUP_DEPTH: usize = 8;

/// What happens to one page object the marks reach.
#[derive(Debug, Clone, PartialEq)]
enum Action {
    /// Deleted whole.
    Remove,
    /// Text: the unmarked runs are re-emitted, the marked characters go.
    Split(Vec<split::Run>),
    /// Image: the pixels under the marks are blanked. `true` = a JPEG original.
    Blank(bool),
    /// Left alone under the fill box (a large straight-segment path).
    Keep,
}

/// One page object that a set of marks hits.
#[derive(Debug, Clone)]
struct Victim {
    index: u32,
    kind: PdfPageObjectType,
    rect: Rect,
    /// Text of the whole object (text objects only).
    text: String,
    /// The part that will be lost although it is outside every mark.
    collateral: String,
    fully_inside: bool,
    action: Action,
}

/// Everything the marks reach on a page, read without changing anything.
struct Analysis {
    victims: Vec<Victim>,
    /// Top-level Form XObjects holding marked text or an image under a mark.
    groups: BTreeSet<u32>,
    /// …of which these hold marked text (the others only images).
    groups_with_text: BTreeSet<u32>,
    /// The page's characters, with the object that drew each.
    chars: Vec<RawChar>,
    /// Top-level handle → index.
    index_of: HashMap<HandleKey, u32>,
    /// Handle of anything inside a group → the top-level group's index.
    in_group: HashMap<HandleKey, u32>,
}

/// `redact_preview` — what [`apply`] would remove, before anything is removed.
pub fn preview(
    doc: &mut OpenDoc<'_>,
    page_index: PageIndex,
    rects: &[Rect],
) -> Result<RedactPreview, EngineError> {
    if rects.is_empty() {
        return Err(EngineError::invalid("redaction needs at least one rect"));
    }
    let analysis = analyze(doc, page_index, rects, &HashSet::new())?;
    let annots = annot::list(doc, page_index)?;

    let mut text_objects = Vec::new();
    let mut image_objects = Vec::new();
    let mut collateral = Vec::new();
    for v in &analysis.victims {
        match (v.kind, &v.action) {
            (PdfPageObjectType::Text, Action::Remove | Action::Split(_)) => {
                if !v.collateral.trim().is_empty() {
                    collateral.push(v.collateral.trim().to_string());
                }
                text_objects.push(RedactTextObject {
                    object_id: v.index,
                    text: v.text.clone(),
                    rect: v.rect,
                    fully_inside: v.fully_inside,
                    split: matches!(v.action, Action::Split(_)),
                });
            }
            (PdfPageObjectType::Image, Action::Remove | Action::Blank(_)) => {
                image_objects.push(RedactImageObject {
                    object_id: v.index,
                    rect: v.rect,
                    fully_inside: v.fully_inside,
                    blank: matches!(v.action, Action::Blank(_)),
                })
            }
            _ => {}
        }
    }

    Ok(RedactPreview {
        page: page_index,
        text_objects,
        image_objects,
        annotations: annots
            .iter()
            .filter(|a| rects.iter().any(|r| r.intersects(&a.rect)))
            .map(|a| a.id.clone())
            .collect(),
        // A widget under a mark means the value would survive in the AcroForm dictionary,
        // which PDFium gives us no way to rewrite — the apply refuses instead of lying.
        form_fields: annots
            .iter()
            .filter(|a| a.subtype == "Widget" && rects.iter().any(|r| r.intersects(&a.rect)))
            .map(|a| {
                if a.contents.is_empty() {
                    a.id.clone()
                } else {
                    a.contents.clone()
                }
            })
            .collect(),
        collateral,
        groups: analysis.groups.iter().copied().collect(),
    })
}

/// What one page's apply did.
#[derive(Debug, Default)]
struct PageOutcome {
    /// Objects removed, split or blanked.
    changed: u32,
    /// Images blanked (see the reload in [`apply_batch`]).
    blanked: u32,
    /// Text removed although outside the marks (whole-run fallbacks).
    collateral: Vec<String>,
}

/// `apply_redactions` for one page inside an open mutation — see [`apply_verified`] for the
/// entry point with the rollback. Returns the number of page objects removed, split or
/// blanked.
pub fn apply(
    doc: &mut OpenDoc<'_>,
    page_index: PageIndex,
    rects: &[Rect],
    options: &RedactOptions,
) -> Result<u32, EngineError> {
    apply_page(doc, page_index, rects, options, &HashSet::new()).map(|o| o.changed)
}

/// The destructive step, verified **before and after**.
///
/// The order matters. `registry::mutate` does not undo a closure that fails half-way by
/// itself — it reloads its snapshot — but everything that can make the redaction impossible
/// is still decided **first**, on an untouched page:
///
/// 1. a form field under a mark → `unsupported` (its value lives in the AcroForm dictionary,
///    which PDFium cannot rewrite);
/// 2. groups under the marks are ungrouped when `options.ungroup` is set;
/// 3. a marked run whose characters belong to no removable page object — text inside a group
///    that was not ungrouped, for instance — → `verifyFailed`, because covering it with a box
///    would be a fake redaction;
/// 4. only then are images blanked, text objects split, objects removed, the boxes drawn and
///    the content stream regenerated;
/// 5. the page is re-parsed: every split run must still be there (else `splitFailed`, and the
///    caller retries with those objects removed whole), and the page text is counted again —
///    a marked string that survived is `verifyFailed`.
fn apply_page(
    doc: &mut OpenDoc<'_>,
    page_index: PageIndex,
    rects: &[Rect],
    options: &RedactOptions,
    forced_whole: &HashSet<u32>,
) -> Result<PageOutcome, EngineError> {
    if rects.is_empty() {
        return Err(EngineError::invalid("redaction needs at least one rect"));
    }
    let annots = annot::list(doc, page_index)?;
    let fields = annots
        .iter()
        .filter(|a| a.subtype == "Widget" && rects.iter().any(|r| r.intersects(&a.rect)))
        .count();
    if fields > 0 {
        return Err(EngineError::new(
            ErrorCode::Unsupported,
            format!("{fields} form field(s) are under the marks; flatten the form first"),
        )
        .with_page(page_index));
    }
    let annotations: Vec<String> = annots
        .iter()
        .filter(|a| rects.iter().any(|r| r.intersects(&a.rect)))
        .map(|a| a.id.clone())
        .collect();

    // 2. groups.
    let mut analysis = analyze(doc, page_index, rects, forced_whole)?;
    if options.ungroup {
        let mut depth = 0;
        while !analysis.groups.is_empty() && depth < MAX_GROUP_DEPTH {
            ungroup_all(doc, page_index, &analysis.groups)?;
            analysis = analyze(doc, page_index, rects, forced_whole)?;
            depth += 1;
        }
    }

    // 3. pre-flight: what the marks cover, and whether each run can actually be deleted.
    let runs = marked_runs(&analysis, rects);
    if let Some(stuck) = runs.iter().find(|r| !r.removable) {
        return Err(EngineError::new(
            ErrorCode::VerifyFailed,
            format!(
                "{:?} cannot be removed — it belongs to no deletable page object (text inside a \
                 Form XObject, for example), so the document was left untouched",
                stuck.text
            ),
        )
        .with_page(page_index));
    }
    let image_groups: Vec<u32> = analysis
        .groups
        .difference(&analysis.groups_with_text)
        .copied()
        .collect();
    if !image_groups.is_empty() {
        return Err(EngineError::new(
            ErrorCode::VerifyFailed,
            format!(
                "an image inside {} group(s) (Form XObject) is under the marks; ungroup to \
                 redact it — the document was left untouched",
                image_groups.len()
            ),
        )
        .with_page(page_index)
        .with_detail("groups"));
    }
    let marked: Vec<String> = runs.into_iter().map(|r| r.text).collect();
    let text_before = text::layer::page_text(doc, page_index)?.text.clone();

    // --- from here on the document is being changed -------------------------------------
    if !annotations.is_empty() {
        annot::delete(doc, page_index, &annotations)?;
    }
    let mut outcome = PageOutcome::default();
    let bindings = doc.bindings();
    let mut split_done: Vec<(u32, Vec<split::Run>)> = Vec::new();
    {
        let mut scratch = ScratchPage::open(doc, page_index)?;
        let page = &scratch.page;

        // 4a. images first, while every index is still the listed one.
        let mut remove: Vec<FPDF_PAGEOBJECT> = Vec::new();
        for v in &analysis.victims {
            if let Action::Blank(jpeg) = v.action {
                match image::blank(bindings, doc.pdf(), page, v.index as usize, rects, jpeg)? {
                    image::Blanked::Done => {
                        outcome.changed += 1;
                        outcome.blanked += 1;
                    }
                    image::Blanked::Nothing => {}
                    image::Blanked::Unfaithful => {
                        remove.push(raw::object_at(bindings, page, v.index as usize)?)
                    }
                }
            }
        }
        // 4b. handles of everything removed whole (handles survive the insertions below).
        for v in &analysis.victims {
            if v.action == Action::Remove {
                remove.push(raw::object_at(bindings, page, v.index as usize)?);
                // The preview named every other whole-run removal already; these are runs it
                // promised to split that failed the check on a first attempt.
                if forced_whole.contains(&v.index) && !v.collateral.trim().is_empty() {
                    outcome.collateral.push(v.collateral.trim().to_string());
                }
            }
        }
        // 4c. splits, highest index first: the copies go right after their original, which
        // only shifts objects already handled.
        let mut splits: Vec<PendingSplit> = Vec::new();
        for v in analysis.victims.iter().rev() {
            let Action::Split(runs) = &v.action else {
                continue;
            };
            let index = v.index as usize;
            let original = raw::object_at(bindings, page, index)?;
            let mut copies = Vec::new();
            let mut ok = true;
            for k in 1..runs.len() {
                // A throw-away second parse per copy: moving an object out of it takes the
                // clip path, colours and marks along (see `raw::transplant_at`).
                let mut source = doc
                    .pdf()
                    .pages()
                    .get(page_index as PdfPageIndex)
                    .ctx(&format!("load page {page_index}"))?;
                source
                    .set_content_regeneration_strategy(PdfPageContentRegenerationStrategy::Manual);
                match raw::transplant_at(bindings, &source, index, page, index + k) {
                    Ok(h) => copies.push(h),
                    Err(_) => {
                        ok = false;
                        break;
                    }
                }
            }
            if ok {
                for (k, run) in runs.iter().enumerate() {
                    if split::rewrite(bindings, page, index + k, run).is_err() {
                        ok = false;
                        break;
                    }
                }
            }
            if ok {
                splits.push(PendingSplit {
                    index: v.index,
                    original,
                    copies,
                    runs: runs.clone(),
                    whole: v.collateral_if_whole(),
                });
            } else {
                remove.push(original);
                remove.extend(copies);
                outcome.collateral.push(v.collateral_if_whole());
            }
        }
        // 4d. every run reads back in place, or its object goes whole after all.
        if !splits.is_empty() {
            let chars = split::page_chars(bindings, page)?;
            for p in splits {
                let handles: Vec<HandleKey> = std::iter::once(p.original)
                    .chain(p.copies.iter().copied())
                    .map(raw::key)
                    .collect();
                let back = p
                    .runs
                    .iter()
                    .zip(&handles)
                    .all(|(run, &h)| split::reads_back(&chars, h, run));
                if back {
                    outcome.changed += 1;
                    split_done.push((p.index, p.runs));
                } else {
                    remove.push(p.original);
                    remove.extend(p.copies);
                    outcome.collateral.push(p.whole);
                }
            }
        }
        // 4e. removal by handle.
        for handle in remove {
            raw::remove_and_destroy(bindings, page, handle)?;
            outcome.changed += 1;
        }
        // 4f. the fill boxes, one per mark.
        let fill = PdfColor::new(options.fill[0], options.fill[1], options.fill[2], 255);
        for r in rects {
            scratch
                .page
                .objects_mut()
                .create_path_object_rect(to_pdf_rect(*r), None, None, Some(fill))
                .ctx("draw the redaction box")?;
        }
        // 4g. the content stream is regenerated exactly once.
        scratch
            .page
            .regenerate_content()
            .ctx("regenerate page content")?;
    }
    doc.text.invalidate_page(page_index);
    doc.annots.remove(&page_index);
    doc.touched.insert(page_index);

    // 5. post-conditions, on a fresh parse of the regenerated content.
    if !split_done.is_empty() {
        let page = doc.page(page_index)?;
        let chars = split::page_chars(bindings, page)?;
        let failed: Vec<u32> = split_done
            .iter()
            .filter(|(_, runs)| !split::survives(&chars, runs))
            .map(|(index, _)| *index)
            .collect();
        if !failed.is_empty() {
            return Err(split_failed(page_index, &failed));
        }
    }
    let text_after = text::layer::page_text(doc, page_index)?.text.clone();
    if let Some(survivor) = first_survivor(&marked, &text_before, &text_after) {
        return Err(EngineError::new(
            ErrorCode::VerifyFailed,
            format!("redacted text {survivor:?} is still extractable; the page was restored"),
        )
        .with_page(page_index));
    }
    Ok(outcome)
}

/// A text object rewritten to its first run, with a copy per further run, not yet read back.
struct PendingSplit {
    index: u32,
    original: FPDF_PAGEOBJECT,
    copies: Vec<FPDF_PAGEOBJECT>,
    runs: Vec<split::Run>,
    /// What goes if it has to be removed whole after all.
    whole: String,
}

impl Victim {
    /// The text a whole-run removal of this object loses outside the marks.
    fn collateral_if_whole(&self) -> String {
        match &self.action {
            Action::Split(runs) => runs
                .iter()
                .map(split::Run::text)
                .collect::<Vec<_>>()
                .join(" ")
                .trim()
                .to_string(),
            _ => self.collateral.trim().to_string(),
        }
    }
}

/// Ungroups every group in `groups` (highest index first) and regenerates the page.
fn ungroup_all(
    doc: &mut OpenDoc<'_>,
    page_index: PageIndex,
    groups: &BTreeSet<u32>,
) -> Result<(), EngineError> {
    let bindings = doc.bindings();
    {
        let mut scratch = ScratchPage::open(doc, page_index)?;
        for &g in groups.iter().rev() {
            ungroup::ungroup_at(bindings, &scratch.page, g as usize, true)?;
        }
        scratch
            .page
            .regenerate_content()
            .ctx("regenerate page content")?;
    }
    doc.text.invalidate_page(page_index);
    Ok(())
}

/// The `detail` of a post-regeneration split failure: `splitFailed:<page>:<i>,<j>,…`.
fn split_failed(page: PageIndex, indices: &[u32]) -> EngineError {
    let list: Vec<String> = indices.iter().map(u32::to_string).collect();
    EngineError::new(
        ErrorCode::VerifyFailed,
        "a split text run did not survive the rewrite; retrying with whole-run removal",
    )
    .with_page(page)
    .with_detail(format!("splitFailed:{page}:{}", list.join(",")))
}

/// `(page, object indices)` of a [`split_failed`] error.
fn parse_split_failed(e: &EngineError) -> Option<(PageIndex, Vec<u32>)> {
    let rest = e.detail.as_deref()?.strip_prefix("splitFailed:")?;
    let (page, list) = rest.split_once(':')?;
    let page = page.parse().ok()?;
    let indices = list.split(',').filter_map(|s| s.parse().ok()).collect();
    Some((page, indices))
}

/// [`apply`] wrapped in the rollback the contract promises.
///
/// A byte snapshot is taken first (5 ms/MB — this is a destructive, user-initiated command,
/// not a hot path) and restored whenever the redaction reports `verifyFailed`, so the
/// document is byte-for-byte what it was whichever of the verification points fired. A split
/// that did not survive the rewrite is retried once with those objects removed whole.
/// This is the entry point the command and the tests use.
pub fn apply_verified(
    st: &mut EngineState<'_>,
    doc_id: &str,
    page_index: PageIndex,
    rects: &[Rect],
    options: &RedactOptions,
) -> Result<u32, EngineError> {
    apply_batch(
        st,
        doc_id,
        &[RedactBatchMark {
            page: page_index,
            rects: rects.to_vec(),
        }],
        options,
    )
    .map(|r| r.removed_objects)
}

/// `apply_redactions_batch` (Stage 8) — every marked page in **one** `registry::mutate`, so
/// the whole 영역 표시 batch is one undo step (`undo.redact`) and one generation.
///
/// Marks for the same page are merged; pages are processed in ascending order, each through
/// [`apply_page`] (its own pre-flight, removal and post-condition). The form-field refusal is
/// checked for **every** page before anything changes; any later failure — a `verifyFailed`
/// on the third page included — makes `mutate` reload its snapshot, so the pages already
/// redacted in this call are rolled back too. As in [`apply_verified`], a byte snapshot is
/// restored on `verifyFailed` as a second safety net.
pub fn apply_batch(
    st: &mut EngineState<'_>,
    doc_id: &str,
    marks: &[RedactBatchMark],
    options: &RedactOptions,
) -> Result<RedactBatchResult, EngineError> {
    let count = st.doc(doc_id)?.page_count();
    let mut by_page: BTreeMap<PageIndex, Vec<Rect>> = BTreeMap::new();
    for mark in marks {
        if mark.page >= count {
            return Err(EngineError::invalid(format!(
                "page {} is out of range (page count {count})",
                mark.page
            ))
            .with_page(mark.page));
        }
        if !mark.rects.is_empty() {
            by_page
                .entry(mark.page)
                .or_default()
                .extend(mark.rects.iter().copied());
        }
    }
    if by_page.is_empty() {
        return Err(EngineError::invalid("redaction needs at least one rect"));
    }

    // Pre-flight for every page before the first one is touched.
    for (&page, rects) in &by_page {
        let plan = preview(st.doc_mut(doc_id)?, page, rects)?;
        if !plan.form_fields.is_empty() {
            return Err(EngineError::new(
                ErrorCode::Unsupported,
                format!(
                    "{} form field(s) are under the marks on page {}; flatten the form first",
                    plan.form_fields.len(),
                    page + 1
                ),
            )
            .with_page(page));
        }
    }

    let pages: Vec<PageIndex> = by_page.keys().copied().collect();
    let snapshot = st.doc(doc_id)?.to_bytes()?;
    let mut forced: HashMap<PageIndex, HashSet<u32>> = HashMap::new();
    let mut attempts = 0;
    let outcome = loop {
        attempts += 1;
        let result = registry::mutate(
            st,
            doc_id,
            MutateOpts::new("undo.redact", ChangeReason::Redact).pages(pages.clone()),
            |doc| {
                let mut total = PageOutcome::default();
                for (&page, rects) in &by_page {
                    let none = HashSet::new();
                    let whole = forced.get(&page).unwrap_or(&none);
                    let o = apply_page(doc, page, rects, options, whole)?;
                    total.changed += o.changed;
                    total.blanked += o.blanked;
                    total.collateral.extend(o.collateral);
                }
                Ok(total)
            },
        );
        match result {
            Err(e) if e.code == ErrorCode::VerifyFailed => {
                registry::replace(st, doc_id, snapshot.clone())?;
                match parse_split_failed(&e) {
                    // `mutate` rolled everything back; go again with those objects whole.
                    Some((page, indices)) if attempts < 3 => {
                        forced.entry(page).or_default().extend(indices);
                        continue;
                    }
                    _ => return Err(e),
                }
            }
            other => break other?,
        }
    };
    if outcome.blanked > 0 {
        // PDFium shares one decoded image between every object that draws the same image
        // XObject — on other pages too — so the blank shows on all of them in memory, while the
        // saved file blanks only this page's (the others still reference the old stream). Reload
        // from the bytes a save would write, so what the viewer shows is what the file holds.
        let bytes = crate::engine::save::serialize(st, doc_id)?;
        registry::replace(st, doc_id, std::sync::Arc::from(bytes.into_boxed_slice()))?;
    }
    Ok(RedactBatchResult {
        removed_objects: outcome.changed,
        verified: true,
        doc_generation: st.doc(doc_id)?.generation,
        pages,
        collateral: outcome.collateral,
    })
}

/// One contiguous run of characters under the marks, and whether it can actually be deleted.
struct MarkedRun {
    text: String,
    removable: bool,
}

/// Every contiguous run of characters the marks cover, in reading order.
///
/// This is what [`apply_page`] verifies against, and it is deliberately independent of the
/// objects that happen to overlap the marks: if a run's characters belong to no removable
/// page object (text inside a group that was not ungrouped, a glyph whose owner could not be
/// identified) the run is listed as **not removable** and the command refuses before touching
/// anything. A character with no owner at all is still removable when it sits inside the
/// bounds of a text object that is being removed.
fn marked_runs(a: &Analysis, rects: &[Rect]) -> Vec<MarkedRun> {
    let removable_ids: HashSet<u32> = a
        .victims
        .iter()
        .filter(|v| {
            v.kind == PdfPageObjectType::Text
                && matches!(v.action, Action::Remove | Action::Split(_))
        })
        .map(|v| v.index)
        .collect();
    let removed_text: Vec<Rect> = a
        .victims
        .iter()
        .filter(|v| v.kind == PdfPageObjectType::Text && v.action == Action::Remove)
        .map(|v| v.rect)
        .collect();
    let mut runs: Vec<MarkedRun> = Vec::new();
    let mut text = String::new();
    let mut removable = true;
    for c in &a.chars {
        if c.generated {
            continue;
        }
        if split::marked(c, rects) {
            text.push(char::from_u32(c.unicode).unwrap_or('\u{fffd}'));
            let owned = match a.index_of.get(&c.object) {
                Some(index) => removable_ids.contains(index),
                None if a.in_group.contains_key(&c.object) => false,
                None => removed_text.iter().any(|r| contains(r, &c.tight)),
            };
            removable &= owned;
        } else if !text.is_empty() {
            runs.push(MarkedRun {
                text: std::mem::take(&mut text),
                removable,
            });
            removable = true;
        }
    }
    if !text.is_empty() {
        runs.push(MarkedRun { text, removable });
    }
    for run in &mut runs {
        run.text = run.text.trim().to_string();
    }
    runs.retain(|r| !r.text.is_empty());
    runs
}

/// The marked string whose occurrence count did not drop, if any.
///
/// Counting rather than "is it absent" is what makes this correct for a word that also occurs
/// somewhere else on the page: redacting one "the" must not require every "the" to vanish.
fn first_survivor<'a>(marked: &'a [String], before: &str, after: &str) -> Option<&'a str> {
    marked.iter().map(String::as_str).find(|needle| {
        let was = before.matches(needle).count();
        let now = after.matches(needle).count();
        was > 0 && now >= was
    })
}

/// Classifies every page object against the marks (never mutates).
fn analyze(
    doc: &mut OpenDoc<'_>,
    page_index: PageIndex,
    rects: &[Rect],
    forced_whole: &HashSet<u32>,
) -> Result<Analysis, EngineError> {
    let bindings = doc.bindings();
    let page = doc.page(page_index)?;
    let chars = raw::TextPage::load(bindings, page)?.chars();
    let index_of = raw::index_map(bindings, page);
    let count = raw::object_count(bindings, page);

    // Groups: every descendant of every top-level form, with images placed on the page.
    let mut in_group: HashMap<HandleKey, u32> = HashMap::new();
    let mut groups: BTreeSet<u32> = BTreeSet::new();
    for index in 0..count {
        let handle = raw::object_at(bindings, page, index)?;
        if raw::object_type(bindings, handle) != raw::OBJ_FORM {
            continue;
        }
        let m = raw::matrix(bindings, handle);
        let mut hit = false;
        walk_group(bindings, handle, m, 0, &mut |child, kind, placed| {
            in_group.insert(raw::key(child), index as u32);
            if kind == raw::OBJ_IMAGE
                && placed.is_some_and(|r| rects.iter().any(|m| m.intersects(&r)))
            {
                hit = true;
            }
        });
        if hit {
            groups.insert(index as u32);
        }
    }

    // Characters per top-level object, and marked text inside groups.
    let mut per_object: BTreeMap<u32, Vec<RawChar>> = BTreeMap::new();
    let mut groups_with_text: BTreeSet<u32> = BTreeSet::new();
    for c in &chars {
        if c.generated {
            continue;
        }
        if let Some(&index) = index_of.get(&c.object) {
            per_object.entry(index).or_default().push(*c);
        } else if let Some(&g) = in_group.get(&c.object) {
            if split::marked(c, rects) {
                groups.insert(g);
                groups_with_text.insert(g);
            }
        }
    }

    let mut victims = Vec::new();
    for index in 0..count {
        let object = page.objects().get(index).ctx("read page object")?;
        let kind = object.object_type();
        let Ok(bounds) = object.bounds() else {
            continue;
        };
        let rect = quad_bounds(&bounds);
        let overlaps = rects.iter().any(|r| r.intersects(&rect));
        let fully_inside = rects.iter().any(|r| contains(r, &rect));
        let own = per_object.get(&(index as u32));
        let any_marked = own.is_some_and(|cs| cs.iter().any(|c| split::marked(c, rects)));
        if !overlaps && !any_marked {
            continue;
        }
        let handle = raw::object_at(bindings, page, index)?;
        let (text, collateral, action) = match kind {
            PdfPageObjectType::Text => {
                let Some(cs) = own else {
                    // No characters attributed to it (rare): the whole run, as before v0.3.
                    let text = object
                        .as_text_object()
                        .map(|t| t.text())
                        .unwrap_or_default();
                    let collateral = if fully_inside {
                        String::new()
                    } else {
                        text.clone()
                    };
                    victims.push(Victim {
                        index: index as u32,
                        kind,
                        rect,
                        text,
                        collateral,
                        fully_inside,
                        action: Action::Remove,
                    });
                    continue;
                };
                if !any_marked {
                    // The bounds overlap but no glyph is under a mark: nothing to remove.
                    continue;
                }
                let text: String = cs
                    .iter()
                    .map(|c| char::from_u32(c.unicode).unwrap_or('\u{fffd}'))
                    .collect();
                let outside: String = cs
                    .iter()
                    .filter(|c| !split::marked(c, rects))
                    .map(|c| char::from_u32(c.unicode).unwrap_or('\u{fffd}'))
                    .collect();
                match split::runs(cs, rects) {
                    Some(runs) if runs.is_empty() => (text, String::new(), Action::Remove),
                    Some(runs)
                        if !forced_whole.contains(&(index as u32))
                            && split::splittable(bindings, handle, cs) =>
                    {
                        (text, String::new(), Action::Split(runs))
                    }
                    _ => (text, outside, Action::Remove),
                }
            }
            PdfPageObjectType::Image => {
                let action = if fully_inside {
                    Action::Remove
                } else {
                    // Decided below on a throw-away parse (`image::can_blank` probes the mask).
                    let jpeg = object.as_image_object().is_some_and(|i| {
                        i.filters()
                            .iter()
                            .any(|f| matches!(f.name(), "DCTDecode" | "JPXDecode"))
                    });
                    Action::Blank(jpeg)
                };
                (String::new(), String::new(), action)
            }
            PdfPageObjectType::Path => {
                let action = if fully_inside || path_goes(bindings, handle, &rect, rects) {
                    Action::Remove
                } else {
                    Action::Keep
                };
                (String::new(), String::new(), action)
            }
            _ => continue,
        };
        victims.push(Victim {
            index: index as u32,
            kind,
            rect,
            text,
            collateral,
            fully_inside,
            action,
        });
    }
    // Partly covered images: can they be blanked faithfully? The probe swaps the object's
    // matrix for a moment, so it runs on a second parse that is dropped unregenerated.
    if victims.iter().any(|v| matches!(v.action, Action::Blank(_))) {
        let mut probe = doc
            .pdf()
            .pages()
            .get(page_index as PdfPageIndex)
            .ctx(&format!("load page {page_index}"))?;
        probe.set_content_regeneration_strategy(PdfPageContentRegenerationStrategy::Manual);
        for v in victims.iter_mut() {
            if matches!(v.action, Action::Blank(_)) {
                let handle = raw::object_at(bindings, &probe, v.index as usize)?;
                if !image::can_blank(bindings, doc.pdf(), &probe, handle) {
                    v.action = Action::Remove;
                }
            }
        }
    }
    Ok(Analysis {
        victims,
        groups,
        groups_with_text,
        chars,
        index_of,
        in_group,
    })
}

/// A partly covered path goes when it is small or curved (module docs).
fn path_goes(
    bindings: &dyn pdfium_render::prelude::PdfiumLibraryBindings,
    handle: FPDF_PAGEOBJECT,
    rect: &Rect,
    rects: &[Rect],
) -> bool {
    if rect.width().max(rect.height()) <= SMALL_PATH_PT {
        return true;
    }
    let area = (rect.width() * rect.height()).max(1e-3);
    let covered: f32 = rects
        .iter()
        .map(|r| {
            let w = (r.r.min(rect.r) - r.l.max(rect.l)).max(0.0);
            let h = (r.t.min(rect.t) - r.b.max(rect.b)).max(0.0);
            w * h
        })
        .sum();
    covered * 4.0 >= area || raw::path_has_curves(bindings, handle)
}

/// Visits every descendant of the form `form` (drawn with the page-space matrix `m`), with its
/// type and — for everything but nested forms — its bounds on the page.
fn walk_group(
    bindings: &dyn pdfium_render::prelude::PdfiumLibraryBindings,
    form: FPDF_PAGEOBJECT,
    m: crate::engine::raw::object::Matrix,
    depth: usize,
    visit: &mut dyn FnMut(FPDF_PAGEOBJECT, i32, Option<Rect>),
) {
    if depth > MAX_GROUP_DEPTH {
        return;
    }
    for child in raw::form_children(bindings, form) {
        let kind = raw::object_type(bindings, child);
        let placed = raw::bounds(bindings, child).map(|r| transform_rect(m, r));
        visit(child, kind, placed);
        if kind == raw::OBJ_FORM {
            let inner = concat(raw::matrix(bindings, child), m);
            walk_group(bindings, child, inner, depth + 1, visit);
        }
    }
}

/// `a` then `b` (PDF row-vector convention).
fn concat(
    a: crate::engine::raw::object::Matrix,
    b: crate::engine::raw::object::Matrix,
) -> crate::engine::raw::object::Matrix {
    [
        a[0] * b[0] + a[1] * b[2],
        a[0] * b[1] + a[1] * b[3],
        a[2] * b[0] + a[3] * b[2],
        a[2] * b[1] + a[3] * b[3],
        a[4] * b[0] + a[5] * b[2] + b[4],
        a[4] * b[1] + a[5] * b[3] + b[5],
    ]
}

/// The bounding box of `r` mapped through `m`.
fn transform_rect(m: crate::engine::raw::object::Matrix, r: Rect) -> Rect {
    let pts = [(r.l, r.b), (r.r, r.b), (r.r, r.t), (r.l, r.t)];
    let (mut l, mut b, mut rr, mut t) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for (x, y) in pts {
        let (px, py) = (m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5]);
        l = l.min(px);
        b = b.min(py);
        rr = rr.max(px);
        t = t.max(py);
    }
    Rect::new(l, b, rr, t)
}

/// `PdfQuadPoints` → the contract's rect, order-agnostic (`to_rect()` assumes one corner
/// order and returns an empty rect for the other).
fn quad_bounds(q: &pdfium_render::prelude::PdfQuadPoints) -> Rect {
    let xs = [q.x1().value, q.x2().value, q.x3().value, q.x4().value];
    let ys = [q.y1().value, q.y2().value, q.y3().value, q.y4().value];
    Rect::new(
        xs.iter().copied().fold(f32::MAX, f32::min),
        ys.iter().copied().fold(f32::MAX, f32::min),
        xs.iter().copied().fold(f32::MIN, f32::max),
        ys.iter().copied().fold(f32::MIN, f32::max),
    )
}

fn contains(outer: &Rect, inner: &Rect) -> bool {
    outer.l <= inner.l && outer.b <= inner.b && outer.r >= inner.r && outer.t >= inner.t
}

/// `PdfRect::new_from_values(bottom, left, top, right)` — the unusual argument order.
fn to_pdf_rect(r: Rect) -> PdfRect {
    PdfRect::new(
        PdfPoints::new(r.b.min(r.t)),
        PdfPoints::new(r.l.min(r.r)),
        PdfPoints::new(r.b.max(r.t)),
        PdfPoints::new(r.l.max(r.r)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_survivor_is_one_whose_count_did_not_drop() {
        let marked = vec!["Gal".to_string(), "the".to_string()];
        // "Gal" gone, one "the" of two gone: nothing survived.
        assert_eq!(
            first_survivor(&marked, "Andreas Gal and the the", "and the"),
            None
        );
        // "Gal" still there.
        assert_eq!(
            first_survivor(&marked, "Andreas Gal and the", "Andreas Gal and"),
            Some("Gal")
        );
    }

    #[test]
    fn containment_is_strict() {
        let outer = Rect::new(0.0, 0.0, 10.0, 10.0);
        assert!(contains(&outer, &Rect::new(1.0, 1.0, 9.0, 9.0)));
        assert!(!contains(&outer, &Rect::new(1.0, 1.0, 11.0, 9.0)));
    }

    #[test]
    fn split_failures_round_trip_through_the_error_detail() {
        let e = split_failed(3, &[4, 17]);
        assert_eq!(e.code, ErrorCode::VerifyFailed);
        assert_eq!(parse_split_failed(&e), Some((3, vec![4, 17])));
        assert_eq!(parse_split_failed(&EngineError::invalid("x")), None);
    }

    #[test]
    fn group_matrices_compose_form_then_page() {
        // A child scaled ×2 inside a form translated by (100, 50).
        let m = concat(
            [2.0, 0.0, 0.0, 2.0, 0.0, 0.0],
            [1.0, 0.0, 0.0, 1.0, 100.0, 50.0],
        );
        assert_eq!(
            transform_rect(m, Rect::new(0.0, 0.0, 10.0, 10.0)),
            Rect::new(100.0, 50.0, 120.0, 70.0)
        );
    }
}
