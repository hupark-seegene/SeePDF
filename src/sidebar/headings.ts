/**
 * 제목에서 목차 만들기 (v0.3 P4) — a heading heuristic over the text layer. Pure; the outline
 * editor feeds it every page's lines and shows the proposals for review before anything is written.
 *
 * A line is a heading candidate when it is short (≤ 80 characters, not a sentence) and either
 *  * is set larger than the body text (≥ 1.15 × the most common size, by character count), or
 *  * starts with section numbering: `제1장` / `제2편` / `제3부` (level 1), `제1절` (level 2),
 *    `Ⅰ.` / `IV.` (level 1), `1.` (level 1), `1.2` (level 2), `1.2.3` (level 3), `가.` (level 3).
 * Numbering decides the level when there is one; otherwise the heading sizes are ranked, largest
 * first, into levels 1–3. Running headers and footers (the same text on many pages) and bare page
 * numbers are dropped.
 */
import type { OutlineNode, PageIndex } from "../ipc/types";
import type { TextLayerView } from "../ipc/binary";

export interface HeadingLine {
  page: PageIndex;
  text: string;
  /** the line's font size, points */
  sizePt: number;
  /** top of the line, PDF user space (y up) */
  top: number;
}

export interface HeadingProposal {
  title: string;
  page: PageIndex;
  /** where the heading is, for the outline's destination */
  y: number;
  level: 1 | 2 | 3;
}

const MAX_CHARS = 80;
const SIZE_RATIO = 1.15;
const MAX_PROPOSALS = 500;

/** The lines of one page's text layer, each with its dominant font size. */
export function linesFromLayer(view: TextLayerView, page: PageIndex): HeadingLine[] {
  const out: HeadingLine[] = [];
  for (let i = 0; i < view.header.lineCount; i++) {
    const line = view.line(i);
    let text = "";
    const sizes: number[] = [];
    for (let c = line.firstChar; c < line.firstChar + line.charCount; c++) {
      const ch = view.char(c);
      text += String.fromCodePoint(ch.code);
      if (ch.code > 32) sizes.push(ch.fontSizePt);
    }
    sizes.sort((a, b) => a - b);
    const size = sizes.length ? sizes[Math.floor(sizes.length / 2)] : 0;
    out.push({ page, text: text.replace(/\s+/g, " ").trim(), sizePt: size, top: line.box.t });
  }
  return out;
}

/** The level section numbering implies, or `null`. */
export function numberingLevel(text: string): 1 | 2 | 3 | null {
  const t = text.trim();
  if (/^제\s*\d+\s*[장편부]/.test(t)) return 1;
  if (/^제\s*\d+\s*절/.test(t)) return 2;
  if (/^[ⅠⅡⅢⅣⅤⅥⅦⅧⅨⅩⅪⅫ]+[.)\s]/.test(t) || /^[IVX]{1,5}\.\s/.test(t)) return 1;
  if (/^\d{1,2}\.\d{1,2}\.\d{1,2}\.?\s/.test(t)) return 3;
  if (/^\d{1,2}\.\d{1,2}\.?\s/.test(t)) return 2;
  if (/^\d{1,2}\.\s+\S/.test(t)) return 1;
  if (/^[가-하]\.\s/.test(t)) return 3;
  return null;
}

/** The body text size: the most common (rounded) size, weighted by characters. */
export function bodySize(lines: HeadingLine[]): number {
  const weight = new Map<number, number>();
  for (const l of lines) {
    if (!l.sizePt) continue;
    const key = Math.round(l.sizePt * 2) / 2;
    weight.set(key, (weight.get(key) ?? 0) + l.text.length);
  }
  let best = 0;
  let bestWeight = -1;
  for (const [size, w] of weight) {
    if (w > bestWeight) {
      best = size;
      bestWeight = w;
    }
  }
  return best;
}

function looksLikeSentence(text: string): boolean {
  // a heading does not end like a sentence ("…했다.", "…입니다.", "... end.") unless it is numbering only
  return /[.!?。다]$/.test(text) && text.length > 20;
}

export function detectHeadings(lines: HeadingLine[]): HeadingProposal[] {
  const body = bodySize(lines);
  const pages = new Set(lines.map((l) => l.page)).size;
  // running headers / footers: the same text on many pages
  const seen = new Map<string, Set<PageIndex>>();
  for (const l of lines) {
    const key = l.text.replace(/\d+/g, "#");
    if (!seen.has(key)) seen.set(key, new Set());
    seen.get(key)!.add(l.page);
  }
  const repeated = (text: string) => {
    const on = seen.get(text.replace(/\d+/g, "#"))?.size ?? 0;
    return pages >= 3 && on >= Math.max(3, pages * 0.3);
  };

  const candidates: { line: HeadingLine; numbered: 1 | 2 | 3 | null }[] = [];
  for (const line of lines) {
    const text = line.text;
    if (!text || text.length > MAX_CHARS) continue;
    if (/^[\d\s\-–—/.|()]+$/.test(text)) continue; // page numbers, rules
    if (repeated(text)) continue;
    const numbered = numberingLevel(text);
    const large = body > 0 && line.sizePt >= body * SIZE_RATIO;
    if (!numbered && !large) continue;
    if (!numbered && looksLikeSentence(text)) continue;
    // numbered body-size lines must still look like headings: short
    if (numbered && !large && text.length > 40) continue;
    candidates.push({ line, numbered });
  }

  // size ranks for the unnumbered ones (largest = level 1)
  const sizes = [...new Set(candidates.filter((c) => !c.numbered).map((c) => Math.round(c.line.sizePt * 2) / 2))].sort(
    (a, b) => b - a,
  );
  const rank = (size: number): 1 | 2 | 3 => {
    const i = sizes.indexOf(Math.round(size * 2) / 2);
    return (Math.min(3, Math.max(1, i + 1)) as 1 | 2 | 3);
  };

  return candidates.slice(0, MAX_PROPOSALS).map(({ line, numbered }) => ({
    title: line.text,
    page: line.page,
    y: line.top,
    level: numbered ?? rank(line.sizePt),
  }));
}

/** Proposals (document order) nested by level into outline nodes pointing at their lines. */
export function toOutlineNodes(proposals: HeadingProposal[]): OutlineNode[] {
  const root: OutlineNode[] = [];
  const stack: { level: number; node: OutlineNode }[] = [];
  for (const p of proposals) {
    const node: OutlineNode = { title: p.title, page: p.page, dest: { y: p.y }, children: [] };
    while (stack.length && stack[stack.length - 1].level >= p.level) stack.pop();
    const parent = stack[stack.length - 1];
    if (parent) {
      parent.node.children.push(node);
      parent.node.open = true;
    } else {
      root.push(node);
    }
    stack.push({ level: p.level, node });
  }
  return root;
}
