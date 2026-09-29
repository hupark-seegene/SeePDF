/**
 * The HTML input overlay of 양식 mode (F-20) — the `forms` slot of `PageShell`.
 *
 * One real DOM control per widget, placed with `ctx.rectToBox`, so the browser gives us focus
 * order, IME, autofill-free text entry and accessibility for nothing.
 *
 * **This overlay is the only renderer of a field while it is mounted.** `annot/bridge.tsx` asks
 * for `forms=0` page and tile URLs in 양식 mode, which makes the engine skip `FPDF_FFLDraw`, so
 * PDFium draws no widget value, no widget wash and no pushbutton caption underneath. Before
 * Stage 2b both drew and the two were a pixel or two apart (`s2-07.png`, F-20). Leaving 양식
 * mode unmounts the overlay and the pages are re-requested with the widgets back on, so a
 * reader always sees the document exactly as the file describes it.
 *
 * **Korean input.** Text fields are *uncontrolled* — `defaultValue` plus a commit on blur / Enter.
 * A controlled `value` would re-render mid-composition and drop the jamo being typed, which is the
 * one bug that makes a Korean form overlay unusable. Checkboxes, radios and selects commit
 * immediately because they have no composition.
 */
import { memo, useRef, useState } from "react";
import type { FormField, NewFieldType, Rect } from "../ipc/types";
import { pageUrl, scaleKey } from "../ipc/protocol";
import type { ToolId } from "../store/appStore";
import "./forms.css";
import type { PageLayerContext } from "../viewer";
import { useT } from "../i18n/useT";
import { useAppStore } from "../store/appStore";
import { commitField, fieldKey, useFormStore } from "./formStore";

function FieldControl({ field, ctx }: { field: FormField; ctx: PageLayerContext }) {
  const t = useT();
  const box = ctx.rectToBox(field.rect);
  const key = fieldKey(field.page, field.index);
  const setFocused = useFormStore((s) => s.setFocused);
  const highlight = useFormStore((s) => s.highlight);
  const composing = useRef(false);
  const style: React.CSSProperties = {
    left: box.x,
    top: box.y,
    width: box.w,
    height: box.h,
    fontSize: Math.max(9, (field.fontSizePt ?? Math.min(12, field.rect.t - field.rect.b - 4)) * ctx.scale),
  };
  const common = {
    id: `field-${key}`,
    name: field.name,
    "data-highlight": highlight || undefined,
    disabled: field.readOnly,
    "aria-label": field.name,
    "aria-required": field.required || undefined,
    onFocus: () => setFocused(key),
    onBlur: () => setFocused(null),
    onPointerDown: (e: React.PointerEvent) => e.stopPropagation(),
    // v0.3 F2: a field has its own menu (값 지우기 · 모든 필드 지우기 · 필드 강조 표시 전환), not the page's
    "data-context-menu": "",
    onContextMenu: (e: React.MouseEvent) => {
      e.preventDefault();
      e.stopPropagation();
      const { clientX, clientY } = e;
      void import("./formActions").then((m) => m.openFieldMenu(field, clientX, clientY));
    },
  };

  switch (field.type) {
    case "checkbox":
      return (
        <input
          {...common}
          type="checkbox"
          className="form-field form-check"
          style={style}
          defaultChecked={field.checked ?? field.value === "On"}
          onChange={(e) => void commitField(field.page, field.index, { checked: e.currentTarget.checked })}
        />
      );
    case "radio":
      return (
        <input
          {...common}
          type="radio"
          className="form-field form-check"
          style={style}
          name={`radio-${field.name}`}
          defaultChecked={field.checked ?? false}
          onChange={(e) => void commitField(field.page, field.index, { checked: e.currentTarget.checked })}
        />
      );
    case "combo":
    case "list": {
      const options = field.options ?? [];
      const selectedIndex = options.findIndex((o) => o.selected);
      return (
        <select
          {...common}
          className="form-field form-select"
          style={style}
          multiple={field.type === "list"}
          defaultValue={
            field.type === "list"
              ? options.flatMap((o, i) => (o.selected ? [String(i)] : []))
              : String(Math.max(0, selectedIndex))
          }
          onChange={(e) => {
            const picked = [...e.currentTarget.selectedOptions].map((o) => Number(o.value));
            void commitField(field.page, field.index, { selected: picked });
          }}
        >
          {options.map((o, i) => (
            <option key={i} value={String(i)}>
              {o.label}
            </option>
          ))}
        </select>
      );
    }
    case "button":
      // v0.3 F2: a pushbutton's caption lives in its widget appearance, which `forms=0`
      // suppresses while this overlay is mounted (F-20). The button's own rectangle of the page
      // rendered *with* the widgets (the ordinary page bitmap, cached like any other) is shown
      // here instead — the caption exactly as the file draws it, and only inside the widget.
      return (
        <div
          className="form-field form-readonly form-button"
          style={{
            ...style,
            backgroundImage: `url("${pageUrl({ doc: ctx.docId, gen: ctx.docGeneration, page: ctx.index, sk: scaleKey(ctx.zoomPercent), rot: ctx.rotation })}")`,
            backgroundSize: `${ctx.width}px ${ctx.height}px`,
            backgroundPosition: `${-box.x - 1}px ${-box.y - 1}px`,
          }}
          data-highlight={highlight || undefined}
          data-context-menu=""
          title={field.name}
          aria-label={field.name}
          role="img"
          onContextMenu={common.onContextMenu}
        />
      );
    case "signature":
      return (
        <div
          className="form-field form-readonly"
          style={style}
          data-highlight={highlight || undefined}
          data-context-menu=""
          title={field.name}
          aria-label={field.name}
          onContextMenu={common.onContextMenu}
          onPointerDown={(e) => {
            e.stopPropagation();
            setFocused(key);
          }}
        >
          {t("sign.title")}
        </div>
      );
    default: {
      const commit = (value: string) => {
        if (value !== (field.value ?? "")) void commitField(field.page, field.index, { text: value });
      };
      const props = {
        ...common,
        className: "form-field form-text",
        style,
        defaultValue: field.value ?? "",
        maxLength: field.maxLen && field.maxLen > 0 ? field.maxLen : undefined,
        onCompositionStart: () => {
          composing.current = true;
        },
        onCompositionEnd: () => {
          composing.current = false;
        },
        onBlur: (e: React.FocusEvent<HTMLInputElement | HTMLTextAreaElement>) => {
          setFocused(null);
          commit(e.currentTarget.value);
        },
        onKeyDown: (e: React.KeyboardEvent<HTMLInputElement | HTMLTextAreaElement>) => {
          e.stopPropagation();
          if (composing.current) return;
          if (e.key === "Enter" && !field.multiline) {
            commit(e.currentTarget.value);
            e.currentTarget.blur();
          } else if (e.key === "Escape") {
            e.currentTarget.value = field.value ?? "";
            e.currentTarget.blur();
          }
        },
      };
      return field.multiline ? <textarea {...props} /> : <input {...props} type="text" />;
    }
  }
}

