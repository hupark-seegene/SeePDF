/**
 * "Has the engine painted my optimistic annotation yet?"
 *
 * ARCHITECTURE §10 wants the ghost removed on the **new tile's `onload`**, and `PageShell` does not
 * expose that to a layer (see STAGE1D_NOTES §7, integration request 1). Until it does, this asks
 * the same question directly: it requests the page's placeholder bitmap at the generation that
 * contains the annotation — the *identical* URL `PageShell` is already loading, so it is a webview
 * cache hit and costs no extra render — and drops the ghost when that image is in.
 *
 * Getting this wrong is visible either way: dropping the ghost too early flashes the annotation
 * off for a frame, never dropping it paints it twice (once by pdfium, once by the SVG).
 */
import { useMemo } from "react";
import { pageUrl } from "../ipc/protocol";
import { placeholderScaleKey, type PageLayerContext } from "../viewer";
import { useAnnotStore } from "../store/annotStore";

export function RenderProbe({ ctx }: { ctx: PageLayerContext }) {
  const ghosts = useAnnotStore((s) => s.ghosts);
  const generation = useMemo(() => {
    let best = 0;
    for (const g of ghosts) {
      if (g.annot.page === ctx.index && g.generation !== null && g.generation > best) best = g.generation;
    }
    return best;
  }, [ghosts, ctx.index]);

  if (generation === 0) return null;
  const settle = () => useAnnotStore.getState().pageRendered(ctx.index, generation);
  return (
    <img
      className="annot-probe"
      alt=""
      aria-hidden
      draggable={false}
      src={pageUrl({
        doc: ctx.docId,
        gen: generation,
        page: ctx.index,
        sk: placeholderScaleKey(ctx.page, ctx.rotation),
        rot: ctx.rotation,
      })}
      onLoad={settle}
      onError={settle}
    />
  );
}
