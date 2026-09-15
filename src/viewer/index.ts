/**
 * The viewer's public surface — what (d) tools/annotations and (e) organizer/dialogs import.
 * Everything here is frozen for Stage 1 (WORKPLAN (c)); see `docs/STAGE1C_NOTES.md`.
 */
export { Viewer, type ViewerProps } from "./Viewer";
export { Scroller, type ScrollerProps } from "./Scroller";
export {
  PageShell,
  makePageLayerContext,
  type PageLayerContext,
  type PageLayerRenderer,
  type PageLayers,
  type PageShellProps,
} from "./PageShell";

export {
  MAX_SCALE_KEY,
  MAX_WHOLE_PAGE_PX,
  MAX_WHOLE_PAGE_SCALE,
  TILE_PX,
  applyMat,
  deviceToPage,
  devicePixelRatio,
  displaySizePt,
  isEmptyRect,
  pageBoxCss,
  pagePixels,
  pageToDevice,
  placeholderScaleKey,
  rectContains,
  rectToBox,
  scaleFromKey,
  scaleKeyFor,
  shouldTile,
  tileGridFor,
  tilePriority,
  tileRect,
  unionRect,
  type Box,
  type Size,
} from "./geometry";

export {
  OVERSCAN_AFTER,
  OVERSCAN_BEFORE,
  PAGE_GAP,
  computeLayout,
  currentPageAt,
  fitZoomPercent,
  onScreenRange,
  pageRows,
  scrollTopForPage,
  visibleRange,
  type DocLayout,
  type LayoutItem,
  type LayoutRow,
} from "./layout";

export {
  anchorAt,
  anchoredScroll,
  clampZoom,
  nextZoomStep,
  previousZoomStep,
  scrollForAnchor,
  zoomForWheel,
  type ViewAnchor,
} from "./zoom";

export { TileManager, type TileRequest } from "./TileManager";

export { PageTextLayer, type Hit } from "./text/TextLayer";
export {
  ensureTextLayer,
  ensureTextLayers,
  getTextLayer,
  invalidateTextLayers,
  useTextLayer,
} from "./text/textLayers";
export {
  isEmptySelection,
  orderSelection,
  pageRange,
  selectionText,
  useSelectionStore,
  type TextPoint,
  type TextSelection,
} from "./text/selection";

export {
  firstHitIndex,
  mergeHits,
  stepHitIndex,
  useSearchStore,
  type SearchOptions,
  type SearchState,
} from "./search/SearchController";

export {
  clearTextSelection, copyToClipboard, currentSelectionText, findStep, selectAllOnCurrentPage,
  useViewerCommands,
} from "./viewerCommands";
