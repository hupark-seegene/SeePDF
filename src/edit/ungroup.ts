/**
 * v0.3 (pkg1, R4) 그룹 해제 before a text edit. Text inside a Form XObject cannot be edited in place
 * (PDFium would drop the change on save), so a click with 텍스트 수정 on such a run asks
 * "그룹 해제 후 편집할까요?"; yes → `ungroup_object` (one undo step 그룹 해제) and the caller probes the
 * paragraph again, now made of ordinary page objects.
 */
import * as api from "../ipc/api";
import type { PageIndex, ParagraphProbe } from "../ipc/types";
import { toast } from "../app/toastStore";
import { askConfirm } from "../dialogs/dialogState";
import { useEditStore } from "./editStore";

/** `true` when the group under the refused probe was ungrouped (the caller re-probes). */
export async function confirmUngroup(docId: string, page: PageIndex, probe: ParagraphProbe): Promise<boolean> {
  const objectId = probe.objectIds[0];
  if (objectId === undefined) return false;
  const ok = await askConfirm({
    titleKey: "edit.ungroup.title",
    bodyKey: "edit.ungroup.body",
    confirmKey: "edit.ungroup.confirm",
  });
  if (!ok) return false;
  try {
    const expectGeneration = probe.docGeneration ?? useEditStore.getState().pages[page]?.docGeneration ?? 0;
    const result = await api.ungroupObject({ docId, page, objectId, expectGeneration });
    useEditStore.getState().setPage(page, { docGeneration: result.docGeneration, objects: result.objects });
    return true;
  } catch (e) {
    toast(api.errorKey(e), undefined, {
      tone: "danger",
      detail: e instanceof Error || api.isSeePdfError(e) ? e.message : String(e),
    });
    return false;
  }
}