/**
 * What a control shows. The controls are uncontrolled, so a re-list that changes it (⌘Z, 모든 필드
 * 지우기, a value the engine truncated, the other radio of a group) must remount the control to
 * reach the DOM; a re-list that leaves it alone keeps the control — and whatever is being typed.
 */
function shownValue(field: FormField): string {
  return JSON.stringify([field.value ?? "", field.checked ?? null, (field.options ?? []).map((o) => o.selected)]);
}

/** The field tools of the 양식 strip (v0.3 F1) and the type each draws. */
const FIELD_TOOL: Partial<Record<ToolId, NewFieldType>> = {
  fieldText: "text",
  fieldCheckbox: "checkbox",
  fieldRadio: "radio",
  fieldCombo: "combo",
  fieldSignature: "signature",
};

/** Smallest field a drag makes (points); a click makes a default-sized one. */
const MIN_FIELD_PT = 4;
const DEFAULT_SIZE_PT: Record<NewFieldType, [number, number]> = {
  text: [160, 20],
  checkbox: [14, 14],
  radio: [14, 14],
  combo: [120, 20],
  signature: [150, 40],
};

/**
 * 필드 만들기 (v0.3 F1): with a field tool armed, a drag on the page draws the new field's
 * rectangle (a click places a default-sized one). Under the controls, so clicking an existing
 * field still selects it for 필드 속성.
 */
function AuthorSurface({ ctx, type }: { ctx: PageLayerContext; type: NewFieldType }) {
  const [draft, setDraft] = useState<{ x0: number; y0: number; x1: number; y1: number } | null>(null);
  const surface = useRef<HTMLDivElement>(null);
  const local = (e: { clientX: number; clientY: number }) => {
    const r = surface.current?.getBoundingClientRect();
    return { x: e.clientX - (r?.left ?? 0), y: e.clientY - (r?.top ?? 0) };
  };
  const onPointerDown = (e: React.PointerEvent) => {
    if (e.button !== 0) return;
    e.stopPropagation();
    e.preventDefault();
    const p = local(e);
    // Alt (⌥) while drawing a radio button starts a new group instead of joining the selected one.
    const newGroup = e.altKey;
    const start = { x0: p.x, y0: p.y, x1: p.x, y1: p.y };
    setDraft(start);
    let last = start;
    const move = (ev: PointerEvent) => {
      const q = local(ev);
      last = { ...start, x1: q.x, y1: q.y };
      setDraft(last);
    };
    const up = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
      setDraft(null);
      const [a0, b0] = ctx.toPage(last.x0, last.y0);
      const [a1, b1] = ctx.toPage(last.x1, last.y1);
      let rect: Rect = { l: Math.min(a0, a1), b: Math.min(b0, b1), r: Math.max(a0, a1), t: Math.max(b0, b1) };
      if (rect.r - rect.l < MIN_FIELD_PT || rect.t - rect.b < MIN_FIELD_PT) {
        const [w, h] = DEFAULT_SIZE_PT[type];
        rect = { l: a0, b: b0 - h, r: a0 + w, t: b0 };
      }
      void import("./formActions").then((m) => m.createFieldAt(ctx.index, rect, type, { newGroup }));
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
  };
  return (
    <div ref={surface} className="form-author" data-context-menu="" onPointerDown={onPointerDown}>
      {draft && (
        <span
          className="form-author-draft"
          style={{
            left: Math.min(draft.x0, draft.x1),
            top: Math.min(draft.y0, draft.y1),
            width: Math.abs(draft.x1 - draft.x0),
            height: Math.abs(draft.y1 - draft.y0),
          }}
        />
      )}
    </div>
  );
}

export const FormLayer = memo(function FormLayer({ ctx }: { ctx: PageLayerContext }) {
  const mode = useAppStore((s) => s.mode);
  const tool = useAppStore((s) => s.tool);
  const fields = useFormStore((s) => s.fields);
  if (mode !== "form") return null;
  const authoring = FIELD_TOOL[tool] ?? null;
  const onPage = fields.filter((f) => f.page === ctx.index);
  if (onPage.length === 0 && !authoring) return null;
  return (
    <div className="form-layer" data-authoring={authoring ?? undefined}>
      {authoring && <AuthorSurface ctx={ctx} type={authoring} />}
      {onPage.map((field) => (
        <FieldControl key={`${fieldKey(field.page, field.index)}:${shownValue(field)}`} field={field} ctx={ctx} />
      ))}
    </div>
  );
});
