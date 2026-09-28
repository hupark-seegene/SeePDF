import { Suspense, lazy, useEffect, useMemo } from "react";
import { TitleBar } from "./app/TitleBar";
import { ToolStrip } from "./app/ToolStrip";
// The sidebar panels and the viewer are lazy (below), but their stylesheets stay in the entry CSS,
// in the place of the cascade they always had (before tokens / base / shell, see main.tsx): the
// 여러 파일에서 검색 dialog reuses the 검색 panel's rows with no document open, and moving them
// after shell.css would change which of two equal-specificity rules wins.
import "./sidebar/sidebar.css";
import "./viewer/viewer.css";
import { Inspector } from "./app/Inspector";
import { StatusBar } from "./app/StatusBar";
import { useCommands } from "./app/useCommands";
import { useKeymap } from "./keys/useKeymap";
import { MENU_IDS, type KeyContext } from "./keys/keymap";
import { onDocChanged, onFileDrop, onMenuCommand, onOpenFile, onRecentsChanged } from "./ipc/events";
import * as api from "./ipc/api";
import { useMock } from "./ipc/env";
import { useAppStore } from "./store/appStore";
import { useDocStore } from "./store/docStore";
import { useDialogStore } from "./dialogs/dialogState";
import { useJobStore } from "./store/jobStore";
import { autosave, offerRecovery, useAutosave } from "./app/autosave";
import { useReadingModeKeys } from "./app/readingMode";
import { useCompareStore } from "./compare/state";
import { useOcrDialogOpen } from "./ocr/dialogState";
import { useToastStore } from "./app/toastStore";
import { useContextMenuStore } from "./app/contextMenuStore";
import { usePrintStore } from "./print/printStore";
import { useT } from "./i18n/useT";
import { useTtsStore } from "./tts/ttsStore";

// Everything below the fold is code-split: the welcome screen, the page organizer, the dialog host
// (which lazily carries the OCR sheet), the toast layer and the context menu. The entry chunk is
// gated at 120 kB gz by `scripts/check-bundle-size.mjs`.
const Welcome = lazy(() => import("./welcome"));
const Organizer = lazy(() => import("./organize"));
const DialogHost = lazy(() => import("./dialogs/DialogHost"));
const Toasts = lazy(() => import("./app/Toasts"));
const ContextMenu = lazy(() => import("./app/ContextMenu"));
// F-26: the print-only DOM. Mounted only while a print job is in flight, so its page images
// (one `<img>` per printed page at 150 DPI) exist for exactly as long as the print panel does.
const PrintRoot = lazy(() => import("./print/PrintRoot"));
// 문서 비교 (P1-6): the full-window side-by-side view, only while compare mode is on.
const CompareView = lazy(() => import("./compare/CompareView"));
// 읽어 주기 (P2): the floating 속도 / 정지 bar, only while the system voice speaks.
const TtsBar = lazy(() => import("./tts/TtsBar"));
// The document UI is only on screen with a document open: the viewer (scroller, tile manager,
// text layer, 분할 보기's panes, the layer slots) and the sidebar's 축소판 / 목차 / 검색 panels.
// Their chunks are fetched once the window is idle after start-up (`prefetchDocumentUi`), so a
// document opened later finds them loaded.
const loadCanvas = () => import("./app/CanvasStub");
const loadSidebar = () => import("./app/SidebarFrame");
const CanvasStub = lazy(() => loadCanvas().then((m) => ({ default: m.CanvasStub })));
const SidebarFrame = lazy(() => loadSidebar().then((m) => ({ default: m.SidebarFrame })));

/** After the first paint, when the window has nothing better to do: fetch the document UI. */
function prefetchDocumentUi(): void {
  const load = () => {
    void loadCanvas().catch(() => undefined);
    void loadSidebar().catch(() => undefined);
  };
  if (typeof window.requestIdleCallback === "function") window.requestIdleCallback(load, { timeout: 2000 });
  else setTimeout(load, 0);
}

/**
 * The window is closing: document B of 문서 비교 lives outside `docStore`, so release it here — the
 * engine does not close a gone window's documents on its own. A run still in the dialog is cancelled.
 */
async function leaveCompare(): Promise<void> {
  const running = useDialogStore.getState().stack.some((e) => e.name === "compare");
  if (!running && !useCompareStore.getState().session) return;
  const { useCompareRun, cancelCompare } = await import("./compare/flow");
  if (useCompareRun.getState().phase !== "idle") cancelCompare();
  await useCompareStore.getState().exit();
}

/**
 * The window is closing mid-batch (여러 파일 OCR, P1-7): the file being recognised is open in the
 * engine outside `docStore`, so stop the queue and let it close that file before the window goes.
 */
