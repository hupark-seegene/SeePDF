/**
 * Reading order (v0.3, V6) and the page's text as it is read — shared by the screen-reader region
 * of every mounted page (H9) and by 읽어 주기 (V4).
 *
 * An untagged page is one run in content order and costs no IPC; a tagged page (`DocInfo.tagged`)
 * asks the engine once per (document, generation, page) for the structure tree's order
 * (`get_reading_order`), cached here.
 */
import { useEffect, useState } from "react";
import * as api from "../../ipc/api";
import type { DocGeneration, DocId, PageIndex } from "../../ipc/types";
import type { PageTextLayer } from "./TextLayer";

export type Runs = [number, number][];

const MAX_CACHED = 64;
const cache = new Map<string, Runs>();
const pending = new Map<string, Promise<Runs>>();

function whole(charCount: number): Runs {
  return charCount > 0 ? [[0, charCount]] : [];
}

/** The page's text-layer runs in reading order. Never rejects: a failure reads in content order. */
export function readingOrderFor(
  docId: DocId,
  gen: DocGeneration,
  page: PageIndex,
  tagged: boolean,
  charCount: number,
): Promise<Runs> {
  if (!tagged) return Promise.resolve(whole(charCount));
  const key = `${docId}:${gen}:${page}`;
  const hit = cache.get(key);
  if (hit) return Promise.resolve(hit);
  const inflight = pending.get(key);
  if (inflight) return inflight;
  const request = api
    .getReadingOrder({ docId, page })
    .then((order) => {
      const runs = order.tagged && order.runs.length ? order.runs : whole(charCount);
      cache.set(key, runs);
      while (cache.size > MAX_CACHED) {
        const oldest = cache.keys().next().value;
        if (oldest === undefined) break;
        cache.delete(oldest);
      }
      return runs;
    })
    .catch(() => whole(charCount))
    .finally(() => pending.delete(key));
  pending.set(key, request);
  return request;
}

/** The runs, re-rendering once a tagged page's order arrives (content order until then). */
export function useReadingOrder(
  docId: DocId,
  gen: DocGeneration,
  page: PageIndex,
  tagged: boolean,
  charCount: number,
): Runs {
  const key = `${docId}:${gen}:${page}:${charCount}`;
  const [state, setState] = useState<{ key: string; runs: Runs } | null>(null);
  useEffect(() => {
    if (!tagged || charCount === 0) return;
    let live = true;
    void readingOrderFor(docId, gen, page, tagged, charCount).then((runs) => {
      if (live) setState({ key, runs });
    });
    return () => {
      live = false;
    };
  }, [docId, gen, page, tagged, charCount, key]);
  return state?.key === key ? state.runs : whole(charCount);
}

/** Test seam. */
export function resetReadingOrders(): void {
  cache.clear();
  pending.clear();
}

export interface SpeechText {
  /** the text, one `\n` per line break (PDFium's generated `\r\n`, a new text line, a new run) */
  text: string;
  /** for every UTF-16 unit of `text`, the text-layer char it came from (`-1` = an inserted break) */
  charAt: Int32Array;
}

/**
 * The page's text in reading order, with the char behind every position — what the screen-reader
 * region shows line by line and what 읽어 주기 splits into sentences (whose rectangles it then finds
 * through `charAt`).
 */
export function speechText(layer: PageTextLayer, runs: Runs): SpeechText {
  let text = "";
  const map: number[] = [];
  const newline = () => {
    if (text.length > 0 && !text.endsWith("\n")) {
      text += "\n";
      map.push(-1);
    }
  };
  for (const [start, end] of runs) {
    newline();
    let lastLine = -1;
    for (let i = Math.max(0, start); i < Math.min(end, layer.charCount); i++) {
      const code = layer.charCode(i);
      if (code === 0) continue;
      if (code === 13 || code === 10) {
        newline();
        continue;
      }
      const line = layer.lineIndexOf(i);
      if (line !== -1 && lastLine !== -1 && line !== lastLine) newline();
      if (line !== -1) lastLine = line;
      const s = String.fromCodePoint(code);
      text += s;
      for (let k = 0; k < s.length; k++) map.push(i);
    }
  }
  if (text.endsWith("\n")) {
    text = text.slice(0, -1);
    map.pop();
  }
  return { text, charAt: Int32Array.from(map) };
}

/** The non-empty lines of the page in reading order (the screen-reader region, H9). */
export function readingLines(layer: PageTextLayer, runs: Runs): string[] {
  return speechText(layer, runs)
    .text.split("\n")
    .map((line) => line.trim())
    .filter((line) => line.length > 0);
}
