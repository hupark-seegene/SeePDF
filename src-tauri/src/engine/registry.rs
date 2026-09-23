//! Open documents, their ids, generations and the **one** `&mut` path into a document.
//!
//! `ARCHITECTURE.md` §2. The rules here are enforced by the code, not by convention:
//!
//! 1. [`OpenDoc::page`] opens through the [`PageLru`], runs `FORM_OnAfterLoadPage` and sets
//!    `PdfPageContentRegenerationStrategy::Manual`;
//! 2. [`mutate`] is the only way to change a document: snapshot → (flush the page LRU when
//!    the change is structural) → run → refresh `pages_meta` → `generation += 1` →
//!    invalidate caches → emit `doc-changed`. One user action = one `mutate` = one
//!    generation = one undo step;
//! 3. [`replace`] (undo / redo / after save) drops pages, then the document, then loads the
//!    new bytes;
//! 4. field order in [`OpenDoc`] is drop order: pages **must** drop before the document.

use crate::engine::history::History;
use crate::engine::page_lru::PageLru;
use crate::engine::raw;
use crate::engine::text::TextCache;
use crate::engine::types::{DocSummary, EngineState};
use crate::ipc::error::PdfiumResultExt;
use crate::ipc::types::{
    Annot, ChangeReason, ChangedPages, DocChangedPayload, DocGeneration, DocId, DocInfo, DocMeta,
    OutlineNode, PageGeom, Permissions, Rect, SecurityRevision,
};
use crate::ipc::{EngineError, ErrorCode};
use pdfium_render::prelude::*;
use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::Arc;
use tauri::Emitter;

/// Documents up to this many pages get their exact `/Rotate`, crop box and label read for
/// **every** page at open (and after every reload), because PDFium has no way to read
/// `/Rotate` without an `FPDF_LoadPage` (≈ 0.5 ms per page after the first). 64 pages costs
/// ~35 ms, well inside the 250 ms open budget.
const REFINE_ALL_UP_TO: u16 = 64;
/// Larger documents only refine the pages the viewer paints first; the rest carry the
/// (correct) rotated size from `page_sizes()` with a provisional rotation of 0 until they are
/// opened. See `STAGE0_NOTES.md` — this is the one place `DocInfo` can lag.
const REFINE_HEAD: u16 = 8;

fn refine_count(page_count: u16) -> u16 {
    if page_count <= REFINE_ALL_UP_TO {
        page_count
    } else {
        REFINE_HEAD
    }
}

pub struct OpenDoc<'p> {
    // ---- field order == drop order: pages MUST drop before the document ----
    pages: PageLru<'p>,
    doc: PdfDocument<'p>,
    // ----------------------------------------------------------------------
    pub doc_id: DocId,
    /// Raw form handle, copied at load so the page LRU can use it without re-borrowing
    /// the document. `None` when the document has no AcroForm.
    form: Option<FPDF_FORMHANDLE>,
    bindings: &'static dyn PdfiumLibraryBindings,
    /// The bytes `doc` was loaded from (undo base, `/raw` route).
    pub bytes: Arc<[u8]>,
    /// `None` for untitled / merged documents.
    pub path: Option<PathBuf>,
    pub password: Option<String>,
    /// +1 on every mutation; part of every cache key and tile URL.
    pub generation: DocGeneration,
    /// dirty <=> `generation != saved_generation`.
    pub saved_generation: DocGeneration,
    pub pages_meta: Vec<PageGeom>,
    /// Pages with annotation edits: they need one render before save so pdfium writes `/AP`.
    pub touched: BTreeSet<u16>,
    pub history: History,
    /// Token for the bundled Hangul font once it has been embedded in **this document**.
    ///
    /// `FPDFText_LoadFont` appends a fresh copy of the ~487 KB subset every time it is called
    /// and PDFium does not deduplicate, so without this Korean text on five different pages of
    /// one document cost five copies (`STAGE1B_NOTES.md` §5.2). It cannot live in a module
    /// cache: the `FPDF_FONT` dies with the `FPDF_DOCUMENT`, which [`replace`] swaps on every
    /// undo, redo and save — so [`replace`] clears it.
    hangul_font: Option<PdfFontToken>,
    pub text: TextCache,
    /// Rebuilt lazily per page per generation by Stage 1 (a).
    pub annots: HashMap<u16, Vec<Annot>>,
    pub encrypted: bool,
    pub permissions: Permissions,
    pub meta: DocMeta,
    pub pdf_version: String,
    pub tagged: bool,
    pub xfa: bool,
    pub has_outline: bool,
}

impl<'p> OpenDoc<'p> {
    pub fn page_count(&self) -> u16 {
        self.pages_meta.len() as u16
    }

    pub fn dirty(&self) -> bool {
        self.generation != self.saved_generation
    }

