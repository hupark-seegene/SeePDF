/**
 * The 문단 편집 / 텍스트 추가 editing box (Stage 7): a mask in the page's paper colour over the
 * original paragraph, a textarea styled like the paragraph (size × zoom, leading, alignment,
 * colour, a font-family approximation), a right-edge width handle and a small floating bar.
 *
 * Korean-IME safe like `annot/editors.tsx`: the textarea is uncontrolled and read on commit, never
 * re-rendered from React state while a Hangul syllable is being composed; a commit that arrives
 * mid-composition (a click outside lands before the browser ends it) waits for it to end.
 *
 * Stage 9: once the text grows past the original box, a dashed line marks the original bottom and a
 * hint says 완료 will push the content below down — the paper-backed textarea covers that content
 * while typing, which otherwise reads as if it had been deleted.
 *
 * Rotation (Stage 8): the engine writes the text along the page's own +x, so under a /Rotate or a
 * view rotation the mask, the textarea and the width handle sit in one frame anchored at the text's
 * top-left and turned with the page (`rotate(deg)`) — the text reads the way it will be written.
 * The bar and the notice stay upright, placed off the frame's on-screen extent.
 */
import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { AlignCenter, AlignJustify, AlignLeft, AlignRight } from "lucide-react";
import type { ParagraphAlign, Rect } from "../ipc/types";
import type { PageLayerContext } from "../viewer";
import { useT } from "../i18n/useT";
import { useViewStore } from "../store/viewStore";
import { Swatches, rgbCss } from "../app/Swatches";
import { useEditStore, type EditSession } from "./editStore";
import { cancelSession, commitSession, setSessionFocuser, setSessionTextReader } from "./actions";
import { deltaToPage, editorRotation, fontStack } from "./geometry";

