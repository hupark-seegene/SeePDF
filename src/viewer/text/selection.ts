/**
 * The text selection model (F-06).
 *
 * A selection is two char offsets, each on a page — like a DOM range, and like a DOM range it can
 * run backwards (the anchor is where the drag started). It lives in a tiny store rather than in a
 * ref, because the selection rectangles are React-rendered; the *drag* itself stays in refs and
 * only commits here when the offsets actually change.
 */
import { create } from "zustand";
import type { DocId, PageIndex } from "../../ipc/types";
import { getTextLayer } from "./textLayers";

export interface TextPoint {
  page: PageIndex;
  /** char offset in `[0, charCount]` */
  offset: number;
}

export interface TextSelection {
  docId: DocId;
  anchor: TextPoint;
  focus: TextPoint;
}

export interface OrderedSelection {
  start: TextPoint;
  end: TextPoint;
}

export function orderSelection(selection: TextSelection): OrderedSelection {
  const { anchor, focus } = selection;
  const backwards = focus.page < anchor.page || (focus.page === anchor.page && focus.offset < anchor.offset);
  return backwards ? { start: focus, end: anchor } : { start: anchor, end: focus };
}

export function isEmptySelection(selection: TextSelection | null): boolean {
  if (!selection) return true;
  const { start, end } = orderSelection(selection);
  return start.page === end.page && start.offset === end.offset;
}

/** The `[start, end)` char range a page contributes to a selection, or `null` when it is outside. */
export function pageRange(
  selection: TextSelection | null,
  page: PageIndex,
  charCount: number,
): [number, number] | null {
  if (!selection) return null;
  const { start, end } = orderSelection(selection);
  if (page < start.page || page > end.page) return null;
  const from = page === start.page ? start.offset : 0;
  const to = page === end.page ? end.offset : charCount;
  const a = Math.max(0, Math.min(charCount, Math.min(from, to)));
  const b = Math.max(0, Math.min(charCount, Math.max(from, to)));
  return b > a ? [a, b] : null;
}

export interface SelectionState {
  selection: TextSelection | null;
  setSelection(selection: TextSelection | null): void;
  clear(): void;
}

export const useSelectionStore = create<SelectionState>((set) => ({
  selection: null,
  setSelection(selection) {
    set({ selection });
  },
  clear() {
    set({ selection: null });
  },
}));

/**
 * The string the clipboard gets: every page the selection touches, in order, joined by a newline.
 * Pages whose text layer is not loaded contribute nothing — they cannot be selected either, since
 * the drag hit-tests against the same layers.
 */
export function selectionText(selection: TextSelection | null, generation: number): string {
  if (!selection || isEmptySelection(selection)) return "";
  const { start, end } = orderSelection(selection);
  const parts: string[] = [];
  for (let page = start.page; page <= end.page; page++) {
    const layer = getTextLayer(selection.docId, generation, page);
    if (!layer) continue;
    const range = pageRange(selection, page, layer.charCount);
    if (!range) continue;
    const text = layer.rangeText(range[0], range[1]);
    if (text) parts.push(text);
  }
  return parts.join("\n");
}
