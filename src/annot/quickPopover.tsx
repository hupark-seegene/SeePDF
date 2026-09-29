/**
 * v0.3 A5 — the quick popover of UI_SPEC §7: when exactly one highlight (or other text markup),
 * shape or ink annotation is selected, a 280 px popover floats 8 px above it with the swatches,
 * 불투명도, 굵기 (where it means something), 메모 and 삭제 — the fast path that saves a trip to the
 * properties panel. It is hidden while the annotation is dragged and while another editor (메모
 * popover, thread, text box) is open. Every control is one `update_annotation` (the slider
 * coalesces, like the panel's).
 */
import { useState } from "react";
import { MessageSquareText, Trash2 } from "lucide-react";
import type { Annot, AnnotId, PageIndex } from "../ipc/types";
import type { PageLayerContext } from "../viewer";
import { useT } from "../i18n/useT";
import { PALETTE, useAnnotStore, type Ghost } from "../store/annotStore";
import { deleteAnnotations, patchAnnotation } from "./actions";
import { rgb } from "./shapes";

const POPOVER_W = 280;
const GAP_PX = 8;
const WIDTHS = [1, 2, 4, 8];

const MARKUP = ["highlight", "underline", "strikeout", "squiggly"];
const STROKED = ["square", "circle", "ink", "line", "arrow", "polygon", "polyline"];

/** The annotation the quick popover is for on `page`, or `null`. */
export function quickPopoverTarget(page: PageIndex, selected: AnnotId[], annots: Annot[], ghosts: Ghost[]): Annot | null {
  if (selected.length !== 1) return null;
  const id = selected[0];
  const a = ghosts.find((g) => g.annot.id === id && g.annot.page === page)?.annot ?? annots.find((x) => x.id === id);
  if (!a || a.page !== page || a.locked || a.editable !== "full" || a.hidden) return null;
  return MARKUP.includes(a.kind) || STROKED.includes(a.kind) ? a : null;
}

export function QuickPopover({ ctx, annot }: { ctx: PageLayerContext; annot: Annot }) {
  const t = useT();
  const [memo, setMemo] = useState(false);
  const box = ctx.rectToBox(annot.rect);
  const left = Math.max(4, Math.min(box.x, Math.max(4, ctx.width - POPOVER_W - 4)));
  // above the annotation; below it when it sits at the very top of the page
  const above = box.y > 120;
  const style = above
    ? { left, top: box.y - GAP_PX, width: POPOVER_W, transform: "translateY(-100%)" }
    : { left, top: box.y + box.h + GAP_PX, width: POPOVER_W };
  const thickness = STROKED.includes(annot.kind);

  return (
    <div
      className="annot-popover annot-quick"
      data-testid="quick-popover"
      style={style}
      onPointerDown={(e) => e.stopPropagation()}
      onKeyDown={(e) => {
        e.stopPropagation();
        if (e.key === "Escape") useAnnotStore.getState().select([]);
      }}
    >
      <div className="swatches">
        {PALETTE.map((sw) => (
          <button
            key={sw.key}
            type="button"
            className="swatch"
            aria-label={t("a11y.colorSwatch", { name: t(sw.key) })}
            data-selected={sw.rgb.join() === annot.color.join() || undefined}
            style={{ background: rgb(sw.rgb) }}
            onClick={() => patchAnnotation(annot.page, annot.id, { color: sw.rgb })}
          />
        ))}
      </div>
      <label className="annot-quick-row text-xs">
        <span>{t("prop.opacity")}</span>
        <input
          className="slider"
          type="range"
          min={5}
          max={100}
          value={Math.round(annot.opacity * 100)}
          aria-label={t("prop.opacity")}
          onChange={(e) => patchAnnotation(annot.page, annot.id, { opacity: Number(e.currentTarget.value) / 100 }, true)}
        />
      </label>
      {thickness && (
        <div className="chip-row" aria-label={t("prop.thickness")}>
          {WIDTHS.map((w) => (
            <button
              key={w}
              type="button"
              className="chip mono"
              data-active={annot.borderWidth === w || undefined}
              onClick={() => patchAnnotation(annot.page, annot.id, { borderWidth: w })}
            >
              {w}
            </button>
          ))}
        </div>
      )}
      {memo && (
        <textarea
          // uncontrolled + written on blur: IME-safe (editors.tsx)
          key={annot.id}
          className="field annot-note-text"
          defaultValue={annot.contents}
          placeholder={t("prop.note.placeholder")}
          autoFocus
          onBlur={(e) => {
            const value = e.currentTarget.value;
            if (value !== annot.contents) patchAnnotation(annot.page, annot.id, { contents: value });
          }}
        />
      )}
      <div className="annot-popover-foot">
        <button type="button" className="btn quiet" data-active={memo || undefined} onClick={() => setMemo((v) => !v)}>
          <MessageSquareText size={14} strokeWidth={1.75} aria-hidden />
          {t("prop.note")}
        </button>
        <button
          type="button"
          className="btn quiet"
          onClick={() => {
            useAnnotStore.getState().select([]);
            void deleteAnnotations(annot.page, [annot.id]);
          }}
        >
          <Trash2 size={14} strokeWidth={1.75} aria-hidden />
          {t("common.delete")}
        </button>
      </div>
    </div>
  );
}
