/**
 * UI_SPEC §7 "텍스트 객체 (편집)" / "이미지 객체" — the properties of the 편집 mode selection
 * (Stage 7): type, 글꼴 (read-only), the editability badge and reason, 크기 / 색상 for a text
 * object (`edit_text_object`), 위치 / 크기 read-outs and 삭제.
 */
import { useT } from "../../i18n/useT";
import type { PageObject } from "../../ipc/types";
import { useEditStore } from "../../edit/editStore";
import { deleteSelection, reasonKey, restyleText } from "../../edit/actions";
import { Swatches } from "../Swatches";

const FONT_SIZES = [8, 10, 11, 12, 14, 18, 24, 36];
const NONE: PageObject[] = [];

function fmt(v: number): string {
  return (Math.round(v * 10) / 10).toString();
}

export function EditPanel() {
  const t = useT();
  const selection = useEditStore((s) => s.selection);
  const objects = useEditStore((s) => (s.selection ? s.pages[s.selection.page]?.objects ?? NONE : NONE));
  const chosen = selection ? objects.filter((o) => selection.ids.includes(o.objectId)) : NONE;

  if (!selection || chosen.length === 0) return <p className="empty">{t("edit.empty")}</p>;
  const one = chosen.length === 1 ? chosen[0] : null;
  const canDelete = chosen.every((o) => o.editable !== "readOnly");

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
        {one && (
          <dl className="inspector-meta">
            <dt>{t("prop.position")}</dt>
            <dd className="mono">
              {fmt(one.rect.l)}, {fmt(one.rect.b)}
            </dd>
            <dt>{t("prop.size")}</dt>
            <dd className="mono">
              {fmt(one.rect.r - one.rect.l)} × {fmt(one.rect.t - one.rect.b)}
            </dd>
          </dl>
        )}
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
