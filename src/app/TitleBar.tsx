import { useState } from "react";
import { ChevronDown, Download, MoreHorizontal, PanelLeft, Redo2, Search, Undo2 } from "lucide-react";
import { IconButton } from "./IconButton";
import { ModeSwitcher } from "./ModeSwitcher";
import { useT } from "../i18n/useT";
import { useAppStore } from "../store/appStore";
import { useDocStore } from "../store/docStore";
import { shortcutFor } from "../keys/keymap";
import type { CommandId } from "./useCommands";

/**
 * The 44 px title-bar toolbar (UI_SPEC §2). The whole strip is `data-tauri-drag-region`; every
 * interactive child opts out with `data-tauri-drag-region="false"` so it stays clickable, and the
 * left gutter is 78 px on macOS (traffic lights) / 8 px on Windows.
 */
export function TitleBar({ run }: { run: (id: CommandId) => void }) {
  const t = useT();
  const os = useAppStore((s) => s.os);
  const sidebarOpen = useAppStore((s) => s.sidebarOpen);
  const info = useDocStore((s) => s.info);
  const [menuOpen, setMenuOpen] = useState(false);
  const sc = (id: string) => shortcutFor(id, os);

  return (
    <header className="titlebar" data-tauri-drag-region>
      <div className="titlebar-left" data-tauri-drag-region="false">
        <IconButton
          icon={PanelLeft}
          label={t("sidebar.toggle")}
          shortcut={sc("view.sidebar")}
          active={sidebarOpen}
          onClick={() => run("view.sidebar")}
        />
        <IconButton
          icon={Undo2}
          label={t("menu.edit.undo")}
          shortcut={sc("edit.undo")}
          disabled={!info?.canUndo}
          onClick={() => run("edit.undo")}
        />
        <IconButton
          icon={Redo2}
          label={t("menu.edit.redo")}
          shortcut={sc("edit.redo")}
          disabled={!info?.canRedo}
          onClick={() => run("edit.redo")}
        />
      </div>

      <div className="titlebar-title" data-tauri-drag-region>
        {info && (
          <div className="doc-title-wrap" data-tauri-drag-region="false">
            <button
              type="button"
              className="doc-title"
              aria-haspopup="menu"
              aria-expanded={menuOpen}
              onClick={() => setMenuOpen((v) => !v)}
            >
              {info.dirty && <span className="dirty-dot" aria-label={t("app.edited")} />}
              <span className="doc-title-text">{info.name}</span>
              <ChevronDown size={14} strokeWidth={1.75} aria-hidden />
            </button>
            {menuOpen && (
              <div className="menu-pop" role="menu" onMouseLeave={() => setMenuOpen(false)}>
                <button type="button" role="menuitem" onClick={() => { setMenuOpen(false); run("file.copyPath"); }}>
                  {t("dialog.docInfo.location")}
                </button>
                <button type="button" role="menuitem" onClick={() => { setMenuOpen(false); run("file.reveal"); }}>
                  {t(os === "windows" ? "menu.file.revealInExplorer" : "menu.file.revealInFinder")}
                </button>
                <button type="button" role="menuitem" onClick={() => { setMenuOpen(false); run("file.docInfo"); }}>
                  {t("menu.file.docInfo")}
                </button>
              </div>
            )}
          </div>
        )}
      </div>

      <div className="titlebar-centre" data-tauri-drag-region="false">
        <ModeSwitcher run={run} />
      </div>

      <div className="titlebar-right" data-tauri-drag-region="false">
        <IconButton icon={Search} label={t("menu.edit.find")} shortcut={sc("edit.find")} onClick={() => run("edit.find")} />
        <IconButton icon={Download} label={t("export.title")} shortcut={sc("file.export")} onClick={() => run("file.export")} />
        <IconButton icon={MoreHorizontal} label={t("common.more")} onClick={() => run("app.overflow")} />
      </div>
    </header>
  );
}
