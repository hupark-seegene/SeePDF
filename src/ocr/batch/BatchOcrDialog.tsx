/**
 * 여러 파일 OCR (P1-7, 640 px, 도구 ▸ 여러 파일 OCR… or ⋯): a queue of PDFs on disk with a status per
 * file (대기 / 진행 중 n/m 페이지 / 완료 / 건너뜀 / 실패 + 이유), the OCR dialog's shared options
 * (언어, 해상도, 이미 텍스트가 있는 페이지 건너뛰기) and where the `-ocr.pdf` copies go.
 *
 * 시작 runs the queue (`flow.ts`); 취소 stops the current file and the queue; 닫기 only hides the
 * dialog — a running batch carries on in the status bar. Lazy-loaded from `DialogHost`.
 */
import { X } from "lucide-react";
import * as api from "../../ipc/api";
import { useT } from "../../i18n/useT";
import { useAppStore } from "../../store/appStore";
import { Dialog, Row } from "../../dialogs/Dialog";
import type { OcrDpi } from "../ocrJob";
import {
  addFiles, cancelBatch, clearFiles, firstOutput, removeFile, resetBatch, revealOutputs, setOptions,
  startBatch, useBatchOcr,
} from "./flow";
import { baseName, isActive, isRunnable, summarize, type BatchItem } from "./queue";
import "./batchOcr.css";

const DPI_CHOICES: OcrDpi[] = ["auto", 200, 300, 400];

export default function BatchOcrDialog({ onClose }: { onClose(): void }) {
  const t = useT();
  const os = useAppStore((s) => s.os);
  const { items, options, phase, runDone, runTotal, cancelling } = useBatchOcr();
  const running = phase === "running";
  const runnable = items.filter(isRunnable).length;
  const summary = summarize(items);
  const output = firstOutput(items);
  const canStart = !running && runnable > 0 && (options.ko || options.en);

  const add = async () => {
    const picked = await api.openFileDialog({ multiple: true, title: t("batchOcr.pick") });
    if (picked?.length) addFiles(picked);
  };

  const pickFolder = async () => {
    const picked = await api.openFileDialog({ directory: true, title: t("batchOcr.output.pickFolder") });
    if (picked?.length) setOptions({ outputDir: picked[0] });
  };

  const close = () => {
    // A finished run's list goes with the dialog; a running one keeps going in the status bar.
    if (phase === "finished") resetBatch();
    onClose();
  };

  const current = items.find((i) => isActive(i.status));

  return (
    <Dialog
      titleKey="batchOcr.title"
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
      <p className="text-sm dim">{t("batchOcr.description")}</p>

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
                <tr key={item.id} data-testid="bocr-row" data-status={item.status}>
                  <td className="bocr-name" title={item.path}>{item.name}</td>
                  <td className="bocr-status" data-status={item.status} title={statusTitle(item)}>
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

      <Row labelKey="ocr.language">
        <div className="chip-row">
          <button
            type="button"
            className="chip"
            data-active={options.ko || undefined}
            aria-pressed={options.ko}
            disabled={running}
            onClick={() => setOptions({ ko: options.en ? !options.ko : true })}
          >
            {t("ocr.language.ko")}
          </button>
          <button
            type="button"
            className="chip"
            data-active={options.en || undefined}
            aria-pressed={options.en}
            disabled={running}
            onClick={() => setOptions({ en: options.ko ? !options.en : true })}
          >
            {t("ocr.language.en")}
          </button>
        </div>
      </Row>

      <Row labelKey="ocr.option.dpi">
        <select
          className="field"
          value={String(options.dpi)}
          disabled={running}
          aria-label={t("ocr.option.dpi")}
          onChange={(e) => setOptions({ dpi: e.target.value === "auto" ? "auto" : (Number(e.target.value) as OcrDpi) })}
        >
          {DPI_CHOICES.map((d) => (
            <option key={String(d)} value={String(d)}>
              {d === "auto" ? t("ocr.dpi.auto") : `${d} DPI`}
            </option>
          ))}
        </select>
      </Row>

      <label className="dlg-check text-base">
        <input
          type="checkbox"
          checked={options.skipPagesWithText}
          disabled={running}
          onChange={(e) => setOptions({ skipPagesWithText: e.target.checked })}
        />
        <span>{t("ocr.option.skipText")}</span>
      </label>

      <Row labelKey="batchOcr.output" hintKey="batchOcr.output.hint">
        <div className="dlg-radio-group" role="radiogroup" aria-label={t("batchOcr.output")}>
          <label className="dlg-radio text-base">
            <input
              type="radio"
              name="bocr-output"
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
                name="bocr-output"
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
          <progress className="job-bar" value={runDone} max={Math.max(1, runTotal)} aria-label={t("batchOcr.running")} />
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
        <p className="banner text-sm" role="status" data-testid="bocr-summary">
          {t("batchOcr.summary", { done: summary.done, skipped: summary.skipped, failed: summary.failed })}
        </p>
      )}
    </Dialog>
  );
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

function statusTitle(item: BatchItem): string | undefined {
  if (item.status === "done") return item.output;
  return item.detail;
}
