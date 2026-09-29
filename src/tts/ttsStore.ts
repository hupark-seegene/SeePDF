/**
 * 읽어 주기 (P2) state. Lives in the entry chunk because the status bar's indicator and `App`'s lazy
 * floating bar read it; everything that talks to the engine is in the lazy `speak.ts`.
 *
 * v0.3 (V4): what is read is a queue of sentences (`sentences.ts`); `index` is the one being read,
 * from the engine's `tts-progress` (and every `tts_status` poll). A page's sentences carry their
 * text-layer ranges, which is what `TtsHighlight` paints.
 */
import { create } from "zustand";
import type { DocGeneration, DocId, PageIndex } from "../ipc/types";

/** The speeds the 속도 control offers (a multiplier of the voice's normal rate). */
export const TTS_RATES = [0.75, 1, 1.25, 1.5, 2] as const;

/** One queued sentence (see `sentences.ts`). */
export interface TtsSentenceRef {
  text: string;
  page: PageIndex | null;
  ranges: [number, number][];
}

export interface TtsState {
  /** the system voice is speaking */
  speaking: boolean;
  /** what is being read (the sentences joined), so a new 속도 can restart it */
  text: string;
  source: "selection" | "page" | null;
  /** 1-based page number when `source` is `page` */
  page: number | null;
  voice: string | null;
  rate: number;
  /** v0.3 (V4): the queue, and the sentence being read */
  sentences: TtsSentenceRef[];
  index: number | null;
  /** the document (and generation) the sentences' ranges belong to */
  docId: DocId | null;
  docGeneration: DocGeneration | null;
}

export const useTtsStore = create<TtsState>(() => ({
  speaking: false,
  text: "",
  source: null,
  page: null,
  voice: null,
  rate: 1,
  sentences: [],
  index: null,
  docId: null,
  docGeneration: null,
}));
