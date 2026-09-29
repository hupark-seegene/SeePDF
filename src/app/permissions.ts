/**
 * v0.3 pkg3 (S5): the document's permission flags, as the UI uses them. Pure and tiny — it sits in
 * the entry chunk because the dispatcher, the mode switcher and the title bar all read it.
 *
 * The engine enforces the same flags (`engine::security::ensure_permitted`); the UI only disables
 * the entry and says why, so nobody hits a `permissionDenied` toast by surprise.
 */
import type { DocInfo, Permissions } from "../ipc/types";

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
