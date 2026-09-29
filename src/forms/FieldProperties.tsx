/**
 * 필드 속성 (v0.3 F1) — the inspector section for the selected form field: 이름, 필수, 최대 글자 수
 * (text), 선택 항목 (combo, one per line), and 필드 삭제. Each change is one `update_form_field`
 * (one undo step), committed on blur / Enter like every other inspector field; Hangul composition
 * is safe because the inputs are uncontrolled until commit.
 */
import { useEffect, useState } from "react";
import { Trash2 } from "lucide-react";
import { useT } from "../i18n/useT";
import { deleteField, updateField } from "./formActions";
import type { FormField } from "../ipc/types";
import "./forms.css";

export default function FieldProperties({ field }: { field: FormField }) {
  const t = useT();
  const [name, setName] = useState(field.name);
  const [maxLen, setMaxLen] = useState(field.maxLen ? String(field.maxLen) : "");
  const [options, setOptions] = useState((field.options ?? []).map((o) => o.label).join("\n"));
  useEffect(() => {
    setName(field.name);
    setMaxLen(field.maxLen ? String(field.maxLen) : "");
    setOptions((field.options ?? []).map((o) => o.label).join("\n"));
  }, [field]);

  const commitName = () => {
    const next = name.trim();
    if (next && next !== field.name) void updateField(field, { name: next });
    else setName(field.name);
  };
  const commitMaxLen = () => {
    const n = maxLen.trim() === "" ? 0 : Math.floor(Number(maxLen));
    if (!Number.isFinite(n) || n < 0) return setMaxLen(field.maxLen ? String(field.maxLen) : "");
    if (n !== (field.maxLen ?? 0)) void updateField(field, { maxLen: n });
  };
  const commitOptions = () => {
    const list = options.split("\n").map((o) => o.trim()).filter(Boolean);
    const before = (field.options ?? []).map((o) => o.label);
    if (list.length && JSON.stringify(list) !== JSON.stringify(before)) void updateField(field, { options: list });
  };
  const enter = (commit: () => void) => (e: React.KeyboardEvent<HTMLInputElement>) => {
    e.stopPropagation();
    if (e.key === "Enter" && !e.nativeEvent.isComposing) {
      e.preventDefault();
      commit();
    }
  };

  return (
    <section className="field-group field-properties" aria-label={t("form.author.properties")}>
      <h3 className="text-sm">{t("form.author.properties")}</h3>
      <label className="prop-row text-sm">
        <span>{t("form.fieldName")}</span>
        <input
          className="field"
          value={name}
          aria-label={t("form.fieldName")}
          onChange={(e) => setName(e.target.value)}
          onBlur={commitName}
          onKeyDown={enter(commitName)}
        />
      </label>
      <label className="dlg-check text-sm">
        <input
          type="checkbox"
          checked={field.required}
          onChange={(e) => void updateField(field, { required: e.currentTarget.checked })}
        />
        <span>{t("form.required")}</span>
      </label>
      {field.type === "text" && (
        <label className="prop-row text-sm">
          <span>{t("form.author.maxLen")}</span>
          <input
            className="field num"
            type="number"
            min={0}
            value={maxLen}
            placeholder={t("form.author.noLimit")}
            aria-label={t("form.author.maxLen")}
            onChange={(e) => setMaxLen(e.target.value)}
            onBlur={commitMaxLen}
            onKeyDown={enter(commitMaxLen)}
          />
        </label>
      )}
      {field.type === "combo" && (
        <label className="prop-row text-sm">
          <span>{t("form.author.options")}</span>
          <textarea
            className="field area"
            rows={4}
            value={options}
            aria-label={t("form.author.options")}
            onChange={(e) => setOptions(e.target.value)}
            onBlur={commitOptions}
            onKeyDown={(e) => e.stopPropagation()}
          />
        </label>
      )}
      <button type="button" className="btn quiet" data-tone="danger" onClick={() => void deleteField(field)}>
        <Trash2 size={14} strokeWidth={1.75} aria-hidden />
        {t("form.author.delete")}
      </button>
    </section>
  );
}