    pub fn has_form(&self) -> bool {
        self.form.is_some()
    }

    /// The raw form handle, for `engine::raw::form::*` and the Stage 1 form module.
    pub fn form_handle(&self) -> Option<FPDF_FORMHANDLE> {
        self.form
    }

    pub fn bindings(&self) -> &'static dyn PdfiumLibraryBindings {
        self.bindings
    }

    /// Read-only access to the pdfium document. Never mutate through this: go through
    /// [`mutate`] so an undo snapshot is taken and the generation is bumped.
    pub fn pdf(&self) -> &PdfDocument<'p> {
        &self.doc
    }

    /// `&mut` access for [`mutate`] and for `pages_mut()` operations inside it.
    pub fn pdf_mut(&mut self) -> &mut PdfDocument<'p> {
        &mut self.doc
    }

    /// [`crate::engine::fonts::token_for`], but against the document-level Hangul token, so
    /// the bundled subset is embedded **once per document** rather than once per call site.
    ///
    /// Splitting the borrow here is the whole point: `fonts::token_for` needs
    /// `&mut PdfDocument` *and* `&mut Option<PdfFontToken>`, and those are two fields of the
    /// same `OpenDoc`.
    pub fn hangul_token_for(
        &mut self,
        text: &str,
    ) -> Result<(PdfFontToken, crate::engine::fonts::Pick), EngineError> {
        let mut cached = self.hangul_font;
        let out = crate::engine::fonts::token_for(&mut self.doc, text, &mut cached);
        self.hangul_font = cached;
        out
    }

    /// Unconditionally load (or reuse) the bundled font — `ocr_apply`'s "one font for the
    /// whole batch" path, which knows up front that it needs Hangul.
    pub fn hangul_token(&mut self) -> Result<PdfFontToken, EngineError> {
        if let Some(token) = self.hangul_font {
            return Ok(token);
        }
        let token = crate::engine::fonts::load(&mut self.doc)?;
        self.hangul_font = Some(token);
        Ok(token)
    }

    /// Adopt a copy of the bundled font that is **already embedded in this page** as the
    /// document's token, so re-editing a document we wrote earlier embeds nothing at all.
    pub fn adopt_hangul_token(&mut self, page: &PdfPage<'p>) {
        if self.hangul_font.is_none() {
            self.hangul_font = crate::engine::fonts::embedded_token(page);
        }
    }

    pub fn page_lru_len(&self) -> usize {
        self.pages.len()
    }

    /// Opens `index` through the LRU (see module docs for what that guarantees) and refines
    /// this page's cached geometry the first time it is seen.
    pub fn page<'s>(&'s mut self, index: u16) -> Result<&'s mut PdfPage<'p>, EngineError> {
        let fresh = !self.pages.contains(index);
        let page = self.pages.get_or_open(&self.doc, index)?;
        if fresh {
            if let Some(meta) = geom_from_page(index, page) {
                if let Some(slot) = self.pages_meta.get_mut(index as usize) {
                    // Keep the label read at open (document-level, cheaper than the page).
                    let label = slot.label.clone().or(meta.label.clone());
                    *slot = PageGeom { label, ..meta };
                }
            }
        }
        Ok(self
            .pages
            .get_or_open(&self.doc, index)
            .expect("page was opened above"))
    }

    /// Drops every open page handle. Required before any structural change and before the
    /// document itself goes away.
    pub fn close_pages(&mut self) {
        self.pages.clear();
    }

    /// Drops one page handle **and** its cached text layer — for an edit that changed the
    /// page's content stream outside [`mutate`]'s bookkeeping.
    pub fn invalidate_page(&mut self, index: u16) {
        self.pages.invalidate(index);
        self.text.invalidate_page(index);
    }

    /// Drops the page **handle** only, keeping the extracted text.
    ///
    /// A PDFium page handle is a parse of the page, not the page itself: dropping it says
    /// nothing about whether the characters on that page changed. The two callers that need
    /// this — `ScratchPage::open` (so two handles never straddle an edit) and
    /// `refresh_pages_meta` (so `page()` re-reads `/Rotate` and the crop box) — are both
    /// inside [`mutate`], which is the one place that knows which pages actually changed and
    /// invalidates their text there. Throwing the layer away here as well cost a 5.6 ms
    /// rebuild the next time the viewer selected text on the page (STAGE1A §5.1).
    pub fn invalidate_page_handle(&mut self, index: u16) {
        self.pages.invalidate(index);
    }

    pub fn geom(&self, index: u16) -> Result<&PageGeom, EngineError> {
        self.pages_meta
            .get(index as usize)
            .ok_or_else(|| EngineError::not_found(format!("page {index}")).with_page(index))
    }

    pub fn name(&self) -> String {
        self.path
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "Untitled.pdf".to_string())
    }

    pub fn info(&self) -> DocInfo {
        DocInfo {
            doc_id: self.doc_id.clone(),
            path: self.path.as_ref().map(|p| p.display().to_string()),
            name: self.name(),
            bytes: self.bytes.len() as u64,
            page_count: self.page_count(),
            pages: self.pages_meta.clone(),
            doc_generation: self.generation,
            dirty: self.dirty(),
            can_undo: self.history.can_undo(),
            can_redo: self.history.can_redo(),
            undo_label: self.history.undo_label(),
            redo_label: self.history.redo_label(),
            encrypted: self.encrypted,
            permissions: self.permissions,
            has_form: self.has_form(),
            xfa: self.xfa,
            has_outline: self.has_outline,
            meta: self.meta.clone(),
            pdf_version: self.pdf_version.clone(),
            tagged: self.tagged,
        }
    }

    pub fn summary(&self) -> DocSummary {
        DocSummary {
            doc_id: self.doc_id.clone(),
            generation: self.generation,
            page_count: self.page_count(),
            path: self.path.clone(),
            pages: Arc::new(self.pages_meta.clone()),
        }
    }

    /// `save_to_bytes()` — the undo snapshot and the save payload (5 ms/MB).
    pub fn to_bytes(&self) -> Result<Arc<[u8]>, EngineError> {
        let bytes = self.doc.save_to_bytes().ctx("save_to_bytes")?;
        Ok(Arc::from(bytes.into_boxed_slice()))
    }

    /// The document's outline as the contract's nested node list. `PdfBookmarks::root()` is
    /// the **first top-level bookmark**, not a synthetic root, so siblings are walked too.
    pub fn outline(&self) -> Vec<OutlineNode> {
        let bookmarks = self.doc.bookmarks();
        let Some(root) = bookmarks.root() else {
            return Vec::new();
        };
        let mut out = Vec::new();
        collect_bookmark(&root, &mut out, 0);
        for sibling in root.iter_siblings() {
            collect_bookmark(&sibling, &mut out, 0);
        }
        out
    }
}

