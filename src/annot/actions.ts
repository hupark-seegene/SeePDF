/**
 * Everything that turns a gesture or a panel change into an IPC call, plus the optimism around it
 * (ARCHITECTURE §10, IPC_CONTRACT §7.1).
 *
 * Two behaviours are worth reading before changing anything here:
 *
 * * **Ghosts.** `create` pushes a locally-built `Annot` into `annotStore.ghosts` *before* the
 *   command runs, so the shape is on screen within a frame. When the command resolves we learn the
 *   generation that contains it (`settleGhost`); the ghost is only dropped once the page has been
 *   re-rendered at that generation (`annotStore.pageRendered`, fed by the bitmap `onload` in
 *   `AnnotOverlay`). Dropping it earlier flashes; never dropping it double-draws.
 * * **Coalescing.** Dragging a slider or an annotation produces one patch per pointer move. They
 *   are merged per (page, id) and sent once the drag idles for `PATCH_COALESCE_MS` or ends, which
 *   keeps the engine at one `update_annotation` — and therefore one undo step — per gesture.
 */
import * as api from "../ipc/api";
import type { Annot, AnnotId, AnnotPatch, AnnotSpec, DocId, PageIndex, Rect } from "../ipc/types";
import { useAnnotStore } from "../store/annotStore";
import { useAppStore } from "../store/appStore";
import { useDocStore } from "../store/docStore";
import { registerEditFlush } from "./dragGate";
import { boundsOfPaths, boundsOfRects } from "../tools/geometry";
import { NOTE_SIZE_PT } from "../tools/note";
import { askConfirm } from "../dialogs/dialogState";
import { toast } from "../app/toastStore";
import { descendantIds, threadRootId, withoutReplies } from "./threads";

/** A drag idling this long flushes its coalesced patch. 140 ms ≈ the panel's own repaint budget. */
export const PATCH_COALESCE_MS = 140;

let ghostSeq = 0;

function docId(): DocId | null {
  return useDocStore.getState().info?.docId ?? null;
}

/** A locally-built `Annot` for the optimistic overlay — the engine's version replaces it later. */
export function ghostFromSpec(page: PageIndex, spec: AnnotSpec, author: string | null): Annot {
  const now = new Date().toISOString();
  const base: Annot = {
    id: `ghost-${++ghostSeq}`,
    page,
    kind: spec.kind === "signature" || (spec.kind === "stamp" && spec.signature) ? "signature" : (spec.kind as Annot["kind"]),
    subtype: "Square",
    rect: { l: 0, b: 0, r: 0, t: 0 },
    color: [0, 0, 0],
    fillColor: null,
    opacity: 1,
    borderWidth: 1,
    contents: "",
    author,
    created: now,
    modified: now,
    hidden: false,
    printed: true,
    locked: false,
    editable: "full",
  };
  switch (spec.kind) {
    case "highlight":
    case "underline":
    case "strikeout":
    case "squiggly":
      return { ...base, subtype: cap(spec.kind), quads: spec.rects, rect: boundsOfRects(spec.rects), color: spec.color, opacity: spec.opacity, contents: spec.contents ?? "" };
    case "note":
      return {
        ...base,
        subtype: "Text",
        color: spec.color,
        contents: spec.contents,
        rect: { l: spec.at[0], t: spec.at[1], r: spec.at[0] + NOTE_SIZE_PT, b: spec.at[1] - NOTE_SIZE_PT },
      };
    case "ink":
    case "signature":
      return { ...base, subtype: "Ink", inkPaths: spec.paths, rect: boundsOfPaths(spec.paths), color: spec.color, borderWidth: spec.width, opacity: spec.opacity };
    case "square":
    case "circle":
      return { ...base, subtype: spec.kind === "square" ? "Square" : "Circle", rect: spec.rect, color: spec.color, fillColor: spec.fillColor, borderWidth: spec.width, opacity: spec.opacity };
    case "line":
    case "arrow":
      return {
        ...base,
        // v0.3: a real /Line (the engine falls back to Ink only in an encrypted document)
        subtype: "Line",
        linePoints: [spec.p1[0], spec.p1[1], spec.p2[0], spec.p2[1]],
        heads: spec.heads ?? (spec.kind === "arrow" ? [false, true] : [false, false]),
        measure: spec.measure,
        rect: boundsOfPaths([[spec.p1[0], spec.p1[1], spec.p2[0], spec.p2[1]]]),
        color: spec.color,
        borderWidth: spec.width,
        opacity: spec.opacity,
      };
    case "textbox":
      return { ...base, subtype: "FreeText", rect: spec.rect, text: spec.text, contents: spec.text, fontSize: spec.fontSize, color: spec.color, fillColor: spec.fillColor, align: spec.align };
    case "callout":
      return {
        ...base, subtype: "FreeText", rect: spec.rect, text: spec.text, contents: spec.text, fontSize: spec.fontSize,
        color: spec.color, fillColor: spec.fillColor, align: spec.align, callout: spec.callout,
      };
    case "polygon":
    case "polyline":
      return {
        ...base, subtype: spec.kind === "polygon" ? "Polygon" : "PolyLine", vertices: spec.vertices, rect: boundsOfPaths([spec.vertices]),
        color: spec.color, fillColor: spec.fillColor, borderWidth: spec.width, opacity: spec.opacity,
        cloudy: spec.cloudy, dashed: spec.dashed, measure: spec.measure,
      };
    case "stamp":
      return {
        ...base,
        subtype: "Stamp",
        rect: spec.rect,
        stampKind: "builtin" in spec.image ? spec.image.builtin : "text" in spec.image ? "SeePDF:TextStamp" : "image",
        ...("text" in spec.image ? { contents: spec.image.text, color: spec.image.color } : {}),
      };
  }
}

