/**
 * 문서 탭 (v0.3 DR1): the documents open in this window, in strip order.
 *
 * The **active** tab's document is the one in `docStore`, and every per-document store (view, pages,
 * annotations, edit, search) describes it — so nothing that reads those stores had to learn about
 * tabs. A background tab keeps its engine document open and its per-document UI state parked in
 * `parked[id]` (`tabs/flow.ts` captures and restores it, the way 분할 보기 parks the other pane).
 *
 * Entry chunk (the title bar's strip reads it): the switching, closing and prompting live in the
 * lazy `tabs/flow.ts`. What is here: the list, the mirror that keeps it in step with `docStore`
 * (a document opened into an empty window, or replaced in place, needs no tab code at all), the
 * window binding, and background `doc-changed`.
 */
import { create } from "zustand";
import * as api from "../ipc/api";
import { windowLabel } from "../ipc/env";
import { useDocStore } from "./docStore";
import type { DocChangedEvent, DocId, DocInfo } from "../ipc/types";
import type { TabSnapshot } from "../tabs/flow";

export interface Tab {
  id: number;
  docId: DocId;
  name: string;
  path: string | null;
  dirty: boolean;
}

export interface TabState {
  tabs: Tab[];
  /** the tab whose document is in `docStore`; `null` while a new tab is being opened (or none) */
  activeId: number | null;
  /** every background tab's per-document state, by tab id */
  parked: Record<number, TabSnapshot>;
  /** paths of closed tabs, newest last (⌘⇧T) */
  closed: string[];
  /**
   * v0.3.0: a document is opening into a new tab (`beginNewTab` parked the active one, so the
   * document `docStore` gets next becomes a tab of its own): switching and closing tabs are refused
   * until it has its tab or the open was rolled back
   */
  opening: boolean;
  /** drag to reorder: `id` goes to position `to` */
  move(id: number, to: number): void;
}

let nextId = 1;

export const useTabStore = create<TabState>((set, get) => ({
  tabs: [],
  activeId: null,
  parked: {},
  closed: [],
  opening: false,
  move(id, to) {
    const tabs = [...get().tabs];
    const from = tabs.findIndex((t) => t.id === id);
    if (from < 0 || from === to) return;
    const [tab] = tabs.splice(from, 1);
    tabs.splice(Math.max(0, Math.min(to, tabs.length)), 0, tab);
    set({ tabs });
  },
}));

function meta(info: DocInfo): Omit<Tab, "id"> {
  return { docId: info.docId, name: info.name, path: info.path, dirty: info.dirty };
}

/** The parked documents of the background tabs (autosave writes their recovery copies too). */
export function backgroundDocs(): DocInfo[] {
  const s = useTabStore.getState();
  return s.tabs.flatMap((t) => (t.id !== s.activeId && s.parked[t.id] ? [s.parked[t.id].doc.info] : []));
}

// The strip follows `docStore`: a document that is not in a tab becomes one — in place of the
// active tab when it replaced that tab's document (다시 불러오기, 합치기 in 새 창 mode), else a new
// tab; a closed document leaves its tab; a name / dirty / path change is copied over.
useDocStore.subscribe((s, prev) => {
  if (s.info === prev.info) return;
  const st = useTabStore.getState();
  const info = s.info;
  if (!info) {
    const tabs = st.tabs.filter((t) => t.docId !== prev.info?.docId);
    if (tabs.length !== st.tabs.length) {
      useTabStore.setState({ tabs, activeId: tabs.some((t) => t.id === st.activeId) ? st.activeId : null });
    }
    return;
  }
  const own = st.tabs.find((t) => t.docId === info.docId);
  const active = st.tabs.find((t) => t.id === st.activeId);
  const target = own ?? (active && active.docId === prev.info?.docId ? active : null);
  if (target) {
    const next = { ...target, ...meta(info) };
    const same = (Object.keys(next) as (keyof Tab)[]).every((k) => next[k] === target[k]);
    // a parked tab whose document is still on screen (a new tab is opening over it) stays parked
    const activeId = st.parked[target.id] ? st.activeId : target.id;
    if (!same || st.activeId !== activeId) {
      useTabStore.setState({ tabs: st.tabs.map((t) => (t === target ? next : t)), activeId });
    }
    return;
  }
  const id = nextId++;
  useTabStore.setState({ tabs: [...st.tabs, { id, ...meta(info) }], activeId: id, opening: false });
});

// The window binding (`window_bind_document`) follows the active tab — the native 편집 menu names
// its undo step — and lists every tab's document, so "already open" (H8) finds a background tab.
let bound = "";
function bindWindow(): void {
  const docId = useDocStore.getState().info?.docId ?? null;
  const tabs = useTabStore.getState().tabs.map((t) => t.docId);
  const key = `${docId}|${tabs.join()}`;
  if (key === bound) return;
  bound = key;
  void api.windowBindDocument({ label: windowLabel(), docId, tabs }).catch(() => undefined);
}
useDocStore.subscribe((s, prev) => {
  if (s.info?.docId !== prev.info?.docId) bindWindow();
});
useTabStore.subscribe((s, prev) => {
  if (s.tabs !== prev.tabs) bindWindow();
});

/**
 * A background tab's document changed outside `docStore` (a `doc-changed`, a save that finished
 * after a switch): its tab and its parked `DocInfo` (`patch`) follow; `dirty` is the tab's dot
 * when it has no snapshot.
 */
export function patchBackgroundDoc(docId: DocId, dirty: boolean, patch: (info: DocInfo) => DocInfo): void {
  const st = useTabStore.getState();
  const tab = st.tabs.find((t) => t.docId === docId && t.id !== st.activeId);
  if (!tab) return;
  const snap = st.parked[tab.id];
  const info = snap && patch(snap.doc.info);
  useTabStore.setState({
    tabs: st.tabs.map((t) => (t === tab ? { ...t, ...(info ? meta(info) : { dirty }) } : t)),
    parked: info ? { ...st.parked, [tab.id]: { ...snap, doc: { ...snap.doc, info } } } : st.parked,
  });
}

/** `doc-changed` for a background tab (an autosave, a batch job): its dot and parked state follow. */
export function tabDocChanged(e: DocChangedEvent): void {
  patchBackgroundDoc(e.docId, e.dirty, (info) => ({
    ...info,
    docGeneration: e.docGeneration,
    dirty: e.dirty,
    canUndo: e.canUndo ?? info.canUndo,
    canRedo: e.canRedo ?? info.canRedo,
  }));
}

/** Test seam: forget every tab (the stores are module singletons). */
export function resetTabs(): void {
  bound = "";
  useTabStore.setState({ tabs: [], activeId: null, parked: {}, closed: [], opening: false });
}
