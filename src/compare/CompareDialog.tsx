/**
 * 문서 비교 (P1-6): pick the second file, 대소문자 무시, 비교 → opens B beside the current document,
 * runs `compare_documents` with an inline progress bar and 취소, then switches to the full-window
 * `CompareView`. Lazy-loaded from `DialogHost`.
 */
import * as api from "../ipc/api";
import { useT } from "../i18n/useT";
import { useDocStore } from "../store/docStore";
import { Dialog, Row } from "../dialogs/Dialog";
import { baseName } from "../dialogs/flows";
import { cancelCompare, resetCompareRun, startCompare, useCompareRun } from "./flow";

export default function CompareDialog({ onClose }: { onClose(): void }) {
  const t = useT();
  const info = useDocStore((s) => s.info);
  const { path, ignoreCase, phase, done, total } = useCompareRun();
  if (!info) return null;
  const running = phase !== "idle";

  const pick = async () => {
    const picked = await api.openFileDialog({ multiple: false, title: t("compare.pick") });
    if (picked?.length) useCompareRun.setState({ path: picked[0] });
  };

  const close = () => {
    resetCompareRun();
    onClose();
  };

  return (
    <Dialog
      titleKey="compare.title"
      size="md"
      onClose={close}
      primary={{ labelKey: "compare.run", onSelect: () => void startCompare(), disabled: !path || running }}
    >
      <Row labelKey="compare.current">
        <span className="text-base cmp-file" title={info.path ?? info.name}>
          {info.name}
        </span>
      </Row>
      <Row labelKey="compare.other" hintKey="compare.hint">
        <div className="inline-row">
          <span className="text-base cmp-file grow" title={path ?? undefined} data-empty={!path || undefined}>
            {path ? baseName(path) : t("compare.noFile")}
          </span>
          <button type="button" className="btn" disabled={running} onClick={() => void pick()}>
            {t("common.browse")}
          </button>
        </div>
      </Row>
      <label className="dlg-check text-base">
        <input
          type="checkbox"
          checked={ignoreCase}
          disabled={running}
          onChange={(e) => useCompareRun.setState({ ignoreCase: e.target.checked })}
        />
        <span>{t("compare.ignoreCase")}</span>
      </label>

      {running && (
        <div className="compress-run">
          <progress className="job-bar" value={done} max={Math.max(1, total)} aria-label={t("compare.run")} />
          <span className="text-sm mono" aria-live="polite">
            {phase === "opening" ? t("compare.opening") : t("compare.progress", { done, total })}
          </span>
          <button type="button" className="btn" onClick={cancelCompare}>
            {t("common.cancel")}
          </button>
        </div>
      )}
    </Dialog>
  );
}
