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
            assert!(
                !page.raw_document_handle().is_null(),
                "page -> FPDF_DOCUMENT"
            );
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
    assert_eq!(
        counts.0, counts.1,
        "raw and high-level see the same annotations"
    );
    assert_eq!(
        counts.0, 76,
        "160F-2019.pdf page 0 has 76 widget annotations"
    );
}

/// STAGE1A §5.2 — a closure that mutates and **then** fails leaves nothing behind.
///
/// `registry::mutate` used to only drop its undo bookkeeping on `Err`, so the half-edit stayed
/// in the in-memory document under the *old* generation: every cached tile, the text layer and
/// `list_annotations` kept describing a document the bytes no longer matched, and the next
/// successful edit silently made the orphan permanent. The rollback reloads the pre-edit
/// snapshot instead.
#[test]
fn registry_mutate_rolls_back_a_half_failed_closure() {
    use seepdf_lib::engine::annot::{create, read};
    use seepdf_lib::engine::registry::MutateOpts;
    use seepdf_lib::ipc::types::{AnnotSpec, MarkupSpec, Rect};
    use seepdf_lib::ipc::{EngineError, ErrorCode};

    let doc = open("tracemonkey.pdf");
    let doc_id = doc.doc_id.clone();

    let spec = || {
        AnnotSpec::Highlight(MarkupSpec {
            rects: vec![Rect::new(100.0, 700.0, 260.0, 712.0)],
            color: [255, 235, 0],
            opacity: 0.5,
            contents: None,
        })
    };

    // One real edit first, so the rollback has to restore a *dirty* document, not the file.
    let kept = with_state({
        let doc_id = doc_id.clone();
        let spec = spec();
        move |st| {
            registry::mutate(
                st,
                &doc_id,
                MutateOpts::new("undo.annotate", ChangeReason::Edit).page(0),
                |d| create::create(d, 0, &spec, None),
            )
        }
    })
    .expect("first annotation");

    let (generation_before, undo_before, redo_before, count_before) = with_state({
        let doc_id = doc_id.clone();
        move |st| {
            let d = st.doc_mut(&doc_id)?;
            let count = read::list_page(d, 0)?.len();
            let d = st.doc_mut(&doc_id)?;
            Ok((
                d.generation,
                d.history.undo_depth(),
                d.history.redo_depth(),
                count,
            ))
        }
    })
    .expect("state before");
    assert_eq!(count_before, 1, "the first annotation is on the page");

    // Now a closure that creates two more annotations and *then* fails.
    let err = with_state({
        let doc_id = doc_id.clone();
        let spec = spec();
        move |st| {
            registry::mutate(
                st,
                &doc_id,
                MutateOpts::new("undo.annotate", ChangeReason::Edit).page(0),
                |d| {
                    create::create(d, 0, &spec, None)?;
                    create::create(d, 0, &spec, None)?;
                    Err::<(), _>(EngineError::new(
                        ErrorCode::VerifyFailed,
                        "deliberate failure",
                    ))
                },
            )
        }
    })
    .expect_err("the closure fails");
    assert_eq!(err.code, ErrorCode::VerifyFailed);

    let (generation_after, undo_after, redo_after, count_after, dirty_after) = with_state({
        let doc_id = doc_id.clone();
        move |st| {
            let d = st.doc_mut(&doc_id)?;
            let count = read::list_page(d, 0)?.len();
            let d = st.doc_mut(&doc_id)?;
            Ok((
                d.generation,
                d.history.undo_depth(),
                d.history.redo_depth(),
                count,
                d.dirty(),
            ))
        }
    })
    .expect("state after");

    assert_eq!(
        count_after, count_before,
        "the two annotations the failed closure created must be gone"
    );
    assert_eq!(generation_after, generation_before, "no generation bump");
    assert_eq!(
        undo_after, undo_before,
        "no undo entry for an edit that failed"
    );
    assert_eq!(
        redo_after, redo_before,
        "and no redo entry either — there is nothing to redo"
    );
    assert!(dirty_after, "the *successful* first edit is still there");

    // The document is still usable and the surviving annotation is still the one we made.
    let ids = with_doc(&doc_id, |d| {
        Ok(read::list_page(d, 0)?
            .into_iter()
            .map(|a| a.id)
            .collect::<Vec<_>>())
    })
    .expect("list after rollback");
    assert_eq!(
        ids,
        vec![kept],
        "the annotation from before the failure survives"
    );
}

