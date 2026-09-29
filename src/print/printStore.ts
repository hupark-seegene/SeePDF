/**
 * The state of one in-flight print job (F-26).
 *
 * `runPrint("document", …)` fills this, `PrintRoot` renders the hidden page images from it,
 * and the job clears itself once the OS print panel has been dismissed. It is deliberately a
 * store rather than local state: the print root is mounted next to the app shell (so the
 * print stylesheet can hide everything *but* it), while the trigger is a dialog five levels
 * away that has already closed by the time the images load.
 *
 * v0.3 (X7/X8/X2): the job also carries the 주석 option (the `print=` flag of every page URL),
 * the page sizes (the sheet orientation is marked on each sheet, and 실제 크기 can
 * be printed in physical units), 흑백, and the chunk being printed: more than `PRINT_CHUNK`
 * pages are printed as consecutive print jobs of at most that many decoded images each.
 *
 * This module is in the entry chunk (App.tsx reads `job !== null`): keep it tiny.
 */
import { create } from "zustand";
import type { DocGeneration, DocId, PageIndex, Rotation } from "../ipc/types";
import type { PaperId } from "./paperChoice";

/** 인쇄 ▸ 주석: 문서와 주석 / 문서만 / 문서와 도장·서명 (the engine's `PrintAnnots`). */
export type PrintAnnots = "all" | "none" | "stamps";
/** 인쇄 ▸ 크기: 맞춤 (the page as wide as the sheet) / 실제 크기 (on the chosen paper). */
export type PrintFit = "fit" | "actual";
/** 인쇄 ▸ 모아찍기. */
export type PerSheet = 1 | 2 | 4 | 6 | 9;
export type NupOrder = "across" | "down";

/** Everything the 인쇄 dialog decides besides the range and the method. */
export interface PrintOptions {
  annots?: PrintAnnots;
  fit?: PrintFit;
  grayscale?: boolean;
  perSheet?: PerSheet;
  order?: NupOrder;
  booklet?: boolean;
  /** v0.3.0: the paper 실제 크기 lays the pages out for (`src/print/paper.ts`). */
  paper?: PaperId;
}

export interface PrintJob {
  docId: DocId;
  generation: DocGeneration;
  /** The pages to print, in order — already resolved from the 인쇄 범위 picker. */
  pages: PageIndex[];
  rotation: Rotation;
  /** `scaleKey` of the page images: `round(dpi / 72 × 100)`. */
  scaleKey: number;
  /** v0.3 (X7): the `print=` flag of every page URL; default `all`. */
  annots?: PrintAnnots;
  /** v0.3 (X8): display size in points of each entry of `pages` (same order). */
  sizes?: [number, number][];
  /** v0.3 (X2): default `fit`. */
  fit?: PrintFit;
  /** v0.3.0: the paper of a 실제 크기 job; default A4. */
  paper?: PaperId;
  /** v0.3.0: the sheet of a 실제 크기 job in points, landscape applied (`paper.ts` paperSize). */
  sheet?: [number, number];
  grayscale?: boolean;
  /** v0.3 (X2): a temporary n-up document to close once the job is over. */
  tempDocId?: DocId;
}

export interface PrintState {
  job: PrintJob | null;
  /** v0.3 (X8): which `PRINT_CHUNK`-page slice of `job.pages` is mounted. */
  chunk: number;
  /** How many images of the mounted chunk have settled (loaded **or** failed). */
  settled: number;
  /**
   * v0.3 (X8): the print panel of a chunk that is not the last has closed and the next chunk
   * waits for the user (`afterprint` fires for 인쇄 and 취소 alike, so it cannot go on alone).
   */
  waiting: boolean;
  start(job: PrintJob): void;
  noteSettled(): void;
  /** The print panel closed: wait for the user before the next chunk, or `clear()` after the last. */
  chunkDone(): void;
  /** The next chunk, or `clear()` after the last one. */
  advance(): void;
  clear(): void;
}

/** v0.3 (X8): at most this many page images are decoded at once (~9 MB each at 150 DPI A4). */
export const PRINT_CHUNK = 50;

export const usePrintStore = create<PrintState>((set, get) => ({
  job: null,
  chunk: 0,
  settled: 0,
  waiting: false,
  start(job) {
    set({ job, chunk: 0, settled: 0, waiting: false });
  },
  noteSettled() {
    set((s) => ({ settled: s.settled + 1 }));
  },
  chunkDone() {
    const { job, chunk } = get();
    if (job && (chunk + 1) * PRINT_CHUNK < job.pages.length) set({ waiting: true, settled: 0 });
    else get().clear();
  },
  advance() {
    const { job, chunk } = get();
    if (job && (chunk + 1) * PRINT_CHUNK < job.pages.length)
      set({ chunk: chunk + 1, settled: 0, waiting: false });
    else get().clear();
  },
  clear() {
    set({ job: null, chunk: 0, settled: 0, waiting: false });
  },
}));

/** 150 DPI is the F-26 default: sharp on paper, and a chunk of 50 pages fits in memory. */
export const PRINT_DPI = 150;

export function scaleKeyForDpi(dpi: number): number {
  return Math.round((dpi / 72) * 100);
}
