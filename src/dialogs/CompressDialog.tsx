/**
 * 압축 (P1-5): pick a target DPI, run 예상 on a scratch copy (a cancellable job), compare the
 * before/after sizes, then 적용 (one undo step) — or close, which discards the pending result.
 * An estimate that saves nothing (no image downsampled, or not smaller) leaves 적용 off with the
 * reason beside it — only 닫기 / 다시 예상 — and its result is released at once.
 * Lazy-loaded from `DialogHost`.
 */
import { useEffect, useMemo, useRef, useState } from "react";
import * as api from "../ipc/api";
import { useLocale, useT } from "../i18n/useT";
import { formatBytes } from "../i18n";
import { useDocStore } from "../store/docStore";
import { useViewStore } from "../store/viewStore";
import { usePagesStore } from "../store/pagesStore";
import { toast } from "../app/toastStore";
import type { CompressPreset, CompressReport, DocId, JobEvent, JobId } from "../ipc/types";
import { Dialog, Row } from "./Dialog";
import { RangePicker } from "./RangePicker";
import { resolveRange, type RangeChoice } from "./pageRange";
import { message } from "./flows";
import { COMPRESS_PRESETS, DEFAULT_PRESET, applyBlock, buildCompressOptions, formatDeltaPct, summarize } from "./compress";

type Phase = "idle" | "estimating" | "ready" | "applying";

interface Live { mounted: boolean; jobId: JobId | null; token: number | null; docId: DocId | null; run: number }

/** Drop the pending result the engine holds for us, if any. */
function discard(ref: Live): void {
  if (ref.token !== null && ref.docId) {
    const token = ref.token;
    ref.token = null;
    void api.compressDiscard({ docId: ref.docId, token }).catch(() => undefined);
  }
}

