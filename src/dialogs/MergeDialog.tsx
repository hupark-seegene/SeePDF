/**
 * 파일 합치기 (UI_SPEC §10, F-16): an ordered list with drag, a per-file 1-based range field and the
 * warnings the engine cannot avoid — `merge_documents` imports pages, and import drops `/AcroForm`
 * and the outline (spikes/pages.md §2).
 */
import { useState } from "react";
import { ChevronDown, ChevronUp, FilePlus2, X } from "lucide-react";
import { useT } from "../i18n/useT";
import * as api from "../ipc/api";
import { Dialog } from "./Dialog";
import { baseName, mergePaths } from "./flows";
import { parsePageRange } from "./pageRange";

interface Input {
  path: string;
  range: string;
}

export function MergeDialog({ onClose, initialPaths }: { onClose(): void; initialPaths?: string[] }) {
  const t = useT();
  const [inputs, setInputs] = useState<Input[]>(() => (initialPaths ?? []).map((path) => ({ path, range: "" })));
  const [dragIndex, setDragIndex] = useState<number | null>(null);
  const [busy, setBusy] = useState(false);

  const add = async () => {
    const picked = await api.openFileDialog({ multiple: true });
    if (!picked?.length) return;
    setInputs((prev) => [...prev, ...picked.filter((p) => !prev.some((i) => i.path === p)).map((path) => ({ path, range: "" }))]);
  };

  const move = (from: number, to: number) => {
    setInputs((prev) => {
      if (to < 0 || to >= prev.length) return prev;
      const next = [...prev];
      const [item] = next.splice(from, 1);
      next.splice(to, 0, item);
      return next;
    });
  };

  const invalid = inputs.some((i) => i.range.trim() !== "" && parsePageRange(i.range, 10_000) === null);

  const run = async () => {
    setBusy(true);
    await mergePaths(inputs.map((i) => ({ path: i.path, range: i.range.trim() || undefined })));
    setBusy(false);
    onClose();
  };

  return (
    <Dialog
      titleKey="dialog.merge.title"
      size="lg"
      onClose={onClose}
      footerExtra={
        <button type="button" className="btn" onClick={() => void add()}>
          <FilePlus2 size={16} strokeWidth={1.75} aria-hidden />
          {t("dialog.merge.addFiles")}
        </button>
      }
      primary={{ labelKey: "pages.merge", onSelect: () => void run(), disabled: busy || inputs.length < 2 || invalid }}
    >
      {inputs.length === 0 ? (
        <p className="empty">{t("welcome.recent.empty")}</p>
      ) : (
        <ul className="merge-list" aria-label={t("dialog.merge.title")}>
          {inputs.map((input, i) => (
            <li
              key={input.path}
              className="merge-item"
              draggable
              data-dragging={dragIndex === i || undefined}
              onDragStart={() => setDragIndex(i)}
              onDragEnd={() => setDragIndex(null)}
              onDragOver={(e) => {
                e.preventDefault();
                if (dragIndex !== null && dragIndex !== i) {
                  move(dragIndex, i);
                  setDragIndex(i);
                }
              }}
            >
              <span className="merge-index text-xs mono">{i + 1}</span>
              <span className="merge-name text-sm" title={input.path}>
                {baseName(input.path)}
              </span>
              <input
                className="field merge-range"
                value={input.range}
                placeholder={t("pages.range.placeholder")}
                aria-label={t("pages.range")}
                onChange={(e) =>
                  setInputs((prev) => prev.map((it, j) => (j === i ? { ...it, range: e.target.value } : it)))
                }
              />
              <button type="button" className="icon-btn" aria-label={t("common.moveUp")} onClick={() => move(i, i - 1)}>
                <ChevronUp size={16} strokeWidth={1.75} aria-hidden />
              </button>
              <button type="button" className="icon-btn" aria-label={t("common.moveDown")} onClick={() => move(i, i + 1)}>
                <ChevronDown size={16} strokeWidth={1.75} aria-hidden />
              </button>
              <button
                type="button"
                className="icon-btn"
                aria-label={t("common.remove")}
                onClick={() => setInputs((prev) => prev.filter((_, j) => j !== i))}
              >
                <X size={16} strokeWidth={1.75} aria-hidden />
              </button>
            </li>
          ))}
        </ul>
      )}
      <p className="dlg-hint text-xs">{t("pages.merge.formWarning")}</p>
      <p className="dlg-hint text-xs">{t("pages.merge.outlineWarning")}</p>
    </Dialog>
  );
}
