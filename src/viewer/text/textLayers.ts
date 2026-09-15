/**
 * Per-page text-layer cache: one `get_text_layer` call per (document, generation, page), an LRU
 * so a 500-page document never holds 500 buffers (≈ 163 kB each), and a `useSyncExternalStore`
 * subscription so React re-renders the selection layer when a buffer lands.
 *
 * Loading is lazy and visible-page-driven: the Scroller calls `ensureTextLayers(...)` for the
 * pages it mounted, which is also what makes ⌘A and a drag-selection work without a round trip
 * for every page of the document.
 */
import { useSyncExternalStore } from "react";
import * as api from "../../ipc/api";
import type { DocGeneration, DocId, PageIndex } from "../../ipc/types";
import { PageTextLayer } from "./TextLayer";

const MAX_CACHED_PAGES = 24;

const cache = new Map<string, PageTextLayer>();
const pending = new Set<string>();
const failed = new Set<string>();
const listeners = new Set<() => void>();
let version = 0;

function keyOf(docId: DocId, gen: DocGeneration, page: PageIndex): string {
  return `${docId}:${gen}:${page}`;
}

function emit(): void {
  version += 1;
  for (const listener of listeners) listener();
}

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

function snapshot(): number {
  return version;
}

export function getTextLayer(docId: DocId, gen: DocGeneration, page: PageIndex): PageTextLayer | null {
  const key = keyOf(docId, gen, page);
  const hit = cache.get(key);
  if (hit) {
    // touch for the LRU
    cache.delete(key);
    cache.set(key, hit);
  }
  return hit ?? null;
}

export function ensureTextLayer(docId: DocId, gen: DocGeneration, page: PageIndex): void {
  const key = keyOf(docId, gen, page);
  if (cache.has(key) || pending.has(key) || failed.has(key)) return;
  pending.add(key);
  void api
    .getTextLayer({ docId, page })
    .then((view) => {
      cache.set(key, new PageTextLayer(view));
      while (cache.size > MAX_CACHED_PAGES) {
        const oldest = cache.keys().next().value;
        if (oldest === undefined) break;
        cache.delete(oldest);
      }
      emit();
    })
    .catch(() => {
      // A page with no extractable text (a scan) answers with an empty layer or an error; either
      // way there is nothing to select, and retrying on every scroll frame would be worse.
      failed.add(key);
    })
    .finally(() => {
      pending.delete(key);
    });
}

export function ensureTextLayers(docId: DocId, gen: DocGeneration, pages: PageIndex[]): void {
  for (const page of pages) ensureTextLayer(docId, gen, page);
}

/** Drop everything for a document (or for every document when `docId` is omitted). */
export function invalidateTextLayers(docId?: DocId, pages?: PageIndex[] | "all"): void {
  for (const key of [...cache.keys()]) {
    if (docId && !key.startsWith(`${docId}:`)) continue;
    if (pages && pages !== "all") {
      const page = Number(key.slice(key.lastIndexOf(":") + 1));
      if (!pages.includes(page)) continue;
    }
    cache.delete(key);
  }
  for (const key of [...failed]) {
    if (!docId || key.startsWith(`${docId}:`)) failed.delete(key);
  }
  emit();
}

/** Re-renders when any text layer arrives; cheap, because the version is a single number. */
export function useTextLayerVersion(): number {
  return useSyncExternalStore(subscribe, snapshot, snapshot);
}

export function useTextLayer(
  docId: DocId | null,
  gen: DocGeneration,
  page: PageIndex,
): PageTextLayer | null {
  useTextLayerVersion();
  return docId ? getTextLayer(docId, gen, page) : null;
}

/** Test seam. */
export function resetTextLayers(): void {
  cache.clear();
  pending.clear();
  failed.clear();
  emit();
}