fn collect_bookmark(node: &PdfBookmark<'_>, out: &mut Vec<OutlineNode>, depth: u8) {
    // Guard against a malformed cyclic outline.
    if depth > 32 {
        return;
    }
    let mut children = Vec::new();
    for child in node.iter_direct_children() {
        collect_bookmark(&child, &mut children, depth + 1);
    }
    let destination = node.destination();
    out.push(OutlineNode {
        title: node.title().unwrap_or_default(),
        page: destination
            .as_ref()
            .and_then(|d| d.page_index().ok())
            .map(|i| i as u16),
        dest: destination.as_ref().and_then(outline_dest),
        children,
    });
}

/// `FPDFDest_GetView` + `FPDFDest_GetLocationInPage`, reduced to the (x, y, zoom) a scroller
/// can act on. Both calls are document-level and need no `FPDF_LoadPage`, which is why the
/// destination rect is affordable for every node at open (`STAGE1C_NOTES.md` §7.1).
fn outline_dest(d: &PdfDestination<'_>) -> Option<crate::ipc::types::OutlineDest> {
    use crate::ipc::types::OutlineDest;
    use pdfium_render::prelude::PdfDestinationViewSettings as View;
    let out = match d.view_settings().ok()? {
        View::SpecificCoordinatesAndZoom(x, y, zoom) => OutlineDest {
            x: x.map(|p| p.value),
            y: y.map(|p| p.value),
            zoom: zoom.filter(|z| *z > 0.0),
        },
        View::FitPageHorizontallyToWindow(y) | View::FitBoundsHorizontallyToWindow(y) => {
            OutlineDest { x: None, y: y.map(|p| p.value), zoom: None }
        }
        View::FitPageVerticallyToWindow(x) | View::FitBoundsVerticallyToWindow(x) => {
            OutlineDest { x: x.map(|p| p.value), y: None, zoom: None }
        }
        View::FitPageToRectangle(rect) => OutlineDest {
            x: Some(rect.left().value),
            y: Some(rect.top().value),
            zoom: None,
        },
        View::Unknown | View::FitPageToWindow | View::FitBoundsToWindow => return None,
    };
    if out.x.is_none() && out.y.is_none() && out.zoom.is_none() {
        return None;
    }
    Some(out)
}

// ---------------------------------------------------------------------------------------
// open / close
// ---------------------------------------------------------------------------------------

