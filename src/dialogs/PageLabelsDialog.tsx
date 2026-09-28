/**
 * 페이지 레이블 (P2): the document's /PageLabels as a list of ranges — 시작 페이지, 스타일, 접두사,
 * 시작 번호 — with a live preview of every page's label. 적용 writes them with `set_page_labels`
 * (one undo step, 실행 취소 in the toast); 모두 제거 + 적용 removes them. Opened from 페이지 mode's
 * rail and from 문서 정보; lazy-loaded from `DialogHost`.
 *
 * An encrypted document is read-only here: the engine writes /PageLabels with lopdf, which would
 * have to re-encrypt the file (`unsupported`, as for 메타데이터 편집).
 */
import { useEffect, useMemo, useState } from "react";
import { Plus, X } from "lucide-react";
import * as api from "../ipc/api";
import type { PageLabelRange, PageLabelStyle } from "../ipc/types";
import { useT } from "../i18n/useT";
import { useDocStore } from "../store/docStore";
import { toast } from "../app/toastStore";
import { Dialog } from "./Dialog";
import { message } from "./flows";
import {
  LABEL_STYLES, labelsFor, newRow, rangesFromRows, rowError, rowsFromRanges, rowsValid, sameRanges, type LabelRow,
} from "./pageLabels";
import "./pageLabels.css";

