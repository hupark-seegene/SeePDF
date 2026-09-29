/**
 * 영역 표시 (F-22 redaction) flows for 편집 mode: pending marks → `redact_preview` (debounced, per
 * page) → 적용 → ONE `apply_redactions_batch` for every marked page (one undo step; a failed
 * verification rolls all of them back, so the marks stay).
 *
 * v0.3 (R2): the engine removes exactly the characters under a mark and re-emits the rest of their
 * run, so a dragged mark is kept as drawn (Stage 8 snapped it to every run it crossed, because a run
 * went whole) and only clipped to the page box. A click still marks the whole run under it.
 *
 * v0.3 (R3): 검색해서 표시 — `findAndMark` runs the keyword / 개인정보 patterns (`redactPatterns.ts`)
 * over the text layer of every page in range, and 검색 ▸ 모든 결과를 영역 표시 (`markSearchHits`) turns
 * search hits into marks. Such marks carry a label (what matched) so the panel can list them for
 * review; each can be removed before 적용.
 *
 * Marks are geometry only (page points, y-up) and live in `editStore` until applied. They survive
 * tool switches inside 편집, and are dropped when the mode is left (after a confirm — the leave
 * guard) or when the document moves under them (undo / redo / page ops / OCR): the preview would
 * describe content that is no longer there. Our own 편집 edits keep them and only re-preview.
 * Closing or replacing the document asks first too (`dialogs/flows.ts` → `editLeaveGuard`).
 *
 * No React here; `redact.flow.test.tsx` drives it against the mock adapter.
 */
import * as api from "../ipc/api";
import type { DocChangedEvent, PageIndex, Point, Rect, RedactBatchMark, RedactPreview, SearchHit } from "../ipc/types";
import { toast } from "../app/toastStore";
import { askConfirm } from "../dialogs/dialogState";
import { useAppStore } from "../store/appStore";
import { useDocStore } from "../store/docStore";
import { toolController } from "../tools/ToolController";
import { clearTextSelection, getTextLayer, isEmptySelection, orderSelection, pageRange, PageTextLayer, useSelectionStore } from "../viewer";
import { useEditStore, type RedactMark } from "./editStore";
import { clipRect, hitObject } from "./geometry";
import { findAll, type PatternKind } from "./redactPatterns";

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

const EVERYWHERE: Rect = { l: -Infinity, b: -Infinity, r: Infinity, t: Infinity };

/** The page's box in user space (its crop box) — nothing outside it is visible or markable. */
export function pageBox(page: PageIndex): Rect {
  const info = useDocStore.getState().info;
  return info && info.docId === docId() ? info.pages[page]?.crop ?? EVERYWHERE : EVERYWHERE;
}

/**
 * What a drag of `rect` on `page` becomes: clipped to the page. (v0.3: no longer snapped to the text
 * runs it crosses — the engine splits a run instead of removing it whole.)
 */
export function markRectFor(page: PageIndex, rect: Rect): Rect | null {
  return clipRect(rect, pageBox(page));
}

/** A dragged rectangle (clipped); `false` when it was too small to be a mark. */
export function markArea(page: PageIndex, rect: Rect): boolean {
  if (rect.r - rect.l < MIN_MARK_PT || rect.t - rect.b < MIN_MARK_PT) return false;
  const snapped = markRectFor(page, rect);
  if (!snapped) return false;
  const [id] = useEditStore.getState().addMarks(page, [snapped]);
  if (id !== undefined) useEditStore.getState().selectMark(id);
  return id !== undefined;
}

