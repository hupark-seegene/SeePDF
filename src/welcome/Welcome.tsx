/**
 * Welcome / recent screen (UI_SPEC §11, F-01). Replaces Stage 0's `WelcomeStub`.
 *
 * Left rail: wordmark, version (`app_info`), 파일 열기…, a quiet 도움말 (단축키) and 설정. Right: recents as cards with their
 * `/recent-thumb` first page, filename, folder, relative date, page count and size; pinned first;
 * a filter field; right-click → 열기 / Finder에서 보기 / 목록에서 제거 / 즐겨찾기.
 * The whole window is a drop target — `App.tsx` feeds `onFileDrop` into `openPaths`, which asks
 * 각각 열기 / 하나로 합치기 for more than one file.
 */
import { useEffect, useMemo, useState } from "react";
import { CircleHelp, FileText, FolderOpen, Pin, Settings } from "lucide-react";
import { useT } from "../i18n/useT";
import { formatBytes, formatRelativeDay } from "../i18n";
import { useAppStore } from "../store/appStore";
import { useDocStore } from "../store/docStore";
import { recentThumbUrl } from "../ipc/protocol";
import { shortcutFor } from "../keys/keymap";
import { openContextMenu } from "../app/contextMenuStore";
import { openDialog } from "../dialogs/dialogState";
import * as api from "../ipc/api";
import { onFileDragState } from "../ipc/events";
import type { RecentEntry, Settings as AppSettings } from "../ipc/types";
import "./welcome.css";

/** A password prompt is already on screen for these — the welcome screen stays quiet. */
function isPasswordError(code: string): boolean {
  return code === "passwordRequired" || code === "passwordWrong";
}


/**
 * `Settings.recentsCount` (Stage 2) with the pre-Stage-2 `toolDefaults.recentsCount` as a
 * fallback. Three lines rather than an import from `dialogs/SettingsDialog`, which would pull
 * the whole settings sheet into the welcome chunk.
 */
function recentsLimit(settings: AppSettings | null): number {
  if (typeof settings?.recentsCount === "number" && settings.recentsCount >= 0) return settings.recentsCount;
  const legacy = settings?.toolDefaults?.recentsCount;
  return typeof legacy === "number" && legacy > 0 ? legacy : 20;
}

