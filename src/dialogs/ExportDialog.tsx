/**
 * 내보내기 (UI_SPEC §10, F-24/F-25): format list on the left, options on the right, an estimated
 * size from `estimate_export`, then progress in the status-bar job slot and a completion toast with
 * Finder에서 보기.
 *
 * P2 주석 목록: every annotation of the range as TXT, CSV (Excel, UTF-8 with BOM) or Markdown
 * (`export_annotation_summary`). The 주석 sidebar's 내보내기… opens the dialog on this format.
 *
 * v0.3 pkg8: N-up PDF (모아찍기 / 소책자, `make_nup`, X2); PNG / JPEG 하나의 이미지로 이어 붙이기,
 * 여러 페이지 TIFF, 이미지 추출 and 텍스트 ▸ 레이아웃 유지 (X3); Word / 한글 / HTML / Markdown text
 * flow — lossy, said so in the dialog (X6).
 */
import { useEffect, useMemo, useState } from "react";
import { FileCode2, FileText, FileType2, Grid2x2, Image as ImageIcon, Images, Layers, MessageSquareText } from "lucide-react";
import { useT } from "../i18n/useT";
import { formatBytes, getLocale } from "../i18n";
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
import { reasonKey, type PermKey } from "../app/permissions";
import type { JobEvent, NupOptions, PageIndex, SummaryFormat, TextFlowFormat } from "../ipc/types";

export type ExportFormat =
  | "pdfFlattened" | "png" | "jpeg" | "text" | "annotations"
  // v0.3 pkg8
  | "nup" | "tiff" | "embedded" | "docx" | "hwpx" | "html" | "md";
type Format = ExportFormat;

const FORMATS: { id: Format; labelKey: string; icon: typeof FileText }[] = [
  { id: "pdfFlattened", labelKey: "export.format.pdfFlattened", icon: Layers },
  { id: "nup", labelKey: "export.format.nup", icon: Grid2x2 },
  { id: "png", labelKey: "export.format.png", icon: ImageIcon },
  { id: "jpeg", labelKey: "export.format.jpeg", icon: ImageIcon },
  { id: "tiff", labelKey: "export.format.tiff", icon: ImageIcon },
  { id: "embedded", labelKey: "export.format.embedded", icon: Images },
  { id: "text", labelKey: "export.format.text", icon: FileText },
  { id: "docx", labelKey: "export.format.docx", icon: FileType2 },
  { id: "hwpx", labelKey: "export.format.hwpx", icon: FileType2 },
  { id: "html", labelKey: "export.format.html", icon: FileCode2 },
  { id: "md", labelKey: "export.format.md", icon: FileCode2 },
  { id: "annotations", labelKey: "export.format.annotations", icon: MessageSquareText },
];

const SUMMARY_FORMATS: SummaryFormat[] = ["csv", "txt", "md"];
const SUMMARY_FILTER: Record<SummaryFormat, { name: string; extensions: string[] }> = {
  csv: { name: "CSV", extensions: ["csv"] },
  txt: { name: "Text", extensions: ["txt"] },
  md: { name: "Markdown", extensions: ["md"] },
};

/** The text-flow formats (X6) and their file extension / filter name. */
const FLOW: Record<TextFlowFormat, { ext: string; name: string }> = {
  docx: { ext: "docx", name: "Word" },
  hwpx: { ext: "hwpx", name: "HWPX" },
  html: { ext: "html", name: "HTML" },
  md: { ext: "md", name: "Markdown" },
};
const isFlow = (f: Format): f is TextFlowFormat => f === "docx" || f === "hwpx" || f === "html" || f === "md";

const PER_SHEET: NupOptions["perSheet"][] = [2, 4, 6, 9];

/**
 * v0.3 integration (pkg8 × pkg3 S5): the permission a format needs, as the engine checks it —
 * text and the formats that carry text / images out need "copy", 모아찍기 is built from the print
 * bytes (and keeps the source's encryption and permission bits). Page images and the flattened PDF
 * are not gated (like `export_images`). The annotation summary is not gated either: without
 * "copy" the engine exports the comments and replies but leaves the quoted-text column empty.
 */
