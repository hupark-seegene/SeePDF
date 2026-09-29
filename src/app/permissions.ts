/**
 * v0.3 pkg3 (S5): the document's permission flags, as the UI uses them. Tiny — it sits in the entry
 * chunk because the dispatcher, the mode switcher and the title bar all read it.
 *
 * The engine enforces the same flags (`engine::security::ensure_permitted`); the UI only disables
 * the entry and says why, so nobody hits a `permissionDenied` toast by surprise. The one side effect
 * is the mode guard at the bottom: the current mode is always one the open document permits.
 */
import type { DocInfo, Permissions } from "../ipc/types";
import { useAppStore, type Mode } from "../store/appStore";
import { useDocStore } from "../store/docStore";
import { toast } from "./toastStore";

export type PermKey = keyof Omit<Permissions, "revision">;

/** Display order of the 제한됨 popover. */
export const PERM_KEYS: readonly PermKey[] = ["print", "extractText", "modify", "annotate", "fillForms", "assemble"];

/** The permissions this open does not grant (empty for an unrestricted or unlocked document). */
export function restrictions(info: DocInfo | null | undefined): PermKey[] {
  if (!info?.encrypted) return [];
  return PERM_KEYS.filter((k) => !info.permissions[k]);
}

/** Which permission each gated command id needs. */
const NEEDS: Record<string, PermKey> = {
  "file.print": "print",
  "mode.edit": "modify",
  "mode.pages": "assemble",
  "mode.annotate": "annotate",
  "mode.form": "fillForms",
  // 도구 › 영역 표시 arms the redaction tool in 편집 mode
  "tools.redact": "modify",
  // v0.3 integration (pkg6 V1 스냅샷): a snapshot copies the page content, like 복사
  "tool.snapshot": "extractText",
  // v0.3 integration (pkg2 F1 / F2 × S5): 필드 만들기 and 양식 평면화 change the form's structure,
  // which a PDF allows only with "modify" on top of "fill forms" (the engine's `refuse_encrypted`)
  "tool.fieldText": "modify",
  "tool.fieldCheckbox": "modify",
  "tool.fieldRadio": "modify",
  "tool.fieldCombo": "modify",
  "tool.fieldSignature": "modify",
  "tool.formFlatten": "modify",
  // v0.3 integration (pkg7 × S5): 텍스트 인식 writes a text layer, a modification (the engine's
  // `perm_for`); pkg7's 스캔 문서 banner and 검색 hint offer the same command
  "tools.ocr": "modify",
};

/** The i18n key of the reason `id` is not allowed on this document, or `null` when it is. */
export function permissionBlock(id: string, info: DocInfo | null | undefined): string | null {
  const need = NEEDS[id];
  if (!need || !info || info.permissions[need]) return null;
  return reasonKey(need);
}

export function reasonKey(perm: PermKey): string {
  return `security.restricted.reason.${perm}`;
}

/** Structure edits (목차, 페이지 레이블, 페이지 링크, 메타데이터) are modifications. */
export function structureLocked(info: DocInfo | null | undefined): boolean {
  return !!info && !info.permissions.modify;
}

/** Copying the document's text (the clipboard mirror, ⌘C, the context menu's 복사). */
export function copyForbidden(info: DocInfo | null | undefined): boolean {
  return !!info && !info.permissions.extractText;
}

/** The reason `mode` is not allowed on this document, or `null` (읽기 always is). */
export function modeBlock(mode: Mode, info: DocInfo | null | undefined): string | null {
  return permissionBlock(`mode.${mode}`, info);
}

/**
 * The mode invariant. Every entry point that changes mode (the switcher, 도구 › 영역 표시, the context
 * menus, the annotation list) is gated on its own, but a mode kept from the previous document — or one
 * a future caller sets without asking — must not leave a forbidden tool armed: when the open document
 * forbids the current mode, fall back to the previous mode (when a mode change caused it and that one
 * is allowed) or 읽기, and say why.
 */
function enforceMode(fallback: Mode): void {
  const info = useDocStore.getState().info;
  const app = useAppStore.getState();
  const blocked = modeBlock(app.mode, info);
  if (!blocked) return;
  app.setMode(modeBlock(fallback, info) ? "read" : fallback);
  toast(blocked, undefined, { tone: "info" });
}

useDocStore.subscribe((s, prev) => {
  if (s.info !== prev.info) enforceMode("read");
});
useAppStore.subscribe((s, prev) => {
  if (s.mode !== prev.mode) enforceMode(prev.mode);
});
