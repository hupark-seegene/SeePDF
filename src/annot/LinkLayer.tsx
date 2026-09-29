/**
 * 읽기 mode links (P2): every Link annotation of the page becomes a transparent hit box — an
 * outline on hover, the pointer cursor, a tooltip naming the target — and a click follows it
 * (`links.ts`: go to the page, or confirm and open the web address).
 *
 * v0.3 (V5): addresses that are only **typed** in the page text (`https://…`, `www.…`, an e-mail
 * address) are clickable too — PDFium's own detector finds them (`get_web_links`, read once per page
 * and generation) and they follow exactly the same confirm-then-open path. An address a Link
 * annotation already covers is not doubled.
 *
 * Mounted in the `surface` slot only in 읽기 mode, above the text layer: a drag that *starts* on a
 * link does not select text, exactly as in other readers; everywhere else the surface lets the
 * pointer through (`.page-surface` is `pointer-events: none`, only the boxes take events).
 */
import { memo, useEffect, useState } from "react";
import * as api from "../ipc/api";
import type { Annot, DocGeneration, DocId, PageIndex, Rect, WebLink } from "../ipc/types";
import type { PageLayerContext } from "../viewer";
import { useT } from "../i18n/useT";
import { useAnnotStore } from "../store/annotStore";
import { useDocStore } from "../store/docStore";
import { displayLabel } from "../viewer/pageLabel";
import { followLink, openWebLink } from "./links";

const NONE: Annot[] = [];
const NO_WEB: WebLink[] = [];

/** Web links per (document, generation, page); a small LRU, the pages scroll by. */
const MAX_CACHED = 200;
const webCache = new Map<string, WebLink[]>();

function remember(key: string, links: WebLink[]): void {
  webCache.delete(key);
  webCache.set(key, links);
  while (webCache.size > MAX_CACHED) {
    const oldest = webCache.keys().next().value;
    if (oldest === undefined) break;
    webCache.delete(oldest);
  }
}

/** Test seam. */
export function resetWebLinks(): void {
  webCache.clear();
}

function useWebLinks(docId: DocId, gen: DocGeneration, page: PageIndex): WebLink[] {
  const key = `${docId}:${gen}:${page}`;
  const [state, setState] = useState<{ key: string; links: WebLink[] } | null>(null);
  useEffect(() => {
    const hit = webCache.get(key);
    if (hit) {
      setState({ key, links: hit });
      return;
    }
    let live = true;
    void api
      .getWebLinks({ docId, page })
      .catch(() => NO_WEB)
      .then((links) => {
        remember(key, links);
        if (live) setState({ key, links });
      });
    return () => {
      live = false;
    };
  }, [key, docId, page]);
  return state?.key === key ? state.links : (webCache.get(key) ?? NO_WEB);
}

function covered(rect: Rect, annots: Annot[]): boolean {
  const x = (rect.l + rect.r) / 2;
  const y = (rect.b + rect.t) / 2;
  return annots.some((a) => x >= a.rect.l && x <= a.rect.r && y >= a.rect.b && y <= a.rect.t);
}

export const LinkLayer = memo(function LinkLayer({ ctx }: { ctx: PageLayerContext }) {
  const t = useT();
  const annots = useAnnotStore((s) => s.byPage[ctx.index]) ?? NONE;
  const labels = useDocStore((s) => s.info?.pageLabels);
  const web = useWebLinks(ctx.docId, ctx.docGeneration, ctx.index);
  const links = annots.filter((a) => a.kind === "link" && !a.hidden && (a.dest || a.uri));
  const typed = web.flatMap((w, i) =>
    w.rects.filter((r) => !covered(r, links)).map((rect, j) => ({ key: `w${i}-${j}`, url: w.url, rect })),
  );
  if (!links.length && !typed.length) return null;
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
      {typed.map((w) => {
        const box = ctx.rectToBox(w.rect);
        return (
          <button
            type="button"
            key={w.key}
            className="link-hit"
            title={w.url}
            aria-label={w.url}
            data-kind="url"
            data-web-link
            style={{ left: box.x, top: box.y, width: box.w, height: box.h }}
            onPointerDown={(e) => e.stopPropagation()}
            onClick={(e) => {
              e.preventDefault();
              e.stopPropagation();
              void openWebLink(w.url);
            }}
          />
        );
      })}
    </div>
  );
});
