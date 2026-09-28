/**
 * 텍스트 인식 (OCR) — the modal sheet of UI_SPEC §10, 520 px.
 *
 *   setup     범위 · 언어 · 출력 · 옵션 (인식 엔진 where Apple Vision exists, 건너뛰기, 해상도,
 *             고급 ▸ 레이아웃 for Tesseract) · 예상 시간 + 취소/시작
 *   running   the same sheet becomes a progress view: `12 / 148`, the page thumbnail, elapsed and
 *             remaining, and a 취소 that is live at every moment
 *   done      an inline success bar with 실행 취소
 *
 * The dialogs module (e) owns dialog hosting, so this component renders nothing until
 * `openOcrDialog()` (src/ocr/dialogState.ts) is called. Everything below the button is `ocrJob.ts`.
 */
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { X } from "lucide-react";
import * as api from "../ipc/api";
import { thumbUrl } from "../ipc/protocol";
import type { PageIndex } from "../ipc/types";
import { useT } from "../i18n/useT";
import { useDocStore } from "../store/docStore";
import { useViewStore } from "../store/viewStore";
import { etaMs, localJobId, useJobStore } from "../store/jobStore";
import { useOcrDialogStore, type OcrDialogContext } from "./dialogState";
import {
  estimateSeconds, formatPageRange, parsePageRange, runOcrJob, type OcrDpi, type OcrJobResult,
} from "./ocrJob";
import { DEFAULT_LAYOUT, defaultWorkerCount, type OcrLayout } from "./tesseractPool";
import {
  ENGINE_CHOICES, isVisionAvailable, resolveEngine, useVisionAvailable, type OcrEngineChoice,
} from "./engine";
import "./OcrDialog.css";

type RangeMode = "all" | "current" | "custom";
type Phase = "setup" | "running" | "finished";

const DPI_CHOICES: OcrDpi[] = ["auto", 200, 300, 400];
const LAYOUTS: { id: OcrLayout; labelKey: string }[] = [
  { id: "auto", labelKey: "ocr.layout.auto" },
  { id: "column", labelKey: "ocr.layout.column" },
  { id: "block", labelKey: "ocr.layout.block" },
];

/** UI language chips → the tesseract language string. `kor` alone garbles English (spike §4.2). */
function langsFor(ko: boolean, en: boolean): string {
  if (ko && en) return "kor+eng";
  if (ko) return "kor+eng";   // Korean documents always carry Latin; never ship `kor` alone
  return "eng";
}

export function OcrDialog() {
  const t = useT();
  const open = useOcrDialogStore((s) => s.open);
  const nonce = useOcrDialogStore((s) => s.nonce);
  const context = useOcrDialogStore((s) => s.context);
  const hide = useOcrDialogStore((s) => s.hide);
  if (!open) return null;
  return <OcrDialogBody key={nonce} context={context} onClose={hide} t={t} />;
}

