/**
 * 페이지 레이블 (P2) — the /PageLabels number tree as the dialog edits it, and the labels it
 * produces. The formatting follows ISO 32000 §12.4.2 and PDFium's `CPDF_PageLabel` exactly (the
 * engine verifies what it wrote against `FPDF_GetPageLabel`, so the preview must agree):
 *
 *   decimal 1, 2, 3 · roman i, ii, iii (thousands as repeated m) · letters a…z, aa…zz, aaa… (the
 *   letter repeated, not base 26) · none = the prefix alone.
 *
 * Pure functions only: the mock adapter, the dialog and its tests share them.
 */
import type { PageLabelRange, PageLabelStyle } from "../ipc/types";

export const LABEL_STYLES: PageLabelStyle[] = ["decimal", "roman", "romanUpper", "alpha", "alphaUpper", "none"];

const ROMAN: [number, string][] = [
  [1000, "m"], [900, "cm"], [500, "d"], [400, "cd"], [100, "c"], [90, "xc"],
  [50, "l"], [40, "xl"], [10, "x"], [9, "ix"], [5, "v"], [4, "iv"], [1, "i"],
];

export function toRoman(n: number): string {
  let rest = Math.floor(n) % 1_000_000;
  let out = "";
  for (const [value, digits] of ROMAN) {
    while (rest >= value) {
      out += digits;
      rest -= value;
    }
  }
  return out;
}

/** a…z for 1…26, then aa…zz, aaa… — the letter repeated (ISO 32000, PDFium `MakeLetters`). */
export function toLetters(n: number): string {
  if (n < 1) return "";
  const i = Math.floor(n) - 1;
  const count = (Math.floor(i / 26) + 1) % 1000;
  return String.fromCharCode(97 + (i % 26)).repeat(count);
}

/** The number part of a label, `n` ≥ 1. */
export function formatNumber(n: number, style: PageLabelStyle): string {
  switch (style) {
    case "decimal":
      return String(n);
    case "roman":
      return toRoman(n);
    case "romanUpper":
      return toRoman(n).toUpperCase();
    case "alpha":
      return toLetters(n);
    case "alphaUpper":
      return toLetters(n).toUpperCase();
    case "none":
      return "";
  }
}

/**
 * Sorted by `start`, duplicates and out-of-range starts dropped, and — like the engine — a leading
 * decimal range added when the first range does not start at page 1 (the tree must cover page 0).
 */
export function normalizeRanges(ranges: PageLabelRange[], pageCount: number): PageLabelRange[] {
  const seen = new Set<number>();
  const sorted = [...ranges]
    .filter((r) => r.start >= 0 && r.start < pageCount)
    .sort((a, b) => a.start - b.start)
    .filter((r) => (seen.has(r.start) ? false : (seen.add(r.start), true)));
  if (sorted.length && sorted[0].start > 0) sorted.unshift({ start: 0, style: "decimal" });
  return sorted;
}

/** Every page's label under `ranges` (`[]` when there are none). */
export function labelsFor(ranges: PageLabelRange[], pageCount: number): string[] {
  const list = normalizeRanges(ranges, pageCount);
  if (!list.length) return [];
  const out: string[] = [];
  for (let page = 0; page < pageCount; page++) {
    let range = list[0];
    for (const r of list) if (r.start <= page) range = r;
    const n = (range.first ?? 1) + (page - range.start);
    out.push(`${range.prefix ?? ""}${formatNumber(n, range.style)}`);
  }
  return out;
}

// ---------------------------------------------------------------------------
// The dialog's rows
// ---------------------------------------------------------------------------

/** One editable range: the text fields stay strings so a half-typed number is not lost. */
export interface LabelRow {
  key: number;
  /** 1-based page number, as typed */
  start: string;
  style: PageLabelStyle;
  prefix: string;
  /** the first number, as typed ("" = 1) */
  first: string;
}

let rowKey = 0;

export function newRow(start: number, style: PageLabelStyle = "decimal"): LabelRow {
  return { key: ++rowKey, start: String(start), style, prefix: "", first: "" };
}

export function rowsFromRanges(ranges: PageLabelRange[]): LabelRow[] {
  return ranges.map((r) => ({
    key: ++rowKey,
    start: String(r.start + 1),
    style: r.style,
    prefix: r.prefix ?? "",
    first: r.first && r.first !== 1 ? String(r.first) : "",
  }));
}

export type RowError = "start" | "duplicate" | "first" | null;

/** The first problem of `row` among `rows`, or `null`. */
export function rowError(row: LabelRow, rows: LabelRow[], pageCount: number): RowError {
  const start = Number(row.start);
  if (!/^\d+$/.test(row.start.trim()) || start < 1 || start > pageCount) return "start";
  if (rows.some((r) => r.key !== row.key && Number(r.start) === start)) return "duplicate";
  // the engine's bound: 1..1 000 000
  if (row.first.trim() && (!/^\d+$/.test(row.first.trim()) || Number(row.first) < 1 || Number(row.first) > 1_000_000)) return "first";
  return null;
}

export function rowsValid(rows: LabelRow[], pageCount: number): boolean {
  return rows.every((r) => rowError(r, rows, pageCount) === null);
}

/** The wire form: 0-based starts, sorted; an empty prefix and a first number of 1 are omitted. */
export function rangesFromRows(rows: LabelRow[]): PageLabelRange[] {
  return rows
    .map((r) => {
      const range: PageLabelRange = { start: Number(r.start) - 1, style: r.style };
      if (r.prefix) range.prefix = r.prefix;
      const first = Number(r.first);
      if (r.first.trim() && first > 1) range.first = first;
      return range;
    })
    .sort((a, b) => a.start - b.start);
}

/** Two range lists describe the same labels as written. */
export function sameRanges(a: PageLabelRange[], b: PageLabelRange[]): boolean {
  if (a.length !== b.length) return false;
  return a.every((r, i) => {
    const o = b[i];
    return r.start === o.start && r.style === o.style && (r.prefix ?? "") === (o.prefix ?? "") && (r.first ?? 1) === (o.first ?? 1);
  });
}

