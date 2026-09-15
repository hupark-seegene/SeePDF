/**
 * Binary payload formats of IPC_CONTRACT §10 — the two responses that travel as `ArrayBuffer`
 * instead of JSON. All little-endian; every section starts 4-byte aligned.
 *
 *   §10.1 "STXL"  get_text_layer   — chars / words / lines / page text (~32 B per char)
 *   §10.2 "SPRX"  render_page_raw  — RGBA8 pixels, top-left origin, straight alpha
 *
 * `src/viewer/text/TextLayer.ts` (owner (c)) wraps `parseTextLayer()` — it must never create a DOM
 * node per character. The encoders exist for the mock adapter and the round-trip tests.
 */
import type { Mat6, PageIndex, Rect } from "./types";

const MAGIC_STXL = 0x4c585453; // "STXL" as a little-endian u32
const MAGIC_SPRX = 0x58525053; // "SPRX"

export const CHAR_STRIDE = 32;
export const WORD_STRIDE = 12;
export const LINE_STRIDE = 36;
export const TEXT_LAYER_HEADER_BYTES = 64;

/** `chars[i].flags` bits (§10.1). */
export const CHAR_GENERATED = 1;
export const CHAR_HYPHEN = 2;
export const CHAR_SPACE = 4;

export interface TextLayerHeader {
  version: number;
  flags: number;
  hasObjectIds: boolean;
  pageIndex: PageIndex;
  charCount: number;
  wordCount: number;
  lineCount: number;
  /** page user space -> device px at s = 1 (crop box + /Rotate) */
  matrix: Mat6;
  crop: Rect;
}

export interface TextChar {
  code: number;
  box: Rect;            // loose box, PDF user space
  baselineY: number;
  fontSizePt: number;
  flags: number;
  objectId: number;
}

export interface TextWord { firstChar: number; charCount: number; lineIndex: number }
export interface TextLine {
  firstWord: number; wordCount: number; baselineY: number; box: Rect;
  firstChar: number; charCount: number;
}

/** Zero-copy views over a `get_text_layer` buffer. */
export class TextLayerView {
  readonly header: TextLayerHeader;
  readonly text: string;
  private readonly charU32: Uint32Array;
  private readonly charF32: Float32Array;
  private readonly charBytes: Uint8Array;
  private readonly wordU32: Uint32Array;
  private readonly lineU32: Uint32Array;
  private readonly lineF32: Float32Array;

  constructor(buffer: ArrayBuffer) {
    const dv = new DataView(buffer);
    if (buffer.byteLength < TEXT_LAYER_HEADER_BYTES || dv.getUint32(0, true) !== MAGIC_STXL) {
      throw new Error("not a SeePDF text layer (bad STXL magic)");
    }
    const charCount = dv.getUint32(12, true);
    const wordCount = dv.getUint32(16, true);
    const lineCount = dv.getUint32(20, true);
    const matrix: number[] = [];
    for (let i = 0; i < 6; i++) matrix.push(dv.getFloat32(24 + i * 4, true));
    this.header = {
      version: dv.getUint16(4, true),
      flags: dv.getUint16(6, true),
      hasObjectIds: (dv.getUint16(6, true) & 1) !== 0,
      pageIndex: dv.getUint32(8, true),
      charCount,
      wordCount,
      lineCount,
      matrix: matrix as unknown as Mat6,
      crop: {
        l: dv.getFloat32(48, true),
        b: dv.getFloat32(52, true),
        r: dv.getFloat32(56, true),
        t: dv.getFloat32(60, true),
      },
    };

    const charsAt = TEXT_LAYER_HEADER_BYTES;
    const wordsAt = charsAt + charCount * CHAR_STRIDE;
    const linesAt = wordsAt + wordCount * WORD_STRIDE;
    const textAt = linesAt + lineCount * LINE_STRIDE;

    this.charU32 = new Uint32Array(buffer, charsAt, charCount * 8);
    this.charF32 = new Float32Array(buffer, charsAt, charCount * 8);
    this.charBytes = new Uint8Array(buffer, charsAt, charCount * CHAR_STRIDE);
    this.wordU32 = new Uint32Array(buffer, wordsAt, wordCount * 3);
    this.lineU32 = new Uint32Array(buffer, linesAt, lineCount * 9);
    this.lineF32 = new Float32Array(buffer, linesAt, lineCount * 9);

    const textLen = textAt + 4 <= buffer.byteLength ? dv.getUint32(textAt, true) : 0;
    this.text = textLen
      ? new TextDecoder().decode(new Uint8Array(buffer, textAt + 4, textLen))
      : "";
  }

  char(i: number): TextChar {
    const u = i * 8;
    return {
      code: this.charU32[u],
      box: { l: this.charF32[u + 1], b: this.charF32[u + 2], r: this.charF32[u + 3], t: this.charF32[u + 4] },
      baselineY: this.charF32[u + 5],
      fontSizePt: ((this.charBytes[i * CHAR_STRIDE + 24] | (this.charBytes[i * CHAR_STRIDE + 25] << 8)) & 0xffff) / 10,
      flags: this.charBytes[i * CHAR_STRIDE + 26],
      objectId: this.charU32[u + 7],
    };
  }

  word(i: number): TextWord {
    const u = i * 3;
    return { firstChar: this.wordU32[u], charCount: this.wordU32[u + 1], lineIndex: this.wordU32[u + 2] };
  }

