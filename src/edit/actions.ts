/**
 * 편집 mode flows (Stage 7): gesture → IPC → store. No React here, so the flows are tested
 * against the mock adapter directly (`edit.flow.test.tsx`).
 *
 * Every mutation passes `expectGeneration`; a `stale` answer re-lists the page and tells the user.
 * Results of our own mutations are stored straight away (they carry the new generation), so the
 * layer does not re-list a page it already knows.
 */
import * as api from "../ipc/api";
import type {
  DocGeneration, DocId, ObjectId, PageIndex, PageObject, ParagraphEdit, ParagraphEditResult, ParagraphFlow, ParagraphProbe,
  Point, Rect, Rgb,
} from "../ipc/types";
import { toast, type ToastAction } from "../app/toastStore";
import { askChoice, askConfirm } from "../dialogs/dialogState";
import { useAppStore } from "../store/appStore";
import { useDocStore } from "../store/docStore";
import { useViewStore } from "../store/viewStore";
import { useEditStore, type EditSession } from "./editStore";
import { scaleArgs, textRectFor, unionRects } from "./geometry";
import { followFlow } from "./redact";

// ---------------------------------------------------------------------------
// Plumbing
// ---------------------------------------------------------------------------

function docId(): string | null {
  return useEditStore.getState().docId ?? useDocStore.getState().info?.docId ?? null;
}

/** The newest generation we know of for a page: our own last result, or the document's. */
export function generationFor(page: PageIndex): DocGeneration {
  const listed = useEditStore.getState().pages[page]?.docGeneration ?? 0;
  const info = useDocStore.getState().info;
  const doc = info && info.docId === docId() ? info.docGeneration : 0;
  return Math.max(listed, doc);
}

export function objectsOn(page: PageIndex): PageObject[] {
  return useEditStore.getState().pages[page]?.objects ?? [];
}

/** The document generation: object generations are document-wide, so the newest of any page. */
function latestGeneration(): DocGeneration {
  const info = useDocStore.getState().info;
  let g = info && info.docId === docId() ? info.docGeneration : 0;
  for (const entry of Object.values(useEditStore.getState().pages)) g = Math.max(g, entry.docGeneration);
  return g;
}

function selectedObjects(): { page: PageIndex; objects: PageObject[] } | null {
  const sel = useEditStore.getState().selection;
  if (!sel) return null;
  const objects = objectsOn(sel.page).filter((o) => sel.ids.includes(o.objectId));
  return objects.length ? { page: sel.page, objects } : null;
}

const inflight = new Map<string, Promise<void>>();

/** List a page's objects (deduplicated per page + generation). */
export function loadPage(doc: string, page: PageIndex, generation = 0): Promise<void> {
  const key = `${doc}:${page}:${generation}`;
  const running = inflight.get(key);
  if (running) return running;
  const job = api
    .listPageObjects({ docId: doc, page })
    .then((result) => {
      if (useEditStore.getState().docId === doc) useEditStore.getState().setPage(page, result);
    })
    .catch(() => undefined)
    .finally(() => inflight.delete(key));
  inflight.set(key, job);
  return job;
}

function detailOf(e: unknown): string {
  return e instanceof Error || api.isSeePdfError(e) ? e.message : String(e);
}

function fail(e: unknown, page: PageIndex): void {
  const doc = docId();
  if (api.isSeePdfError(e) && e.code === "stale" && doc) void loadPage(doc, page, -1);
  toast(api.errorKey(e), undefined, { tone: "danger", detail: detailOf(e) });
}

/** i18n key for a `PageObject.reason` / probe reason. */
export function reasonKey(reason: string | undefined): string {
  switch (reason) {
    case "insideXObject":
      return "edit.readOnly.xobject";
    case "noUnicode":
      return "edit.readOnly.noUnicode";
    case "type3":
      return "edit.readOnly.type3";
    case "invisible":
      return "edit.reason.invisible";
    case "permissions":
      return "edit.reason.permissions";
    case "glyphsMissing":
      return "edit.reason.glyphsMissing";
    case "rotatedText":
      return "edit.reason.rotatedText";
    case "unwritableContent":
      return "edit.reason.unwritableContent";
    default:
      return "edit.reason.unknown";
  }
}

// ---------------------------------------------------------------------------
// 선택: move / scale / delete
// ---------------------------------------------------------------------------

/**
 * Translate objects by `(dx, dy)` points: one gesture, sent as one `transform_object` per object
 * in order (the engine coalesces them). An object that fails does not stop the others unless the
 * document moved on (`stale` — every later call would fail too); a part-way failure says how many
 * moved instead of a bare error.
 */
