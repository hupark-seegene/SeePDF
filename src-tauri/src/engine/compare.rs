//! Compare two documents — a word diff per page pair (P1-6, `IPC_CONTRACT.md` §7.6b).
//!
//! Both documents are already open in the registry. [`begin`] validates the request;
//! [`dispatch`] queues one `Lane::Background` command per row (so Cancel is observed between
//! rows and the viewer's tiles interleave) plus a final command that assembles the
//! [`CompareReport`] and sends it on the `done` event. A document closed mid-job ends the
//! job with `cancelled` (every command checks both documents first).
//!
//! ## Rows (Stage 8 `alignPages`, default on)
//!
//! With `alignPages: false` the candidate lists pair by position, as in Stage 5. Otherwise one
//! scan command per candidate page collects its words, an align command pairs the pages by
//! word-set similarity ([`align`]: identical head / tail pair directly, the middle is a
//! Needleman–Wunsch with free gaps) and then queues the row diffs, which reuse the scanned
//! words. An inserted or deleted page becomes a null-sided row — `changed: true` even when the
//! page has no words — instead of shifting every later pair.
//!
//! ## Words
//!
//! Each page's text comes from the engine text layer ([`layer::layer`]). A word is a maximal
//! run of characters that are neither Unicode whitespace nor PDFium-generated (the synthetic
//! spaces and `\r\n` PDFium inserts between text runs), so "whitespace runs are always
//! normalised" falls out of the tokenisation. With `ignoreCase` each char goes through
//! [`layer::fold`] before comparison; the reported text is always the original.
//!
//! ## Diff
//!
//! [`diff`] is Myers' O(ND) greedy algorithm on interned word ids, after trimming the common
//! prefix and suffix. The trace keeps one `V` slice per edit distance (≈ D² ints), so it is
//! capped at [`MAX_EDIT_DISTANCE`]; a pair more different than that (two unrelated 2,000-word
//! pages) is reported as one `replace` over the untrimmed middle, which is what a human would
//! call it anyway. Adjacent delete / insert runs collapse into one `replace`.
//!
//! ## Rects
//!
//! A changed run's rects are [`TextLayer::range_rects`] over the code-point span from its first
//! word's first char to its last word's last char: one rect per line fragment, so a run that
//! wraps yields one rect per line.

use crate::engine::export::job::JobReporter;
use crate::engine::jobs::JobToken;
use crate::engine::text::layer::{self, fold, TextLayer, FLAG_GENERATED};
use crate::engine::types::{CmdStatus, EngineState};
use crate::engine::{EngineHandle, Lane, Submit};
use crate::ipc::types::{
    CompareOptions, ComparePage, CompareReport, DiffKind, DiffOp, DocId, PageIndex, Rect,
};
use crate::ipc::{EngineError, ErrorCode};
use parking_lot::Mutex;
use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;
use std::time::Instant;

/// Myers' trace costs ≈ D² `isize`s; 2,000 edits is ≈ 32 MB at worst.
pub const MAX_EDIT_DISTANCE: usize = 2000;

/// One row of the report: which page of each document (either may be absent).
pub type Pair = (Option<PageIndex>, Option<PageIndex>);

/// One row as positions in the candidate lists (`pagesA` / `pagesB`).
pub type Row = (Option<usize>, Option<usize>);

/// Above this many DP cells (candidate pages A × B, after the identical head and tail are
/// trimmed) the alignment falls back to positional pairing for the middle: each cell costs a
/// word-set Jaccard, and 250 000 of them is ≈ 0.2 s on the engine thread.
pub const MAX_ALIGN_CELLS: usize = 250_000;

/// A running compare. Shared by the job's commands; nothing lives on the `OpenDoc`s.
pub struct Work {
    pub doc_a: DocId,
    pub doc_b: DocId,
    /// The candidate pages of each side, in order.
    pub pages_a: Vec<PageIndex>,
    pub pages_b: Vec<PageIndex>,
    pub ignore_case: bool,
    /// Stage 8 `alignPages`.
    pub align: bool,
    started: Instant,
    /// Report rows: known at [`begin`] for positional pairing, set by the align step otherwise.
    rows: Mutex<Vec<Row>>,
    /// `alignPages`: each candidate page's words, from the scan steps (taken by the row diffs).
    words_a: Mutex<Vec<Option<PageWords>>>,
    words_b: Mutex<Vec<Option<PageWords>>>,
    results: Mutex<Vec<Option<ComparePage>>>,
}