export default function CompressDialog({ onClose }: { onClose(): void }) {
  const t = useT();
  const locale = useLocale();
  const info = useDocStore((s) => s.info);
  const currentPage = useViewStore((s) => s.currentPage);
  const selected = usePagesStore((s) => s.selected);
  const [dpi, setDpi] = useState<CompressPreset>(DEFAULT_PRESET);
  const [range, setRange] = useState<RangeChoice>({ mode: "all", text: "" });
  const [phase, setPhase] = useState<Phase>("idle");
  const [progress, setProgress] = useState({ done: 0, total: 0 });
  const [report, setReport] = useState<CompressReport | null>(null);

  // What must be cleaned up however the dialog goes away (취소, Esc, backdrop, closeAll):
  // the running job and the pending result the engine is holding for us.
  const live = useRef<Live>({ mounted: true, jobId: null, token: null, docId: null, run: 0 });

  const pageCount = info?.pageCount ?? 0;
  const pages = useMemo(
    () => resolveRange(range, { pageCount, currentPage, selected }),
    [range, pageCount, currentPage, selected],
  );

  useEffect(() => {
    const ref = live.current;
    ref.mounted = true;
    return () => {
      ref.mounted = false;
      if (ref.jobId !== null) void api.cancelJob({ jobId: ref.jobId }).catch(() => false);
      discard(ref);
    };
  }, []);

  if (!info) return null;
  const docId = info.docId;
  live.current.docId = docId;

  /** Options changed: the shown estimate no longer describes what 적용 would do. */
  const invalidate = () => {
    if (phase === "estimating") return;
    discard(live.current);
    setReport(null);
    setPhase("idle");
  };

  const estimate = async () => {
    if (!pages?.length) return;
    discard(live.current);
    setReport(null);
    setPhase("estimating");
    setProgress({ done: 0, total: pages.length });
    const run = ++live.current.run;
    let finished = false;
    const onEvent = (e: JobEvent) => {
      const ref = live.current;
      if (e.type === "done" || e.type === "cancelled" || e.type === "error") finished = true;
      // a result that arrives after the dialog closed (or after a newer run) is dropped at once
      if (!ref.mounted || run !== ref.run) {
        if (e.type === "done" && e.report) void api.compressDiscard({ docId, token: e.report.token }).catch(() => undefined);
        return;
      }
      switch (e.type) {
        case "started":
          ref.jobId = e.jobId;
          setProgress({ done: 0, total: e.total });
          break;
        case "progress":
          ref.jobId = e.jobId;
          setProgress({ done: e.done, total: e.total });
          break;
        case "done":
          ref.jobId = null;
          if (e.report) {
            ref.token = e.report.token;
            // nothing to apply: the engine need not hold the rewritten copy while the dialog is open
            if (applyBlock(e.report)) discard(ref);
            setReport(e.report);
            setPhase("ready");
          } else {
            setPhase("idle");
          }
          break;
        case "cancelled":
          ref.jobId = null;
          setPhase("idle");
          break;
        case "error":
          ref.jobId = null;
          setPhase("idle");
          toast("compress.failed", undefined, { tone: "danger", detail: e.error.message });
          break;
      }
    };
    try {
      const jobId = await api.compressEstimate({ docId, options: buildCompressOptions(dpi, pages, range.mode === "all") }, onEvent);
      // mock mode (and a fast engine) may have streamed every event before the id comes back
      if (finished) return;
      if (live.current.run === run && live.current.mounted) live.current.jobId = jobId;
      // 취소 / close happened before the id was known: nobody else can cancel this job
      else void api.cancelJob({ jobId }).catch(() => false);
    } catch (e) {
      if (live.current.run === run) setPhase("idle");
      toast("compress.failed", undefined, { tone: "danger", detail: message(e) });
    }
  };

  const cancel = () => {
    const jobId = live.current.jobId;
    if (jobId !== null) void api.cancelJob({ jobId }).catch(() => false);
    live.current.run += 1; // anything still in flight belongs to a dead run now
    live.current.jobId = null;
    setPhase("idle");
  };

  const apply = async () => {
    if (!report || applyBlock(report)) return;
    setPhase("applying");
    live.current.token = null; // consumed by apply, success or not
    try {
      const next = await api.compressApply({ docId, token: report.token });
      useDocStore.getState().adopt(next);
      toast(
        "compress.done",
        { before: formatBytes(report.beforeBytes), after: formatBytes(report.afterBytes) },
        {
          tone: "success",
          actions: [{ labelKey: "common.undo", onSelect: () => void import("../annot/sync").then((m) => m.undoWithAnnots()) }],
        },
      );
      onClose();
    } catch (e) {
      setReport(null);
      setPhase("idle");
      if (api.isSeePdfError(e) && e.code === "stale") {
        // the document changed after the estimate (the engine spent the token): estimate again
        toast("compress.stale", undefined, { tone: "info" });
        void estimate();
        return;
      }
      toast("compress.failed", undefined, { tone: "danger", detail: message(e) });
    }
  };

  const summary = report ? summarize(report) : null;
  const block = report ? applyBlock(report) : null;
  const estimating = phase === "estimating";

  return (
    <Dialog
      titleKey="compress.title"
      size="lg"
      onClose={onClose}
      // a result that saves nothing has nothing to cancel: the choices are 닫기 / 다시 예상
      cancelKey={block ? "common.close" : undefined}
      footerExtra={
        block ? (
          <span className="dlg-hint text-xs" data-testid="compress-blocked">
            {t("compress.applyBlocked")}
          </span>
        ) : undefined
      }
      primary={{ labelKey: "common.apply", onSelect: () => void apply(), disabled: phase !== "ready" || block !== null }}
    >
      <Row labelKey="compress.quality" hintKey="compress.hint">
        <div className="compress-presets" role="radiogroup" aria-label={t("compress.quality")}>
          {COMPRESS_PRESETS.map((p) => (
            <label key={p.dpi} className="dlg-radio text-base">
              <input
                type="radio"
                name="compress-dpi"
                checked={dpi === p.dpi}
                disabled={estimating}
                onChange={() => {
                  setDpi(p.dpi);
                  invalidate();
                }}
              />
              <span>{t(p.labelKey)}</span>
            </label>
          ))}
        </div>
      </Row>

      <Row labelKey="pages.range">
        <RangePicker
          value={range}
          onChange={(next) => {
            setRange(next);
            invalidate();
          }}
          pageCount={pageCount}
          selectedCount={selected.length}
        />
      </Row>

      <div className="compress-run">
        {estimating ? (
          <>
            <progress className="job-bar" value={progress.done} max={Math.max(1, progress.total)} aria-label={t("compress.estimate")} />
            <span className="text-sm mono" aria-live="polite">
              {t("compress.estimating", { done: progress.done, total: progress.total })}
            </span>
            <button type="button" className="btn" onClick={cancel}>
              {t("common.cancel")}
            </button>
          </>
        ) : (
          <button type="button" className="btn" disabled={!pages?.length || phase === "applying"} onClick={() => void estimate()}>
            {t(report ? "compress.reestimate" : "compress.estimate")}
          </button>
        )}
      </div>

      {report && summary ? (
        <>
          <table className="compress-table text-sm" aria-live="polite">
            <tbody>
              <tr>
                <th scope="row">{t("compress.before")}</th>
                <td className="mono">{formatBytes(report.beforeBytes)}</td>
              </tr>
              <tr>
                <th scope="row">{t("compress.after")}</th>
                <td className="mono">{formatBytes(report.afterBytes)}</td>
              </tr>
              <tr>
                <th scope="row">{t("compress.delta")}</th>
                <td className={`mono ${summary.noGain ? "compress-warn" : "gain"}`} data-testid="compress-delta">
                  {formatDeltaPct(summary.deltaPct, locale)}
                </td>
              </tr>
              <tr>
                <th scope="row">{t("compress.images")}</th>
                <td className="mono">{t("compress.imagesValue", { done: report.imagesDownsampled, total: report.imagesTotal })}</td>
              </tr>
            </tbody>
          </table>
          {block && (
            <p className="compress-warn text-sm" role="alert">
              {block === "noGain" ? t("compress.noGain") : t("compress.noImages")}
            </p>
          )}
        </>
      ) : (
        !estimating && <p className="dlg-hint text-xs">{t("compress.needEstimate")}</p>
      )}
    </Dialog>
  );
}
