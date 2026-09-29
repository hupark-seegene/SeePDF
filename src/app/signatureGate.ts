/**
 * v0.3 pkg3 (S1): the first document-changing command on a digitally signed document asks once —
 * "편집하면 서명이 무효화될 수 있습니다" — and every later one goes straight through. 취소 rejects the
 * command with `cancelled` (`api.DECLINED`, which toasts skip), so the edit simply does not happen.
 *
 * Installed once from the title bar (entry chunk); it hooks `api.ts`'s one `call()` so no caller
 * has to know about signatures.
 */
import { setMutationGate } from "../ipc/api";
import { askConfirm } from "../dialogs/dialogState";
import { useDocStore } from "../store/docStore";
import { backgroundDocs } from "../store/tabStore";
import type { DocId } from "../ipc/types";

/** Documents whose signature prompt was already answered 편집 계속 (per docId, per session). */
const acknowledged = new Set<DocId>();
/** A prompt in flight per document, so a burst of edits asks once. */
const pending = new Map<DocId, Promise<boolean>>();

export async function signatureGate(_command: string, args: { docId?: unknown }): Promise<boolean> {
  const docId = typeof args.docId === "string" ? args.docId : null;
  if (!docId) return true;
  // v0.3.0: a change can land on a background tab's document (텍스트 인식 applies its pages after
  // recognition, and the tab may have been switched meanwhile) — that one asks too (the prompt
  // names the document)
  const shown = useDocStore.getState().info;
  const info = shown?.docId === docId ? shown : backgroundDocs().find((d) => d.docId === docId);
  if (!info?.signatures?.length) return true;
  if (acknowledged.has(docId)) return true;
  let asking = pending.get(docId);
  if (!asking) {
    asking = askConfirm({
      titleKey: "security.signed.editTitle",
      bodyKey: "security.signed.editBody",
      bodyParams: { name: info.name },
      confirmKey: "security.signed.editConfirm",
      danger: true,
    }).finally(() => pending.delete(docId));
    pending.set(docId, asking);
  }
  const ok = await asking;
  if (ok) acknowledged.add(docId);
  return ok;
}

/** Test seam: forget every answer. */
export function resetSignatureGate(): void {
  acknowledged.clear();
  pending.clear();
}

/** Idempotent: the gate is one function, set again. */
export function installSignatureGate(): void {
  setMutationGate(signatureGate);
}
