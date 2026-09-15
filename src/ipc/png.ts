/**
 * A tiny PNG encoder (RGBA8, stored/uncompressed deflate) used only by the mock adapter, so the
 * shell can paint real `image/png` bytes as `data:` URLs without a backend or a canvas.
 * ~100 lines, no dependency; the real app never encodes PNG in JS (the engine does, IPC_CONTRACT §9).
 */

const SIGNATURE = [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a];

let crcTable: Uint32Array | null = null;
function crc32(bytes: Uint8Array): number {
  if (!crcTable) {
    crcTable = new Uint32Array(256);
    for (let n = 0; n < 256; n++) {
      let c = n;
      for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
      crcTable[n] = c >>> 0;
    }
  }
  let c = 0xffffffff;
  for (let i = 0; i < bytes.length; i++) c = crcTable[(c ^ bytes[i]) & 0xff] ^ (c >>> 8);
  return (c ^ 0xffffffff) >>> 0;
}

function adler32(bytes: Uint8Array): number {
  let a = 1;
  let b = 0;
  for (let i = 0; i < bytes.length; i++) {
    a = (a + bytes[i]) % 65521;
    b = (b + a) % 65521;
  }
  return ((b << 16) | a) >>> 0;
}

function chunk(type: string, data: Uint8Array): Uint8Array {
  const out = new Uint8Array(12 + data.length);
  const dv = new DataView(out.buffer);
  dv.setUint32(0, data.length, false);
  for (let i = 0; i < 4; i++) out[4 + i] = type.charCodeAt(i);
  out.set(data, 8);
  dv.setUint32(8 + data.length, crc32(out.subarray(4, 8 + data.length)), false);
  return out;
}

/** zlib stream with stored (uncompressed) deflate blocks — valid, and encodes in one pass. */
function zlibStore(raw: Uint8Array): Uint8Array {
  const MAX = 65535;
  const blocks = Math.max(1, Math.ceil(raw.length / MAX));
  const out = new Uint8Array(2 + blocks * 5 + raw.length + 4);
  let at = 0;
  out[at++] = 0x78;
  out[at++] = 0x01;
  for (let i = 0; i < blocks; i++) {
    const start = i * MAX;
    const len = Math.min(MAX, raw.length - start);
    out[at++] = i === blocks - 1 ? 1 : 0;
    out[at++] = len & 0xff;
    out[at++] = (len >> 8) & 0xff;
    out[at++] = ~len & 0xff;
    out[at++] = (~len >> 8) & 0xff;
    out.set(raw.subarray(start, start + len), at);
    at += len;
  }
  const sum = adler32(raw);
  out[at++] = (sum >>> 24) & 0xff;
  out[at++] = (sum >>> 16) & 0xff;
  out[at++] = (sum >>> 8) & 0xff;
  out[at++] = sum & 0xff;
  return out.subarray(0, at);
}

/** Encode RGBA8 pixels (row-major, top-left origin) as PNG bytes. */
export function encodePng(width: number, height: number, rgba: Uint8Array): Uint8Array {
  const ihdr = new Uint8Array(13);
  const dv = new DataView(ihdr.buffer);
  dv.setUint32(0, width, false);
  dv.setUint32(4, height, false);
  ihdr[8] = 8;   // bit depth
  ihdr[9] = 6;   // colour type RGBA
  const raw = new Uint8Array(height * (1 + width * 4));
  for (let y = 0; y < height; y++) {
    raw[y * (1 + width * 4)] = 0; // filter: none
    raw.set(rgba.subarray(y * width * 4, (y + 1) * width * 4), y * (1 + width * 4) + 1);
  }
  const parts = [
    new Uint8Array(SIGNATURE),
    chunk("IHDR", ihdr),
    chunk("IDAT", zlibStore(raw)),
    chunk("IEND", new Uint8Array(0)),
  ];
  const total = parts.reduce((n, p) => n + p.length, 0);
  const png = new Uint8Array(total);
  let at = 0;
  for (const p of parts) {
    png.set(p, at);
    at += p.length;
  }
  return png;
}

export function toDataUrl(png: Uint8Array): string {
  let bin = "";
  const CHUNK = 0x8000;
  for (let i = 0; i < png.length; i += CHUNK) {
    bin += String.fromCharCode.apply(null, png.subarray(i, i + CHUNK) as unknown as number[]);
  }
  return `data:image/png;base64,${btoa(bin)}`;
}

export type RgbaPixel = [number, number, number, number];

/** `data:image/png` of one solid colour (the 1×1 tile of IPC_CONTRACT §13). */
export function solidPngDataUrl(width: number, height: number, [r, g, b, a]: RgbaPixel): string {
  const rgba = new Uint8Array(width * height * 4);
  for (let i = 0; i < width * height; i++) {
    rgba[i * 4] = r;
    rgba[i * 4 + 1] = g;
    rgba[i * 4 + 2] = b;
    rgba[i * 4 + 3] = a;
  }
  return toDataUrl(encodePng(width, height, rgba));
}

/** `data:image/png` from a per-pixel function — used for the mock's "page of text" placeholder. */
export function pngDataUrl(width: number, height: number, pixel: (x: number, y: number) => RgbaPixel): string {
  const rgba = new Uint8Array(width * height * 4);
  for (let y = 0; y < height; y++) {
    for (let x = 0; x < width; x++) {
      const [r, g, b, a] = pixel(x, y);
      const i = (y * width + x) * 4;
      rgba[i] = r;
      rgba[i + 1] = g;
      rgba[i + 2] = b;
      rgba[i + 3] = a;
    }
  }
  return toDataUrl(encodePng(width, height, rgba));
}
