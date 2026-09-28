//! Bounded cache of open `PdfPage` handles.
//!
//! `PdfPages::get()` calls `FPDF_LoadPage` every time (0.46–26 ms, render spike §1), so the
//! registry keeps up to [`DEFAULT_CAPACITY`] pages open. Two invariants are enforced here
//! rather than by convention:
//!
//! 1. every page is opened with `FORM_OnAfterLoadPage` (when the document has a form) and
//!    `PdfPageContentRegenerationStrategy::Manual` (annotations spike §5.10);
//! 2. every eviction runs `FORM_OnBeforeClosePage` **before** the `PdfPage` drops, and the
//!    whole LRU is flushed before its document drops (`PdfPage<'a>` borrows the `Pdfium`
//!    lifetime, not the document, so the compiler does not catch that use-after-free).

use crate::engine::raw;
use crate::ipc::error::PdfiumResultExt;
use crate::ipc::EngineError;
use pdfium_render::prelude::{
    PdfDocument, PdfPage, PdfPageContentRegenerationStrategy, PdfPageIndex, PdfiumLibraryBindings,
    FPDF_FORMHANDLE,
};
use std::collections::{HashMap, VecDeque};

/// ~1–2 MiB of pdfium state per open page; 24 keeps a two-page spread plus prefetch.
pub const DEFAULT_CAPACITY: usize = 24;

pub struct PageLru<'p> {
    capacity: usize,
    pages: HashMap<u16, PdfPage<'p>>,
    /// Least-recently used at the front.
    order: VecDeque<u16>,
    bindings: &'static dyn PdfiumLibraryBindings,
    form: Option<FPDF_FORMHANDLE>,
}

impl<'p> PageLru<'p> {
    pub fn new(
        bindings: &'static dyn PdfiumLibraryBindings,
        form: Option<FPDF_FORMHANDLE>,
    ) -> Self {
        Self {
            capacity: DEFAULT_CAPACITY,
            pages: HashMap::new(),
            order: VecDeque::new(),
            bindings,
            form,
        }
    }

    pub fn len(&self) -> usize {
        self.pages.len()
    }

    pub fn is_empty(&self) -> bool {
        self.pages.is_empty()
    }

    pub fn contains(&self, index: u16) -> bool {
        self.pages.contains_key(&index)
    }

    /// Replaces the form handle after a `registry::replace` (the old document is gone).
    pub fn set_form(&mut self, form: Option<FPDF_FORMHANDLE>) {
        self.form = form;
    }

    /// Opens `index` (or returns the cached handle) and marks it most-recently-used.
    pub fn get_or_open<'s>(
        &'s mut self,
        doc: &PdfDocument<'p>,
        index: u16,
    ) -> Result<&'s mut PdfPage<'p>, EngineError> {
        if !self.pages.contains_key(&index) {
            let count = doc.pages().len();
            if (index as PdfPageIndex) >= count {
                return Err(
                    EngineError::not_found(format!("page {index} of {count}")).with_page(index)
                );
            }
            let mut page = doc
                .pages()
                .get(index as PdfPageIndex)
                .ctx(&format!("load page {index}"))?;
            // Annotation and form work must never regenerate the page content stream
            // (the default strategy rewrites it on every annotation create).
            page.set_content_regeneration_strategy(PdfPageContentRegenerationStrategy::Manual);
            if let Some(form) = self.form {
                raw::form::on_after_load_page(self.bindings, &page, form);
            }
            self.evict_to(self.capacity.saturating_sub(1));
            self.pages.insert(index, page);
            self.order.push_back(index);
        } else {
            self.touch(index);
        }
        Ok(self
            .pages
            .get_mut(&index)
            .expect("page was just inserted or already present"))
    }

    fn touch(&mut self, index: u16) {
        if let Some(pos) = self.order.iter().position(|&i| i == index) {
            self.order.remove(pos);
        }
        self.order.push_back(index);
    }

    fn evict_to(&mut self, target: usize) {
        while self.pages.len() > target {
            let Some(victim) = self.order.pop_front() else {
                break;
            };
            self.close(victim);
        }
    }

    fn close(&mut self, index: u16) {
        if let Some(page) = self.pages.remove(&index) {
            if let Some(form) = self.form {
                raw::form::on_before_close_page(self.bindings, &page, form);
            }
            drop(page);
        }
    }

    /// Drops one page (after a rotate or an edit that invalidated its handle).
    pub fn invalidate(&mut self, index: u16) {
        if let Some(pos) = self.order.iter().position(|&i| i == index) {
            self.order.remove(pos);
        }
        self.close(index);
    }

    /// Drops **every** open page. Called before any structural change (a raw
    /// `FPDF_MovePages` / `FPDF_ImportPagesByIndex` bypasses pdfium-render's
    /// `PdfPageIndexCache`), before `registry::replace` and before the document drops.
    pub fn clear(&mut self) {
        while let Some(index) = self.order.pop_front() {
            self.close(index);
        }
        // Anything left had no order entry; close it too rather than dropping silently.
        let leftovers: Vec<u16> = self.pages.keys().copied().collect();
        for index in leftovers {
            self.close(index);
        }
    }
}

impl Drop for PageLru<'_> {
    fn drop(&mut self) {
        self.clear();
    }
}