async function stopBatchOcr(): Promise<void> {
  if (!useJobStore.getState().jobs.some((j) => j.kind === "batchOcr" && j.state === "running")) return;
  const { cancelBatch } = await import("./ocr/batch/flow");
  await cancelBatch();
}

/** The window is closing while the system voice reads: stop it (the voice is app-wide, not per window). */
async function stopReading(): Promise<void> {
  if (!useTtsStore.getState().speaking) return;
  const { stopSpeaking } = await import("./tts/speak");
  await stopSpeaking();
}

/**
 * The window is closing mid-search (여러 파일에서 검색, P2): the file being read is open in the engine
 * outside `docStore`, so stop the run and let it close that file before the window goes.
 */
async function stopMultiSearch(): Promise<void> {
  if (!useJobStore.getState().jobs.some((j) => j.kind === "multiSearch" && j.state === "running")) return;
  const { cancelSearch } = await import("./multisearch/flow");
  await cancelSearch();
}

/** Cheap synchronous test so the default menu is only suppressed over a page (the import is async). */
function pageLike(target: EventTarget | null): boolean {
  const el = target as HTMLElement | null;
  return Boolean(el?.closest?.(".page-shell, .thumb")) && !el?.closest?.("[data-context-menu]");
}

export default function App() {
  const t = useT();
  const run = useCommands();
  const os = useAppStore((s) => s.os);
  const ready = useAppStore((s) => s.ready);
  const mode = useAppStore((s) => s.mode);
  const sidebarOpen = useAppStore((s) => s.sidebarOpen);
  const inspectorOpen = useAppStore((s) => s.inspectorOpen);
  const sidebarWidth = useAppStore((s) => s.sidebarWidth);
  const bootstrap = useAppStore((s) => s.bootstrap);
  const refreshRecents = useAppStore((s) => s.refreshRecents);
  const releaseMomentary = useAppStore((s) => s.releaseMomentary);
  const info = useDocStore((s) => s.info);
  const applyDocChanged = useDocStore((s) => s.applyDocChanged);
  const dialogOpen = useDialogStore((s) => s.stack.length > 0);
  const ocrOpen = useOcrDialogOpen();
  const hasToasts = useToastStore((s) => s.toasts.length > 0);
  const menuOpen = useContextMenuStore((s) => s.menu !== null);
  const printing = usePrintStore((s) => s.job !== null);
  const comparing = useCompareStore((s) => s.session !== null);
  const speaking = useTtsStore((s) => s.speaking);
  const readingMode = useAppStore((s) => s.readingMode);
  // 읽기 모드 (P1-12) only means something over a document
  const reading = readingMode && info !== null;

  // P1-8: recovery copies of the dirty document on a timer
  useAutosave();

  // P1-12: Esc leaves 읽기 모드 (and 전체 화면 with it); closing the document leaves it too
  useReadingModeKeys();
  useEffect(() => {
    if (!info && readingMode) useAppStore.getState().setReadingMode(false);
  }, [info, readingMode]);

  // 1. settings + recents + theme + locale, then drain anything the OS handed us before mount
  useEffect(() => {
    void bootstrap().then(async () => {
      prefetchDocumentUi();
      // 업데이트 확인 on launch (v0.2.0): silent, main window only, never in the way of recovery
      void import("./update/launch").then((m) => m.checkOnLaunch(useAppStore.getState().settings));
      await offerRecovery();
      const pending = await api.takePendingOpens().catch(() => []);
      if (pending.length) {
        const { openPaths } = await import("./dialogs/flows");
        await openPaths(pending.map((p) => p.path));
      }
    });
  }, [bootstrap]);

  // 2. app-wide broadcasts
  useEffect(() => onDocChanged(applyDocChanged), [applyDocChanged]);
  useEffect(() => onOpenFile((e) => void import("./dialogs/flows").then((m) => m.openPaths([e.path]))), []);
  useEffect(() => onRecentsChanged(() => void refreshRecents()), [refreshRecents]);
  useEffect(
    () => onFileDrop((e) => e.paths.length && void import("./dialogs/flows").then((m) => m.openPaths(e.paths))),
    [],
  );
  useEffect(() => onMenuCommand((id) => run(id), MENU_IDS), [run]);

  // 3. closing the window with unsaved changes asks 저장 / 저장 안 함 / 취소 (F-23)
  useEffect(() => {
    if (useMock()) return;
    let unlisten: (() => void) | undefined;
    let cancelled = false;
    void import("@tauri-apps/api/window")
      .then(({ getCurrentWindow }) => {
        const win = getCurrentWindow();
        return win.onCloseRequested(async (event) => {
          const current = useDocStore.getState().info;
          const flows = await import("./dialogs/flows");
          if (current?.dirty) {
            event.preventDefault();
            if (await flows.confirmUnsaved()) {
              if (current) await flows.touchRecent(useDocStore.getState().info ?? current).catch(() => undefined);
              // 저장 or 저장 안 함: a clean close, the recovery copy goes (P1-8)
              if (current) await autosave.clear(current.docId);
              await leaveCompare();
              await stopBatchOcr();
              await stopReading();
              await stopMultiSearch();
              await win.destroy();
            }
            return;
          }
          if (current) {
            await flows.touchRecent(current).catch(() => undefined);
            await autosave.clear(current.docId);
          }
          await leaveCompare();
          await stopBatchOcr();
          await stopReading();
          await stopMultiSearch();
        });
      })
      .then((fn) => {
        if (cancelled) fn();
        else unlisten = fn;
      })
      .catch(() => undefined);
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, []);

  // 4. the canvas and 축소판 context menus (UI_SPEC §12) — one listener, no edits in (c)'s files
  useEffect(() => {
    const onMenu = (e: MouseEvent) => {
      void import("./app/pageMenus").then(({ openPageContextMenu, pageFromEvent }) => {
        const hit = pageFromEvent(e.target);
        if (hit) openPageContextMenu(hit.page, hit.source, e.clientX, e.clientY, e.target);
      });
      if (pageLike(e.target)) e.preventDefault();
    };
    // capture: (d)'s pointer surface stops contextmenu propagation, and a layer that wants to own
    // the menu opts out with [data-context-menu] rather than by swallowing the event
    document.addEventListener("contextmenu", onMenu, true);
    return () => document.removeEventListener("contextmenu", onMenu, true);
  }, []);

  // 4b. dev only: the Stage 2 smoke drives the real app through `scripts/dev-bridge.mjs`.
  // `import.meta.env.DEV` is statically false in a build, so this whole branch — and the
  // `src/dev/**` chunk it imports — is dropped by the bundler.
  useEffect(() => {
    if (!import.meta.env.DEV) return;
    void import("./dev/testHook").then((m) => m.startDevBridge(run));
  }, [run]);

  // 5. window title follows the document and its dirty state (UI_SPEC §15.1)
  useEffect(() => {
    document.title = info ? t("app.window.document", { name: `${info.dirty ? "• " : ""}${info.name}` }) : t("app.name");
  }, [info, t]);

  const contexts = useMemo<KeyContext[]>(() => {
    const list: KeyContext[] = ["always"];
    // compare mode covers the document: none of its shortcuts may act on it unseen
    if (info && !comparing) {
      // 페이지 mode owns the arrow keys, ⌫ and the tool letters: the grid is not the canvas
      list.push("doc", mode === "pages" ? "pages" : "canvas");
    }
    return list;
  }, [info, mode, comparing]);

  useKeymap({
    os,
    contexts,
    // A modal dialog owns the keyboard: nothing may act on the document behind it (⌫ under
    // 페이지 추출 deleted the very pages being extracted). Esc is the dialog's own.
    enabled: !dialogOpen && !ocrOpen,
    onCommand: (id, binding, e) => run(id, { momentary: binding.momentary && !e.repeat }),
    onRelease: () => releaseMomentary(),
  });

  const organizing = mode === "pages";

  return (
    <>
    <div className="app-shell" data-ready={ready || undefined} data-reading={reading || undefined}>
      {!reading && <TitleBar run={run} />}
      <div className="app-body">
        {info && sidebarOpen && !organizing && !reading && (
          // the placeholder keeps the sidebar's width, so the canvas never lays out wider first
          <Suspense fallback={<aside className="sidebar" style={{ width: sidebarWidth }} />}>
            <SidebarFrame />
          </Suspense>
        )}
        <main className="app-main">
          {info ? (
            organizing ? (
              <Suspense fallback={null}>
                <Organizer />
              </Suspense>
            ) : (
              <>
                {!reading && <ToolStrip />}
                <Suspense fallback={<div className="viewer-panes" />}>
                  <CanvasStub />
                </Suspense>
              </>
            )
          ) : (
            <Suspense fallback={null}>
              <Welcome />
            </Suspense>
          )}
        </main>
        {info && inspectorOpen && mode !== "read" && !organizing && !reading && <Inspector />}
      </div>
      {!reading && <StatusBar />}
      {comparing && (
        <Suspense fallback={null}>
          <CompareView />
        </Suspense>
      )}
      {(dialogOpen || ocrOpen) && (
        <Suspense fallback={null}>
          <DialogHost />
        </Suspense>
      )}
      {speaking && (
        <Suspense fallback={null}>
          <TtsBar />
        </Suspense>
      )}
      {hasToasts && (
        <Suspense fallback={null}>
          <Toasts />
        </Suspense>
      )}
      {menuOpen && (
        <Suspense fallback={null}>
          <ContextMenu />
        </Suspense>
      )}
    </div>
    {printing && (
      <Suspense fallback={null}>
        <PrintRoot />
      </Suspense>
    )}
    </>
  );
}
