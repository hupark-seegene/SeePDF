/**
 * Pure logic behind the 보안 dialog (P1-2 / P1-3): password validation and the `set_password`
 * payload. Kept out of the component so it is unit-testable.
 */
import type { DocId, Permissions, SanitizeOptions } from "../ipc/types";

/** v0.3 S3 문서 정리: the checkboxes, in display order. */
export const SANITIZE_KEYS: readonly (keyof SanitizeOptions)[] = [
  "javascript", "attachments", "actions", "metadata", "hiddenLayers",
];

export interface SecurityForm {
  openPassword: string;
  openConfirm: string;
  ownerPassword: string;
  ownerConfirm: string;
  print: boolean;
  copy: boolean;
  modify: boolean;
  annotate: boolean;
}

export const EMPTY_SECURITY_FORM: SecurityForm = {
  openPassword: "",
  openConfirm: "",
  ownerPassword: "",
  ownerConfirm: "",
  print: true,
  copy: true,
  modify: true,
  annotate: true,
};

export type PasswordError = "empty" | "openMismatch" | "ownerMismatch" | "ownerRequired";

/** True when any permission checkbox is off, i.e. the file will carry restrictions. */
export function restricts(form: SecurityForm): boolean {
  return !(form.print && form.copy && form.modify && form.annotate);
}

export function validatePasswords(form: SecurityForm): { ok: boolean; error?: PasswordError } {
  if (form.openPassword !== form.openConfirm) return { ok: false, error: "openMismatch" };
  if (form.ownerPassword !== form.ownerConfirm) return { ok: false, error: "ownerMismatch" };
  if (!form.openPassword && !form.ownerPassword) return { ok: false, error: "empty" };
  // A restriction only holds if the owner password is one the reader does not have. With no
  // owner password (or one equal to the open password) anyone who can open the file gets
  // owner rights and the unchecked permissions mean nothing — refuse instead of silently
  // writing a file that looks restricted but is not.
  if (restricts(form) && (!form.ownerPassword || form.ownerPassword === form.openPassword)) {
    return { ok: false, error: "ownerRequired" };
  }
  return { ok: true };
}

/** The checkboxes, in `Permissions` terms. Every flag is sent so an explicit `false` is never lost. */
export function permissionsOf(form: SecurityForm): Partial<Permissions> {
  return { print: form.print, extractText: form.copy, modify: form.modify, annotate: form.annotate };
}

/**
 * 권한 암호 is optional when every permission is allowed: blank → the open password doubles as
 * the owner password (`validatePasswords` refuses that combination once a permission is
 * unchecked). A blank open password means "no password to open", i.e. `userPassword` is omitted.
 */
export function buildSetPasswordArgs(docId: DocId, outPath: string, form: SecurityForm) {
  return {
    docId,
    outPath,
    userPassword: form.openPassword || undefined,
    ownerPassword: form.ownerPassword || form.openPassword,
    permissions: permissionsOf(form),
  };
}