export function exportNeeds(format: ExportFormat): PermKey | null {
  if (format === "nup") return "print";
  if (format === "text" || format === "embedded" || isFlow(format)) return "extractText";
  return null;
}

export function ExportDialog({ onClose, initialFormat }: { onClose(): void; initialFormat?: ExportFormat }) {
  const t = useT();
  const info = useDocStore((s) => s.info);
  const currentPage = useViewStore((s) => s.currentPage);
  const selected = usePagesStore((s) => s.selected);
  const [format, setFormat] = useState<Format>(initialFormat ?? "png");
  const [summaryFormat, setSummaryFormat] = useState<SummaryFormat>("csv");
  const [range, setRange] = useState<RangeChoice>({ mode: "all", text: "" });
  const [dpi, setDpi] = useState(150);
  const [quality, setQuality] = useState(85);
  const [transparent, setTransparent] = useState(false);
  const [annotations, setAnnotations] = useState(true);
  const [forms, setForms] = useState(true);
  // v0.3 pkg8
  const [stitch, setStitch] = useState(false);
  const [preserveLayout, setPreserveLayout] = useState(false);
  const [perSheet, setPerSheet] = useState<NupOptions["perSheet"]>(2);
  const [order, setOrder] = useState<"across" | "down">("across");
  const [booklet, setBooklet] = useState(false);
  const [paper, setPaper] = useState<"auto" | "a4" | "letter">("a4");
  const [estimate, setEstimate] = useState<number | null>(null);
  const [busy, setBusy] = useState(false);

  const pageCount = info?.pageCount ?? 0;
  const pages = useMemo(
    () => resolveRange(range, { pageCount, currentPage, selected }),
    [range, pageCount, currentPage, selected],
  );
  const isImage = format === "png" || format === "jpeg";
  const usesDpi = isImage || format === "tiff";
  const need = exportNeeds(format);
  const blocked = need && info && !info.permissions[need] ? reasonKey(need) : null;

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
      if (isImage && stitch) await runStitched(info.docId, pages, format as "png" | "jpeg");
      else if (isImage) await runImages(info.docId, pages, format as "png" | "jpeg");
      else if (format === "text") await runText(info.docId, pages);
      else if (format === "annotations") await runAnnotations(info.docId, pages);
      else if (format === "nup") await runNup(info.docId, pages);
      else if (format === "tiff") await runTiff(info.docId, pages);
      else if (format === "embedded") await runEmbedded(info.docId, pages);
      else if (isFlow(format)) await runFlow(info.docId, pages, format);
      else await runFlattened(info.docId, pages);
    } catch (e) {
      toast("export.failed", undefined, { tone: "danger", detail: message(e) });
    } finally {
      setBusy(false);
      onClose();
    }
  };

  /** Status-bar progress and the completion toast of a one-file job. */
  const fileJob = (outPath: string) => {
    const jobs = useJobStore.getState();
    return (e: JobEvent) => {
      jobs.apply("export", "export.title", e);
      if (e.type === "done") {
        toast("export.done", { name: baseNameOf(outPath) }, { tone: "success", actions: [revealAction(dirName(outPath))] });
      }
      if (e.type === "error") toast("export.failed", undefined, { tone: "danger", detail: e.error.message });
    };
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

  const runStitched = async (docId: string, list: PageIndex[], fmt: "png" | "jpeg") => {
    const ext = fmt === "png" ? "png" : "jpg";
    const outPath = await api.saveFileDialog({
      defaultPath: `${stem(info?.name ?? "document")}.${ext}`,
      filters: [{ name: fmt.toUpperCase(), extensions: [ext] }],
    });
    if (!outPath) return;
    const started = await api.exportStitchedImage({ docId, pages: list, dpi, format: fmt, outPath }, fileJob(outPath));
    if (started.lowered) toast("export.stitch.lowered", { dpi: started.dpi }, { tone: "info" });
  };

  const runTiff = async (docId: string, list: PageIndex[]) => {
    const outPath = await api.saveFileDialog({
      defaultPath: `${stem(info?.name ?? "document")}.tiff`,
      filters: [{ name: "TIFF", extensions: ["tiff", "tif"] }],
    });
    if (!outPath) return;
    await api.exportTiff({ docId, pages: list, dpi, outPath }, fileJob(outPath));
  };

  const runEmbedded = async (docId: string, list: PageIndex[]) => {
    const dir = await api.openFileDialog({ directory: true, multiple: false });
    if (!dir?.length) return;
    const outDir = dir[0];
    const baseName = stem(info?.name ?? "document");
    const jobs = useJobStore.getState();
    await api.exportEmbeddedImages({ docId, pages: list, outDir, baseName }, (e: JobEvent) => {
      jobs.apply("export", "export.title", e);
      if (e.type === "done") {
        const count = e.outputs?.length ?? 0;
        if (count === 0) toast("export.embedded.none", undefined, { tone: "info" });
        else toast("export.embedded.done", { count }, { tone: "success", actions: [revealAction(outDir)] });
      }
      if (e.type === "error") toast("export.failed", undefined, { tone: "danger", detail: e.error.message });
    });
  };

  const runNup = async (docId: string, list: PageIndex[]) => {
    const outPath = await api.saveFileDialog({ defaultPath: suggestName(info?.name ?? "document.pdf", booklet ? "booklet" : `${perSheet}up`) });
    if (!outPath) return;
    const all = list.length === (info?.pageCount ?? 0);
    await api.makeNup({
      docId, pages: all ? undefined : list, outPath,
      options: { perSheet: booklet ? 2 : perSheet, order, booklet, paper },
    });
    toast("export.done", { name: baseNameOf(outPath) }, { tone: "success", actions: [revealAction(dirName(outPath))] });
  };

  const runFlow = async (docId: string, list: PageIndex[], fmt: TextFlowFormat) => {
    const outPath = await api.saveFileDialog({
      defaultPath: `${stem(info?.name ?? "document")}.${FLOW[fmt].ext}`,
      filters: [{ name: FLOW[fmt].name, extensions: [FLOW[fmt].ext] }],
    });
    if (!outPath) return;
    await api.exportTextFlow({ docId, pages: list, format: fmt, outPath }, fileJob(outPath));
  };

  const runText = async (docId: string, list: PageIndex[]) => {
    const outPath = await api.saveFileDialog({ defaultPath: `${stem(info?.name ?? "document")}.txt` });
    if (!outPath) return;
    await api.exportText({ docId, pages: list, outPath, preserveLayout: preserveLayout || undefined });
    toast("export.done", { name: baseNameOf(outPath) }, { tone: "success", actions: [revealAction(dirName(outPath))] });
  };

  const runAnnotations = async (docId: string, list: PageIndex[]) => {
    const ext = summaryFormat;
    const outPath = await api.saveFileDialog({
      defaultPath: `${stem(info?.name ?? "document")}-${t("annotSummary.title")}.${ext}`,
      filters: [SUMMARY_FILTER[summaryFormat]],
    });
    if (!outPath) return;
    const all = list.length === (info?.pageCount ?? 0);
    const result = await api.exportAnnotationSummary({
      docId, path: outPath, format: summaryFormat, pages: all ? undefined : list, locale: getLocale(),
    });
    if (result.count === 0) toast("export.annotations.empty", undefined, { tone: "info" });
    else
      toast("export.annotations.done", { count: result.count }, {
        tone: "success",
        actions: [revealAction(dirName(outPath))],
      });
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
      primary={{ labelKey: "export.button", onSelect: () => void run(), disabled: busy || !pages?.length || !!blocked }}
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

          {usesDpi && (
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
          )}

          {isImage && (
            <>
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
              {format === "png" && !stitch && (
                <label className="dlg-check text-base">
                  <input type="checkbox" checked={transparent} onChange={(e) => setTransparent(e.target.checked)} />
                  <span>{t("export.transparentBg")}</span>
                </label>
              )}
              <label className="dlg-check text-base">
                <input type="checkbox" checked={stitch} onChange={(e) => setStitch(e.target.checked)} />
                <span>{t("export.singleImage")}</span>
              </label>
              <p className="dlg-hint text-xs">{t(stitch ? "export.stitch.hint" : "export.onePerPage")}</p>
            </>
          )}

          {format === "tiff" && <p className="dlg-hint text-xs">{t("export.tiff.hint")}</p>}
          {format === "embedded" && <p className="dlg-hint text-xs">{t("export.embedded.hint")}</p>}

          {format === "text" && (
            <label className="dlg-check text-base">
              <input type="checkbox" checked={preserveLayout} onChange={(e) => setPreserveLayout(e.target.checked)} />
              <span>{t("export.preserveLayout")}</span>
            </label>
          )}

          {isFlow(format) && (
            <p className="dlg-hint text-xs" role="note" data-testid="export-lossy">
              {t("export.flow.lossy")}
            </p>
          )}

          {format === "nup" && (
            <>
              <Row labelKey="print.perSheet">
                <div className="inline-row">
                  <select
                    className="field"
                    value={perSheet}
                    disabled={booklet}
                    aria-label={t("print.perSheet")}
                    onChange={(e) => setPerSheet(Number(e.currentTarget.value) as NupOptions["perSheet"])}
                  >
                    {PER_SHEET.map((n) => (
                      <option key={n} value={n}>
                        {t("print.perSheet.n", { count: n })}
                      </option>
                    ))}
                  </select>
                  {!booklet && (
                    <select
                      className="field"
                      value={order}
                      aria-label={t("print.order")}
                      onChange={(e) => setOrder(e.currentTarget.value as "across" | "down")}
                    >
                      <option value="across">{t("print.order.across")}</option>
                      <option value="down">{t("print.order.down")}</option>
                    </select>
                  )}
                </div>
              </Row>
              <Row labelKey="export.nup.paper">
                <select
                  className="field"
                  value={paper}
                  aria-label={t("export.nup.paper")}
                  onChange={(e) => setPaper(e.currentTarget.value as "auto" | "a4" | "letter")}
                >
                  <option value="a4">A4</option>
                  <option value="letter">Letter</option>
                  <option value="auto">{t("export.nup.paperAuto")}</option>
                </select>
              </Row>
              <label className="dlg-check text-base">
                <input type="checkbox" checked={booklet} onChange={(e) => setBooklet(e.target.checked)} />
                <span>{t("print.booklet")}</span>
              </label>
              <p className="dlg-hint text-xs">{t(booklet ? "print.bookletHint" : "export.nup.hint")}</p>
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

          {format === "annotations" && (
            <Row labelKey="export.annotations.format">
              <div className="dlg-radio-group" role="radiogroup" aria-label={t("export.annotations.format")}>
                {SUMMARY_FORMATS.map((f) => (
                  <label key={f} className="dlg-radio text-base">
                    <input
                      type="radio"
                      name="summary-format"
                      checked={summaryFormat === f}
                      onChange={() => setSummaryFormat(f)}
                    />
                    <span>{t(`export.annotations.${f}`)}</span>
                  </label>
                ))}
              </div>
              <p className="dlg-hint text-xs">{t("export.annotations.hint")}</p>
            </Row>
          )}

          <p className="export-estimate text-sm" aria-live="polite" data-testid="export-estimate">
            {blocked
              ? t(blocked)
              : estimate !== null && !stitch
                ? t("export.estimatedSize", { size: formatBytes(estimate) })
                : ""}
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
