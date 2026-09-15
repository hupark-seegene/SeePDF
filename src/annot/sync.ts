/**
 * Keeping `annotStore` honest across generations (IPC_CONTRACT §7.8, §8).
 *
 * Undo and redo do not tell us *what* they undid: the engine pops a snapshot and broadcasts a new
 * `docGeneration` with a `changedPages` set. So the reconciliation is "re-list those pages at the
 * new generation and trust the answer" — the store drops a reply that belongs to an older
 * generation (`setPage`'s guard), which is what keeps a slow `list_annotations` from resurrecting
 * an annotation the user has already undone.
 *
 * `changedPages: 'all'` (what undo/redo actually send) re-lists every page we have ever listed,
 * not every page of the document: a 500-page file must not fire 500 commands because of one ⌘Z.
 */
import * as api from "../ipc/api";
import { onDocChanged } from "../ipc/events";
import type { AnnotScanEvent, DocChangedEvent, DocId, PageIndex } from "../ipc/types";
import { useAnnotStore } from "../store/annotStore";
import { useDocStore } from "../store/docStore";
import { flushPatches } from "./actions";

/** The document the store currently describes; a change resets everything. */
let boundDoc: DocId | null = null;
let scanJob: number | null = null;

export async function reloadPages(docId: DocId, pages: PageIndex[]): Promise<void> {
  await Promise.all(
    pages.map(async (page) => {
      try {
        const list = await api.listAnnotations({ docId, page });
        useAnnotStore.getState().setPage(page, list.annots, list.docGeneration);
      } catch {
        // `unsupported` until (a) lands; keep what we have rather than blanking the overlay.
      }
    }),
  );
}

/** Pages worth re-listing for a `doc-changed`: the named ones, or everything already known. */
export function pagesToReload(changed: PageIndex[] | "all", known: PageIndex[]): PageIndex[] {
  if (changed === "all") return known;
  return [...new Set(changed)];
}

export async function applyDocChanged(e: DocChangedEvent): Promise<void> {
  const state = useAnnotStore.getState();
  if (boundDoc !== e.docId) return;
  // Stage 2: `canUndo`/`canRedo` now travel on `doc-changed` and `docStore.applyDocChanged`
  // folds them into `DocInfo`, so there is no `get_document` per annotation edit any more
  // (STAGE1D_NOTES §7.4). Only the undo *labels* still need a refresh, and the title bar does
  // not show them.
  const known = Object.keys(state.byPage).map(Number);
  const pages = pagesToReload(e.changedPages, known);
  if (pages.length === 0) return;
  await reloadPages(e.docId, pages);
}

/** One `scan_annotations` pass so the 주석 sidebar can list a document nobody has scrolled yet. */
export function scanDocument(docId: DocId): void {
  cancelScan();
  void api
    .scanAnnotations({ docId }, (event: AnnotScanEvent) => {
      if (event.type !== "page") return;
      if (useDocStore.getState().info?.docId !== docId) return;
      const generation = useDocStore.getState().info?.docGeneration;
      useAnnotStore.getState().setPage(event.page, event.annots, generation);
    })
    .then((jobId) => {
      scanJob = jobId;
    })
    .catch(() => {
      // `scan_annotations` is optional: the per-page `list_annotations` of the mounted pages still
      // fills the overlay, the sidebar just starts empty for pages nobody has visited.
    });
}

export function cancelScan(): void {
  if (scanJob !== null) {
    void api.cancelJob({ jobId: scanJob }).catch(() => undefined);
    scanJob = null;
  }
}

/**
 * Wire the store to the document. Returns an unsubscribe; called once by the annotation host.
 * `App.tsx` already routes `doc-changed` into `docStore`; this is a second, independent listener,
 * which is exactly what `events.ts` supports (every helper is a fan-out subscription).
 */
export function startAnnotSync(): () => void {
  const offDocChanged = onDocChanged((e) => void applyDocChanged(e));
  const offDoc = useDocStore.subscribe((s) => {
    const id = s.info?.docId ?? null;
    if (id === boundDoc) return;
    cancelScan();
    boundDoc = id;
    useAnnotStore.getState().reset();
    if (id) scanDocument(id);
  });
  // The store may already hold a document by the time the lazy chunk arrives.
  const current = useDocStore.getState().info?.docId ?? null;
  if (current && current !== boundDoc) {
    boundDoc = current;
    useAnnotStore.getState().reset();
    scanDocument(current);
  }
  return () => {
    offDocChanged();
    offDoc();
    cancelScan();
    boundDoc = null;
  };
}

/** Undo/redo: flush anything coalesced first, or the engine would undo a patch we never sent. */
export async function undoWithAnnots(): Promise<void> {
  await flushPatches();
  await useDocStore.getState().undo();
}

export async function redoWithAnnots(): Promise<void> {
  await flushPatches();
  await useDocStore.getState().redo();
}

/** Test seam. */
export function resetAnnotSync(): void {
  boundDoc = null;
  scanJob = null;
}

/** Test seam: bind without starting the subscriptions. */
export function bindDocument(docId: DocId | null): void {
  boundDoc = docId;
}
