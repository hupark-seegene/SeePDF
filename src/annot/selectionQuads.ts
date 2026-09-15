/**
 * The bridge from the viewer's text selection to a markup annotation's quads (F-08).
 *
 * `PageTextLayer.rangeRects` already returns **one rectangle per line run**, which is exactly the
 * `rects` an `AnnotSpec` wants: the engine turns each into an `FS_QUADPOINTSF` in PDFium's
 * TL,TR,BL,BR order (IPC_CONTRACT §3 — `PdfQuadPoints::from_rect()` is the wrong order and renders
 * a 1-px sliver, spikes/annotations §1.4). Nothing here reorders or merges them: a 3-line
 * selection must stay 3 quads or the highlight paints over the gutter.
 */
import type { PageIndex, Rect } from "../ipc/types";
import { getTextLayer, orderSelection, pageRange, useSelectionStore } from "../viewer";
import { useDocStore } from "../store/docStore";

/** The line rectangles the current text selection contributes to one page, in user space. */
export function selectionRectsForPage(page: PageIndex): Rect[] {
  const selection = useSelectionStore.getState().selection;
  const info = useDocStore.getState().info;
  if (!selection || !info || selection.docId !== info.docId) return [];
  const { start, end } = orderSelection(selection);
  if (page < start.page || page > end.page) return [];
  const layer = getTextLayer(selection.docId, info.docGeneration, page);
  if (!layer) return [];
  const range = pageRange(selection, page, layer.charCount);
  if (!range) return [];
  return layer.rangeRects(range[0], range[1]);
}

/** Every page the current selection touches — a markup drag may cross a page boundary. */
export function selectedPages(): PageIndex[] {
  const selection = useSelectionStore.getState().selection;
  if (!selection) return [];
  const { start, end } = orderSelection(selection);
  const pages: PageIndex[] = [];
  for (let page = start.page; page <= end.page; page++) pages.push(page);
  return pages;
}

export function hasTextSelection(): boolean {
  const selection = useSelectionStore.getState().selection;
  if (!selection) return false;
  const { start, end } = orderSelection(selection);
  return start.page !== end.page || start.offset !== end.offset;
}