export default function PageLabelsDialog({ onClose }: { onClose(): void }) {
  const t = useT();
  const info = useDocStore((s) => s.info);
  const [initial, setInitial] = useState<PageLabelRange[] | null>(null);
  const [rows, setRows] = useState<LabelRow[]>([]);
  const [busy, setBusy] = useState(false);
  const docId = info?.docId;
  const encrypted = info?.encrypted ?? false;

  useEffect(() => {
    if (!docId) return;
    let live = true;
    const load = encrypted ? Promise.resolve<PageLabelRange[]>([]) : api.getPageLabels({ docId }).catch(() => [] as PageLabelRange[]);
    void load.then((ranges) => {
      if (!live) return;
      setInitial(ranges);
      setRows(rowsFromRanges(ranges));
    });
    return () => {
      live = false;
    };
  }, [docId, encrypted]);

  const pageCount = info?.pageCount ?? 0;
  const valid = rowsValid(rows, pageCount);
  const ranges = useMemo(() => (valid ? rangesFromRows(rows) : null), [rows, valid]);
  const preview = useMemo(() => (ranges ? labelsFor(ranges, pageCount) : []), [ranges, pageCount]);
  const changed = !!ranges && !!initial && !sameRanges(ranges, initial);
  if (!info) return null;
  const leading = !!ranges?.length && ranges[0].start > 0;

  const patch = (key: number, p: Partial<LabelRow>) => setRows((prev) => prev.map((r) => (r.key === key ? { ...r, ...p } : r)));

  function add() {
    setRows((prev) => {
      if (!prev.length) return [newRow(1, "roman")];
      const last = Math.max(...prev.map((r) => Number(r.start) || 1));
      return [...prev, newRow(Math.min(pageCount, last + 1), "decimal")];
    });
  }

  async function apply() {
    if (!info || !ranges) return;
    setBusy(true);
    try {
      const next = await api.setPageLabels({ docId: info.docId, ranges });
      useDocStore.getState().adopt(next);
      toast(ranges.length ? "pageLabels.applied" : "pageLabels.removed", undefined, {
        tone: "success",
        actions: [{ labelKey: "common.undo", onSelect: () => void import("../annot/sync").then((m) => m.undoWithAnnots()) }],
      });
      onClose();
    } catch (e) {
      setBusy(false);
      const unsupported = api.isSeePdfError(e) && e.code === "unsupported";
      toast(unsupported ? "structure.encrypted" : "error.generic", undefined, { tone: "danger", detail: message(e) });
    }
  }

  return (
    <Dialog
      titleKey="pageLabels.title"
      size="lg"
      onClose={onClose}
      primary={{ labelKey: "common.apply", onSelect: () => void apply(), disabled: !changed || !valid || encrypted || busy }}
      footerExtra={
        <button type="button" className="btn quiet" disabled={!rows.length || encrypted || busy} onClick={() => setRows([])}>
          {t("pageLabels.removeAll")}
        </button>
      }
    >
      {encrypted && <p className="dlg-hint text-xs">{t("structure.encrypted")}</p>}
      <div className="labels-editor">
        {rows.length > 0 && (
          <div className="labels-grid" role="table" aria-label={t("pageLabels.title")}>
            <div className="labels-head text-xs dim" role="row">
              <span role="columnheader">{t("pageLabels.start")}</span>
              <span role="columnheader">{t("pageLabels.style")}</span>
              <span role="columnheader">{t("pageLabels.prefix")}</span>
              <span role="columnheader">{t("pageLabels.first")}</span>
              <span />
            </div>
            {rows.map((row, i) => {
              const error = rowError(row, rows, pageCount);
              return (
                <div className="labels-row" role="row" key={row.key}>
                  <input
                    className="field mono"
                    inputMode="numeric"
                    aria-label={`${t("pageLabels.start")} ${i + 1}`}
                    aria-invalid={error === "start" || error === "duplicate" || undefined}
                    value={row.start}
                    disabled={encrypted}
                    onChange={(e) => patch(row.key, { start: e.target.value.replace(/[^\d]/g, "") })}
                  />
                  <select
                    className="field"
                    aria-label={`${t("pageLabels.style")} ${i + 1}`}
                    value={row.style}
                    disabled={encrypted}
                    onChange={(e) => patch(row.key, { style: e.target.value as PageLabelStyle })}
                  >
                    {LABEL_STYLES.map((s) => (
                      <option key={s} value={s}>
                        {t(`pageLabels.style.${s}`)}
                      </option>
                    ))}
                  </select>
                  <input
                    className="field"
                    aria-label={`${t("pageLabels.prefix")} ${i + 1}`}
                    value={row.prefix}
                    disabled={encrypted}
                    onChange={(e) => patch(row.key, { prefix: e.target.value })}
                  />
                  <input
                    className="field mono"
                    inputMode="numeric"
                    placeholder="1"
                    aria-label={`${t("pageLabels.first")} ${i + 1}`}
                    aria-invalid={error === "first" || undefined}
                    value={row.first}
                    disabled={encrypted || row.style === "none"}
                    onChange={(e) => patch(row.key, { first: e.target.value.replace(/[^\d]/g, "") })}
                  />
                  <button
                    type="button"
                    className="icon-btn"
                    aria-label={`${t("pageLabels.remove")} ${i + 1}`}
                    disabled={encrypted}
                    onClick={() => setRows((prev) => prev.filter((r) => r.key !== row.key))}
                  >
                    <X size={14} strokeWidth={1.75} aria-hidden />
                  </button>
                  {error && (
                    <p className="labels-error text-xs" role="alert">
                      {t(`pageLabels.error.${error}`, { count: pageCount })}
                    </p>
                  )}
                </div>
              );
            })}
          </div>
        )}
        {rows.length === 0 && initial !== null && <p className="dlg-hint text-sm">{t("pageLabels.empty")}</p>}
        <button type="button" className="btn quiet labels-add" disabled={encrypted || initial === null} onClick={add}>
          <Plus size={14} strokeWidth={1.75} aria-hidden />
          {t("pageLabels.add")}
        </button>
        {leading && <p className="dlg-hint text-xs">{t("pageLabels.leadingHint")}</p>}
      </div>

      <h3 className="field-label text-xs labels-preview-title">{t("pageLabels.preview")}</h3>
      <ol className="labels-preview" aria-label={t("pageLabels.preview")}>
        {Array.from({ length: pageCount }, (_, page) => (
          <li key={page} className="labels-preview-item">
            <span className="text-xs dim mono">{page + 1}</span>
            <span className="text-sm mono">{preview.length ? preview[page] || "—" : page + 1}</span>
          </li>
        ))}
      </ol>
    </Dialog>
  );
}