function cap(s: string): string {
  return s[0].toUpperCase() + s.slice(1);
}

export interface CreateOptions {
  /** redo re-creates an annotation with the same `/NM` (IPC_CONTRACT §7.1) */
  id?: AnnotId;
  /** select the result once it exists (everything except a markup drag does) */
  select?: boolean;
  /** open the note popover / text editor on the result */
  edit?: boolean;
}

/** Create an annotation optimistically. Resolves with the engine's version, or `null` on failure. */
export async function createAnnotation(page: PageIndex, spec: AnnotSpec, opts: CreateOptions = {}): Promise<Annot | null> {
  const id = docId();
  if (!id) return null;
  const store = useAnnotStore.getState();
  // v0.3 A9: the engine writes 설정 ▸ 작성자 as /T (blank = none); the ghost says the same
  const author = useAppStore.getState().settings?.author?.trim() || null;
  const ghost = ghostFromSpec(page, spec, author);
  store.addGhost(ghost);
  try {
    const result = await api.createAnnotation({ docId: id, page, spec, id: opts.id });
    const annot = result.annot;
    const generation = result.list.docGeneration;
    useAnnotStore.getState().setPage(page, result.list.annots, generation);
    if (annot) {
      // Keep the ghost until the page bitmap of `generation` has painted it, but re-key it to the
      // real /NM so a selection or an edit made in the meantime points at the right annotation.
      useAnnotStore.setState((s) => ({
        ghosts: s.ghosts.map((g) => (g.annot.id === ghost.id ? { annot: { ...annot }, generation } : g)),
      }));
      if (opts.select !== false) useAnnotStore.getState().select([annot.id]);
      if (opts.edit) useAnnotStore.getState().setEditing({ page, id: annot.id });
    } else {
      useAnnotStore.getState().clearGhostById(ghost.id);
    }
    return annot;
  } catch {
    useAnnotStore.getState().clearGhostById(ghost.id);
    return null;
  }
}

// ---------------------------------------------------------------- coalescing

export interface PendingPatch {
  page: PageIndex;
  id: AnnotId;
  patch: AnnotPatch;
}

const pending = new Map<string, PendingPatch>();
let timer: ReturnType<typeof setTimeout> | null = null;
let inFlight: Promise<void> = Promise.resolve();
/**
 * While a drag has its annotations hidden (`dragHide.ts`) nothing is sent until the drop: an
 * `update_annotation` snapshots the document for undo, and a snapshot taken while `/F` has the
 * HIDDEN bit would bring the annotation back hidden on 실행 취소.
 */
let held = false;

export function holdPatchFlush(on: boolean): void {
  held = on;
  if (on && timer) {
    clearTimeout(timer);
    timer = null;
  }
}

function keyOf(page: PageIndex, id: AnnotId): string {
  return `${page}:${id}`;
}