/// Opens `bytes` as a new document. `path` is `None` for untitled / merged documents.
pub fn open<'p>(
    st: &mut EngineState<'p>,
    path: Option<PathBuf>,
    bytes: Vec<u8>,
    password: Option<String>,
) -> Result<DocInfo, EngineError> {
    let doc_id = st.alloc_doc_id();
    let byte_len = bytes.len();
    let shared_bytes: Arc<[u8]> = Arc::from(bytes.clone().into_boxed_slice());
    let doc = st
        .pdfium
        .load_pdf_from_byte_vec(bytes, password.as_deref())
        .map_err(|e| {
            let err = EngineError::pdfium("load document", e);
            if err.code == ErrorCode::PasswordRequired && password.is_some() {
                EngineError::new(ErrorCode::PasswordWrong, "supplied password was rejected")
            } else {
                err
            }
        })?;

    let bindings = raw::bindings(st.pdfium);
    let form = doc.form().map(|f| f.raw_handle());
    if let Some(form) = form {
        // The viewer draws field highlighting itself (the `hl=1` tile parameter).
        raw::form::set_field_highlight_alpha(bindings, form, 0);
    }
    let xfa = doc
        .form()
        .map(|f| matches!(f.form_type(), PdfFormType::XfaFull | PdfFormType::XfaForeground))
        .unwrap_or(false);
    let encrypted = security_revision(bindings, &doc) != -1 || password.is_some();

    let permissions = read_permissions(bindings, &doc);
    let meta = read_metadata(bindings, &doc);
    let pdf_version = version_string(doc.version());
    let tagged = doc.catalog().is_tagged();
    let has_outline = doc.bookmarks().root().is_some();
    let pages_meta = read_pages_meta(bindings, &doc)?;

    let spill_dir = st.spill_dir.join(&doc_id);
    let mut open_doc = OpenDoc {
        pages: PageLru::new(bindings, form),
        doc,
        doc_id: doc_id.clone(),
        form,
        bindings,
        bytes: shared_bytes,
        path,
        password,
        generation: 1,
        saved_generation: 1,
        pages_meta,
        touched: BTreeSet::new(),
        history: History::new(spill_dir, byte_len, st.history_budget),
        hangul_font: None,
        text: TextCache::default(),
        annots: HashMap::new(),
        encrypted,
        permissions,
        meta,
        pdf_version,
        tagged,
        xfa,
        has_outline,
    };

    // Exact geometry (rotation, crop box, label) for as many pages as the budget allows.
    for i in 0..refine_count(open_doc.page_count()) {
        let _ = open_doc.page(i);
    }

    let info = open_doc.info();
    st.shared
        .docs
        .write()
        .insert(doc_id.clone(), open_doc.summary());
    st.docs.insert(doc_id, open_doc);
    Ok(info)
}

/// Closes a document: pages → form → document, in that order.
pub fn close(st: &mut EngineState<'_>, doc_id: &str) -> Result<(), EngineError> {
    let Some(mut doc) = st.docs.remove(doc_id) else {
        return Err(EngineError::not_found(format!("unknown document '{doc_id}'")));
    };
    if let Some(form) = doc.form {
        raw::form::force_to_kill_focus(doc.bindings, form);
    }
    doc.close_pages();
    doc.history.clear();
    st.shared.docs.write().remove(doc_id);
    st.shared.tiles.drop_document(doc_id);
    let spill = doc.history.spill_dir().to_path_buf();
    drop(doc);
    let _ = std::fs::remove_dir_all(spill);
    Ok(())
}

// ---------------------------------------------------------------------------------------
// mutate / replace
// ---------------------------------------------------------------------------------------

/// What a [`mutate`] call is going to do, so the registry can invalidate the right things.
#[derive(Debug, Clone)]
pub struct MutateOpts {
    /// i18n key for the undo label, e.g. `"undo.annotCreate"`.
    pub label: &'static str,
    /// The page list changes (move / delete / insert / duplicate). Flushes the page LRU
    /// **before** `f` runs, because a raw `FPDF_MovePages` bypasses pdfium-render's
    /// `PdfPageIndexCache`.
    pub structural: bool,
    /// Pages whose pixels changed; `All` for structural edits.
    pub pages: ChangedPages,
    pub reason: ChangeReason,
    /// Drag / slider gestures: collapse repeats of the same label within 500 ms.
    pub coalesce: bool,
    /// The edit does not touch the pages' **content streams**, so the extracted text of the
    /// changed pages is still valid and its cached layer is kept. True for annotation
    /// create / update / delete: an annotation lives in `/Annots` and draws from its own
    /// appearance stream, and `FPDFText_*` only ever reads the page content stream — even a
    /// Korean text box bakes its glyphs into the annotation's `/AP`, never into the page.
    pub keeps_text: bool,
}

impl MutateOpts {
    pub fn new(label: &'static str, reason: ChangeReason) -> Self {
        Self {
            label,
            structural: false,
            pages: ChangedPages::Some(Vec::new()),
            reason,
            coalesce: false,
            keeps_text: false,
        }
    }

    pub fn structural(mut self) -> Self {
        self.structural = true;
        self.pages = ChangedPages::All(crate::ipc::types::AllPages::All);
        self
    }

