/**
 * 서명 → 입력 (P1-9): a typed name rendered in a script-like style to a PNG.
 *
 * Offline by construction: every style is a CSS `font-family` stack of fonts the OS already has
 * (macOS: Snell Roundhand, Bradley Hand, AppleMyungjo; Windows: Segoe Script, Segoe Print, Ink
 * Free, Batang/바탕) and nothing is downloaded. Hangul falls back per glyph to the system Korean
 * font, which is why the third style is a 명조 (serif) stack — the one Korean look every
 * install has.
 *
 * The PNG has a transparent background (cargo `annot_stamp_png_keeps_transparency` pins that the
 * page shows through) and is written by `write_signature_image`, then placed through the
 * existing `StampImage { path }` spec.
 */
export interface SignatureStyle {
  id: string;
  labelKey: string;
  fontFamily: string;
  fontStyle?: "italic";
}

export const SIGNATURE_STYLES: SignatureStyle[] = [
  {
    id: "script",
    labelKey: "sign.style.script",
    fontFamily:
      '"Snell Roundhand", "Segoe Script", "Apple Chancery", "Brush Script MT", "Nanum Pen Script", "나눔손글씨 펜", cursive',
  },
  {
    id: "hand",
    labelKey: "sign.style.hand",
    fontFamily: '"Bradley Hand", "Segoe Print", "Ink Free", "Noteworthy", "Nanum Pen Script", "나눔손글씨 펜", cursive',
  },
  {
    id: "formal",
    labelKey: "sign.style.formal",
    fontFamily: '"AppleMyungjo", "Batang", "바탕", "Nanum Myeongjo", "Georgia", "Times New Roman", serif',
    fontStyle: "italic",
  },
];

export const DEFAULT_SIGNATURE_STYLE = SIGNATURE_STYLES[0].id;
/** Longest name the 입력 field accepts. */
export const MAX_TYPED_LENGTH = 40;
/** Ink of a typed signature — the drawn signature's `SIGNATURE_INK` colour. */
export const TYPED_INK = "#111318";
/** Render size: ~4× the 160 pt placement, so it stays sharp when printed. */
const FONT_PX = 96;

export function signatureStyle(id: string): SignatureStyle {
  return SIGNATURE_STYLES.find((s) => s.id === id) ?? SIGNATURE_STYLES[0];
}

/** The CSS `font` shorthand for a style at `px`. */
export function signatureFont(id: string, px: number): string {
  const style = signatureStyle(id);
  return `${style.fontStyle ?? "normal"} 400 ${px}px ${style.fontFamily}`;
}

export interface RenderedSignature {
  bytes: Uint8Array;
  /** width ÷ height of the PNG, so the placement keeps its shape */
  aspect: number;
}

/**
 * Draws `text` tight to its ink box (plus a small margin) and encodes a PNG. Throws when the
 * webview cannot give a 2D context (never in a real webview; jsdom tests mock this module).
 */
export async function renderTypedSignature(text: string, styleId: string): Promise<RenderedSignature> {
  const value = text.trim().slice(0, MAX_TYPED_LENGTH);
  if (!value) throw new Error("empty signature");
  const font = signatureFont(styleId, FONT_PX);
  // A system font is normally ready at once; `load` makes sure before we measure.
  try {
    await document.fonts?.load(font, value);
  } catch {
    // no FontFaceSet (old webview): measure with whatever resolves
  }
  const canvas = document.createElement("canvas");
  let ctx = canvas.getContext("2d");
  if (!ctx) throw new Error("no 2d canvas context");
  ctx.font = font;
  const m = ctx.measureText(value);
  const ascent = m.actualBoundingBoxAscent || FONT_PX * 0.8;
  const descent = m.actualBoundingBoxDescent || FONT_PX * 0.25;
  const left = m.actualBoundingBoxLeft || 0;
  const right = m.actualBoundingBoxRight || m.width;
  const pad = Math.round(FONT_PX * 0.12);
  canvas.width = Math.min(4096, Math.ceil(left + right + pad * 2));
  canvas.height = Math.min(4096, Math.ceil(ascent + descent + pad * 2));
  // Resizing resets the context state.
  ctx = canvas.getContext("2d")!;
  ctx.font = font;
  ctx.fillStyle = TYPED_INK;
  ctx.textBaseline = "alphabetic";
  ctx.fillText(value, pad + left, pad + ascent);
  const blob = await new Promise<Blob | null>((resolve) => canvas.toBlob(resolve, "image/png"));
  if (!blob) throw new Error("canvas.toBlob gave nothing");
  return { bytes: new Uint8Array(await blob.arrayBuffer()), aspect: canvas.width / canvas.height };
}
