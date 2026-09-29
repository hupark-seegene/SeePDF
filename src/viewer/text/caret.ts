/**
 * 캐럿 탐색 (H9, v0.3): F7 puts a text caret on the page, so selection and markup need no pointer.
 *
 * * ← / → move the caret one character, ↑ / ↓ one line (keeping its x), Home / End to the line's
 *   ends; at a page's edge the caret moves on to the next / previous page.
 * * ⇧ + any of those extends the **existing** selection store (`selection.ts`), so every shortcut that
 *   works on a mouse selection — ⌘C, the context menu, 형광펜 — works on this one too.
 * * With a selection, H / U / K make 형광펜 / 밑줄 / 취소선 from it; N puts a 메모 at the caret.
 *
 * This file is the state and the pure geometry (unit-tested); the key handler is `caretKeys.ts`,
 * mounted by the focused pane's scroller. It is small and dependency-free because the command
 * dispatcher (entry chunk) toggles it.
 */
import { create } from "zustand";
import type { DocId, PageIndex, Rect } from "../../ipc/types";
import type { PageTextLayer } from "./TextLayer";

export interface CaretState {
  on: boolean;
  docId: DocId | null;
  page: PageIndex;
  /** a char offset in `[0, charCount]`, like a selection end */
  offset: number;
  /** the x (PDF points) ↑ / ↓ aim for, so a column survives short lines; `null` = the caret's own */
  goalX: number | null;
  set(on: boolean, at?: { docId: DocId; page: PageIndex; offset: number }): void;
  moveTo(page: PageIndex, offset: number, goalX?: number | null): void;
}

export const useCaretStore = create<CaretState>((set) => ({
  on: false,
  docId: null,
  page: 0,
  offset: 0,
  goalX: null,
  set(on, at) {
    set(at ? { on, docId: at.docId, page: at.page, offset: at.offset, goalX: null } : { on });
  },
  moveTo(page, offset, goalX = null) {
    set({ page, offset, goalX });
  },
}));

function isBreak(layer: PageTextLayer, i: number): boolean {
  const code = layer.charCode(i);
  return code === 13 || code === 10;
}

/** A generated char (PDFium's `\r\n`) has no width. */
function hasBox(layer: PageTextLayer, i: number): boolean {
  const box = layer.charBox(i);
  return box.r > box.l;
}

/** The caret's x in PDF points: the left edge of the char after it, or the right edge of the one before. */
export function caretX(layer: PageTextLayer, offset: number): number {
  const n = layer.charCount;
  if (offset >= 0 && offset < n && hasBox(layer, offset)) return layer.charBox(offset).l;
  for (let i = Math.min(offset, n) - 1; i >= 0; i--) {
    if (hasBox(layer, i)) return layer.charBox(i).r;
  }
  for (let i = Math.max(0, offset + 1); i < n; i++) {
    if (hasBox(layer, i)) return layer.charBox(i).l;
  }
  return 0;
}

/** The line a caret offset sits on: the char after it, else the one before. */
export function caretLine(layer: PageTextLayer, offset: number): number {
  const after = layer.lineIndexOf(offset);
  if (after !== -1 && !isBreak(layer, offset)) return after;
  for (let i = offset - 1; i >= 0; i--) {
    const line = layer.lineIndexOf(i);
    if (line !== -1) return line;
  }
  for (let i = offset; i < layer.charCount; i++) {
    const line = layer.lineIndexOf(i);
    if (line !== -1) return line;
  }
  return -1;
}

/** The thin bar drawn for the caret, in PDF user space (zero width; the marks layer gives it 2 px). */
export function caretRect(layer: PageTextLayer, offset: number): Rect | null {
  if (layer.charCount === 0) return null;
  const line = caretLine(layer, offset);
  if (line < 0) return null;
  const box = layer.line(line).box;
  const x = caretX(layer, offset);
  return { l: x, r: x, b: box.b, t: box.t };
}

/**
 * One character left or right, clamped to the page (`null` past its edge). A generated `\r\n` pair
 * is one step, not two.
 */
export function stepChar(layer: PageTextLayer, offset: number, delta: 1 | -1): number | null {
  const n = layer.charCount;
  let next = offset + delta;
  if (next < 0 || next > n) return null;
  // passing a \r of a \r\n pair: take the \n with it
  const passed = delta > 0 ? next - 1 : next;
  if (layer.charCode(passed) === 13 && delta > 0 && next < n && layer.charCode(next) === 10) next += 1;
  if (layer.charCode(passed) === 10 && delta < 0 && next > 0 && layer.charCode(next - 1) === 13) next -= 1;
  return next;
}

/** The offset on the line `delta` lines away whose caret x is nearest `goalX`; `null` past the page. */
export function stepLine(layer: PageTextLayer, offset: number, delta: 1 | -1, goalX: number): number | null {
  const line = caretLine(layer, offset);
  const target = line + delta;
  if (line < 0 || target < 0 || target >= layer.lineCount) return null;
  return offsetOnLine(layer, target, goalX);
}

/** The offset on `line` whose caret x is nearest `x`. */
export function offsetOnLine(layer: PageTextLayer, line: number, x: number): number {
  const { firstChar, charCount } = layer.line(line);
  const end = Math.min(layer.charCount, firstChar + charCount);
  let best = firstChar;
  let bestDistance = Infinity;
  const consider = (offset: number, at: number) => {
    const d = Math.abs(at - x);
    if (d < bestDistance) {
      bestDistance = d;
      best = offset;
    }
  };
  let lastRight: number | null = null;
  let lastOffset = firstChar;
  for (let i = firstChar; i < end; i++) {
    if (isBreak(layer, i) || !hasBox(layer, i)) continue;
    const box = layer.charBox(i);
    consider(i, box.l);
    lastRight = box.r;
    lastOffset = i + 1;
  }
  // after the line's last visible char (not on the next line's first one)
  if (lastRight !== null) consider(lastOffset, lastRight);
  return best;
}

/** Home / End: the start or the end of the caret's line. */
export function lineEdge(layer: PageTextLayer, offset: number, end: boolean): number {
  const line = caretLine(layer, offset);
  if (line < 0) return offset;
  const { firstChar, charCount } = layer.line(line);
  if (!end) return firstChar;
  let last = firstChar + charCount;
  while (last > firstChar && isBreak(layer, last - 1)) last -= 1;
  return last;
}