  line(i: number): TextLine {
    const u = i * 9;
    return {
      firstWord: this.lineU32[u],
      wordCount: this.lineU32[u + 1],
      baselineY: this.lineF32[u + 2],
      box: { l: this.lineF32[u + 3], b: this.lineF32[u + 4], r: this.lineF32[u + 5], t: this.lineF32[u + 6] },
      firstChar: this.lineU32[u + 7],
      charCount: this.lineU32[u + 8],
    };
  }

  /** The substring a word covers, from the decoded page text. */
  wordText(i: number): string {
    const w = this.word(i);
    let out = "";
    for (let c = w.firstChar; c < w.firstChar + w.charCount; c++) out += String.fromCodePoint(this.char(c).code);
    return out;
  }
}

export function parseTextLayer(buffer: ArrayBuffer): TextLayerView {
  return new TextLayerView(buffer);
}

export interface TextLayerInput {
  pageIndex: PageIndex;
  matrix: Mat6;
  crop: Rect;
  chars: TextChar[];
  words: TextWord[];
  lines: TextLine[];
  text: string;
  hasObjectIds?: boolean;
}

/** Build a §10.1 buffer (mock adapter + round-trip tests). */
export function encodeTextLayer(input: TextLayerInput): ArrayBuffer {
  const textBytes = new TextEncoder().encode(input.text);
  const textPadded = (textBytes.length + 3) & ~3;
  const size =
    TEXT_LAYER_HEADER_BYTES +
    input.chars.length * CHAR_STRIDE +
    input.words.length * WORD_STRIDE +
    input.lines.length * LINE_STRIDE +
    4 + textPadded;
  const buffer = new ArrayBuffer(size);
  const dv = new DataView(buffer);
  dv.setUint32(0, MAGIC_STXL, true);
  dv.setUint16(4, 1, true);
  dv.setUint16(6, input.hasObjectIds ? 1 : 0, true);
  dv.setUint32(8, input.pageIndex, true);
  dv.setUint32(12, input.chars.length, true);
  dv.setUint32(16, input.words.length, true);
  dv.setUint32(20, input.lines.length, true);
  for (let i = 0; i < 6; i++) dv.setFloat32(24 + i * 4, input.matrix[i], true);
  dv.setFloat32(48, input.crop.l, true);
  dv.setFloat32(52, input.crop.b, true);
  dv.setFloat32(56, input.crop.r, true);
  dv.setFloat32(60, input.crop.t, true);

  let at = TEXT_LAYER_HEADER_BYTES;
  for (const c of input.chars) {
    dv.setUint32(at, c.code, true);
    dv.setFloat32(at + 4, c.box.l, true);
    dv.setFloat32(at + 8, c.box.b, true);
    dv.setFloat32(at + 12, c.box.r, true);
    dv.setFloat32(at + 16, c.box.t, true);
    dv.setFloat32(at + 20, c.baselineY, true);
    dv.setUint16(at + 24, Math.round(c.fontSizePt * 10), true);
    dv.setUint8(at + 26, c.flags);
    dv.setUint8(at + 27, 0);
    dv.setUint32(at + 28, c.objectId, true);
    at += CHAR_STRIDE;
  }
  for (const w of input.words) {
    dv.setUint32(at, w.firstChar, true);
    dv.setUint32(at + 4, w.charCount, true);
    dv.setUint32(at + 8, w.lineIndex, true);
    at += WORD_STRIDE;
  }
  for (const l of input.lines) {
    dv.setUint32(at, l.firstWord, true);
    dv.setUint32(at + 4, l.wordCount, true);
    dv.setFloat32(at + 8, l.baselineY, true);
    dv.setFloat32(at + 12, l.box.l, true);
    dv.setFloat32(at + 16, l.box.b, true);
    dv.setFloat32(at + 20, l.box.r, true);
    dv.setFloat32(at + 24, l.box.t, true);
    dv.setUint32(at + 28, l.firstChar, true);
    dv.setUint32(at + 32, l.charCount, true);
    at += LINE_STRIDE;
  }
  dv.setUint32(at, textBytes.length, true);
  new Uint8Array(buffer, at + 4, textBytes.length).set(textBytes);
  return buffer;
}

// ---------------------------------------------------------------------------
// §10.2 raw page buffer
// ---------------------------------------------------------------------------

export interface RawPage { width: number; height: number; stride: number; pixels: Uint8ClampedArray }

export function parseRawPage(buffer: ArrayBuffer): RawPage {
  const dv = new DataView(buffer);
  if (buffer.byteLength < 32 || dv.getUint32(0, true) !== MAGIC_SPRX) {
    throw new Error("not a SeePDF raw page (bad SPRX magic)");
  }
  const format = dv.getUint16(6, true);
  if (format !== 1) throw new Error(`unsupported raw page format ${format}`);
  const width = dv.getUint32(8, true);
  const height = dv.getUint32(12, true);
  const stride = dv.getUint32(16, true);
  return { width, height, stride, pixels: new Uint8ClampedArray(buffer, 32, stride * height) };
}

export function encodeRawPage(width: number, height: number, pixels: Uint8ClampedArray | Uint8Array): ArrayBuffer {
  const stride = width * 4;
  const buffer = new ArrayBuffer(32 + stride * height);
  const dv = new DataView(buffer);
  dv.setUint32(0, MAGIC_SPRX, true);
  dv.setUint16(4, 1, true);
  dv.setUint16(6, 1, true);
  dv.setUint32(8, width, true);
  dv.setUint32(12, height, true);
  dv.setUint32(16, stride, true);
  new Uint8Array(buffer, 32).set(pixels.subarray(0, stride * height));
  return buffer;
}
