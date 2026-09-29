/**
 * 문서 탭 (v0.3 DR1) — everything a tab does beyond the list in `store/tabStore.ts`: switching
 * (park the active document's per-document UI state, restore the other tab's), opening into a new
 * tab, closing (with the usual guards), ⌘⇧T, the tab menus, and the combined unsaved-changes prompt
 * a window close or 종료 shows when several tabs have changes.
 *
 * Lazy (`import("../tabs/flow")`): nothing here is on the start-up path.
 *
 * **What a tab keeps** while in the background (`TabSnapshot`): the document (`DocInfo`, outline,
 * the saved page count), the view (zoom, layout, rotation, page, 뒤로 / 앞으로, 분할 보기 with both
 * panes — like `focusPane`, the whole `PaneView` is moved, never copied field by field), the
 * position under the top of the viewport, the 페이지 selection, the annotation selection and
 * filter, the mode and tool, and the 검색 results. The engine document stays open; the annotation
 * lists, text layers and tiles are re-read from the engine's caches on the way back.
 *
 * **What a switch does first**: an open 편집 session is committed and pending 영역 표시 marks ask
 * (the same `editLeaveGuard` as leaving 편집 mode or closing the document; 취소 stays), a coalesced
 * annotation patch is flushed, and a running search is stopped (it runs again on return).
 */
import * as api from "../ipc/api";
import { useTabStore, type Tab } from "../store/tabStore";
import { useDocStore } from "../store/docStore";
import {
  PANE_FIELDS, currentViewTarget, useViewStore, type PaneView, type ViewState, type ViewTarget,
} from "../store/viewStore";
import { usePagesStore, type PagesState } from "../store/pagesStore";
import { useAnnotStore, type AnnotState } from "../store/annotStore";
import { useAppStore, type Mode, type ToolId } from "../store/appStore";
import { useSearchStore, type SearchState } from "../viewer/search/SearchController";
import { useCompareStore } from "../compare/state";
import { usePrintStore } from "../print/printStore";
import { toolController } from "../tools/ToolController";
import { editLeaveGuard } from "../tools/commands";
import { whenEditsSettled } from "../annot/dragGate";
import { clearTextSelection } from "../viewer/viewerCommands";
import { autosave, markRecovered, recoveredEntry } from "../app/autosave";
import { openContextMenu, type MenuEntry } from "../app/contextMenuStore";
import { askChoice } from "../dialogs/dialogState";
import { useMock } from "../ipc/env";
import type { DocInfo, OutlineNode, RecoveryEntry } from "../ipc/types";

type ViewSlice = PaneView & Pick<ViewState, "layout" | "split" | "focusedPane" | "parked">;
type SearchSlice = Pick<
  SearchState,
  "query" | "matchCase" | "wholeWord" | "hits" | "byPage" | "total" | "scanned" | "current" | "hitsQuery" | "error"
> & { rerun: boolean };

export interface TabSnapshot {
  doc: { info: DocInfo; outline: OutlineNode[]; savedPageCount: number | null };
  view: ViewSlice;
  /** the page under the top of the focused pane, and the PDF y of that edge */
  at: ViewTarget;
  pages: Pick<PagesState, "selected" | "lastAnchor" | "focus">;
  annot: Pick<AnnotState, "selected" | "filter">;
  app: { mode: Mode; tool: ToolId };
  search: SearchSlice | null;
}

/** ⇧⌘T remembers this many closed tabs. */
const CLOSED_MAX = 20;

const flows = () => import("../dialogs/flows");

// ---------------------------------------------------------------------------
// Park / restore
// ---------------------------------------------------------------------------

