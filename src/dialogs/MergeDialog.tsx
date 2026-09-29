/**
 * 파일 합치기 (UI_SPEC §10, F-16): an ordered list, a per-file 1-based range field.
 *
 * v0.3: reordered by **pointer** drag (H7 — HTML5 drag and drop never fires in the Windows webview
 * while Tauri's file drop is on), with ↑ / ↓ as the keyboard fallback; and the engine now carries
 * each file's bookmarks (under a node named after the file), page labels and form fields into the
 * merge (P1), so the old "import drops the form and the outline" warnings are gone.
 */
import { useRef, useState } from "react";
import { ChevronDown, ChevronUp, FilePlus2, X } from "lucide-react";
import { useT } from "../i18n/useT";
import * as api from "../ipc/api";
import { Dialog } from "./Dialog";
import { baseName, mergePaths } from "./flows";
import { parsePageRange } from "./pageRange";
import { insertionIndex, moveItem, reorderTarget, startPointerDrag } from "./pointerDrag";
import "./reorder.css";

interface Input {
  path: string;
  range: string;
}

export function MergeDialog({ onClose, initialPaths }: { onClose(): void; initialPaths?: string[] }) {
  const t = useT();
  const [inputs, setInputs] = useState<Input[]>(() => (initialPaths ?? []).map((path) => ({ path, range: "" })));
  const [drag, setDrag] = useState<{ from: number; to: number } | null>(null);
  const [busy, setBusy] = useState(false);
  const listRef = useRef<HTMLUListElement>(null);

  const add = async () => {
    const picked = await api.openFileDialog({ multiple: true });
    if (!picked?.length) return;
    setInputs((prev) => [...prev, ...picked.filter((p) => !prev.some((i) => i.path === p)).map((path) => ({ path, range: "" }))]);
  };

  const move = (from: number, to: number) => {
    setInputs((prev) => {
      if (to < 0 || to >= prev.length) return prev;
      return moveItem(prev, from, to);
    });
  };

  const invalid = inputs.some((i) => i.range.trim() !== "" && parsePageRange(i.range, 10_000) === null);

  const run = async () => {
    setBusy(true);
    await mergePaths(inputs.map((i) => ({ path: i.path, range: i.range.trim() || undefined })));
    setBusy(false);
    onClose();
  };

  const beginDrag = (e: React.PointerEvent, from: number) =>
    startPointerDrag(e, {
      onStart: () => setDrag({ from, to: from }),
      onMove: (m) => setDrag({ from, to: insertionIndex(listRef.current, m.clientY, ".merge-item") }),
      onDrop: (m) => {
        const index = reorderTarget(from, insertionIndex(listRef.current, m.clientY, ".merge-item"));
        setDrag(null);
        if (index !== null) setInputs((prev) => moveItem(prev, from, index));
      },
      onCancel: () => setDrag(null),
    });

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
        <ul ref={listRef} className="merge-list" aria-label={t("dialog.merge.title")} data-dragging={drag ? "" : undefined}>
          {inputs.map((input, i) => (
            <li
              key={input.path}
              className="merge-item"
              data-dragging={drag?.from === i || undefined}
              data-drop-before={(drag && drag.to === i && reorderTarget(drag.from, drag.to) !== null) || undefined}
              data-drop-after={(drag && i === inputs.length - 1 && drag.to === inputs.length && reorderTarget(drag.from, drag.to) !== null) || undefined}
              onPointerDown={(e) => beginDrag(e, i)}
            >
              <span className="merge-index text-xs mono" aria-hidden>
                {i + 1}
              </span>
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
      <p className="dlg-hint text-xs">{t("pages.merge.carries")}</p>
    </Dialog>
  );
}
