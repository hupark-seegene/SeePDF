/**
 * R4 그룹 해제 (v0.3.1). Text inside a group (Form XObject) cannot be edited in place — PDFium would
 * drop the change on save — so reaching such a run asks "그룹 해제 후 편집할까요?"; yes →
 * `ungroup_object` down to the group that holds the clicked text (one undo step 그룹 해제), and the
 * caller probes the paragraph again, now made of ordinary page objects. Every way in ends here:
 * a click with 텍스트 수정, a double-click with 선택 (`actions.beginParagraphEdit`), and the Inspector's
 * 그룹 해제 button (`ungroupSelection`, no question: the button is the answer).
 *
 * The engine verifies every 그룹 해제 by rendering the page before and after; a group whose look
 * depends on staying a group comes back `lookChanged` (nothing changed) or with only its text taken
 * out (`partial`). Each failure has its own toast — never the engine's English sentence alone.
 *
 * `stale` (the document moved on between the probe and the 그룹 해제 — a second click, an undo) is
 * recovered, not reported: the text is probed again at the same point, and if it is still inside a
 * group that group is ungrouped without asking again (the user already said yes); if someone else
 * already ungrouped it, the editor simply opens. The Inspector's button has no point to probe: the
 * page is listed again and the same group (same id and box) is ungrouped at the new generation; only
 * a second `stale` asks the user to press again (`error.stale`, the group still selected) — and
 * nothing is said when the group is gone (already ungrouped). The button runs one 그룹 해제 at a time: a
 * second press while one is running gets the running answer.
 */
import * as api from "../ipc/api";
import type { DocGeneration, ObjectId, PageIndex, ParagraphProbe, Point, Rect, UngroupResult } from "../ipc/types";
import { toast } from "../app/toastStore";
import { askConfirm } from "../dialogs/dialogState";
import { useDocStore } from "../store/docStore";
import { useEditStore } from "./editStore";

/** How a 그룹 해제 for an edit ended. */
export type UngroupOutcome =
  | { kind: "ungrouped"; result: UngroupResult }
  /** the text is editable already (another 그룹 해제 got there first) — probe again */
  | { kind: "editable" }
  /** declined, or failed (the toast has said why) */
  | { kind: "none" };

const NONE: UngroupOutcome = { kind: "none" };

function newest(page: PageIndex): DocGeneration {
  const listed = useEditStore.getState().pages[page]?.docGeneration ?? 0;
  const info = useDocStore.getState().info;
  return Math.max(listed, info?.docGeneration ?? 0);
}

/** The i18n key of a failed 그룹 해제. */
export function ungroupErrorKey(e: unknown): string {
  if (!api.isSeePdfError(e)) return "edit.ungroup.failed.generic";
  switch (e.detail ?? "") {
    case "lookChanged":
    case "groupTransparency":
      return "edit.ungroup.failed.lookChanged";
    case "notAGroup":
      return "edit.ungroup.failed.notAGroup";
    default:
      break;
  }
  switch (e.code) {
    case "stale":
      return "edit.ungroup.failed.stale";
    case "notFound":
      return "edit.ungroup.failed.notFound";
    case "permissionDenied":
      return "error.permissionDenied";
    case "cancelled":
      return "";
    default:
      return "edit.ungroup.failed.generic";
  }
}

function fail(e: unknown): void {
  const key = ungroupErrorKey(e);
  if (!key) return; // 취소 on the signed-document prompt: nothing to say
  // only the unexplained failure carries the engine's own words (for a bug report)
  const detail = key === "edit.ungroup.failed.generic" && (e instanceof Error || api.isSeePdfError(e)) ? e.message : undefined;
  toast(key, undefined, { tone: "danger", ...(detail ? { detail } : null) });
}

function stored(page: PageIndex, result: UngroupResult): void {
  useEditStore.getState().setPage(page, { docGeneration: result.docGeneration, objects: result.objects });
  if (result.partial) toast("edit.ungroup.partial");
}

/** The page's objects again (after `stale`); `loadPage` without importing the actions module. */
async function relist(docId: string, page: PageIndex): Promise<void> {
  try {
    const result = await api.listPageObjects({ docId, page });
    if (useEditStore.getState().docId === docId) useEditStore.getState().setPage(page, result);
  } catch {
    // the next render lists it
  }
}

/**
 * `ungroup_object` with one recovery: on `stale` the text at `at` is probed again (see the module
 * docs). Without `at` (the Inspector) a `stale` re-lists the page and says so.
 */
async function ungroup(
  docId: string, page: PageIndex, objectId: ObjectId, expectGeneration: DocGeneration, at?: Point,
): Promise<UngroupOutcome> {
  try {
    const result = await api.ungroupObject({ docId, page, objectId, expectGeneration, ...(at ? { at } : null) });
    stored(page, result);
    return { kind: "ungrouped", result };
  } catch (e) {
    if (!(api.isSeePdfError(e) && e.code === "stale")) {
      fail(e);
      return NONE;
    }
    if (!at) return ungroupAgain(docId, page, objectId);
  }
  // stale: where is the text now?
  let probe: ParagraphProbe | null;
  try {
    probe = await api.probeParagraph({ docId, page, at });
  } catch (e) {
    fail(e);
    return NONE;
  }
  if (!probe) {
    toast("edit.ungroup.failed.gone", undefined, { tone: "danger" });
    return NONE;
  }
  if (probe.strategy !== "refused") return { kind: "editable" };
  if (probe.reason !== "insideXObject" || probe.groupObjectId === undefined) {
    toast(refusalKey(probe.reason), undefined, { tone: "danger" });
    return NONE;
  }
  try {
    const result = await api.ungroupObject({
      docId, page, objectId: probe.groupObjectId, expectGeneration: probe.docGeneration ?? newest(page), at,
    });
    stored(page, result);
    return { kind: "ungrouped", result };
  } catch (e) {
    fail(e);
    return NONE;
  }
}

