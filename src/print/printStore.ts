/**
 * The state of one in-flight print job (F-26).
 *
 * `runPrint("document", …)` fills this, `PrintRoot` renders the hidden page images from it,
 * and the job clears itself once the OS print panel has been dismissed. It is deliberately a
 * store rather than local state: the print root is mounted next to the app shell (so the
 * print stylesheet can hide everything *but* it), while the trigger is a dialog five levels
 * away that has already closed by the time the images load.
 */
import { create } from "zustand";
import type { DocGeneration, DocId, PageIndex, Rotation } from "../ipc/types";

export interface PrintJob {
  docId: DocId;
  generation: DocGeneration;
  /** The pages to print, in order — already resolved from the 인쇄 범위 picker. */
  pages: PageIndex[];
  rotation: Rotation;
  /** `scaleKey` of the page images: `round(dpi / 72 × 100)`. */
  scaleKey: number;
}

export interface PrintState {
  job: PrintJob | null;
  /** How many of `job.pages` have their `<img>` settled (loaded **or** failed). */
  settled: number;
  start(job: PrintJob): void;
  noteSettled(): void;
  clear(): void;
}

export const usePrintStore = create<PrintState>((set) => ({
  job: null,
  settled: 0,
  start(job) {
    set({ job, settled: 0 });
  },
  noteSettled() {
    set((s) => ({ settled: s.settled + 1 }));
  },
  clear() {
    set({ job: null, settled: 0 });
  },
}));

/** 150 DPI is the F-26 default: sharp on paper, and a 500-page job still fits in memory. */
export const PRINT_DPI = 150;

export function scaleKeyForDpi(dpi: number): number {
  return Math.round((dpi / 72) * 100);
}
