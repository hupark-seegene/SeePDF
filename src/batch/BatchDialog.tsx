/**
 * 여러 파일 처리 (v0.3 pkg8, X1; 도구 ▸ 여러 파일 처리…): a queue of PDFs on disk, one action for
 * all of them, a status per file (대기 / 진행 중 / 완료 / 건너뜀 / 실패 + 이유) and where the copies
 * go. The queue and the per-file steps are `flow.ts`; this is only the form.
 *
 * 워터마크·머리글·Bates reuses the 워터마크 dialog's form: 설정… opens it in batch mode on top of
 * this one and it hands its spec back. Lazy-loaded from `DialogHost`.
 */
import { X } from "lucide-react";
import * as api from "../ipc/api";
import { useT } from "../i18n/useT";
import { useAppStore } from "../store/appStore";
import { openDialog } from "../dialogs/dialogState";
import { Dialog, Row } from "../dialogs/Dialog";
import type { CompressPreset, StampSpec } from "../ipc/types";
import { baseName, isActive, isRunnable, summarize, type BatchItem } from "../ocr/batch/queue";
import {
  addFiles, cancelBatch, clearFiles, firstOutput, optionsProblem, removeFile, resetBatch, revealOutputs, setOptions,
  startBatch, useBatch, type BatchAction,
} from "./flow";
import "../ocr/batch/batchOcr.css";

const ACTIONS: BatchAction[] = ["stamp", "compress", "password", "flatten", "images"];
const PRESETS: CompressPreset[] = [300, 150, 96];
const DPIS = [72, 150, 300];