function OcrDialogBody(
  { context, onClose, t }: { context: OcrDialogContext; onClose: () => void; t: ReturnType<typeof useT> },
) {
  const info = useDocStore((s) => s.info);
  const currentPageFromView = useViewStore((s) => s.currentPage);

  const docId = context.docId ?? info?.docId;
  const docGeneration = context.docGeneration ?? info?.docGeneration ?? 0;
  const pageCount = context.pageCount ?? info?.pageCount ?? 0;
  const pageGeom = context.pages ?? info?.pages;
  const currentPage = context.currentPage ?? currentPageFromView ?? 0;

  const [rangeMode, setRangeMode] = useState<RangeMode>(context.selectedPages?.length ? "custom" : "all");
  const [rangeText, setRangeText] = useState(
    context.selectedPages?.length ? formatPageRange(context.selectedPages) : "",
  );
  const [ko, setKo] = useState(true);
  const [en, setEn] = useState(true);
  const [skipText, setSkipText] = useState(true);
  const [dpi, setDpi] = useState<OcrDpi>("auto");
  const [layout, setLayout] = useState<OcrLayout>(DEFAULT_LAYOUT);
  const [advanced, setAdvanced] = useState(false);
  const [engineChoice, setEngineChoice] = useState<OcrEngineChoice>("auto");
  const visionAvailable = useVisionAvailable();
  const engine = resolveEngine(engineChoice, visionAvailable === true);

  const [phase, setPhase] = useState<Phase>("setup");
  const [jobId, setJobId] = useState<number | null>(null);
  const [activePage, setActivePage] = useState<PageIndex | null>(null);
  const [result, setResult] = useState<OcrJobResult | null>(null);
  const [now, setNow] = useState(Date.now());
  const startedAt = useRef(0);

  const job = useJobStore((s) => s.jobs.find((j) => j.id === jobId) ?? null);
  const cancel = useJobStore((s) => s.cancel);

  // 1 Hz tick for elapsed/remaining while a job runs.
  useEffect(() => {
    if (phase !== "running") return;
    const id = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(id);
  }, [phase]);

  const pages = useMemo<PageIndex[] | null>(() => {
    if (pageCount <= 0) return null;
    if (rangeMode === "all") return Array.from({ length: pageCount }, (_, i) => i);
    if (rangeMode === "current") return [Math.max(0, Math.min(pageCount - 1, currentPage))];
    return parsePageRange(rangeText, pageCount);
  }, [rangeMode, rangeText, pageCount, currentPage]);

  const rangeInvalid = rangeMode === "custom" && pages === null;
  const workers = defaultWorkerCount();
  const seconds = estimateSeconds(pages?.length ?? 0, workers, engine);
  const canStart = !!docId && !!pages && pages.length > 0 && (ko || en) && phase === "setup";

  const start = useCallback(async () => {
    if (!docId || !pages) return;
    const id = localJobId();
    setJobId(id);
    setPhase("running");
    setActivePage(pages[0] ?? null);
    startedAt.current = Date.now();
    // The capability answer may still be in flight on a very fast click: ask the cached promise.
    const runEngine = resolveEngine(engineChoice, await isVisionAvailable());
    const r = await runOcrJob({
      docId,
      docGeneration,
      pages,
      pageGeom,
      langs: langsFor(ko, en),
      layout,
      dpi,
      skipPagesWithText: skipText,
      workers,
      engine: runEngine,
      jobId: id,
      onPageDone: (page) => {
        const next = pages[pages.indexOf(page) + 1];
        setActivePage(next ?? page);
      },
    });
    setResult(r);
    setPhase("finished");
  }, [docId, docGeneration, pages, pageGeom, ko, en, layout, dpi, skipText, workers, engineChoice]);

  const undoAll = useCallback(async () => {
    if (!docId || !result) return;
    // Each page is its own `registry::mutate`, so undoing the run means one `undo` per applied page.
    for (let i = 0; i < result.applied.length; i++) {
      await api.undo({ docId }).catch(() => undefined);
    }
    onClose();
  }, [docId, result, onClose]);

  // Esc closes — but never while a job is running: 취소 must be a deliberate click (UI_SPEC §10).
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape" && phase !== "running") { e.stopPropagation(); onClose(); }
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [phase, onClose]);

  const done = job?.done ?? 0;
  const total = job?.total ?? pages?.length ?? 0;
  const percent = total > 0 ? Math.round((done / total) * 100) : 0;
  const remaining = job ? etaMs(job) : null;
  const elapsed = Math.max(0, Math.round((now - startedAt.current) / 1000));

  return (
    <div className="ocr-backdrop" role="presentation">
      <div
        className="ocr-sheet"
        role="dialog"
        aria-modal="true"
        aria-labelledby="ocr-title"
      >
        <header className="ocr-header">
          <h2 id="ocr-title" className="ocr-title">{t("ocr.title")}</h2>
          {phase !== "running" && (
            <button className="icon-btn ocr-close" onClick={onClose} aria-label={t("common.close")} type="button">
              <X size={16} strokeWidth={1.75} />
            </button>
          )}
        </header>

        {phase === "setup" && (
          <div className="ocr-body">
            <p className="ocr-description">{t("ocr.description")}</p>

            <section className="ocr-section">
              <h3 className="ocr-label">{t("ocr.range")}</h3>
              <div className="segmented" role="radiogroup" aria-label={t("ocr.range")}>
                {(["all", "current", "custom"] as RangeMode[]).map((m) => (
                  <button
                    key={m}
                    type="button"
                    role="radio"
                    aria-checked={rangeMode === m}
                    className="segment"
                    data-active={rangeMode === m || undefined}
                    onClick={() => setRangeMode(m)}
                  >
                    {t(m === "all" ? "ocr.range.all" : m === "current" ? "ocr.range.current" : "ocr.range.custom")}
                  </button>
                ))}
              </div>
              {rangeMode === "custom" && (
                <>
                  <input
                    className="field ocr-range-input"
                    value={rangeText}
                    placeholder={t("ocr.range.placeholder")}
                    aria-label={t("ocr.range.custom")}
                    aria-invalid={rangeInvalid || undefined}
                    onChange={(e) => setRangeText(e.target.value)}
                  />
                  {rangeInvalid && <p className="ocr-error">{t("ocr.range.invalid")}</p>}
                </>
              )}
            </section>

            <section className="ocr-section">
              <h3 className="ocr-label">{t("ocr.language")}</h3>
              <div className="ocr-chips">
                <button
                  type="button"
                  className="chip"
                  data-active={ko || undefined}
                  aria-pressed={ko}
                  onClick={() => setKo((v) => (en ? !v : true))}
                >
                  {t("ocr.language.ko")}
                </button>
                <button
                  type="button"
                  className="chip"
                  data-active={en || undefined}
                  aria-pressed={en}
                  onClick={() => setEn((v) => (ko ? !v : true))}
                >
                  {t("ocr.language.en")}
                </button>
              </div>
            </section>

            <section className="ocr-section">
              <h3 className="ocr-label">{t("ocr.output")}</h3>
              <p className="ocr-output">{t("ocr.output.searchable")}</p>
              <p className="ocr-hint">{t("ocr.output.searchableHint")}</p>
            </section>

            <section className="ocr-section">
              <h3 className="ocr-label">{t("ocr.options")}</h3>
              {visionAvailable && (
                <>
                  <label className="ocr-row">
                    <span className="ocr-row-label">{t("ocr.engine")}</span>
                    <select
                      className="field"
                      value={engineChoice}
                      onChange={(e) => setEngineChoice(e.target.value as OcrEngineChoice)}
                    >
                      {ENGINE_CHOICES.map((c) => <option key={c.id} value={c.id}>{t(c.labelKey)}</option>)}
                    </select>
                  </label>
                  {engineChoice === "auto" && <p className="ocr-hint">{t("ocr.engine.autoHint")}</p>}
                </>
              )}
              <label className="ocr-check">
                <input type="checkbox" checked={skipText} onChange={(e) => setSkipText(e.target.checked)} />
                <span>{t("ocr.option.skipText")}</span>
              </label>
              <label className="ocr-row">
                <span className="ocr-row-label">{t("ocr.option.dpi")}</span>
                <select
                  className="field"
                  value={String(dpi)}
                  onChange={(e) => setDpi(e.target.value === "auto" ? "auto" : (Number(e.target.value) as OcrDpi))}
                >
                  {DPI_CHOICES.map((d) => (
                    <option key={String(d)} value={String(d)}>
                      {d === "auto" ? t("ocr.dpi.auto") : `${d} DPI`}
                    </option>
                  ))}
                </select>
              </label>
              <button
                type="button"
                className="ocr-disclosure"
                aria-expanded={advanced}
                onClick={() => setAdvanced((v) => !v)}
              >
                <span className="ocr-caret" data-open={advanced || undefined}>›</span>
                {t("settings.tab.advanced")}
              </button>
              {advanced && engine === "tesseract" && (
                <label className="ocr-row">
                  <span className="ocr-row-label">{t("ocr.layout")}</span>
                  <select
                    className="field"
                    value={layout}
                    onChange={(e) => setLayout(e.target.value as OcrLayout)}
                  >
                    {LAYOUTS.map((l) => <option key={l.id} value={l.id}>{t(l.labelKey)}</option>)}
                  </select>
                </label>
              )}
            </section>
          </div>
        )}

        {phase === "running" && (
          <div className="ocr-body ocr-progress-body">
            <div className="ocr-progress-row">
              {docId && activePage !== null && (
                <img
                  className="ocr-thumb"
                  alt=""
                  src={thumbUrl({ doc: docId, gen: docGeneration, page: activePage, w: 96 })}
                />
              )}
              <div className="ocr-progress-text">
                <p className="ocr-progress-count">
                  {t("ocr.progress", { current: Math.min(done + 1, total), total })}
                </p>
                <p className="ocr-hint">
                  {t("ocr.elapsed", { seconds: elapsed })}
                  {remaining !== null ? ` · ${t("ocr.remaining", { seconds: Math.round(remaining / 1000) })}` : ""}
                </p>
              </div>
            </div>
            <div className="ocr-bar" role="progressbar" aria-valuemin={0} aria-valuemax={100} aria-valuenow={percent}>
              <div className="ocr-bar-fill" style={{ width: `${percent}%` }} />
            </div>
          </div>
        )}

        {phase === "finished" && result && (
          <div className="ocr-body">
            <div className="banner ocr-result" data-tone={result.status === "done" ? "ok" : result.status}>
              <p className="ocr-result-title">{t(result.messageKey)}</p>
              {result.applied.length > 0 && (
                <p className="ocr-hint">
                  {t("ocr.doneCount", { count: result.applied.length })}
                  {" · "}
                  {t("ocr.result.words", { words: result.words, confidence: result.confidence })}
                </p>
              )}
              {result.skipped.length > 0 && (
                <p className="ocr-hint">{t("ocr.skipped", { count: result.skipped.length })}</p>
              )}
              {result.detailKey && <p className="ocr-hint">{t(result.detailKey)}</p>}
            </div>
          </div>
        )}

        <footer className="ocr-footer">
          {phase === "setup" && (
            <span className="ocr-estimate">
              {pages && pages.length > 0 ? t("ocr.estimate", { seconds }) : ""}
            </span>
          )}
          {phase === "running" && <span className="ocr-estimate">{t("status.ocr")}</span>}
          <div className="ocr-actions">
            {phase === "setup" && (
              <>
                <button type="button" className="btn quiet" onClick={onClose}>{t("common.cancel")}</button>
                <button type="button" className="btn primary" disabled={!canStart} onClick={start}>
                  {t("ocr.start")}
                </button>
              </>
            )}
            {phase === "running" && (
              <button type="button" className="btn" onClick={() => jobId !== null && void cancel(jobId)}>
                {t("common.cancel")}
              </button>
            )}
            {phase === "finished" && (
              <>
                {result && result.applied.length > 0 && (
                  <button type="button" className="btn quiet" onClick={undoAll}>{t("common.undo")}</button>
                )}
                <button type="button" className="btn primary" onClick={onClose}>{t("common.done")}</button>
              </>
            )}
          </div>
        </footer>
      </div>
    </div>
  );
}
