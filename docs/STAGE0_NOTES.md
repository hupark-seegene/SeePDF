# Stage 0 — backend notes

What the Stage 0 backend actually shipped, where it deviates from `WORKPLAN.md` /
`ARCHITECTURE.md` and why, and **everything a Stage 1 backend agent needs to start writing
code on day 1**. Companion documents: `ARCHITECTURE.md` (design), `IPC_CONTRACT.md` (the
seam), `WORKPLAN.md` (ownership and DoD).

Read §1 (engine API) and §5 (gotchas) before touching `src-tauri/src/engine/**`.

---

## 0. What exists

```
src-tauri/src/
  lib.rs                     run(), the one generate_handler!, `--smoke`
  ipc/{types,error}.rs       IPC_CONTRACT.md as Rust; the single {code, message} error
  engine/
    thread.rs                EngineHandle + the seepdf-engine loop (lanes, stale drop, guard)
    types.rs                 Lane, Cmd, CmdStatus, Reply, EngineShared, EngineState, Viewport
    registry.rs              OpenDoc, open/close/mutate/replace/undo  <- the core API
    page_lru.rs              24 open PdfPage handles, form hooks, drop order
    history.rs               snapshot undo/redo with disk spill
    jobs.rs                  JobId + Arc<AtomicBool> cancellation
    stats.rs                 engine_stats counters
    raw/{mod,consts,doc,form}.rs   the ONLY unsafe; Stage 1 (a) extends it
    render/{geometry,tiles,encode,cache}.rs
    text/{layer,serialize,search}.rs
  protocol/mod.rs            seepdf:// routes, headers, status codes
  app/{pdfium_path,files,windows,menu,store}.rs
  commands/*.rs              every command of the contract
tests/{common/mod.rs,engine,registry,render,text,history,protocol}.rs
examples/gen_fixtures.rs     fixtures/gen/*
```

`src-tauri/src/spike/**` has been deleted. `src-tauri/examples/spike_*.rs` are kept as
historical reference; `spike_render.rs` lost its §5 "engine ownership model" section, which
proved things about the `thread_safe` feature and cannot compile now that the feature is off
(that is decision D2 working as intended — the findings live in `docs/spikes/render.md` §5).

---

## 1. The engine API Stage 1 calls

### 1.1 Getting onto the engine thread

```rust
use seepdf_lib::engine::{EngineHandle, Lane, Submit};

// From a #[tauri::command]:
engine.call(Lane::Edit, "create_annotation", move |st| { /* -> Result<T, EngineError> */ }).await
// Outside the async runtime (tests, --smoke):
engine.call_blocking(Lane::Edit, "label", move |st| { … })
// Fire-and-forget, never dropped:
engine.send(Lane::Background, "label", move |st| { … })
// Full control (priority, page, viewport generation, cancellation):
engine.dispatch(
    Submit::new(Lane::Background, "label").priority(i).page(p).cancel(token.cancel.clone()),
    move |st, status| match status {            // status is Run | Stale | Cancelled
        CmdStatus::Run => { … }
        _ => { /* you MUST still answer your caller */ }
    },
)
```

`st` is `&mut EngineState<'_>`: `st.doc(id)? -> &OpenDoc`, `st.doc_mut(id)? -> &mut OpenDoc`,
`st.pdfium`, `st.encode` (the PNG pool), `st.shared` (tile cache, viewport, doc mirror),
`st.app` (`Option<AppHandle>` — `None` in tests, so never `unwrap()` it).

Closures must be `Send + 'static`; **nothing pdfium may escape them** (pdfium types are
`!Send` because the `thread_safe` feature is off — that is the compiler enforcing D1).

### 1.2 Mutating a document

`registry::mutate` is the only `&mut` path. It takes the undo snapshot, flushes the page LRU
for structural edits, bumps the generation, refreshes `pages_meta`, invalidates the text /
annotation / tile caches and emits `doc-changed`. **If your closure returns `Err`, nothing
happened**: the generation is not bumped and the snapshot is rolled back.

