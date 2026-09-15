//! Shared test harness.
//!
//! `Pdfium::bind_to_library` works **once per process** and every `tests/*.rs` file is its
//! own process, so each file gets one engine from a `OnceLock` and shares it between its
//! tests (the engine is a thread behind a channel, so parallel test threads are fine).
//!
//! ```rust,ignore
//! mod common;
//! use common::*;
//!
//! #[test]
//! fn my_test() {
//!     let doc = open("tracemonkey.pdf");           // DocInfo; closed by `doc.close()`
//!     let chars = with_doc(&doc.doc_id, |d| Ok(text::layer::layer(d, 0)?.chars.len())).unwrap();
//!     assert_eq!(chars, 5087);
//! }
//! ```

#![allow(dead_code)]

use seepdf_lib::engine::registry::{self, OpenDoc};
use seepdf_lib::engine::types::EngineState;
use seepdf_lib::engine::{EngineHandle, Lane};
use seepdf_lib::ipc::types::DocInfo;
use seepdf_lib::ipc::EngineError;
use std::path::PathBuf;
use std::sync::OnceLock;

static ENGINE: OnceLock<EngineHandle> = OnceLock::new();

/// The one engine of this test process. Panics (with the resolver's message) when the host
/// libpdfium is missing — run `npm run fetch:pdfium` first.
pub fn engine() -> &'static EngineHandle {
    ENGINE.get_or_init(|| {
        let (library, dir) = seepdf_lib::app::pdfium_path::resolve_local()
            .expect("libpdfium: run `npm run fetch:pdfium`");
        let spill = std::env::temp_dir().join(format!("seepdf-test-{}", std::process::id()));
        seepdf_lib::engine::spawn(
            library,
            dir,
            None,
            spill,
            seepdf_lib::engine::render::cache::DEFAULT_CAPACITY_BYTES,
        )
        .expect("spawn engine")
    })
}

/// `fixtures/` in the repository root. `name` may contain a subdirectory (`"gen/500p.pdf"`).
pub fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("src-tauri has a parent")
        .join("fixtures")
        .join(name)
}

/// Every fixture the repository ships, generated ones included, in a stable order.
pub fn all_fixtures() -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = Vec::new();
    for dir in [fixture(""), fixture("gen")] {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut batch: Vec<PathBuf> = entries
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| {
                p.extension()
                    .map(|e| e.eq_ignore_ascii_case("pdf"))
                    .unwrap_or(false)
            })
            .collect();
        batch.sort();
        found.extend(batch);
    }
    found
}

/// A document open in the shared engine; closed on drop.
pub struct TestDoc {
    pub info: DocInfo,
    pub doc_id: String,
}

impl Drop for TestDoc {
    fn drop(&mut self) {
        let doc_id = self.doc_id.clone();
        let _ = engine().call_blocking(Lane::Edit, "test/close", move |st| {
            registry::close(st, &doc_id)
        });
    }
}

pub fn open(name: &str) -> TestDoc {
    try_open(name, None).unwrap_or_else(|e| panic!("open {name}: {e}"))
}

pub fn try_open(name: &str, password: Option<&str>) -> Result<TestDoc, EngineError> {
    let path = fixture(name);
    let bytes = std::fs::read(&path).map_err(EngineError::from)?;
    let password = password.map(str::to_owned);
    let info = engine().call_blocking(Lane::Edit, "test/open", move |st| {
        registry::open(st, Some(path), bytes, password)
    })?;
    let doc_id = info.doc_id.clone();
    Ok(TestDoc { info, doc_id })
}

/// Runs `f` on the engine thread with the document borrowed mutably.
pub fn with_doc<T, F>(doc_id: &str, f: F) -> Result<T, EngineError>
where
    T: Send + 'static,
    F: FnOnce(&mut OpenDoc<'_>) -> Result<T, EngineError> + Send + 'static,
{
    let doc_id = doc_id.to_string();
    engine().call_blocking(Lane::Edit, "test/with_doc", move |st| {
        let doc = st.doc_mut(&doc_id)?;
        f(doc)
    })
}

/// Runs `f` on the engine thread with the whole engine state.
pub fn with_state<T, F>(f: F) -> Result<T, EngineError>
where
    T: Send + 'static,
    F: FnOnce(&mut EngineState<'_>) -> Result<T, EngineError> + Send + 'static,
{
    engine().call_blocking(Lane::Edit, "test/with_state", f)
}