/** Apply a patch to the local copy so the overlay follows the drag without waiting for the engine. */
function applyLocally(page: PageIndex, id: AnnotId, patch: AnnotPatch): void {
  const store = useAnnotStore.getState();
  const found = (store.byPage[page] ?? []).find((a) => a.id === id);
  const ghost = store.ghosts.find((g) => g.annot.id === id);
  const source = found ?? ghost?.annot;
  if (!source) return;
  const next: Annot = { ...source };
  if (patch.rect) next.rect = patch.rect;
  if (patch.rects) next.quads = patch.rects;
  if (patch.paths) next.inkPaths = patch.paths;
  if (patch.p1 && patch.p2) next.linePoints = [patch.p1[0], patch.p1[1], patch.p2[0], patch.p2[1]];
  if (patch.color) next.color = patch.color;
  if (patch.fillColor !== undefined) next.fillColor = patch.fillColor;
  if (patch.opacity !== undefined) next.opacity = patch.opacity;
  if (patch.borderWidth !== undefined) next.borderWidth = patch.borderWidth;
  if (patch.contents !== undefined) next.contents = patch.contents;
  if (patch.author !== undefined) next.author = patch.author;
  if (patch.text !== undefined) next.text = patch.text;
  if (patch.fontSize !== undefined) next.fontSize = patch.fontSize;
  if (patch.locked !== undefined) next.locked = patch.locked;
  // v0.3 pkg4
  if (patch.align !== undefined) next.align = patch.align;
  if (patch.heads !== undefined) {
    next.heads = patch.heads;
    if (next.kind === "line" || next.kind === "arrow") next.kind = patch.heads[0] || patch.heads[1] ? "arrow" : "line";
  }
  if (patch.printed !== undefined) next.printed = patch.printed;
  if (patch.dashed !== undefined) next.dashed = patch.dashed;
  if (patch.vertices) next.vertices = patch.vertices;
  if (patch.callout) next.callout = patch.callout;
  if (found) useAnnotStore.getState().upsert(next);
  if (ghost) {
    useAnnotStore.setState((s) => ({
      ghosts: s.ghosts.map((g) => (g.annot.id === id ? { ...g, annot: next } : g)),
    }));
  }
}

/**
 * Queue a patch. `live` (a slider being dragged, an annotation being moved) coalesces; the final
 * call of a gesture passes `live: false` and flushes immediately.
 */
export function patchAnnotation(page: PageIndex, id: AnnotId, patch: AnnotPatch, live = false): void {
  const key = keyOf(page, id);
  const merged = { page, id, patch: { ...(pending.get(key)?.patch ?? {}), ...patch } };
  pending.set(key, merged);
  applyLocally(page, id, patch);
  if (held) return;
  if (timer) clearTimeout(timer);
  if (live) {
    timer = setTimeout(() => void flushPatches(), PATCH_COALESCE_MS);
  } else {
    timer = null;
    void flushPatches();
  }
}

/** Send everything queued. Awaited by undo/redo and by save, so nothing is lost on a generation bump. */
export function flushPatches(): Promise<void> {
  return sendPatches(takePendingPatches());
}

/**
 * Take everything queued out of the coalescing queue without sending it — a drag's drop owns the
 * patches it made, so a drag that starts before that drop has settled cannot sweep them up (or
 * have its own swept up by it).
 */
export function takePendingPatches(): PendingPatch[] {
  if (timer) {
    clearTimeout(timer);
    timer = null;
  }
  const batch = [...pending.values()];
  pending.clear();
  return batch;
}

/** Send a batch taken with `takePendingPatches`, after whatever is already in flight. */
export function sendPatches(batch: PendingPatch[]): Promise<void> {
  if (batch.length === 0) return inFlight;
  const id = docId();
  if (!id) return inFlight;
  inFlight = inFlight.then(async () => {
    for (const item of batch) {
      try {
        const result = await api.updateAnnotation({ docId: id, page: item.page, id: item.id, patch: item.patch });
        useAnnotStore.getState().setPage(item.page, result.list.annots, result.list.docGeneration);
      } catch {
        // The engine refused (unsupported, stale, locked): re-list so the overlay stops lying.
        void reloadPage(item.page);
      }
    }
  });
  return inFlight;
}

// ⌘S / 다른 이름으로 저장 / autosave drain the queue through the drag gate (entry-chunk safe).
registerEditFlush(async () => {
  const any = pending.size > 0;
  await flushPatches();
  return any;
});

/** `true` while a coalesced patch is still queued — the tests and `undo` both wait on this. */
export function hasPendingPatches(): boolean {
  return pending.size > 0;
}

