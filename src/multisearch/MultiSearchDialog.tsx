/**
 * 여러 파일에서 검색 (P2, 640 px — 편집 ▸ 여러 파일에서 검색…, ⋯, the 검색 panel): 파일 추가… /
 * 폴더 추가… (every PDF below it) / 목록 비우기, the query with 대소문자 구분 and 단어 단위로, then
 * one job over every file (`flow.ts`). Results are grouped by file with a count per file and
 * ±40 characters of context per hit; a click opens that file at the hit. 닫기 only hides the
 * dialog — a running search carries on in the status bar. Lazy-loaded from `DialogHost`.
 */
import { useState } from "react";
import { ChevronDown, ChevronRight, X } from "lucide-react";
import * as api from "../ipc/api";
import type { SearchHit } from "../ipc/types";
import { useT } from "../i18n/useT";
import { Dialog, Row } from "../dialogs/Dialog";
import {
  addFiles, addFolder, cancelSearch, clearFiles, openResult, removeFile, setOptions, setQuery, startSearch,
  summarize, useMultiSearch, type SearchFile,
} from "./flow";
import "./multiSearch.css";

/** Hits shown per file before 더 보기. */
const PER_FILE = 50;

export default function MultiSearchDialog({ onClose }: { onClose(): void }) {
  const t = useT();
  const { files, query, options, phase, runDone, runTotal, cancelling, searched } = useMultiSearch();
  const running = phase === "running";
  const canStart = !running && files.length > 0 && query.trim().length > 0;
  const summary = summarize(files);

  const pickFiles = async () => {
    const picked = await api.openFileDialog({ multiple: true, title: t("multiSearch.pickFiles") });
    if (picked?.length) addFiles(picked);
  };
  const pickFolder = async () => {
    const picked = await api.openFileDialog({ directory: true, title: t("multiSearch.pickFolder") });
    if (picked?.length) await addFolder(picked[0]);
  };

  const current = files.find((f) => f.status === "opening" || f.status === "searching");

  return (
    <Dialog
      titleKey="multiSearch.title"
      size="lg"
      onClose={onClose}
      cancelKey="common.close"
      primary={{ labelKey: "multiSearch.start", onSelect: () => void startSearch(), disabled: !canStart }}
    >
      <div className="inline-row">
        <input
          className="field grow"
          type="search"
          value={query}
          placeholder={t("sidebar.search.placeholder")}
          aria-label={t("multiSearch.query")}
          autoComplete="off"
          spellCheck={false}
          disabled={running}
          onChange={(e) => setQuery(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && !e.nativeEvent.isComposing && canStart) {
              e.preventDefault();
              void startSearch();
            }
          }}
        />
      </div>
      <div className="search-options">
        <label className="search-toggle text-sm">
          <input
            type="checkbox"
            checked={options.matchCase}
            disabled={running}
            onChange={(e) => setOptions({ matchCase: e.target.checked })}
          />
          {t("sidebar.search.matchCase")}
        </label>
        <label className="search-toggle text-sm">
          <input
            type="checkbox"
            checked={options.wholeWord}
            disabled={running}
            onChange={(e) => setOptions({ wholeWord: e.target.checked })}
          />
          {t("sidebar.search.wholeWord")}
        </label>
      </div>

      <Row labelKey="multiSearch.files">
        <div className="inline-row">
          <button type="button" className="btn" disabled={running} onClick={() => void pickFiles()}>
            {t("multiSearch.addFiles")}
          </button>
          <button type="button" className="btn" disabled={running} onClick={() => void pickFolder()}>
            {t("multiSearch.addFolder")}
          </button>
          <button type="button" className="btn quiet" disabled={running || files.length === 0} onClick={clearFiles}>
            {t("batchOcr.clear")}
          </button>
          <span className="grow" />
          <span className="text-sm dim">{t("batchOcr.count", { count: files.length })}</span>
        </div>
      </Row>

      <div className="msearch-list">
        {files.length === 0 ? (
          <p className="msearch-empty text-sm">{t("multiSearch.empty")}</p>
        ) : (
          <ul>
            {files.map((file) => (
              <FileResults key={file.id} file={file} running={running} />
            ))}
          </ul>
        )}
      </div>

      {running && (
        <div className="compress-run">
          <progress className="job-bar" value={runDone} max={Math.max(1, runTotal)} aria-label={t("multiSearch.running")} />
          <span className="text-sm mono" aria-live="polite">
            {t("batchOcr.progress", { done: Math.min(runDone + 1, runTotal), total: runTotal })}
            {current ? ` · ${current.name}` : ""}
          </span>
          <button type="button" className="btn" disabled={cancelling} onClick={() => void cancelSearch()}>
            {t("common.cancel")}
          </button>
        </div>
      )}

      {phase === "finished" && searched && (
        <p className="banner text-sm" role="status" data-testid="msearch-summary">
          {t("multiSearch.summary", { hits: summary.hits, files: summary.withHits, skipped: summary.skipped, failed: summary.failed })}
        </p>
      )}
    </Dialog>
  );
}

