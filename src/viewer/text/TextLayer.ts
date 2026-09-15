/**
 * The per-page text layer: hit-testing, word/line snapping, selection rectangles and the string
 * a copy produces — all on top of the §10.1 binary buffer, with **no DOM node per character**
 * (ARCHITECTURE §5, IPC_CONTRACT §10.1).
 *
 * Offsets are char offsets in `[0, charCount]`, like a DOM range: `rangeRects(2, 5)` covers the
 * characters 2, 3 and 4. Everything is in PDF user space (points, y-up, unrotated) — the caller
 * multiplies by `pageToDevice` to get CSS pixels, so nothing here changes on zoom.
 */
import { CHAR_GENERATED, type TextLayerView, type TextLine } from "../../ipc/binary";
import type { PageIndex, Rect } from "../../ipc/types";
import { isEmptyRect, unionRect } from "../geometry";

export interface Hit {
  /** the character the point is over (or nearest to) */
  char: number;
  line: number;
  word: number;
  /** caret offset: `char` when the point is in the left half, `char + 1` in the right half */
  offset: number;
  /** true when the point is actually inside the character box */
  inside: boolean;
}

export class PageTextLayer {
  readonly page: PageIndex;
  readonly view: TextLayerView;
  readonly charCount: number;
  readonly lineCount: number;
  readonly wordCount: number;

  /** char boxes, flattened: l, b, r, t per char — one allocation instead of charCount objects */
  private readonly boxes: Float32Array;
  private readonly codes: Uint32Array;
  private readonly flags: Uint8Array;
  private readonly lines: TextLine[];
  /** line index per char, so a hit on a char can snap to its line in O(1) */
  private readonly lineOfChar: Int32Array;
  private readonly wordOfChar: Int32Array;

  constructor(view: TextLayerView) {
    this.view = view;
    this.page = view.header.pageIndex;
    this.charCount = view.header.charCount;
    this.lineCount = view.header.lineCount;
    this.wordCount = view.header.wordCount;

    this.boxes = new Float32Array(this.charCount * 4);
    this.codes = new Uint32Array(this.charCount);
    this.flags = new Uint8Array(this.charCount);
    for (let i = 0; i < this.charCount; i++) {
      const c = view.char(i);
      this.boxes[i * 4] = c.box.l;
      this.boxes[i * 4 + 1] = c.box.b;
      this.boxes[i * 4 + 2] = c.box.r;
      this.boxes[i * 4 + 3] = c.box.t;
      this.codes[i] = c.code;
      this.flags[i] = c.flags;
    }

    this.lines = [];
    this.lineOfChar = new Int32Array(this.charCount).fill(-1);
    for (let i = 0; i < this.lineCount; i++) {
      const line = view.line(i);
      this.lines.push(line);
      for (let c = line.firstChar; c < line.firstChar + line.charCount && c < this.charCount; c++) {
        this.lineOfChar[c] = i;
      }
    }

    this.wordOfChar = new Int32Array(this.charCount).fill(-1);
    for (let i = 0; i < this.wordCount; i++) {
      const word = view.word(i);
      for (let c = word.firstChar; c < word.firstChar + word.charCount && c < this.charCount; c++) {
        this.wordOfChar[c] = i;
      }
    }
  }

  charBox(i: number): Rect {
    const o = i * 4;
    return { l: this.boxes[o], b: this.boxes[o + 1], r: this.boxes[o + 2], t: this.boxes[o + 3] };
  }

  charCode(i: number): number {
    return this.codes[i];
  }

  /** pdfium's generated whitespace has a zero-size box: never hit it, always copy it. */
  isGenerated(i: number): boolean {
    return (this.flags[i] & CHAR_GENERATED) !== 0;
  }

  line(i: number): TextLine {
    return this.lines[i];
  }

  lineIndexOf(charIndex: number): number {
    return charIndex >= 0 && charIndex < this.charCount ? this.lineOfChar[charIndex] : -1;
  }

  wordIndexOf(charIndex: number): number {
    return charIndex >= 0 && charIndex < this.charCount ? this.wordOfChar[charIndex] : -1;
  }

  /**
   * The character under a point in PDF user space, or the nearest one: the line comes first
   * (vertical distance to the line box), then the character inside that line.
   */
  hitTest(x: number, y: number): Hit | null {
    if (this.charCount === 0) return null;
    const lineIndex = this.nearestLine(y, x);
    if (lineIndex < 0) return this.nearestCharGlobal(x, y);

    const line = this.lines[lineIndex];
    let best = -1;
    let bestDistance = Infinity;
    let inside = false;
    const end = Math.min(this.charCount, line.firstChar + line.charCount);
    for (let c = line.firstChar; c < end; c++) {
      const box = this.charBox(c);
      if (isEmptyRect(box)) continue;
      if (x >= box.l && x <= box.r) {
        best = c;
        bestDistance = 0;
        inside = y >= box.b && y <= box.t;
        break;
      }
      const distance = x < box.l ? box.l - x : x - box.r;
      if (distance < bestDistance) {
        bestDistance = distance;
        best = c;
      }
    }
    if (best < 0) return this.nearestCharGlobal(x, y);

    const box = this.charBox(best);
    const mid = (box.l + box.r) / 2;
    const offset = x > mid ? best + 1 : best;
    return { char: best, line: lineIndex, word: this.wordIndexOf(best), offset, inside };
  }

