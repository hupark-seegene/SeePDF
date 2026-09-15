/**
 * Public surface of the OCR module (Stage 1 (f)) — what the rest of the frontend may import.
 *
 * For the dialogs module (e), the whole integration is two lines:
 *
 *   // in the dialog host (lazy: the OCR chunk carries tesseract.js)
 *   const OcrDialog = lazy(() => import("../ocr").then((m) => ({ default: m.OcrDialog })));
 *   …
 *   <Suspense fallback={null}><OcrDialog /></Suspense>
 *
 *   // in useCommands.ts, for `tools.ocr` / the ⋯ overflow menu
 *   import("../ocr").then((m) => m.openOcrDialog());
 *
 * `OcrDialog` renders `null` until `openOcrDialog()` is called, so mounting it costs nothing.
 * `openOcrDialog({ selectedPages })` preselects 페이지 범위 from the organizer's selection.
 */
export { OcrDialog } from "./OcrDialog";
export { openOcrDialog, closeOcrDialog, useOcrDialogOpen, type OcrDialogContext } from "./dialogState";
export {
  runOcrJob, parsePageRange, formatPageRange, estimateSeconds, resolveDpi,
  type OcrRunOptions, type OcrJobResult, type OcrDpi,
} from "./ocrJob";
export {
  TesseractPool, defaultWorkerCount, hasWasmSimd, coreFileName, ocrAssetUrl, psmFor,
  DEFAULT_LANGS, DEFAULT_LAYOUT, DEFAULT_DPI, OcrCancelledError, type OcrLayout,
} from "./tesseractPool";
export {
  normalizeTesseract, normalizeVision, countWords, meanConfidence, imageBoxToPdfRect,
  devicePixelToPdfPoint, ocrScale, HANGUL_CER_BUDGET,
  type NormalizeContext, type TessPage, type VisionResult,
} from "./normalize";
