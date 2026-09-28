/**
 * 분할 보기 (P2): what the two scrollers of one document tell each other — nothing per frame goes
 * through React or a store (ARCHITECTURE §10).
 *
 * * **동기화 스크롤.** A pane that scrolls moves the other by the same number of pages (page units,
 *   `layout.positionAt`), so the two keep whatever distance the user put between them, at any zoom.
 *   The pane moved programmatically ignores its own scroll events for a beat, so the move never
 *   echoes back.
 * * **One viewport hint for two panes.** `set_viewport` is one per engine; each pane publishes its
 *   on-screen pages here and the hint covers both (first = the lower, last = the higher, centre =
 *   the focused pane's), so the engine never drops the other pane's renders as "scrolled away".
 */
import type { PageIndex } from "../ipc/types";
import type { PaneId } from "../store/viewStore";

export interface PaneHandle {
  /** where the pane is: the page at the top of its viewport + the fraction scrolled into it */
  position(): number;
  /** scroll so `position` is at the top (a 단일-mode pane may turn the page instead) */
  scrollToPosition(position: number): void;
}

export interface PaneRange {
  first: PageIndex;
  last: PageIndex;
  centre: PageIndex;
}

/** How long a pane moved by the other one ignores its own scroll events. */
export const ECHO_MS = 160;

const handles = new Map<PaneId, PaneHandle>();
const lastPosition = new Map<PaneId, number>();
const quietUntil = new Map<PaneId, number>();
const ranges = new Map<PaneId, PaneRange>();

function now(): number {
  return typeof performance === "undefined" ? Date.now() : performance.now();
}

export function registerPane(id: PaneId, handle: PaneHandle): () => void {
  handles.set(id, handle);
  lastPosition.set(id, handle.position());
  return () => {
    if (handles.get(id) !== handle) return;
    handles.delete(id);
    lastPosition.delete(id);
    quietUntil.delete(id);
    ranges.delete(id);
  };
}

/**
 * A pane scrolled (called from its scroll tick). With `sync` on, the other pane follows by the
 * same page delta; a pane that was just moved that way only records where it is.
 */
export function paneScrolled(id: PaneId, sync: boolean): void {
  const handle = handles.get(id);
  if (!handle) return;
  const position = handle.position();
  const previous = lastPosition.get(id);
  lastPosition.set(id, position);
  if ((quietUntil.get(id) ?? 0) > now()) return;
  if (!sync || previous === undefined) return;
  const delta = position - previous;
  if (Math.abs(delta) < 1e-4) return;
  for (const [other, h] of handles) {
    if (other === id) continue;
    const target = Math.max(0, h.position() + delta);
    quietUntil.set(other, now() + ECHO_MS);
    lastPosition.set(other, target);
    h.scrollToPosition(target);
  }
}

/** 동기화 스크롤 was just switched on: start counting from where both panes are now. */
export function resyncPanes(): void {
  for (const [id, h] of handles) lastPosition.set(id, h.position());
}

/** A pane's on-screen pages; returns the hint covering every pane. */
export function viewportFor(id: PaneId, range: PaneRange, focused: PaneId): PaneRange {
  ranges.set(id, range);
  let first = range.first;
  let last = range.last;
  for (const r of ranges.values()) {
    first = Math.min(first, r.first);
    last = Math.max(last, r.last);
  }
  const centre = (ranges.get(focused) ?? range).centre;
  return { first, last, centre };
}

/** Test seam. */
export function resetPanes(): void {
  handles.clear();
  lastPosition.clear();
  quietUntil.clear();
  ranges.clear();
}
