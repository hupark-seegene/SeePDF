/**
 * 인쇄 (F-26): pick a range, a method and the v0.3 options, then print.
 *
 * Two methods, both of which put the document on paper (UI_SPEC §10, `flows.runPrint`):
 * the print-only DOM plus the system print panel (default), or the flattened temp file handed
 * to the OS PDF handler.
 *
 * v0.3 (pkg8): 주석 (X7: 문서와 주석 / 문서만 / 문서와 도장·서명), 모아찍기 1/2/4/6/9 with its
 * order and 소책자 (X2, the engine's `make_nup`), and — print-only DOM only — 크기 (맞춤, the X8
 * default, the page as wide as the sheet, / 실제 크기) and 흑백.
 *
 * v0.3.0: 실제 크기 asks for the 용지 it lays the pages out for (`print/paper.ts`, `paperChoice.ts`) and says how
 * the pages will come out — at their real size, and how many are larger than the paper and
 * will be shrunk to fit it rather than spill onto a second sheet.
 */
import { useEffect, useState } from "react";
import { useDocStore } from "../store/docStore";
import { useViewStore } from "../store/viewStore";
import { usePagesStore } from "../store/pagesStore";
import { useT } from "../i18n/useT";
import { Dialog, Row } from "./Dialog";
import { RangePicker } from "./RangePicker";
import { resolveRange, type RangeChoice } from "./pageRange";
import { runPrint, type PrintMethod } from "./flows";
import {
  PRINT_CHUNK, type NupOrder, type PerSheet, type PrintAnnots, type PrintFit,
} from "../print/printStore";
import { defaultPaper, PAPERS, type PaperId } from "../print/paperChoice";

type PrintFlowModule = typeof import("../print/printFlow");

const PER_SHEET: PerSheet[] = [1, 2, 4, 6, 9];

