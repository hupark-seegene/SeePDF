/**
 * 문서 정보 (⌘I). 제목 / 작성자 / 주제 / 키워드 are editable (P1-1, `set_metadata`); the rest is
 * read-only. A password-protected document cannot be edited, so the fields stay read-only there.
 */
import { useState } from "react";
import * as api from "../ipc/api";
import { useLocale, useT } from "../i18n/useT";
import { formatBytes } from "../i18n";
import { useDocStore } from "../store/docStore";
import { toast } from "../app/toastStore";
import { Dialog } from "./Dialog";
import { dirName, message } from "./flows";
import { encryptionKey, formatPdfDate, metaChanged, metaFormOf, metaFromForm, type MetaForm } from "./docInfo";

const FIELDS: { key: keyof MetaForm; labelKey: string; hintKey?: string }[] = [
  { key: "title", labelKey: "dialog.docInfo.docTitle" },
  { key: "author", labelKey: "prop.author" },
  { key: "subject", labelKey: "dialog.docInfo.subject" },
  { key: "keywords", labelKey: "dialog.docInfo.keywords", hintKey: "dialog.docInfo.keywordsHint" },
];

export function DocInfoDialog({ onClose }: { onClose(): void }) {
  const t = useT();
  const locale = useLocale();
  const info = useDocStore((s) => s.info);
  const [form, setForm] = useState<MetaForm>(() => metaFormOf(info?.meta ?? {}));
  const [busy, setBusy] = useState(false);
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
    { key: "prop.created", value: formatPdfDate(info.meta.created, locale) },
    { key: "prop.modified", value: formatPdfDate(info.meta.modified, locale) },
    { key: "dialog.docInfo.tagged", value: t(info.tagged ? "common.yes" : "common.no") },
    { key: "security.encryption", value: t(encryptionKey(info.permissions.revision, info.encrypted)) },
  ];
  const changed = metaChanged(info.meta, form);

  async function apply() {
    if (!info) return;
    setBusy(true);
    try {
      await api.setMetadata({ docId: info.docId, meta: metaFromForm(form) });
      await useDocStore.getState().refresh();
      toast("dialog.docInfo.updated", undefined, { tone: "success", timeoutMs: 2200 });
      onClose();
    } catch (e) {
      toast("error.generic", undefined, { tone: "danger", detail: message(e) });
      setBusy(false);
    }
  }

  return (
    <Dialog
      titleKey="dialog.docInfo.title"
      onClose={onClose}
      primary={{ labelKey: "common.apply", onSelect: () => void apply(), disabled: !changed || info.encrypted || busy }}
    >
      <div className="docinfo-fields">
        {FIELDS.map((f) => (
          <label className="dlg-row" key={f.key}>
            <span className="dlg-label text-sm">{t(f.labelKey)}</span>
            <span className="dlg-control">
              <input
                className="field grow"
                type="text"
                value={form[f.key]}
                readOnly={info.encrypted}
                onChange={(e) => setForm((prev) => ({ ...prev, [f.key]: e.target.value }))}
              />
              {f.hintKey && <span className="dlg-hint text-xs">{t(f.hintKey)}</span>}
            </span>
          </label>
        ))}
        {info.encrypted && <p className="dlg-hint text-xs">{t("dialog.security.protectedHint")}</p>}
      </div>
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
