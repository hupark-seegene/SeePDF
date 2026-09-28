/**
 * 복구 (P1-8): recovery copies a previous session left behind (it crashed, or was killed) — shown once
 * at launch. Per row 열기 / 삭제, plus 모두 삭제 and 나중에 (keep them for the next launch).
 * 열기 goes through the normal open path; the document is marked as a recovery copy so 저장 asks
 * for a location. Lazy-loaded from `DialogHost`.
 */
import { useEffect, useRef, useState } from "react";
import * as api from "../ipc/api";
import { useLocale, useT } from "../i18n/useT";
import { formatBytes } from "../i18n";
import { toast } from "../app/toastStore";
import type { RecoveryEntry } from "../ipc/types";
import { Dialog } from "./Dialog";
import { openPath } from "./flows";

export function formatSavedAt(iso: string, locale: string): string {
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return iso;
  return new Intl.DateTimeFormat(locale, { dateStyle: "medium", timeStyle: "short" }).format(d);
}

export default function RecoveryDialog({ entries: initial, onClose }: { entries: RecoveryEntry[]; onClose(): void }) {
  const t = useT();
  const locale = useLocale();
  const [entries, setEntries] = useState(initial);
  const [busy, setBusy] = useState(false);
  const touched = useRef(false);

  // re-read on mount: this dialog also remounts after a prompt opened over it
  useEffect(() => {
    let alive = true;
    void api
      .listRecovery()
      .then((list) => {
        if (alive && !touched.current) setEntries(list);
      })
      .catch(() => undefined);
    return () => {
      alive = false;
    };
  }, []);

  const discard = async (ids: string[]) => {
    touched.current = true;
    setBusy(true);
    await Promise.all(ids.map((id) => api.discardRecovery({ id }).catch(() => undefined)));
    const rest = entries.filter((e) => !ids.includes(e.id));
    setEntries(rest);
    setBusy(false);
    if (rest.length === 0) onClose();
  };

  const open = async (entry: RecoveryEntry) => {
    setBusy(true);
    onClose();
    const info = await openPath(entry.recoveryPath, { recovery: entry });
    if (info) toast("recovery.opened", undefined, { tone: "info", timeoutMs: 8000 });
  };

  return (
    <Dialog
      titleKey="recovery.title"
      size="lg"
      onClose={onClose}
      cancelKey="recovery.later"
      footerExtra={
        <button
          type="button"
          className="btn quiet"
          data-danger
          disabled={busy || entries.length === 0}
          onClick={() => void discard(entries.map((e) => e.id))}
        >
          {t("recovery.discardAll")}
        </button>
      }
    >
      <p className="text-sm dim">{t("recovery.intro")}</p>
      <table className="recovery-table text-sm">
        <thead>
          <tr>
            <th scope="col">{t("recovery.col.name")}</th>
            <th scope="col">{t("recovery.col.savedAt")}</th>
            <th scope="col">{t("recovery.col.pages")}</th>
            <th scope="col">{t("recovery.col.size")}</th>
            <th scope="col" aria-label={t("recovery.col.actions")} />
          </tr>
        </thead>
        <tbody>
          {entries.map((e) => (
            <tr key={e.id}>
              <td className="recovery-name" title={e.originalPath ?? e.name}>
                {e.name}
              </td>
              <td className="mono">{formatSavedAt(e.savedAt, locale)}</td>
              <td className="mono">{e.pages}</td>
              <td className="mono">{formatBytes(e.bytes)}</td>
              <td className="recovery-actions">
                <button
                  type="button"
                  className="btn"
                  disabled={busy}
                  aria-label={t("recovery.openNamed", { name: e.name })}
                  onClick={() => void open(e)}
                >
                  {t("common.open")}
                </button>
                <button
                  type="button"
                  className="btn quiet"
                  disabled={busy}
                  aria-label={t("recovery.discardNamed", { name: e.name })}
                  onClick={() => void discard([e.id])}
                >
                  {t("common.delete")}
                </button>
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </Dialog>
  );
}
