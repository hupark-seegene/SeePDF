import { Suspense, lazy, useEffect, useMemo } from "react";
import { TitleBar } from "./app/TitleBar";
import { ToolStrip } from "./app/ToolStrip";
import { SidebarFrame } from "./app/SidebarFrame";
import { CanvasStub } from "./app/CanvasStub";
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
import { useOcrDialogOpen } from "./ocr/dialogState";
import { useToastStore } from "./app/toastStore";
import { useContextMenuStore } from "./app/contextMenuStore";
import { useT } from "./i18n/useT";

// Everything below the fold is code-split: the welcome screen, the page organizer, the dialog host
// (which lazily carries the OCR sheet), the toast layer and the context menu. The entry chunk is
// gated at 120 kB gz by `scripts/check-bundle-size.mjs`.
const Welcome = lazy(() => import("./welcome"));
const Organizer = lazy(() => import("./organize"));
const DialogHost = lazy(() => import("./dialogs/DialogHost"));
const Toasts = lazy(() => import("./app/Toasts"));
const ContextMenu = lazy(() => import("./app/ContextMenu"));

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
  const bootstrap = useAppStore((s) => s.bootstrap);
  const refreshRecents = useAppStore((s) => s.refreshRecents);
  const releaseMomentary = useAppStore((s) => s.releaseMomentary);
  const info = useDocStore((s) => s.info);
  const applyDocChanged = useDocStore((s) => s.applyDocChanged);
  const dialogOpen = useDialogStore((s) => s.stack.length > 0);
  const ocrOpen = useOcrDialogOpen();
  const hasToasts = useToastStore((s) => s.toasts.length > 0);
  const menuOpen = useContextMenuStore((s) => s.menu !== null);

  // 1. settings + recents + theme + locale, then drain anything the OS handed us before mount
  useEffect(() => {
    void bootstrap().then(async () => {
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
              await win.destroy();
            }
            return;
          }
          if (current) await flows.touchRecent(current).catch(() => undefined);
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
        if (hit) openPageContextMenu(hit.page, hit.source, e.clientX, e.clientY);
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
    if (info) {
      // 페이지 mode owns the arrow keys, ⌫ and the tool letters: the grid is not the canvas
      list.push("doc", mode === "pages" ? "pages" : "canvas");
    }
    return list;
  }, [info, mode]);

  useKeymap({
    os,
    contexts,
    onCommand: (id, binding, e) => run(id, { momentary: binding.momentary && !e.repeat }),
    onRelease: () => releaseMomentary(),
  });

  const organizing = mode === "pages";

  return (
    <div className="app-shell" data-ready={ready || undefined}>
      <TitleBar run={run} />
      <div className="app-body">
        {info && sidebarOpen && !organizing && <SidebarFrame />}
        <main className="app-main">
          {info ? (
            organizing ? (
              <Suspense fallback={null}>
                <Organizer />
              </Suspense>
            ) : (
              <>
                <ToolStrip />
                <CanvasStub />
              </>
            )
          ) : (
            <Suspense fallback={null}>
              <Welcome />
            </Suspense>
          )}
        </main>
        {info && inspectorOpen && mode !== "read" && !organizing && <Inspector />}
      </div>
      <StatusBar />
      {(dialogOpen || ocrOpen) && (
        <Suspense fallback={null}>
          <DialogHost />
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
  );
}
