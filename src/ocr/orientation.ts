/**
 * 페이지 회전 자동 감지 (v0.3 O2): which way up is a scan?
 *
 * With the option on, every page to recognise is first read at ~100 DPI — as rendered, and turned
 * 90°, 180° and 270° clockwise — and the turn whose words the recogniser is most confident about wins,
 * if it wins clearly (`pickOrientation`, the twin of `engine/ocr/orientation.rs` `pick`). The page is
 * then recognised at full resolution turned that way, and `ocr_apply` sets its `/Rotate` in the same
 * undo step as the text layer (`{ page, ocr, setRotation }`).
 *
 * Who reads the trial pages:
 *   Apple Vision   `ocr_detect_orientation` — Vision reads sideways text as confidently as upright
 *                  text, so the backend asks it for each line's *reading direction* instead
 *   anything else  tesseract here (`detectWithTesseract`): four reads of the `/ocr` image at 100 DPI,
 *                  whose mean confidence does collapse on a wrong turn. Windows OCR reports no
 *                  confidence at all, so a Windows run detects with tesseract too.
 */
import type { OcrPage, OrientationScore, Rotation } from "../ipc/types";
import { countWords, meanConfidence, normalizeTesseract } from "./normalize";
import type { TessPage } from "./normalize";

/** The resolution of the trial reads. */
export const DETECT_DPI = 100;
/** The winner must beat the runner-up (and the page as it is) by this factor. */
export const MARGIN = 1.15;
/** Fewer words than this at the best turn is no evidence. */
export const MIN_WORDS = 3;

export const TURNS: Rotation[] = [0, 90, 180, 270];

/** One trial read's score: mean word confidence (0..100) and word count. */
export function scoreOf(rotation: Rotation, page: OcrPage): OrientationScore {
  const words = countWords(page);
  return { rotation, confidence: words ? meanConfidence(page) : 0, words };
}

/**
 * The clockwise turn that makes the page upright, or 0 when no turn wins clearly: the highest mean
 * confidence, at least `MIN_WORDS` words, and `MARGIN` × both the runner-up and the page as it is.
 * Turning a correct page is worse than leaving a sideways one.
 */
export function pickOrientation(scores: readonly OrientationScore[]): Rotation {
  const ranked = scores
    .filter((s) => s.words >= MIN_WORDS && Number.isFinite(s.confidence))
    .sort((a, b) => b.confidence - a.confidence);
  const best = ranked[0];
  if (!best || best.rotation % 360 === 0) return 0;
  const runnerUp = ranked[1]?.confidence ?? 0;
  const asIs = scores.find((s) => s.rotation % 360 === 0);
  const asIsConfidence = asIs && asIs.words > 0 ? asIs.confidence : 0;
  return best.confidence >= runnerUp * MARGIN && best.confidence >= asIsConfidence * MARGIN
    ? ((best.rotation % 360) as Rotation)
    : 0;
}

/** `(rotation + delta) % 360`. */
export function addRotation(rotation: Rotation, delta: Rotation): Rotation {
  return ((rotation + delta) % 360) as Rotation;
}

/** A page image turned `delta` degrees clockwise, as a PNG blob, with its new size. */
export interface TurnedImage { blob: Blob; widthPx: number; heightPx: number }

/**
 * Turns `blob` (a PNG) `delta` degrees clockwise on a canvas. `delta = 0` hands the image back
 * untouched. The WebView (WKWebView ≥ 16.4, WebView2) has `createImageBitmap` and `OffscreenCanvas`;
 * a document canvas is the fallback.
 */
export async function turnImage(blob: Blob, widthPx: number, heightPx: number, delta: Rotation): Promise<TurnedImage> {
  const turn = (delta % 360) as Rotation;
  if (turn === 0) return { blob, widthPx, heightPx };
  const bitmap = await createImageBitmap(blob);
  const sideways = turn === 90 || turn === 270;
  const w = sideways ? heightPx : widthPx;
  const h = sideways ? widthPx : heightPx;
  const offscreen = typeof OffscreenCanvas !== "undefined" ? new OffscreenCanvas(w, h) : null;
  const canvas = offscreen ?? Object.assign(document.createElement("canvas"), { width: w, height: h });
  const ctx = canvas.getContext("2d") as CanvasRenderingContext2D | OffscreenCanvasRenderingContext2D | null;
  if (!ctx) throw new Error("no 2D canvas to turn the page image");
  ctx.fillStyle = "#fff";
  ctx.fillRect(0, 0, w, h);
  ctx.translate(w / 2, h / 2);
  ctx.rotate((turn * Math.PI) / 180);
  ctx.drawImage(bitmap, -widthPx / 2, -heightPx / 2);
  bitmap.close?.();
  const out = offscreen
    ? await offscreen.convertToBlob({ type: "image/png" })
    : await new Promise<Blob>((resolve, reject) =>
      (canvas as HTMLCanvasElement).toBlob((b) => (b ? resolve(b) : reject(new Error("toBlob failed"))), "image/png"));
  return { blob: out, widthPx: w, heightPx: h };
}

/** What `detectWithTesseract` needs from the job: a way to read one image. */
export type RecognizeImage = (image: Blob, dpi: number) => Promise<TessPage>;

/**
 * Tesseract's four trial reads of a `DETECT_DPI` page image; returns the turn and the scores.
 * `turn` is injectable for tests (jsdom has no canvas).
 */
export async function detectWithTesseract(
  a: {
    page: number; image: Blob; widthPx: number; heightPx: number; recognize: RecognizeImage;
    turn?: typeof turnImage; signal?: AbortSignal;
  },
): Promise<{ rotation: Rotation; scores: OrientationScore[] }> {
  const turn = a.turn ?? turnImage;
  const scores: OrientationScore[] = [];
  for (const delta of TURNS) {
    if (a.signal?.aborted) break;
    const turned = await turn(a.image, a.widthPx, a.heightPx, delta);
    const raw = await a.recognize(turned.blob, DETECT_DPI);
    const page = normalizeTesseract(raw, {
      page: a.page, dpi: DETECT_DPI, widthPx: turned.widthPx, heightPx: turned.heightPx, rotation: delta,
    });
    scores.push(scoreOf(delta, page));
  }
  return { rotation: pickOrientation(scores), scores };
}