/** A click: mark the bounds of the text run (listed page object) under the point. */
export function markTextRunAt(page: PageIndex, at: Point): boolean {
  const objects = (useEditStore.getState().pages[page]?.objects ?? []).filter((o) => o.type === "text");
  const hit = hitObject(objects, at[0], at[1]);
  const rect = hit && clipRect(hit.rect, pageBox(page));
  if (!rect) {
    useEditStore.getState().selectMark(null);
    return false;
  }
  const [id] = useEditStore.getState().addMarks(page, [rect]);
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
    const box = pageBox(page);
    const rects = layer.rangeRects(range[0], range[1]).flatMap((r) => clipRect(r, box) ?? []);
    added += useEditStore.getState().addMarks(page, rects).length;
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
  } catch (e) {
    const refused = api.isSeePdfError(e) && e.code === "verifyFailed";
    if (latest.get(page) === mine && marksOn(page).length) useEditStore.getState().setPreview(page, { status: "error", refused });
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
 * one `apply_redactions_batch` with every marked page — one undo step. On success exactly the
 * marks that were sent go (one drawn while the call ran stays); on any failure, `verifyFailed`
 * included, the engine rolled everything back and every mark stays.
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
    // v0.3 (R4): content inside a group (Form XObject) can only go after the group is ungrouped —
    // one confirm says both, and the engine ungroups in the same undo step.
    const ungroup = previews.some((p) => (p?.groups?.length ?? 0) > 0);
    // v0.3 integration (R2/R3 × S1): on a signed document the redaction forces a full rewrite at the
    // next save (an incremental save would keep the removed text in the signed revision) — say so.
    const signed = (useDocStore.getState().info?.signatures?.length ?? 0) > 0;
    const ok = await askConfirm(
      ungroup
        ? {
            titleKey: "redact.groups.title",
            bodyKey: signed ? "redact.groups.bodySigned" : "redact.groups.body",
            confirmKey: "redact.groups.confirm",
            danger: true,
          }
        : {
            titleKey: "redact.confirmTitle",
            bodyKey: signed ? "redact.applyWarningSigned" : "redact.applyWarning",
            confirmKey: "redact.apply",
            danger: true,
          },
    );
    if (!ok) return false;

    const { redactFill, redactOverlay } = useEditStore.getState();
    const overlayText = redactOverlay.trim();
    const sent = useEditStore.getState().marks.filter((m) => pages.includes(m.page));
    const marks: RedactBatchMark[] = pages
      .map((page) => ({ page, rects: sent.filter((m) => m.page === page).map((m) => m.rect) }))
      .filter((m) => m.rects.length > 0);
    if (marks.length === 0) return false;
    let collateral: string[] = [];
    try {
      const result = await api.applyRedactionsBatch({
        docId: doc,
        marks,
        options: {
          fill: redactFill,
          ...(overlayText ? { overlayText } : null),
          ...(ungroup ? { ungroup: true } : null),
        },
      });
      collateral = result.collateral ?? [];
    } catch (e) {
      const verify = api.isSeePdfError(e) && e.code === "verifyFailed";
      toast(verify ? "redact.verifyFailed" : api.errorKey(e), undefined, {
        tone: "danger",
        detail: e instanceof Error ? e.message : api.isSeePdfError(e) ? e.message : String(e),
      });
      for (const page of pages) schedulePreview(page);
      return false;
    }
    for (const page of pages) cancelTimer(page);
    useEditStore.getState().removeMarks(sent.map((m) => m.id));
    for (const m of sent) labels.delete(m.id);
    toast("redact.done", { count: sent.length }, { tone: "success" });
    // a run the preview promised to split went whole after all (its re-emission failed a check)
    if (collateral.length) toast("redact.splitFallback", { text: collateral.join(", ") }, { tone: "info" });
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
  labels.clear();
}

/** The document moved: undo / redo / page ops / OCR drop the marks; our own edits re-preview. */
export function onDocChangedForMarks(e: DocChangedEvent): void {
  if (e.docId !== useEditStore.getState().docId || useEditStore.getState().marks.length === 0) return;
  if (e.reason === "edit" || e.reason === "redact") {
    if (applying) return; // our own apply; the loop owns the marks
    const pages = e.changedPages === "all" ? markedPages() : e.changedPages.filter((p) => marksOn(p).length);
    for (const p of pages) schedulePreview(p);
    return;
  }
  dropMarks();
  toast("redact.marksDropped");
}