export function resetPatchQueue(): void {
  if (timer) clearTimeout(timer);
  timer = null;
  held = false;
  pending.clear();
  inFlight = Promise.resolve();
}

// ---------------------------------------------------------------- delete / duplicate / list

/**
 * Delete annotations. The engine removes their replies with them (P2 threads), so an annotation
 * that has replies asks first — 주석과 답글 N개를 삭제할까요? — unless `confirmed`. Resolves
 * `false` when the user kept them.
 */
export async function deleteAnnotations(
  page: PageIndex,
  ids: AnnotId[],
  opts: { confirmed?: boolean } = {},
): Promise<boolean> {
  const id = docId();
  if (!id || ids.length === 0) return false;
  const list = useAnnotStore.getState().byPage[page] ?? [];
  const replies = [...new Set(ids.flatMap((target) => descendantIds(list, target)))].filter((r) => !ids.includes(r));
  if (replies.length && !opts.confirmed) {
    const ok = await askConfirm({
      titleKey: "annot.thread.deleteTitle",
      bodyKey: "annot.thread.deleteBody",
      bodyParams: { count: replies.length },
      confirmKey: "common.delete",
      danger: true,
    });
    if (!ok) return false;
  }
  useAnnotStore.getState().remove(page, [...ids, ...replies]); // optimistic: the shapes go now
  try {
    const result = await api.deleteAnnotations({ docId: id, page, ids });
    useAnnotStore.getState().setPage(page, result.list.annots, result.list.docGeneration);
  } catch {
    void reloadPage(page);
  }
  return true;
}

/**
 * P2 threads: reply to `parentId` with `contents`, as Settings' 작성자. Coalesced patches go
 * first (the engine rewrites the serialised document). Resolves with the reply, or `null` — an
 * encrypted document (`unsupported`) or a failure is toasted.
 */
export async function replyToAnnotation(page: PageIndex, parentId: AnnotId, contents: string): Promise<Annot | null> {
  const id = docId();
  const text = contents.trim();
  if (!id || !text) return null;
  await flushPatches();
  const author = useAppStore.getState().settings?.author?.trim() || null;
  try {
    const result = await api.replyAnnotation({ docId: id, page, parentId, contents: text, author });
    useAnnotStore.getState().setPage(page, result.list.annots, result.list.docGeneration);
    return result.annot;
  } catch (e) {
    const encrypted = api.isSeePdfError(e) && e.code === "unsupported";
    toast(encrypted ? "annot.thread.encrypted" : "annot.thread.failed", undefined, {
      tone: "danger",
      detail: e instanceof Error ? e.message : String(e),
    });
    void reloadPage(page);
    return null;
  }
}

/**
 * 답글 / a click on a thread: open the thread of `id` (its root) in a popover — a note's own
 * popover, the thread popover for any other kind — with the reply box focused when `focusReply`.
 */
export function openThread(page: PageIndex, id: AnnotId, focusReply = true): void {
  const store = useAnnotStore.getState();
  const list = store.byPage[page] ?? [];
  const root = threadRootId(list, id);
  const annot = list.find((a) => a.id === root);
  if (!annot) return;
  store.select([root]);
  if (annot.kind === "note") store.setEditing({ page, id: root });
  store.setThread({ page, id: root, focusReply });
}

/** ⌘D on a selection: the same annotation, offset by 12 pt so it is visibly a copy. */
export async function duplicateAnnotations(page: PageIndex, ids: AnnotId[], offset = 12): Promise<void> {
  const store = useAnnotStore.getState();
  const annots = (store.byPage[page] ?? []).filter((a) => ids.includes(a.id));
  const created: AnnotId[] = [];
  for (const a of annots) {
    const spec = specFromAnnot(a, offset, offset);
    if (!spec) continue;
    const made = await createAnnotation(page, spec, { select: false });
    if (made) created.push(made.id);
  }
  if (created.length) useAnnotStore.getState().select(created);
}