/** The active document's per-document UI state, or `null` with no document. */
export function capture(): TabSnapshot | null {
  const d = useDocStore.getState();
  if (!d.info) return null;
  const v = useViewStore.getState();
  const view = { layout: v.layout, split: v.split, focusedPane: v.focusedPane, parked: v.parked } as ViewSlice;
  for (const key of PANE_FIELDS) Object.assign(view, { [key]: v[key] });
  const pages = usePagesStore.getState();
  const annot = useAnnotStore.getState();
  const app = useAppStore.getState();
  const s = useSearchStore.getState();
  const search: SearchSlice | null =
    s.docId === d.info.docId && s.query
      ? {
          query: s.query, matchCase: s.matchCase, wholeWord: s.wholeWord, hits: s.hits, byPage: s.byPage,
          total: s.total, scanned: s.scanned, current: s.current, hitsQuery: s.hitsQuery, error: s.error,
          rerun: s.running,
        }
      : null;
  return {
    doc: { info: d.info, outline: d.outline, savedPageCount: d.savedPageCount },
    view,
    at: currentViewTarget(),
    pages: { selected: pages.selected, lastAnchor: pages.lastAnchor, focus: pages.focus },
    annot: { selected: annot.selected, filter: annot.filter },
    app: { mode: app.mode, tool: app.momentaryFrom ?? app.tool },
    search,
  };
}

/** Park the active tab: its state goes into `parked`, the search pass stops. */
function park(): void {
  const st = useTabStore.getState();
  const snap = capture();
  if (st.activeId === null || !snap) return;
  useTabStore.setState({ parked: { ...st.parked, [st.activeId]: snap } });
  // a pass still streaming must not write another document's hits into the store
  useSearchStore.getState().clear();
  clearTextSelection();
}

function omit<T>(record: Record<number, T>, id: number): Record<number, T> {
  const next = { ...record };
  delete next[id];
  return next;
}

/** Write a parked tab back into the stores (its tab is already the active one). */
function restore(snap: TabSnapshot): void {
  const { info } = snap.doc;
  // the view first: the scroller mounted for this document (keyed by docId) lands on the page
  const request = (pane: PaneView, at?: ViewTarget) => ({
    page: at?.page ?? pane.currentPage,
    nonce: (pane.scrollRequest?.nonce ?? 0) + 1,
    ...(at?.y !== undefined ? { yPt: at.y } : {}),
  });
  useViewStore.setState({
    ...snap.view,
    scrollRequest: request(snap.view, snap.at),
    parked: snap.view.parked ? { ...snap.view.parked, scrollRequest: request(snap.view.parked) } : null,
  });
  usePagesStore.setState({ ...snap.pages, dropAt: null, pendingOrder: null });
  // 읽기 while the document changes: `permissions.ts` checks the mode against each new document,
  // and the outgoing tab's mode may be one the incoming document forbids (or the other way round)
  if (useAppStore.getState().mode !== "read") useAppStore.setState({ mode: "read", momentaryFrom: null });
  useDocStore.setState((s) => ({
    docId: info.docId,
    info,
    outline: snap.doc.outline,
    status: "ready",
    error: null,
    changedPages: "all",
    changeNonce: s.changeNonce + 1,
  }));
  // after the document: its subscription resets the baseline for a document it has not seen
  useDocStore.setState({ savedPageCount: snap.doc.savedPageCount });
  // the annotation lists are re-read by `annot/sync` (docId changed); the selection comes back
  useAnnotStore.setState({ selected: snap.annot.selected, filter: snap.annot.filter, editing: null, thread: null });
  useAppStore.setState({ mode: snap.app.mode, tool: snap.app.tool, momentaryFrom: null });
  toolController.arm(snap.app.tool);
  const search = useSearchStore.getState();
  if (snap.search) {
    const { rerun, ...rest } = snap.search;
    useSearchStore.setState({ ...rest, docId: info.docId, running: false, jobId: null });
    if (rerun) void search.run(info.docId, rest.query, snap.view.currentPage);
  }
  // whatever the engine did while the tab was in the background (a save, an autosave, a batch job)
  void useDocStore.getState().refresh();
}

/** A tab opened for a new document starts clean: one pane, no search, nothing selected. */
export function freshTab(): void {
  useViewStore.getState().closeSplit();
  useViewStore.setState({ rotation: 0 });
  useSearchStore.getState().clear();
  useAnnotStore.getState().select([]);
  clearTextSelection();
}

// ---------------------------------------------------------------------------
// Switching
// ---------------------------------------------------------------------------

