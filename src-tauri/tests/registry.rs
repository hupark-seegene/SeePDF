//! Registry invariants: drop order, opening every fixture, outline and page labels.

mod common;

use common::*;
use seepdf_lib::engine::page_lru::DEFAULT_CAPACITY;
use seepdf_lib::engine::registry;
use seepdf_lib::ipc::types::ChangeReason;

/// Closing a document with pages still open must drop the pages first: `PdfPage<'a>` borrows
/// the `Pdfium` lifetime, not its document, so `FPDF_ClosePage` after `FPDF_CloseDocument`
/// compiles and is a use-after-free (render spike §5). A regression here is a SIGSEGV, which
/// is why this test opens more pages than the LRU holds and then closes the document.
#[test]
fn registry_drop_order() {
    let doc = open("tracemonkey.pdf");
    let doc_id = doc.doc_id.clone();

    let open_pages = with_doc(&doc_id, |d| {
        for page in 0..d.page_count().min(20) {
            let _ = d.page(page)?;
        }
        Ok(d.page_lru_len())
    })
    .expect("open 20 pages");
    assert!(open_pages > 0 && open_pages <= DEFAULT_CAPACITY);

    // Explicit close (the harness's `Drop` would do the same, and tolerates the double
    // close) while pages are still cached. A regression is a crash, not an assertion.
    let remaining = with_state({
        let doc_id = doc_id.clone();
        move |st| {
            registry::close(st, &doc_id)?;
            Ok(st.docs.contains_key(&doc_id))
        }
    })
    .expect("close with open pages");
    assert!(!remaining);

    // Opening and closing again on the same engine proves nothing was left dangling.
    let again = open("tracemonkey.pdf");
    assert_eq!(again.info.page_count, 14);
}

/// Every fixture opens, reports a page count, and can render its geometry.
#[test]
fn registry_open_all_fixtures() {
    let fixtures = all_fixtures();
    assert!(
        fixtures.len() >= 9,
        "expected the shipped fixtures, found {fixtures:?}"
    );
    let mut opened = 0;
    for path in fixtures {
        let name = path
            .strip_prefix(fixture(""))
            .unwrap_or(&path)
            .to_string_lossy()
            .to_string();
        match try_open(&name, None) {
            Ok(doc) => {
                assert!(doc.info.page_count > 0, "{name} has no pages");
                assert_eq!(
                    doc.info.pages.len(),
                    doc.info.page_count as usize,
                    "{name}: pages[] must match pageCount"
                );
                for geom in &doc.info.pages {
                    assert!(
                        geom.width_pt > 0.0 && geom.height_pt > 0.0,
                        "{name} page {} has no size",
                        geom.index
                    );
                }
                assert_eq!(doc.info.doc_generation, 1, "{name} opens at generation 1");
                assert!(!doc.info.dirty, "{name} opens clean");
                opened += 1;
            }
            Err(e) if e.is_password() => {
                // gen/encrypted-rc4-40.pdf — covered by engine_password_error_mapping.
            }
            Err(e) => panic!("{name}: {e}"),
        }
    }
    assert!(opened >= 8, "only {opened} fixtures opened");
}

/// The outline is depth-first and includes the siblings of `root()`, which is the **first
/// top-level bookmark**, not a synthetic root (pages spike §6).
#[test]
fn registry_outline_and_labels() {
    let name = "gen/outline-labels.pdf";
    if !fixture(name).exists() {
        eprintln!("skipping: run `cargo run --release --example gen_fixtures` first");
        return;
    }
    let doc = open(name);
    let labels: Vec<Option<String>> = doc.info.pages.iter().map(|p| p.label.clone()).collect();
    assert_eq!(
        labels,
        vec![
            Some("i".into()),
            Some("ii".into()),
            Some("1".into()),
            Some("2".into()),
            Some("App-A".into()),
            Some("App-B".into())
        ],
        "/PageLabels is read at open without loading a single page"
    );

    assert!(doc.info.has_outline);
    let outline = with_doc(&doc.doc_id, |d| Ok(d.outline())).expect("outline");
    let titles: Vec<&str> = outline.iter().map(|n| n.title.as_str()).collect();
    assert_eq!(titles, vec!["Chapter A", "Chapter B", "Chapter C"]);
    assert_eq!(outline[0].children.len(), 2);
    assert_eq!(outline[0].children[0].title, "A.1 Introduction");
    assert_eq!(outline[0].children[0].page, Some(1));
    assert_eq!(outline[1].children[0].children[0].title, "B.1.a Detail");
    assert_eq!(outline[1].children[0].children[0].page, Some(5));
    let total = count(&outline);
    assert_eq!(total, 7, "7 nodes over 3 levels");
}