/** The inverse of `ghostFromSpec` — used by 복제 and by the annotation clipboard. */
export function specFromAnnot(a: Annot, dx = 0, dy = 0): AnnotSpec | null {
  const move = (r: Rect): Rect => ({ l: r.l + dx, r: r.r + dx, b: r.b - dy, t: r.t - dy });
  switch (a.kind) {
    case "highlight":
    case "underline":
    case "strikeout":
    case "squiggly":
      return { kind: a.kind, rects: (a.quads ?? [a.rect]).map(move), color: a.color, opacity: a.opacity, contents: a.contents };
    case "note":
      return { kind: "note", at: [a.rect.l + dx, a.rect.t - dy], color: a.color, contents: a.contents };
    case "ink":
    case "signature":
      // a typed / image signature is a stamp: its image path is not known here, like any stamp's
      if (!a.inkPaths?.length) return null;
      return {
        kind: "ink",
        paths: (a.inkPaths ?? []).map((p) => p.map((v, i) => (i % 2 === 0 ? v + dx : v - dy))),
        color: a.color,
        width: a.borderWidth,
        opacity: a.opacity,
      };
    case "square":
    case "circle":
      return { kind: a.kind, rect: move(a.rect), color: a.color, fillColor: a.fillColor, width: a.borderWidth, opacity: a.opacity };
    case "line":
    case "arrow": {
      const p = a.linePoints ?? [a.rect.l, a.rect.b, a.rect.r, a.rect.t];
      return {
        kind: a.kind, p1: [p[0] + dx, p[1] - dy], p2: [p[2] + dx, p[3] - dy], color: a.color, width: a.borderWidth, opacity: a.opacity,
        // v0.3 A1: the copy keeps its heads (and its measuring label)
        ...(a.heads ? { heads: a.heads } : {}),
        ...(a.measure ? { measure: a.measure } : {}),
      };
    }
    case "textbox":
      // v0.3 A1: 복제 / 붙여넣기 keep the alignment
      return { kind: "textbox", rect: move(a.rect), text: a.text ?? a.contents, fontSize: a.fontSize ?? 12, color: a.color, align: a.align ?? "left", fillColor: a.fillColor };
    case "callout":
      return {
        kind: "callout", rect: move(a.rect), text: a.text ?? a.contents, fontSize: a.fontSize ?? 12, color: a.color,
        align: a.align ?? "left", fillColor: a.fillColor,
        callout: (a.callout ?? []).map((v, i) => (i % 2 === 0 ? v + dx : v - dy)),
      };
    case "polygon":
    case "polyline":
      if (!a.vertices?.length) return null;
      return {
        kind: a.kind, vertices: a.vertices.map((v, i) => (i % 2 === 0 ? v + dx : v - dy)), color: a.color, fillColor: a.fillColor,
        width: a.borderWidth, opacity: a.opacity, cloudy: a.cloudy, dashed: a.dashed, measure: a.measure,
      };
    default:
      return null;
  }
}

/**
 * The annotations of a page as the UI sees them: the listed ones, with a ghost of the same id
 * taking precedence, plus the ghosts the engine has not confirmed yet. This is what hit-testing
 * and the sidebar read, so a freshly drawn shape is selectable before its command resolves.
 */
export function annotsOnPage(page: PageIndex): Annot[] {
  const s = useAnnotStore.getState();
  // P2 threads: a reply is never drawn on the page, so it is never hit, moved or copied there
  const base = withoutReplies(s.byPage[page] ?? []);
  const ghosts = s.ghosts.filter((g) => g.annot.page === page);
  if (ghosts.length === 0) return base;
  const known = new Set(base.map((a) => a.id));
  return [
    ...base.map((a) => ghosts.find((g) => g.annot.id === a.id)?.annot ?? a),
    ...ghosts.filter((g) => !known.has(g.annot.id)).map((g) => g.annot),
  ];
}

/** Re-list one page (after a doc-changed, or after a command we could not apply optimistically). */
export async function reloadPage(page: PageIndex): Promise<void> {
  const id = docId();
  if (!id) return;
  try {
    const list = await api.listAnnotations({ docId: id, page });
    useAnnotStore.getState().setPage(page, list.annots, list.docGeneration);
  } catch {
    // `unsupported` while (a) is still landing: leave whatever we have rather than blanking it.
  }
}

// ---------------------------------------------------------------- v0.3 pkg4-annotations-stamps-objects

/**
 * A3 부분 지우개: the pieces left of each touched ink annotation — one `update_annotation {paths}`
 * per annotation (one undo step each, sent at once, not coalesced), or a delete when nothing is
 * left.
 */
export function erasePartial(page: PageIndex, edits: { id: AnnotId; paths: number[][] }[]): void {
  const gone = edits.filter((e) => e.paths.length === 0).map((e) => e.id);
  for (const e of edits) {
    if (e.paths.length) patchAnnotation(page, e.id, { paths: e.paths });
  }
  if (gone.length) void deleteAnnotations(page, gone);
}
