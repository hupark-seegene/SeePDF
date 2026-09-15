import { useEffect, useMemo, useState } from "react";
import { FileText, FolderOpen, Pin, Settings } from "lucide-react";
import { useT } from "../i18n/useT";
import { formatBytes, formatRelativeDay } from "../i18n";
import { useAppStore } from "../store/appStore";
import { useDocStore } from "../store/docStore";
import { recentThumbUrl } from "../ipc/protocol";
import { shortcutFor } from "../keys/keymap";
import * as api from "../ipc/api";

/**
 * Welcome / recent screen — PLACEHOLDER. Stage 1 (e) owns the real one in `src/welcome/**`
 * (context menus, multi-file drop → 각각 열기 / 하나로 합치기, pinning, restored reading position).
 * This version is wired to the real `get_recent` / `open_document` commands so the shell is usable.
 */
export function WelcomeStub() {
  const t = useT();
  const os = useAppStore((s) => s.os);
  const recents = useAppStore((s) => s.recents);
  const refreshRecents = useAppStore((s) => s.refreshRecents);
  const open = useDocStore((s) => s.open);
  const status = useDocStore((s) => s.status);
  const error = useDocStore((s) => s.error);
  const [filter, setFilter] = useState("");

  useEffect(() => {
    void refreshRecents();
  }, [refreshRecents]);

  const shown = useMemo(() => {
    const q = filter.trim().toLowerCase();
    const matched = q ? recents.filter((r) => r.name.toLowerCase().includes(q) || r.dir.toLowerCase().includes(q)) : recents;
    return [...matched].sort((a, b) => Number(b.pinned) - Number(a.pinned));
  }, [recents, filter]);

  const openDialog = async () => {
    const picked = await api.openFileDialog({ title: t("menu.file.open") });
    if (picked?.length) await open(picked[0]);
  };

  return (
    <div className="welcome">
      <div className="welcome-rail">
        <div className="wordmark">
          <span className="wordmark-mark" aria-hidden>
            <FileText size={22} strokeWidth={1.75} />
          </span>
          <span className="wordmark-text text-lg">{t("app.name")}</span>
        </div>
        <p className="text-xs dim">{t("app.version", { version: "0.1.0" })}</p>

        <button type="button" className="btn primary" onClick={() => void openDialog()}>
          <FolderOpen size={16} strokeWidth={1.75} aria-hidden />
          {t("welcome.open")}
          <span className="btn-key">{shortcutFor("file.open", os)}</span>
        </button>

        <div className="rail-spacer" />
        <button type="button" className="btn quiet">
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

        {status === "error" && error && (
          <p className="banner danger text-sm">{t("error.openFailed")}</p>
        )}

        {shown.length === 0 ? (
          <div className="dropzone">
            <p className="text-lg">{t("welcome.dropZone")}</p>
            <p className="text-sm dim">{t("welcome.dropZone.hint", { shortcut: shortcutFor("file.open", os) })}</p>
          </div>
        ) : (
          <ul className="recent-grid">
            {shown.map((entry) => (
              <li key={entry.path}>
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
