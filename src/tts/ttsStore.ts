/**
 * 읽어 주기 (P2) state. Lives in the entry chunk because the status bar's indicator and `App`'s lazy
 * floating bar read it; everything that talks to the engine is in the lazy `speak.ts`.
 */
import { create } from "zustand";

/** The speeds the 속도 control offers (a multiplier of the voice's normal rate). */
export const TTS_RATES = [0.75, 1, 1.25, 1.5, 2] as const;

export interface TtsState {
  /** the system voice is speaking */
  speaking: boolean;
  /** what is being read, so a new 속도 can restart it */
  text: string;
  source: "selection" | "page" | null;
  /** 1-based page number when `source` is `page` */
  page: number | null;
  voice: string | null;
  rate: number;
}

export const useTtsStore = create<TtsState>(() => ({
  speaking: false,
  text: "",
  source: null,
  page: null,
  voice: null,
  rate: 1,
}));
