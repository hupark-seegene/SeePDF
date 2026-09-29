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
/// The most pages a document may have: every page index is a `u16`.
pub const MAX_PAGES: usize = u16::MAX as usize;

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
    /// Stage 8 `open_document { displayName }`: what [`OpenDoc::name`] reports instead of the
    /// file name, so a recovered copy (`<uuid>.pdf`) shows the original document's name.
    pub display_name: Option<String>,
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
    /// A running `compress_estimate` (its scratch copy of the document). Independent of `doc`,
    /// so its place in the drop order does not matter.
    pub compress_work: Option<crate::engine::compress::Work<'p>>,
    /// The finished `compress_estimate` result waiting for `compress_apply` /
    /// `compress_discard`. At most one per document; dropped by [`replace`] and on close.
    pub compress_pending: Option<crate::engine::compress::Pending>,
    /// Stage 5 autosave: this document's id in the recovery directory, assigned on the first
    /// `write_recovery` and kept for the document's lifetime (reloads keep it). `close` does
    /// not delete the files — `clear_recovery` does.
    pub recovery_id: Option<String>,
    /// `list_annotations` stamped a `/NM` on an annotation that had none since `bytes` was
    /// loaded, so the in-memory document is no longer `bytes`: the next [`mutate`] must
    /// snapshot `to_bytes()`, or undoing it would reload the file and hand out new ids.
    pub ids_stamped: bool,
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
        if let Some(name) = self.display_name.as_ref().filter(|n| !n.trim().is_empty()) {
            return name.clone();
        }
        self.path
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "Untitled.pdf".to_string())
    }

    /// [`OpenDoc::name`] without its extension — the `{{filename}}` stamp token.
    pub fn name_stem(&self) -> String {
        let name = self.name();
        std::path::Path::new(&name)
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or(name)
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
            page_labels: self.page_labels(),
        }
    }

    /// `DocInfo.pageLabels`: every page's label (`""` where a page has none), or `None` when
    /// no page of the document has a label.
    pub fn page_labels(&self) -> Option<Vec<String>> {
        if self.pages_meta.iter().all(|p| p.label.is_none()) {
            return None;
        }
        Some(
            self.pages_meta
                .iter()
                .map(|p| p.label.clone().unwrap_or_default())
                .collect(),
        )
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

    /// The document's outline as the contract's nested node list, depth-first.
    ///
    /// Read with the raw `FPDFBookmark_*` API (`raw::outline::read`) because pdfium-render
    /// hides the sign of `/Count` (open / closed, P2) and has no URI accessor for a node. The
    /// destination mapping is unchanged since Stage 2 (`STAGE1C_NOTES.md` §7.1): document-level
    /// calls only, no `FPDF_LoadPage`, so it stays affordable for every node at open.
    pub fn outline(&self) -> Vec<OutlineNode> {
        raw::outline::read(self.bindings, &self.doc)
    }
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
    open_named(st, path, bytes, password, None)
}

/// [`open`] with `open_document`'s `displayName` (Stage 8): when given (and not blank), the
/// document reports it as `DocInfo.name` — used for recovered copies, whose file is
/// `<uuid>.pdf` but whose title must be the original document's name.
pub fn open_named<'p>(
    st: &mut EngineState<'p>,
    path: Option<PathBuf>,
    bytes: Vec<u8>,
    password: Option<String>,
    display_name: Option<String>,
) -> Result<DocInfo, EngineError> {
    let display_name = display_name.filter(|n| !n.trim().is_empty());
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
        .map(|f| {
            matches!(
                f.form_type(),
                PdfFormType::XfaFull | PdfFormType::XfaForeground
            )
        })
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
        display_name,
        password,
        generation: 1,
        saved_generation: 1,
        pages_meta,
        touched: BTreeSet::new(),
        history: History::shared(spill_dir, byte_len, st.history_budget.clone()),
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
        compress_work: None,
        compress_pending: None,
        recovery_id: None,
        ids_stamped: false,
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
        return Err(EngineError::not_found(format!(
            "unknown document '{doc_id}'"
        )));
    };
    if let Some(form) = doc.form {
        raw::form::force_to_kill_focus(doc.bindings, form);
    }
    doc.close_pages();
    doc.history.clear();
    st.shared.docs.write().remove(doc_id);
    st.shared.tiles.drop_document(doc_id);
    #[cfg(target_os = "macos")]
    if let Some(app) = &st.app {
        crate::app::menu::forget_document(app, doc_id);
    }
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
    let base = pre_edit_bytes(doc)?;
    let pushed = doc.history.push(opts.label, base.clone(), opts.coalesce)?;

    // 2. structural edits invalidate every open page handle.
    if opts.structural {
        doc.close_pages();
    }

    // 3. the actual work, then the new page list — which can refuse the result (a page count
    //    past `u16::MAX`), and that is rolled back like any other failure.
    let refine: Vec<u16> = match &opts.pages {
        ChangedPages::Some(pages) if pages.len() <= 32 => pages.clone(),
        _ => Vec::new(),
    };
    let result =
        f(doc).and_then(|out| refresh_pages_meta(doc, opts.structural, &refine).map(|()| out));
    let out = match result {
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

    // 4. bump the generation, invalidate caches. (Geometry was refreshed in step 3: page
    //    sizes are re-read every time — `page_sizes()` costs 0.1 ms per 14 pages and a rotate
    //    swaps them — and the exact `/Rotate` and crop box of the pages the edit names are
    //    re-read from the page itself.)
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
    announce(
        st,
        doc_id,
        opts.pages,
        opts.structural,
        opts.reason,
        TileDrop::Older(generation),
    )?;
    Ok(out)
}

