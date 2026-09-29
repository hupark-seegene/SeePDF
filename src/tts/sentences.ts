/**
 * 읽어 주기 by sentence (V4, v0.3): the page's text is split into sentences here, each keeping the
 * text-layer characters it came from, so the engine can queue them (`tts_speak { sentences }`), the
 * viewer can highlight the one being read (`TtsHighlight`), and a 속도 change continues from it.
 *
 * Korean-aware and deliberately simple: a sentence ends at . ? ! … (and their full-width forms, and
 * 。) followed by a space or the end — so `3.14`, `v0.3` and `e.g.x` do not split — unless the text
 * before it is a list number (`2.`, `a.`, `iv.`); and at a line break after a **short** line (a
 * heading, a list item, the last line of a paragraph) or before a blank line — but not inside a
 * paragraph whose lines merely wrap.
 */
import type { PageIndex } from "../ipc/types";
import type { PageTextLayer } from "../viewer/text/TextLayer";
import { speechText, type Runs } from "../viewer/text/readingOrder";

/** `[start, end)` in the text that was split. */
export interface Span {
  start: number;
  end: number;
}

/** One queued sentence: what is spoken, and where it is on the page (none for a selection). */
export interface TtsSentence {
  text: string;
  page: PageIndex | null;
  /** text-layer char ranges `[start, end)` on `page` */
  ranges: [number, number][];
}

/** A line shorter than this share of the longest line ends a sentence at its line break. */
export const SHORT_LINE = 0.6;

const TERMINAL = /[.?!…。？！．]/;
/** Punctuation that stays with the sentence it closes: 다." 요!) 」 */
const CLOSING = /[.?!…。？！．"'”’)\]}」』》〉]/;
/** These end a sentence even with no space after them (CJK punctuation). */
const TIGHT = /[。？！]/;
const LIST_MARKER = /^(\d{1,3}|[A-Za-z]|[ivxlcIVXLC]{1,5})$/;

/** Whitespace folded (a PDF's line ends become pauses no voice needs), hyphenated line breaks joined. */
export function speakableText(text: string): string {
  return text
    .replace(/(\p{L})-\r?\n(\p{Ll})/gu, "$1$2")
    // PDFium reports a line-end soft hyphen as U+0002 (or U+00AD): "com\u0002\r\npile" is "compile"
    .replace(/(\p{L})[\u0002\u00ad]\s*(\p{L})/gu, "$1$2")
    .replace(/[\u0002\u00ad]/g, "")
    .replace(/\s+/g, " ")
    .trim();
}

export function splitSentences(text: string): Span[] {
  const out: Span[] = [];
  const len = text.length;
  const longest = Math.max(1, ...text.split("\n").map((l) => l.trim().length));
  let start = 0;
  const push = (end: number) => {
    let s = start;
    let e = end;
    while (s < e && /\s/.test(text[s])) s++;
    while (e > s && /\s/.test(text[e - 1])) e--;
    if (e > s) out.push({ start: s, end: e });
    start = end;
  };
  for (let i = 0; i < len; i++) {
    const c = text[i];
    if (TERMINAL.test(c)) {
      let j = i + 1;
      while (j < len && CLOSING.test(text[j])) j++;
      const boundary = j >= len || /\s/.test(text[j]) || TIGHT.test(c);
      if (boundary && !LIST_MARKER.test(text.slice(start, i).trim())) push(j);
      i = j - 1;
      continue;
    }
    if (c === "\n") {
      const lineStart = text.lastIndexOf("\n", i - 1) + 1;
      const line = text.slice(lineStart, i).trim().length;
      const blankNext = /^[^\S\n]*\n/.test(text.slice(i + 1));
      if (line > 0 && (line < SHORT_LINE * longest || blankNext)) push(i);
    }
  }
  push(len);
  return out;
}

/** A selection (or any plain text) as sentences, with no place on a page. */
export function textSentences(text: string): TtsSentence[] {
  return splitSentences(text)
    .map((span) => ({ text: speakableText(text.slice(span.start, span.end)), page: null, ranges: [] }))
    .filter((s) => s.text.length > 0);
}

/** A page's text (in reading order) as sentences, each with its text-layer ranges. */
export function pageSentences(layer: PageTextLayer, runs: Runs, page: PageIndex): TtsSentence[] {
  const { text, charAt } = speechText(layer, runs);
  const out: TtsSentence[] = [];
  for (const span of splitSentences(text)) {
    const spoken = speakableText(text.slice(span.start, span.end));
    if (!spoken) continue;
    const ranges: [number, number][] = [];
    for (let i = span.start; i < span.end; i++) {
      const c = charAt[i];
      if (c < 0) continue;
      const last = ranges[ranges.length - 1];
      if (last && c >= last[0] && c < last[1]) continue; // a surrogate pair's second unit
      if (last && last[1] === c) last[1] = c + 1;
      else ranges.push([c, c + 1]);
    }
    out.push({ text: spoken, page, ranges });
  }
  return out;
}
