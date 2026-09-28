//! Document structure — outline, links, page labels (P2, `IPC_CONTRACT.md` §7.10).
//!
//! Thin bodies over `engine::structure`: PDFium cannot write `/Outlines`, `/PageLabels` or a
//! link's `/Dest`, so those go through lopdf on the engine thread
//! (`registry::mutate_bytes_checked`: serialise → rewrite → reopen and check with PDFium →
//! replace, one undo step); a URI link, moving a link and deleting one stay with PDFium.

use crate::engine::structure::{labels, links, outline};
use crate::engine::{EngineHandle, Lane};
use crate::ipc::types::{
    AnnotResult, DocInfo, LinkTarget, OutlineNode, PageIndex, PageLabelRange, Rect,
};
use crate::ipc::EngineError;
use tauri::State;

/// 목차 편집 — replaces the whole outline (`[]` removes it); `undo.outlineEdit`.
#[tauri::command]
pub async fn set_outline(
    engine: State<'_, EngineHandle>,
    doc_id: String,
    nodes: Vec<OutlineNode>,
) -> Result<DocInfo, EngineError> {
    engine
        .call(Lane::Edit, "set_outline", move |st| {
            outline::set_outline(st, &doc_id, &nodes)
        })
        .await
}

/// 링크 만들기 — a Link annotation to a page (`{ page, x?, y?, zoom? }`) or a web address
/// (`{ url }`); `undo.linkCreate`.
#[tauri::command]
pub async fn create_link(
    engine: State<'_, EngineHandle>,
    doc_id: String,
    page: PageIndex,
    rect: Rect,
    target: LinkTarget,
) -> Result<AnnotResult, EngineError> {
    engine
        .call(Lane::Edit, "create_link", move |st| {
            links::create_link(st, &doc_id, page, rect, &target)
        })
        .await
}

/// 링크 편집 — a new click area and / or target; `undo.linkEdit`.
#[tauri::command]
pub async fn update_link(
    engine: State<'_, EngineHandle>,
    doc_id: String,
    page: PageIndex,
    id: String,
    rect: Option<Rect>,
    target: Option<LinkTarget>,
) -> Result<AnnotResult, EngineError> {
    engine
        .call(Lane::Edit, "update_link", move |st| {
            links::update_link(st, &doc_id, page, &id, rect, target.as_ref())
        })
        .await
}

/// 링크 삭제; `undo.linkDelete`.
#[tauri::command]
pub async fn delete_link(
    engine: State<'_, EngineHandle>,
    doc_id: String,
    page: PageIndex,
    id: String,
) -> Result<AnnotResult, EngineError> {
    engine
        .call(Lane::Edit, "delete_link", move |st| {
            links::delete_link(st, &doc_id, page, &id)
        })
        .await
}

/// 페이지 레이블 — replaces `/PageLabels` (`[]` removes it); `undo.pageLabels`.
#[tauri::command]
pub async fn set_page_labels(
    engine: State<'_, EngineHandle>,
    doc_id: String,
    ranges: Vec<PageLabelRange>,
) -> Result<DocInfo, EngineError> {
    engine
        .call(Lane::Edit, "set_page_labels", move |st| {
            labels::set_page_labels(st, &doc_id, &ranges)
        })
        .await
}

/// The document's `/PageLabels` ranges, for the 페이지 레이블 dialog.
#[tauri::command]
pub async fn get_page_labels(
    engine: State<'_, EngineHandle>,
    doc_id: String,
) -> Result<Vec<PageLabelRange>, EngineError> {
    engine
        .call(Lane::Interactive, "get_page_labels", move |st| {
            labels::get_page_labels(st, &doc_id)
        })
        .await
}
