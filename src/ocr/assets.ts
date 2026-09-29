/**
 * Where `scripts/prepare-ocr.mjs` puts the offline OCR bundle, relative to the app origin. Split out
 * of `tesseractPool.ts` (v0.3 O1) so the capability loader can read `tessdata/languages.json` without
 * importing tesseract.js.
 */
export const OCR_ASSET_DIR = "/ocr";

/**
 * Absolute URL of an OCR asset. tesseract.js resolves relative paths against `window.location.href`
 * itself, but the worker then fetches `langPath` from *inside* the worker, so we hand it an absolute
 * URL and stop guessing. In the bundle the origin is `tauri://localhost`, in `vite dev`
 * `http://localhost:1420`, and `'self'` covers both.
 */
export function ocrAssetUrl(path: string): string {
  const base = typeof document !== "undefined" && document.baseURI
    ? document.baseURI
    : typeof location !== "undefined" ? location.href : "http://localhost/";
  return new URL(`${OCR_ASSET_DIR}/${path}`.replace(/\/{2,}/g, "/"), base).href;
}
