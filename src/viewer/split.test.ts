/**
 * 분할 보기 (P2) plumbing: one TileManager for two panes, the page-unit scroll position, and the
 * 동기화 스크롤 / shared-viewport registry.
 */
import { afterEach, describe, expect, it, vi } from "vitest";
import { TileManager, type TileRequest } from "./TileManager";
import { computeLayout, positionAt, scrollTopForPosition } from "./layout";
import { ECHO_MS, paneScrolled, registerPane, resetPanes, resyncPanes, viewportFor } from "./panes";
import type { PageGeom } from "../ipc/types";

function tile(page: number, tx: number, priority: number): TileRequest {
  return {
    key: `d1:1:${page}:200:0:${tx}:0:0`,
    page, tx, ty: 0, priority,
    url: `seepdf://localhost/tile?page=${page}&tx=${tx}`,
    box: { x: tx * 256, y: 0, w: 256, h: 256 },
  };
}

describe("TileManager — two clients (panes)", () => {
  it("requests a tile both panes want once, and hands each pane only its own tiles", () => {
    const tiles = new TileManager({ maxInflight: 100 });
    tiles.setDesired(1, [tile(0, 0, 0), tile(0, 1, 1)], { client: "main" });
    tiles.setDesired(1, [tile(0, 1, 0), tile(9, 0, 0)], { client: "second" });
    expect(tiles.mounted()).toHaveLength(3);
    expect(tiles.mounted("main").map((t) => t.key)).toEqual([tile(0, 0, 0).key, tile(0, 1, 0).key]);
    expect(tiles.mounted("second").map((t) => t.key).sort()).toEqual([tile(0, 1, 0).key, tile(9, 0, 0).key].sort());
    // the snapshot is stable between changes (useSyncExternalStore depends on it)
    expect(tiles.mounted("main")).toBe(tiles.mounted("main"));
  });

  it("the in-flight budget is shared, not doubled", () => {
    const tiles = new TileManager({ maxInflight: 4 });
    tiles.setDesired(1, Array.from({ length: 6 }, (_, i) => tile(0, i, i)), { client: "main" });
    tiles.setDesired(1, Array.from({ length: 6 }, (_, i) => tile(5, i, i)), { client: "second" });
    expect(tiles.stats().inflight).toBe(4);
  });

  it("a pane that closes gives its tiles back; the other pane's stay", () => {
    const tiles = new TileManager({ maxInflight: 100 });
    tiles.setDesired(1, [tile(0, 0, 0)], { client: "main" });
    tiles.setDesired(1, [tile(0, 0, 0), tile(7, 0, 0)], { client: "second" });
    const listener = vi.fn();
    tiles.subscribe(listener);
    tiles.removeClient("second");
    expect(listener).toHaveBeenCalled();
    expect(tiles.mounted().map((t) => t.page)).toEqual([0]);
  });

  it("a tile the other pane already had mounted shows up for this pane too", () => {
    const tiles = new TileManager({ maxInflight: 100 });
    tiles.setDesired(1, [tile(3, 0, 0)], { client: "main" });
    tiles.setDesired(1, [], { client: "second" });
    expect(tiles.mounted("second")).toEqual([]);
    tiles.setDesired(1, [tile(3, 0, 0)], { client: "second" });
    expect(tiles.mounted("second").map((t) => t.page)).toEqual([3]);
  });
});

describe("TileManager — the bounded cache memory with two panes at different zooms", () => {
  /** a tile at scale key `sk` (the pane's zoom × DPR) */
  function at(sk: number, page: number, tx: number, priority = tx): TileRequest {
    return {
      key: `d1:1:${page}:${sk}:0:${tx}:0:0`,
      page, tx, ty: 0, priority,
      url: `seepdf://localhost/tile?page=${page}&sk=${sk}&tx=${tx}`,
      box: { x: tx * 256, y: 0, w: 256, h: 256 },
    };
  }

  /** the webview answers every admitted request */
  function loadAll(tiles: TileManager): void {
    for (let round = 0; round < 50; round++) {
      const pending = tiles.mounted().filter((t) => !tiles.isLoaded(t.key));
      if (!pending.length) return;
      for (const t of pending) tiles.notifyLoaded(t.key);
    }
  }

  it("one pane's zoom forgets only that pane's keys: the other pane's visible and scrolled-past tiles stay cached", () => {
    const tiles = new TileManager({ maxInflight: 1, maxMounted: 1000 });
    const main100 = [at(100, 0, 0), at(100, 0, 1)];
    const second200 = [at(200, 5, 0), at(200, 5, 1)];
    const scrolledPast = at(200, 4, 0);
    tiles.setDesired(1, main100, { client: "main" });
    tiles.setDesired(1, [...second200, scrolledPast], { client: "second" });
    loadAll(tiles);
    // the second pane scrolls on (same generation): page 4's tile leaves the DOM, stays cached
    tiles.setDesired(1, second200, { client: "second" });
    expect(tiles.stats().cachedKeys).toBe(5);

    // the main pane zooms to 150 %: a new render generation for main only
    const main150 = [at(150, 0, 0), at(150, 0, 1)];
    tiles.setDesired(2, main150, { client: "main" });
    // main's 100 % keys are gone; the second pane's three are not
    expect(tiles.stats().cachedKeys).toBe(3);
    // the second pane's visible tiles stay mounted and painted
    for (const t of second200) expect(tiles.isLoaded(t.key)).toBe(true);
    expect(tiles.mounted("second").map((t) => t.key)).toEqual(second200.map((t) => t.key));

    // main's first 150 % tile holds the only in-flight slot; scrolling the second pane back to
    // page 4 still paints from the webview cache instead of waiting behind it (a tile it never
    // loaded does wait)
    expect(tiles.stats().inflight).toBe(1);
    const fresh = at(200, 6, 0);
    tiles.setDesired(1, [...second200, scrolledPast, fresh], { client: "second" });
    const secondKeys = tiles.mounted("second").map((t) => t.key);
    expect(secondKeys).toContain(scrolledPast.key);
    expect(secondKeys).not.toContain(fresh.key);

    // now the second pane zooms: its keys go, main's loaded 150 % tiles stay
    loadAll(tiles);
    tiles.setDesired(2, [at(300, 5, 0)], { client: "second" });
    expect(tiles.stats().cachedKeys).toBe(2);
    for (const t of main150) expect(tiles.isLoaded(t.key)).toBe(true);
  });

  it("a tile both panes share survives one pane's zoom and is forgotten once neither pane's generation can want it", () => {
    const tiles = new TileManager({ maxInflight: 100, maxMounted: 1000 });
    const shared = at(100, 2, 0);
    tiles.setDesired(1, [shared], { client: "main" });
    loadAll(tiles);
    tiles.setDesired(1, [shared], { client: "second" });
    // main zooms away from it: the second pane still shows it
    tiles.setDesired(2, [at(200, 2, 0)], { client: "main" });
    loadAll(tiles);
    expect(tiles.isLoaded(shared.key)).toBe(true);
    // the second pane scrolls past it within its generation: still remembered
    tiles.setDesired(1, [at(100, 9, 0)], { client: "second" });
    loadAll(tiles);
    expect(tiles.stats().cachedKeys).toBe(3);
    // the second pane zooms too: nobody's generation can ask for it again
    tiles.setDesired(2, [at(50, 9, 0)], { client: "second" });
    expect(tiles.stats().cachedKeys).toBe(1);
    // and a closed pane gives up its claims (main's 200 % tile is main's alone)
    loadAll(tiles);
    tiles.removeClient("second");
    expect(tiles.stats().cachedKeys).toBe(1);
  });
});