export default function BatchDialog({ onClose }: { onClose(): void }) {
  const t = useT();
  const os = useAppStore((s) => s.os);
  const { items, options, phase, runDone, runTotal, cancelling } = useBatch();
  const running = phase === "running";
  const runnable = items.filter(isRunnable).length;
  const summary = summarize(items);
  const output = firstOutput(items);
  const problem = optionsProblem(options);
  const canStart = !running && runnable > 0 && !problem;
  const current = items.find((i) => isActive(i.status));

  const add = async () => {
    const picked = await api.openFileDialog({ multiple: true, title: t("batch.pick") });
    if (picked?.length) addFiles(picked);
  };

  const pickFolder = async () => {
    const picked = await api.openFileDialog({ directory: true, title: t("batchOcr.output.pickFolder") });
    if (picked?.length) setOptions({ outputDir: picked[0] });
  };

  const close = () => {
    if (phase === "finished") resetBatch();
    onClose();
  };

  const configureStamp = () =>
    openDialog("stamp", { onBatchSpec: (spec: StampSpec) => setOptions({ stamp: spec }) });

  return (
    <Dialog
      titleKey="batch.title"
      size="lg"
      onClose={close}
      cancelKey="common.close"
      primary={{ labelKey: "ocr.start", onSelect: () => void startBatch(), disabled: !canStart }}
      footerExtra={
        phase === "finished" && output ? (
          <button type="button" className="btn quiet" onClick={revealOutputs}>
            {t(os === "windows" ? "export.revealExplorer" : "export.revealFinder")}
          </button>
        ) : null
      }
    >
      <p className="text-sm dim">{t("batch.description")}</p>

      <div className="inline-row">
        <button type="button" className="btn" disabled={running} onClick={() => void add()}>
          {t("batchOcr.add")}
        </button>
        <button type="button" className="btn quiet" disabled={running || items.length === 0} onClick={clearFiles}>
          {t("batchOcr.clear")}
        </button>
        <span className="grow" />
        <span className="text-sm dim">{t("batchOcr.count", { count: items.length })}</span>
      </div>

      <div className="bocr-list">
        {items.length === 0 ? (
          <p className="bocr-empty text-sm">{t("batchOcr.empty")}</p>
        ) : (
          <table className="recovery-table text-sm">
            <thead>
              <tr>
                <th scope="col">{t("batchOcr.col.file")}</th>
                <th scope="col" className="bocr-status">{t("batchOcr.col.status")}</th>
                <th scope="col" className="bocr-remove" aria-label={t("common.remove")} />
              </tr>
            </thead>
            <tbody>
              {items.map((item) => (
                <tr key={item.id} data-testid="batch-row" data-status={item.status}>
                  <td className="bocr-name" title={item.path}>{item.name}</td>
                  <td className="bocr-status" data-status={item.status} title={item.status === "done" ? item.output : item.detail}>
                    {statusText(item, t)}
                  </td>
                  <td className="bocr-remove">
                    <button
                      type="button"
                      className="icon-btn"
                      disabled={running}
                      aria-label={t("batchOcr.remove", { name: item.name })}
                      onClick={() => removeFile(item.id)}
                    >
                      <X size={14} strokeWidth={1.75} aria-hidden />
                    </button>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </div>

      <Row labelKey="batch.action">
        <select
          className="field"
          value={options.action}
          disabled={running}
          aria-label={t("batch.action")}
          onChange={(e) => setOptions({ action: e.target.value as BatchAction })}
        >
          {ACTIONS.map((a) => (
            <option key={a} value={a}>
              {t(`batch.action.${a}`)}
            </option>
          ))}
        </select>
      </Row>

      {options.action === "stamp" && (
        <Row labelKey="batch.stamp">
          <div className="inline-row">
            <span className="text-sm grow" data-testid="batch-stamp-summary">
              {options.stamp ? stampSummary(options.stamp, t) : t("batch.need.stamp")}
            </span>
            <button type="button" className="btn" disabled={running} onClick={configureStamp}>
              {t("batch.stamp.configure")}
            </button>
          </div>
          <p className="dlg-hint text-xs">{t("batch.stamp.hint")}</p>
        </Row>
      )}

      {options.action === "compress" && (
        <Row labelKey="compress.quality">
          <select
            className="field"
            value={options.compressDpi}
            disabled={running}
            aria-label={t("compress.quality")}
            onChange={(e) => setOptions({ compressDpi: Number(e.target.value) as CompressPreset })}
          >
            {PRESETS.map((p) => (
              <option key={p} value={p}>
                {t(`compress.preset.${p}`)}
              </option>
            ))}
          </select>
        </Row>
      )}

      {options.action === "password" && (
        <Row labelKey="batch.password" hintKey="batch.password.hint">
          <input
            className="field"
            type="password"
            autoComplete="new-password"
            aria-label={t("batch.password")}
            disabled={running}
            value={options.password}
            onChange={(e) => setOptions({ password: e.target.value })}
          />
        </Row>
      )}

      {options.action === "flatten" && <p className="dlg-hint text-xs">{t("batch.flatten.hint")}</p>}

      {options.action === "images" && (
        <Row labelKey="export.format">
          <div className="inline-row">
            <select
              className="field"
              value={options.imageFormat}
              disabled={running}
              aria-label={t("export.format")}
              onChange={(e) => setOptions({ imageFormat: e.target.value as "png" | "jpeg" })}
            >
              <option value="png">{t("export.format.png")}</option>
              <option value="jpeg">{t("export.format.jpeg")}</option>
            </select>
            <select
              className="field"
              value={options.imageDpi}
              disabled={running}
              aria-label={t("export.dpi")}
              onChange={(e) => setOptions({ imageDpi: Number(e.target.value) })}
            >
              {DPIS.map((d) => (
                <option key={d} value={d}>{`${d} DPI`}</option>
              ))}
            </select>
          </div>
        </Row>
      )}

      <Row labelKey="batchOcr.output" hintKey="batch.output.hint">
        <div className="dlg-radio-group" role="radiogroup" aria-label={t("batchOcr.output")}>
          <label className="dlg-radio text-base">
            <input
              type="radio"
              name="batch-output"
              checked={options.outputDir === null}
              disabled={running}
              onChange={() => setOptions({ outputDir: null })}
            />
            <span>{t("batchOcr.output.beside")}</span>
          </label>
          <div className="inline-row">
            <label className="dlg-radio text-base">
              <input
                type="radio"
                name="batch-output"
                checked={options.outputDir !== null}
                disabled={running}
                onChange={() => (options.outputDir === null ? void pickFolder() : undefined)}
              />
              <span>{t("batchOcr.output.folder")}</span>
            </label>
            <span
              className="text-sm cmp-file grow"
              title={options.outputDir ?? undefined}
              data-empty={!options.outputDir || undefined}
            >
              {options.outputDir ? options.outputDir : t("batchOcr.output.noFolder")}
            </span>
            <button type="button" className="btn" disabled={running} onClick={() => void pickFolder()}>
              {t("common.browse")}
            </button>
          </div>
        </div>
      </Row>

      {running && (
        <div className="compress-run">
          <progress className="job-bar" value={runDone} max={Math.max(1, runTotal)} aria-label={t("batch.running")} />
          <span className="text-sm mono" aria-live="polite">
            {t("batchOcr.progress", { done: Math.min(runDone + 1, runTotal), total: runTotal })}
            {current ? ` · ${current.name}` : ""}
          </span>
          <button type="button" className="btn" disabled={cancelling} onClick={() => void cancelBatch()}>
            {t("common.cancel")}
          </button>
        </div>
      )}

      {phase === "finished" && (
        <p className="banner text-sm" role="status" data-testid="batch-summary">
          {t("batchOcr.summary", { done: summary.done, skipped: summary.skipped, failed: summary.failed })}
        </p>
      )}
    </Dialog>
  );
}

export function stampSummary(spec: StampSpec, t: ReturnType<typeof useT>): string {
  // v0.3 integration: pkg4's 배경색 source (A10) has no text or file to name
  const what =
    spec.source.kind === "text"
      ? spec.source.text
      : spec.source.kind === "background"
        ? t("stamp.source.background")
        : baseName(spec.source.path);
  return `${t(`stamp.role.${spec.role}`)} · ${what}`;
}

function statusText(item: BatchItem, t: ReturnType<typeof useT>): string {
  switch (item.status) {
    case "queued":
      return t("batchOcr.status.queued");
    case "opening":
      return t("batchOcr.status.opening");
    case "running":
      return t("batchOcr.status.running", { done: item.done, total: item.total });
    case "saving":
      return t("batchOcr.status.saving");
    case "done":
      return item.output ? `${t("batchOcr.status.done")} · ${baseName(item.output)}` : t("batchOcr.status.done");
    case "cancelled":
      return t("batchOcr.status.cancelled");
    case "skipped":
    case "failed": {
      const label = t(item.status === "skipped" ? "batchOcr.status.skipped" : "batchOcr.status.failed");
      return item.reasonKey ? `${label} · ${t(item.reasonKey)}` : label;
    }
  }
}