export async function moveObjects(page: PageIndex, ids: ObjectId[], dx: number, dy: number): Promise<boolean> {
  const doc = docId();
  if (!doc || (dx === 0 && dy === 0)) return false;
  const movable = ids.filter((id) => objectsOn(page).find((o) => o.objectId === id)?.editable !== "readOnly");
  if (movable.length === 0) return false;
  let done = 0;
  let error: unknown = null;
  for (const objectId of movable) {
    try {
      const result = await api.transformObject({
        docId: doc, page, objectId, expectGeneration: generationFor(page), translate: [dx, dy],
      });
      useEditStore.getState().setPage(page, result, true);
      done += 1;
    } catch (e) {
      error ??= e;
      if (api.isSeePdfError(e) && e.code === "stale") break;
    }
  }
  if (error === null) return true;
  if (done === 0) {
    fail(error, page);
  } else {
    if (api.isSeePdfError(error) && error.code === "stale") void loadPage(doc, page, -1);
    toast("edit.move.partial", { done, total: movable.length }, { tone: "danger", detail: detailOf(error) });
  }
  return false;
}

/**
 * Inspector X / Y / 너비 / 높이 (PDF points, bottom-left origin — the read-out's convention). X / Y
 * move the whole selection so its bounding box starts there; 너비 / 높이 scale one object about its
 * bottom-left corner (the engine's anchor), which is `transform_object` scale with no translate.
 */
export async function setSelectionGeometry(patch: { x?: number; y?: number; w?: number; h?: number }): Promise<boolean> {
  const chosen = selectedObjects();
  const box = chosen && unionRects(chosen.objects.map((o) => o.rect));
  if (!chosen || !box) return false;
  if (patch.w !== undefined || patch.h !== undefined) {
    if (chosen.objects.length !== 1) return false;
    const o = chosen.objects[0];
    const w = patch.w ?? o.rect.r - o.rect.l;
    const h = patch.h ?? o.rect.t - o.rect.b;
    if (!(w > 0) || !(h > 0)) return false;
    return resizeObject(chosen.page, o.objectId, { l: o.rect.l, b: o.rect.b, r: o.rect.l + w, t: o.rect.b + h });
  }
  const dx = patch.x !== undefined ? patch.x - box.l : 0;
  const dy = patch.y !== undefined ? patch.y - box.b : 0;
  const round = (v: number) => Math.round(v * 100) / 100;
  return moveObjects(chosen.page, chosen.objects.map((o) => o.objectId), round(dx), round(dy));
}

/** Scale one object so its box becomes `to` (anchored scale + translate, one call). */
export async function resizeObject(page: PageIndex, id: ObjectId, to: Rect): Promise<boolean> {
  const doc = docId();
  const object = objectsOn(page).find((o) => o.objectId === id);
  if (!doc || !object || object.editable !== "full") return false;
  const { scale, translate } = scaleArgs(object.rect, to);
  if (Math.abs(scale[0] - 1) < 1e-3 && Math.abs(scale[1] - 1) < 1e-3 && translate[0] === 0 && translate[1] === 0) return false;
  try {
    const result = await api.transformObject({
      docId: doc, page, objectId: id, expectGeneration: generationFor(page), scale, translate,
    });
    useEditStore.getState().setPage(page, result, true);
    return true;
  } catch (e) {
    fail(e, page);
    return false;
  }
}

/** ⌫ / ⌦: delete the selection. Read-only objects refuse with their reason. */
export async function deleteSelection(): Promise<boolean> {
  const sel = useEditStore.getState().selection;
  const doc = docId();
  if (!sel || !doc) return false;
  const objects = objectsOn(sel.page).filter((o) => sel.ids.includes(o.objectId));
  const locked = objects.find((o) => o.editable === "readOnly");
  if (locked) {
    toast(reasonKey(locked.reason), undefined, { tone: "danger" });
    return false;
  }
  try {
    const result = await api.deleteObjects({
      docId: doc, page: sel.page, objectIds: objects.map((o) => o.objectId), expectGeneration: generationFor(sel.page),
    });
    useEditStore.getState().clearSelection();
    useEditStore.getState().setPage(sel.page, result);
    return true;
  } catch (e) {
    fail(e, sel.page);
    return false;
  }
}

// ---------------------------------------------------------------------------
// ⌘C / ⌘V / ⌘D — object copies (`duplicate_objects`, Stage 8)
// ---------------------------------------------------------------------------

/** A paste / duplicate lands this far right and down (points); repeated pastes cascade. */
export const PASTE_OFFSET_PT = 10;

