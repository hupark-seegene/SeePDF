/**
 * "이 문서에는 검색 가능한 텍스트가 없습니다" (v0.3 O6): does the open document need OCR?
 *
 * The primary user receives scanned 공문 and contracts. Search on such a file answers 0 results and a
 * drag selects nothing, with no hint that 텍스트 인식 would fix it. So after a document opens — off the
 * critical path, once the window is idle — the first `SAMPLE_PAGES` pages go through `ocr_page_status`;
 * if none of them has text, the document is marked image-only: the viewer shows `NeedsOcrBanner` and
 * the 검색 panel's empty state says the same.
 *
 * Tiny on purpose (a zustand store and one call): the viewer and the 검색 panel import it, and neither
 * may pull the OCR sheet or tesseract.js with it.
 */
import { create } from "zustand";
import * as api from "../ipc/api";
import type { DocId, PageIndex } from "../ipc/types";

/** How many leading pages are sampled; a scan is a scan from its first page. */
export const SAMPLE_PAGES = 10;

interface NeedsOcrState {
  /** the document the answer is about */
  docId: DocId | null;
  /** `true`: none of the sampled pages has text */
  imageOnly: boolean;
  /** document paths whose banner was dismissed this session */
  dismissed: string[];
}

export const useNeedsOcr = create<NeedsOcrState>(() => ({ docId: null, imageOnly: false, dismissed: [] }));

/** The key a dismissal is remembered under: the path, or the id of an unsaved document. */
export function dismissKey(docId: DocId, path: string | null | undefined): string {
  return path || `doc:${docId}`;
}

/** `true` when `docId` is known to be image-only. */
export function isImageOnly(docId: DocId | null | undefined): boolean {
  const s = useNeedsOcr.getState();
  return !!docId && s.docId === docId && s.imageOnly;
}

/** The hook the 검색 panel reads: is the open document image-only? */
export function useImageOnly(docId: DocId | null | undefined): boolean {
  return useNeedsOcr((s) => !!docId && s.docId === docId && s.imageOnly);
}

let probe = 0;

/**
 * Sample `ocr_page_status` for `docId` (its first `SAMPLE_PAGES` pages). The latest call wins; a
 * failure leaves the document marked as having text (no banner is better than a wrong one).
 */
export async function probeDocument(docId: DocId, pageCount: number): Promise<boolean> {
  const mine = ++probe;
  const pages: PageIndex[] = Array.from({ length: Math.min(SAMPLE_PAGES, Math.max(0, pageCount)) }, (_, i) => i);
  if (pages.length === 0) {
    useNeedsOcr.setState({ docId, imageOnly: false });
    return false;
  }
  let imageOnly = false;
  try {
    const status = await api.ocrPageStatus({ docId, pages });
    imageOnly = status.length > 0 && status.every((s) => !s.hasText);
  } catch {
    imageOnly = false;
  }
  if (mine === probe) useNeedsOcr.setState({ docId, imageOnly });
  return imageOnly;
}

/** Re-check after an edit that may have added text (OCR, a text box) — only while marked image-only. */
export function recheckAfterChange(docId: DocId, pageCount: number): void {
  if (isImageOnly(docId)) void probeDocument(docId, pageCount);
}

/** 닫기 on the banner: not again for this document (by path) in this session. */
export function dismiss(key: string): void {
  const s = useNeedsOcr.getState();
  if (!s.dismissed.includes(key)) useNeedsOcr.setState({ dismissed: [...s.dismissed, key] });
}

/** Tests only. */
export function resetNeedsOcr(): void {
  probe++;
  useNeedsOcr.setState({ docId: null, imageOnly: false, dismissed: [] });
}
