/**
 * UI_SPEC §7 "영역 표시 pending" (F-22): 채우기 색상 (기본 검정), 덮어쓸 문구, 표시된 영역 {{count}}개,
 * the live `redact_preview` per marked page — what goes, the collateral text in amber with why —
 * and **적용** (destructive; `applyMarks` confirms), disabled with the reason when a mark covers a
 * form field. Styles live in `edit/edit.css` (this panel only shows in 편집 mode).
 */
import { useState } from "react";
import { useT } from "../../i18n/useT";
import type { PageIndex, Rgb } from "../../ipc/types";
import { useEditStore, type RedactPreviewState } from "../../edit/editStore";
import { applyMarks, dropMarks, markedPages } from "../../edit/redact";
import { rgbCss } from "../Swatches";

const FILLS: { key: string; rgb: Rgb }[] = [
  { key: "color.black", rgb: [0, 0, 0] },
  { key: "color.gray", rgb: [128, 128, 128] },
  { key: "color.white", rgb: [255, 255, 255] },
];

export function RedactPanel() {
  const t = useT();
  const marks = useEditStore((s) => s.marks);
  const previews = useEditStore((s) => s.previews);
  const fill = useEditStore((s) => s.redactFill);
  const overlay = useEditStore((s) => s.redactOverlay);
  const setOptions = useEditStore((s) => s.setRedactOptions);
  const [busy, setBusy] = useState(false);

  const pages = markedPages(marks);
  const fields = pages.flatMap((p) => {
    const state = previews[p];
    return state?.status === "ready" ? state.result.formFields : [];
  });
  const blockedReason = fields.length ? t("redact.formFields", { names: fields.join(", ") }) : undefined;

  const apply = async () => {
    setBusy(true);
    try {
      await applyMarks();
    } finally {
      setBusy(false);
    }
  };

  return (
    <section className="field-group redact-panel" aria-label={t("redact.title")}>
      <h3 className="field-label text-xs">{t("redact.title")}</h3>
      <p className="text-sm" data-testid="redact-count">
        {marks.length ? t("redact.marked", { count: marks.length }) : t("redact.empty")}
      </p>

      <h3 className="field-label text-xs">{t("prop.fillColor")}</h3>
      <div className="redact-fills" role="radiogroup" aria-label={t("prop.fillColor")}>
        {FILLS.map((f) => {
          const on = f.rgb.join() === fill.join();
          return (
            <button
              key={f.key}
              type="button"
              role="radio"
              aria-checked={on}
              aria-label={t("a11y.colorSwatch", { name: t(f.key) })}
              className="swatch"
              data-selected={on || undefined}
              style={{ background: rgbCss(f.rgb) }}
              onClick={() => setOptions({ fill: f.rgb })}
            />
          );
        })}
      </div>

      <h3 className="field-label text-xs">
        <label htmlFor="redact-overlay">{t("redact.overlayText")}</label>
      </h3>
      <input
        id="redact-overlay"
        className="field"
        type="text"
        value={overlay}
        placeholder={t("redact.overlayPlaceholder")}
        onChange={(e) => setOptions({ overlay: e.currentTarget.value })}
      />

      {pages.length > 0 && <h3 className="field-label text-xs">{t("redact.previewTitle")}</h3>}
      {pages.map((p) => (
        <PagePreview key={p} page={p} state={previews[p]} />
      ))}

      {blockedReason && (
        <p className="banner danger text-sm" role="alert">
          {blockedReason}
        </p>
      )}

      <div className="redact-actions">
        <button
          type="button"
          className="btn redact-apply"
          disabled={marks.length === 0 || !!blockedReason || busy}
          title={blockedReason}
          onClick={() => void apply()}
        >
          {t("redact.apply")}
        </button>
        {marks.length > 0 && (
          <button type="button" className="btn quiet" disabled={busy} onClick={dropMarks}>
            {t("redact.clearAll")}
          </button>
        )}
      </div>
    </section>
  );
}

function PagePreview({ page, state }: { page: PageIndex; state: RedactPreviewState | undefined }) {
  const t = useT();
  if (!state || state.status === "loading") {
    return (
      <p className="redact-page text-sm dim">
        {t("redact.pageLabel", { page: page + 1 })} · {t("redact.previewLoading")}
      </p>
    );
  }
  if (state.status === "error") {
    return (
      <p className="redact-page text-sm dim">
        {t("redact.pageLabel", { page: page + 1 })} · {t("redact.previewFailed")}
      </p>
    );
  }
  const r = state.result;
  return (
    <div className="redact-page" data-testid={`redact-preview-${page}`}>
      <p className="text-sm">
        {t("redact.pageSummary", {
          page: page + 1,
          text: r.textObjects.length,
          images: r.imageObjects.length,
          annots: r.annotations.length,
        })}
      </p>
      {r.textObjects.length > 0 && (
        <ul className="redact-list text-xs">
          {r.textObjects.map((o) => (
            <li key={o.objectId} data-collateral={!o.fullyInside || undefined}>
              {o.text || "—"}
            </li>
          ))}
        </ul>
      )}
      {r.collateral.length > 0 && (
        <div className="redact-collateral-note text-xs" role="note">
          <strong>{t("redact.collateral")}</strong>
          <ul>
            {r.collateral.map((text, i) => (
              <li key={i}>{text}</li>
            ))}
          </ul>
          <span className="dim">{t("redact.collateralHint")}</span>
        </div>
      )}
    </div>
  );
}