interface ClipItem {
  objectId: ObjectId;
  type: PageObject["type"];
  rect: Rect;
  text?: string;
}

/**
 * The object clipboard is in-process, like the annotation one: a reference to objects of this
 * document (page + ids at a generation). Nothing goes to the system pasteboard. Ids only hold
 * within one generation, so a paste after the document moved finds the objects again by their
 * box and text (`resolveClip`).
 */
interface ObjectClip {
  docId: DocId;
  page: PageIndex;
  generation: DocGeneration;
  items: ClipItem[];
  /** pastes so far per target page (each lands one more offset step away) */
  pastes: Map<PageIndex, number>;
}

let clip: ObjectClip | null = null;

export function hasObjectClipboard(): boolean {
  return clip !== null && clip.docId === docId();
}

export function clearObjectClipboard(): void {
  clip = null;
}

/** The selection minus read-only objects; `null` (with the reason toasted) when none are left. */
function copyableSelection(): { page: PageIndex; objects: PageObject[] } | null {
  const chosen = selectedObjects();
  if (!chosen) return null;
  const usable = chosen.objects.filter((o) => o.editable !== "readOnly");
  if (usable.length === 0) {
    toast(reasonKey(chosen.objects[0].reason), undefined, { tone: "danger" });
    return null;
  }
  return { page: chosen.page, objects: usable };
}

/** ⌘C: remember the selected objects. `true` when there was a selection to claim the key for. */
export function copySelection(): boolean {
  const doc = docId();
  const store = useEditStore.getState();
  if (!doc || !store.selection || store.session) return false;
  const chosen = copyableSelection();
  if (!chosen) return true;
  clip = {
    docId: doc,
    page: chosen.page,
    generation: store.pages[chosen.page]?.docGeneration ?? generationFor(chosen.page),
    items: chosen.objects.map((o) => ({ objectId: o.objectId, type: o.type, rect: o.rect, text: o.text })),
    pastes: new Map(),
  };
  return true;
}

function sameRect(a: Rect, b: Rect, tol = 0.05): boolean {
  return Math.abs(a.l - b.l) <= tol && Math.abs(a.b - b.b) <= tol && Math.abs(a.r - b.r) <= tol && Math.abs(a.t - b.t) <= tol;
}

/**
 * The clipboard's ids at the current generation: unchanged ids when nothing moved, else the
 * object with the same type, box and text (or, for our own moves, the same id with the same type
 * and text). `null` when any of them is gone.
 */
async function resolveClip(c: ObjectClip): Promise<ObjectId[] | null> {
  let entry = useEditStore.getState().pages[c.page];
  if (!entry || entry.docGeneration < latestGeneration()) {
    try {
      entry = await api.listPageObjects({ docId: c.docId, page: c.page });
    } catch {
      return null;
    }
    useEditStore.getState().setPage(c.page, entry);
  }
  if (entry.docGeneration === c.generation) return c.items.map((i) => i.objectId);
  const taken = new Set<ObjectId>();
  const ids: ObjectId[] = [];
  for (const item of c.items) {
    const same = (o: PageObject) => !taken.has(o.objectId) && o.type === item.type && (o.text ?? "") === (item.text ?? "");
    const found =
      entry.objects.find((o) => o.objectId === item.objectId && same(o) && sameRect(o.rect, item.rect)) ??
      entry.objects.find((o) => same(o) && sameRect(o.rect, item.rect)) ??
      entry.objects.find((o) => o.objectId === item.objectId && same(o));
    if (!found) return null;
    taken.add(found.objectId);
    ids.push(found.objectId);
  }
  c.generation = entry.docGeneration;
  c.items = c.items.map((item, k) => ({ ...item, objectId: ids[k] }));
  return ids;
}

/** `duplicate_objects`, then select the copies where they landed. */
async function duplicate(page: PageIndex, ids: ObjectId[], offset: [number, number], target = page): Promise<boolean> {
  const doc = docId();
  if (!doc || ids.length === 0) return false;
  try {
    const result = await api.duplicateObjects({
      docId: doc,
      page,
      objectIds: ids,
      expectGeneration: latestGeneration(),
      offset,
      ...(target !== page ? { targetPage: target } : null),
    });
    const store = useEditStore.getState();
    store.setPage(target, { objects: result.objects, docGeneration: result.docGeneration });
    store.select(target, result.newObjectIds);
    return true;
  } catch (e) {
    if (api.isSeePdfError(e) && e.code === "unsupported") {
      toast(target !== page ? "edit.duplicate.crossPage" : "edit.duplicate.unsupported", undefined, {
        tone: "danger",
        detail: e.message,
      });
      return false;
    }
    fail(e, page);
    return false;
  }
}