```rust
use seepdf_lib::engine::registry::{self, MutateOpts};
use seepdf_lib::ipc::types::ChangeReason;

registry::mutate(
    st,
    &doc_id,
    MutateOpts::new("undo.annotCreate", ChangeReason::Edit).page(page),
    |doc| {                                  // doc: &mut OpenDoc<'p>
        let page_ref = doc.page(page)?;      // opens through the LRU
        // … pdfium work …
        doc.touched.insert(page);            // annotations: needs an /AP render before save
        Ok(annot_id)
    },
)?
```

`MutateOpts` builders:

| builder | meaning |
|---|---|
| `new(label, reason)` | `label` is an i18n key (`undo.annotCreate`, `undo.pageDelete`, …) shown in the Edit menu; `reason` goes into `doc-changed` |
| `.page(p)` / `.pages(vec)` | which pages' pixels changed — their tiles, text layer and annotation cache are dropped, and their `/Rotate` + crop box are re-read (≤ 32 pages) |
| `.structural()` | the page **list** changes (move / delete / insert / duplicate). Flushes the whole page LRU *before* the closure runs, because a raw `FPDF_MovePages` / `FPDF_ImportPagesByIndex` bypasses pdfium-render's `PdfPageIndexCache`. Sets `pages = All` |
| `.coalesced()` | drag / slider gestures: repeats of the same label within 500 ms are one undo step |

Other registry entry points:

```rust
registry::open(st, Some(path), bytes, password) -> Result<DocInfo, EngineError>
registry::close(st, &doc_id)                     // pages -> form -> document, in that order
registry::replace(st, &doc_id, bytes)            // swap the bytes, keep id/path/history
registry::undo(st, &doc_id, /* redo = */ false) -> Result<DocInfo, EngineError>
```

### 1.3 `OpenDoc`

```rust
doc.page(index) -> Result<&mut PdfPage<'p>, EngineError>   // LRU + FORM_OnAfterLoadPage + Manual
doc.pdf() -> &PdfDocument<'p>        doc.pdf_mut() -> &mut PdfDocument<'p>   // inside mutate only
doc.form_handle() -> Option<FPDF_FORMHANDLE>       doc.has_form() -> bool
doc.bindings() -> &'static dyn PdfiumLibraryBindings
doc.close_pages()                    // before any structural change
doc.invalidate_page(index)           // drop one page handle + its text layer
doc.geom(index) -> Result<&PageGeom, _>            doc.page_count() -> u16
doc.to_bytes() -> Result<Arc<[u8]>, _>             // save_to_bytes(), 5 ms/MB
doc.info() -> DocInfo                doc.summary() -> DocSummary
doc.outline() -> Vec<OutlineNode>
// public fields: bytes, path, password, generation, saved_generation, pages_meta,
//                touched, history, text, annots, encrypted, permissions, meta,
//                pdf_version, tagged, xfa, has_outline
```

`doc.page()` already sets `PdfPageContentRegenerationStrategy::Manual`, so **page-object work
must call `page.regenerate_content()` exactly once at the end of the command**; annotation and
form work must not call it at all.

### 1.4 Raw FFI (`engine/raw/`) — owned by Stage 1 (a)

The pdfium-render patch (`docs/pdfium-patch.diff`) adds five accessors, all live and tested
(`cargo test --release --test registry registry_raw_handles_are_live`):

```rust
PdfDocument::raw_handle()        -> FPDF_DOCUMENT
PdfPage::raw_handle()            -> FPDF_PAGE
PdfPage::raw_document_handle()   -> FPDF_DOCUMENT      // extra convenience
PdfForm::raw_handle()            -> FPDF_FORMHANDLE
PdfPageAnnotation::raw_handle()  -> FPDF_ANNOTATION
Pdfium::raw_bindings()           -> &'static dyn PdfiumLibraryBindings   // the SAME instance
```

Get the bindings with `engine::raw::bindings(st.pdfium)` or `doc.bindings()`. There is **no**
second `bind_to_library()` anywhere: the raw calls act on the same `FPDF_DOCUMENT` the
high-level API is using, which is the whole reason the patch exists.

