/**
 * 인쇄 (F-26): pick a range and a method, then print.
 *
 * Two methods, both of which put the document on paper (UI_SPEC §10, `flows.runPrint`):
 * the print-only DOM plus the system print panel (default), or the flattened temp file handed
 * to the OS PDF handler.
 */
import { useState } from "react";
import { useDocStore } from "../store/docStore";
import { useViewStore } from "../store/viewStore";
import { usePagesStore } from "../store/pagesStore";
import { useT } from "../i18n/useT";
import { Dialog, Row } from "./Dialog";
import { RangePicker } from "./RangePicker";
import { resolveRange, type RangeChoice } from "./pageRange";
import { runPrint, type PrintMethod } from "./flows";

export function PrintDialog({ onClose }: { onClose(): void }) {
  const t = useT();
  const info = useDocStore((s) => s.info);
  const currentPage = useViewStore((s) => s.currentPage);
  const selected = usePagesStore((s) => s.selected);
  const [range, setRange] = useState<RangeChoice>({ mode: "all", text: "" });
  const [method, setMethod] = useState<PrintMethod>("document");

  const pageCount = info?.pageCount ?? 0;
  const pages = resolveRange(range, { pageCount, currentPage, selected });

  return (
    <Dialog
      titleKey="print.title"
      onClose={onClose}
      primary={{
        labelKey: "print.title",
        disabled: !pages?.length,
        onSelect: () => {
          const all = pages?.length === pageCount;
          void runPrint(all ? undefined : pages ?? undefined, method);
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
    </Dialog>
  );
}