/** ⌘D: copy the selection in place, one offset step away, and select the copies. */
export async function duplicateSelection(): Promise<boolean> {
  if (useEditStore.getState().session) return false;
  const chosen = copyableSelection();
  if (!chosen) return false;
  return duplicate(chosen.page, chosen.objects.map((o) => o.objectId), [PASTE_OFFSET_PT, -PASTE_OFFSET_PT]);
}

/**
 * ⌘V: the copied objects onto `target` (the page in view), one more offset step per paste onto
 * that page. Another page goes through `targetPage`; an object the engine cannot carry there
 * answers `unsupported`, which is a toast, not an error.
 */
export async function pasteObjects(target: PageIndex = useViewStore.getState().currentPage): Promise<boolean> {
  const c = clip;
  if (!c || c.docId !== docId() || useEditStore.getState().session) return false;
  const ids = await resolveClip(c);
  if (!ids) {
    clip = null;
    toast("edit.clipboard.gone", undefined, { tone: "danger" });
    return false;
  }
  const n = (c.pastes.get(target) ?? 0) + 1;
  const ok = await duplicate(c.page, ids, [PASTE_OFFSET_PT * n, -PASTE_OFFSET_PT * n], target);
  if (ok) c.pastes.set(target, n);
  return ok;
}

/** Inspector: 크기 / 색상 of one text object (`edit_text_object`). */
export async function restyleText(page: PageIndex, id: ObjectId, patch: { fontSizePt?: number; color?: Rgb }): Promise<boolean> {
  const doc = docId();
  if (!doc) return false;
  try {
    const result = await api.editTextObject({
      docId: doc, page, objectId: id, expectGeneration: generationFor(page), patch, allowFontSubstitution: false,
    });
    useEditStore.getState().setPage(page, result, true);
    return true;
  } catch (e) {
    fail(e, page);
    return false;
  }
}

// ---------------------------------------------------------------------------
// 텍스트 수정: paragraph sessions
// ---------------------------------------------------------------------------

/** Reads the live textarea (uncontrolled — Hangul IME safe); set by the open editor. */
let readText: (() => string) | null = null;
/** Resolves once no IME composition is in progress (set by the open editor with the reader). */
let settleText: (() => Promise<void> | null) | null = null;

/**
 * The open editor registers how to read its text and, optionally, how to wait for an IME
 * composition to end: `settle` returns `null` when nothing is being composed, else a promise.
 */
export function setSessionTextReader(fn: (() => string) | null, settle?: () => Promise<void> | null): void {
  readText = fn;
  settleText = fn ? (settle ?? null) : null;
}

export function sessionText(): string {
  return readText ? readText() : "";
}

/**
 * The text to commit. A click outside arrives on `pointerdown`, before the browser ends a Hangul
 * composition, so while one is in progress wait for it (the editor bounds the wait) and read again;
 * if the editor is gone by then, keep what it showed when the commit started.
 */
async function settledSessionText(): Promise<string> {
  const early = sessionText();
  const pending = settleText?.();
  if (!pending) return early;
  await pending;
  return readText ? readText() : early;
}

/** Click with 텍스트 수정: probe the paragraph under `at` and open the editor on it. */
export async function beginParagraphEdit(page: PageIndex, at: Point): Promise<boolean> {
  const doc = docId();
  if (!doc) return false;
  let probe;
  try {
    probe = await api.probeParagraph({ docId: doc, page, at });
  } catch (e) {
    fail(e, page);
    return false;
  }
  if (!probe) return false;
  if (probe.strategy === "refused") {
    toast(reasonKey(probe.reason), undefined, { tone: "danger" });
    return false;
  }
  useEditStore.getState().openSession({
    kind: "paragraph",
    page,
    probe,
    width: probe.rect.r - probe.rect.l,
    fontSizePt: probe.fontSizePt,
    color: probe.color,
    align: probe.align,
    consented: false,
    firstLine: firstLineOf(page, probe),
  });
  return true;
}

/**
 * The paragraph's first line in the page listing the probe ran on: points inside its objects (where
 * the paragraph is looked up again if the document changes under the editor) and their text (to find
 * it when it moved). `undefined` when the listing is of another generation.
 */