Stage 0 ships `raw/form.rs` (`on_after_load_page`, `on_before_close_page`,
`force_to_kill_focus`, `set_field_highlight_alpha`), `raw/doc.rs` (`page_label`,
`decode_utf16le`) and `raw/consts.rs` (subtypes, colour types, appearance modes, `/F` flags,
`FLAT_*`, `FPDF_REMOVE_SECURITY = 4`, bitmap formats — pinned by a unit test). Stage 1 (a)
adds `annot.rs`, `page.rs` (`move_pages`, `import_pages_by_index`, `flatten`) and `save.rs`
(`save_as_copy` with flags) beside them. House rules: `unsafe` on the single FFI line with a
`// SAFETY:` note, never store a raw handle, return `Result<_, EngineError>`.

### 1.5 Rendering and text, for anyone who needs pixels or characters

```rust
engine::render::tiles::render(st, &RenderRequest::new(tile_key)) -> Result<RawImage, _>
engine::render::tiles::render_raw_buffer(st, doc, page, scale, rect) -> Result<Vec<u8>, _>
engine::render::tiles::generate_appearances(st, doc)   // the pre-save /AP pass; clears `touched`
engine::render::encode::encode_png(&raw) -> Result<Vec<u8>, _>
st.encode.submit(Some(key), raw, Some(cache), Some(stats), move |result| { … })  // off-thread

engine::text::layer::layer(doc, page)      -> Arc<TextLayer>   // chars + words + lines, 5.6 ms
engine::text::layer::page_text(doc, page)  -> Arc<PageText>    // code points only, ~0.5 ms
engine::text::search::search_page(doc, page, &Query)
engine::text::serialize::serialize(&layer) -> Vec<u8>          // IPC_CONTRACT §10.1
```

`TextLayer.chars[i].tight` is the glyph ink box redaction hit-tests against; `.loose` is the
selection box; `.object_id` is the owning page-object index (or `u32::MAX`).

### 1.6 Adding a command body

Every command is already registered in `lib.rs` with its real serde signature, so **`lib.rs`
never changes again**. To implement one, open your `commands/<area>.rs`, delete the
`Err(EngineError::unsupported("…"))` and write the body; drop the `_` prefixes from the
parameters you now use. Progress goes over the `Channel<JobEvent>` parameter that is already
there; cancellation is `engine.jobs.create()` → put `token.cancel.clone()` on every `Submit`
→ `engine.jobs.finish(token.id)` when the last chunk lands.

---

## 2. Deviations from the plan, and why