/**
 * Stage 9: a paragraph edit moved the content below it by `shifted` points (`shiftedPt`, > 0 = down)
 * out of `band` (`movedBand`: the paragraph's column, from its old bottom down to the moved stack).
 * Pending marks on that page in the column below the paragraph whose vertical centre lies in the band
 * move with the content — a mark drawn with a margin around the last moved line reaches below the band
 * and still moves — so they keep covering what they were drawn over (the engine moves annotations by
 * the same rule). A mark that lies mostly in the band without moving (it reaches up into the edited
 * paragraph) covers moved and unmoved content at once: it is dropped (with the notice) rather than
 * redact the wrong thing after the move.
 */
export function followFlow(page: PageIndex, band: Rect, shifted: number): void {
  if (Math.abs(shifted) < 0.05) return;
  const marks = marksOn(page);
  if (marks.length === 0) return;
  const slack = 0.15 * (band.r - band.l);
  const inColumn = (r: Rect) => r.l >= band.l - slack && r.r <= band.r + slack;
  const moves = (r: Rect) => inColumn(r) && r.t <= band.t + 1 && (r.b + r.t) / 2 >= band.b;
  const straddles = (r: Rect) => {
    const across = Math.min(r.r, band.r) - Math.max(r.l, band.l);
    const down = Math.min(r.t, band.t) - Math.max(r.b, band.b);
    return across > 0 && down > 0.5 * (r.t - r.b);
  };
  const move = marks.filter((m) => moves(m.rect)).map((m) => m.id);
  const drop = marks.filter((m) => !moves(m.rect) && straddles(m.rect)).map((m) => m.id);
  const store = useEditStore.getState();
  if (move.length) store.moveMarks(move, -shifted);
  if (drop.length) {
    store.removeMarks(drop);
    toast("redact.marksDropped");
  }
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

// ---------------------------------------------------------------------------
// v0.3 (R3) 검색해서 표시: labelled marks, pattern search over the text layers, search hits
// ---------------------------------------------------------------------------

/** What made a mark: a keyword, a 개인정보 pattern, a 검색 hit — or a hand-drawn area. */
export type MarkSource = "keyword" | PatternKind | "search" | "area";

interface MarkLabel {
  group: number;
  source: Exclude<MarkSource, "area">;
  text: string;
}

/** mark id → what matched (one match spanning two lines is two marks of one group) */
const labels = new Map<number, MarkLabel>();
let nextGroup = 1;

/** One reviewable entry of the panel's 표시 목록: every mark of one match, or one hand-drawn mark. */
export interface MarkGroup {
  key: string;
  page: PageIndex;
  ids: number[];
  source: MarkSource;
  text: string;
}

export function markGroups(marks: RedactMark[] = useEditStore.getState().marks): MarkGroup[] {
  const out: MarkGroup[] = [];
  const byGroup = new Map<number, MarkGroup>();
  for (const m of marks) {
    const label = labels.get(m.id);
    if (!label) {
      out.push({ key: `m${m.id}`, page: m.page, ids: [m.id], source: "area", text: "" });
      continue;
    }
    const known = byGroup.get(label.group);
    if (known) {
      known.ids.push(m.id);
      continue;
    }
    const group: MarkGroup = { key: `g${label.group}`, page: m.page, ids: [m.id], source: label.source, text: label.text };
    byGroup.set(label.group, group);
    out.push(group);
  }
  return out.sort((a, b) => a.page - b.page || a.ids[0] - b.ids[0]);
}

/**
 * A search-derived rect a hair narrower than the characters' boxes: the engine removes every
 * character whose ink the mark overlaps, and the boxes of kerned neighbours can overlap by a
 * fraction of a point.
 */
function inset(r: Rect): Rect {
  const dx = Math.min(0.25, (r.r - r.l) * 0.1);
  return { l: r.l + dx, b: r.b, r: r.r - dx, t: r.t };
}

function sameRect(a: Rect, b: Rect): boolean {
  return Math.abs(a.l - b.l) < 0.5 && Math.abs(a.b - b.b) < 0.5 && Math.abs(a.r - b.r) < 0.5 && Math.abs(a.t - b.t) < 0.5;
}

/** Adds the marks of one match (skipping any identical mark already there); returns their ids. */
function addLabelled(page: PageIndex, rects: Rect[], source: MarkLabel["source"], text: string): number[] {
  const box = pageBox(page);
  const existing = marksOn(page).map((m) => m.rect);
  const fresh = rects
    .flatMap((r) => clipRect(inset(r), box) ?? [])
    .filter((r) => !existing.some((e) => sameRect(e, r)));
  if (fresh.length === 0) return [];
  const ids = useEditStore.getState().addMarks(page, fresh);
  const group = nextGroup++;
  for (const id of ids) labels.set(id, { group, source, text: text.replace(/\s+/g, " ").trim() });
  return ids;
}

/** Switches to 편집 · 영역 표시 (after marks were added from outside the edit layer). */
function armRedact(): void {
  const app = useAppStore.getState();
  if (app.mode !== "edit") app.setMode("edit");
  useAppStore.getState().setTool("redact");
  toolController.arm("redact");
}

/**
 * 검색 ▸ 모든 결과를 영역 표시 / a result's 영역 표시: every hit's line rects become marks (labelled
 * with the matched text) and 편집 · 영역 표시 is armed. Returns the number of hits marked.
 */
export function markSearchHits(hits: SearchHit[]): number {
  const info = useDocStore.getState().info;
  if (!info || hits.length === 0) return 0;
  useEditStore.getState().bind(info.docId);
  let marked = 0;
  for (const hit of hits) {
    const [start, length] = hit.contextMatch;
    const text = hit.context.slice(start, start + length);
    if (addLabelled(hit.page, hit.rects, "search", text).length) marked += 1;
  }
  if (marked) armRedact();
  return marked;
}

export interface FindOptions {
  keyword: string;
  patterns: PatternKind[];
  pages: PageIndex[];
  matchCase?: boolean;
}

/** The page's characters as one string, and the character index of every UTF-16 unit of it. */
function layerText(layer: PageTextLayer): { text: string; charOf: number[] } {
  let text = "";
  const charOf: number[] = [];
  for (let i = 0; i < layer.charCount; i++) {
    const code = layer.charCode(i);
    const s = code > 0 && code <= 0x10ffff ? String.fromCodePoint(code) : "�";
    text += s;
    for (let k = 0; k < s.length; k++) charOf.push(i);
  }
  return { text, charOf };
}

/**
 * 검색해서 표시: the keyword and the chosen patterns over every page of `opts.pages` (each page's
 * text layer is fetched once); each match's character boxes become labelled marks. `onProgress`
 * after every page; `signal.cancelled` stops between pages (marks found so far stay). Returns the
 * number of matches marked.
 */
export async function findAndMark(
  opts: FindOptions,
  onProgress?: (done: number, total: number) => void,
  signal?: { cancelled: boolean },
): Promise<number> {
  const info = useDocStore.getState().info;
  const doc = docId();
  if (!info || !doc || (!opts.keyword.trim() && opts.patterns.length === 0)) return 0;
  useEditStore.getState().bind(doc);
  let marked = 0;
  let done = 0;
  for (const page of opts.pages) {
    if (signal?.cancelled) break;
    try {
      const layer = new PageTextLayer(await api.getTextLayer({ docId: doc, page }));
      const { text, charOf } = layerText(layer);
      for (const m of findAll(text, { keyword: opts.keyword, patterns: opts.patterns, matchCase: opts.matchCase })) {
        if (m.end <= m.start) continue;
        const rects = layer.rangeRects(charOf[m.start], charOf[m.end - 1] + 1);
        if (addLabelled(page, rects, m.kind, m.text).length) marked += 1;
      }
    } catch {
      // a page without a text layer (a scan without OCR) has nothing to find
    }
    done += 1;
    onProgress?.(done, opts.pages.length);
  }
  return marked;
}