let queue: Promise<unknown> = Promise.resolve();
/** Tab actions run one at a time: a switch never interleaves with a close or another switch. */
function serial<T>(fn: () => Promise<T>): Promise<T> {
  const run = queue.then(fn, fn);
  queue = run.catch(() => undefined);
  return run;
}

async function switchTo(id: number): Promise<boolean> {
  const st = useTabStore.getState();
  if (st.activeId === id) return true;
  if (!st.tabs.some((t) => t.id === id) || !st.parked[id]) return false;
  // 문서 비교 covers the window and holds the active document as A
  if (useCompareStore.getState().session) return false;
  if (useDocStore.getState().info) {
    const pending = editLeaveGuard();
    if (pending && !(await pending)) return false;
    await whenEditsSettled();
    park();
  }
  const snap = useTabStore.getState().parked[id];
  if (!snap) return false;
  useTabStore.setState((s) => ({ activeId: id, parked: omit(s.parked, id) }));
  restore(snap);
  return true;
}

/** Bring tab `id` to the front. `false`: the user stayed (취소 on pending marks), or no such tab. */
export function activateTab(id: number): Promise<boolean> {
  return serial(() => switchTo(id));
}

/** ⌃Tab / ⌃⇧Tab, ⌥⌘→ / ⌥⌘← — wraps around. */
export function cycleTab(step: 1 | -1): Promise<boolean> {
  const { tabs, activeId } = useTabStore.getState();
  if (tabs.length < 2) return Promise.resolve(false);
  const at = tabs.findIndex((t) => t.id === activeId);
  return activateTab(tabs[(at + step + tabs.length) % tabs.length].id);
}

/** The tab showing `docId` (a `focus-document` from another window, H8). */
export function activateDocument(docId: string): Promise<boolean> {
  const tab = useTabStore.getState().tabs.find((t) => t.docId === docId);
  return tab ? activateTab(tab.id) : Promise.resolve(false);
}

/**
 * H8 within the window: the tab that already has the file at `path` comes to the front, and its
 * (fresh) document is the answer; `null` when no tab has it.
 */
export async function focusOwnTab(path: string): Promise<DocInfo | null> {
  const tab = useTabStore.getState().tabs.find((t) => t.path === path);
  if (!tab || !(await activateTab(tab.id))) return null;
  return useDocStore.getState().info;
}

/**
 * Before a document opens into a new tab: the active one is parked (after its 편집 guard), so the
 * next document `docStore` gets becomes a tab of its own. Resolves with the rollback for a failed or
 * cancelled open — the parked tab is active again, untouched — or `null` when the guard said 취소.
 */
export async function beginNewTab(): Promise<(() => void) | null> {
  const id = useTabStore.getState().activeId;
  if (id === null || !useDocStore.getState().info) return () => undefined;
  const pending = editLeaveGuard();
  if (pending && !(await pending)) return null;
  await whenEditsSettled();
  park();
  useTabStore.setState({ activeId: null });
  return () => {
    const s = useTabStore.getState();
    if (s.activeId !== null || !s.parked[id] || useDocStore.getState().info?.docId !== s.parked[id].doc.info.docId) return;
    useTabStore.setState({ activeId: id, parked: omit(s.parked, id) });
  };
}

// ---------------------------------------------------------------------------
// Closing
// ---------------------------------------------------------------------------

function rememberClosed(tab: Tab | undefined): void {
  if (!tab?.path || recoveredEntry(tab.docId)) return;
  const path = tab.path;
  useTabStore.setState((s) => ({ closed: [...s.closed.filter((p) => p !== path), path].slice(-CLOSED_MAX) }));
}

/**
 * Close the active tab: pending marks, then 저장 / 저장 안 함 / 취소, then the reading position goes
 * to 최근 항목, the recovery copy goes, and the neighbour (right, else left) comes to the front — or
 * the window shows the welcome screen when it was the last tab.
 */
