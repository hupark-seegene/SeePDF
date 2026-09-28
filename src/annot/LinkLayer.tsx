/**
 * 읽기 mode links (P2): every Link annotation of the page becomes a transparent hit box — an
 * outline on hover, the pointer cursor, a tooltip naming the target — and a click follows it
 * (`links.ts`: go to the page, or confirm and open the web address).
 *
 * Mounted in the `surface` slot only in 읽기 mode, above the text layer: a drag that *starts* on a
 * link does not select text, exactly as in other readers; everywhere else the surface lets the
 * pointer through (`.page-surface` is `pointer-events: none`, only the boxes take events).
 */
import { memo } from "react";
import type { Annot } from "../ipc/types";
import type { PageLayerContext } from "../viewer";
import { useT } from "../i18n/useT";
import { useAnnotStore } from "../store/annotStore";
import { useDocStore } from "../store/docStore";
import { displayLabel } from "../viewer/pageLabel";
import { followLink } from "./links";

const NONE: Annot[] = [];

export const LinkLayer = memo(function LinkLayer({ ctx }: { ctx: PageLayerContext }) {
  const t = useT();
  const annots = useAnnotStore((s) => s.byPage[ctx.index]) ?? NONE;
  const labels = useDocStore((s) => s.info?.pageLabels);
  const links = annots.filter((a) => a.kind === "link" && !a.hidden && (a.dest || a.uri));
  if (!links.length) return null;
  return (
    <div className="link-layer">
      {links.map((a) => {
        const box = ctx.rectToBox(a.rect);
        const title = a.dest ? t("link.goToPage", { page: displayLabel(labels, a.dest.page) }) : a.uri;
        return (
          <button
            type="button"
            key={a.id}
            className="link-hit"
            title={title}
            aria-label={title}
            data-kind={a.dest ? "page" : "url"}
            style={{ left: box.x, top: box.y, width: box.w, height: box.h }}
            onPointerDown={(e) => e.stopPropagation()}
            onClick={(e) => {
              e.preventDefault();
              e.stopPropagation();
              followLink(a);
            }}
          />
        );
      })}
    </div>
  );
});