    /// Every page may look different (the document was reloaded from new bytes) but the
    /// page list itself did not change: `changedPages: "all"`, `structure: false`.
    pub fn all_pages(mut self) -> Self {
        self.pages = ChangedPages::All(crate::ipc::types::AllPages::All);
        self
    }

    pub fn page(mut self, page: u16) -> Self {
        self.pages = ChangedPages::Some(vec![page]);
        self
    }

    pub fn pages(mut self, pages: Vec<u16>) -> Self {
        self.pages = ChangedPages::Some(pages);
        self
    }

    pub fn coalesced(mut self) -> Self {
        self.coalesce = true;
        self
    }

    /// See [`MutateOpts::keeps_text`]. Only for edits that provably leave every page's
    /// content stream alone.
    pub fn keeps_text(mut self) -> Self {
        self.keeps_text = true;
        self
    }
}

/// **The only `&mut` path into a document.**
///
/// Stage 1 modules call this with a closure that does their pdfium work; everything around
/// it (undo snapshot, LRU flush, generation bump, cache invalidation, `doc-changed`) is
/// handled here.
///
/// **If `f` returns `Err`, the document is rolled back to the pre-edit snapshot** — not merely
/// left alone. A closure that mutated and then failed (a three-annotation batch whose third
/// `FPDFPage_CreateAnnot` returns null; `apply_redactions` losing its verification pass after
/// it has already deleted objects) used to leave the half-edit in memory under the *old*
/// generation, so the UI kept showing cached tiles of a document that no longer matched the
/// bytes. `mutate` now reloads from the snapshot it took in step 1, the generation is still
/// not bumped, no `doc-changed` is emitted, and the undo entry is discarded without pushing a
/// redo entry — the document never left the state that snapshot describes (STAGE1A §5.2).
///
/// The reload only happens on the error path (≈15 ms for a 1 MB document), so the success
/// path is unchanged. `redact::apply_verified` keeps its own byte snapshot; it is now
/// belt-and-braces rather than the only safety net.
pub fn mutate<'p, T>(
    st: &mut EngineState<'p>,
    doc_id: &str,
    opts: MutateOpts,
    f: impl FnOnce(&mut OpenDoc<'p>) -> Result<T, EngineError>,
) -> Result<T, EngineError> {
    let doc = st
        .docs
        .get_mut(doc_id)
        .ok_or_else(|| EngineError::not_found(format!("unknown document '{doc_id}'")))?;

    // 1. undo snapshot of the *pre-edit* state. The first push after open/save/undo reuses
    //    the bytes we already have and is free.
    let base = if doc.generation == doc.saved_generation && doc.history.undo_depth() == 0 {
        doc.bytes.clone()
    } else {
        doc.to_bytes()?
    };
    let pushed = doc.history.push(opts.label, base.clone(), opts.coalesce)?;

    // 2. structural edits invalidate every open page handle.
    if opts.structural {
        doc.close_pages();
    }

    // 3. the actual work.
    let out = match f(doc) {
        Ok(out) => out,
        Err(e) => {
            // Undo the bookkeeping: the snapshot describes a state we never left.
            if pushed {
                doc.history.discard_last_undo();
            }
            // …and undo the *edit*, which may have got half-way. `replace` drops the page
            // handles and reloads from the snapshot, so whatever the closure changed in
            // PDFium's object tree is gone. The generation is untouched, so no cached tile
            // and no `doc-changed` ever described the half-edited document.
            if let Err(restore) = replace(st, doc_id, base) {
                tracing::error!(
                    doc_id,
                    error = %restore,
                    "rollback after a failed mutation could not reload the snapshot"
                );
                return Err(e.with_detail(format!("rollback failed: {}", restore.message)));
            }
            return Err(e);
        }
    };

    // 4. refresh geometry, bump the generation, invalidate caches.
    //    Page sizes are re-read every time (`page_sizes()` costs 0.1 ms per 14 pages and a
    //    rotate swaps them); the exact `/Rotate` and crop box of the pages the edit names are
    //    re-read from the page itself.
    let refine: Vec<u16> = match &opts.pages {
        ChangedPages::Some(pages) if pages.len() <= 32 => pages.clone(),
        _ => Vec::new(),
    };
    refresh_pages_meta(doc, opts.structural, &refine)?;
    doc.generation += 1;
    let generation = doc.generation;
    match &opts.pages {
        ChangedPages::All(_) => {
            if !opts.keeps_text {
                doc.text.clear();
            }
            doc.annots.clear();
        }
        ChangedPages::Some(pages) => {
            for &p in pages {
                if !opts.keeps_text {
                    doc.text.invalidate_page(p);
                }
                doc.annots.remove(&p);
            }
        }
    }
    announce(st, doc_id, opts.pages, opts.structural, opts.reason, TileDrop::Older(generation))?;
    Ok(out)
}