impl Work {
    /// The report rows known so far — all of them for positional pairing, none before the
    /// align step otherwise.
    pub fn pairs(&self) -> Vec<Pair> {
        self.rows
            .lock()
            .iter()
            .map(|&(a, b)| (a.map(|i| self.pages_a[i]), b.map(|i| self.pages_b[i])))
            .collect()
    }

    /// The job's initial unit count. Positional pairing: one per row. Aligning: one per
    /// candidate page (the scans) plus a lower bound of the rows (every page is in exactly one
    /// row, so there are at least `max(|A|, |B|)`); the align step raises it to the exact
    /// figure with `JobReporter::set_total`.
    pub fn initial_total(&self) -> u32 {
        if self.align {
            let (a, b) = (self.pages_a.len(), self.pages_b.len());
            (a + b + a.max(b)) as u32
        } else {
            self.rows.lock().len() as u32
        }
    }
}

/// Validates the request and returns the job's shared state.
pub fn begin(
    st: &EngineState<'_>,
    doc_a: &str,
    doc_b: &str,
    options: &CompareOptions,
) -> Result<Arc<Work>, EngineError> {
    let count_a = st.doc(doc_a)?.page_count();
    let count_b = st.doc(doc_b)?.page_count();
    if doc_a == doc_b {
        return Err(EngineError::invalid("docA and docB are the same document"));
    }
    let pages = |list: &Option<Vec<PageIndex>>, count: u16, side: &str| {
        let pages: Vec<PageIndex> = match list {
            None => (0..count).collect(),
            Some(list) => list.clone(),
        };
        if let Some(&bad) = pages.iter().find(|&&p| p >= count) {
            return Err(EngineError::invalid(format!(
                "pages{side}: page {bad} is out of range (page count {count})"
            ))
            .with_page(bad));
        }
        Ok(pages)
    };
    let pages_a = pages(&options.pages_a, count_a, "A")?;
    let pages_b = pages(&options.pages_b, count_b, "B")?;
    let align = options.align();
    let rows: Vec<Row> = if align {
        Vec::new()
    } else {
        positional(0..pages_a.len(), 0..pages_b.len())
    };
    let slots = |n: usize, on: bool| -> Vec<Option<PageWords>> {
        if on {
            (0..n).map(|_| None).collect()
        } else {
            Vec::new()
        }
    };
    Ok(Arc::new(Work {
        doc_a: doc_a.to_string(),
        doc_b: doc_b.to_string(),
        results: Mutex::new((0..rows.len()).map(|_| None).collect()),
        rows: Mutex::new(rows),
        words_a: Mutex::new(slots(pages_a.len(), align)),
        words_b: Mutex::new(slots(pages_b.len(), align)),
        pages_a,
        pages_b,
        ignore_case: options.ignore_case,
        align,
        started: Instant::now(),
    }))
}

/// Pairs two position ranges by position; the longer one's extra positions get a null side.
fn positional(a: std::ops::Range<usize>, b: std::ops::Range<usize>) -> Vec<Row> {
    let n = a.len().max(b.len());
    (0..n)
        .map(|k| {
            (
                (k < a.len()).then(|| a.start + k),
                (k < b.len()).then(|| b.start + k),
            )
        })
        .collect()
}

fn submit(token: &JobToken, label: &'static str) -> Submit {
    Submit::new(Lane::Background, label).cancel(token.cancel.clone())
}

/// Queues the job. Positional pairing: one diff command per row, then the report. Aligning:
/// one scan command per candidate page, then the align command, which queues the row diffs
/// and the report itself once it knows the rows.
pub fn dispatch(
    engine: &EngineHandle,
    work: Arc<Work>,
    token: &JobToken,
    reporter: Arc<JobReporter>,
) {
    let queued = if work.align {
        dispatch_scans(engine, &work, token, &reporter)
    } else {
        dispatch_rows(engine, &work, token, &reporter)
    };
    if let Err(e) = queued {
        reporter.fail(e);
    }
}

#[derive(Clone, Copy)]
enum Side {
    A,
    B,
}

