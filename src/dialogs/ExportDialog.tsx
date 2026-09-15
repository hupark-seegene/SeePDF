/**
 * 내보내기 (UI_SPEC §10, F-24/F-25): format list on the left, options on the right, an estimated
 * size from `estimate_export`, then progress in the status-bar job slot and a completion toast with
 * Finder에서 보기.
 */
import { useEffect, useMemo, useState } from "react";
import { FileText, Image as ImageIcon, Layers } from "lucide-react";
import { useT } from "../i18n/useT";
import { formatBytes } from "../i18n";
import * as api from "../ipc/api";
import { useDocStore } from "../store/docStore";
import { useViewStore } from "../store/viewStore";
import { usePagesStore } from "../store/pagesStore";
import { useJobStore } from "../store/jobStore";
import { toast } from "../app/toastStore";
import { Dialog, Row } from "./Dialog";
import { RangePicker } from "./RangePicker";
import { resolveRange, type RangeChoice } from "./pageRange";
import { dirName, message, revealAction, suggestName } from "./flows";
import type { JobEvent, PageIndex } from "../ipc/types";

type Format = "pdfFlattened" | "png" | "jpeg" | "text";

const FORMATS: { id: Format; labelKey: string; icon: typeof FileText }[] = [
  { id: "pdfFlattened", labelKey: "export.format.pdfFlattened", icon: Layers },
  { id: "png", labelKey: "export.format.png", icon: ImageIcon },
  { id: "jpeg", labelKey: "export.format.jpeg", icon: ImageIcon },
  { id: "text", labelKey: "export.format.text", icon: FileText },
];