function firstLineOf(page: PageIndex, p: ParagraphProbe): { at: Point[]; texts: string[] } | undefined {
  const entry = useEditStore.getState().pages[page];
  if (!entry || entry.docGeneration !== p.docGeneration) return undefined;
  const members = entry.objects.filter((o) => o.type === "text" && p.objectIds.includes(o.objectId));
  if (members.length === 0) return undefined;
  const baseline = Math.max(...members.map((o) => o.matrix[5]));
  const line = members
    .filter((o) => Math.abs(o.matrix[5] - baseline) <= 0.25 * p.fontSizePt)
    .sort((a, b) => a.rect.l - b.rect.l);
  return {
    at: line.slice(0, 3).map((o): Point => [(o.rect.l + o.rect.r) / 2, (o.rect.b + o.rect.t) / 2]),
    texts: line.map((o) => o.text ?? "").filter((t) => t.trim() !== ""),
  };
}

/** Click with 텍스트 추가: an empty editor in the tool's default style. */
export function beginAddText(page: PageIndex, at: Point): void {
  const defaults = (useAppStore.getState().settings?.toolDefaults?.addText ?? {}) as { fontSize?: number; color?: Rgb };
  const fontSizePt = typeof defaults.fontSize === "number" && defaults.fontSize > 0 ? defaults.fontSize : 12;
  const color: Rgb = Array.isArray(defaults.color) && defaults.color.length === 3 ? defaults.color : [0, 0, 0];
  useEditStore.getState().openSession({ kind: "addText", page, at, width: 0, fontSizePt, color, align: "left" });
}

export function cancelSession(): void {
  useEditStore.getState().closeSession();
}

function confirmFont(): Promise<boolean> {
  return askConfirm({
    titleKey: "edit.font.confirmTitle",
    bodyKey: "edit.font.confirmHangul",
    confirmKey: "edit.font.replace",
  });
}

/** The engine refused because the paragraph's own font cannot draw the new text. */
function isFontRefusal(e: unknown): boolean {
  if (!api.isSeePdfError(e)) return false;
  if (e.code === "fontCoverage") return true;
  return /replaceFont|font ?substitut/i.test(`${e.message} ${e.detail ?? ""}`);
}

let inflightCommit: Promise<boolean> | null = null;

/**
 * 완료 / ⌘↵ / click outside. Resolves `true` when the session is over (written or unchanged) and
 * `false` when it stays open (the user declined the font change, chose 계속 편집 when the paragraph
 * has no room to grow, the document changed under the editor, or a paragraph write failed — what was
 * typed is never dropped after the probe said the paragraph could be edited).
 *
 * One commit at a time: a call while one runs gets that commit's answer. A click on a mode tab
 * starts the commit on `pointerdown` (the editor's click-outside) and asks the leave guard on
 * `click`; the guard must wait for the real answer, not see a refusal because a commit is running.
 */
export function commitSession(text?: string): Promise<boolean> {
  if (inflightCommit) return inflightCommit;
  const session = useEditStore.getState().session;
  const doc = docId();
  if (!session || !doc) return Promise.resolve(true);
  const job = (async () => {
    const typed = text ?? (await settledSessionText());
    // the latest state of the same session (a width / size change while the IME settled counts)
    const current = useEditStore.getState().session;
    if (!current || current.kind !== session.kind || current.page !== session.page) return true; // cancelled meanwhile
    return current.kind === "addText" ? commitAddText(doc, current, typed) : commitParagraph(doc, current, typed);
  })();
  const running = job.finally(() => {
    if (inflightCommit === running) inflightCommit = null;
  });
  inflightCommit = running;
  return running;
}

/** Put the caret back in the open editor (after a prompt answered 계속 편집). */
let focusEditor: (() => void) | null = null;

export function setSessionFocuser(fn: (() => void) | null): void {
  focusEditor = fn;
}

async function commitAddText(doc: string, s: Extract<EditSession, { kind: "addText" }>, raw: string): Promise<boolean> {
  const text = raw.replace(/\s+$/, "");
  const store = useEditStore.getState();
  if (!text.trim()) {
    store.closeSession();
    return true;
  }
  try {
    const result = await api.addTextObject({
      docId: doc,
      page: s.page,
      rect: textRectFor(s.at, text, s.fontSizePt, s.width || undefined),
      text,
      fontSizePt: s.fontSizePt,
      color: s.color,
      align: s.align === "justify" ? "left" : s.align,
    });
    store.setPage(s.page, result);
  } catch (e) {
    fail(e, s.page);
  }
  useEditStore.getState().closeSession();
  return true;
}

