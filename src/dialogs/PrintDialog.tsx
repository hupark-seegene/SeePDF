/**
 * 인쇄 (F-26): pick a range, then `print_prepare` + the OS handler, with webview printing as the
 * primary path where it exists (WORKPLAN §6 fallback table).
 */
import { useState } from "react";
import { useDocStore } from "../store/docStore";
import { useViewStore } from "../store/viewStore";
import { usePagesStore } from "../store/pagesStore";
import { Dialog, Row } from "./Dialog";
import { RangePicker } from "./RangePicker";
import { resolveRange, type RangeChoice } from "./pageRange";
import { runPrint } from "./flows";

export function PrintDialog({ onClose }: { onClose(): void }) {
  const info = useDocStore((s) => s.info);
  const currentPage = useViewStore((s) => s.currentPage);
  const selected = usePagesStore((s) => s.selected);
  const [range, setRange] = useState<RangeChoice>({ mode: "all", text: "" });

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
          void runPrint(all ? undefined : pages ?? undefined);
          onClose();
        },
      }}
    >
      <Row labelKey="print.range">
        <RangePicker value={range} onChange={setRange} pageCount={pageCount} selectedCount={selected.length} />
      </Row>
    </Dialog>
  );
}
