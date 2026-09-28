/**
 * The 문단 편집 / 텍스트 추가 editing box (Stage 7): a mask in the page's paper colour over the
 * original paragraph, a textarea styled like the paragraph (size × zoom, leading, alignment,
 * colour, a font-family approximation), a right-edge width handle and a small floating bar.
 *
 * Korean-IME safe like `annot/editors.tsx`: the textarea is uncontrolled and read on commit, never
 * re-rendered from React state while a Hangul syllable is being composed.
 */
import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { AlignCenter, AlignJustify, AlignLeft, AlignRight } from "lucide-react";
import type { ParagraphAlign, Rect } from "../ipc/types";
import type { PageLayerContext } from "../viewer";
import { useT } from "../i18n/useT";
import { useViewStore } from "../store/viewStore";
import { Swatches, rgbCss } from "../app/Swatches";
import { useEditStore, type EditSession } from "./editStore";
import { cancelSession, commitSession, setSessionTextReader } from "./actions";
import { fontStack } from "./geometry";

const ALIGNS: { id: ParagraphAlign; icon: typeof AlignLeft }[] = [
  { id: "left", icon: AlignLeft },
  { id: "center", icon: AlignCenter },
  { id: "right", icon: AlignRight },
  { id: "justify", icon: AlignJustify },
];
const BAR_H = 36;
/** approximate bar width, so it is pulled left rather than clipped by the page box */
const BAR_W = 460;

/** The page rect the session covers before any typing. */
function sessionRect(s: EditSession): Rect {
  if (s.kind === "paragraph") return s.probe.rect;
  const h = s.fontSizePt * 1.2;
  return { l: s.at[0], t: s.at[1], r: s.at[0] + Math.max(s.width, s.fontSizePt * 8), b: s.at[1] - h };
}

