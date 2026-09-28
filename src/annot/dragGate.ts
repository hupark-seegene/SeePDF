/**
 * Is an annotation drag hiding something right now? (P1-12 hide-while-dragging, Stage 8 safety.)
 *
 * While `dragHide` has an annotation hidden in the engine (`/F HIDDEN` via
 * `set_annotations_hidden`), nothing may snapshot the document: an autosave copy, a 저장 or an
 * undo step would capture the transient HIDDEN bit. This module is the gate they wait on. It
 * imports nothing, so the entry chunk (`app/autosave.ts`) can use it without pulling in the
 * annotation code.
 *
 * `dragHide` opens it on the first live patch and closes it once the drop has fully settled
 * (unhidden and the one `update_annotation` sent). Writers that were already in flight when a
 * drag starts are awaited before the hide (`trackWrite` / `writesSettled`).
 */

let active = 0;
let waiters: (() => void)[] = [];
const writes = new Set<Promise<unknown>>();
/** The annotation chunk's coalesced-patch queue (`actions.flushPatches`), once it is loaded. */
let flushEdits: (() => Promise<boolean>) | null = null;

/** A drag started hiding annotations. Pair with `dragSettled`. */
export function dragBegan(): void {
  active += 1;
}

/** The drag's restore + commit is done. */
export function dragSettled(): void {
  active = Math.max(0, active - 1);
  if (active > 0) return;
  const pending = waiters;
  waiters = [];
  for (const resolve of pending) resolve();
}

export function isDragHiding(): boolean {
  return active > 0;
}

/** Resolves at once when no drag is hiding anything, else after the drop has settled. */
export function whenDragIdle(): Promise<void> {
  if (active === 0) return Promise.resolve();
  return new Promise((resolve) => waiters.push(resolve));
}

/**
 * `annot/actions.ts` registers its patch queue here when it loads, so the entry chunk (autosave)
 * and the save flows can drain it without importing the annotation code. `flush` resolves `true`
 * when it sent something.
 */
export function registerEditFlush(flush: () => Promise<boolean>): void {
  flushEdits = flush;
}

/**
 * Everything the user has done is in the engine: no drag is hiding an annotation, and the last
 * nudge / slider patch still waiting out its coalescing delay has been sent. ⌘S, 다른 이름으로
 * 저장, the unsaved-changes prompt and an autosave beat wait on this. Resolves `true` when a
 * queued patch had to be sent (the document just changed).
 */
export async function whenEditsSettled(): Promise<boolean> {
  // a new drag may start while the previous one settles: wait until none is hiding anything
  while (active > 0) await whenDragIdle();
  return flushEdits ? flushEdits() : false;
}

/** A snapshotting write (an autosave copy) is in flight: a drag must not hide until it lands. */
export function trackWrite<T>(p: Promise<T>): Promise<T> {
  writes.add(p);
  const done = () => writes.delete(p);
  p.then(done, done);
  return p;
}

export function hasPendingWrites(): boolean {
  return writes.size > 0;
}

/** Every tracked write has landed (successfully or not). */
export function writesSettled(): Promise<void> {
  if (writes.size === 0) return Promise.resolve();
  return Promise.allSettled([...writes]).then(() => undefined);
}

/** Test seam. */
export function resetDragGate(): void {
  active = 0;
  const pending = waiters;
  waiters = [];
  for (const resolve of pending) resolve();
  writes.clear();
}
