import { invoke } from "@tauri-apps/api/core";

export interface BenchRow {
  name: string;
  ms: number;
  ok: boolean;
  note?: string;
}

export interface PutInfo {
  len: number;
  checksum: number;
  docHeader: string | null;
}

export const BENCH_BYTES = 14 * 1024 * 1024;

function sum(u8: Uint8Array): number {
  let s = 0;
  for (let i = 0; i < u8.length; i++) s += u8[i];
  return s;
}

function patternOk(u8: Uint8Array): boolean {
  // sample a few thousand positions of the i % 251 pattern
  for (let i = 0; i < u8.length; i += 4099) if (u8[i] !== i % 251) return false;
  return true;
}

function toBase64(u8: Uint8Array): string {
  const CHUNK = 0x8000;
  let bin = "";
  for (let i = 0; i < u8.length; i += CHUNK) {
    bin += String.fromCharCode.apply(null, u8.subarray(i, i + CHUNK) as unknown as number[]);
  }
  return btoa(bin);
}

function fromBase64(s: string): Uint8Array {
  const bin = atob(s);
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
}

const mbps = (bytes: number, ms: number) => `${(bytes / 1048576 / (ms / 1000)).toFixed(0)} MB/s`;

/** Runs every transport for a 14 MB payload in both directions. Reports each row as it lands. */
export async function runIpcBench(n: number, report: (row: BenchRow) => void): Promise<Uint8Array> {
  // 1. Rust -> JS raw bytes (tauri::ipc::Response -> ArrayBuffer)
  let t = performance.now();
  const buf = await invoke<ArrayBuffer>("ipc_get_bytes", { n });
  let ms = performance.now() - t;
  const u8 = new Uint8Array(buf);
  report({ name: "Rust→JS raw (ipc::Response → ArrayBuffer)", ms, ok: buf.byteLength === n && patternOk(u8), note: mbps(n, ms) });

  // 2. JS -> Rust raw bytes (Uint8Array as args -> InvokeBody::Raw) + header metadata
  const expected = sum(u8);
  t = performance.now();
  const put = await invoke<PutInfo>("ipc_put_bytes", u8, { headers: { "x-doc": "bench-doc" } });
  ms = performance.now() - t;
  report({ name: "JS→Rust raw (Uint8Array → InvokeBody::Raw)", ms, ok: put.len === n && put.checksum === expected && put.docHeader === "bench-doc", note: mbps(n, ms) });

  // 3. Rust -> JS base64 JSON string (+ atob decode)
  t = performance.now();
  const b64 = await invoke<string>("ipc_get_base64", { n });
  const tInvoke = performance.now() - t;
  const dec = fromBase64(b64);
  ms = performance.now() - t;
  report({ name: "Rust→JS base64 string (JSON) + atob", ms, ok: dec.length === n && patternOk(dec), note: `${mbps(n, ms)}; invoke alone ${tInvoke.toFixed(0)} ms` });

  // 4. JS -> Rust base64 (btoa encode + JSON string)
  t = performance.now();
  const enc = toBase64(u8);
  const tEnc = performance.now() - t;
  const putB = await invoke<PutInfo>("ipc_put_base64", { data: enc });
  ms = performance.now() - t;
  report({ name: "JS→Rust base64 string (btoa + JSON)", ms, ok: putB.len === n && putB.checksum === expected, note: `${mbps(n, ms)}; btoa alone ${tEnc.toFixed(0)} ms` });

  // 5. Rust -> JS Vec<u8> as JSON number array
  t = performance.now();
  const arr = await invoke<number[]>("ipc_get_json_array", { n });
  const tArr = performance.now() - t;
  const fromArr = Uint8Array.from(arr);
  ms = performance.now() - t;
  report({ name: "Rust→JS Vec<u8> as JSON number array", ms, ok: fromArr.length === n && patternOk(fromArr), note: `${mbps(n, ms)}; invoke alone ${tArr.toFixed(0)} ms` });

  // 6. JS -> Rust JSON number array (Array.from(u8) as a normal arg)
  t = performance.now();
  const plain = Array.from(u8);
  const tConv = performance.now() - t;
  const putA = await invoke<PutInfo>("ipc_put_json_array", { data: plain });
  ms = performance.now() - t;
  report({ name: "JS→Rust number[] as JSON arg (Vec<u8>)", ms, ok: putA.len === n && putA.checksum === expected, note: `${mbps(n, ms)}; Array.from ${tConv.toFixed(0)} ms` });

  return u8;
}
