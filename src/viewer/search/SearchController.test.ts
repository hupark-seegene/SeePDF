import { beforeEach, describe, expect, it, vi } from "vitest";
import type { SearchHit } from "../../ipc/types";
import { useDocStore } from "../../store/docStore";
import { firstHitIndex, mergeHits, stepHitIndex, useSearchStore } from "./SearchController";

const SAMPLE = "/Users/veri/Documents/SeePDF-샘플.pdf";

function hit(page: number, charStart: number): SearchHit {
  return {
    page,
    charStart,
    charLength: 3,
    rects: [{ l: 0, b: 0, r: 10, t: 10 }],
    context: "…",
    contextMatch: [0, 3],
  };
}

beforeEach(() => {
  useSearchStore.setState({
    docId: null,
    query: "",
    matchCase: false,
    wholeWord: false,
    running: false,
    jobId: null,
    hits: [],
    byPage: new Map(),
    total: 0,
    scanned: 0,
    current: -1,
    navNonce: 0,
    error: null,
  });
});

describe("SearchController", () => {
  it("merge: keeps the list in document order however the pages stream in", () => {
    // `search_start` scans outward from the current page: 7, 8, … 0, 1 …
    let hits: SearchHit[] = [];
    hits = mergeHits(hits, 7, [hit(7, 10), hit(7, 40)]);
    hits = mergeHits(hits, 8, [hit(8, 5)]);
    hits = mergeHits(hits, 0, [hit(0, 99), hit(0, 2)]);
    hits = mergeHits(hits, 3, [hit(3, 1)]);

    expect(hits.map((h) => [h.page, h.charStart])).toEqual([
      [0, 2],
      [0, 99],
      [3, 1],
      [7, 10],
      [7, 40],
      [8, 5],
    ]);
  });

  it("merge: re-delivering a page replaces its hits instead of duplicating them", () => {
    let hits = mergeHits([], 2, [hit(2, 1), hit(2, 9)]);
    hits = mergeHits(hits, 5, [hit(5, 1)]);
    hits = mergeHits(hits, 2, [hit(2, 1)]);
    expect(hits.map((h) => h.page)).toEqual([2, 5]);
    expect(hits).toHaveLength(2);

    // a page that lost its only hit (an edit removed the word) disappears from the list
    expect(mergeHits(hits, 5, []).map((h) => h.page)).toEqual([2]);
    expect(mergeHits([], 0, [])).toEqual([]);
  });

  it("merge: selects the first hit at or after the page the search started on", () => {
    const hits = [hit(0, 1), hit(3, 1), hit(7, 1)];
    expect(firstHitIndex(hits, 0)).toBe(0);
    expect(firstHitIndex(hits, 3)).toBe(1);
    expect(firstHitIndex(hits, 4)).toBe(2);
    expect(firstHitIndex(hits, 99)).toBe(0); // wraps to the top of the document
    expect(firstHitIndex([], 0)).toBe(-1);
  });

  it("merge: ⌘G / ⇧⌘G wrap around the list", () => {
    const hits = [hit(0, 1), hit(1, 1), hit(2, 1)];
    expect(stepHitIndex(hits, -1, 1)).toBe(0);
    expect(stepHitIndex(hits, -1, -1)).toBe(2);
    expect(stepHitIndex(hits, 2, 1)).toBe(0);
    expect(stepHitIndex(hits, 0, -1)).toBe(2);
    expect(stepHitIndex([], 0, 1)).toBe(-1);
  });

  it("streams a real pass through the mock adapter, grouped by page, then navigates", async () => {
    const info = await useDocStore.getState().open(SAMPLE);
    expect(info).not.toBeNull();

    await useSearchStore.getState().run(info!.docId, "PDF", 0);
    await vi.waitFor(() => expect(useSearchStore.getState().running).toBe(false), { timeout: 3000 });

    const state = useSearchStore.getState();
    expect(state.total).toBeGreaterThan(0);
    expect(state.hits).toHaveLength(state.total);
    expect(state.scanned).toBe(info!.pageCount);
    expect(state.current).toBe(0);
    // grouped for the in-page highlight layer
    for (const [page, list] of state.byPage) {
      expect(list.every((h) => h.page === page)).toBe(true);
    }

    const nonce = state.navNonce;
    state.step(1);
    expect(useSearchStore.getState().current).toBe(1 % state.total);
    expect(useSearchStore.getState().navNonce).toBe(nonce + 1);

    useSearchStore.getState().clear();
    expect(useSearchStore.getState().hits).toEqual([]);
    expect(useSearchStore.getState().current).toBe(-1);
  });

  it("a new query replaces the previous pass", async () => {
    const info = await useDocStore.getState().open(SAMPLE);
    void useSearchStore.getState().run(info!.docId, "PDF", 0);
    await useSearchStore.getState().run(info!.docId, "한국어", 0);
    await vi.waitFor(() => expect(useSearchStore.getState().running).toBe(false), { timeout: 3000 });

    const state = useSearchStore.getState();
    expect(state.query).toBe("한국어");
    expect(state.hits.every((h) => h.charLength === 3)).toBe(true);
  });

  it("an empty query clears without calling the engine", async () => {
    const info = await useDocStore.getState().open(SAMPLE);
    await useSearchStore.getState().run(info!.docId, "", 0);
    expect(useSearchStore.getState().running).toBe(false);
    expect(useSearchStore.getState().hits).toEqual([]);
  });
});