/// STAGE1A §5.1 — an annotation edit keeps the page's cached text layer.
///
/// `OpenDoc::invalidate_page` dropped the handle **and** the text, so every annotation write
/// cost a 5.6 ms `FPDFText_LoadPage` the next time the viewer selected text on that page —
/// even though an annotation lives in `/Annots` and `FPDFText_*` only reads the page content
/// stream. `ScratchPage` now drops the handle only, and `mutate` keeps the layer when the
/// caller says the edit `keeps_text()`.
#[test]
fn registry_annotation_edit_keeps_the_text_layer() {
    use seepdf_lib::engine::annot::create;
    use seepdf_lib::engine::registry::MutateOpts;
    use seepdf_lib::engine::text;
    use seepdf_lib::ipc::types::{AnnotSpec, MarkupSpec, Rect};

    let doc = open("tracemonkey.pdf");
    let doc_id = doc.doc_id.clone();

    // Build and cache the layer, and remember what it said.
    let before = with_doc(&doc_id, |d| {
        let layer = text::layer::layer(d, 0)?;
        Ok((layer.chars.len(), d.text.layer(0).is_some()))
    })
    .expect("build the text layer");
    assert!(before.1, "the layer is cached after the first build");
    assert_eq!(before.0, 5087, "tracemonkey page 1 has 5 087 characters");

    with_state({
        let doc_id = doc_id.clone();
        move |st| {
            registry::mutate(
                st,
                &doc_id,
                MutateOpts::new("undo.annotCreate", ChangeReason::Edit)
                    .page(0)
                    .keeps_text(),
                |d| {
                    create::create(
                        d,
                        0,
                        &AnnotSpec::Highlight(MarkupSpec {
                            rects: vec![Rect::new(100.0, 700.0, 260.0, 712.0)],
                            color: [255, 235, 0],
                            opacity: 0.5,
                            contents: None,
                        }),
                        None,
                    )
                },
            )
        }
    })
    .expect("annotate");

    let after = with_doc(&doc_id, |d| {
        // The *cache* getter, not `layer()`: it answers `None` if anything invalidated it.
        let cached = d.text.layer(0);
        Ok((cached.is_some(), cached.map(|l| l.chars.len()).unwrap_or(0)))
    })
    .expect("read the cache");
    assert!(
        after.0,
        "the annotation write must not have dropped the cached text layer"
    );
    assert_eq!(after.1, before.0, "and the text itself is unchanged");

    // A *content* edit still drops it: `keeps_text()` is opt-in, not the default.
    with_state({
        let doc_id = doc_id.clone();
        move |st| {
            registry::mutate(
                st,
                &doc_id,
                MutateOpts::new("undo.objectAdd", ChangeReason::Edit).page(0),
                |_| Ok(()),
            )
        }
    })
    .expect("content edit");
    let dropped = with_doc(&doc_id, |d| Ok(d.text.layer(0).is_some())).expect("read the cache");
    assert!(
        !dropped,
        "an edit without keeps_text() still invalidates the layer"
    );
}

