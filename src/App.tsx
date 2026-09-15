import { useEffect, useMemo } from "react";
import { TitleBar } from "./app/TitleBar";
import { ToolStrip } from "./app/ToolStrip";
import { SidebarFrame } from "./app/SidebarFrame";
import { CanvasStub } from "./app/CanvasStub";
import { Inspector } from "./app/Inspector";
import { StatusBar } from "./app/StatusBar";
import { WelcomeStub } from "./app/WelcomeStub";
import { useCommands } from "./app/useCommands";
import { useKeymap } from "./keys/useKeymap";
import { MENU_IDS, type KeyContext } from "./keys/keymap";
import { onDocChanged, onFileDrop, onMenuCommand, onOpenFile, onRecentsChanged } from "./ipc/events";
import * as api from "./ipc/api";
import { useAppStore } from "./store/appStore";
import { useDocStore } from "./store/docStore";
import { useT } from "./i18n/useT";

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
  const openDoc = useDocStore((s) => s.open);
  const applyDocChanged = useDocStore((s) => s.applyDocChanged);

  // 1. settings + recents + theme + locale, then drain anything the OS handed us before mount
  useEffect(() => {
    void bootstrap().then(async () => {
      const pending = await api.takePendingOpens().catch(() => []);
      if (pending.length) await openDoc(pending[0].path);
    });
  }, [bootstrap, openDoc]);

  // 2. app-wide broadcasts
  useEffect(() => onDocChanged(applyDocChanged), [applyDocChanged]);
  useEffect(() => onOpenFile((e) => void openDoc(e.path)), [openDoc]);
  useEffect(() => onRecentsChanged(() => void refreshRecents()), [refreshRecents]);
  useEffect(() => onFileDrop((e) => e.paths[0] && void openDoc(e.paths[0])), [openDoc]);
  useEffect(() => onMenuCommand((id) => run(id), MENU_IDS), [run]);

  // 3. window title follows the document and its dirty state (UI_SPEC §15.1)
  useEffect(() => {
    document.title = info ? t("app.window.document", { name: `${info.dirty ? "• " : ""}${info.name}` }) : t("app.name");
  }, [info, t]);

  const contexts = useMemo<KeyContext[]>(() => {
    const list: KeyContext[] = ["always"];
    if (info) {
      list.push("doc", "canvas");
      if (mode === "pages") list.push("pages");
    }
    return list;
  }, [info, mode]);

  useKeymap({
    os,
    contexts,
    onCommand: (id, binding, e) => run(id, { momentary: binding.momentary && !e.repeat }),
    onRelease: () => releaseMomentary(),
  });

  return (
    <div className="app-shell" data-ready={ready || undefined}>
      <TitleBar run={run} />
      <div className="app-body">
        {info && sidebarOpen && <SidebarFrame />}
        <main className="app-main">
          {info ? (
            <>
              <ToolStrip />
              <CanvasStub />
            </>
          ) : (
            <WelcomeStub />
          )}
        </main>
        {info && inspectorOpen && mode !== "read" && <Inspector />}
      </div>
      <StatusBar />
    </div>
  );
}