fn count(nodes: &[seepdf_lib::ipc::types::OutlineNode]) -> usize {
    nodes.iter().map(|n| 1 + count(&n.children)).sum()
}

/// `registry::mutate` is the only `&mut` path: it snapshots, bumps the generation, sweeps the
/// tile cache and marks the document dirty. A failing closure changes nothing.
#[test]
fn registry_mutate_bumps_generation() {
    let doc = open("rotation.pdf");
    let doc_id = doc.doc_id.clone();
    assert_eq!(doc.info.doc_generation, 1);

    let after = with_state({
        let doc_id = doc_id.clone();
        move |st| {
            registry::mutate(
                st,
                &doc_id,
                registry::MutateOpts::new("undo.pageRotate", ChangeReason::Pages).page(0),
                |d| {
                    let page = d.page(0)?;
                    page.set_rotation(pdfium_render::prelude::PdfPageRenderRotation::Degrees90);
                    Ok(())
                },
            )?;
            let d = st.doc(&doc_id)?;
            Ok((d.generation, d.dirty(), d.history.can_undo()))
        }
    })
    .expect("mutate");
    assert_eq!(after, (2, true, true));

    let failed = with_state({
        let doc_id = doc_id.clone();
        move |st| {
            let err = registry::mutate(
                st,
                &doc_id,
                registry::MutateOpts::new("undo.test", ChangeReason::Edit),
                |_d| Err::<(), _>(seepdf_lib::ipc::EngineError::invalid("nope")),
            )
            .unwrap_err();
            let d = st.doc(&doc_id)?;
            Ok((err.code, d.generation, d.history.undo_depth()))
        }
    })
    .expect("failed mutate");
    assert_eq!(
        failed,
        (seepdf_lib::ipc::ErrorCode::InvalidArgument, 2, 1),
        "a failing mutate leaves the generation and the undo stack untouched"
    );
}

/// The five accessors of the pdfium-render patch (`ARCHITECTURE.md` §1.3) reach the **same**
/// PDFium objects the high-level API uses. This is the test that proves the fork is wired up
/// before Stage 1 (a) builds the whole annotation and form pipeline on it.
#[test]
fn registry_raw_handles_are_live() {
    let doc = open("160F-2019.pdf");
    let counts = with_state({
        let doc_id = doc.doc_id.clone();
        move |st| {
            let bindings = seepdf_lib::engine::raw::bindings(st.pdfium);
            let d = st.doc_mut(&doc_id)?;
            assert!(d.has_form(), "160F-2019.pdf has an AcroForm");
            assert!(d.form_handle().is_some(), "and therefore a raw form handle");
            assert!(!d.pdf().raw_handle().is_null(), "FPDF_DOCUMENT");

            let page = d.page(0)?;
            assert!(!page.raw_handle().is_null(), "FPDF_PAGE");
            assert!(!page.raw_document_handle().is_null(), "page -> FPDF_DOCUMENT");
            // Drive one raw call through the patched bindings and cross-check it against the
            // high-level count: the two must be looking at the same page.
            // SAFETY: the page handle is live for this borrow and we are on the engine thread.
            let raw_annots = unsafe { bindings.FPDFPage_GetAnnotCount(page.raw_handle()) };
            let high_level = page.annotations().len();

            let annotation = page
                .annotations()
                .get(0)
                .map_err(|e| seepdf_lib::ipc::EngineError::pdfium("annotations().get(0)", e))?;
            assert!(!annotation.raw_handle().is_null(), "FPDF_ANNOTATION");
            Ok((raw_annots as usize, high_level))
        }
    })
    .expect("raw handles");
    assert_eq!(counts.0, counts.1, "raw and high-level see the same annotations");
    assert_eq!(counts.0, 76, "160F-2019.pdf page 0 has 76 widget annotations");
}