/// Which cached tiles an [`announce`] throws away.
enum TileDrop {
    /// Everything below this generation (an in-place edit).
    Older(crate::ipc::types::DocGeneration),
    /// Every tile of the document (undo / redo).
    All,
}

/// The tail every mutation shares once the generation is bumped: publish the new
/// [`DocSummary`], drop stale tiles and emit `doc-changed`.
fn announce(
    st: &mut EngineState<'_>,
    doc_id: &str,
    changed_pages: ChangedPages,
    structure: bool,
    reason: ChangeReason,
    tiles: TileDrop,
) -> Result<(), EngineError> {
    let doc = st.doc(doc_id)?;
    let payload = DocChangedPayload {
        doc_id: doc.doc_id.clone(),
        doc_generation: doc.generation,
        changed_pages,
        structure,
        dirty: doc.dirty(),
        reason,
        can_undo: doc.history.can_undo(),
        can_redo: doc.history.can_redo(),
    };
    let summary = doc.summary();

    st.shared.docs.write().insert(doc_id.to_string(), summary);
    match tiles {
        TileDrop::Older(generation) => st.shared.tiles.drop_older_generations(doc_id, generation),
        TileDrop::All => st.shared.tiles.drop_document(doc_id),
    }
    if let Some(app) = &st.app {
        let _ = app.emit("doc-changed", payload);
    }
    Ok(())
}

/// A mutation that rewrites the **serialised file** instead of PDFium's object tree — the
/// path for what PDFium can read but not write (`/Info`, XMP; P1-1). One call = one undo step,
/// exactly like [`mutate`]:
///
/// 1. undo snapshot of the pre-edit state (discarded again on any error);
/// 2. `save::serialize` — `/AP` generation and form kill-focus included, so nothing pending
///    is lost by the reload;
/// 3. `f(bytes, doc)` returns the rewritten file;
/// 4. `save::verify_bytes` reopens it (same password, same page count) — a rewrite that
///    PDFium cannot read never replaces the document;
/// 5. [`replace`] (which re-reads `meta`, permissions, pages and clears every per-document
///    cache), generation + 1, `doc-changed` with `changedPages: "all"`.
///
/// Steps 2–4 leave the open document as it was, so there is nothing to roll back when they
/// fail; a failing [`replace`] reloads the snapshot like [`mutate`] does.
pub fn mutate_bytes(
    st: &mut EngineState<'_>,
    doc_id: &str,
    opts: MutateOpts,
    f: impl FnOnce(&[u8], &OpenDoc<'_>) -> Result<Vec<u8>, EngineError>,
) -> Result<DocInfo, EngineError> {
    let doc = st
        .docs
        .get_mut(doc_id)
        .ok_or_else(|| EngineError::not_found(format!("unknown document '{doc_id}'")))?;
    let base = if doc.generation == doc.saved_generation && doc.history.undo_depth() == 0 {
        doc.bytes.clone()
    } else {
        doc.to_bytes()?
    };
    let pushed = doc.history.push(opts.label, base.clone(), opts.coalesce)?;

    let rewritten = (|| {
        let current = super::save::serialize(st, doc_id)?;
        let doc = st.doc(doc_id)?;
        let (pages, password) = (doc.page_count(), doc.password.clone());
        let out = f(&current, doc)?;
        super::save::verify_bytes(st, &out, pages, password)?;
        replace(st, doc_id, Arc::from(out.into_boxed_slice()))
    })();
    if let Err(e) = rewritten {
        let doc = st.doc_mut(doc_id)?;
        if pushed {
            doc.history.discard_last_undo();
        }
        // Only a failed `replace` can have touched the open document; reloading the
        // snapshot is cheap and harmless in the other cases.
        if let Err(restore) = replace(st, doc_id, base) {
            return Err(e.with_detail(format!("rollback failed: {}", restore.message)));
        }
        return Err(e);
    }

    let doc = st.doc_mut(doc_id)?;
    doc.generation += 1;
    let generation = doc.generation;
    announce(st, doc_id, opts.pages, opts.structural, opts.reason, TileDrop::Older(generation))?;
    Ok(st.doc(doc_id)?.info())
}

/// Replaces a document's bytes in place, keeping its id, path and history (undo / redo and
/// the post-save reload). Pages are dropped first, then the old document.
pub fn replace<'p>(
    st: &mut EngineState<'p>,
    doc_id: &str,
    bytes: Arc<[u8]>,
) -> Result<(), EngineError> {
    let pdfium = st.pdfium;
    let doc = st
        .docs
        .get_mut(doc_id)
        .ok_or_else(|| EngineError::not_found(format!("unknown document '{doc_id}'")))?;

    doc.close_pages();
    let password = doc.password.clone();
    // Load first (it does not touch the old document), then swap and drop the old one.
    let loaded = pdfium
        .load_pdf_from_byte_vec(bytes.to_vec(), password.as_deref())
        .map_err(|e| EngineError::pdfium("reload document", e))?;
    let form = loaded.form().map(|f| f.raw_handle());
    let permissions = read_permissions(doc.bindings, &loaded);
    let meta = read_metadata(doc.bindings, &loaded);
    let pdf_version = version_string(loaded.version());
    let tagged = loaded.catalog().is_tagged();
    let has_outline = loaded.bookmarks().root().is_some();
    let pages_meta = read_pages_meta(doc.bindings, &loaded)?;

    let old = std::mem::replace(&mut doc.doc, loaded);
    drop(old);

    if let Some(form) = form {
        raw::form::set_field_highlight_alpha(doc.bindings, form, 0);
    }
    doc.form = form;
    doc.pages.set_form(form);
    doc.bytes = bytes;
    doc.pages_meta = pages_meta;
    doc.permissions = permissions;
    doc.meta = meta;
    doc.pdf_version = pdf_version;
    doc.tagged = tagged;
    doc.has_outline = has_outline;
    doc.text.clear();
    doc.annots.clear();
    doc.touched.clear();
    // The `FPDF_FONT` belonged to the document we just dropped.
    doc.hangul_font = None;

    for i in 0..refine_count(doc.page_count()) {
        let _ = doc.page(i);
    }
    Ok(())
}

