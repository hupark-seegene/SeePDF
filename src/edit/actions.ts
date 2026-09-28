/**
 * 편집 mode flows (Stage 7): gesture → IPC → store. No React here, so the flows are tested
 * against the mock adapter directly (`edit.flow.test.tsx`).
 *
 * Every mutation passes `expectGeneration`; a `stale` answer re-lists the page and tells the user.
 * Results of our own mutations are stored straight away (they carry the new generation), so the
 * layer does not re-list a page it already knows.
 */
import * as api from "../ipc/api";
import type { DocGeneration, DocId, ObjectId, PageIndex, PageObject, ParagraphEdit, ParagraphEditResult, Point, Rect, Rgb } from "../ipc/types";
import { toast } from "../app/toastStore";
import { askConfirm } from "../dialogs/dialogState";
import { useAppStore } from "../store/appStore";
import { useDocStore } from "../store/docStore";
import { useViewStore } from "../store/viewStore";
import { useEditStore, type EditSession } from "./editStore";
import { scaleArgs, textRectFor, unionRects } from "./geometry";

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

export function setSessionTextReader(fn: (() => string) | null): void {
  readText = fn;
}

export function sessionText(): string {
  return readText ? readText() : "";
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
  });
  return true;
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

let committing = false;

/**
 * 완료 / ⌘↵ / click outside. Resolves `true` when the session is over (written, unchanged, or
 * failed with a toast) and `false` when it stays open (the user declined the font change).
 */
export async function commitSession(text = sessionText()): Promise<boolean> {
  const session = useEditStore.getState().session;
  const doc = docId();
  if (!session || !doc) return true;
  if (committing) return false;
  committing = true;
  try {
    return session.kind === "addText" ? await commitAddText(doc, session, text) : await commitParagraph(doc, session, text);
  } finally {
    committing = false;
  }
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

  // Everything deleted: remove the paragraph's objects instead of writing an empty one.
  if (!text.trim()) {
    try {
      const result = await api.deleteObjects({
        docId: doc, page: s.page, objectIds: p.objectIds, expectGeneration: generationFor(s.page),
      });
      store.setPage(s.page, result);
    } catch (e) {
      fail(e, s.page);
    }
    useEditStore.getState().closeSession();
    return true;
  }

  let allow = s.consented;
  if (p.strategy === "replaceFont" && !allow) {
    if (!(await confirmFont())) return false;
    allow = true;
    useEditStore.getState().patchSession({ consented: true });
  }

  const run = (allowFontSubstitution: boolean): Promise<ParagraphEditResult> =>
    api.editParagraph({ docId: doc, page: s.page, expectGeneration: generationFor(s.page), edit, allowFontSubstitution });

  let result: ParagraphEditResult;
  try {
    try {
      result = await run(allow);
    } catch (e) {
      if (allow || !isFontRefusal(e)) throw e;
      if (!(await confirmFont())) return false;
      useEditStore.getState().patchSession({ consented: true });
      result = await run(true);
    }
  } catch (e) {
    fail(e, s.page);
    useEditStore.getState().closeSession();
    return true;
  }
  useEditStore.getState().setPage(s.page, result.objects);
  useEditStore.getState().closeSession();
  if (result.overflowPt > 0) toast("edit.paragraph.overflow");
  return true;
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