export function Welcome() {
  const t = useT();
  const os = useAppStore((s) => s.os);
  const recents = useAppStore((s) => s.recents);
  const settings = useAppStore((s) => s.settings);
  // v0.3 pkg5 (H1): the running build's version (`app_info`), not a hard-coded one
  const version = useAppStore((s) => s.appInfo?.version);
  const refreshRecents = useAppStore((s) => s.refreshRecents);
  const status = useDocStore((s) => s.status);
  const error = useDocStore((s) => s.error);
  const [filter, setFilter] = useState("");
  const [dragOver, setDragOver] = useState(false);

  useEffect(() => {
    void refreshRecents();
  }, [refreshRecents]);

  // v0.3 H7: the native drag-drop events drive the highlight (HTML5 dragover never fires on Windows)
  useEffect(() => onFileDragState((state) => setDragOver(state === "over")), []);

  const shown = useMemo(() => {
    const q = filter.trim().toLowerCase();
    const matched = q
      ? recents.filter((r) => r.name.toLowerCase().includes(q) || r.dir.toLowerCase().includes(q))
      : recents;
    return [...matched]
      .sort((a, b) => Number(b.pinned) - Number(a.pinned) || b.lastOpened.localeCompare(a.lastOpened))
      .slice(0, recentsLimit(settings));
  }, [recents, filter, settings]);

  const open = async (path: string) => {
    const { openPath } = await import("../dialogs/flows");
    await openPath(path);
  };

  const openPicker = async () => {
    const { openFileFlow } = await import("../dialogs/flows");
    await openFileFlow();
  };

  const cardMenu = (entry: RecentEntry, x: number, y: number) => {
    openContextMenu({
      x,
      y,
      labelKey: "welcome.recent",
      items: [
        { id: "open", labelKey: "common.open", onSelect: () => void open(entry.path) },
        {
          id: "reveal",
          labelKey: os === "windows" ? "menu.file.revealInExplorer" : "menu.file.revealInFinder",
          onSelect: () => void api.revealInFileManager({ path: entry.path }).catch(() => undefined),
        },
        {
          id: "pin",
          labelKey: entry.pinned ? "welcome.recent.unpin" : "welcome.recent.pin",
          onSelect: () => void api.setRecentPinned({ path: entry.path, pinned: !entry.pinned }).then(refreshRecents),
        },
        { id: "sep", separator: true },
        {
          id: "remove",
          labelKey: "welcome.recent.remove",
          danger: true,
          onSelect: () => void api.removeRecent({ path: entry.path }).then(refreshRecents),
        },
      ],
    });
  };

  return (
    <div
      className="welcome"
      data-dragover={dragOver || undefined}
      onDragOver={(e) => {
        e.preventDefault();
        setDragOver(true);
      }}
      onDragLeave={(e) => {
        if (e.currentTarget === e.target) setDragOver(false);
      }}
      onDrop={() => setDragOver(false)}
    >
      <div className="welcome-rail">
        <div className="wordmark">
          <span className="wordmark-mark" aria-hidden>
            <FileText size={22} strokeWidth={1.75} />
          </span>
          <span className="wordmark-text text-lg">{t("app.name")}</span>
        </div>
        {version && <p className="text-xs dim">{t("app.version", { version })}</p>}

        <button type="button" className="btn primary" onClick={() => void openPicker()}>
          <FolderOpen size={16} strokeWidth={1.75} aria-hidden />
          {t("welcome.open")}
          <span className="btn-key">{shortcutFor("file.open", os)}</span>
        </button>

        <div className="rail-spacer" />
        {/* v0.3 pkg5 (H1): 도움말 — the shortcuts sheet (Windows has no menu bar to find it in) */}
        <button type="button" className="btn quiet" onClick={() => openDialog("shortcuts")}>
          <CircleHelp size={16} strokeWidth={1.75} aria-hidden />
          {t("menu.help")}
          <span className="btn-key">{shortcutFor("help.shortcuts", os)}</span>
        </button>
        <button type="button" className="btn quiet" onClick={() => openDialog("settings")}>
          <Settings size={16} strokeWidth={1.75} aria-hidden />
          {t("settings.title")}
        </button>
      </div>

      <div className="welcome-main">
        <header className="welcome-head">
          <h1 className="text-md">{t("welcome.recent")}</h1>
          <input
            className="field"
            type="search"
            value={filter}
            placeholder={t("welcome.recent.search")}
            aria-label={t("welcome.recent.search")}
            onChange={(e) => setFilter(e.target.value)}
          />
        </header>

        {status === "error" && error && !isPasswordError(error.code) && (
          <p className="banner danger text-sm">{t(api.openErrorKey(error))}</p>
        )}

        {shown.length === 0 ? (
          <div className="dropzone">
            <p className="text-lg">{t("welcome.dropZone")}</p>
            <p className="text-sm dim">{t("welcome.dropZone.hint", { shortcut: shortcutFor("file.open", os) })}</p>
            {recents.length === 0 && <p className="text-sm dim">{t("welcome.recent.empty")}</p>}
          </div>
        ) : (
          <ul className="recent-grid">
            {shown.map((entry) => (
              <li key={entry.path} onContextMenu={(e) => { e.preventDefault(); cardMenu(entry, e.clientX, e.clientY); }}>
                <button
                  type="button"
                  className="recent-card"
                  onClick={() => void open(entry.path)}
                  disabled={status === "opening"}
                >
                  <span className="recent-thumb">
                    <img src={recentThumbUrl(entry.thumbId ?? entry.name)} alt="" draggable={false} />
                    {entry.pinned && (
                      <span className="recent-pin" aria-label={t("welcome.recent.pin")}>
                        <Pin size={12} strokeWidth={2} />
                      </span>
                    )}
                  </span>
                  <span className="recent-name text-sm">{entry.name}</span>
                  <span className="recent-dir text-xs dim">{entry.dir}</span>
                  <span className="recent-meta text-xs dim">
                    {formatRelativeDay(entry.lastOpened)} · {t("welcome.pageCount", { count: entry.pages })} ·{" "}
                    {formatBytes(entry.bytes)}
                  </span>
                </button>
              </li>
            ))}
          </ul>
        )}
      </div>
    </div>
  );
}

export default Welcome;
