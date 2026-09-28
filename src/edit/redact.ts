/**
 * 영역 표시 (F-22 redaction) flows for 편집 mode: pending marks → `redact_preview` (debounced, per
 * page) → 적용 → `apply_redactions` per page, sequentially, stopping on the first error.
 *
 * Marks are geometry only (page points, y-up) and live in `editStore` until applied. They survive
 * tool switches inside 편집, and are dropped when the mode is left (after a confirm — the leave
 * guard) or when the document moves under them (undo / redo / page ops / OCR): the preview would
 * describe content that is no longer there. Our own 편집 edits keep them and only re-preview.
 *
 * No React here; `redact.flow.test.tsx` drives it against the mock adapter.
 */
import * as api from "../ipc/api";
import type { DocChangedEvent, PageIndex, Point, Rect, RedactPreview } from "../ipc/types";
import { toast } from "../app/toastStore";
import { askConfirm } from "../dialogs/dialogState";
import { useAppStore } from "../store/appStore";
import { useDocStore } from "../store/docStore";
import { toolController } from "../tools/ToolController";
import { clearTextSelection, getTextLayer, isEmptySelection, orderSelection, pageRange, useSelectionStore } from "../viewer";
import { useEditStore, type RedactMark } from "./editStore";
import { hitObject } from "./geometry";

export const PREVIEW_DEBOUNCE_MS = 250;
/** a drag smaller than this (points, either side) is a click */
const MIN_MARK_PT = 2;

function docId(): string | null {
  return useEditStore.getState().docId ?? useDocStore.getState().info?.docId ?? null;
}

export function marksOn(page: PageIndex, marks: RedactMark[] = useEditStore.getState().marks): RedactMark[] {
  return marks.filter((m) => m.page === page);
}

export function markedPages(marks: RedactMark[] = useEditStore.getState().marks): PageIndex[] {
  return [...new Set(marks.map((m) => m.page))].sort((a, b) => a - b);
}

/** The topmost mark under a page point. */
export function markAt(marks: RedactMark[], x: number, y: number): RedactMark | null {
  for (let i = marks.length - 1; i >= 0; i--) {
    const r = marks[i].rect;
    if (x >= r.l && x <= r.r && y >= r.b && y <= r.t) return marks[i];
  }
  return null;
}

// ---------------------------------------------------------------------------
// Marking
// ---------------------------------------------------------------------------

/** A dragged rectangle; `false` when it was too small to be a mark. */
export function markArea(page: PageIndex, rect: Rect): boolean {
  if (rect.r - rect.l < MIN_MARK_PT || rect.t - rect.b < MIN_MARK_PT) return false;
  const [id] = useEditStore.getState().addMarks(page, [rect]);
  if (id !== undefined) useEditStore.getState().selectMark(id);
  return id !== undefined;
}

/** A click: mark the bounds of the text run (listed page object) under the point. */
export function markTextRunAt(page: PageIndex, at: Point): boolean {
  const objects = (useEditStore.getState().pages[page]?.objects ?? []).filter((o) => o.type === "text");
  const hit = hitObject(objects, at[0], at[1]);
  if (!hit) {
    useEditStore.getState().selectMark(null);
    return false;
  }
  const [id] = useEditStore.getState().addMarks(page, [hit.rect]);
  if (id !== undefined) useEditStore.getState().selectMark(id);
  return id !== undefined;
}

/**
 * 영역 표시로 표시 (text-selection context menu): the selection's line rects on every page it
 * touches become marks, then 편집 · 영역 표시 is armed. Returns the number of marks added.
 */
export function markTextSelection(): number {
  const info = useDocStore.getState().info;
  const selection = useSelectionStore.getState().selection;
  if (!info || !selection || selection.docId !== info.docId || isEmptySelection(selection)) return 0;
  const store = useEditStore.getState();
  store.bind(info.docId);
  const { start, end } = orderSelection(selection);
  let added = 0;
  for (let page = start.page; page <= end.page; page++) {
    const layer = getTextLayer(info.docId, info.docGeneration, page);
    const range = layer && pageRange(selection, page, layer.charCount);
    if (!layer || !range) continue;
    added += useEditStore.getState().addMarks(page, layer.rangeRects(range[0], range[1])).length;
  }
  if (added === 0) return 0;
  clearTextSelection();
  const app = useAppStore.getState();
  if (app.mode !== "edit") app.setMode("edit");
  useAppStore.getState().setTool("redact");
  toolController.arm("redact");
  return added;
}

// ---------------------------------------------------------------------------
// Preview
// ---------------------------------------------------------------------------

const timers = new Map<PageIndex, ReturnType<typeof setTimeout>>();
const latest = new Map<PageIndex, number>();
let seq = 0;

function cancelTimer(page: PageIndex): void {
  const timer = timers.get(page);
  if (timer !== undefined) clearTimeout(timer);
  timers.delete(page);
}

/** Re-preview `page` after the marks settle (typing a drag of marks costs one call). */
export function schedulePreview(page: PageIndex): void {
  cancelTimer(page);
  if (marksOn(page).length === 0) {
    latest.delete(page);
    useEditStore.getState().setPreview(page, null);
    return;
  }
  if (useEditStore.getState().previews[page]?.status !== "loading") {
    useEditStore.getState().setPreview(page, { status: "loading" });
  }
  timers.set(page, setTimeout(() => void runPreview(page), PREVIEW_DEBOUNCE_MS));
}

