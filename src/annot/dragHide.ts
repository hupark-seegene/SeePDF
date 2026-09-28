/**
 * Hide the annotation while it is dragged (P1-12; STAGE1D_NOTES §7 item 6).
 *
 * Moving or resizing an annotation used to show it twice: PDFium's page bitmap still had it at
 * the old place while the overlay drew an outline at the new one. Now the first live patch of a
 * 선택-tool drag
 *
 * 1. repaints the dragged annotations **in full** in the overlay (as ghosts with no generation,
 *    so nothing drops them early),
 * 2. holds the coalesced patches (`holdPatchFlush`) — no `update_annotation` while hidden,
 * 3. calls `set_annotations_hidden(page, ids, true)` and, once it answers, puts the returned
 *    `viewNonce` into that page's bitmap URLs (`annotStore.viewNonce`), so the page re-renders
 *    without them. Only the overlay copy moves.
 *
 * The drop (and pointercancel, a tool switch mid-drag, the host unmounting) ends the session in
 * this order: wait for the hide → `hidden: false` → release the hold → flush the one coalesced
 * `update_annotation` → settle the ghosts at the page's new generation, so they disappear the
 * moment the new bitmap (which has the annotation at its new place) paints. Unhiding **before**
 * the update keeps the HIDDEN bit out of the undo snapshot; every step tolerates the one
 * before it failing, so an annotation can never be left hidden by an error.
 *
 * Only annotations the overlay can repaint are hidden (`canRepaint`). Since Stage 8 that includes
 * image stamps, foreign stamps and image signatures: before the hide, `snapshots.ts` takes the
 * engine's pixels of their rectangle and the ghost paints that bitmap (an outline if it fails).
 *
 * Stage 8 safety (`dragGate.ts`): the session holds the gate from the first live patch until the
 * drop has settled, so an autosave beat, ⌘S and ⌘Z wait (or are refused) instead of capturing
 * the transient HIDDEN bit; and the hide itself waits for an autosave write already in flight.
 */
import * as api from "../ipc/api";
import type { Annot, AnnotId, AnnotPatch, PageIndex } from "../ipc/types";
import { useAnnotStore } from "../store/annotStore";
import { useDocStore } from "../store/docStore";
import { toolController } from "../tools/ToolController";
import { isBuiltinStampKind } from "../tools/stampCatalog";
import {
  annotsOnPage,
  flushPatches,
  holdPatchFlush,
  patchAnnotation,
  sendPatches,
  takePendingPatches,
  type PendingPatch,
} from "./actions";
import { dragBegan, dragSettled, hasPendingWrites, writesSettled } from "./dragGate";
import { needsSnapshot, pruneSnapshots, takeSnapshots } from "./snapshots";

interface Session {
  docId: string;
  page: PageIndex;
  /** the ids actually hidden (a subset of the dragged ones) */
  ids: AnnotId[];
  /** the page's generation when the drag started */
  generation: number;
  /** resolves with the hide's `viewNonce`, or `null` when it failed / hid nothing */
  hide: Promise<number | null>;
  closed: boolean;
  /** the page's URLs switched to the hidden render */
  applied: boolean;
  /** the patches this drag made, taken out of the queue at its drop */
  batch: PendingPatch[];
}

let session: Session | null = null;
let ending: Promise<void> = Promise.resolve();

const PAINTED: ReadonlySet<Annot["kind"]> = new Set([
  "highlight", "underline", "strikeout", "squiggly", "note", "square", "circle", "line", "arrow", "textbox",
]);

/** Can `AnnotShape` draw `a` well enough that the bitmap copy may go? */
export function canRepaint(a: Annot): boolean {
  if (a.id.startsWith("ghost-")) return false; // not in the engine yet
  if (a.kind === "ink") return (a.inkPaths?.length ?? 0) > 0;
  // drawn signatures are strokes; typed / image ones are stamps painted from a snapshot
  if (a.kind === "signature") return (a.inkPaths?.length ?? 0) > 0 || needsSnapshot(a);
  if (a.kind === "stamp") return isBuiltinStampKind(a.stampKind) || needsSnapshot(a);
  return PAINTED.has(a.kind);
}

/** The page a drag is hiding annotations on, `null` when none is. */
export function dragHidePage(): PageIndex | null {
  return session?.page ?? null;
}

