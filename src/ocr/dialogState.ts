/**
 * The seam between the dialogs module (e) and the OCR module (f).
 *
 * (e) renders `<OcrDialog />` once in its dialog host and calls `openOcrDialog()` from the
 * `tools.ocr` command / the ⋯ overflow menu; the dialog is invisible until then. Everything else
 * (the worker pool, the job, the progress) stays behind this file.
 *
 * A tiny store rather than React context: the command dispatcher (`useCommands.ts`) is not a
 * component and must be able to open the dialog from anywhere.
 */
import { create } from "zustand";
import type { DocGeneration, DocId, PageGeom, PageIndex } from "../ipc/types";

/** Everything the dialog needs about the document. Omitted fields are read from `docStore`. */
export interface OcrDialogContext {
  docId?: DocId;
  docGeneration?: DocGeneration;
  pageCount?: number;
  pages?: PageGeom[];
  /** 0-based, for 현재 페이지 */
  currentPage?: PageIndex;
  /** preselect 페이지 범위 with these pages (the page organizer's selection) */
  selectedPages?: PageIndex[];
}

interface OcrDialogStore {
  open: boolean;
  context: OcrDialogContext;
  /** bumped on every open so the dialog remounts its form state */
  nonce: number;
  show(context?: OcrDialogContext): void;
  hide(): void;
}

export const useOcrDialogStore = create<OcrDialogStore>((set, get) => ({
  open: false,
  context: {},
  nonce: 0,
  show(context = {}) {
    set({ open: true, context, nonce: get().nonce + 1 });
  },
  hide() {
    set({ open: false });
  },
}));

/** Open the OCR dialog (UI_SPEC §10). Safe to call from outside React. */
export function openOcrDialog(context?: OcrDialogContext): void {
  useOcrDialogStore.getState().show(context);
}

/** Close it. The dialog closes itself on 취소/Esc; this exists for the host. */
export function closeOcrDialog(): void {
  useOcrDialogStore.getState().hide();
}

/** `true` while the dialog is on screen — for the host's backdrop/focus bookkeeping. */
export function useOcrDialogOpen(): boolean {
  return useOcrDialogStore((s) => s.open);
}