/// `undo` / `redo` (contract §7.8). Pops a snapshot, replaces the document, bumps the
/// generation and broadcasts `doc-changed`.
pub fn undo(st: &mut EngineState<'_>, doc_id: &str, redo: bool) -> Result<DocInfo, EngineError> {
    let doc = st
        .docs
        .get_mut(doc_id)
        .ok_or_else(|| EngineError::not_found(format!("unknown document '{doc_id}'")))?;
    let current = doc.to_bytes()?;
    let popped = if redo {
        doc.history.take_redo(current)?
    } else {
        doc.history.take_undo(current)?
    };
    let Some((label, bytes)) = popped else {
        return Err(EngineError::invalid(if redo {
            "nothing to redo"
        } else {
            "nothing to undo"
        }));
    };
    replace(st, doc_id, bytes)?;

    let doc = st.doc_mut(doc_id)?;
    doc.generation += 1;
    let info = doc.info();
    tracing::debug!(doc_id, label, redo, "history step applied");
    announce(
        st,
        doc_id,
        ChangedPages::All(crate::ipc::types::AllPages::All),
        true,
        if redo {
            ChangeReason::Redo
        } else {
            ChangeReason::Undo
        },
        TileDrop::All,
    )?;
    Ok(info)
}

// ---------------------------------------------------------------------------------------
// metadata helpers
// ---------------------------------------------------------------------------------------

/// Re-reads the page list after an edit.
///
/// `page_sizes()` needs no page load and already reflects `/Rotate`, but the exact rotation
/// value and crop box do need one, so they are kept from the previous pass when the page list
/// itself did not change, and re-read for the pages the edit names.
fn refresh_pages_meta(
    doc: &mut OpenDoc<'_>,
    structural: bool,
    refine: &[u16],
) -> Result<(), EngineError> {
    let mut fresh = read_pages_meta(doc.bindings, &doc.doc)?;
    if !structural {
        // The page list did not change, so the geometry already read for each index is still
        // about the same page; keep it rather than falling back to the provisional values.
        for (index, geom) in fresh.iter_mut().enumerate() {
            if let Some(previous) = doc.pages_meta.get(index) {
                geom.rotation = previous.rotation;
                geom.crop = previous.crop;
                if geom.label.is_none() {
                    geom.label = previous.label.clone();
                }
            }
        }
    }
    doc.pages_meta = fresh;

    let mut to_refine: Vec<u16> = refine.to_vec();
    if structural {
        to_refine.extend(0..refine_count(doc.page_count()));
    }
    to_refine.sort_unstable();
    to_refine.dedup();
    for page in to_refine {
        if page < doc.page_count() {
            // Drop the cached handle so `page()` treats it as fresh and re-reads its
            // geometry. The *handle* only: whether the text changed is `mutate`'s call.
            doc.invalidate_page_handle(page);
            let _ = doc.page(page)?;
        }
    }
    Ok(())
}

