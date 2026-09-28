/**
 * 페이지 크기 변경 (P2, 페이지 mode): A4 / 레터 / A3 or a custom size in mm, the content scaled to
 * fit (centred) or kept at 100 % and centred, for the selected pages or every page — one
 * `resize_pages` call, one undo step `undo.pageResize`, a toast with 실행 취소. A named size follows
 * each page's orientation. Lazy-loaded from `DialogHost`.
 */
import { useMemo, useState } from "react";
import * as api from "../ipc/api";
import { useT } from "../i18n/useT";
import { useDocStore } from "../store/docStore";
import { usePagesStore } from "../store/pagesStore";
import { toast } from "../app/toastStore";
import type { PageIndex } from "../ipc/types";
import { Dialog, Row } from "./Dialog";
import { message } from "./flows";
import {
  MAX_SIDE_MM, RESIZE_MODES, SIZE_CHOICES, initialResizeForm, resizeTarget, sizeLabelMm, type ResizeForm,
} from "./resize";

type Scope = "targets" | "all";

export default function ResizeDialog({ onClose, pages: given }: { onClose(): void; pages?: PageIndex[] }) {
  const t = useT();
  const info = useDocStore((s) => s.info);
  const selected = usePagesStore((s) => s.selected);
  const focus = usePagesStore((s) => s.focus);
  // pages the user picked (a cell's menu, the organizer selection); else the focused page is only
  // the preview and the default is every page
  const explicit = given?.length ? given : selected.length ? selected : null;
  const targets = useMemo<PageIndex[]>(
    () => [...new Set(explicit ?? [focus ?? 0])].sort((a, b) => a - b),
    [explicit, focus],
  );
  const [scope, setScope] = useState<Scope>(explicit ? "targets" : "all");
  const [form, setForm] = useState<ResizeForm>(() => initialResizeForm(info?.pages[targets[0] ?? 0]));
  const [busy, setBusy] = useState(false);

  if (!info) return null;
  const target = resizeTarget(form);
  const first = info.pages[targets[0] ?? 0];
  const now = first ? sizeLabelMm(first) : null;
  const count = scope === "all" ? info.pageCount : targets.length;
  const patch = (p: Partial<ResizeForm>) => setForm((f) => ({ ...f, ...p }));

  const apply = async () => {
    if (!target) return;
    setBusy(true);
    try {
      const next = await api.resizePages({
        docId: info.docId,
        pages: scope === "all" ? "all" : targets,
        size: target,
        mode: form.mode,
      });
      useDocStore.getState().adopt(next);
      toast("resize.done", { count }, {
        tone: "success",
        actions: [{ labelKey: "common.undo", onSelect: () => void import("../annot/sync").then((m) => m.undoWithAnnots()) }],
      });
      onClose();
    } catch (e) {
      toast("resize.failed", undefined, { tone: "danger", detail: message(e) });
      setBusy(false);
    }
  };

  return (
    <Dialog
      titleKey="resize.title"
      size="md"
      onClose={onClose}
      primary={{ labelKey: "common.apply", onSelect: () => void apply(), disabled: busy || !target }}
    >
      <Row labelKey="resize.size">
        <div className="dlg-radio-group" role="radiogroup" aria-label={t("resize.size")}>
          {SIZE_CHOICES.map((s) => (
            <label key={s} className="dlg-radio text-base">
              <input type="radio" name="resize-size" checked={form.size === s} onChange={() => patch({ size: s })} />
              <span>{t(`resize.size.${s}`)}</span>
            </label>
          ))}
        </div>
        {form.size === "custom" && (
          <div className="inline-row">
            <input
              className="field num"
              type="number"
              min={1}
              max={MAX_SIDE_MM}
              aria-label={t("resize.width")}
              value={form.widthMm}
              onChange={(e) => patch({ widthMm: e.target.value })}
            />
            <span className="text-sm dim">×</span>
            <input
              className="field num"
              type="number"
              min={1}
              max={MAX_SIDE_MM}
              aria-label={t("resize.height")}
              value={form.heightMm}
              onChange={(e) => patch({ heightMm: e.target.value })}
            />
            <span className="text-sm dim">mm</span>
          </div>
        )}
        {form.size === "custom" && !target && <p className="dlg-hint danger text-xs">{t("resize.invalid")}</p>}
        {form.size !== "custom" && <p className="dlg-hint text-xs">{t("resize.orientationHint")}</p>}
        {now && <p className="dlg-hint text-xs mono">{t("resize.current", now)}</p>}
      </Row>
      <Row labelKey="resize.mode">
        <div className="dlg-radio-group" role="radiogroup" aria-label={t("resize.mode")}>
          {RESIZE_MODES.map((m) => (
            <label key={m} className="dlg-radio text-base">
              <input type="radio" name="resize-mode" checked={form.mode === m} onChange={() => patch({ mode: m })} />
              <span>{t(`resize.mode.${m}`)}</span>
            </label>
          ))}
        </div>
      </Row>
      <Row labelKey="crop.applyTo">
        <div className="dlg-radio-group" role="radiogroup" aria-label={t("crop.applyTo")}>
          <label className="dlg-radio text-base">
            <input type="radio" name="resize-scope" checked={scope === "targets"} onChange={() => setScope("targets")} />
            <span>{t("crop.applyTo.selected", { count: targets.length })}</span>
          </label>
          <label className="dlg-radio text-base">
            <input type="radio" name="resize-scope" checked={scope === "all"} onChange={() => setScope("all")} />
            <span>{t("crop.applyTo.all")}</span>
          </label>
        </div>
      </Row>
    </Dialog>
  );
}