fn dispatch_scans(
    engine: &EngineHandle,
    work: &Arc<Work>,
    token: &JobToken,
    reporter: &Arc<JobReporter>,
) -> Result<(), EngineError> {
    for (side, n) in [(Side::A, work.pages_a.len()), (Side::B, work.pages_b.len())] {
        for i in 0..n {
            let (work, reporter) = (work.clone(), reporter.clone());
            engine.dispatch(submit(token, "compare_scan"), move |st, status| {
                if !proceed(&reporter, st, &work, status) {
                    return;
                }
                let (doc, page, slots) = match side {
                    Side::A => (&work.doc_a, work.pages_a[i], &work.words_a),
                    Side::B => (&work.doc_b, work.pages_b[i], &work.words_b),
                };
                match page_words(st, doc, Some(page), work.ignore_case) {
                    Ok(words) => {
                        slots.lock()[i] = Some(words);
                        reporter.step(None, None);
                    }
                    Err(e) => fail(&reporter, e),
                }
            })?;
        }
    }
    let (w, r, e, t) = (
        work.clone(),
        reporter.clone(),
        engine.clone(),
        token.clone(),
    );
    engine.dispatch(submit(token, "compare_align"), move |st, status| {
        if !proceed(&r, st, &w, status) {
            return;
        }
        let rows = align_work(&w);
        let n = rows.len();
        *w.results.lock() = (0..n).map(|_| None).collect();
        *w.rows.lock() = rows;
        r.set_total((w.pages_a.len() + w.pages_b.len() + n) as u32);
        if let Err(err) = dispatch_rows(&e, &w, &t, &r) {
            r.fail(err);
        }
    })?;
    Ok(())
}

fn dispatch_rows(
    engine: &EngineHandle,
    work: &Arc<Work>,
    token: &JobToken,
    reporter: &Arc<JobReporter>,
) -> Result<(), EngineError> {
    let rows = work.rows.lock().len();
    for index in 0..rows {
        let (work, reporter) = (work.clone(), reporter.clone());
        engine.dispatch(submit(token, "compare_page"), move |st, status| {
            if !proceed(&reporter, st, &work, status) {
                return;
            }
            let (ia, ib) = work.rows.lock()[index];
            let page_a = ia.map(|i| work.pages_a[i]);
            let page_b = ib.map(|i| work.pages_b[i]);
            let row = if work.align {
                // Each position is in exactly one row, so its words can be moved out.
                let a = ia
                    .and_then(|i| work.words_a.lock()[i].take())
                    .unwrap_or_default();
                let b = ib
                    .and_then(|i| work.words_b.lock()[i].take())
                    .unwrap_or_default();
                Ok(compare_words(page_a, page_b, &a, &b))
            } else {
                compare_pair(
                    st,
                    &work.doc_a,
                    &work.doc_b,
                    page_a,
                    page_b,
                    work.ignore_case,
                )
            };
            match row {
                Ok(page) => {
                    work.results.lock()[index] = Some(page);
                    reporter.step(Some(index as PageIndex), None);
                }
                Err(e) => fail(&reporter, e),
            }
        })?;
    }
    let (work, reporter) = (work.clone(), reporter.clone());
    engine.dispatch(submit(token, "compare_finish"), move |st, status| {
        if !proceed(&reporter, st, &work, status) {
            return;
        }
        match finish(&work) {
            Ok(report) => reporter.finish_with_compare(report),
            Err(e) => fail(&reporter, e),
        }
    })?;
    Ok(())
}

/// Every job command's preamble. A cancelled job — or one whose document was **closed** while
/// it ran — ends with `cancelled`, not `error`.
fn proceed(reporter: &JobReporter, st: &EngineState<'_>, work: &Work, status: CmdStatus) -> bool {
    if reporter.is_finished() {
        return false;
    }
    if status != CmdStatus::Run || st.doc(&work.doc_a).is_err() || st.doc(&work.doc_b).is_err() {
        reporter.cancel();
        return false;
    }
    true
}

fn fail(reporter: &JobReporter, e: EngineError) {
    if e.code == ErrorCode::Cancelled {
        reporter.cancel();
    } else {
        reporter.fail(e);
    }
}

