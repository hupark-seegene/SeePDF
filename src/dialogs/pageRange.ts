/**
 * The 1-based page-range string people type — `"1-3, 5, 8-"` — and its 0-based `PageIndex[]`.
 *
 * Used by 내보내기, 인쇄, 분할, 파일에서 삽입 and 페이지 추출. The engine's own range strings
 * (`PageOp.insertFrom.range`, `split_document.ranges`) stay **1-based** on the wire
 * (IPC_CONTRACT §7.3), so `formatPageRange` is what goes back over the seam.
 *
 * `pageRange.parse` is the vitest name that pins this file.
 */
import type { PageIndex } from "../ipc/types";

/** `null` = the text is not a valid range for a document of `pageCount` pages. */
export function parsePageRange(spec: string, pageCount: number): PageIndex[] | null {
  const text = spec.trim();
  if (!text) return null;
  if (pageCount <= 0) return null;
  const out = new Set<PageIndex>();
  for (const rawPart of text.split(/[,;]/)) {
    const part = rawPart.trim();
    if (!part) continue;
    const m = /^(\d+)?\s*(-|–|—|–)?\s*(\d+)?$/.exec(part);
    if (!m) return null;
    const [, rawFrom, dash, rawTo] = m;
    if (!dash) {
      if (!rawFrom || rawTo) return null;
      const n = Number(rawFrom);
      if (!inRange(n, pageCount)) return null;
      out.add(n - 1);
      continue;
    }
    // open-ended on either side: "-4" = 1..4, "8-" = 8..end
    const from = rawFrom ? Number(rawFrom) : 1;
    const to = rawTo ? Number(rawTo) : pageCount;
    if (!inRange(from, pageCount) || !inRange(to, pageCount)) return null;
    const [lo, hi] = from <= to ? [from, to] : [to, from];
    for (let n = lo; n <= hi; n++) out.add(n - 1);
  }
  if (out.size === 0) return null;
  return [...out].sort((a, b) => a - b);
}

function inRange(n: number, pageCount: number): boolean {
  return Number.isInteger(n) && n >= 1 && n <= pageCount;
}

/** 0-based indices → the compact 1-based string (`[0,1,2,4]` → `"1-3, 5"`). */
export function formatPageRange(pages: readonly PageIndex[]): string {
  const sorted = [...new Set(pages)].sort((a, b) => a - b);
  const parts: string[] = [];
  let start: number | null = null;
  let prev: number | null = null;
  const flush = () => {
    if (start === null || prev === null) return;
    parts.push(start === prev ? `${start + 1}` : `${start + 1}-${prev + 1}`);
  };
  for (const p of sorted) {
    if (start === null) {
      start = p;
      prev = p;
      continue;
    }
    if (prev !== null && p === prev + 1) {
      prev = p;
      continue;
    }
    flush();
    start = p;
    prev = p;
  }
  flush();
  return parts.join(", ");
}

export type RangeMode = "all" | "current" | "selected" | "custom";

export interface RangeChoice {
  mode: RangeMode;
  text: string;
}

/**
 * Resolve a range picker to page indices. `null` means "the custom text is not valid yet", which
 * every dialog renders as `pages.range.invalid` and uses to disable its primary button.
 */
export function resolveRange(
  choice: RangeChoice,
  ctx: { pageCount: number; currentPage: PageIndex; selected: readonly PageIndex[] },
): PageIndex[] | null {
  switch (choice.mode) {
    case "all":
      return Array.from({ length: ctx.pageCount }, (_, i) => i);
    case "current":
      return ctx.pageCount > 0 ? [Math.max(0, Math.min(ctx.pageCount - 1, ctx.currentPage))] : null;
    case "selected":
      return ctx.selected.length ? [...ctx.selected].sort((a, b) => a - b) : null;
    case "custom":
      return parsePageRange(choice.text, ctx.pageCount);
  }
}