  /** The line whose box is nearest to `y`; ties break on horizontal distance. */
  private nearestLine(y: number, x: number): number {
    let best = -1;
    let bestDistance = Infinity;
    for (let i = 0; i < this.lineCount; i++) {
      const box = this.lines[i].box;
      const dy = y > box.t ? y - box.t : y < box.b ? box.b - y : 0;
      const dx = x > box.r ? x - box.r : x < box.l ? box.l - x : 0;
      const distance = dy * 1000 + dx;
      if (distance < bestDistance) {
        bestDistance = distance;
        best = i;
      }
    }
    return best;
  }

  private nearestCharGlobal(x: number, y: number): Hit | null {
    let best = -1;
    let bestDistance = Infinity;
    for (let i = 0; i < this.charCount; i++) {
      const box = this.charBox(i);
      if (isEmptyRect(box)) continue;
      const dx = x < box.l ? box.l - x : x > box.r ? x - box.r : 0;
      const dy = y < box.b ? box.b - y : y > box.t ? y - box.t : 0;
      const distance = dx * dx + dy * dy;
      if (distance < bestDistance) {
        bestDistance = distance;
        best = i;
      }
    }
    if (best < 0) return null;
    const box = this.charBox(best);
    return {
      char: best,
      line: this.lineIndexOf(best),
      word: this.wordIndexOf(best),
      offset: x > (box.l + box.r) / 2 ? best + 1 : best,
      inside: false,
    };
  }

  /** `[start, end)` of the word containing a character (double-click). */
  wordRange(charIndex: number): [number, number] {
    const index = this.wordIndexOf(charIndex);
    if (index < 0) return [charIndex, Math.min(this.charCount, charIndex + 1)];
    const word = this.view.word(index);
    return [word.firstChar, Math.min(this.charCount, word.firstChar + word.charCount)];
  }

  /** `[start, end)` of the line containing a character (triple-click). */
  lineRange(charIndex: number): [number, number] {
    const index = this.lineIndexOf(charIndex);
    if (index < 0) return [charIndex, Math.min(this.charCount, charIndex + 1)];
    const line = this.lines[index];
    return [line.firstChar, Math.min(this.charCount, line.firstChar + line.charCount)];
  }

  /** One rectangle per line the range spans (F-06: a 3-line selection produces 3 rectangles). */
  rangeRects(start: number, end: number): Rect[] {
    const from = Math.max(0, Math.min(start, end));
    const to = Math.min(this.charCount, Math.max(start, end));
    if (to <= from) return [];
    const rects: Rect[] = [];
    for (let i = 0; i < this.lineCount; i++) {
      const line = this.lines[i];
      const lineStart = line.firstChar;
      const lineEnd = line.firstChar + line.charCount;
      const a = Math.max(from, lineStart);
      const b = Math.min(to, lineEnd);
      if (b <= a) continue;
      let span: Rect | null = null;
      for (let c = a; c < b; c++) {
        const box = this.charBox(c);
        if (isEmptyRect(box)) continue;
        span = unionRect(span, box);
      }
      if (!span) continue;
      rects.push({ l: span.l, r: span.r, b: Math.min(span.b, line.box.b), t: Math.max(span.t, line.box.t) });
    }
    if (rects.length === 0) {
      // A selection entirely inside generated whitespace still deserves a caret-thin rect.
      const box = this.charBox(Math.min(from, this.charCount - 1));
      if (!isEmptyRect(box)) rects.push(box);
    }
    return rects;
  }

  /**
   * The string a copy produces. Built from the character array (not from `view.text`), because
   * that is what makes "select Trace-based, copy" produce exactly `Trace-based` — pdfium's own
   * `\r\n` generated characters become single newlines.
   */
  rangeText(start: number, end: number): string {
    const from = Math.max(0, Math.min(start, end));
    const to = Math.min(this.charCount, Math.max(start, end));
    let out = "";
    for (let i = from; i < to; i++) {
      const code = this.codes[i];
      if (code === 0) continue;
      if (code === 13) {
        // \r or \r\n -> one newline
        if (i + 1 < to && this.codes[i + 1] === 10) continue;
        out += "\n";
        continue;
      }
      out += String.fromCodePoint(code);
    }
    return out;
  }

  /** Char offset range covering the whole page (⌘A). */
  allRange(): [number, number] {
    return [0, this.charCount];
  }
}
