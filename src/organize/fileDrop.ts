/**
 * 페이지 mode drops (v0.3 P2 / P3): files from the OS and pages from another window land **where
 * they are dropped** — at the insertion caret under the pointer — instead of opening.
 *
 * The organizer registers how to turn a window point into a caret (`setOrganizerCaret`) while it
 * is mounted; `App.tsx`'s drop handler asks {@link dropFilesOnOrganizer} first and falls back to
 * opening the files when no organizer is on screen.
 *
 *  * a PDF → `page_ops [insertFrom]` at the caret (one file: its outline, labels and fields come
 *    along — v0.3 P1; several: one batch, one undo step);
 *  * images → `create_from_images` into a scratch document (A4, fitted), then
 *    `import_pages_from_doc` at the caret, and the scratch document is closed again.
 */
import * as api from "../ipc/api";
import { useDocStore } from "../store/docStore";
import { toast } from "../app/toastStore";
import type { PageOp } from "../ipc/types";

type CaretAt = (clientX: number, clientY: number) => number;

let caretAt: CaretAt | null = null;

/** The organizer's point → caret mapping while it is mounted (`null` when it unmounts). */
export function setOrganizerCaret(fn: CaretAt | null): void {
  caretAt = fn;
}

export function organizerMounted(): boolean {
  return caretAt !== null;
}

/** The caret for a window point, or the end of the document when there is no point. */
export function caretFor(position: { x: number; y: number } | undefined): number | null {
  if (!caretAt) return null;
  const count = useDocStore.getState().info?.pageCount ?? 0;
  if (!position) return count;
  return Math.max(0, Math.min(count, caretAt(position.x, position.y)));
}

const PDF_FILE = /\.pdf$/i;
const IMAGE_FILE = /\.(png|jpe?g)$/i;

/** `true` when the organizer took the drop. */
export async function dropFilesOnOrganizer(paths: string[], position?: { x: number; y: number }): Promise<boolean> {
  const info = useDocStore.getState().info;
  const at = caretFor(position);
  if (!info || at === null) return false;
  const pdfs = paths.filter((p) => PDF_FILE.test(p));
  const images = paths.filter((p) => IMAGE_FILE.test(p));
  if (!pdfs.length && !images.length) return false;
  const { runPageOps } = await import("../dialogs/flows");
  let cursor = at;
  if (pdfs.length) {
    // Several files are one batch (one undo step), every op inserting at the same caret — so they
    // run last file first, and the first file ends up first.
    const ops: PageOp[] = pdfs.map((path): PageOp => ({ kind: "insertFrom", at: cursor, path })).reverse();
    const before = info.pageCount;
    const ok = await runPageOps(ops);
    if (!ok) return true;
    cursor += (useDocStore.getState().info?.pageCount ?? before) - before;
  }
  if (images.length) await insertImages(images, cursor);
  return true;
}

/** Images at `at`: a scratch document from `create_from_images`, imported, then closed. */
async function insertImages(paths: string[], at: number): Promise<void> {
  const target = useDocStore.getState().info;
  if (!target) return;
  let scratch: string | null = null;
  try {
    const made = await api.createFromImages({ paths, pageSize: "a4", margin: 0, fit: "contain" }, () => undefined);
    scratch = made.docId;
    const pages = Array.from({ length: made.pageCount }, (_, i) => i);
    const next = await api.importPagesFromDoc({ srcDocId: made.docId, pages, dstDocId: target.docId, at });
    useDocStore.getState().adopt(next);
  } catch (e) {
    toast("error.generic", undefined, { tone: "danger", detail: e instanceof Error ? e.message : String(e) });
  } finally {
    if (scratch) await api.closeDocument({ docId: scratch }).catch(() => undefined);
  }
}
