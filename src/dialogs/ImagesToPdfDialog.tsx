/**
 * 이미지로 PDF 만들기 (v0.3 D1): the images in page order (pointer drag or ↑ / ↓ to reorder, × to
 * remove, 이미지 추가…), the page size (원본 크기 — the image's own size at its resolution —, A4,
 * 레터), the margin and, on A4 / 레터, how the image sits (페이지에 맞춤 / 실제 크기). 만들기 runs
 * `create_from_images` (status-bar progress past 5 images) and opens the new, unsaved document.
 */
import { useRef, useState } from "react";
import { ChevronDown, ChevronUp, ImagePlus, X } from "lucide-react";
import { useT } from "../i18n/useT";
import * as api from "../ipc/api";
import { Dialog } from "./Dialog";
import { baseName } from "./flows";
import { createFromImagesFlow, isImagePath, type ImagesOptions } from "./imagesFlow";
import { insertionIndex, moveItem, reorderTarget, startPointerDrag } from "./pointerDrag";
import type { ImageFit, ImagePageSize } from "../ipc/types";
import "./reorder.css";

/** 여백 presets, in points: 없음 / 좁게 (≈ 6 mm) / 보통 (≈ 13 mm) / 넓게 (1 in). */
const MARGINS: { value: number; labelKey: string }[] = [
  { value: 0, labelKey: "imagesToPdf.margin.none" },
  { value: 18, labelKey: "imagesToPdf.margin.narrow" },
  { value: 36, labelKey: "imagesToPdf.margin.normal" },
  { value: 72, labelKey: "imagesToPdf.margin.wide" },
];

const SIZES: { value: ImagePageSize; labelKey: string }[] = [
  { value: "original", labelKey: "imagesToPdf.size.original" },
  { value: "a4", labelKey: "imagesToPdf.size.a4" },
  { value: "letter", labelKey: "imagesToPdf.size.letter" },
];

const FITS: { value: ImageFit; labelKey: string }[] = [
  { value: "contain", labelKey: "imagesToPdf.fit.contain" },
  { value: "actual", labelKey: "imagesToPdf.fit.actual" },
];

export default function ImagesToPdfDialog({ onClose, paths: initial }: { onClose(): void; paths: string[] }) {
  const t = useT();
  const [paths, setPaths] = useState<string[]>(() => [...new Set(initial)]);
  const [options, setOptions] = useState<ImagesOptions>({ pageSize: "a4", margin: 0, fit: "contain" });
  const [drag, setDrag] = useState<{ from: number; to: number } | null>(null);
  const [busy, setBusy] = useState(false);
  const listRef = useRef<HTMLUListElement>(null);

  const add = async () => {
    const picked = await api.openFileDialog({
      multiple: true,
      filters: [{ name: "Images", extensions: ["png", "jpg", "jpeg"] }],
    });
    if (!picked?.length) return;
    setPaths((prev) => [...prev, ...picked.filter((p) => isImagePath(p) && !prev.includes(p))]);
  };

  const move = (from: number, to: number) =>
    setPaths((prev) => (to < 0 || to >= prev.length ? prev : moveItem(prev, from, to)));

  const beginDrag = (e: React.PointerEvent, from: number) =>
    startPointerDrag(e, {
      onStart: () => setDrag({ from, to: from }),
      onMove: (m) => setDrag({ from, to: insertionIndex(listRef.current, m.clientY, ".merge-item") }),
      onDrop: (m) => {
        const index = reorderTarget(from, insertionIndex(listRef.current, m.clientY, ".merge-item"));
        setDrag(null);
        if (index !== null) setPaths((prev) => moveItem(prev, from, index));
      },
      onCancel: () => setDrag(null),
    });

  const run = async () => {
    setBusy(true);
    const created = await createFromImagesFlow(paths, options);
    setBusy(false);
    if (created) onClose();
  };

  const named = options.pageSize !== "original";

  return (
    <Dialog
      titleKey="imagesToPdf.title"
      size="lg"
      onClose={onClose}
      footerExtra={
        <button type="button" className="btn" onClick={() => void add()}>
          <ImagePlus size={16} strokeWidth={1.75} aria-hidden />
          {t("imagesToPdf.add")}
        </button>
      }
      primary={{ labelKey: "imagesToPdf.create", onSelect: () => void run(), disabled: busy || paths.length === 0 }}
    >
      {paths.length === 0 ? (
        <p className="empty">{t("imagesToPdf.empty")}</p>
      ) : (
        <ul ref={listRef} className="merge-list" aria-label={t("imagesToPdf.order")} data-dragging={drag ? "" : undefined}>
          {paths.map((path, i) => (
            <li
              key={path}
              className="merge-item"
              data-dragging={drag?.from === i || undefined}
              data-drop-before={(drag && drag.to === i && reorderTarget(drag.from, drag.to) !== null) || undefined}
              data-drop-after={(drag && i === paths.length - 1 && drag.to === paths.length && reorderTarget(drag.from, drag.to) !== null) || undefined}
              onPointerDown={(e) => beginDrag(e, i)}
            >
              <span className="merge-index text-xs mono" aria-hidden>
                {i + 1}
              </span>
              <span className="merge-name text-sm" title={path}>
                {baseName(path)}
              </span>
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
                onClick={() => setPaths((prev) => prev.filter((_, j) => j !== i))}
              >
                <X size={16} strokeWidth={1.75} aria-hidden />
              </button>
            </li>
          ))}
        </ul>
      )}

      <div className="images-options">
        <span className="text-sm">{t("imagesToPdf.pageSize")}</span>
        <div className="seg" role="radiogroup" aria-label={t("imagesToPdf.pageSize")}>
          {SIZES.map((s) => (
            <label key={s.value} className="dlg-radio text-sm">
              <input
                type="radio"
                name="images-size"
                checked={options.pageSize === s.value}
                onChange={() => setOptions((o) => ({ ...o, pageSize: s.value }))}
              />
              <span>{t(s.labelKey)}</span>
            </label>
          ))}
        </div>
        <span className="text-sm">{t("imagesToPdf.margin")}</span>
        <select
          className="field"
          aria-label={t("imagesToPdf.margin")}
          value={options.margin}
          onChange={(e) => setOptions((o) => ({ ...o, margin: Number(e.target.value) }))}
        >
          {MARGINS.map((m) => (
            <option key={m.value} value={m.value}>
              {t(m.labelKey)}
            </option>
          ))}
        </select>
        {named && (
          <>
            <span className="text-sm">{t("imagesToPdf.fit")}</span>
            <div className="seg" role="radiogroup" aria-label={t("imagesToPdf.fit")}>
              {FITS.map((f) => (
                <label key={f.value} className="dlg-radio text-sm">
                  <input
                    type="radio"
                    name="images-fit"
                    checked={options.fit === f.value}
                    onChange={() => setOptions((o) => ({ ...o, fit: f.value }))}
                  />
                  <span>{t(f.labelKey)}</span>
                </label>
              ))}
            </div>
          </>
        )}
      </div>
      <p className="dlg-hint text-xs">{t(named ? "imagesToPdf.hint.named" : "imagesToPdf.hint.original")}</p>
    </Dialog>
  );
}