export function TextEditor({ ctx, session }: { ctx: PageLayerContext; session: EditSession }) {
  const t = useT();
  const night = useViewStore((s) => s.night);
  const patch = useEditStore((s) => s.patchSession);
  const ref = useRef<HTMLTextAreaElement>(null);
  const rootRef = useRef<HTMLDivElement>(null);
  const [composing, setComposing] = useState(false);
  const [heightPx, setHeightPx] = useState(0);

  const probe = session.kind === "paragraph" ? session.probe : null;
  const rect = sessionRect(session);
  const box = ctx.rectToBox(rect);
  const s = ctx.scale;
  const fontPx = session.fontSizePt * s;
  const leadingPt = probe ? probe.lineHeightPt * (session.fontSizePt / probe.fontSizePt) : session.fontSizePt * 1.2;
  const widthPx = session.kind === "paragraph" || session.width > 0 ? session.width * s : Math.max(box.w, fontPx * 8);
  // the first line's box starts at the ascender; CSS centres the glyphs in the line box
  const topPx = box.y - Math.max(0, (leadingPt - session.fontSizePt) / 2) * s;

  const grow = () => {
    const el = ref.current;
    if (!el) return;
    el.style.height = "auto";
    el.style.height = `${el.scrollHeight}px`;
    setHeightPx(el.scrollHeight);
  };

  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    el.focus({ preventScroll: true });
    el.setSelectionRange(el.value.length, el.value.length);
    grow();
    setSessionTextReader(() => ref.current?.value ?? "");
    return () => setSessionTextReader(null);
  }, []);

  // size / width changes re-flow the textarea
  useLayoutEffect(grow, [session.fontSizePt, session.width, session.align, s]);

  // Commit on a click anywhere outside the box and its bar (dialogs excluded: the font prompt).
  useEffect(() => {
    const onDown = (e: PointerEvent) => {
      const target = e.target as Element | null;
      if (!target || rootRef.current?.contains(target)) return;
      if (target.closest?.("[role=dialog], .dlg-backdrop, .dlg")) return;
      void commitSession();
    };
    document.addEventListener("pointerdown", onDown, true);
    return () => document.removeEventListener("pointerdown", onDown, true);
  }, []);

  const colour = night === "dark" ? "var(--page-ink)" : rgbCss(session.color);
  const barTop = box.y - BAR_H - 8 >= 0 ? box.y - BAR_H - 8 : box.y + Math.max(box.h, heightPx) + 8;

  const startWidthDrag = (e: React.PointerEvent<HTMLDivElement>) => {
    e.stopPropagation();
    e.preventDefault();
    const startX = e.clientX;
    const startW = session.width > 0 ? session.width : widthPx / s;
    const el = e.currentTarget;
    el.setPointerCapture?.(e.pointerId);
    const move = (ev: PointerEvent) => patch({ width: Math.max(session.fontSizePt * 2, startW + (ev.clientX - startX) / s) });
    const up = () => {
      el.removeEventListener("pointermove", move);
      el.removeEventListener("pointerup", up);
      el.removeEventListener("pointercancel", up);
      ref.current?.focus({ preventScroll: true });
    };
    el.addEventListener("pointermove", move);
    el.addEventListener("pointerup", up);
    el.addEventListener("pointercancel", up);
  };

  return (
    <div
      ref={rootRef}
      className="edit-session"
      onPointerDown={(e) => e.stopPropagation()}
      onKeyDown={(e) => e.stopPropagation()}
    >
      {probe && <div className="edit-mask" style={{ left: box.x - 1, top: box.y - 1, width: box.w + 2, height: box.h + 2 }} />}
      <textarea
        ref={ref}
        className="edit-text"
        defaultValue={probe?.text ?? ""}
        aria-label={t(probe ? "edit.paragraph.label" : "edit.addText.label")}
        spellCheck={false}
        style={{
          left: box.x,
          top: topPx,
          width: widthPx,
          minHeight: Math.max(box.h, leadingPt * s),
          fontSize: fontPx,
          lineHeight: `${leadingPt * s}px`,
          fontFamily: fontStack(probe?.fontName ?? "SeePDF Hangul"),
          textAlign: session.align,
          textAlignLast: session.align === "justify" ? "left" : undefined,
          textIndent: probe ? probe.firstLineIndentPt * s : 0,
          color: colour,
        }}
        onInput={grow}
        onCompositionStart={() => setComposing(true)}
        onCompositionEnd={() => setComposing(false)}
        onKeyDown={(e) => {
          e.stopPropagation();
          if (composing || e.nativeEvent.isComposing) return;
          if (e.key === "Escape") {
            e.preventDefault();
            cancelSession();
          } else if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
            e.preventDefault();
            void commitSession();
          }
        }}
      />
      <div
        className="edit-width-handle"
        role="separator"
        aria-orientation="vertical"
        aria-label={t("edit.paragraph.width")}
        style={{ left: box.x + widthPx - 3, top: topPx, height: Math.max(box.h, heightPx) }}
        onPointerDown={startWidthDrag}
      />
      <div className="edit-bar" style={{ left: Math.max(4, Math.min(box.x, ctx.width - BAR_W)), top: barTop }}>
        <label className="edit-bar-size text-xs">
          <span className="visually-hidden">{t("prop.fontSize")}</span>
          <input
            className="field mono"
            type="number"
            min={4}
            max={144}
            step={0.5}
            value={session.fontSizePt}
            aria-label={t("prop.fontSize")}
            onChange={(e) => {
              const v = Number(e.currentTarget.value);
              if (Number.isFinite(v) && v >= 4 && v <= 144) patch({ fontSizePt: v });
            }}
          />
        </label>
        <Swatches value={session.color} label={t("prop.color")} onPick={(c) => patch({ color: c })} />
        <div className="edit-bar-align" role="radiogroup" aria-label={t("prop.align")}>
          {ALIGNS.map(({ id, icon: Icon }) => (
            <button
              key={id}
              type="button"
              role="radio"
              aria-checked={session.align === id}
              aria-label={t(`prop.align.${id}`)}
              className="icon-btn"
              data-active={session.align === id || undefined}
              onClick={() => patch({ align: id })}
            >
              <Icon size={16} strokeWidth={1.75} aria-hidden />
            </button>
          ))}
        </div>
        <button type="button" className="btn quiet" onClick={() => cancelSession()}>
          {t("common.cancel")}
        </button>
        <button type="button" className="btn primary" onClick={() => void commitSession()}>
          {t("common.done")}
        </button>
      </div>
      {probe?.mixedStyles && (
        <p className="edit-notice text-xs" style={{ left: Math.max(4, box.x), top: box.y + Math.max(box.h, heightPx) + 4 }}>
          {t("edit.paragraph.mixedStyles")}
        </p>
      )}
    </div>
  );
}