/** `redact_preview` for one page now; a reply overtaken by a newer request is dropped. */
export async function runPreview(page: PageIndex): Promise<RedactPreview | null> {
  cancelTimer(page);
  const doc = docId();
  const rects = marksOn(page).map((m) => m.rect);
  if (!doc || rects.length === 0) {
    useEditStore.getState().setPreview(page, null);
    return null;
  }
  const mine = ++seq;
  latest.set(page, mine);
  try {
    const result = await api.redactPreview({ docId: doc, page, rects });
    if (latest.get(page) === mine && marksOn(page).length) {
      useEditStore.getState().setPreview(page, { status: "ready", result });
    }
    return result;
  } catch {
    if (latest.get(page) === mine && marksOn(page).length) useEditStore.getState().setPreview(page, { status: "error" });
    return null;
  }
}

function stopPreviews(): void {
  for (const page of [...timers.keys()]) cancelTimer(page);
  latest.clear();
}

// ---------------------------------------------------------------------------
// Apply
// ---------------------------------------------------------------------------

let applying = false;

export function isApplying(): boolean {
  return applying;
}

/**
 * 적용: fresh previews (the form-field refusal must not hang on a debounced answer) → confirm →
 * `apply_redactions` per marked page in page order, stopping on the first error. Each page's
 * marks are dropped as soon as that page is applied, so a failure leaves only what was not done.
 */
export async function applyMarks(): Promise<boolean> {
  const doc = docId();
  const pages = markedPages();
  if (!doc || pages.length === 0 || applying) return false;
  applying = true;
  try {
    const previews = await Promise.all(pages.map((p) => runPreview(p)));
    const fields = previews.flatMap((p) => p?.formFields ?? []);
    if (fields.length) {
      toast("redact.formFields", { names: fields.join(", ") }, { tone: "danger" });
      return false;
    }
    const ok = await askConfirm({
      titleKey: "redact.confirmTitle",
      bodyKey: "redact.applyWarning",
      confirmKey: "redact.apply",
      danger: true,
    });
    if (!ok) return false;

    const { redactFill, redactOverlay } = useEditStore.getState();
    const overlayText = redactOverlay.trim();
    let removed = 0;
    for (const page of pages) {
      const rects = marksOn(page).map((m) => m.rect);
      if (rects.length === 0) continue;
      try {
        await api.applyRedactions({
          docId: doc,
          page,
          rects,
          options: overlayText ? { fill: redactFill, overlayText } : { fill: redactFill },
        });
      } catch (e) {
        const verify = api.isSeePdfError(e) && e.code === "verifyFailed";
        toast(verify ? "redact.verifyFailed" : api.errorKey(e), undefined, {
          tone: "danger",
          detail: e instanceof Error ? e.message : String(e),
        });
        void runPreview(page);
        return false;
      }
      removed += rects.length;
      cancelTimer(page);
      useEditStore.getState().clearMarks([page]);
    }
    toast("redact.done", { count: removed }, { tone: "success" });
    return true;
  } finally {
    applying = false;
  }
}

// ---------------------------------------------------------------------------
// Lifecycle (called from `edit/index.tsx` start / stop)
// ---------------------------------------------------------------------------

/** Leaving 편집 with marks: ask, and drop them on yes. `null` = nothing pending. */
export function confirmLeave(): Promise<boolean> | null {
  if (useEditStore.getState().marks.length === 0) return null;
  return askConfirm({
    titleKey: "redact.leaveTitle",
    bodyKey: "redact.leaveBody",
    confirmKey: "redact.discard",
    danger: true,
  }).then((ok) => {
    if (ok) dropMarks();
    return ok;
  });
}

export function dropMarks(): void {
  stopPreviews();
  useEditStore.getState().clearMarks();
}

/** The document moved: undo / redo / page ops / OCR drop the marks; our own edits re-preview. */
export function onDocChangedForMarks(e: DocChangedEvent): void {
  if (e.docId !== useEditStore.getState().docId || useEditStore.getState().marks.length === 0) return;
  if (e.reason === "save") return;
  if (e.reason === "edit" || e.reason === "redact") {
    if (applying) return; // our own apply; the loop owns the marks
    const pages = e.changedPages === "all" ? markedPages() : e.changedPages.filter((p) => marksOn(p).length);
    for (const p of pages) schedulePreview(p);
    return;
  }
  dropMarks();
  toast("redact.marksDropped");
}

/** Re-preview every page whose marks changed (a store subscription while 편집 is active). */
export function watchMarks(): () => void {
  for (const p of markedPages()) schedulePreview(p);
  return useEditStore.subscribe((s, prev) => {
    if (s.marks === prev.marks) return;
    const before = new Map<PageIndex, string>();
    const after = new Map<PageIndex, string>();
    for (const m of prev.marks) before.set(m.page, `${before.get(m.page) ?? ""}${m.id},`);
    for (const m of s.marks) after.set(m.page, `${after.get(m.page) ?? ""}${m.id},`);
    for (const p of new Set([...before.keys(), ...after.keys()])) {
      if (before.get(p) !== after.get(p) && !(applying && !after.has(p))) schedulePreview(p);
    }
  });
}
