/**
 * 실행 취소 / 다시 실행 with the step's name: the engine sends `DocInfo.undoLabel` /
 * `redoLabel` as i18n keys (`undo.watermark`, …). A key this build does not know (an older
 * frontend against a newer engine) falls back to the plain command name instead of leaking the key.
 */
import { catalogues, getLocale, t, type Locale } from "../i18n";

export function historyTitle(kind: "undo" | "redo", key: string | null | undefined, locale: Locale = getLocale()): string {
  const plain = t(kind === "undo" ? "menu.edit.undo" : "menu.edit.redo", undefined, locale);
  if (!key) return plain;
  const cat = catalogues[locale] ?? catalogues.ko;
  const action = cat[key];
  if (action === undefined) return plain;
  return t(kind === "undo" ? "menu.edit.undoAction" : "menu.edit.redoAction", { action }, locale);
}
