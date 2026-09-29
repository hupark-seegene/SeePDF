import { useState } from "react";
import { ChevronDown, Download, MoreHorizontal, PanelLeft, Redo2, Search, Undo2 } from "lucide-react";
import { IconButton } from "./IconButton";
import { ModeSwitcher } from "./ModeSwitcher";
import { useT } from "../i18n/useT";
import { useAppStore } from "../store/appStore";
import { useDocStore } from "../store/docStore";
import { shortcutFor } from "../keys/keymap";
import { historyTitle } from "./historyLabel";
import { DocBadges } from "./DocBadges";
import { TabStrip } from "./TabStrip";
import { useTabStore } from "../store/tabStore";
import { installSignatureGate } from "./signatureGate";
import type { CommandId } from "./useCommands";

// v0.3 pkg3 (S1): the first edit of a signed document asks once (see `signatureGate.ts`).
installSignatureGate();

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
  // v0.3 DR1: with two or more documents the tab strip takes the title's place
  const tabbed = useTabStore((s) => s.tabs.length > 1);
  const [menuOpen, setMenuOpen] = useState(false);
  const sc = (id: string) => shortcutFor(id, os);
  // `useT` re-renders on a locale change, so the labels below follow it.
  const undoTitle = historyTitle("undo", info?.canUndo ? info.undoLabel : null);
  const redoTitle = historyTitle("redo", info?.canRedo ? info.redoLabel : null);
  // `doc-changed` carries canUndo/canRedo but not the labels (they are dropped as stale), so
  // fetch them once when the pointer reaches ↶ / ↷ — cheaper than a get_document per edit.
  const labelsMissing = !!info && ((info.canUndo && !info.undoLabel) || (info.canRedo && !info.redoLabel));

  return (
    <header className="titlebar" data-tauri-drag-region>
      <div
        className="titlebar-left"
        data-tauri-drag-region="false"
        onPointerEnter={labelsMissing ? () => void useDocStore.getState().refresh() : undefined}
      >
        <IconButton
          icon={PanelLeft}
          label={t("sidebar.toggle")}
          shortcut={sc("view.sidebar")}
          active={sidebarOpen}
          onClick={() => run("view.sidebar")}
        />
        <IconButton
          icon={Undo2}
          label={undoTitle}
          shortcut={sc("edit.undo")}
          disabled={!info?.canUndo}
          onClick={() => run("edit.undo")}
        />
        <IconButton
          icon={Redo2}
          label={redoTitle}
          shortcut={sc("edit.redo")}
          disabled={!info?.canRedo}
          onClick={() => run("edit.redo")}
        />
      </div>

      <div className="titlebar-title" data-tauri-drag-region>
        {tabbed && <TabStrip />}
        {info && !tabbed && (
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
        {/* v0.3 pkg3: 서명됨 (n) · 제한됨 */}
        <DocBadges />
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