async function commitParagraph(doc: string, s: Extract<EditSession, { kind: "paragraph" }>, text: string): Promise<boolean> {
  const p = s.probe;
  const store = useEditStore.getState();
  const width0 = p.rect.r - p.rect.l;
  const edit: ParagraphEdit = { objectIds: p.objectIds, text };
  if (Math.abs(s.width - width0) > 0.5) edit.width = s.width;
  if (s.fontSizePt !== p.fontSizePt) edit.fontSizePt = s.fontSizePt;
  if (s.color.join() !== p.color.join()) edit.color = s.color;
  if (s.align !== p.align) edit.align = s.align;
  const unchanged = text === p.text && Object.keys(edit).length === 2;
  if (unchanged) {
    store.closeSession();
    return true;
  }

  // The probe's object ids belong to the generation it ran at: every call below is pinned to it,
  // so a change while the editor or one of its prompts is open (undo / redo under the prompt)
  // makes the engine refuse the write as `stale` instead of replacing whatever those ids name now.
  const generation = p.docGeneration ?? generationFor(s.page);

  // Everything deleted: `edit_paragraph` with no text removes the paragraph and pulls what followed it
  // up into its place (a push dry run is never blocked by that; no font is involved).
  const emptied = !text.trim();

  let allow = s.consented;
  if (p.strategy === "replaceFont" && !allow && !emptied) {
    if (!(await confirmFont())) return false;
    allow = true;
    useEditStore.getState().patchSession({ consented: true });
  }

  // the same session (a patch keeps the probe): Esc while a call is in flight writes nothing
  const open = () => {
    const current = useEditStore.getState().session;
    return current?.kind === "paragraph" && current.probe === p;
  };
  const run = (flow: ParagraphFlow, dryRun: boolean): Promise<ParagraphEditResult> =>
    api.editParagraph({
      docId: doc,
      page: s.page,
      expectGeneration: generation,
      edit: { ...edit, flow, ...(dryRun ? { dryRun: true } : null) },
      allowFontSubstitution: allow,
    });

  // Stage 9: plan the flow first (nothing written), so a paragraph that grew pushes the content
  // below it down — or, when that is blocked, the user picks what to do before anything changes.
  let plan: ParagraphEditResult;
  // false: the engine cannot move what follows on this page; the text can still be written over it
  let pushable = true;
  try {
    try {
      plan = await run("push", true);
    } catch (e) {
      if (allow || !isFontRefusal(e)) throw e;
      if (!(await confirmFont())) return false;
      allow = true;
      useEditStore.getState().patchSession({ consented: true });
      plan = await run("push", true);
    }
  } catch (e) {
    if (isStale(e)) return refind(doc, s.page, p);
    if (!isUnwritable(e)) return keepOpen(s.page, e);
    // Only the push is unwritable (what follows sits in a stream that cannot be rewritten): nothing
    // moves, and `overlap` / `fit` may still write the paragraph — offer them instead of dropping
    // what was typed.
    try {
      plan = await run("overlap", true);
      pushable = false;
    } catch (e2) {
      if (isStale(e2)) return refind(doc, s.page, p);
      return keepOpen(s.page, e2);
    }
  }
  if (!open()) return true;

  let flow: ParagraphFlow = pushable ? "push" : "overlap";
  const blocked = pushable ? plan.blocked : plan.overflowPt > 0.05 ? "obstacle" : undefined;
  if (blocked) {
    const choice = await askBlocked({ ...plan, blocked }, run, pushable);
    if (!open()) return true;
    if (choice === "keepEditing") {
      setTimeout(() => focusEditor?.(), 0);
      return false;
    }
    if (choice === "fit") flow = "fit";
  }

  let result: ParagraphEditResult;
  try {
    result = await run(flow, false);
  } catch (e) {
    if (isStale(e)) return refind(doc, s.page, p);
    return keepOpen(s.page, e);
  }
  useEditStore.getState().setPage(s.page, result.objects);
  useEditStore.getState().closeSession();
  // pending 영역 표시 marks over the content that moved go with it
  if (result.movedBand) followFlow(s.page, result.movedBand, result.shiftedPt);
  flowToast(result, flow);
  return true;
}

function isStale(e: unknown): boolean {
  return api.isSeePdfError(e) && e.code === "stale";
}

function isUnwritable(e: unknown): boolean {
  return api.isSeePdfError(e) && (e.detail?.startsWith("unwritableContent") ?? false);
}

/**
 * Points inside the paragraph's first line from its box alone (when the listing the probe ran on is
 * not at hand): the middle, past the indent, and before the right edge — a short first line of
 * right-aligned or centred text holds one of them.
 */
function anchorsOf(p: ParagraphProbe): Point[] {
  const y = p.rect.t - 0.5 * p.fontSizePt;
  return [
    [(p.rect.l + p.rect.r) / 2, y],
    [p.rect.l + Math.max(0, p.firstLineIndentPt) + p.fontSizePt, y],
    [p.rect.r - p.fontSizePt, y],
  ];
}