/// Stage 8 `open_document { displayName }`: a recovered copy (`<uuid>.pdf`) reports the
/// original name — in `DocInfo.name`, after an edit, after undo, and in the `{{filename}}`
/// stamp token. A blank display name falls back to the file name.
#[test]
fn display_name_replaces_the_file_name() {
    use seepdf_lib::engine::stamp;
    use seepdf_lib::engine::text::layer;
    use seepdf_lib::ipc::types::{
        PageSelection, PageStampSource, PageStampSpec, StampAnchor, StampRole,
    };
    let recovered = std::env::temp_dir().join("0f8fad5b-d9cb-469f-a165-70867728950e.pdf");
    std::fs::copy(fixture("tracemonkey.pdf"), &recovered).unwrap();
    let open = |name: Option<&str>| {
        let (path, bytes) = (recovered.clone(), std::fs::read(&recovered).unwrap());
        let name = name.map(str::to_owned);
        let info = with_state(move |st| registry::open_named(st, Some(path), bytes, None, name))
            .expect("open");
        let doc_id = info.doc_id.clone();
        TestDoc { info, doc_id }
    };

    let doc = open(Some("분기 보고서.pdf"));
    assert_eq!(doc.info.name, "분기 보고서.pdf");
    assert_eq!(
        doc.info.path.as_deref(),
        Some(recovered.to_str().unwrap()),
        "the path is the real file"
    );
    let spec = PageStampSpec {
        role: StampRole::Footer,
        source: PageStampSource::Text {
            text: "{{filename}}".into(),
            font_size_pt: 10.0,
            color: [0, 0, 0],
        },
        anchor: StampAnchor::Bc,
        margin_pt: 20.0,
        rotate_deg: 0.0,
        opacity: 1.0,
        pages: PageSelection::List(vec![0]),
        bates: Default::default(),
    };
    let d = doc.doc_id.clone();
    let stamped = with_state(move |st| stamp::add_stamp(st, &d, &spec)).expect("stamp");
    assert_eq!(stamped.info.name, "분기 보고서.pdf");
    let d = doc.doc_id.clone();
    let text =
        with_state(move |st| Ok(layer::page_text(st.doc_mut(&d)?, 0)?.text.clone())).unwrap();
    assert!(
        text.contains("분기 보고서"),
        "{{{{filename}}}} is the display name's stem"
    );
    assert!(!text.contains("0f8fad5b"));
    let d = doc.doc_id.clone();
    let undone = with_state(move |st| registry::undo(st, &d, false)).expect("undo");
    assert_eq!(undone.name, "분기 보고서.pdf");

    assert_eq!(
        open(None).info.name,
        "0f8fad5b-d9cb-469f-a165-70867728950e.pdf"
    );
    assert_eq!(
        open(Some("  ")).info.name,
        "0f8fad5b-d9cb-469f-a165-70867728950e.pdf"
    );
}

/// Bug hunt: page indices are `u16`, and a document with more than 65,535 pages used to open
/// with its count truncated (`len() as u16`: 65,536 pages became 0) and wrapped indices. It is
/// refused with `unsupported`; an edit that would cross the limit is refused and rolled back.
#[test]
fn registry_refuses_more_than_65535_pages() {
    use seepdf_lib::engine::{pages, raw};
    use seepdf_lib::ipc::types::PageOp;
    use seepdf_lib::ipc::{EngineError, ErrorCode};
    use std::os::raw::c_int;

    let (at_limit, over) = with_state(|st| {
        let bindings = raw::bindings(st.pdfium);
        let doc = st
            .pdfium
            .create_new_pdf()
            .map_err(|e| EngineError::pdfium("create", e))?;
        let add = |count: usize| {
            for _ in 0..count {
                // SAFETY: a live document on the engine thread; the page is closed at once.
                unsafe {
                    let page = bindings.FPDFPage_New(doc.raw_handle(), c_int::MAX, 100.0, 100.0);
                    bindings.FPDF_ClosePage(page);
                }
            }
        };
        add(u16::MAX as usize);
        let at_limit =
            raw::save::save_as_copy(bindings, &doc, raw::save::SaveFlags::NoIncremental)?;
        add(1);
        let over = raw::save::save_as_copy(bindings, &doc, raw::save::SaveFlags::NoIncremental)?;
        Ok((at_limit, over))
    })
    .expect("build the documents");

    let refused = with_state(move |st| registry::open(st, None, over, None).map(|i| i.page_count));
    assert_eq!(
        refused.map_err(|e| e.code),
        Err(ErrorCode::Unsupported),
        "65,536 pages are refused, not truncated"
    );

    let info =
        with_state(move |st| registry::open(st, None, at_limit, None)).expect("65,535 pages");
    let doc = TestDoc {
        doc_id: info.doc_id.clone(),
        info,
    };
    assert_eq!(doc.info.page_count, u16::MAX);
    assert_eq!(doc.info.pages.len(), u16::MAX as usize);
    let before = with_doc(&doc.doc_id, |d| Ok((d.generation, d.history.undo_depth()))).unwrap();

    let grown = with_state({
        let doc_id = doc.doc_id.clone();
        move |st| {
            pages::apply_ops(st, &doc_id, vec![PageOp::Duplicate { pages: vec![0] }])
                .map(|i| i.page_count)
        }
    });
    assert_eq!(grown.map_err(|e| e.code), Err(ErrorCode::Unsupported));
    let after = with_doc(&doc.doc_id, |d| {
        Ok((d.page_count(), d.generation, d.history.undo_depth()))
    })
    .unwrap();
    assert_eq!(
        after,
        (u16::MAX, before.0, before.1),
        "the refused edit was rolled back"
    );
}
