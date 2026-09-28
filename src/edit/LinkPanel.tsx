/**
 * The inspector's 링크 section (P2): what the selected link does, a form to change its target
 * (`update_link`, one undo step), 링크 열기 to try it, and 삭제 (`delete_link`). Shown for the 편집
 * mode 링크 tool's selection and for a link selected with the 주석 mode 선택 tool.
 */
import type { Annot } from "../ipc/types";
import { useT } from "../i18n/useT";
import { useAnnotStore } from "../store/annotStore";
import { useDocStore } from "../store/docStore";
import { displayLabel } from "../viewer/pageLabel";
import { followLink } from "../annot/links";
import { LinkTargetForm } from "./LinkTargetForm";
import { deleteLink, updateLink, type LinkRef } from "./linkActions";

export function LinkPanel({ link }: { link: LinkRef | null }) {
  const t = useT();
  const labels = useDocStore((s) => s.info?.pageLabels);
  const annot: Annot | undefined = useAnnotStore((s) => (link ? s.byPage[link.page]?.find((a) => a.id === link.id) : undefined));

  if (!link || !annot) return <p className="empty">{t("link.emptyPanel")}</p>;
  const target = annot.dest ?? (annot.uri !== undefined ? { url: annot.uri } : undefined);
  const summary = annot.dest
    ? t("link.goToPage", { page: displayLabel(labels, annot.dest.page) })
    : annot.uri ?? t("link.noTarget");

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