export function PrintDialog({ onClose }: { onClose(): void }) {
  const t = useT();
  const info = useDocStore((s) => s.info);
  const currentPage = useViewStore((s) => s.currentPage);
  const selected = usePagesStore((s) => s.selected);
  const rotation = useViewStore((s) => s.rotation);
  const [range, setRange] = useState<RangeChoice>({ mode: "all", text: "" });
  const [method, setMethod] = useState<PrintMethod>("document");
  const [annots, setAnnots] = useState<PrintAnnots>("all");
  const [perSheet, setPerSheet] = useState<PerSheet>(1);
  const [order, setOrder] = useState<NupOrder>("across");
  const [booklet, setBooklet] = useState(false);
  const [fit, setFit] = useState<PrintFit>("fit");
  const [grayscale, setGrayscale] = useState(false);
  const [paper, setPaper] = useState<PaperId>(() => defaultPaper(globalThis.navigator?.language));
  // The page-size math of the 실제 크기 notice lives with the print flow, loaded on demand (a static
  // import would put a new shared chunk into the entry's preload map).
  const [flow, setFlow] = useState<PrintFlowModule | null>(null);
  useEffect(() => {
    if (fit !== "actual" || flow) return;
    let live = true;
    void import("../print/printFlow").then((m) => live && setFlow(m));
    return () => {
      live = false;
    };
  }, [fit, flow]);

  const pageCount = info?.pageCount ?? 0;
  const pages = resolveRange(range, { pageCount, currentPage, selected });
  const dom = method === "document";
  const sheets = !pages?.length ? 0 : booklet ? Math.ceil(pages.length / 4) * 2 : Math.ceil(pages.length / perSheet);
  // 실제 크기: how many of the chosen pages are larger than the paper (n-up sheets are made by the
  // engine at the pages' own paper size, so they are not counted here).
  const shrunk =
    flow && dom && fit === "actual" && !booklet && perSheet === 1 && pages?.length
      ? flow.shrunkCount(paper, pages.map((p) => flow.displaySize(info?.pages[p], rotation)))
      : 0;

  return (
    <Dialog
      titleKey="print.title"
      onClose={onClose}
      primary={{
        labelKey: "print.title",
        disabled: !pages?.length,
        onSelect: () => {
          const all = pages?.length === pageCount;
          void runPrint(all ? undefined : pages ?? undefined, method, {
            annots, perSheet, order, booklet, fit, grayscale, paper,
          });
          onClose();
        },
      }}
    >
      <Row labelKey="print.range">
        <RangePicker value={range} onChange={setRange} pageCount={pageCount} selectedCount={selected.length} />
      </Row>
      <Row labelKey="print.method" hintKey={method === "handler" ? "print.method.handlerHint" : undefined}>
        <select
          className="field"
          value={method}
          aria-label={t("print.method")}
          onChange={(e) => setMethod(e.currentTarget.value as PrintMethod)}
        >
          <option value="document">{t("print.method.document")}</option>
          <option value="handler">{t("print.method.handler")}</option>
        </select>
      </Row>
      <Row labelKey="print.annots">
        <select
          className="field"
          value={annots}
          aria-label={t("print.annots")}
          onChange={(e) => setAnnots(e.currentTarget.value as PrintAnnots)}
        >
          <option value="all">{t("print.annots.all")}</option>
          <option value="none">{t("print.annots.none")}</option>
          <option value="stamps">{t("print.annots.stamps")}</option>
        </select>
      </Row>
      <Row labelKey="print.perSheet">
        <div className="inline-row">
          <select
            className="field"
            value={perSheet}
            disabled={booklet}
            aria-label={t("print.perSheet")}
            onChange={(e) => setPerSheet(Number(e.currentTarget.value) as PerSheet)}
          >
            {PER_SHEET.map((n) => (
              <option key={n} value={n}>
                {t("print.perSheet.n", { count: n })}
              </option>
            ))}
          </select>
          {perSheet > 1 && !booklet && (
            <select
              className="field"
              value={order}
              aria-label={t("print.order")}
              onChange={(e) => setOrder(e.currentTarget.value as NupOrder)}
            >
              <option value="across">{t("print.order.across")}</option>
              <option value="down">{t("print.order.down")}</option>
            </select>
          )}
        </div>
      </Row>
      <label className="dlg-check text-base">
        <input type="checkbox" checked={booklet} onChange={(e) => setBooklet(e.target.checked)} />
        <span>{t("print.booklet")}</span>
      </label>
      {booklet && <p className="dlg-hint text-xs">{t("print.bookletHint")}</p>}
      {dom && (
        <>
          <Row labelKey="print.fit">
            <select
              className="field"
              value={fit}
              aria-label={t("print.fit")}
              onChange={(e) => setFit(e.currentTarget.value as PrintFit)}
            >
              <option value="fit">{t("print.fit.fit")}</option>
              <option value="actual">{t("print.fit.actual")}</option>
            </select>
          </Row>
          {fit === "actual" && (
            <>
              <Row labelKey="print.paper">
                <select
                  className="field"
                  value={paper}
                  aria-label={t("print.paper")}
                  onChange={(e) => setPaper(e.currentTarget.value as PaperId)}
                >
                  {PAPERS.map((id) => (
                    <option key={id} value={id}>
                      {t(`print.paper.${id}`)}
                    </option>
                  ))}
                </select>
              </Row>
              <p className="dlg-hint text-xs" data-testid="print-actual-hint">
                {t("print.actualHint")}
                {shrunk > 0 && <> {t("print.actualShrunk", { count: shrunk })}</>}
              </p>
            </>
          )}
          <label className="dlg-check text-base">
            <input type="checkbox" checked={grayscale} onChange={(e) => setGrayscale(e.target.checked)} />
            <span>{t("print.grayscale")}</span>
          </label>
          {sheets > PRINT_CHUNK && (
            <p className="dlg-hint text-xs">
              {t("print.chunkHint", { size: PRINT_CHUNK, parts: Math.ceil(sheets / PRINT_CHUNK) })}
            </p>
          )}
        </>
      )}
    </Dialog>
  );
}
