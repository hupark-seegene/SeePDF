/**
 * Stage 0's placeholder canvas is gone: `src/viewer/**` (Stage 1 (c)) is the real thing —
 * virtualised scroller, tile manager, text layer, search highlights, layer slots for (d)/(e).
 *
 * This file only keeps the import site in `App.tsx` stable. New code imports from `src/viewer`.
 */
export { Viewer as CanvasStub } from "../viewer/Viewer";
