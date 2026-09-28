import { describe, expect, it } from "vitest";
import { MAX_CACHED_KEYS, MAX_INFLIGHT, TileManager, type TileRequest } from "./TileManager";

function tile(page: number, tx: number, ty: number, priority: number, gen = 1): TileRequest {
  return {
    key: `d1:${gen}:${page}:200:0:${tx}:${ty}:0`,
    page,
    tx,
    ty,
    priority,
    url: `seepdf://localhost/tile?page=${page}&tx=${tx}&ty=${ty}`,
    box: { x: tx * 256, y: ty * 256, w: 256, h: 256 },
  };
}

/** A 5×5 grid around a centre tile, handed over in deliberately scrambled order. */
function grid(centre: { tx: number; ty: number }, gen = 1): TileRequest[] {
  const out: TileRequest[] = [];
  for (let ty = centre.ty - 2; ty <= centre.ty + 2; ty++) {
    for (let tx = centre.tx - 2; tx <= centre.tx + 2; tx++) {
      out.push(tile(0, tx, ty, Math.max(Math.abs(tx - centre.tx), Math.abs(ty - centre.ty)), gen));
    }
  }
  return out.reverse();
}

describe("TileManager", () => {
  it("order: admits tiles centre-out, whatever order they arrive in", () => {
    const manager = new TileManager({ maxInflight: 100 });
    manager.setDesired(1, grid({ tx: 4, ty: 4 }));
    const mounted = manager.mounted();

    expect(mounted).toHaveLength(25);
    expect(mounted[0].priority).toBe(0);
    expect(mounted[0].tx).toBe(4);
    expect(mounted[0].ty).toBe(4);
    // never a farther ring before a nearer one
    for (let i = 1; i < mounted.length; i++) {
      expect(mounted[i].priority).toBeGreaterThanOrEqual(mounted[i - 1].priority);
    }
    expect(mounted.slice(1, 9).every((t) => t.priority === 1)).toBe(true);
  });

  it("order: the current page's tiles come before a neighbour page's", () => {
    const manager = new TileManager({ maxInflight: 100 });
    manager.setDesired(1, [tile(1, 0, 0, 64), tile(0, 3, 3, 2), tile(0, 0, 0, 0)]);
    expect(manager.mounted().map((t) => [t.page, t.priority])).toEqual([
      [0, 0],
      [0, 2],
      [1, 64],
    ]);
  });

  it("inflightCap: never more than 24 requests in flight, and a load admits the next one", () => {
    const manager = new TileManager();
    const many = Array.from({ length: 100 }, (_, i) => tile(0, i % 10, Math.floor(i / 10), i));
    manager.setDesired(1, many);

    expect(manager.mounted()).toHaveLength(MAX_INFLIGHT);
    expect(manager.stats().inflight).toBe(MAX_INFLIGHT);

    // loading ten frees ten slots — and the ten that follow are the next by priority
    for (const t of manager.mounted().slice(0, 10)) manager.notifyLoaded(t.key);
    expect(manager.stats().inflight).toBe(MAX_INFLIGHT);
    expect(manager.mounted()).toHaveLength(MAX_INFLIGHT + 10);
    expect(manager.mounted()[MAX_INFLIGHT].priority).toBe(MAX_INFLIGHT);

    // an error frees its slot too (410 Gone / 409 stale / 503 busy)
    manager.notifyError(manager.mounted()[MAX_INFLIGHT].key);
    expect(manager.mounted()).toHaveLength(MAX_INFLIGHT + 11);
  });

  it("inflightCap: a fling drops the budget to 8, and engine pressure halves it again", () => {
    const manager = new TileManager();
    const many = Array.from({ length: 60 }, (_, i) => tile(0, i, 0, i));
    manager.setDesired(1, many, { fling: true });
    expect(manager.mounted()).toHaveLength(8);

    manager.setPressure("high");
    manager.setDesired(2, many, { fling: false });
    expect(manager.stats().limit).toBe(12);
    expect(manager.mounted().length).toBeLessThanOrEqual(12);
  });

  it("cancels tiles the viewport moved off, and drops pending tiles of an old generation", () => {
    const manager = new TileManager({ maxInflight: 4 });
    manager.setDesired(1, [tile(0, 0, 0, 0), tile(0, 1, 0, 1)]);
    const first = manager.mounted()[0];
    manager.notifyLoaded(first.key);

    // scrolled on: neither tile is wanted any more
    manager.setDesired(1, [tile(0, 5, 5, 0)]);
    expect(manager.mounted().map((t) => t.key)).toEqual([tile(0, 5, 5, 0).key]);

    // a new scale key (generation 2) invalidates everything that has not painted yet
    manager.setDesired(2, [tile(0, 5, 5, 0, 2), tile(0, 6, 5, 1, 2)]);
    expect(manager.mounted().every((t) => t.key.startsWith("d1:2:"))).toBe(true);
  });

  it("keeps the mounted set bounded so the webview can reclaim decoded bitmaps", () => {
    const manager = new TileManager({ maxInflight: 1000, maxMounted: 16 });
    const many = Array.from({ length: 64 }, (_, i) => tile(0, i, 0, i));
    manager.setDesired(1, many);
    expect(manager.mounted()).toHaveLength(16);
    // the farthest tiles are the ones that were dropped
    expect(Math.max(...manager.mounted().map((t) => t.priority))).toBe(15);
  });

  it("notifies subscribers when the mounted set changes and reuses the array otherwise", () => {
    const manager = new TileManager();
    let calls = 0;
    const off = manager.subscribe(() => {
      calls += 1;
    });
    manager.setDesired(1, [tile(0, 0, 0, 0)]);
    expect(calls).toBe(1);
    const snapshot = manager.mounted();
    expect(manager.mounted()).toBe(snapshot);
    manager.setDesired(1, [tile(0, 0, 0, 0)]);
    expect(calls).toBe(1);
    off();
    manager.setDesired(1, []);
    expect(calls).toBe(1);
  });

  it("the loaded-key memory is bounded within a generation and pruned when the generation moves on", () => {
    const manager = new TileManager({ maxInflight: 1000, maxMounted: 1000 });
    // a long fling through a big document at 400 %: thousands of distinct tiles in one generation
    for (let batch = 0; batch < 100; batch++) {
      const tiles = Array.from({ length: 50 }, (_, i) => tile(batch, i, 0, i));
      manager.setDesired(1, tiles);
      for (const t of tiles) manager.notifyLoaded(t.key);
    }
    expect(manager.stats().cachedKeys).toBeLessThanOrEqual(MAX_CACHED_KEYS);

    // an edit bumps the render generation: none of the old keys can ever be asked for again
    manager.setDesired(2, grid({ tx: 4, ty: 4 }, 2));
    expect(manager.stats().cachedKeys).toBe(0);
  });

  it("a tile that stays wanted is still treated as cached after a remount", () => {
    const manager = new TileManager({ maxInflight: 1 });
    const a = tile(0, 0, 0, 0);
    const b = tile(0, 1, 0, 1);
    manager.setDesired(1, [a]);
    manager.notifyLoaded(a.key);
    manager.setDesired(1, []);
    // a is back from the webview cache: it does not take b's only in-flight slot
    manager.setDesired(1, [a, b]);
    expect(manager.mounted().map((t) => t.key)).toEqual([a.key, b.key]);
  });
});
