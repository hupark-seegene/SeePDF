//! `inspect_pdf` — what is actually inside a PDF file, read through the SeePDF engine.
//!
//! ```sh
//! cd src-tauri && cargo run --release --example inspect_pdf -- ../fixtures/out/stage2/annotated.pdf
//! ```
//!
//! Written for the Stage 2 end-to-end smoke (`docs/STAGE2_INTEGRATION.md`): after the app has
//! saved a file, this reopens it with a *fresh* engine and prints the page list, every
//! annotation with its subtype / rect / colour / contents, every AcroForm field with its value,
//! and the head of each page's extracted text. It is the independent check that what the UI
//! showed really reached the bytes on disk — `list_annotations` in the running app reads the
//! in-memory document, this reads the file.
//!
//! Second argument (optional): `--text` to print more of the page text, `--page N` to restrict.

use seepdf_lib::engine::{registry, EngineHandle, Lane};
use seepdf_lib::ipc::EngineError;
use std::path::PathBuf;

fn main() {
    let mut args = std::env::args().skip(1);
    let Some(path) = args.next() else {
        eprintln!("usage: cargo run --release --example inspect_pdf -- <file.pdf> [--text] [--page N]");
        std::process::exit(2);
    };
    let rest: Vec<String> = args.collect();
    let want_text = rest.iter().any(|a| a == "--text");
    let only_page: Option<u16> = rest
        .iter()
        .position(|a| a == "--page")
        .and_then(|i| rest.get(i + 1))
        .and_then(|v| v.parse().ok());

    match run(&path, want_text, only_page) {
        Ok(()) => {}
        Err(e) => {
            eprintln!("inspect_pdf: {:?} — {}", e.code, e.message);
            std::process::exit(1);
        }
    }
}

fn engine() -> Result<EngineHandle, EngineError> {
    let (library, dir) = seepdf_lib::app::pdfium_path::resolve_local().map_err(EngineError::io)?;
    let spill = std::env::temp_dir().join(format!("seepdf-inspect-{}", std::process::id()));
    seepdf_lib::engine::spawn(
        library,
        dir,
        None,
        spill,
        seepdf_lib::engine::render::cache::DEFAULT_CAPACITY_BYTES,
    )
}

fn run(path: &str, want_text: bool, only_page: Option<u16>) -> Result<(), EngineError> {
    let engine = engine()?;
    let file = PathBuf::from(path);
    let bytes = std::fs::read(&file).map_err(EngineError::from)?;
    let size = bytes.len();
    let report = engine.call_blocking(Lane::Interactive, "inspect", move |st| {
        let info = registry::open(st, Some(file.clone()), bytes, None)?;
        let doc_id = info.doc_id.clone();
        let mut lines: Vec<String> = Vec::new();
        lines.push(format!(
            "file      {}\nbytes     {size}\npages     {}\npdf       {}  encrypted={}  form={}  outline={}  tagged={}",
            file.display(),
            info.page_count,
            info.pdf_version,
            info.encrypted,
            info.has_form,
            info.has_outline,
            info.tagged,
        ));

        let fields = seepdf_lib::engine::form::list(st.doc_mut(&doc_id)?, None)?;
        if !fields.is_empty() {
            lines.push(format!("form      {} field(s)", fields.len()));
            for f in &fields {
                lines.push(format!(
                    "  field p{} #{} {:?} name={:?} value={:?} checked={:?} readOnly={}",
                    f.page, f.index, f.field_type, f.name, f.value, f.checked, f.read_only
                ));
            }
        }

        let pages: Vec<u16> = match only_page {
            Some(p) => vec![p],
            None => (0..info.page_count).collect(),
        };
        for page in pages {
            let geom = st.doc(&doc_id)?.geom(page)?.clone();
            let annots = seepdf_lib::engine::annot::read::list_page(st.doc_mut(&doc_id)?, page)?;
            lines.push(format!(
                "page {page}   {:.1} x {:.1} pt  rotate={}  annots={}",
                geom.width_pt,
                geom.height_pt,
                geom.rotation,
                annots.len()
            ));
            for a in &annots {
                lines.push(format!(
                    "  annot {} {:?} subtype={} rect=({:.1},{:.1})-({:.1},{:.1}) color={:?} opacity={:.2} \
                     border={:.1} contents={:?} text={:?} author={:?}",
                    a.id,
                    a.kind,
                    a.subtype,
                    a.rect.l,
                    a.rect.b,
                    a.rect.r,
                    a.rect.t,
                    a.color,
                    a.opacity,
                    a.border_width,
                    truncate(&a.contents, 60),
                    a.text.as_deref().map(|t| truncate(t, 60)),
                    a.author,
                ));
            }
            let text = seepdf_lib::engine::text::layer::page_text(st.doc_mut(&doc_id)?, page)?
                .text
                .clone();
            let head = truncate(text.trim(), if want_text { 1200 } else { 160 });
            lines.push(format!("  text   {} chars: {head:?}", text.chars().count()));
        }
        registry::close(st, &doc_id)?;
        Ok(lines.join("\n"))
    })?;

    println!("{report}");
    Ok(())
}

fn truncate(s: &str, n: usize) -> String {
    let flat = s.replace(['\n', '\r'], " ");
    if flat.chars().count() <= n {
        return flat;
    }
    flat.chars().take(n).collect::<String>() + "…"
}
