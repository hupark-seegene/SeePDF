/**
 * 서명 보관함 (P1-9): the saved signatures in `Settings.signatures`, at most
 * [`MAX_SAVED_SIGNATURES`], newest last. Pure functions over the list, so the rules are
 * testable without the dialog; `SignatureDialog` persists the result through
 * `appStore.patchSettings`.
 */
import type { SavedSignature } from "../ipc/types";
import type { DrawnSignature } from "../tools/stamp";

/** Same cap as the Rust side (`MAX_SAVED_SIGNATURES`). */
export const MAX_SAVED_SIGNATURES = 10;

/** 4 decimals of unit space is 1/10000 of the signature — far below a pixel, a third of the JSON. */
export function roundPaths(paths: number[][]): number[][] {
  return paths.map((stroke) => stroke.map((v) => Math.round(v * 10000) / 10000));
}

export function isFull(list: SavedSignature[]): boolean {
  return list.length >= MAX_SAVED_SIGNATURES;
}

function newId(): string {
  const c = globalThis.crypto;
  return c?.randomUUID ? c.randomUUID() : `sig-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 8)}`;
}

export function savedFromDrawn(signature: DrawnSignature, now = new Date()): SavedSignature {
  return { kind: "drawn", id: newId(), paths: roundPaths(signature.paths), aspect: signature.aspect, createdAt: now.toISOString() };
}

export function savedFromTyped(text: string, style: string, now = new Date()): SavedSignature {
  return { kind: "typed", id: newId(), text: text.trim(), style, createdAt: now.toISOString() };
}

/** v0.3 T3: a picked image (scan, 도장) — `path` is the library copy (`copy_library_image`). */
export function savedFromImage(path: string, aspect: number, now = new Date()): SavedSignature {
  return { kind: "image", id: newId(), path, aspect, createdAt: now.toISOString() };
}

/** Same signature, whatever its id: a re-save of an identical one is a no-op. */
export function sameSignature(a: SavedSignature, b: SavedSignature): boolean {
  if (a.kind === "typed" && b.kind === "typed") return a.text === b.text && a.style === b.style;
  if (a.kind === "drawn" && b.kind === "drawn") return JSON.stringify(a.paths) === JSON.stringify(b.paths);
  // the library copy is content-addressed: the same image has the same path
  if (a.kind === "image" && b.kind === "image") return a.path === b.path;
  return false;
}

/**
 * The list with `entry` appended, or `null` when it is full. Never evicts: dropping a
 * signature the user kept is their call (삭제), not ours. A duplicate leaves the list as is.
 */
export function addSignature(list: SavedSignature[], entry: SavedSignature): SavedSignature[] | null {
  if (list.some((s) => sameSignature(s, entry))) return list;
  if (isFull(list)) return null;
  return [...list, entry];
}

export function removeSignature(list: SavedSignature[], id: string): SavedSignature[] {
  return list.filter((s) => s.id !== id);
}

/** What `Settings.signatures` holds, defended against a hand-edited or older file. */
export function readSignatures(value: unknown): SavedSignature[] {
  if (!Array.isArray(value)) return [];
  const out: SavedSignature[] = [];
  for (const v of value) {
    if (!v || typeof v !== "object") continue;
    const s = v as Record<string, unknown>;
    if (typeof s.id !== "string") continue;
    if (s.kind === "typed" && typeof s.text === "string" && typeof s.style === "string") {
      out.push({ kind: "typed", id: s.id, text: s.text, style: s.style, createdAt: String(s.createdAt ?? "") });
    } else if (
      s.kind === "drawn" &&
      Array.isArray(s.paths) &&
      s.paths.every((p) => Array.isArray(p) && p.every((n) => typeof n === "number")) &&
      typeof s.aspect === "number"
    ) {
      out.push({ kind: "drawn", id: s.id, paths: s.paths as number[][], aspect: s.aspect, createdAt: String(s.createdAt ?? "") });
    } else if (s.kind === "image" && typeof s.path === "string" && typeof s.aspect === "number" && s.aspect > 0) {
      // v0.3 T3
      out.push({ kind: "image", id: s.id, path: s.path, aspect: s.aspect, createdAt: String(s.createdAt ?? "") });
    }
    if (out.length >= MAX_SAVED_SIGNATURES) break;
  }
  return out;
}