/// The align step: word sets of every candidate page, interned across both documents.
fn align_work(work: &Work) -> Vec<Row> {
    let words_a = work.words_a.lock();
    let words_b = work.words_b.lock();
    fn set<'k>(ids: &mut HashMap<&'k str, u32>, w: &'k Option<PageWords>) -> Vec<u32> {
        let mut out: Vec<u32> = w.as_ref().map(|w| intern(ids, &w.keys)).unwrap_or_default();
        out.sort_unstable();
        out.dedup();
        out
    }
    let mut ids: HashMap<&str, u32> = HashMap::new();
    let sets_a: Vec<Vec<u32>> = words_a.iter().map(|w| set(&mut ids, w)).collect();
    let sets_b: Vec<Vec<u32>> = words_b.iter().map(|w| set(&mut ids, w)).collect();
    align(&sets_a, &sets_b)
}

/// Jaccard similarity of two sorted, deduplicated sets; two empty pages are identical.
pub fn jaccard<T: Ord>(a: &[T], b: &[T]) -> f64 {
    if a.is_empty() && b.is_empty() {
        return 1.0;
    }
    let (mut i, mut j, mut inter) = (0, 0, 0usize);
    while i < a.len() && j < b.len() {
        match a[i].cmp(&b[j]) {
            std::cmp::Ordering::Less => i += 1,
            std::cmp::Ordering::Greater => j += 1,
            std::cmp::Ordering::Equal => {
                inter += 1;
                i += 1;
                j += 1;
            }
        }
    }
    inter as f64 / (a.len() + b.len() - inter) as f64
}

/// Page alignment (Stage 8 `alignPages`): a monotone pairing of the two page sequences that
/// maximises the summed word-set similarity — Needleman–Wunsch with free gaps. A page that has
/// no counterpart becomes a null-sided row instead of shifting every later pair.
///
/// * Identical leading and trailing pages (equal word sets) pair directly, so one inserted
///   page in a 500-page document costs one DP cell, not 250 000.
/// * Every match scores its Jaccard similarity plus a tiny bonus, so among equally similar
///   alignments the one with more pairs wins: a completely rewritten page still pairs with
///   the page in its place instead of becoming two one-sided rows.
/// * A middle larger than [`MAX_ALIGN_CELLS`] is paired by position (the Stage 5 behaviour).
pub fn align<T: Ord>(a: &[Vec<T>], b: &[Vec<T>]) -> Vec<Row> {
    const MATCH_BONUS: f64 = 1e-6;
    let (n, m) = (a.len(), b.len());
    let prefix = a.iter().zip(b).take_while(|(x, y)| x == y).count();
    let suffix = a[prefix..]
        .iter()
        .rev()
        .zip(b[prefix..].iter().rev())
        .take_while(|(x, y)| x == y)
        .count();
    let (a1, b1) = (n - suffix, m - suffix);
    let (h, w) = (a1 - prefix, b1 - prefix);

    let mut rows: Vec<Row> = (0..prefix).map(|i| (Some(i), Some(i))).collect();
    if h == 0 || w == 0 || h * w > MAX_ALIGN_CELLS {
        rows.extend(positional(prefix..a1, prefix..b1));
    } else {
        // score[i][j]: best total for a[prefix..prefix+i] against b[prefix..prefix+j].
        // step: 0 = pair, 1 = A-only, 2 = B-only.
        let mut score = vec![0f64; (h + 1) * (w + 1)];
        let mut step = vec![0u8; (h + 1) * (w + 1)];
        let at = |i: usize, j: usize| i * (w + 1) + j;
        for i in 1..=h {
            step[at(i, 0)] = 1;
        }
        for j in 1..=w {
            step[at(0, j)] = 2;
        }
        for i in 1..=h {
            for j in 1..=w {
                let pair = score[at(i - 1, j - 1)]
                    + jaccard(&a[prefix + i - 1], &b[prefix + j - 1])
                    + MATCH_BONUS;
                let up = score[at(i - 1, j)];
                let left = score[at(i, j - 1)];
                let (best, s) = if pair >= up && pair >= left {
                    (pair, 0)
                } else if up >= left {
                    (up, 1)
                } else {
                    (left, 2)
                };
                score[at(i, j)] = best;
                step[at(i, j)] = s;
            }
        }
        let mut middle: Vec<Row> = Vec::with_capacity(h + w);
        let (mut i, mut j) = (h, w);
        while i > 0 || j > 0 {
            match step[at(i, j)] {
                0 => {
                    middle.push((Some(prefix + i - 1), Some(prefix + j - 1)));
                    i -= 1;
                    j -= 1;
                }
                1 => {
                    middle.push((Some(prefix + i - 1), None));
                    i -= 1;
                }
                _ => {
                    middle.push((None, Some(prefix + j - 1)));
                    j -= 1;
                }
            }
        }
        middle.reverse();
        rows.extend(middle);
    }
    rows.extend((0..suffix).map(|k| (Some(a1 + k), Some(b1 + k))));
    rows
}