/**
 * The document changed while the editor (or one of its prompts) was open, so the engine refused the
 * write as `stale`: the probe's object ids may name other objects by now. Nothing was written. The
 * paragraph is looked up again — inside its first line where it was, then, if it moved (an undo or a
 * redo shifted it), wherever a line with its first line's text is now. When it is found with the same
 * text, the editor stays open on it with everything typed kept (the same editor: its key does not
 * depend on where the paragraph is), and the next 완료 writes it. When it is gone, the editor stays
 * open too, so what was typed can still be copied; Esc closes it.
 */
async function refind(doc: string, page: PageIndex, p: ParagraphProbe): Promise<boolean> {
  void loadPage(doc, page, -1);
  const session = useEditStore.getState().session;
  const firstLine = session?.kind === "paragraph" && session.probe === p ? session.firstLine : undefined;
  const same = (found: ParagraphProbe | null): found is ParagraphProbe =>
    found !== null && found.strategy !== "refused" && found.text === p.text && found.lines === p.lines;
  const probeAt = (at: Point) => api.probeParagraph({ docId: doc, page, at }).catch(() => null);
  let fresh: ParagraphProbe | null = null;
  for (const at of [...(firstLine?.at ?? []), ...anchorsOf(p)]) {
    const found = await probeAt(at);
    if (same(found)) {
      fresh = found;
      break;
    }
  }
  if (!fresh && firstLine?.texts.length) {
    const listed = await api.listPageObjects({ docId: doc, page }).catch(() => null);
    const lines = (listed?.objects ?? []).filter((o) => o.type === "text" && firstLine.texts.includes(o.text ?? ""));
    for (const o of lines.slice(0, 6)) {
      const found = await probeAt([(o.rect.l + o.rect.r) / 2, (o.rect.b + o.rect.t) / 2]);
      if (same(found)) {
        fresh = found;
        break;
      }
    }
  }
  const current = useEditStore.getState().session;
  if (current?.kind !== "paragraph" || current.probe !== p) return true; // closed meanwhile
  setTimeout(() => focusEditor?.(), 0);
  if (fresh) {
    useEditStore.getState().patchSession({
      probe: fresh,
      firstLine: firstLineOf(page, fresh) ?? (firstLine && { at: [], texts: firstLine.texts }),
    });
    toast("edit.flow.docChanged");
    return false;
  }
  toast("edit.flow.paragraphGone", undefined, { tone: "danger" });
  return false;
}

/**
 * A paragraph call failed after the probe said the paragraph could be edited: say why and keep the
 * editor open with what was typed (it can be changed, copied, or dropped with Esc).
 */
function keepOpen(page: PageIndex, e: unknown): false {
  if (isUnwritable(e)) {
    toast(reasonKey("unwritableContent"), undefined, { tone: "danger" });
  } else {
    fail(e, page);
  }
  setTimeout(() => focusEditor?.(), 0);
  return false;
}

type BlockedChoice = "fit" | "overlap" | "keepEditing";

/** Points for a toast / prompt: one decimal, never "-0". */
const pts = (v: number) => Math.round(Math.abs(v) * 10) / 10;

/** Nothing below the paragraph: the push was blocked by the page bottom, and nothing is overlapped. */
function runsPastBottom(r: ParagraphEditResult): boolean {
  return r.overflowPt <= 0.05 && (r.pastBottomPt ?? 0) > 0.05;
}

/**
 * 문단이 들어갈 자리가 부족합니다: 글자 크기 줄여 맞추기 (N%, from a `fit` dry run — disabled when
 * even the smallest size still does not fit) · 아래로 밀고 남은 부분은 겹치기 · 계속 편집. With nothing
 * below the paragraph, the text would run past the page's bottom margin instead: the body and the
 * second option say so. When the content below cannot be moved at all on this page (`pushable`
 * false), the body says that and the second option writes the text over it.
 */
