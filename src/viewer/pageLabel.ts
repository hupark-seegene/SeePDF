/**
 * Page labels on screen (P2) — the two helpers the always-loaded chrome needs (status bar, 축소판,
 * 목차, 페이지 mode). `DocInfo.pageLabels` is absent when the document has none; a page whose label
 * is empty shows its number. The label *formatting* lives with the dialog (`dialogs/pageLabels.ts`).
 */
import type { PageIndex } from "../ipc/types";

/** What a page is called on screen: its label when it has a non-empty one, else its number. */
export function displayLabel(labels: string[] | undefined, page: PageIndex): string {
  return labels?.[page] || String(page + 1);
}

/**
 * The page a status-bar entry names: an exact label first (a label is what the user sees), then a
 * case-insensitive label, then a plain 1-based number. `null` when nothing matches.
 */
export function pageForEntry(entry: string, labels: string[] | undefined, pageCount: number): PageIndex | null {
  const text = entry.trim();
  if (!text) return null;
  if (labels?.length) {
    const exact = labels.indexOf(text);
    if (exact >= 0) return exact;
    const lower = text.toLowerCase();
    const loose = labels.findIndex((l) => l.toLowerCase() === lower);
    if (loose >= 0) return loose;
  }
  if (/^\d+$/.test(text)) {
    const n = Number(text);
    if (n >= 1 && n <= pageCount) return n - 1;
  }
  return null;
}
