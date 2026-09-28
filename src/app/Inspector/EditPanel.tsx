/**
 * UI_SPEC §7 "텍스트 객체 (편집)" / "이미지 객체" — the properties of the 편집 mode selection
 * (Stage 7): type, 글꼴 (read-only), the editability badge and reason, 크기 / 색상 for a text
 * object (`edit_text_object`), 위치 / 크기 as X · Y · 너비 · 높이 fields (Stage 8: X / Y move the
 * selection's bounding box, 너비 / 높이 scale one object — both `transform_object`) and 삭제.
 */
import { useState } from "react";
import { useT } from "../../i18n/useT";
import type { PageObject } from "../../ipc/types";
import { useEditStore } from "../../edit/editStore";
import { deleteSelection, reasonKey, restyleText, setSelectionGeometry } from "../../edit/actions";
import { unionRects } from "../../edit/geometry";
import { Swatches } from "../Swatches";
import { useAppStore } from "../../store/appStore";
import { RedactPanel } from "./RedactPanel";

const FONT_SIZES = [8, 10, 11, 12, 14, 18, 24, 36];
const NONE: PageObject[] = [];

function fmt(v: number): string {
  return (Math.round(v * 10) / 10).toString();
}

/** 영역 표시 armed or marks pending: the redaction section first, then the object selection. */
export function EditPanel() {
  const redacting = useAppStore((s) => s.tool === "redact");
  const pending = useEditStore((s) => s.marks.length > 0);
  if (redacting) return <RedactPanel />;
  return (
    <>
      {pending && <RedactPanel />}
      <ObjectPanel hideEmpty={pending} />
    </>
  );
}

function ObjectPanel({ hideEmpty }: { hideEmpty: boolean }) {
  const t = useT();
  const selection = useEditStore((s) => s.selection);
  const objects = useEditStore((s) => (s.selection ? s.pages[s.selection.page]?.objects ?? NONE : NONE));
  const chosen = selection ? objects.filter((o) => selection.ids.includes(o.objectId)) : NONE;

  if (!selection || chosen.length === 0) return hideEmpty ? null : <p className="empty">{t("edit.empty")}</p>;
  const one = chosen.length === 1 ? chosen[0] : null;
  const canDelete = chosen.every((o) => o.editable !== "readOnly");
  const box = unionRects(chosen.map((o) => o.rect))!;
  const canMove = chosen.some((o) => o.editable !== "readOnly");
  const canScale = one?.editable === "full";

  return (
    <>
      <section className="field-group">
        <h3 className="field-label text-xs">
          {one ? t(`edit.objectType.${one.type}`) : t("edit.selected.count", { count: chosen.length })}
        </h3>
        {one && one.editable !== "full" && (
          <p className="text-sm dim">
            {t(one.editable === "readOnly" ? "edit.badge.readOnly" : "edit.badge.moveOnly")} · {t(reasonKey(one.reason))}
          </p>
        )}
        {one?.type === "text" && (
          <>
            <h3 className="field-label text-xs">{t("prop.font")}</h3>
            <p className="text-sm mono">{one.fontName ?? "—"}</p>
            {one.editable === "full" && (
              <>
                <h3 className="field-label text-xs">{t("prop.fontSize")}</h3>
                <div className="chip-row">
                  {FONT_SIZES.map((v) => (
                    <button
                      key={v}
                      type="button"
                      className="chip mono"
                      data-active={Math.round(one.fontSizePt ?? 0) === v || undefined}
                      onClick={() => void restyleText(selection.page, one.objectId, { fontSizePt: v })}
                    >
                      {v}
                    </button>
                  ))}
                </div>
                <h3 className="field-label text-xs">{t("prop.color")}</h3>
                <Swatches
                  value={one.color ?? null}
                  label={t("prop.color")}
                  onPick={(c) => void restyleText(selection.page, one.objectId, { color: c })}
                />
              </>
            )}
          </>
        )}
      </section>
      <section className="field-group" aria-label={`${t("prop.position")} · ${t("prop.size")}`}>
        <h3 className="field-label text-xs">
          {t("prop.position")} · {t("prop.size")}
        </h3>
        {/* keyed by the selection: a half-typed value never carries over to another object */}
        <div className="edit-geom" key={`${selection.page}:${selection.ids.join(",")}`}>
          <NumberField label={t("edit.geometry.x")} value={box.l} disabled={!canMove} onCommit={(x) => void setSelectionGeometry({ x })} />
          <NumberField label={t("edit.geometry.y")} value={box.b} disabled={!canMove} onCommit={(y) => void setSelectionGeometry({ y })} />
          <NumberField
            label={t("edit.geometry.w")}
            value={box.r - box.l}
            min={1}
            disabled={!canScale}
            onCommit={(w) => void setSelectionGeometry({ w })}
          />
          <NumberField
            label={t("edit.geometry.h")}
            value={box.t - box.b}
            min={1}
            disabled={!canScale}
            onCommit={(h) => void setSelectionGeometry({ h })}
          />
        </div>
        <p className="text-xs dim">{t("edit.geometry.hint")}</p>
      </section>
      {canDelete && (
        <section className="field-group">
          <button type="button" className="btn quiet" onClick={() => void deleteSelection()}>
            {t("common.delete")}
          </button>
        </section>
      )}
    </>
  );
}

/**
 * A points field that commits on Enter or blur (Esc restores): typing "1", "12", "120" must not
 * send three transforms, and every commit is one undo step.
 */
function NumberField({ label, value, min, disabled, onCommit }: {
  label: string;
  value: number;
  min?: number;
  disabled?: boolean;
  onCommit(v: number): void;
}) {
  const [draft, setDraft] = useState<string | null>(null);
  const commit = () => {
    if (draft === null) return;
    const v = Number(draft.trim().replace(",", "."));
    setDraft(null);
    if (!draft.trim() || !Number.isFinite(v) || (min !== undefined && v < min)) return;
    if (Math.abs(v - value) >= 0.05) onCommit(v);
  };
  return (
    <label className="edit-geom-field text-xs">
      <span className="dim">{label}</span>
      <input
        className="field mono"
        type="number"
        step={0.5}
        min={min}
        inputMode="decimal"
        value={draft ?? fmt(value)}
        disabled={disabled}
        onChange={(e) => setDraft(e.currentTarget.value)}
        onBlur={commit}
        onKeyDown={(e) => {
          if (e.key === "Enter") {
            e.preventDefault();
            commit();
          } else if (e.key === "Escape") {
            e.preventDefault();
            e.stopPropagation();
            setDraft(null);
          }
        }}
      />
    </label>
  );
}
