//! 첨부 파일 (v0.3 S4): the document-level attachments (`/Names /EmbeddedFiles`).
//!
//! Everything goes through PDFium (`FPDFDoc_GetAttachment*`, `FPDFDoc_AddAttachment`,
//! `FPDFAttachment_SetFile`, `FPDFDoc_DeleteAttachment`), so adding and deleting are ordinary
//! [`registry::mutate`] edits: one undo step each, and a signed document keeps its
//! incremental save (S1). PDFium's delete only unlinks the entry; the full-rewrite save drops
//! the now unreferenced file stream.
//!
//! Attachment annotations (`FileAttachment`, a paperclip on a page) are not listed here: they
//! belong to their page. 문서 정리 (S3) removes both kinds.

use crate::engine::pages::write_atomic;
use crate::engine::registry::{self, MutateOpts, OpenDoc};
use crate::engine::types::EngineState;
use crate::ipc::error::PdfiumResultExt;
use crate::ipc::types::{AttachmentInfo, BytesWritten, ChangeReason};
use crate::ipc::EngineError;
use std::path::Path;

/// Attachments larger than this are refused on add (the whole file is held in memory and
/// copied into every undo snapshot).
pub const MAX_ATTACHMENT_BYTES: u64 = 256 * 1024 * 1024;

/// `list_attachments`, in PDFium's order.
pub fn list(doc: &OpenDoc<'_>) -> Vec<AttachmentInfo> {
    doc.pdf()
        .attachments()
        .iter()
        .enumerate()
        .map(|(i, a)| AttachmentInfo {
            index: i as u32,
            name: a.name(),
            size: a.len() as u64,
        })
        .collect()
}

/// `save_attachment` — writes attachment `index` to `out_path` (atomically).
pub fn save(doc: &OpenDoc<'_>, index: u32, out_path: &str) -> Result<BytesWritten, EngineError> {
    let bytes = get(doc, index)?.save_to_bytes().ctx("read attachment")?;
    let bytes_written = write_atomic(Path::new(out_path), &bytes)?;
    Ok(BytesWritten {
        bytes: bytes_written,
    })
}

fn get<'a>(
    doc: &'a OpenDoc<'_>,
    index: u32,
) -> Result<pdfium_render::prelude::PdfAttachment<'a>, EngineError> {
    let len = doc.pdf().attachments().len();
    if index as usize >= len as usize {
        return Err(EngineError::not_found(format!(
            "attachment {index} of {len}"
        )));
    }
    doc.pdf()
        .attachments()
        .get(index as _)
        .ctx("load attachment")
}

/// `add_attachment` — embeds the file at `path` under `name` (default: the file name; a
/// name already in use gets ` (2)`, ` (3)`… before its extension). One undo step
/// (`undo.attachmentAdd`). Returns the new list.
pub fn add(
    st: &mut EngineState<'_>,
    doc_id: &str,
    path: &str,
    name: Option<&str>,
) -> Result<Vec<AttachmentInfo>, EngineError> {
    let source = Path::new(path);
    let size = std::fs::metadata(source).map_err(EngineError::from)?.len();
    if size > MAX_ATTACHMENT_BYTES {
        return Err(EngineError::invalid(format!(
            "the file is {size} bytes; attachments may be at most {MAX_ATTACHMENT_BYTES}"
        )));
    }
    let bytes = std::fs::read(source).map_err(EngineError::from)?;
    let wanted = name
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .map(str::to_owned)
        .or_else(|| source.file_name().map(|n| n.to_string_lossy().to_string()))
        .unwrap_or_else(|| "attachment".to_string());
    let taken: Vec<String> = list(st.doc(doc_id)?).into_iter().map(|a| a.name).collect();
    let name = unique_name(&wanted, &taken);
    let opts = MutateOpts::new("undo.attachmentAdd", ChangeReason::Edit).keeps_text();
    registry::mutate(st, doc_id, opts, |doc| {
        doc.pdf_mut()
            .attachments_mut()
            .create_attachment_from_bytes(&name, &bytes)
            .map(|_| ())
            .ctx("add attachment")
    })?;
    Ok(list(st.doc(doc_id)?))
}

/// `delete_attachment` — one undo step (`undo.attachmentDelete`). Returns the new list.
pub fn delete(
    st: &mut EngineState<'_>,
    doc_id: &str,
    index: u32,
) -> Result<Vec<AttachmentInfo>, EngineError> {
    get(st.doc(doc_id)?, index)?;
    let opts = MutateOpts::new("undo.attachmentDelete", ChangeReason::Edit).keeps_text();
    registry::mutate(st, doc_id, opts, |doc| {
        doc.pdf_mut()
            .attachments_mut()
            .delete_at_index(index as _)
            .ctx("delete attachment")
    })?;
    Ok(list(st.doc(doc_id)?))
}

/// `wanted`, or `stem (n).ext` for the first `n ≥ 2` nobody uses.
pub fn unique_name(wanted: &str, taken: &[String]) -> String {
    if !taken.iter().any(|t| t == wanted) {
        return wanted.to_string();
    }
    let (stem, ext) = match wanted.rfind('.') {
        Some(dot) if dot > 0 => (&wanted[..dot], &wanted[dot..]),
        _ => (wanted, ""),
    };
    (2..)
        .map(|n| format!("{stem} ({n}){ext}"))
        .find(|candidate| !taken.iter().any(|t| t == candidate))
        .expect("an unbounded range always finds a free name")
}

#[cfg(test)]
mod tests {
    use super::unique_name;

    #[test]
    fn unique_names_keep_the_extension() {
        let taken = vec!["보고서.xlsx".to_string(), "보고서 (2).xlsx".to_string()];
        assert_eq!(unique_name("메모.txt", &taken), "메모.txt");
        assert_eq!(unique_name("보고서.xlsx", &taken), "보고서 (3).xlsx");
        assert_eq!(unique_name("README", &["README".to_string()]), "README (2)");
    }
}