function begin(page: PageIndex, ids: AnnotId[]): void {
  const docId = useDocStore.getState().info?.docId;
  if (!docId) return;
  const store = useAnnotStore.getState();
  const targets = annotsOnPage(page).filter((a) => ids.includes(a.id) && canRepaint(a));
  holdPatchFlush(true);
  dragBegan();
  // snapshots of ghosts already gone are no longer needed
  pruneSnapshots(new Set(store.ghosts.map((g) => g.annot.id)));
  for (const a of targets) store.addGhost({ ...a });
  const s: Session = {
    docId,
    page,
    ids: targets.map((a) => a.id),
    generation: store.pageGeneration[page] ?? useDocStore.getState().info?.docGeneration ?? 0,
    hide: Promise.resolve(null),
    closed: false,
    applied: false,
    batch: [],
  };
  // Grabbed again before the previous drop has settled: that drop must unhide and commit first,
  // or its `update_annotation` would snapshot this drag's HIDDEN bit (⌘Z would bring it back hidden).
  const previous = ending;
  if (s.ids.length) {
    // The pixels of what the overlay cannot draw, and any autosave write in flight, come first:
    // both must see the annotation before it is hidden.
    const shots = targets.filter(needsSnapshot);
    const rotation = useDocStore.getState().info?.pages[page]?.rotation ?? 0;
    const before =
      shots.length || hasPendingWrites()
        ? Promise.all([shots.length ? takeSnapshots(docId, page, rotation, shots) : undefined, writesSettled()])
        : null;
    const hide = () => api.setAnnotationsHidden({ docId, page, ids: s.ids, hidden: true });
    s.hide = previous
      .then(() => (before ? before.then(hide) : hide()))
      .then(({ viewNonce }) => {
        // A drop that beat the answer must not switch the page to the hidden render.
        if (!s.closed) {
          useAnnotStore.getState().setViewNonce(page, viewNonce);
          s.applied = true;
        }
        return viewNonce;
      })
      .catch(() => null);
  }
  session = s;
}

/**
 * What the annotation host's sink calls for every move/resize patch of the 선택 tool:
 * `live` patches start (or continue) a drag, the final one ends it.
 */
export function dragPatch(page: PageIndex, edits: { id: AnnotId; patch: AnnotPatch }[], live: boolean): void {
  if (live && !session) begin(page, edits.map((e) => e.id));
  for (const edit of edits) patchAnnotation(page, edit.id, edit.patch, live);
  if (!live) void endDrag();
}

/** Drop, cancel, tool switch, unmount: restore and commit. Safe to call when nothing is active. */
export function endDrag(): Promise<void> {
  const s = session;
  if (!s) return ending;
  session = null;
  s.closed = true;
  s.batch = takePendingPatches();
  ending = finish(s);
  return ending;
}

async function finish(s: Session): Promise<void> {
  try {
    await restoreAndCommit(s);
  } finally {
    // autosave, ⌘S and ⌘Z may proceed: nothing is hidden any more and the move is committed
    dragSettled();
  }
}

async function restoreAndCommit(s: Session): Promise<void> {
  let restored: number | null = null;
  try {
    await s.hide;
    if (s.ids.length) {
      // Even when the hide failed: it may have reached the engine and lost only its answer.
      const out = await api.setAnnotationsHidden({ docId: s.docId, page: s.page, ids: s.ids, hidden: false });
      restored = out.viewNonce;
    }
  } catch {
    // the document may be gone (closed mid-drag); nothing left to restore
  } finally {
    // A newer drag (grabbed before this drop settled) holds the queue now: its patches wait for
    // its own drop.
    if (!session) holdPatchFlush(false);
  }
  await sendPatches(s.batch).catch(() => undefined);
  if (!session) await flushPatches().catch(() => undefined);
  const store = useAnnotStore.getState();
  // the newer drag's ghosts of the same annotations are its own to settle
  const regrabbed = new Set(session && session.page === s.page ? session.ids : []);
  const generation = store.pageGeneration[s.page] ?? s.generation;
  const moved = generation > s.generation;
  // Nothing committed (a failed update, a cancel before any movement) but the page did switch to
  // the hidden render: ask for a fresh one with the annotations back.
  const rerender = !moved && s.applied && restored !== null;
  if (rerender) store.setViewNonce(s.page, restored);
  for (const id of s.ids) {
    if (regrabbed.has(id)) continue;
    // The ghost goes when a bitmap of `generation` paints — the moved one, or the re-render.
    // If the bitmap never lost the annotation, the ghost goes now.
    if (moved || rerender) store.settleGhost(id, generation);
    else store.clearGhostById(id);
  }
}

/** A tool switch mid-drag cancels the gesture without an `onUp`: end the session then too. */
export function startDragHide(): () => void {
  const off = toolController.subscribe(() => {
    if (session && !toolController.active()) void endDrag();
  });
  return () => {
    off();
    void endDrag();
  };
}
