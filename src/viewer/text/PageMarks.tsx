/**
 * Selection and search rectangles for one page — the `.page-marks` layer of `PageShell`.
 *
 * Plain absolutely positioned divs with `mix-blend-mode: multiply`, so a highlight darkens the
 * glyphs underneath instead of covering them, and so the same rectangles work over a white page
 * and over an inverted (night-mode) one.
 */
import { useMemo } from "react";
import type { Rect } from "../../ipc/types";
import type { PageLayerContext } from "../PageShell";
import { useSearchStore } from "../search/SearchController";
import { pageRange, useSelectionStore } from "./selection";
import { useTextLayer } from "./textLayers";

export function PageMarks({ ctx }: { ctx: PageLayerContext }) {
  const layer = useTextLayer(ctx.docId, ctx.docGeneration, ctx.index);
  const selection = useSelectionStore((s) => s.selection);
  // hits of another document (the one this window showed before) are never painted on this one
  const hits = useSearchStore((s) => (s.docId === null || s.docId === ctx.docId ? s.byPage.get(ctx.index) : undefined));
  const currentHit = useSearchStore((s) => s.hits[s.current]);

  const selectionRects = useMemo<Rect[]>(() => {
    if (!layer || !selection || selection.docId !== ctx.docId) return [];
    const range = pageRange(selection, ctx.index, layer.charCount);
    return range ? layer.rangeRects(range[0], range[1]) : [];
  }, [layer, selection, ctx.docId, ctx.index]);

  if (selectionRects.length === 0 && !hits?.length) return null;

  return (
    <>
      {selectionRects.map((rect, i) => (
        <div key={`s${i}`} className="mark sel" style={boxStyle(ctx, rect)} />
      ))}
      {hits?.map((hit, i) =>
        hit.rects.map((rect, j) => (
          <div
            key={`h${i}-${j}`}
            className="mark hit"
            data-current={hit === currentHit || undefined}
            style={boxStyle(ctx, rect)}
          />
        )),
      )}
    </>
  );
}

function boxStyle(ctx: PageLayerContext, rect: Rect): React.CSSProperties {
  const box = ctx.rectToBox(rect);
  return { left: box.x, top: box.y, width: Math.max(1, box.w), height: Math.max(1, box.h) };
}
