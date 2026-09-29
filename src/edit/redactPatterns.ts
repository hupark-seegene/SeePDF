/**
 * 검색해서 표시 (v0.3 R3): the pure matchers behind search-and-redact — a literal keyword and the
 * Korean personal-data patterns 주민등록번호 · 전화번호 · 이메일 · 계좌번호. No I/O and no React: the
 * page texts come from `redact.ts`, which turns each match's character range into marks.
 *
 * Every pattern refuses to match inside a longer run of digits (`(?<!\d)` … `(?!\d)`), so a 13-digit
 * account number never yields a phone number and a date never yields anything.
 */

export type PatternKind = "rrn" | "phone" | "email" | "account";

export const PATTERN_KINDS: readonly PatternKind[] = ["rrn", "phone", "email", "account"];

export interface TextMatch {
  /** UTF-16 offsets into the searched string, `end` exclusive */
  start: number;
  end: number;
  kind: PatternKind | "keyword";
  text: string;
}

/**
 * 주민등록번호 / 외국인등록번호 shape: YYMMDD (a real month and day), an optional hyphen (spaces
 * around it tolerated), the 7th digit 1–4, then six digits.
 */
const RRN = /(?<!\d)\d{2}(?:0[1-9]|1[0-2])(?:0[1-9]|[12]\d|3[01])\s?[-‐–]?\s?[1-4]\d{6}(?!\d)/g;

/**
 * Mobile (010, 011, 016–019), Seoul (02), the regional codes 031–064, 070 and 050x, with or
 * without separators (`-`, `.`, space, `)`), optionally as `+82` without the leading 0.
 */
const PHONE =
  /(?<![\d+])(?:\+82[-.\s]?\(?0?|\(?0)(?:1[016789]|2|[3-6][1-5]|70|50[2-8])\)?[-.\s]?\d{3,4}[-.\s]?\d{4}(?!\d)/g;

/** A mailbox: ASCII or 한글 local part, ASCII or 한글 labels, an ASCII (or `.한국`) top level. */
const EMAIL =
  /(?:[A-Za-z0-9._%+-]+|[가-힣]+)@(?:(?:[A-Za-z0-9-]+|[가-힣]+)\.)+(?:[A-Za-z]{2,}|한국)(?![A-Za-z0-9])/g;

/** 계좌번호-like: three or four hyphenated digit groups, 10–16 digits in all… */
const ACCOUNT_GROUPED = /(?<![\d-])\d{2,6}(?:-\d{2,8}){2,3}(?![\d-])/g;
/** …or a bare run of 11–14 digits. */
const ACCOUNT_PLAIN = /(?<!\d)\d{11,14}(?!\d)/g;

function all(re: RegExp, text: string, kind: TextMatch["kind"]): TextMatch[] {
  const out: TextMatch[] = [];
  re.lastIndex = 0;
  for (const m of text.matchAll(re)) {
    const start = m.index ?? 0;
    out.push({ start, end: start + m[0].length, kind, text: m[0] });
  }
  return out;
}

function digitCount(s: string): number {
  return (s.match(/\d/g) ?? []).length;
}

/** Is the whole of `s` a phone number (so it is not also reported as an account)? */
function isPhone(s: string): boolean {
  PHONE.lastIndex = 0;
  const m = PHONE.exec(s);
  return !!m && m.index === 0 && m[0].length === s.length;
}

export function findPatterns(text: string, kinds: readonly PatternKind[]): TextMatch[] {
  const out: TextMatch[] = [];
  for (const kind of kinds) {
    switch (kind) {
      case "rrn":
        out.push(...all(RRN, text, "rrn"));
        break;
      case "phone":
        out.push(...all(PHONE, text, "phone"));
        break;
      case "email":
        out.push(...all(EMAIL, text, "email"));
        break;
      case "account":
        out.push(
          ...all(ACCOUNT_GROUPED, text, "account").filter((m) => {
            const n = digitCount(m.text);
            return n >= 10 && n <= 16 && !isPhone(m.text);
          }),
          ...all(ACCOUNT_PLAIN, text, "account").filter((m) => !isPhone(m.text)),
        );
        break;
    }
  }
  return mergeMatches(out);
}

function escapeRegExp(s: string): string {
  return s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

/**
 * Every occurrence of `keyword` (trimmed). Whitespace in the keyword matches any run of whitespace
 * in the text — PDF text breaks lines with generated `\r\n` — and case is ignored unless asked.
 */
export function findKeyword(text: string, keyword: string, matchCase = false): TextMatch[] {
  const words = keyword.trim().split(/\s+/).filter(Boolean);
  if (words.length === 0) return [];
  const re = new RegExp(words.map(escapeRegExp).join("\\s+"), matchCase ? "gu" : "giu");
  return all(re, text, "keyword");
}

/** Sorted by start; an overlapping match is folded into the one before it (the union stays marked). */
export function mergeMatches(matches: TextMatch[]): TextMatch[] {
  const sorted = [...matches].sort((a, b) => a.start - b.start || b.end - a.end);
  const out: TextMatch[] = [];
  for (const m of sorted) {
    const last = out[out.length - 1];
    if (last && m.start < last.end) {
      if (m.end > last.end) {
        last.text += m.text.slice(last.end - m.start);
        last.end = m.end;
      }
      continue;
    }
    out.push({ ...m });
  }
  return out;
}

/** Keyword + patterns over one page's text, merged. */
export function findAll(
  text: string,
  opts: { keyword?: string; patterns?: readonly PatternKind[]; matchCase?: boolean },
): TextMatch[] {
  return mergeMatches([
    ...(opts.keyword ? findKeyword(text, opts.keyword, opts.matchCase) : []),
    ...(opts.patterns?.length ? findPatterns(text, opts.patterns) : []),
  ]);
}
