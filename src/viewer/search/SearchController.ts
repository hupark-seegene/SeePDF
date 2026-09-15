/**
 * Search (F-07): `search_start` streams one event per page, outward from the current page, and a
 * new query cancels the previous pass. The merge has to cope with that ordering — page 7 arrives
 * before page 0 — while the results list, the hit counter and ⌘G / ⇧⌘G all work off one list in
 * document order.
 *
 * The merge and the navigation are pure functions so they can be tested without a backend
 * (`SearchController.merge`); the store is the thin part that calls the API.
 */
import { create } from "zustand";
import * as api from "../../ipc/api";
import type { DocId, JobId, PageIndex, SearchEvent, SearchHit } from "../../ipc/types";

export interface SearchOptions {
  matchCase: boolean;
  wholeWord: boolean;
}

/**
 * Replace one page's hits inside the document-ordered list. Replacing (rather than appending)
 * keeps a re-delivered page — a re-query of cached text, an edit that re-scans one page — from
 * duplicating its hits.
 */
export function mergeHits(hits: SearchHit[], page: PageIndex, pageHits: SearchHit[]): SearchHit[] {
  const kept = hits.filter((hit) => hit.page !== page);
  const incoming = [...pageHits].sort((a, b) => a.charStart - b.charStart);
  const out = [...kept, ...incoming];
  out.sort((a, b) => a.page - b.page || a.charStart - b.charStart || a.charLength - b.charLength);
  return out;
}

/** The hit a fresh search should select first: the first one at or after `fromPage`, else the first. */
export function firstHitIndex(hits: SearchHit[], fromPage: PageIndex): number {
  if (hits.length === 0) return -1;
  const index = hits.findIndex((hit) => hit.page >= fromPage);
  return index >= 0 ? index : 0;
}

/** ⌘G / ⇧⌘G — wraps around, and works before the pass has finished. */
export function stepHitIndex(hits: SearchHit[], current: number, direction: 1 | -1): number {
  if (hits.length === 0) return -1;
  if (current < 0) return direction === 1 ? 0 : hits.length - 1;
  return (current + direction + hits.length) % hits.length;
}

export interface SearchState {
  docId: DocId | null;
  query: string;
  matchCase: boolean;
  wholeWord: boolean;
  running: boolean;
  jobId: JobId | null;
  hits: SearchHit[];
  /** hits grouped by page, for the in-page highlight layer */
  byPage: Map<PageIndex, SearchHit[]>;
  total: number;
  scanned: number;
  current: number;
  /** bumped whenever the current hit changes, so the scroller can react to a repeat ⌘G */
  navNonce: number;
  error: string | null;

  setOptions(options: Partial<SearchOptions>): void;
  run(docId: DocId, query: string, fromPage: PageIndex): Promise<void>;
  rerun(docId: DocId, fromPage: PageIndex): Promise<void>;
  cancel(): Promise<void>;
  select(index: number): void;
  step(direction: 1 | -1): void;
  clear(): void;
}

function group(hits: SearchHit[]): Map<PageIndex, SearchHit[]> {
  const map = new Map<PageIndex, SearchHit[]>();
  for (const hit of hits) {
    const list = map.get(hit.page);
    if (list) list.push(hit);
    else map.set(hit.page, [hit]);
  }
  return map;
}

/** Only the newest pass may write into the store; older channels keep arriving until cancel lands. */
let activeToken: symbol | null = null;

export const useSearchStore = create<SearchState>((set, get) => ({
  docId: null,
  query: "",
  matchCase: false,
  wholeWord: false,
  running: false,
  jobId: null,
  hits: [],
  byPage: new Map(),
  total: 0,
  scanned: 0,
  current: -1,
  navNonce: 0,
  error: null,

  setOptions(options) {
    set({ ...options });
  },

  async cancel() {
    const jobId = get().jobId;
    set({ jobId: null, running: false });
    if (jobId !== null) await api.cancelJob({ jobId }).catch(() => undefined);
  },

  async run(docId, query, fromPage) {
    await get().cancel();
    const trimmed = query;
    set({
      docId,
      query: trimmed,
      hits: [],
      byPage: new Map(),
      total: 0,
      scanned: 0,
      current: -1,
      error: null,
      running: trimmed.length > 0,
    });
    if (trimmed.length === 0) return;

    const { matchCase, wholeWord } = get();
    const token = Symbol("search");
    activeToken = token;

    const onEvent = (event: SearchEvent) => {
      if (activeToken !== token) return;
      const state = get();
      if (event.type === "page") {
        const hits = mergeHits(state.hits, event.page, event.hits);
        const current =
          state.current >= 0 && state.hits[state.current]
            ? hits.indexOf(state.hits[state.current])
            : firstHitIndex(hits, fromPage);
        set({
          hits,
          byPage: group(hits),
          total: hits.length,
          scanned: event.scanned,
          current,
          navNonce: current !== state.current ? state.navNonce + 1 : state.navNonce,
        });
      } else if (event.type === "done") {
        set({ running: false, jobId: null, scanned: event.pagesScanned });
      } else if (event.type === "cancelled") {
        set({ running: false, jobId: null });
      } else if (event.type === "error") {
        set({ running: false, jobId: null, error: event.error.code });
      }
    };

    try {
      const jobId = await api.searchStart({ docId, query: trimmed, matchCase, wholeWord, fromPage }, onEvent);
      if (activeToken === token) set({ jobId });
      else await api.cancelJob({ jobId }).catch(() => undefined);
    } catch (e) {
      if (activeToken === token) set({ running: false, error: api.isSeePdfError(e) ? e.code : "pdfium" });
    }
  },

  async rerun(docId, fromPage) {
    await get().run(docId, get().query, fromPage);
  },

  select(index) {
    const state = get();
    if (index < 0 || index >= state.hits.length) return;
    set({ current: index, navNonce: state.navNonce + 1 });
  },

  step(direction) {
    const state = get();
    const index = stepHitIndex(state.hits, state.current, direction);
    if (index < 0) return;
    set({ current: index, navNonce: state.navNonce + 1 });
  },

  clear() {
    activeToken = null;
    void get().cancel();
    set({ query: "", hits: [], byPage: new Map(), total: 0, scanned: 0, current: -1, error: null });
  },
}));