/// Assembles the report once every pair has a result.
pub fn finish(work: &Work) -> Result<CompareReport, EngineError> {
    let results = std::mem::take(&mut *work.results.lock());
    let mut pages = Vec::with_capacity(results.len());
    for (i, page) in results.into_iter().enumerate() {
        pages.push(page.ok_or_else(|| {
            // Only reachable when a pair was dropped without ending the job.
            EngineError::cancelled(format!("compare pair {i} has no result"))
        })?);
    }
    let mut report = CompareReport {
        doc_a: work.doc_a.clone(),
        doc_b: work.doc_b.clone(),
        changed_pages: pages.iter().filter(|p| p.changed).count() as u32,
        inserted: 0,
        deleted: 0,
        pages,
        elapsed_ms: 0.0,
    };
    for op in report.pages.iter().flat_map(|p| p.ops.iter()) {
        match op.kind {
            DiffKind::Equal => {}
            DiffKind::Insert => report.inserted += op.words,
            DiffKind::Delete => report.deleted += op.words,
            DiffKind::Replace => {
                report.deleted += op.words;
                report.inserted += op.text_b.as_deref().map(word_count).unwrap_or(0);
            }
        }
    }
    report.elapsed_ms = work.started.elapsed().as_secs_f64() * 1000.0;
    Ok(report)
}

fn word_count(text: &str) -> u32 {
    text.split(' ').filter(|w| !w.is_empty()).count() as u32
}

/// A page's words: code-point range in the text layer and the (optionally folded) key.
#[derive(Debug, Default)]
pub struct PageWords {
    pub layer: Option<Arc<TextLayer>>,
    /// `(first char, char count)` per word.
    pub spans: Vec<(u32, u32)>,
    pub keys: Vec<String>,
}

impl PageWords {
    fn text(&self, range: Range<usize>) -> String {
        let Some(layer) = &self.layer else {
            return String::new();
        };
        let mut out = String::new();
        for &(first, count) in &self.spans[range] {
            if !out.is_empty() {
                out.push(' ');
            }
            out.extend(
                layer.chars[first as usize..(first + count) as usize]
                    .iter()
                    .map(|c| char::from_u32(c.codepoint).unwrap_or('\u{fffd}')),
            );
        }
        out
    }

    fn rects(&self, range: Range<usize>) -> Vec<Rect> {
        let Some(layer) = &self.layer else {
            return Vec::new();
        };
        if range.is_empty() {
            return Vec::new();
        }
        let (start, _) = self.spans[range.start];
        let (last, last_len) = self.spans[range.end - 1];
        layer.range_rects(start, last + last_len - start)
    }
}

/// Splits a text layer into words (see the module docs).
pub fn tokenize(layer: Arc<TextLayer>, ignore_case: bool) -> PageWords {
    let mut spans = Vec::new();
    let mut keys = Vec::new();
    let mut current: Option<(u32, String)> = None;
    for (i, c) in layer.chars.iter().enumerate() {
        let ch = char::from_u32(c.codepoint).unwrap_or('\u{fffd}');
        let separator = c.flags & FLAG_GENERATED != 0
            || ch.is_whitespace()
            || matches!(ch, '\u{fffe}' | '\u{ffff}' | '\0');
        if separator {
            if let Some((first, key)) = current.take() {
                spans.push((first, i as u32 - first));
                keys.push(key);
            }
            continue;
        }
        let ch = if ignore_case { fold(ch) } else { ch };
        match &mut current {
            Some((_, key)) => key.push(ch),
            None => current = Some((i as u32, ch.to_string())),
        }
    }
    if let Some((first, key)) = current.take() {
        spans.push((first, layer.chars.len() as u32 - first));
        keys.push(key);
    }
    PageWords {
        layer: Some(layer),
        spans,
        keys,
    }
}

fn page_words(
    st: &mut EngineState<'_>,
    doc_id: &str,
    page: Option<PageIndex>,
    ignore_case: bool,
) -> Result<PageWords, EngineError> {
    let Some(page) = page else {
        return Ok(PageWords::default());
    };
    let doc = st.doc_mut(doc_id)?;
    let layer = layer::layer(doc, page).map_err(|e| e.with_page(page))?;
    Ok(tokenize(layer, ignore_case))
}