const ALIGNS: { id: ParagraphAlign; icon: typeof AlignLeft }[] = [
  { id: "left", icon: AlignLeft },
  { id: "center", icon: AlignCenter },
  { id: "right", icon: AlignRight },
  { id: "justify", icon: AlignJustify },
];
const BAR_H = 36;
/** approximate bar width, so it is pulled left rather than clipped by the page box */
const BAR_W = 460;
/** longest a commit waits for an IME composition to end before it reads the text anyway */
const SETTLE_MS = 300;

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
  // a ref, not state: nothing re-renders while a syllable is being composed
  const composing = useRef(false);
  const settleWaiters = useRef<(() => void)[]>([]);
  const [heightPx, setHeightPx] = useState(0);

  const probe = session.kind === "paragraph" ? session.probe : null;
  const rect = sessionRect(session);
  const s = ctx.scale;
  const deg = editorRotation(ctx.page.rotation, ctx.rotation);
  // the frame's local space: origin at the text's top-left, x along the page's +x, y down its −y
  const [originX, originY] = ctx.toDevice(rect.l, rect.t);
  const boxW = (rect.r - rect.l) * s;
  const boxH = (rect.t - rect.b) * s;
  const fontPx = session.fontSizePt * s;
  const leadingPt = probe ? probe.lineHeightPt * (session.fontSizePt / probe.fontSizePt) : session.fontSizePt * 1.2;
  const widthPx = session.kind === "paragraph" || session.width > 0 ? session.width * s : Math.max(boxW, fontPx * 8);
  // the first line's box starts at the ascender; CSS centres the glyphs in the line box
  const topPx = -Math.max(0, (leadingPt - session.fontSizePt) / 2) * s;
  const bottomPx = Math.max(boxH, topPx + heightPx);
  // the text reaches past the original paragraph by more than half a line (a 문단 편집 only)
  const grown = probe !== null && heightPx > 0 && topPx + heightPx > boxH + (leadingPt * s) / 2;
  // the frame's on-screen extent (upright), for the bar and the notice
  const extent = ctx.rectToBox({
    l: rect.l,
    r: rect.l + Math.max(boxW, widthPx) / s,
    t: rect.t - topPx / s,
    b: rect.t - bottomPx / s,
  });

  const grow = () => {
    const el = ref.current;
    if (!el) return;
    el.style.height = "auto";
    el.style.height = `${el.scrollHeight}px`;
    setHeightPx(el.scrollHeight);
  };

  const releaseSettled = () => {
    for (const resolve of settleWaiters.current.splice(0)) resolve();
  };

  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    el.focus({ preventScroll: true });
    el.setSelectionRange(el.value.length, el.value.length);
    grow();
    setSessionTextReader(
      () => ref.current?.value ?? "",
      () =>
        composing.current
          ? new Promise<void>((resolve) => {
              settleWaiters.current.push(resolve);
              setTimeout(resolve, SETTLE_MS);
            })
          : null,
    );
    setSessionFocuser(() => ref.current?.focus({ preventScroll: true }));
    return () => {
      setSessionTextReader(null);
      setSessionFocuser(null);
      releaseSettled();
    };
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
  const barAbove = extent.y - BAR_H - 8 >= 0;
  const barTop = barAbove ? extent.y - BAR_H - 8 : extent.y + extent.h + 8;
  // notices sit under the box, or under the bar when the bar had to go below it
  const noticeTop = barAbove ? extent.y + extent.h + 4 : barTop + BAR_H + 4;

  const startWidthDrag = (e: React.PointerEvent<HTMLDivElement>) => {
    e.stopPropagation();
    e.preventDefault();
    const startX = e.clientX;
    const startY = e.clientY;
    const startW = session.width > 0 ? session.width : widthPx / s;
    const el = e.currentTarget;
    el.setPointerCapture?.(e.pointerId);
    // the pointer's travel along the page's +x, whatever the rotation
    const move = (ev: PointerEvent) => {
      const [dxPt] = deltaToPage(ctx.inverse, ev.clientX - startX, ev.clientY - startY);
      patch({ width: Math.max(session.fontSizePt * 2, startW + dxPt) });
    };
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
      <div
        className="edit-frame"
        data-rotation={deg}
        style={{ left: originX, top: originY, transform: deg ? `rotate(${deg}deg)` : undefined }}
      >
        {probe && <div className="edit-mask" style={{ left: -1, top: -1, width: boxW + 2, height: boxH + 2 }} />}
        {grown && (
          <div
            className="edit-origin-line"
            data-testid="edit-origin-line"
            aria-hidden
            style={{ left: -1, top: boxH, width: Math.max(boxW, widthPx) + 2 }}
          />
        )}
        <textarea
          ref={ref}
          className="edit-text"
          defaultValue={probe?.text ?? ""}
          aria-label={t(probe ? "edit.paragraph.label" : "edit.addText.label")}
          spellCheck={false}
          style={{
            left: 0,
            top: topPx,
            width: widthPx,
            minHeight: Math.max(boxH, leadingPt * s),
            fontSize: fontPx,
            lineHeight: `${leadingPt * s}px`,
            fontFamily: fontStack(probe?.fontName ?? "SeePDF Hangul"),
            textAlign: session.align,
            textAlignLast: session.align === "justify" ? "left" : undefined,
            textIndent: probe ? probe.firstLineIndentPt * s : 0,
            color: colour,
          }}
          onInput={grow}
          onCompositionStart={() => {
            composing.current = true;
          }}
          onCompositionEnd={() => {
            composing.current = false;
            // the final `input` may follow `compositionend` in the same task: read after both
            setTimeout(releaseSettled, 0);
          }}
          onKeyDown={(e) => {
            e.stopPropagation();
            if (composing.current || e.nativeEvent.isComposing) return;
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
          style={{ left: widthPx - 3, top: topPx, height: Math.max(boxH, heightPx) }}
          onPointerDown={startWidthDrag}
        />
      </div>
      <div className="edit-bar" style={{ left: Math.max(4, Math.min(extent.x, ctx.width - BAR_W)), top: barTop }}>
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
      {(probe?.mixedStyles || grown) && (
        <div className="edit-notices" style={{ left: Math.max(4, extent.x), top: noticeTop }}>
          {probe?.mixedStyles && <p className="edit-notice text-xs">{t("edit.paragraph.mixedStyles")}</p>}
          {grown && (
            <p className="edit-notice text-xs" role="status">
              {t("edit.flow.hint")}
            </p>
          )}
        </div>
      )}
    </div>
  );
}
