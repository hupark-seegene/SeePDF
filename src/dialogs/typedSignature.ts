/**
 * 서명 → 입력 (P1-9): a typed name rendered in a script-like style to a PNG.
 *
 * Offline by construction: every style is a CSS `font-family` stack of fonts the OS already has
 * and nothing is downloaded. The styles on offer follow the name's script:
 *
 * * **Latin** — 필기체 / 손글씨 / 정자체 (macOS: Snell Roundhand, Bradley Hand, AppleMyungjo;
 *   Windows: Segoe Script, Segoe Print / Ink Free, Batang).
 * * **Hangul** (Stage 8) — only faces a stock install really has, because a Latin script font
 *   would fall back per glyph to the plain system Korean font: 궁서체 (Gungsuh, part of
 *   Windows' Korean fonts — not on macOS), 명조 (AppleMyungjo / Batang) and 펜글씨 (Apple SD
 *   Gothic Neo UltraLight / Malgun Gothic Semilight with a drawn 12° lean). There is no Korean
 *   handwriting font on a stock macOS (Nanum Pen Script is a download), so none is claimed.
 *
 * A style whose fonts are not installed is not offered (`installedFamily`, a canvas width probe),
 * so two buttons never render the same fallback face. When the probe cannot run (no canvas) every
 * style is kept.
 *
 * The PNG has a transparent background (cargo `annot_stamp_png_keeps_transparency` pins that the
 * page shows through) and is written by `write_signature_image`, then placed through the
 * existing `StampImage { path }` spec with `signature: true`.
 */
export type SignatureScript = "latin" | "hangul";

export interface SignatureStyle {
  id: string;
  labelKey: string;
  script: SignatureScript;
  fontFamily: string;
  fontStyle?: "italic";
  fontWeight?: number;
  /** a forward lean in degrees, drawn as a skew (a handwriting slant for an upright face) */
  slantDeg?: number;
  /** offered only when one of these families is installed; absent = always offered */
  requires?: string[];
}

export const SIGNATURE_STYLES: SignatureStyle[] = [
  {
    id: "script",
    labelKey: "sign.style.script",
    script: "latin",
    fontFamily: '"Snell Roundhand", "Segoe Script", "Apple Chancery", "Brush Script MT", cursive',
  },
  {
    id: "hand",
    labelKey: "sign.style.hand",
    script: "latin",
    fontFamily: '"Bradley Hand", "Segoe Print", "Ink Free", "Noteworthy", cursive',
  },
  {
    id: "formal",
    labelKey: "sign.style.formal",
    script: "latin",
    fontFamily: '"AppleMyungjo", "Batang", "바탕", "Georgia", "Times New Roman", serif',
    fontStyle: "italic",
  },
  {
    id: "kr-brush",
    labelKey: "sign.style.krBrush",
    script: "hangul",
    fontFamily: '"Gungsuh", "궁서", "GungsuhChe", "궁서체"',
    requires: ["Gungsuh", "궁서"],
  },
  {
    id: "kr-myungjo",
    labelKey: "sign.style.krMyungjo",
    script: "hangul",
    fontFamily: '"AppleMyungjo", "Batang", "바탕", serif',
    requires: ["AppleMyungjo", "Batang", "바탕"],
  },
  {
    id: "kr-pen",
    labelKey: "sign.style.krPen",
    script: "hangul",
    fontFamily: '"Apple SD Gothic Neo", "Malgun Gothic", "맑은 고딕", sans-serif',
    fontWeight: 200,
    slantDeg: 12,
    requires: ["Apple SD Gothic Neo", "Malgun Gothic", "맑은 고딕"],
  },
];

export const DEFAULT_SIGNATURE_STYLE = SIGNATURE_STYLES[0].id;
/** Longest name the 입력 field accepts. */
export const MAX_TYPED_LENGTH = 40;
/** Ink of a typed signature — the drawn signature's `SIGNATURE_INK` colour. */
export const TYPED_INK = "#111318";
/** Render size: ~4× the 160 pt placement, so it stays sharp when printed. */
const FONT_PX = 96;

const HANGUL = /[ᄀ-ᇿ㄰-㆏가-힯]/;

/** A name with any Hangul is styled with the Korean faces. */
export function scriptOf(text: string): SignatureScript {
  return HANGUL.test(text) ? "hangul" : "latin";
}

