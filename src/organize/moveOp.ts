/**
 * Drag-reorder → exactly one `PageOp`.
 *
 * `FPDF_MovePages(pages, dest)` has ordered-list semantics (spikes/pages.md §1): the listed pages are
 * lifted out in the order given and re-inserted as one block, so `move([3,2] → 1)` on `A B C D` gives
 * `A D C B`. `dest` is therefore an index into the array *with the moved pages removed*, while the
 * grid's insertion caret counts every cell — `caretToDestination` is that conversion.
 *
 * F-16 / `organize.dnd`: one drag = one `page_ops` call = one generation = one undo step.
 */
import type { PageIndex, PageOp } from "../ipc/types";

/** Caret index in the displayed order → `dest` for `FPDF_MovePages`. */
export function caretToDestination(moving: readonly PageIndex[], caret: number, order: readonly PageIndex[]): number {
  const before = order.slice(0, caret).filter((p) => moving.includes(p)).length;
  const rest = order.length - moving.length;
  return Math.max(0, Math.min(rest, caret - before));
}

/** The display order after dropping `moving` at `caret`, without waiting for the engine. */
export function applyMove(
  order: readonly PageIndex[],
  moving: readonly PageIndex[],
  caret: number,
): PageIndex[] {
  const block = order.filter((p) => moving.includes(p));
  const rest = order.filter((p) => !moving.includes(p));
  const dest = caretToDestination(moving, caret, order);
  return [...rest.slice(0, dest), ...block, ...rest.slice(dest)];
}

/** True when dropping there changes nothing (the block is already at the caret). */
export function isNoOpMove(order: readonly PageIndex[], moving: readonly PageIndex[], caret: number): boolean {
  const next = applyMove(order, moving, caret);
  return next.length === order.length && next.every((p, i) => p === order[i]);
}

/**
 * The single op a drop commits, or `null` when the drop changes nothing.
 * `moving` is taken in document order — that is what the grid drags and what the engine expects.
 */
export function moveOpFor(
  order: readonly PageIndex[],
  moving: readonly PageIndex[],
  caret: number,
): Extract<PageOp, { kind: "move" }> | null {
  const block = order.filter((p) => moving.includes(p));
  if (block.length === 0 || block.length === order.length) return null;
  if (isNoOpMove(order, block, caret)) return null;
  return { kind: "move", pages: [...block], to: caretToDestination(block, caret, order) };
}

/** Identity order for a document of `count` pages. */
export function identityOrder(count: number): PageIndex[] {
  return Array.from({ length: count }, (_, i) => i);
}