async function closeActive(opts: { remember?: boolean } = {}): Promise<boolean> {
  const f = await flows();
  if (!(await f.confirmLeaveDocument())) return false;
  const st = useTabStore.getState();
  const tab = st.tabs.find((t) => t.id === st.activeId);
  const info = useDocStore.getState().info;
  if (info) {
    await f.touchRecent(info).catch(() => undefined);
    // 저장 or 저장 안 함 (or nothing to save): a clean close, so the recovery copy goes
    await autosave.clear(info.docId);
  }
  if (opts.remember !== false) rememberClosed(tab);
  const index = tab ? st.tabs.indexOf(tab) : -1;
  const rest = st.tabs.filter((t) => t !== tab);
  const next = rest[index] ?? rest[index - 1];
  const snap = next && st.parked[next.id];
  if (!info || !next || !snap) {
    usePagesStore.getState().reset();
    await useDocStore.getState().close();
    return true;
  }
  useSearchStore.getState().clear();
  clearTextSelection();
  useTabStore.setState((s) => ({ tabs: rest, activeId: next.id, parked: omit(s.parked, next.id) }));
  restore(snap);
  await api.closeDocument({ docId: info.docId }).catch(() => undefined);
  return true;
}

/** A clean background tab closes without being shown. */
async function closeBackground(tab: Tab): Promise<void> {
  const snap = useTabStore.getState().parked[tab.id];
  if (snap) {
    const f = await flows();
    await f.touchRecent(snap.doc.info, { page: snap.view.currentPage, zoomPercent: snap.view.zoomPercent, layout: snap.view.layout }).catch(() => undefined);
  }
  await autosave.clear(tab.docId);
  rememberClosed(tab);
  useTabStore.setState((s) => ({ tabs: s.tabs.filter((t) => t.id !== tab.id), parked: omit(s.parked, tab.id) }));
  await api.closeDocument({ docId: tab.docId }).catch(() => undefined);
}

/**
 * × on a tab, a middle click, ⌘W. A background tab with changes is brought to the front first, so
 * the 저장 / 저장 안 함 / 취소 prompt is about the document on screen.
 */
export function closeTab(id: number): Promise<boolean> {
  return serial(async () => {
    const st = useTabStore.getState();
    const tab = st.tabs.find((t) => t.id === id);
    if (!tab || printing(tab.docId)) return false;
    if (id !== st.activeId) {
      if (!tab.dirty) {
        await closeBackground(tab);
        return true;
      }
      if (!(await switchTo(id))) return false;
    }
    return closeActive();
  });
}

/** The active document, closed like a tab (파일 › 닫기 while the window has several). */
export function closeActiveTab(): Promise<boolean> {
  return serial(async () => {
    const docId = useDocStore.getState().info?.docId;
    return docId && printing(docId) ? false : closeActive();
  });
}

/**
 * A print job still mounts page images of `docId` (a chunked job waits between chunks with the
 * window usable): its tab stays open until the job ends, or the images come out blank.
 */
function printing(docId: string): boolean {
  return usePrintStore.getState().job?.docId === docId;
}

/** 다른 탭 닫기: every other tab, stopping at the first 취소. */
export async function closeOtherTabs(id: number): Promise<void> {
  if (!(await activateTab(id))) return;
  for (const tab of useTabStore.getState().tabs.filter((t) => t.id !== id)) {
    if (!(await closeTab(tab.id))) return;
    if (useTabStore.getState().activeId !== id && !(await activateTab(id))) return;
  }
}

/** ⌘W / Ctrl+W: the tab; with one tab (or none) left, the window (its close runs the usual gate). */
export async function closeTabOrWindow(): Promise<void> {
  if (useTabStore.getState().tabs.length > 1) {
    const id = useTabStore.getState().activeId;
    if (id !== null) await closeTab(id);
    return;
  }
  if (useMock()) {
    // no native window in the browser build / tests: close the document instead
    await (await flows()).closeDocumentFlow();
    return;
  }
  const { getCurrentWindow } = await import("@tauri-apps/api/window");
  await getCurrentWindow().close();
}