export function signatureStyle(id: string): SignatureStyle {
  return SIGNATURE_STYLES.find((s) => s.id === id) ?? SIGNATURE_STYLES[0];
}

/**
 * Is `family` installed? The classic width probe: a sample measured in `"family", base` differs
 * from `base` alone for some generic base only when the family resolved. `null` = cannot tell
 * (no 2D canvas). Replaceable in tests.
 */
export const fontProbe = {
  installed(family: string): boolean | null {
    const cached = probeCache.get(family);
    if (cached !== undefined) return cached;
    let ctx: CanvasRenderingContext2D | null = null;
    try {
      ctx = typeof document === "undefined" ? null : document.createElement("canvas").getContext("2d");
    } catch {
      ctx = null;
    }
    if (!ctx) return null;
    const sample = "가나다라마 Aa Bb 123";
    const width = (font: string) => {
      ctx!.font = `72px ${font}`;
      return ctx!.measureText(sample).width;
    };
    const found = ["monospace", "serif", "sans-serif"].some((base) => width(`"${family}", ${base}`) !== width(base));
    probeCache.set(family, found);
    return found;
  },
};
const probeCache = new Map<string, boolean>();

function offered(style: SignatureStyle): boolean {
  if (!style.requires) return true;
  let unknown = false;
  for (const family of style.requires) {
    const found = fontProbe.installed(family);
    if (found) return true;
    if (found === null) unknown = true;
  }
  return unknown;
}

/** The styles to offer for `text`: its script's, minus those whose fonts are missing. */
export function stylesFor(text: string): SignatureStyle[] {
  const script = scriptOf(text);
  const list = SIGNATURE_STYLES.filter((s) => s.script === script && offered(s));
  // never empty: 명조 / 정자체 stacks end in a generic family that always renders
  return list.length ? list : SIGNATURE_STYLES.filter((s) => s.script === script).slice(0, 1);
}

/** `id` when it is on offer for `text`, else the first style that is. */
export function effectiveStyle(text: string, id: string): string {
  const list = stylesFor(text);
  return list.some((s) => s.id === id) ? id : list[0].id;
}

/** The CSS `font` shorthand for a style at `px`. */
export function signatureFont(id: string, px: number): string {
  const style = signatureStyle(id);
  return `${style.fontStyle ?? "normal"} ${style.fontWeight ?? 400} ${px}px ${style.fontFamily}`;
}

/** Inline CSS for a sample of the style (the style picker, the 보관함 thumbnails). */
export function signatureCss(id: string): {
  fontFamily: string;
  fontStyle?: "italic";
  fontWeight?: number;
  transform?: string;
  display?: "inline-block";
} {
  const style = signatureStyle(id);
  return {
    fontFamily: style.fontFamily,
    fontStyle: style.fontStyle,
    fontWeight: style.fontWeight,
    ...(style.slantDeg ? { transform: `skewX(${-style.slantDeg}deg)`, display: "inline-block" as const } : {}),
  };
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
  const lean = Math.tan(((signatureStyle(styleId).slantDeg ?? 0) * Math.PI) / 180);
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
  // the lean moves the tops right by `lean × ascent` and the descenders left by `lean × descent`
  const leanLeft = Math.ceil(lean * descent);
  const leanRight = Math.ceil(lean * ascent);
  canvas.width = Math.min(4096, Math.ceil(left + right + pad * 2 + leanLeft + leanRight));
  canvas.height = Math.min(4096, Math.ceil(ascent + descent + pad * 2));
  // Resizing resets the context state.
  ctx = canvas.getContext("2d")!;
  ctx.font = font;
  ctx.fillStyle = TYPED_INK;
  ctx.textBaseline = "alphabetic";
  const baseline = pad + ascent;
  // x' = x + lean × (baseline − y): upright at the baseline, leaning forward above it
  if (lean) ctx.setTransform(1, 0, -lean, 1, lean * baseline, 0);
  ctx.fillText(value, pad + left + leanLeft, baseline);
  const blob = await new Promise<Blob | null>((resolve) => canvas.toBlob(resolve, "image/png"));
  if (!blob) throw new Error("canvas.toBlob gave nothing");
  return { bytes: new Uint8Array(await blob.arrayBuffer()), aspect: canvas.width / canvas.height };
}
