/**
 * The HTML input overlay of 양식 mode (F-20) — the `forms` slot of `PageShell`.
 *
 * One real DOM control per widget, placed with `ctx.rectToBox`, so the browser gives us focus
 * order, IME, autofill-free text entry and accessibility for nothing. The PDF's own widget
 * appearance stays visible underneath (the tile URL's `hl=1` tints it); the input is transparent
 * and only paints its text.
 *
 * **Korean input.** Text fields are *uncontrolled* — `defaultValue` plus a commit on blur / Enter.
 * A controlled `value` would re-render mid-composition and drop the jamo being typed, which is the
 * one bug that makes a Korean form overlay unusable. Checkboxes, radios and selects commit
 * immediately because they have no composition.
 */
import { memo, useRef } from "react";
import type { FormField } from "../ipc/types";
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
    case "signature":
      return (
        <div className="form-field form-readonly" style={style} title={t("form.fieldType")}>
          {field.type === "signature" ? t("sign.title") : field.name}
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

export const FormLayer = memo(function FormLayer({ ctx }: { ctx: PageLayerContext }) {
  const mode = useAppStore((s) => s.mode);
  const fields = useFormStore((s) => s.fields);
  if (mode !== "form") return null;
  const onPage = fields.filter((f) => f.page === ctx.index);
  if (onPage.length === 0) return null;
  return (
    <div className="form-layer">
      {onPage.map((field) => (
        <FieldControl key={fieldKey(field.page, field.index)} field={field} ctx={ctx} />
      ))}
    </div>
  );
});