/// The undo snapshot of the document as it is now. Right after open / save / undo the
/// document *is* `bytes`, so that is free — unless `list_annotations` has since stamped ids
/// into it, which only `to_bytes()` (5 ms/MB) captures.
fn pre_edit_bytes(doc: &OpenDoc<'_>) -> Result<Arc<[u8]>, EngineError> {
    if doc.generation == doc.saved_generation && doc.history.undo_depth() == 0 && !doc.ids_stamped {
        Ok(doc.bytes.clone())
    } else {
        doc.to_bytes()
    }
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
    #[cfg(target_os = "macos")]
    let labels = (doc.history.undo_label(), doc.history.redo_label());

    st.shared.docs.write().insert(doc_id.to_string(), summary);
    match tiles {
        TileDrop::Older(generation) => st.shared.tiles.drop_older_generations(doc_id, generation),
        TileDrop::All => st.shared.tiles.drop_document(doc_id),
    }
    if let Some(app) = &st.app {
        let _ = app.emit("doc-changed", payload);
        // Stage 8: the native 편집 menu names the step (실행 취소: 워터마크) for the focused
        // window's document.
        #[cfg(target_os = "macos")]
        crate::app::menu::history_changed(app, doc_id, labels.0, labels.1);
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
    mutate_bytes_checked(st, doc_id, opts, f, |_, _| Ok(()))
}

/// [`mutate_bytes`] with a structural check of the rewrite (P2): `check` runs on the
/// PDFium-reopened bytes in step 4, **before** anything replaces the open document, so a
/// rewrite PDFium reads differently from what was meant (an outline whose `/Count` or links
/// it does not follow, a `/PageLabels` tree it labels differently, a link whose `/Dest` it
/// cannot resolve) fails with the check's error — by convention `verifyFailed` — the undo
/// entry is dropped and the document is exactly as it was.
pub fn mutate_bytes_checked(
    st: &mut EngineState<'_>,
    doc_id: &str,
    opts: MutateOpts,
    f: impl FnOnce(&[u8], &OpenDoc<'_>) -> Result<Vec<u8>, EngineError>,
    check: impl FnOnce(&'static dyn PdfiumLibraryBindings, &PdfDocument<'_>) -> Result<(), EngineError>,
) -> Result<DocInfo, EngineError> {
    mutate_bytes_inner(st, doc_id, opts, None, f, check)
}

// v0.3 pkg2-pages-structure-forms: a byte-level rewrite that changes the page count (insert
// from a file or from another open document, carrying the source's outline / labels / fields).
/// [`mutate_bytes_checked`] for a rewrite that **changes the page count**: step 4 expects
/// `expected_pages` instead of the current count. Everything else — one undo step, rollback,
/// [`replace`], `doc-changed` — is identical; pass [`MutateOpts::structural`].
pub fn mutate_bytes_resized(
    st: &mut EngineState<'_>,
    doc_id: &str,
    opts: MutateOpts,
    expected_pages: u16,
    f: impl FnOnce(&[u8], &OpenDoc<'_>) -> Result<Vec<u8>, EngineError>,
    check: impl FnOnce(&'static dyn PdfiumLibraryBindings, &PdfDocument<'_>) -> Result<(), EngineError>,
) -> Result<DocInfo, EngineError> {
    mutate_bytes_inner(st, doc_id, opts, Some(expected_pages), f, check)
}

fn mutate_bytes_inner(
    st: &mut EngineState<'_>,
    doc_id: &str,
    opts: MutateOpts,
    expected_pages: Option<u16>,
    f: impl FnOnce(&[u8], &OpenDoc<'_>) -> Result<Vec<u8>, EngineError>,
    check: impl FnOnce(&'static dyn PdfiumLibraryBindings, &PdfDocument<'_>) -> Result<(), EngineError>,
) -> Result<DocInfo, EngineError> {
    let doc = st
        .docs
        .get_mut(doc_id)
        .ok_or_else(|| EngineError::not_found(format!("unknown document '{doc_id}'")))?;
    let base = pre_edit_bytes(doc)?;
    let pushed = doc.history.push(opts.label, base.clone(), opts.coalesce)?;

    let rewritten = (|| {
        let current = super::save::serialize(st, doc_id)?;
        let doc = st.doc(doc_id)?;
        let pages = expected_pages.unwrap_or(doc.page_count());
        let password = doc.password.clone();
        let out = f(&current, doc)?;
        super::save::verify_bytes_with(st, &out, pages, password, check)?;
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
    announce(
        st,
        doc_id,
        opts.pages,
        opts.structural,
        opts.reason,
        TileDrop::Older(generation),
    )?;
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
    // A pending compress result describes bytes that are no longer the document's.
    doc.compress_pending = None;
    // The document is exactly `bytes` again.
    doc.ids_stamped = false;

    for i in 0..refine_count(doc.page_count()) {
        let _ = doc.page(i);
    }
    Ok(())
}

/// `undo` / `redo` (contract §7.8). Loads the snapshot, replaces the document, and only then
/// moves the history stacks, bumps the generation and broadcasts `doc-changed`.
///
/// Transactional: a snapshot that cannot be read (a spilled file the OS purged) or a replace
/// that fails leaves both stacks, the generation and the document exactly as they were — the
/// step is not consumed, and no redo entry appears for a state the document never left.
pub fn undo(st: &mut EngineState<'_>, doc_id: &str, redo: bool) -> Result<DocInfo, EngineError> {
    let doc = st
        .docs
        .get_mut(doc_id)
        .ok_or_else(|| EngineError::not_found(format!("unknown document '{doc_id}'")))?;
    let current = doc.to_bytes()?;
    let Some(step) = doc.history.prepare(redo, current)? else {
        return Err(EngineError::invalid(if redo {
            "nothing to redo"
        } else {
            "nothing to undo"
        }));
    };
    if let Err(e) = replace(st, doc_id, step.bytes()) {
        st.doc_mut(doc_id)?.history.abandon(step);
        return Err(e);
    }

    let doc = st.doc_mut(doc_id)?;
    let label = doc.history.commit(step);
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
    // Page indices are `u16` everywhere (contract `PageIndex`): past 65,535 they would wrap
    // (`len() as u16` made 65,536 pages count as 0). Refuse the document — or, through
    // `mutate`'s rollback, the edit that would get it there.
    if sizes.len() > MAX_PAGES {
        return Err(EngineError::new(
            ErrorCode::Unsupported,
            format!(
                "the document has {} pages; SeePDF handles at most {MAX_PAGES}",
                sizes.len()
            ),
        ));
    }
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
pub fn geom_from_page(index: u16, page: &PdfPage<'_>) -> Option<PageGeom> {
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
    // `unsigned long` is 64 bits on macOS and 32 on Windows; /P is a 32-bit field either way.
    #[allow(clippy::unnecessary_cast)]
    let bits = unsafe { bindings.FPDF_GetDocPermissions(doc.raw_handle()) } as u32;
    let bit = |n: u32| bits & (1 << (n - 1)) != 0;
    let revision = match revision_raw {
        2 => SecurityRevision::R2,
        3 => SecurityRevision::R3,
        4 => SecurityRevision::R4,
        5 => SecurityRevision::R5,
        6 => SecurityRevision::R6,
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