/** ⌘⇧T: the last closed tab, from its file (a file open elsewhere is focused there, H8). */
export async function reopenClosedTab(): Promise<boolean> {
  const closed = useTabStore.getState().closed;
  const path = closed[closed.length - 1];
  if (!path) return false;
  useTabStore.setState({ closed: closed.slice(0, -1) });
  return (await (await flows()).openPath(path, { target: "tab" })) !== null;
}

/**
 * 새 창으로 이동 (the tab menu — the strip does not drag tabs out of the window): the file closes
 * here (저장 / 저장 안 함 / 취소 first when it has changes) and opens in a new window.
 */
export async function moveTabToNewWindow(id: number): Promise<boolean> {
  const tab = useTabStore.getState().tabs.find((t) => t.id === id);
  if (!tab?.path) return false;
  const path = tab.path;
  const ok = await serial(async () => (await switchTo(id)) && (await closeActive({ remember: false })));
  if (!ok) return false;
  await api.openInNewWindow({ path }).catch(() => undefined);
  return true;
}

// ---------------------------------------------------------------------------
// Window close / 종료
// ---------------------------------------------------------------------------

/** Every tab with unsaved changes, in strip order (the active one from `docStore`). */
export function dirtyTabs(): Tab[] {
  const { tabs, activeId } = useTabStore.getState();
  const active = useDocStore.getState().info;
  return tabs.filter((t) => (t.id === activeId ? !!active?.dirty : t.dirty));
}

export type UnsavedTabsAnswer = "saveAll" | "dontSave" | "cancel";

/**
 * The window is closing (or the app quitting) with changes in more than one tab — or in a tab that
 * is not on screen: one prompt lists them all. 모두 저장 shows and saves each in turn (a Save As
 * panel for a new document, the changed-on-disk question, a 취소 anywhere stops the close);
 * 저장 안 함 lets them go (a document from a 복구 copy still asks whether to keep the copy).
 * `true`: close. Pending 편집 marks on the active tab were asked about before this.
 */
export async function confirmUnsavedTabs(): Promise<boolean> {
  const f = await flows();
  if (await whenEditsSettled()) await useDocStore.getState().refresh().catch(() => undefined);
  const dirty = dirtyTabs();
  if (dirty.length === 0) return true;
  const activeId = useTabStore.getState().activeId;
  if (dirty.length === 1 && dirty[0].id === activeId) return f.confirmUnsaved();
  const answer = await askChoice<UnsavedTabsAnswer>({
    titleKey: "tabs.unsaved.title",
    bodyKey: "tabs.unsaved.body",
    bodyParams: { count: dirty.length },
    hintKey: "tabs.unsaved.list",
    hintParams: { names: dirty.map((t) => t.name).join(", ") },
    options: [
      { value: "saveAll", labelKey: "tabs.unsaved.saveAll", primary: true },
      { value: "dontSave", labelKey: "common.dontSave" },
    ],
    cancel: { value: "cancel", labelKey: "common.cancel" },
  });
  if (answer === "cancel") return false;
  for (const tab of dirty) {
    if (answer === "saveAll") {
      if (!(await activateTab(tab.id)) || !(await f.saveFlow())) return false;
    } else {
      const info = tab.id === useTabStore.getState().activeId ? useDocStore.getState().info : useTabStore.getState().parked[tab.id]?.doc.info;
      if (info) await f.askKeepRecovery(info);
    }
  }
  return true;
}

/**
 * The window is going (after its gate): every tab's reading position goes to 최근 항목, its
 * recovery copy goes (a clean close), and the engine lets go of its document — tabs never outlive
 * their window.
 */
export async function releaseWindowTabs(): Promise<void> {
  const f = await flows();
  const { tabs, activeId, parked } = useTabStore.getState();
  const active = useDocStore.getState().info;
  if (active) {
    await f.touchRecent(active).catch(() => undefined);
    await autosave.clear(active.docId);
  }
  for (const tab of tabs) {
    if (tab.id === activeId || tab.docId === active?.docId) continue;
    const snap = parked[tab.id];
    if (snap) {
      const at = { page: snap.view.currentPage, zoomPercent: snap.view.zoomPercent, layout: snap.view.layout };
      await f.touchRecent(snap.doc.info, at).catch(() => undefined);
    }
    await autosave.clear(tab.docId);
    await api.closeDocument({ docId: tab.docId }).catch(() => undefined);
  }
  // the document on screen last: the window is about to go, nothing renders it any more
  if (active) await api.closeDocument({ docId: active.docId }).catch(() => undefined);
}