/** Same place, same size: the listing still shows the same group under that id. */
function sameRect(a: Rect, b: Rect): boolean {
  const near = (x: number, y: number) => Math.abs(x - y) <= 0.5;
  return near(a.l, b.l) && near(a.b, b.b) && near(a.r, b.r) && near(a.t, b.t);
}

/**
 * The Inspector's 그룹 해제 came back `stale`: the page is listed again and, when the same group is
 * still there (same id, same box — the document moved on elsewhere), it is ungrouped once more at
 * the new generation without a word. A second `stale` keeps the group selected and says to press
 * again; a group that is gone (someone else ungrouped it) says nothing.
 */
async function ungroupAgain(docId: string, page: PageIndex, objectId: ObjectId): Promise<UngroupOutcome> {
  const before = useEditStore.getState().pages[page]?.objects.find((o) => o.objectId === objectId);
  await relist(docId, page);
  const listing = useEditStore.getState().pages[page];
  const now = listing?.objects.find((o) => o.objectId === objectId);
  if (!listing || !now || now.type !== "form" || (before && !sameRect(before.rect, now.rect))) return NONE;
  try {
    const result = await api.ungroupObject({ docId, page, objectId, expectGeneration: newest(page) });
    stored(page, result);
    return { kind: "ungrouped", result };
  } catch (e) {
    if (!(api.isSeePdfError(e) && e.code === "stale")) {
      fail(e);
      return NONE;
    }
    await relist(docId, page);
    // still the same group: keep it selected, so pressing again really works
    const again = useEditStore.getState().pages[page]?.objects.find((o) => o.objectId === objectId);
    if (again?.type === "form" && sameRect(now.rect, again.rect)) {
      useEditStore.getState().select(page, [objectId]);
      toast("error.stale", undefined, { tone: "danger" });
    }
    return NONE;
  }
}

/** `reasonKey` without importing the actions module (it imports this one lazily). */
function refusalKey(reason: string | undefined): string {
  switch (reason) {
    case "noUnicode":
      return "edit.readOnly.noUnicode";
    case "type3":
      return "edit.readOnly.type3";
    case "invisible":
      return "edit.reason.invisible";
    default:
      return "edit.reason.unknown";
  }
}

/**
 * The probe at `at` was refused `insideXObject`: ask 그룹 해제 후 편집할까요?, then ungroup the group the
 * probe names (`groupObjectId`) down to the clicked text.
 */
export async function confirmUngroup(docId: string, page: PageIndex, probe: ParagraphProbe, at: Point): Promise<UngroupOutcome> {
  const objectId = probe.groupObjectId ?? probe.objectIds[0];
  if (objectId === undefined) return NONE;
  const ok = await askConfirm({
    titleKey: "edit.ungroup.title",
    bodyKey: "edit.ungroup.body",
    confirmKey: "edit.ungroup.confirm",
  });
  if (!ok) return NONE;
  return ungroup(docId, page, objectId, probe.docGeneration ?? newest(page), at);
}

/**
 * After a 그룹 해제 for an edit the paragraph could still not be edited (the probe refused for
 * another reason): take the 그룹 해제 back — the user asked for an edit, not for a changed document —
 * as long as it is still the last step.
 */
export async function undoUngroup(docId: string, result: UngroupResult): Promise<void> {
  const info = await api.getDocument({ docId }).catch(() => null);
  if (!info || info.docGeneration !== result.docGeneration || info.undoLabel !== "undo.ungroup") return;
  if (useDocStore.getState().info?.docId === docId) await useDocStore.getState().undo();
  else await api.undo({ docId }).catch(() => undefined);
}

let selecting: Promise<boolean> | null = null;

/**
 * Inspector 그룹 해제: the selected group — and the groups inside it that hold text — become their
 * objects. One at a time: a second press while one runs (a double-click on the button) would be
 * pinned to the same generation and come back `stale`.
 */
export function ungroupSelection(): Promise<boolean> {
  selecting ??= ungroupSelected().finally(() => {
    selecting = null;
  });
  return selecting;
}

async function ungroupSelected(): Promise<boolean> {
  const store = useEditStore.getState();
  const sel = store.selection;
  const docId = store.docId ?? useDocStore.getState().info?.docId;
  if (!sel || !docId || sel.ids.length !== 1) return false;
  const page = sel.page;
  const object = store.pages[page]?.objects.find((o) => o.objectId === sel.ids[0]);
  if (!object || object.type !== "form") return false;
  // the newest generation, like every other object action (`actions.generationFor`): the page's
  // listing may be older than the document (an edit on another page) with its objects unchanged
  const outcome = await ungroup(docId, page, object.objectId, newest(page));
  if (outcome.kind !== "ungrouped") return false;
  const ids = outcome.result.newObjectIds;
  if (ids.length) useEditStore.getState().select(page, ids);
  toast("edit.ungroup.done", { count: ids.length });
  return true;
}
