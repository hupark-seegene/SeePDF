/**
 * The `annotations` slot of one `PageShell` (STAGE1C_NOTES §1.2) — an SVG group drawn in PDF user
 * space. It paints, bottom to top:
 *
 *   1. the selection / hover outline of annotations the bitmap already contains
 *   2. the ghosts (created optimistically, not yet in a bitmap)
 *   3. the live tool preview
 *   4. the resize handles of the selection
 *
 * It never handles a pointer event: `.page-annots` is `pointer-events: none` in the viewer's CSS
 * and all hit-testing is done in user space by `src/tools/hit.ts` from the surface above.
 */
import { memo, useSyncExternalStore } from "react";
import type { Annot, AnnotId } from "../ipc/types";
import type { PageLayerContext } from "../viewer";
import { toolController } from "../tools/ToolController";
import { ALL_HANDLES, handlePoint, isResizable } from "../tools/hit";
import { HANDLE_PX } from "../tools/select";
import { useAnnotStore } from "../store/annotStore";
import { AnnotShape, PreviewShape } from "./shapes";

function usePreview() {
  return useSyncExternalStore(
    (l) => toolController.subscribe(l),
    () => toolController.version(),
  );
}

/** The dashed outline that says "this is an annotation" without repainting it. */
function Outline({ annot, state }: { annot: Annot; state: "hover" | "selected" }) {
  const quads = annot.quads?.length ? annot.quads : null;
  if (quads) {
    return (
      <g className="annot-outline" data-state={state}>
        {quads.map((q, i) => (
          <rect key={i} x={Math.min(q.l, q.r)} y={Math.min(q.b, q.t)} width={Math.abs(q.r - q.l)} height={Math.abs(q.t - q.b)} vectorEffect="non-scaling-stroke" />
        ))}
      </g>
    );
  }
  const r = annot.rect;
  return (
    <rect
      className="annot-outline"
      data-state={state}
      x={Math.min(r.l, r.r)}
      y={Math.min(r.b, r.t)}
      width={Math.abs(r.r - r.l)}
      height={Math.abs(r.t - r.b)}
      vectorEffect="non-scaling-stroke"
    />
  );
}

function Handles({ annot, scale }: { annot: Annot; scale: number }) {
  if (!isResizable(annot)) return null;
  const size = (HANDLE_PX * 2) / scale;
  return (
    <g className="annot-handles">
      {ALL_HANDLES.map((handle) => {
        const [x, y] = handlePoint(annot.rect, handle);
        return <rect key={handle} x={x - size / 2} y={y - size / 2} width={size} height={size} vectorEffect="non-scaling-stroke" />;
      })}
    </g>
  );
}

export interface AnnotOverlayProps {
  ctx: PageLayerContext;
}

export const AnnotOverlay = memo(function AnnotOverlay({ ctx }: AnnotOverlayProps) {
  usePreview();
  const annots = useAnnotStore((s) => s.byPage[ctx.index]);
  const ghosts = useAnnotStore((s) => s.ghosts);
  const selected = useAnnotStore((s) => s.selected);
  const hovered = useAnnotStore((s) => s.hovered);
  const editing = useAnnotStore((s) => s.editing);

  const preview = toolController.preview();
  const pageGhosts = ghosts.filter((g) => g.annot.page === ctx.index);
  const ghostIds = new Set<AnnotId>(pageGhosts.map((g) => g.annot.id));
  const list = annots ?? [];
  const selectedHere = list.filter((a) => selected.includes(a.id));

  return (
    <>
      {list.map((a) =>
        // A ghost of the same annotation is painted in full below; skip the outline for it here.
        ghostIds.has(a.id) || a.hidden ? null : selected.includes(a.id) ? (
          <Outline key={a.id} annot={a} state="selected" />
        ) : hovered === a.id ? (
          <Outline key={a.id} annot={a} state="hover" />
        ) : null,
      )}

      {pageGhosts.map((g) => (
        <g key={g.annot.id} className="annot-ghost" data-editing={editing?.id === g.annot.id || undefined}>
          <AnnotShape annot={g.annot} />
        </g>
      ))}

      {preview && preview.page === ctx.index && <PreviewShape preview={preview} scale={ctx.scale} />}

      {selectedHere.map((a) => (
        <Handles key={a.id} annot={a} scale={ctx.scale} />
      ))}
    </>
  );
});