const page = (index: number): PageGeom => ({
  index, widthPt: 600, heightPt: 800, rotation: 0, crop: { l: 0, b: 0, r: 600, t: 800 }, label: null,
});

describe("layout — position in pages", () => {
  const pages = Array.from({ length: 10 }, (_, i) => page(i));
  const layout = computeLayout({ pages, zoomPercent: 100, rotation: 0, mode: "continuous", viewport: { w: 900, h: 700 } });

  it("round-trips a scroll offset through page units", () => {
    const top = layout.byPage.get(4)!.y + 200;
    const pos = positionAt(layout, top);
    expect(Math.floor(pos)).toBe(4);
    expect(scrollTopForPosition(layout, pos)).toBeCloseTo(top, 0);
    expect(positionAt(layout, 0)).toBe(0);
  });

  it("a spread counts as two pages; 단일 cannot place a page it does not lay out", () => {
    const two = computeLayout({ pages, zoomPercent: 100, rotation: 0, mode: "two", viewport: { w: 1400, h: 700 } });
    const row = two.rows[2];
    expect(positionAt(two, row.y)).toBe(4);
    const single = computeLayout({ pages, zoomPercent: 100, rotation: 0, mode: "single", viewport: { w: 900, h: 700 }, currentPage: 3 });
    expect(scrollTopForPosition(single, 6.2)).toBeNull();
    expect(scrollTopForPosition(single, 3.5)).not.toBeNull();
  });
});

describe("panes — 동기화 스크롤 and the shared viewport hint", () => {
  afterEach(() => {
    resetPanes();
    vi.useRealTimers();
  });

  function fakePane(start: number) {
    const pane = { pos: start, moves: [] as number[] };
    return {
      pane,
      handle: {
        position: () => pane.pos,
        scrollToPosition: (p: number) => {
          pane.moves.push(p);
          pane.pos = p;
        },
      },
    };
  }

  it("with sync on, the other pane moves by the same number of pages and does not echo back", () => {
    vi.useFakeTimers();
    const a = fakePane(1);
    const b = fakePane(5);
    registerPane("main", a.handle);
    registerPane("second", b.handle);
    a.pane.pos = 2.5;
    paneScrolled("main", true);
    expect(b.pane.moves).toEqual([6.5]);
    // the moved pane's own scroll event is swallowed
    paneScrolled("second", true);
    expect(a.pane.moves).toEqual([]);
    // later, a real scroll of the second pane drives the first
    vi.advanceTimersByTime(ECHO_MS + 10);
    b.pane.pos = 7.5;
    paneScrolled("second", true);
    expect(a.pane.moves).toEqual([3.5]);
  });

  it("with sync off each pane scrolls alone; switching it on starts from where they are", () => {
    const a = fakePane(0);
    const b = fakePane(8);
    registerPane("main", a.handle);
    registerPane("second", b.handle);
    a.pane.pos = 3;
    paneScrolled("main", false);
    expect(b.pane.moves).toEqual([]);
    b.pane.pos = 9;
    resyncPanes();
    a.pane.pos = 4;
    paneScrolled("main", true);
    expect(b.pane.moves).toEqual([10]);
  });

  it("one viewport hint covers both panes, centred on the focused one", () => {
    viewportFor("main", { first: 2, last: 3, centre: 2 }, "main");
    const hint = viewportFor("second", { first: 40, last: 41, centre: 40 }, "main");
    expect(hint).toEqual({ first: 2, last: 41, centre: 2 });
    expect(viewportFor("second", { first: 40, last: 41, centre: 40 }, "second").centre).toBe(40);
  });
});