/// Diffs one page pair. A missing side has no words.
pub fn compare_pair(
    st: &mut EngineState<'_>,
    doc_a: &str,
    doc_b: &str,
    page_a: Option<PageIndex>,
    page_b: Option<PageIndex>,
    ignore_case: bool,
) -> Result<ComparePage, EngineError> {
    let a = page_words(st, doc_a, page_a, ignore_case)?;
    let b = page_words(st, doc_b, page_b, ignore_case)?;
    Ok(compare_words(page_a, page_b, &a, &b))
}

/// [`compare_pair`] after the words are in hand.
pub fn compare_words(
    page_a: Option<PageIndex>,
    page_b: Option<PageIndex>,
    a: &PageWords,
    b: &PageWords,
) -> ComparePage {
    // Intern the keys so the diff compares integers.
    let mut ids: HashMap<&str, u32> = HashMap::new();
    let ids_a = intern(&mut ids, &a.keys);
    let ids_b = intern(&mut ids, &b.keys);
    let hunks = diff(&ids_a, &ids_b);
    let ops: Vec<DiffOp> = hunks
        .into_iter()
        .map(|h| {
            let (ra, rb) = (h.a.clone(), h.b.clone());
            let side_a = !matches!(h.kind, DiffKind::Equal | DiffKind::Insert);
            let side_b = !matches!(h.kind, DiffKind::Equal | DiffKind::Delete);
            DiffOp {
                kind: h.kind,
                words: if h.kind == DiffKind::Insert {
                    rb.len() as u32
                } else {
                    ra.len() as u32
                },
                text_a: side_a.then(|| a.text(ra.clone())),
                text_b: side_b.then(|| b.text(rb.clone())),
                rects_a: side_a.then(|| a.rects(ra)),
                rects_b: side_b.then(|| b.rects(rb)),
            }
        })
        .collect();
    ComparePage {
        page_a,
        page_b,
        // A page present on one side only is a change even when it has no words (an inserted
        // blank page), or 변경된 페이지만 would hide it.
        changed: page_a.is_none() != page_b.is_none()
            || ops.iter().any(|op| op.kind != DiffKind::Equal),
        words_a: a.keys.len() as u32,
        words_b: b.keys.len() as u32,
        ops,
    }
}

fn intern<'k>(ids: &mut HashMap<&'k str, u32>, keys: &'k [String]) -> Vec<u32> {
    keys.iter()
        .map(|k| {
            let next = ids.len() as u32;
            *ids.entry(k.as_str()).or_insert(next)
        })
        .collect()
}

/// One run of the diff: `a` / `b` are index ranges into the two sequences.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hunk {
    pub kind: DiffKind,
    pub a: Range<usize>,
    pub b: Range<usize>,
}

