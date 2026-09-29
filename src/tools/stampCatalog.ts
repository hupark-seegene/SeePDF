/**
 * The built-in stamps (P1-12): what the 도장 picker offers, how big each one is placed, and the
 * colour the overlay paints its ghost with. The engine draws the real thing
 * (`engine::annot::create::stamp` — a border plus the label, Hangul from the bundled subset) and
 * writes the id as `/Subj`, which is what `Annot.stampKind` reads back.
 *
 * Keep `color` in step with `builtin_color` in `src-tauri/src/engine/annot/create.rs`.
 */
import type { Rgb } from "../ipc/types";

export interface BuiltinStamp {
  /** the `StampImage.builtin` id, and the annotation's `/Subj` */
  id: string;
  /** what the stamp says (the engine upper-cases Latin ids the same way) */
  label: string;
  color: Rgb;
  /** default placement size, PDF points */
  size: { w: number; h: number };
}

/** 인주 red — `KOREAN_SEAL_RED` in `create.rs`. */
export const KOREAN_SEAL_RED: Rgb = [206, 32, 41];

export const BUILTIN_STAMPS: BuiltinStamp[] = [
  { id: "결재", label: "결재", color: KOREAN_SEAL_RED, size: { w: 64, h: 64 } },
  { id: "승인", label: "승인", color: KOREAN_SEAL_RED, size: { w: 64, h: 64 } },
  { id: "기밀", label: "기밀", color: KOREAN_SEAL_RED, size: { w: 96, h: 44 } },
  { id: "approved", label: "APPROVED", color: [0, 140, 60], size: { w: 150, h: 44 } },
  { id: "final", label: "FINAL", color: [0, 140, 60], size: { w: 120, h: 44 } },
  { id: "draft", label: "DRAFT", color: [200, 120, 0], size: { w: 120, h: 44 } },
  { id: "confidential", label: "CONFIDENTIAL", color: [200, 0, 0], size: { w: 170, h: 44 } },
];

export function builtinStamp(id: string | null | undefined): BuiltinStamp | null {
  if (!id) return null;
  return BUILTIN_STAMPS.find((s) => s.id === id) ?? null;
}

/**
 * A built-in stamp's `Annot.stampKind` is its id (the engine's `/Subj`). An image stamp has none
 * (the mock says `"image"`) and ours are `SeePDF:*`; a foreign stamp's `/Subj` only counts when
 * it is one of the catalogue's ids.
 */
export function isBuiltinStampKind(kind: string | null | undefined): kind is string {
  return !!builtinStamp(kind);
}

/** The engine's label rule (`builtin_label`): Latin upper-cased, Hangul as is. */
export function builtinLabel(id: string): string {
  return builtinStamp(id)?.label ?? id.toUpperCase();
}

/** The engine's colour rule (`builtin_color`), for ids outside the catalogue too. */
export function builtinColor(id: string): Rgb {
  const known = builtinStamp(id);
  if (known) return known.color;
  switch (id.toLowerCase()) {
    case "completed":
      return [0, 140, 60];
    case "forcomment":
    case "notapproved":
      return [200, 120, 0];
    case "urgent":
    case "void":
      return [200, 0, 0];
    default:
      return [40, 70, 160];
  }
}

// ---------------------------------------------------------------------------------------------
// v0.3 pkg4-annotations-stamps-objects (T2)
// ---------------------------------------------------------------------------------------------

/** ✓ ✗ ● — built-in ids the engine draws as vector marks (`create.rs` `QUICK_MARKS`). */
export const QUICK_MARKS = ["check", "cross", "dot"] as const;
export type QuickMark = (typeof QUICK_MARKS)[number];

/** Ink blue of the quick marks — `QUICK_MARK_COLOR` in `create.rs`. */
export const QUICK_MARK_COLOR: Rgb = [24, 64, 170];

export function isQuickMark(id: string | null | undefined): id is QuickMark {
  return !!id && (QUICK_MARKS as readonly string[]).includes(id);
}

/** 오늘 날짜: a borderless text stamp that says today's date (`{{date}}` expands at placement). */
export const TODAY_STAMP = { text: "{{date}}", color: [206, 32, 41] as Rgb, shape: "none" as const };

/** The `/Subj` of a custom text stamp — what `Annot.stampKind` reads back. */
export const TEXT_STAMP_KIND = "SeePDF:TextStamp";

/**
 * Default placement size of a text stamp: 40 pt high, as wide as the text at the engine's label
 * size (half the height), tokens counted at their expanded length.
 */
export function textStampSize(text: string): { w: number; h: number } {
  const shown = text.replaceAll("{{date}}", "0000.00.00").replaceAll("{{author}}", "홍길동");
  let em = 0;
  for (const ch of shown) em += /[ㄱ-힝]/.test(ch) ? 1 : 0.6;
  const w = Math.min(260, Math.max(64, em * 20 / 0.84 + 12));
  return { w: Math.round(w), h: 40 };
}
