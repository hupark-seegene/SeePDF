/**
 * The inspector's 링크 section (P2): what the selected link does, a form to change its target
 * (`update_link`, one undo step), 링크 열기 to try it, and 삭제 (`delete_link`). Shown for the 편집
 * mode 링크 tool's selection and for a link selected with the 주석 mode 선택 tool.
 */
import type { Annot, Rgb } from "../ipc/types";
import { Swatches } from "../app/Swatches";
import { useT } from "../i18n/useT";
import { useAnnotStore } from "../store/annotStore";
import { useDocStore } from "../store/docStore";
import { displayLabel } from "../viewer/pageLabel";
import { followLink } from "../annot/links";
import { LinkTargetForm } from "./LinkTargetForm";
import { deleteLink, updateLink, type LinkRef } from "./linkActions";

/** v0.3 P5: a new border is blue, the colour links traditionally have. */
const DEFAULT_BORDER: Rgb = [0, 102, 204];

export function LinkPanel({ link }: { link: LinkRef | null }) {
  const t = useT();
  const labels = useDocStore((s) => s.info?.pageLabels);
  const annot: Annot | undefined = useAnnotStore((s) => (link ? s.byPage[link.page]?.find((a) => a.id === link.id) : undefined));

  if (!link || !annot) return <p className="empty">{t("link.emptyPanel")}</p>;
  const target = annot.dest ?? (annot.uri !== undefined ? { url: annot.uri } : undefined);
  const summary = annot.dest
    ? t("link.goToPage", { page: displayLabel(labels, annot.dest.page) })
    : annot.uri ?? t("link.noTarget");
  const bordered = annot.borderWidth > 0;

  return (
    <section className="field-group link-panel">
      <h3 className="field-label text-xs">{t("link.title")}</h3>
      <p className="text-sm link-summary" title={summary}>
        {summary}
      </p>
      <h3 className="field-label text-xs">{t("link.target")}</h3>
      <LinkTargetForm
        key={`${annot.id}:${annot.modified ?? ""}:${annot.uri ?? ""}:${annot.dest?.page ?? ""}:${annot.dest?.y ?? ""}`}
        initial={target}
        submitKey="common.apply"
        onSubmit={(next) => void updateLink(link.page, link.id, { target: next })}
      />
      {/* v0.3 P5: an optional visible box (/Border + /C); links are invisible by default */}
      <label className="dlg-check text-sm link-border-toggle">
        <input
          type="checkbox"
          checked={bordered}
          onChange={(e) =>
            void updateLink(link.page, link.id, {
              border: { width: e.currentTarget.checked ? 1 : 0, color: bordered ? annot.color : DEFAULT_BORDER },
            })
          }
        />
        <span>{t("link.border")}</span>
      </label>
      {bordered && (
        <Swatches
          label={t("link.borderColor")}
          value={annot.color}
          onPick={(color) => void updateLink(link.page, link.id, { border: { width: annot.borderWidth || 1, color } })}
        />
      )}
      <div className="link-panel-actions">
        <button type="button" className="btn quiet" disabled={!target} onClick={() => followLink(annot)}>
          {t("link.follow")}
        </button>
        <button type="button" className="btn quiet" data-tone="danger" onClick={() => void deleteLink(link.page, link.id)}>
          {t("common.delete")}
        </button>
      </div>
    </section>
  );
}
