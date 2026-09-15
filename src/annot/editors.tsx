/**
 * The two inline editors that float over a page: the 메모 popover (F-12) and the 텍스트 상자 editor
 * (F-11). Both are HTML, not SVG, because both need a real text input — and both are **Korean-IME
 * safe**: the field is uncontrolled and the value is read on blur / explicit commit, never on every
 * `input`. A controlled React `value` re-render in the middle of a Hangul composition drops the
 * jamo being composed, which is the single most visible Korean-input bug in an editor like this.
 */
import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { X } from "lucide-react";
import type { Annot, Rect } from "../ipc/types";
import type { PageLayerContext } from "../viewer";
import { useT } from "../i18n/useT";
import { PALETTE, useAnnotStore } from "../store/annotStore";
import { createAnnotation, deleteAnnotations, patchAnnotation } from "./actions";
import { rgb } from "./shapes";

const POPOVER_W = 280;
const GAP_PX = 8;

function clampLeft(x: number, width: number): number {
  return Math.max(4, Math.min(x, Math.max(4, width - POPOVER_W - 4)));
}

/** 메모 popover — swatches, 불투명도 and the note text, exactly the fast surface of UI_SPEC §7. */
export function NotePopover({ ctx, annot }: { ctx: PageLayerContext; annot: Annot }) {
  const t = useT();
  const setEditing = useAnnotStore((s) => s.setEditing);
  const ref = useRef<HTMLTextAreaElement>(null);
  const box = ctx.rectToBox(annot.rect);

  useEffect(() => {
    ref.current?.focus();
    ref.current?.setSelectionRange(ref.current.value.length, ref.current.value.length);
  }, [annot.id]);

  const commit = () => {
    const value = ref.current?.value ?? "";
    if (value !== annot.contents) patchAnnotation(annot.page, annot.id, { contents: value });
  };

  return (
    <div
      className="annot-popover"
      style={{ left: clampLeft(box.x, ctx.width), top: Math.max(4, box.y - GAP_PX - 132), width: POPOVER_W }}
      onPointerDown={(e) => e.stopPropagation()}
      onKeyDown={(e) => {
        e.stopPropagation();
        if (e.key === "Escape") {
          commit();
          setEditing(null);
        }
      }}
    >
      <div className="annot-popover-head">
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
        <button type="button" className="icon-btn" aria-label={t("common.close")} onClick={() => { commit(); setEditing(null); }}>
          <X size={16} strokeWidth={1.75} aria-hidden />
        </button>
      </div>
      <textarea
        ref={ref}
        className="field annot-note-text"
        defaultValue={annot.contents}
        placeholder={t("prop.note.placeholder")}
        onBlur={commit}
      />
      <div className="annot-popover-foot">
        <span className="text-xs">{annot.author ?? ""}</span>
        <button
          type="button"
          className="btn quiet"
          onClick={() => {
            setEditing(null);
            void deleteAnnotations(annot.page, [annot.id]);
          }}
        >
          {t("common.delete")}
        </button>
      </div>
    </div>
  );
}

export interface TextDraft {
  page: number;
  rect: Rect;
}

/**
 * The 텍스트 상자 editor. For a draft it creates the annotation on commit (one undo step, and no
 * empty box is ever written); for an existing one it patches `text`.
 */
export function TextBoxEditor({
  ctx,
  draft,
  annot,
  onClose,
}: {
  ctx: PageLayerContext;
  draft?: TextDraft;
  annot?: Annot;
  onClose(): void;
}) {
  const t = useT();
  const style = useAnnotStore((s) => s.style);
  const ref = useRef<HTMLTextAreaElement>(null);
  const [composing, setComposing] = useState(false);
  const rect = annot?.rect ?? draft?.rect;
  const page = annot?.page ?? draft?.page;

  useLayoutEffect(() => {
    ref.current?.focus();
    const value = ref.current?.value ?? "";
    ref.current?.setSelectionRange(value.length, value.length);
  }, []);

  if (!rect || page === undefined) return null;
  const box = ctx.rectToBox(rect);
  const fontSize = (annot?.fontSize ?? style.fontSize) * ctx.scale;
  const colour = rgb(annot?.color ?? style.color);

  const commit = () => {
    const value = (ref.current?.value ?? "").replace(/\s+$/, "");
    if (annot) {
      if (value !== (annot.text ?? annot.contents)) patchAnnotation(annot.page, annot.id, { text: value, contents: value });
    } else if (value) {
      void createAnnotation(page, {
        kind: "textbox",
        rect,
        text: value,
        fontSize: style.fontSize,
        color: style.color,
        align: style.align,
        fillColor: style.fillColor,
      });
    }
    onClose();
  };

  return (
    <textarea
      ref={ref}
      className="annot-text-editor"
      defaultValue={annot?.text ?? annot?.contents ?? ""}
      aria-label={t("tool.textbox")}
      style={{
        left: box.x,
        top: box.y,
        width: Math.max(box.w, 40),
        height: Math.max(box.h, fontSize * 1.6),
        fontSize,
        lineHeight: 1.25,
        color: colour,
      }}
      onPointerDown={(e) => e.stopPropagation()}
      onCompositionStart={() => setComposing(true)}
      onCompositionEnd={() => setComposing(false)}
      onBlur={commit}
      onKeyDown={(e) => {
        e.stopPropagation();
        if (composing) return;
        if (e.key === "Escape") {
          e.preventDefault();
          onClose();
        } else if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
          e.preventDefault();
          commit();
        }
      }}
    />
  );
}
