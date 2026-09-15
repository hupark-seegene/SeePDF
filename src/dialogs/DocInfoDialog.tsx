/**
 * 문서 정보 (⌘I) — read-only in v1: pdfium cannot write `/Info` (P1-1 needs `lopdf`).
 */
import { useT } from "../i18n/useT";
import { formatBytes } from "../i18n";
import { useDocStore } from "../store/docStore";
import { Dialog } from "./Dialog";
import { dirName } from "./flows";

export function DocInfoDialog({ onClose }: { onClose(): void }) {
  const t = useT();
  const info = useDocStore((s) => s.info);
  if (!info) return null;
  const first = info.pages[0];
  const rows: { key: string; value: string }[] = [
    { key: "dialog.docInfo.fileName", value: info.name },
    { key: "dialog.docInfo.location", value: info.path ? dirName(info.path) : t("app.untitled") },
    { key: "dialog.docInfo.fileSize", value: formatBytes(info.bytes) },
    { key: "dialog.docInfo.pages", value: String(info.pageCount) },
    {
      key: "dialog.docInfo.pageSize",
      value: first ? `${round(first.widthPt)} × ${round(first.heightPt)} pt` : "—",
    },
    { key: "dialog.docInfo.pdfVersion", value: info.pdfVersion },
    { key: "dialog.docInfo.producer", value: info.meta.producer ?? "—" },
    { key: "prop.author", value: info.meta.author ?? "—" },
    { key: "dialog.docInfo.tagged", value: t(info.tagged ? "common.yes" : "common.no") },
    { key: "status.encrypted", value: t(info.encrypted ? "common.yes" : "common.no") },
  ];

  return (
    <Dialog titleKey="dialog.docInfo.title" onClose={onClose} cancelKey="common.close">
      <dl className="info-list">
        {rows.map((row) => (
          <div className="info-row" key={row.key}>
            <dt className="text-sm dim">{t(row.key)}</dt>
            <dd className="text-sm">{row.value}</dd>
          </div>
        ))}
      </dl>
    </Dialog>
  );
}

function round(n: number): number {
  return Math.round(n * 10) / 10;
}