export function ExportDialog({ onClose }: { onClose(): void }) {
  const t = useT();
  const info = useDocStore((s) => s.info);
  const currentPage = useViewStore((s) => s.currentPage);
  const selected = usePagesStore((s) => s.selected);
  const [format, setFormat] = useState<Format>("png");
  const [range, setRange] = useState<RangeChoice>({ mode: "all", text: "" });
  const [dpi, setDpi] = useState(150);
  const [quality, setQuality] = useState(85);
  const [transparent, setTransparent] = useState(false);
  const [annotations, setAnnotations] = useState(true);
  const [forms, setForms] = useState(true);
  const [estimate, setEstimate] = useState<number | null>(null);
  const [busy, setBusy] = useState(false);

  const pageCount = info?.pageCount ?? 0;
  const pages = useMemo(
    () => resolveRange(range, { pageCount, currentPage, selected }),
    [range, pageCount, currentPage, selected],
  );
  const isImage = format === "png" || format === "jpeg";

  // The size estimate is what stops people writing 1.17 MB/page PNGs into a temp folder (spike §16).
  useEffect(() => {
    if (!info || !isImage || !pages?.length) {
      setEstimate(null);
      return;
    }
    let live = true;
    const timer = window.setTimeout(() => {
      void api
        .estimateExport({ docId: info.docId, pages, format: format as "png" | "jpeg", dpi })
        .then((r) => live && setEstimate(r.bytes))
        .catch(() => live && setEstimate(null));
    }, 120);
    return () => {
      live = false;
      window.clearTimeout(timer);
    };
  }, [info, isImage, format, dpi, pages]);

  const run = async () => {
    if (!info || !pages?.length) return;
    setBusy(true);
    try {
      if (isImage) await runImages(info.docId, pages, format as "png" | "jpeg");
      else if (format === "text") await runText(info.docId, pages);
      else await runFlattened(info.docId, pages);
    } catch (e) {
      toast("export.failed", undefined, { tone: "danger", detail: message(e) });
    } finally {
      setBusy(false);
      onClose();
    }
  };

  const runImages = async (docId: string, list: PageIndex[], fmt: "png" | "jpeg") => {
    const dir = await api.openFileDialog({ directory: true, multiple: false });
    if (!dir?.length) return;
    const outDir = dir[0];
    const baseName = stem(info?.name ?? "document");
    const jobs = useJobStore.getState();
    await api.exportImages(
      {
        docId, pages: list, format: fmt, dpi, quality: fmt === "jpeg" ? quality : undefined,
        outDir, baseName, transparentBackground: fmt === "png" ? transparent : undefined,
      },
      (e: JobEvent) => {
        jobs.apply("export", "export.title", e);
        if (e.type === "done") {
          toast("export.done", { name: baseName }, { tone: "success", actions: [revealAction(outDir)] });
        }
        if (e.type === "error") toast("export.failed", undefined, { tone: "danger", detail: e.error.message });
      },
    );
  };

  const runText = async (docId: string, list: PageIndex[]) => {
    const outPath = await api.saveFileDialog({ defaultPath: `${stem(info?.name ?? "document")}.txt` });
    if (!outPath) return;
    await api.exportText({ docId, pages: list, outPath });
    toast("export.done", { name: baseNameOf(outPath) }, { tone: "success", actions: [revealAction(dirName(outPath))] });
  };

  const runFlattened = async (docId: string, list: PageIndex[]) => {
    const outPath = await api.saveFileDialog({ defaultPath: suggestName(info?.name ?? "document.pdf", "flat") });
    if (!outPath) return;
    const jobs = useJobStore.getState();
    await api.exportFlattened({ docId, outPath, annotations, forms, pages: list }, (e: JobEvent) => {
      jobs.apply("export", "export.title", e);
      if (e.type === "done") {
        toast("export.done", { name: baseNameOf(outPath) }, { tone: "success", actions: [revealAction(dirName(outPath))] });
      }
    });
  };

  return (
    <Dialog
      titleKey="export.title"
      size="xl"
      onClose={onClose}
      primary={{ labelKey: "export.button", onSelect: () => void run(), disabled: busy || !pages?.length }}
    >
      <div className="export-grid">
        <ul className="format-list" role="listbox" aria-label={t("export.format")}>
          {FORMATS.map((f) => (
            <li key={f.id}>
              <button
                type="button"
                role="option"
                aria-selected={format === f.id}
                data-active={format === f.id || undefined}
                onClick={() => setFormat(f.id)}
              >
                <f.icon size={16} strokeWidth={1.75} aria-hidden />
                {t(f.labelKey)}
              </button>
            </li>
          ))}
        </ul>

        <div className="export-options">
          <Row labelKey="pages.range">
            <RangePicker value={range} onChange={setRange} pageCount={pageCount} selectedCount={selected.length} />
          </Row>

          {isImage && (
            <>
              <Row labelKey="export.dpi">
                <div className="inline-row">
                  <input
                    className="slider"
                    type="range"
                    min={72}
                    max={600}
                    step={1}
                    value={dpi}
                    aria-label={t("export.dpi")}
                    onChange={(e) => setDpi(Number(e.target.value))}
                  />
                  <span className="text-sm mono">{dpi}</span>
                </div>
              </Row>
              {format === "jpeg" && (
                <Row labelKey="export.quality">
                  <div className="inline-row">
                    <input
                      className="slider"
                      type="range"
                      min={40}
                      max={100}
                      step={1}
                      value={quality}
                      aria-label={t("export.quality")}
                      onChange={(e) => setQuality(Number(e.target.value))}
                    />
                    <span className="text-sm mono">{quality}</span>
                  </div>
                </Row>
              )}
              {format === "png" && (
                <label className="dlg-check text-base">
                  <input type="checkbox" checked={transparent} onChange={(e) => setTransparent(e.target.checked)} />
                  <span>{t("export.transparentBg")}</span>
                </label>
              )}
              <p className="dlg-hint text-xs">{t("export.onePerPage")}</p>
            </>
          )}

          {format === "pdfFlattened" && (
            <>
              <label className="dlg-check text-base">
                <input type="checkbox" checked={annotations} onChange={(e) => setAnnotations(e.target.checked)} />
                <span>{t("export.flattenAnnotations")}</span>
              </label>
              <label className="dlg-check text-base">
                <input type="checkbox" checked={forms} onChange={(e) => setForms(e.target.checked)} />
                <span>{t("export.flattenForms")}</span>
              </label>
            </>
          )}

          <p className="export-estimate text-sm" aria-live="polite">
            {estimate !== null ? t("export.estimatedSize", { size: formatBytes(estimate) }) : ""}
          </p>
        </div>
      </div>
    </Dialog>
  );
}

function stem(name: string): string {
  const dot = name.lastIndexOf(".");
  return dot > 0 ? name.slice(0, dot) : name;
}

function baseNameOf(path: string): string {
  const parts = path.split(/[\\/]/);
  return parts[parts.length - 1] || path;
}