/// The fast open path: `FPDF_GetPageSizeByIndexF` for every page (0.1 ms per 14 pages) plus
/// the document-level page labels. Rotation and crop are provisional; [`OpenDoc::page`]
/// refines them when a page is first opened.
fn read_pages_meta(
    bindings: &dyn PdfiumLibraryBindings,
    doc: &PdfDocument<'_>,
) -> Result<Vec<PageGeom>, EngineError> {
    let sizes = doc.pages().page_sizes().ctx("page_sizes")?;
    Ok(sizes
        .iter()
        .enumerate()
        .map(|(i, size)| {
            let width = size.width().value;
            let height = size.height().value;
            PageGeom {
                index: i as u16,
                width_pt: width,
                height_pt: height,
                rotation: 0,
                crop: Rect::new(0.0, 0.0, width, height),
                label: raw::doc::page_label(bindings, doc, i as u16),
            }
        })
        .collect())
}

/// Exact geometry, from a loaded page.
fn geom_from_page(index: u16, page: &PdfPage<'_>) -> Option<PageGeom> {
    let rotation = page
        .rotation()
        .map(|r| r.as_degrees() as i32)
        .unwrap_or(0)
        .rem_euclid(360) as u16;
    let bounds = page
        .boundaries()
        .crop()
        .or_else(|_| page.boundaries().media())
        .ok()?
        .bounds;
    Some(PageGeom {
        index,
        width_pt: page.width().value,
        height_pt: page.height().value,
        rotation,
        crop: Rect::new(
            bounds.left().value,
            bounds.bottom().value,
            bounds.right().value,
            bounds.top().value,
        ),
        label: page.label().map(str::to_owned),
    })
}

/// `FPDF_GetSecurityHandlerRevision`: -1 when the document is not encrypted, else the
/// standard security handler's `/R` (2, 3, 4, or 5 / 6 for AES-256).
///
/// Read raw because pdfium-render's `PdfSecurityHandlerRevision` stops at R4 and returns
/// `Err` for R5 / R6 — which made every AES-256 file (our own `set_password` output
/// included) look unencrypted with every permission granted.
fn security_revision(bindings: &dyn PdfiumLibraryBindings, doc: &PdfDocument<'_>) -> i32 {
    // SAFETY: a live document handle on the engine thread.
    unsafe { bindings.FPDF_GetSecurityHandlerRevision(doc.raw_handle()) }
}

/// The permission flags the **current** open grants (an owner-password open gets all bits).
///
/// ISO 32000-2 table 22, bit n = `1 << (n - 1)`: 3 print, 4 modify, 5 copy / extract,
/// 6 annotate (R2: and fill forms), 9 fill forms (R ≥ 3), 11 assemble (R ≥ 3; R2 folds it
/// into 4). Bit 10 (accessibility extraction) is deliberately not what `extract_text` means.
fn read_permissions(bindings: &dyn PdfiumLibraryBindings, doc: &PdfDocument<'_>) -> Permissions {
    let revision_raw = security_revision(bindings, doc);
    if revision_raw == -1 {
        return Permissions::default();
    }
    // SAFETY: as above.
    let bits = unsafe { bindings.FPDF_GetDocPermissions(doc.raw_handle()) } as u32;
    let bit = |n: u32| bits & (1 << (n - 1)) != 0;
    let revision = match revision_raw {
        2 => SecurityRevision::R2,
        3 => SecurityRevision::R3,
        4 => SecurityRevision::R4,
        // R5 / R6 (AES-256) have no value in the contract's union yet.
        _ => SecurityRevision::Unknown,
    };
    let r2 = revision_raw == 2;
    Permissions {
        print: bit(3),
        modify: bit(4),
        extract_text: bit(5),
        annotate: bit(6),
        fill_forms: if r2 { bit(6) } else { bit(9) || bit(6) },
        assemble: if r2 { bit(4) } else { bit(11) },
        revision,
    }
}

fn read_metadata(bindings: &dyn PdfiumLibraryBindings, doc: &PdfDocument<'_>) -> DocMeta {
    let get = |tag: &str| raw::doc::meta_text(bindings, doc, tag);
    DocMeta {
        title: get("Title"),
        author: get("Author"),
        subject: get("Subject"),
        keywords: get("Keywords"),
        creator: get("Creator"),
        producer: get("Producer"),
        created: get("CreationDate"),
        modified: get("ModDate"),
    }
}

fn version_string(v: PdfDocumentVersion) -> String {
    match v {
        PdfDocumentVersion::Pdf1_0 => "1.0",
        PdfDocumentVersion::Pdf1_1 => "1.1",
        PdfDocumentVersion::Pdf1_2 => "1.2",
        PdfDocumentVersion::Pdf1_3 => "1.3",
        PdfDocumentVersion::Pdf1_4 => "1.4",
        PdfDocumentVersion::Pdf1_5 => "1.5",
        PdfDocumentVersion::Pdf1_6 => "1.6",
        PdfDocumentVersion::Pdf1_7 => "1.7",
        PdfDocumentVersion::Pdf2_0 => "2.0",
        _ => "unknown",
    }
    .to_string()
}