/// Word-level diff of `a` against `b`. Equal runs alternate with change runs; a change run with
/// words on both sides is one `Replace`.
pub fn diff<T: Eq>(a: &[T], b: &[T]) -> Vec<Hunk> {
    // Common prefix / suffix are free and make the Myers part small for typical edits.
    let prefix = a.iter().zip(b).take_while(|(x, y)| x == y).count();
    let suffix = a[prefix..]
        .iter()
        .rev()
        .zip(b[prefix..].iter().rev())
        .take_while(|(x, y)| x == y)
        .count();
    let (mid_a, mid_b) = (&a[prefix..a.len() - suffix], &b[prefix..b.len() - suffix]);

    // Per-element script for the middle: true = equal pair, else a delete or an insert.
    let mut script: Vec<Edit> = Vec::with_capacity(mid_a.len() + mid_b.len());
    script.extend(std::iter::repeat_n(Edit::Equal, prefix));
    match myers(mid_a, mid_b, MAX_EDIT_DISTANCE) {
        Some(edits) => script.extend(edits),
        None => {
            script.extend(std::iter::repeat_n(Edit::Delete, mid_a.len()));
            script.extend(std::iter::repeat_n(Edit::Insert, mid_b.len()));
        }
    }
    script.extend(std::iter::repeat_n(Edit::Equal, suffix));
    hunks(&script)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Edit {
    Equal,
    Delete,
    Insert,
}

fn hunks(script: &[Edit]) -> Vec<Hunk> {
    let mut out: Vec<Hunk> = Vec::new();
    let (mut x, mut y) = (0usize, 0usize);
    let mut i = 0;
    while i < script.len() {
        let (x0, y0) = (x, y);
        if script[i] == Edit::Equal {
            while i < script.len() && script[i] == Edit::Equal {
                x += 1;
                y += 1;
                i += 1;
            }
            out.push(Hunk {
                kind: DiffKind::Equal,
                a: x0..x,
                b: y0..y,
            });
        } else {
            while i < script.len() && script[i] != Edit::Equal {
                match script[i] {
                    Edit::Delete => x += 1,
                    Edit::Insert => y += 1,
                    Edit::Equal => unreachable!(),
                }
                i += 1;
            }
            let kind = match (x > x0, y > y0) {
                (true, true) => DiffKind::Replace,
                (true, false) => DiffKind::Delete,
                _ => DiffKind::Insert,
            };
            out.push(Hunk {
                kind,
                a: x0..x,
                b: y0..y,
            });
        }
    }
    out
}

/// Myers' greedy shortest edit script. `None` when it needs more than `max_d` edits.
fn myers<T: Eq>(a: &[T], b: &[T], max_d: usize) -> Option<Vec<Edit>> {
    let (n, m) = (a.len() as isize, b.len() as isize);
    if n == 0 || m == 0 {
        let mut out = vec![Edit::Delete; n as usize];
        out.extend(std::iter::repeat_n(Edit::Insert, m as usize));
        return Some(out);
    }
    let max = (n + m) as usize;
    let limit = max.min(max_d);
    let offset = max as isize + 1;
    let mut v = vec![0isize; 2 * max + 3];
    // trace[d] = v[k] for k in -(d+1)..=(d+1), as it was at the start of round d.
    let mut trace: Vec<Vec<isize>> = Vec::new();
    let at = |k: isize| (k + offset) as usize;
    let mut found = None;
    'outer: for d in 0..=limit as isize {
        trace.push(v[at(-d - 1)..=at(d + 1)].to_vec());
        let mut k = -d;
        while k <= d {
            let mut x = if k == -d || (k != d && v[at(k - 1)] < v[at(k + 1)]) {
                v[at(k + 1)]
            } else {
                v[at(k - 1)] + 1
            };
            let mut y = x - k;
            while x < n && y < m && a[x as usize] == b[y as usize] {
                x += 1;
                y += 1;
            }
            v[at(k)] = x;
            if x >= n && y >= m {
                found = Some(d);
                break 'outer;
            }
            k += 2;
        }
    }
    found?;

    // Backtrack from (n, m).
    let mut rev: Vec<Edit> = Vec::with_capacity((n + m) as usize);
    let (mut x, mut y) = (n, m);
    for (d, snap) in trace.iter().enumerate().rev() {
        let d = d as isize;
        let get = |k: isize| snap[(k + d + 1) as usize];
        let k = x - y;
        let prev_k = if k == -d || (k != d && get(k - 1) < get(k + 1)) {
            k + 1
        } else {
            k - 1
        };
        let prev_x = get(prev_k);
        let prev_y = prev_x - prev_k;
        while x > prev_x && y > prev_y {
            rev.push(Edit::Equal);
            x -= 1;
            y -= 1;
        }
        if d > 0 {
            if x == prev_x {
                rev.push(Edit::Insert);
            } else {
                rev.push(Edit::Delete);
            }
            x = prev_x;
            y = prev_y;
        }
    }
    rev.reverse();
    Some(rev)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(s: &str) -> Vec<&str> {
        s.split_whitespace().collect()
    }

    fn kinds(h: &[Hunk]) -> Vec<(DiffKind, Range<usize>, Range<usize>)> {
        h.iter()
            .map(|h| (h.kind, h.a.clone(), h.b.clone()))
            .collect()
    }

    /// The hunks must tile both sequences and equal hunks must really be equal.
    fn check(a: &[&str], b: &[&str]) -> Vec<Hunk> {
        let h = diff(a, b);
        let (mut x, mut y) = (0, 0);
        for hunk in &h {
            assert_eq!((hunk.a.start, hunk.b.start), (x, y), "{h:?}");
            if hunk.kind == DiffKind::Equal {
                assert_eq!(&a[hunk.a.clone()], &b[hunk.b.clone()]);
            }
            x = hunk.a.end;
            y = hunk.b.end;
        }
        assert_eq!((x, y), (a.len(), b.len()));
        // No two adjacent change hunks, no two adjacent equal hunks.
        for w in h.windows(2) {
            assert!(
                (w[0].kind == DiffKind::Equal) != (w[1].kind == DiffKind::Equal),
                "{h:?}"
            );
        }
        h
    }

    #[test]
    fn identical_is_one_equal() {
        let a = words("the quick brown fox");
        assert_eq!(kinds(&check(&a, &a)), vec![(DiffKind::Equal, 0..4, 0..4)]);
        assert!(check(&[], &[]).is_empty());
    }

    #[test]
    fn pure_insert_and_delete() {
        let a = words("a b c");
        let b = words("a x y b c");
        assert_eq!(
            kinds(&check(&a, &b)),
            vec![
                (DiffKind::Equal, 0..1, 0..1),
                (DiffKind::Insert, 1..1, 1..3),
                (DiffKind::Equal, 1..3, 3..5)
            ]
        );
        assert_eq!(
            kinds(&check(&b, &a)),
            vec![
                (DiffKind::Equal, 0..1, 0..1),
                (DiffKind::Delete, 1..3, 1..1),
                (DiffKind::Equal, 3..5, 1..3)
            ]
        );
        assert_eq!(kinds(&check(&[], &a)), vec![(DiffKind::Insert, 0..0, 0..3)]);
        assert_eq!(kinds(&check(&a, &[])), vec![(DiffKind::Delete, 0..3, 0..0)]);
    }

    #[test]
    fn delete_plus_insert_collapses_to_replace() {
        let a = words("one two three four");
        let b = words("one deux trois four");
        assert_eq!(
            kinds(&check(&a, &b)),
            vec![
                (DiffKind::Equal, 0..1, 0..1),
                (DiffKind::Replace, 1..3, 1..3),
                (DiffKind::Equal, 3..4, 3..4)
            ]
        );
        // Unequal replace sizes.
        let b = words("one 2 four");
        assert_eq!(kinds(&check(&a, &b))[1], (DiffKind::Replace, 1..3, 1..2));
    }

    #[test]
    fn classic_myers_example_is_minimal() {
        // ABCABBA → CBABAC has an edit distance of 5 (LCS 4).
        let a: Vec<&str> = "A B C A B B A".split(' ').collect();
        let b: Vec<&str> = "C B A B A C".split(' ').collect();
        let h = check(&a, &b);
        let equal: usize = h
            .iter()
            .filter(|h| h.kind == DiffKind::Equal)
            .map(|h| h.a.len())
            .sum();
        assert_eq!(equal, 4, "{h:?}");
    }

    #[test]
    fn lcs_matches_brute_force_on_small_inputs() {
        // Deterministic pseudo-random sequences over a tiny alphabet.
        let mut seed: u32 = 12345;
        let mut next = || {
            seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            (seed >> 16) % 4
        };
        for _ in 0..200 {
            let la = (next() * 3) as usize;
            let lb = (next() * 3) as usize;
            let a: Vec<u32> = (0..la).map(|_| next()).collect();
            let b: Vec<u32> = (0..lb).map(|_| next()).collect();
            let h = diff(&a, &b);
            let equal: usize = h
                .iter()
                .filter(|h| h.kind == DiffKind::Equal)
                .map(|h| h.a.len())
                .sum();
            // DP LCS.
            let mut dp = vec![vec![0usize; lb + 1]; la + 1];
            for i in 1..=la {
                for j in 1..=lb {
                    dp[i][j] = if a[i - 1] == b[j - 1] {
                        dp[i - 1][j - 1] + 1
                    } else {
                        dp[i - 1][j].max(dp[i][j - 1])
                    };
                }
            }
            assert_eq!(equal, dp[la][lb], "a={a:?} b={b:?} h={h:?}");
        }
    }

    #[test]
    fn beyond_the_edit_cap_is_one_replace() {
        let a: Vec<u32> = (0..3000).collect();
        let b: Vec<u32> = (10_000..13_000).collect();
        let mut a2 = vec![7_777_777u32];
        a2.extend(&a);
        let mut b2 = vec![7_777_777u32];
        b2.extend(&b);
        assert_eq!(
            kinds(&diff(&a2, &b2)),
            vec![
                (DiffKind::Equal, 0..1, 0..1),
                (DiffKind::Replace, 1..3001, 1..3001)
            ]
        );
    }
}