/**
 * H4 for background tabs: a panicking engine command closed their documents. Each reopens in its
 * tab (parked state kept) — from its newest 자동 저장 copy when it had changes and one exists (marked,
 * so 저장 asks for a location), else from its file; a tab with neither (a never-saved document, a
 * password) goes.
 */
export async function recoverBackgroundTabs(docIds: string[]): Promise<void> {
  const { tabs, activeId, parked } = useTabStore.getState();
  for (const tab of tabs) {
    if (tab.id === activeId || !docIds.includes(tab.docId)) continue;
    const snap = parked[tab.id];
    const info = snap?.doc.info;
    const copies = info?.dirty ? await api.listRecovery().catch(() => [] as RecoveryEntry[]) : [];
    const copy =
      copies.find((c) => (info?.path ? c.originalPath === info.path : c.originalPath === null && c.name === info?.name)) ??
      recoveredEntry(tab.docId);
    const path = copy?.recoveryPath ?? info?.path;
    const fresh = path
      ? await api.openDocument(copy ? { path, displayName: copy.name } : { path }).catch(() => null)
      : null;
    if (!fresh || !snap) {
      useTabStore.setState((s) => ({ tabs: s.tabs.filter((t) => t.id !== tab.id), parked: omit(s.parked, tab.id) }));
      continue;
    }
    if (copy) markRecovered(fresh.docId, copy);
    useTabStore.setState((s) => ({
      tabs: s.tabs.map((t) => (t.id === tab.id ? { ...t, docId: fresh.docId, name: fresh.name, path: fresh.path, dirty: fresh.dirty } : t)),
      parked: { ...s.parked, [tab.id]: { ...snap, doc: { ...snap.doc, info: fresh, outline: [] } } },
    }));
  }
}

// ---------------------------------------------------------------------------
// Menus
// ---------------------------------------------------------------------------

/** Right-click on a tab. */
export function openTabMenu(id: number, x: number, y: number): void {
  const { tabs } = useTabStore.getState();
  const tab = tabs.find((t) => t.id === id);
  if (!tab) return;
  const windows = useAppStore.getState().os === "windows";
  const path = tab.path;
  const items: MenuEntry[] = [
    { id: "close", labelKey: "tabs.close", onSelect: () => void closeTab(id) },
    { id: "closeOthers", labelKey: "tabs.closeOthers", disabled: tabs.length < 2, onSelect: () => void closeOtherTabs(id) },
    { id: "newWindow", labelKey: "tabs.moveToNewWindow", disabled: !path, onSelect: () => void moveTabToNewWindow(id) },
    { id: "sep", separator: true },
    {
      id: "copyPath",
      labelKey: "tabs.copyPath",
      disabled: !path,
      onSelect: () => {
        if (path && typeof navigator !== "undefined" && navigator.clipboard) void navigator.clipboard.writeText(path).catch(() => undefined);
      },
    },
    {
      id: "reveal",
      labelKey: windows ? "menu.file.revealInExplorer" : "menu.file.revealInFinder",
      disabled: !path,
      onSelect: () => {
        if (path) void api.revealInFileManager({ path }).catch(() => undefined);
      },
    },
  ];
  openContextMenu({ x, y, labelKey: "tabs.label", items });
}

/** The strip's ⌄ when the tabs do not fit: every tab, by name. */
export function openTabList(x: number, y: number): void {
  const { tabs, activeId } = useTabStore.getState();
  openContextMenu({
    x,
    y,
    labelKey: "tabs.all",
    items: tabs.map((t) => ({
      id: `tab-${t.id}`,
      labelKey: "tabs.label",
      label: `${t.id === activeId ? "✓ " : ""}${t.dirty ? "• " : ""}${t.name}`,
      onSelect: () => void activateTab(t.id),
    })),
  });
}
