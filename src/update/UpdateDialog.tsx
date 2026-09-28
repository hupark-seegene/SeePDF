/**
 * 업데이트 확인 (v0.2.0) — renders the phase of `updateStore`: 확인 중 → 최신 버전 / 새 버전 (버전,
 * 릴리스 노트, 다운로드) → 진행률 → 다시 시작. Closing the dialog does not stop a download; opening
 * it again shows where it got to.
 */
import { useT } from "../i18n/useT";
import { formatBytes } from "../i18n";
import { Dialog } from "../dialogs/Dialog";
import {
  downloadUpdate,
  installAndRestart,
  progressPercent,
  retryUpdate,
  useUpdateStore,
} from "./updateStore";
import "./update.css";

async function confirmLeave(): Promise<boolean> {
  const { confirmLeaveDocument } = await import("../dialogs/flows");
  return confirmLeaveDocument();
}

export default function UpdateDialog({ onClose }: { onClose(): void }) {
  const t = useT();
  const s = useUpdateStore();
  const percent = progressPercent(s.downloaded, s.total);

  const primary =
    s.phase === "available"
      ? { labelKey: "update.download", onSelect: () => void downloadUpdate() }
      : s.phase === "ready"
        ? { labelKey: "update.restart", onSelect: () => void installAndRestart(confirmLeave) }
        : s.phase === "error"
          ? { labelKey: "common.retry", onSelect: () => void retryUpdate() }
          : s.phase === "downloading" || s.phase === "installing"
            ? { labelKey: s.phase === "installing" ? "update.installing" : "update.downloading", onSelect: () => undefined, disabled: true }
            : undefined;

  return (
    <Dialog titleKey="update.title" size="md" onClose={onClose} cancelKey="common.close" primary={primary}>
      <div className="update-body" aria-live="polite">
        {(s.phase === "idle" || s.phase === "checking") && <p className="text-base">{t("update.checking")}</p>}

        {s.phase === "upToDate" && (
          <p className="text-base">{t("update.upToDate", { version: s.currentVersion ?? "" })}</p>
        )}

        {(s.phase === "available" || s.phase === "downloading" || s.phase === "ready" || s.phase === "installing") && (
          <>
            <p className="text-base update-headline">{t("update.available", { version: s.version ?? "" })}</p>
            {s.currentVersion && (
              <p className="text-sm dim">{t("update.current", { version: s.currentVersion })}</p>
            )}
            {s.notes && (
              <section className="update-notes" aria-label={t("update.notes")}>
                <h3 className="text-sm">{t("update.notes")}</h3>
                <div className="update-notes-text text-sm">{s.notes}</div>
              </section>
            )}
          </>
        )}

        {s.phase === "downloading" && (
          <div className="update-progress">
            <div
              className="update-bar"
              role="progressbar"
              aria-label={t("update.progress")}
              aria-valuemin={0}
              aria-valuemax={100}
              aria-valuenow={percent ?? undefined}
            >
              <div className="update-bar-fill" data-indeterminate={percent === null || undefined} style={{ width: `${percent ?? 100}%` }} />
            </div>
            <p className="text-sm dim mono">
              {percent !== null
                ? t("update.progress.percent", { percent, downloaded: formatBytes(s.downloaded), total: formatBytes(s.total ?? 0) })
                : t("update.progress.bytes", { downloaded: formatBytes(s.downloaded) })}
            </p>
          </div>
        )}

        {s.phase === "ready" && <p className="text-sm">{t("update.ready")}</p>}
        {s.phase === "installing" && <p className="text-sm">{t("update.installing")}</p>}

        {s.phase === "error" && (
          <>
            <p className="text-base">{t(s.failedStep === "check" ? "update.error.check" : "update.error.install")}</p>
            {s.error && <p className="text-xs dim mono update-error">{s.error}</p>}
          </>
        )}
      </div>
    </Dialog>
  );
}
