/**
 * Stage 0's placeholder canvas is gone: `src/viewer/**` (Stage 1 (c)) is the real thing —
 * virtualised scroller, tile manager, text layer, search highlights, layer slots for (d)/(e).
 *
 * This file only keeps the import site in `App.tsx` stable. Stage 1 (d) points it at
 * `src/annot/bridge.tsx`, which mounts exactly that viewer and fills its `layers` slot with the
 * annotation overlay, the form inputs and the tool surface once the (lazily imported) annotation
 * chunk has arrived. New code imports from `src/viewer` or `src/annot`.
 */
export { AnnotatedCanvas as CanvasStub } from "../annot/bridge";