| # | Plan | What shipped | Why |
|---|---|---|---|
| D-1 | `[patch.crates-io] pdfium-render = { git = "…", rev = … }` | `path = "../vendor/pdfium-render"` | Decided before Stage 0 started: the crate is vendored so the build never depends on GitHub. The patch is byte-for-byte the one `ARCHITECTURE.md` §1.3 describes and is also kept in `docs/pdfium-patch.diff`. |
| D-2 | patch = five accessors | five accessors **plus two packaging changes** to the vendored `Cargo.toml`: `crate-type = ["lib"]` (the published `lib + staticlib + cdylib` made two units write `libpdfium_render.dylib`, which broke `cargo test` with `E0460`), and a `[lints]` table allowing the crate's own warnings (cargo does not apply `--cap-lints allow` to a *path* dependency, so its pre-existing `dead_code` warnings would fail `-D warnings`). | Both are packaging-only, both are in the diff. |
| D-3 | — | The vendored crate was pruned: `test/`, `.github/`, `Cargo.lock`, `Cargo.toml.orig` and every `include/pdfium_*` except `pdfium_7881` (24 MB → 13 MB). | `include/` is read by `build.rs` only under the `bindings` feature, which SeePDF never enables. `src/bindgen/*.rs` is **kept in full** (9.9 MB) because `src/lib.rs` `include!`s each file behind a feature flag; deleting them would break every other `pdfium_*` feature. |
| D-4 | `DocInfo.pages[].rotation` / `.crop` exact at open | Exact for every page of documents up to **64 pages**; larger documents get the first 8 plus lazy refinement as pages are opened. | PDFium has no way to read `/Rotate` or the crop box without `FPDF_LoadPage` (0.46–26 ms/page). 500 pages would be ~250 ms, over the open budget. `page_sizes()` (which *is* used for every page) already returns the rotated display size, so layout is always correct; only the `rotation` number and `crop` lag. The authoritative matrix for text travels in the text-layer header (`IPC_CONTRACT.md` §10.1 offset 24) and the authoritative pixel size in `X-Image-Width/Height`. See §4 open issues. |
| D-5 | `set_viewport` "fire-and-forget" | It is a synchronous command returning `Result<(), EngineError>`. | It touches no pdfium; making it async would add a round trip for nothing. It still bumps `viewport_gen` exactly once per call. |
| D-6 | commands split per `WORKPLAN.md` 0.3 | `set_viewport`, `render_page_raw` and `engine_stats` live in `commands/documents.rs` (there is no `view.rs` in the row's file list). | Keeps the file list of row 0.3 exact. |
| D-7 | `engine/raw/**` owned by Stage 1 (a) | Stage 0 created it with the form-page hooks, the label reader and the constant table. | `registry::page` cannot call `FORM_OnAfterLoadPage` without it, and that is a Stage 0 requirement (`WORKPLAN.md` row 0.5). Stage 1 (a) owns the directory from here. |
| D-8 | — | `app_info` is registered although it is not in `IPC_CONTRACT.md` §12. | Diagnostics for the status bar and bug reports (version, os/arch, pdfium version and directory, locale, theme). No pdfium call. Flag it to the integrator if the contract should stay closed. |
| D-9 | `src-tauri/resources/fonts/**` owned by Stage 1 (b) | Stage 0 committed `resources/fonts/README.md`. | `bundle.resources` globs that directory and a glob that matches nothing makes `build.rs` fail with `GlobPathNotFound`. The file explains what Stage 1 (b) puts there. |

### The §6 probe Stage 0 owns

**`Cache-Control: immutable` on a custom scheme.** Not probed live: it needs a
`tauri build --debug` bundle plus a running webview, and the frontend is being rewritten in
parallel (the DoD's `npm run tauri dev` was explicitly out of scope for this agent). What
shipped instead is **both** paths at once, so the question is no longer load-bearing:

* every image response carries `Cache-Control: private, max-age=31536000, immutable` — correct
  by construction, because the URL contains `gen` and a generation bump serves `410 Gone`;
* the spike-verified fallback is still implemented: a request that actually carries
  `If-None-Match` matching the `ETag` is answered `304` from the encoded cache, with no pdfium
  call (`protocol_routes` asserts this). A hand-crafted 304 for a `fetch()` is never produced,
  which is what made WebKit throw `TypeError: Load failed` in the tauri spike.

So a webview that honours `immutable` never revalidates, and one that ignores it gets a cheap
304. Someone with a bundle should still confirm the header is honoured and record it here.

---

## 3. Test harness

```sh
cd src-tauri
cargo test --release                       # 43 tests, all green
cargo test --release --test render         # one file
cargo test --release render_tile_matches   # one test
cargo run --release --example gen_fixtures # (re)build fixtures/gen/*
```

The `WORKPLAN.md` DoD list maps onto these test functions:

| DoD name | file |
|---|---|
| `engine_lanes`, `engine_stale_drop` (+ `engine_cancel_between_chunks`, `engine_password_error_mapping`) | `tests/engine.rs` |
| `registry_drop_order`, `registry_open_all_fixtures` (+ `registry_outline_and_labels`, `registry_mutate_bumps_generation`, `registry_raw_handles_are_live`) | `tests/registry.rs` |
| `render_tile_matches_crop`, `render_rotation_sizes` (+ thumbnails, `SPRX` header, cache keys) | `tests/render.rs` |
| `text_layer_tracemonkey`, `text_matrix_vs_pdfium`, `search_counts_match_pdfium` (+ binary round trip, hit rects, cheap text cache) | `tests/text.rs` |
| `history_roundtrip` (+ disk spill at a 4 MB budget, gesture coalescing) | `tests/history.rs` |
| `protocol_routes` (+ `protocol_generation_bump_invalidates`) | `tests/protocol.rs` |
| contract serde shapes, geometry, tile cache, PNG encode, search matching, route parsing, the 7881 constants | unit tests in `src/**` |

`tests/common/mod.rs`:

```rust
mod common;
use common::*;

#[test]
fn annot_markup_roundtrip() {
    let doc = open("tracemonkey.pdf");            // TestDoc; closed on drop
    let n = with_doc(&doc.doc_id, |d| {           // &mut OpenDoc on the engine thread
        Ok(d.page(0)?.annotations().len())
    }).unwrap();

    with_state(|st| { … })                        // &mut EngineState, for registry::mutate
        .unwrap();
}
```

* `engine()` — the process-wide `OnceLock<EngineHandle>`. `Pdfium::bind_to_library` works once
  per process and **every `tests/*.rs` file is its own process**, so one engine per file,
  shared by its tests; parallel test threads are fine because the engine is a thread behind a
  channel.
* `fixture("gen/500p.pdf")`, `all_fixtures()`, `open(name)`, `try_open(name, password)`.
* A document per test (ids are never reused), so tests do not interfere.
* Missing libpdfium panics with "run `npm run fetch:pdfium`".

Fixtures in `fixtures/gen/` (all deterministic, `examples/gen_fixtures.rs`):

| file | contents |
|---|---|
| `500p.pdf` | 500 letter pages, each stamped with its number (203 kB) |
| `outline-labels.pdf` | 6 pages, a 7-node / 3-level outline, `/PageLabels` `i, ii, 1, 2, App-A, App-B` |
| `encrypted-rc4-40.pdf` | standard security handler V=1 R=2 RC4-40, user `user`, owner `owner`, `/P -21` |
| `korean-300dpi.pdf` | **not generated yet** — see §4 |

The first three are written by a ~200-line PDF writer inside the example (with MD5 and RC4 for
the encrypted one) because PDFium can write **none** of an outline, page labels or encryption
(pages spike §5, §6, §8).

---

## 4. Open issues and known gaps

1. **`cargo fmt --check` and `cargo clippy -- -D warnings` could not be run here.** Neither
   component can be installed on this machine: `rustup component add rustfmt` / `clippy`
   fails with `Connection reset by peer` against `static.rust-lang.org` (the same corporate
   firewall that blocks crates.io, see `CLAUDE.md`). Compensations: the tree compiles with
   **zero warnings** under `cargo check --all-targets`, code was written in rustfmt style with
   no non-comment line over 100 columns, and the obvious clippy lints were applied by hand
   (`too_many_arguments` / `type_complexity` allows where deliberate, `collapsible_if`,
   `needless_range_loop`, `len_without_is_empty`). **Someone on a network that can reach
   `static.rust-lang.org` must run both commands before M0 is signed off**, and CI will run
   them on every push regardless.
2. **`fixtures/gen/korean-300dpi.pdf` is not generated.** It needs a Hangul font to embed, and
   PDFium embeds the *whole* font file with no subsetting: Arial Unicode produced a 15.3 MB
   fixture. The generator now waits for `src-tauri/resources/fonts/SeePDF-Hangul.ttf`
   (Stage 1 (b), `examples/build_hangul_font.rs`) and prints a skip line until then. Re-run
   `cargo run --release --example gen_fixtures` once the font lands.
3. **Provisional page rotation for documents over 64 pages** (D-4 above). If the page
   organizer needs exact rotation for every page of a 500-page document, the cheap fix is for
   it to refine visible rows: any `doc.page(i)` call updates `pages_meta[i]` as a side effect,
   so rendering a thumbnail already fixes that row.
4. **`OpenDoc::bytes` doubles the document's resident size.** `load_pdf_from_byte_vec` hands
   the `Vec` to pdfium, and `ARCHITECTURE.md` §2 also wants the bytes kept as the undo base and
   for `/raw`. A 100 MB scan is therefore ~200 MB resident. The follow-up is to drop `bytes`
   for documents over 48 MB and re-read from `path` on demand; it is behind the single
   `doc.bytes` accessor.
5. **Save is not implemented** (Stage 1 (b)), so `saved_generation` never moves and every
   document stays dirty after the first edit. `doc.saved_generation = doc.generation` belongs
   at the end of `save_document`, together with `emit("doc-saved", …)`.
6. **The native menu is English only.** It is built before the webview reports its locale and
   macOS caches it. Rebuilding it on `set_settings` is a P1 item. Item ids are the
   `UI_SPEC.md` §15.2 keys with the `menu.` prefix stripped (`menu.file.open` → `file.open`),
   and `app::menu::MENU_IDS` is the full list the frontend keymap must agree with.
7. **`engine_stats().rss_bytes` shells out to `ps` on macOS and returns 0 elsewhere.** Good
   enough for a status-bar reading; not for the perf harness.
8. **The tile cache budget is read from settings once, at startup** (`tileCacheMb`). Changing
   it takes a restart.
9. **No `engine-pressure` event is emitted yet.** The type exists (`EnginePressurePayload`);
   nothing decides when budgets are halved.
10. **`window_bind_document` records the binding but nothing consumes it yet.** Stage 2 wires
    "close the window → close the document".

---

## 5. Gotchas worth knowing before you edit engine code

1. **Drop order.** `PdfPage<'a>` borrows the *`Pdfium`* lifetime, not its document, so
   `drop(doc)` with pages alive compiles and is a use-after-free. `OpenDoc`'s field order is
   the drop order (`pages` then `doc`) and `PageLru::drop` clears itself; keep it that way.
   `registry_drop_order` is the regression test and it fails as a SIGSEGV, not an assertion.
2. **Never call `set_fill_color` / `set_stroke_color` on an annotation that has an `/AP`** —
   pdfium-render's fallback treats a `CPDF_AnnotContext*` as a `CPDF_PageObject*` and
   SIGSEGVs. Every annotation read from disk has an AP. Use raw
   `FPDFAnnot_SetAP(annot, NORMAL, NULL)` first (annotations spike §5.1).
3. **Quads are TL, TR, BL, BR.** `PdfQuadPoints::from_rect()` is BL,BR,TR,TL and renders a
   1-px sliver.
4. **Appearance streams are generated lazily, at render time.** Anything that creates a markup
   annotation must add its page to `doc.touched` so the pre-save pass
   (`render::tiles::generate_appearances`) renders it once; otherwise the annotation is
   invisible in Preview and Acrobat.
5. **Never `PdfRenderConfig::translate/scale/clip`** — any of them silently sets
   `render_form_data = false`. Tiles use `set_origin(-tx, -ty)`; that is what
   `render_tile_matches_crop` pins.
6. **Never `thumbnail(n)`** — it squashes the page to n×n and turns off annotation and form
   rendering. Use `set_target_width(w).set_maximum_height(w * 2)`.
7. **`PdfPageTextObject::text()` loads an entire `FPDF_TEXTPAGE` per call** (0.45 ms). Use
   `PdfPageText::for_object(&obj)` with one open text page.
8. **`PdfRect::new_from_values(bottom, left, top, right)`** — unusual argument order.
   `FS_RECTF` is `{left, top, right, bottom}`. Our `ipc::types::Rect` is `{l, b, r, t}`.
9. **`copy_pages_from_document(src, "1-3,5", at)` is 1-based**, while
   `copy_page_range_from_document(src, 2..=5, at)` is 0-based. Never mix them.
10. **Extract and split must copy the original and delete the other pages.**
    `FPDF_ImportPages*` keeps the widget annotations but drops `/AcroForm`, the outline and the
    metadata.
11. **The protocol handler must always respond.** A dropped `UriSchemeResponder` hangs the
    `<img>` until the webview times out, so every path — including `CmdStatus::Stale` and
    `Cancelled` — answers. Keep it that way when you add a route.
12. **Async `#[tauri::command]`s that borrow `State` must return `Result`** (macro restriction),
    and `tauri::async_runtime` re-exports tokio's mpsc but **not** `oneshot` — `Reply::oneshot()`
    is the zero-dependency substitute.