function FileResults({ file, running }: { file: SearchFile; running: boolean }) {
  const t = useT();
  const [open, setOpen] = useState(true);
  const [limit, setLimit] = useState(PER_FILE);
  const hits = file.hits;
  const Chevron = open ? ChevronDown : ChevronRight;
  return (
    <li className="msearch-file" data-testid="msearch-file" data-status={file.status}>
      <div className="msearch-file-head">
        <button
          type="button"
          className="icon-btn"
          aria-expanded={open}
          aria-label={t(open ? "annot.thread.collapse" : "annot.thread.expand", { count: hits.length })}
          disabled={hits.length === 0}
          onClick={() => setOpen((v) => !v)}
        >
          <Chevron size={14} strokeWidth={1.75} aria-hidden />
        </button>
        <span className="msearch-name text-sm" title={file.path}>{file.name}</span>
        <span className="msearch-status text-xs" data-status={file.status} title={file.detail}>
          {statusText(file, t)}
        </span>
        <button
          type="button"
          className="icon-btn"
          disabled={running}
          aria-label={t("batchOcr.remove", { name: file.name })}
          onClick={() => removeFile(file.id)}
        >
          <X size={14} strokeWidth={1.75} aria-hidden />
        </button>
      </div>
      {open && hits.length > 0 && (
        <ol className="msearch-hits">
          {hits.slice(0, limit).map((hit, index) => (
            <li key={`${hit.page}:${hit.charStart}`}>
              <button
                type="button"
                className="search-result"
                data-testid="msearch-hit"
                aria-label={t("multiSearch.openHit", { name: file.name, page: hit.page + 1 })}
                disabled={running}
                onClick={() => void openResult(file.id, index)}
              >
                <span className="msearch-page text-xs dim mono">{t("sidebar.search.pageLabel", { page: hit.page + 1 })}</span>
                <Context hit={hit} />
              </button>
            </li>
          ))}
          {hits.length > limit && (
            <li>
              <button type="button" className="btn quiet" onClick={() => setLimit((n) => n + PER_FILE)}>
                {t("common.more")}
              </button>
            </li>
          )}
        </ol>
      )}
    </li>
  );
}

function statusText(file: SearchFile, t: ReturnType<typeof useT>): string {
  switch (file.status) {
    case "queued":
      return t("batchOcr.status.queued");
    case "opening":
      return t("batchOcr.status.opening");
    case "searching":
      return t("sidebar.search.searching");
    case "done":
      return t("sidebar.search.results", { count: file.hits.length });
    case "cancelled":
      return t("batchOcr.status.cancelled");
    case "skipped":
    case "failed": {
      const label = t(file.status === "skipped" ? "batchOcr.status.skipped" : "batchOcr.status.failed");
      return file.reasonKey ? `${label} · ${t(file.reasonKey)}` : label;
    }
  }
}

/** pdfium's page text carries generated control characters; they render as tofu in a list. */
function clean(text: string): string {
  return text.replace(/[\u0000-\u001f\u007f]+/g, " ");
}

function Context({ hit }: { hit: SearchHit }) {
  const [start, length] = hit.contextMatch;
  return (
    <span className="text-sm">
      {clean(hit.context.slice(0, start))}
      <mark>{clean(hit.context.slice(start, start + length))}</mark>
      {clean(hit.context.slice(start + length))}
    </span>
  );
}