async function askBlocked(
  plan: ParagraphEditResult,
  run: (flow: ParagraphFlow, dryRun: boolean) => Promise<ParagraphEditResult>,
  pushable = true,
): Promise<BlockedChoice> {
  const fit = await run("fit", true).catch(() => null);
  const scale = fit?.fitScale;
  const fits = fit !== null && scale !== undefined && fit.overflowPt <= 0.05 && (fit.pastBottomPt ?? 0) <= 0.05;
  const pct = Math.round((scale ?? MIN_FIT_SCALE) * 100);
  const past = runsPastBottom(plan);
  const bodyKey = !pushable
    ? "edit.flow.blocked.unmovable"
    : past
      ? "edit.flow.blocked.pastBottom"
      : plan.blocked === "obstacle"
        ? "edit.flow.blocked.obstacle"
        : "edit.flow.blocked.pageBottom";
  const overlapKey = !pushable ? "edit.flow.overlapInPlace" : past ? "edit.flow.keepPastBottom" : "edit.flow.overlap";
  return askChoice<BlockedChoice>({
    titleKey: "edit.flow.blockedTitle",
    bodyKey,
    bodyParams: { pt: pts(past ? (plan.pastBottomPt ?? 0) : plan.overflowPt) },
    ...(fits ? null : { hintKey: "edit.flow.fitTooSmall", hintParams: { pct: Math.round(MIN_FIT_SCALE * 100) } }),
    options: [
      { value: "fit", labelKey: "edit.flow.fit", labelParams: { pct }, primary: true, disabled: !fits },
      { value: "overlap", labelKey: overlapKey },
    ],
    cancel: { value: "keepEditing", labelKey: "edit.flow.keepEditing" },
  });
}

/** The smallest `fit` factor the engine tries (Stage 9 contract). */
const MIN_FIT_SCALE = 0.7;

/** What the flow did, with 실행 취소 (the edit and its moves are one undo step). */
function flowToast(result: ParagraphEditResult, flow: ParagraphFlow): void {
  const undo: ToastAction = {
    labelKey: "common.undo",
    onSelect: () => void import("../annot/sync").then((m) => m.undoWithAnnots()),
  };
  const shifted = pts(result.shiftedPt);
  const over = pts(result.overflowPt);
  const past = pts(result.pastBottomPt ?? 0);
  if (over === 0 && past > 0) {
    toast("edit.flow.pastBottom", { pt: past }, { actions: [undo] });
  } else if (over > 0) {
    if (shifted > 0) toast("edit.flow.pushedOverlap", { pt: shifted, over }, { actions: [undo] });
    else toast("edit.flow.overlaps", { pt: over }, { actions: [undo] });
  } else if (flow === "fit" && result.fitScale !== undefined && result.fitScale < 1) {
    toast("edit.flow.fitted", { pct: Math.round(result.fitScale * 100) }, { actions: [undo] });
  } else if (shifted > 0) {
    toast(result.shiftedPt > 0 ? "edit.flow.pushed" : "edit.flow.pulled", { pt: shifted }, { actions: [undo] });
  }
}

// ---------------------------------------------------------------------------
// 이미지 추가
// ---------------------------------------------------------------------------

/** Pick a PNG / JPEG and place it inside `rect` (aspect kept, centred — the engine's rule). */
export async function addImageFlow(page: PageIndex, rect: Rect): Promise<boolean> {
  const doc = docId();
  if (!doc) return false;
  const picked = await api
    .openFileDialog({ multiple: false, filters: [{ name: "PNG / JPEG", extensions: ["png", "jpg", "jpeg"] }] })
    .catch(() => null);
  const path = picked?.[0];
  if (!path) return false;
  try {
    const result = await api.addImageObject({ docId: doc, page, rect, path, keepAspect: true });
    useEditStore.getState().setPage(page, result);
    return true;
  } catch (e) {
    fail(e, page);
    return false;
  }
}

/**
 * v0.3 pkg2-pages-structure-forms (D1): ⌘V with an **image on the system clipboard** (and nothing
 * on the in-app object clipboard) places it on the page in view, centred, at most half the page on
 * each side (aspect kept by the engine) — one `add_image_object`, one undo step. `data` is the
 * DOM `paste` event's clipboard (macOS menu route); without it the async clipboard is read (the
 * Windows keydown route). `false` when there is no image.
 */
export async function pasteSystemImage(data?: DataTransfer | null): Promise<boolean> {
  const doc = docId();
  if (!doc || useEditStore.getState().session) return false;
  const { readClipboardImage } = await import("../dialogs/imagesFlow");
  const bytes = await readClipboardImage(data);
  if (!bytes) return false;
  const page = useViewStore.getState().currentPage;
  const geom = useDocStore.getState().info?.pages[page];
  if (!geom) return false;
  const crop = geom.crop;
  const w = (crop.r - crop.l) / 2;
  const h = (crop.t - crop.b) / 2;
  const cx = (crop.l + crop.r) / 2;
  const cy = (crop.b + crop.t) / 2;
  const rect: Rect = { l: cx - w / 2, b: cy - h / 2, r: cx + w / 2, t: cy + h / 2 };
  try {
    const path = await api.writeTempImage(bytes);
    const result = await api.addImageObject({ docId: doc, page, rect, path, keepAspect: true });
    useEditStore.getState().setPage(page, result);
    return true;
  } catch (e) {
    fail(e, page);
    return false;
  }
}
