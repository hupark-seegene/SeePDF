/**
 * Which tiles are allowed to be in the DOM right now.
 *
 * Tiles are `<img src="seepdf://…/tile?…">` elements: the webview fetches, decodes off the JS
 * thread and caches them (`immutable`, IPC_CONTRACT §9). What the frontend still has to own is
 * *admission* — ARCHITECTURE §3.3:
 *
 *   * visible tiles are requested **centre-out** (Chebyshev priority),
 *   * at most 24 requests are in flight at once, 8 during a fling (> 1.5 px/ms),
 *   * a tile whose viewport generation has moved on is dropped before it is ever mounted
 *     (the engine drops the matching render as stale; `set_viewport` bumps the generation),
 *   * at most `maxMounted` tiles stay mounted so the webview can reclaim decoded bitmaps,
 *     least-useful (farthest from the viewport centre) first.
 *
 * It is a plain class, not a store: the tile inventory must never re-render React per frame
 * (ARCHITECTURE §10). The component subscribes once and re-reads `mounted()` when it changes.
 *
 * **Clients (P2 분할 보기).** The two panes of a split share one manager: each hands over its own
 * desired set under a client id (`setDesired(…, { client })`), the queue is their union (a tile
 * both panes want — same page, same zoom — is requested once), the in-flight and mounted budgets
 * are the window's, not doubled, and `mounted(client)` is the admitted tiles that client wants.
 * With one client (the default `"main"`) it behaves exactly as before.
 */
import type { PageIndex } from "../ipc/types";
import type { Box } from "./geometry";

export interface TileRequest {
  /** unique and stable: `doc:gen:page:sk:rot:tx:ty:night:hl` */
  key: string;
  page: PageIndex;
  tx: number;
  ty: number;
  /** Chebyshev distance from the viewport centre — lower is sooner. */
  priority: number;
  url: string;
  /** position inside the page box, CSS px */
  box: Box;
}

export interface TileManagerOptions {
  maxInflight?: number;
  flingInflight?: number;
  maxMounted?: number;
}

export const MAX_INFLIGHT = 24;
export const FLING_INFLIGHT = 8;
export const MAX_MOUNTED_TILES = 120;

interface Admitted {
  req: TileRequest;
  loaded: boolean;
  failed: boolean;
  seq: number;
}

export class TileManager {
  private readonly maxInflight: number;
  private readonly flingInflight: number;
  private maxMounted: number;

  private fling = false;
  private seq = 0;
  private pressure: "normal" | "high" = "normal";

  /** per client (a pane of 분할 보기): its viewport generation and what it wants */
  private clients = new Map<string, { generation: number; tiles: TileRequest[] }>();
  /** everything the viewports want, priority-sorted, one entry per key */
  private queue: TileRequest[] = [];
  /** what is in the DOM, in admission order */
  private admitted = new Map<string, Admitted>();
  /** keys the webview has already cached — a re-mount paints from cache, so it is not a request */
  private everLoaded = new Set<string>();
  private listeners = new Set<() => void>();
  private cachedMounted: TileRequest[] | null = null;
  private cachedByClient = new Map<string, TileRequest[]>();

  constructor(options: TileManagerOptions = {}) {
    this.maxInflight = options.maxInflight ?? MAX_INFLIGHT;
    this.flingInflight = options.flingInflight ?? FLING_INFLIGHT;
    this.maxMounted = options.maxMounted ?? MAX_MOUNTED_TILES;
  }

  subscribe(listener: () => void): () => void {
    this.listeners.add(listener);
    return () => {
      this.listeners.delete(listener);
    };
  }

  /** `engine-pressure: high` halves the budgets (IPC_CONTRACT §8). */
  setPressure(level: "normal" | "high"): void {
    if (this.pressure === level) return;
    this.pressure = level;
    this.maxMounted = level === "high" ? Math.floor(MAX_MOUNTED_TILES / 2) : MAX_MOUNTED_TILES;
    this.evict();
    this.emit();
  }

  get inflightLimit(): number {
    const base = this.fling ? this.flingInflight : this.maxInflight;
    return this.pressure === "high" ? Math.max(4, Math.floor(base / 2)) : base;
  }

  /**
   * The viewport moved: `generation` is the viewport generation (bumped with every `set_viewport`),
   * `tiles` is everything visible now, in any order.
   */
  setDesired(generation: number, tiles: TileRequest[], opts: { fling?: boolean; client?: string } = {}): void {
    const client = opts.client ?? "main";
    const generationChanged = generation !== (this.clients.get(client)?.generation ?? 0);
    this.clients.set(client, { generation, tiles });
    this.fling = opts.fling ?? false;
    const mine = new Set(tiles.map((t) => t.key));
    const wanted = this.rebuildQueue();

    // Cancellation: a tile nobody wants any more is unmounted at once. On a generation bump the
    // engine has already dropped its render, so even a loaded tile would repaint stale pixels.
    let removed = false;
    for (const [key, entry] of [...this.admitted]) {
      if (!wanted.has(key) || (generationChanged && !entry.loaded && mine.has(key))) {
        this.admitted.delete(key);
        removed = true;
      }
    }

    // With two panes, a tile the other pane already had mounted may now be this one's too.
    if (!this.pump(removed) && this.clients.size > 1) this.emit();
  }

  /** A pane went away (분할 보기 closed): its tiles are no longer wanted on its account. */
  removeClient(client: string): void {
    if (!this.clients.delete(client)) return;
    const wanted = this.rebuildQueue();
    for (const key of [...this.admitted.keys()]) if (!wanted.has(key)) this.admitted.delete(key);
    this.pump(true);
  }

  /** The union of every client's tiles, priority-sorted, the best priority winning a shared key. */
  private rebuildQueue(): Set<string> {
    const byKey = new Map<string, TileRequest>();
    for (const { tiles } of this.clients.values()) {
      for (const tile of tiles) {
        const known = byKey.get(tile.key);
        if (!known || tile.priority < known.priority) byKey.set(tile.key, tile);
      }
    }
    this.queue = [...byKey.values()].sort(
      (a, b) => a.priority - b.priority || a.page - b.page || a.ty - b.ty || a.tx - b.tx,
    );
    return new Set(byKey.keys());
  }

  /**
   * The `<img>`s the viewer should render, in admission (priority) order — every admitted tile, or
   * with `client` only the ones that client wants (a pane of 분할 보기).
   */
  mounted(client?: string): TileRequest[] {
    if (!this.cachedMounted) {
      this.cachedMounted = [...this.admitted.values()].sort((a, b) => a.seq - b.seq).map((e) => e.req);
    }
    if (client === undefined) return this.cachedMounted;
    let mine = this.cachedByClient.get(client);
    if (!mine) {
      const keys = new Set((this.clients.get(client)?.tiles ?? []).map((t) => t.key));
      mine = this.cachedMounted.filter((t) => keys.has(t.key));
      this.cachedByClient.set(client, mine);
    }
    return mine;
  }

  mountedFor(page: PageIndex): TileRequest[] {
    return this.mounted().filter((t) => t.page === page);
  }

  isLoaded(key: string): boolean {
    return this.admitted.get(key)?.loaded ?? false;
  }

  notifyLoaded(key: string): void {
    const entry = this.admitted.get(key);
    if (!entry || entry.loaded) return;
    entry.loaded = true;
    this.everLoaded.add(key);
    this.pump();
  }

  /** A 409/410/503 answer: free the slot, do not retry in this generation. */
  notifyError(key: string): void {
    const entry = this.admitted.get(key);
    if (!entry) return;
    entry.failed = true;
    entry.loaded = true;
    this.pump();
  }

  stats(): { generation: number; inflight: number; mounted: number; queued: number; limit: number } {
    let inflight = 0;
    for (const entry of this.admitted.values()) if (!entry.loaded) inflight += 1;
    return {
      generation: this.clients.get("main")?.generation ?? 0,
      inflight,
      mounted: this.admitted.size,
      queued: this.queue.length,
      limit: this.inflightLimit,
    };
  }

  reset(): void {
    this.queue = [];
    this.admitted.clear();
    this.everLoaded.clear();
    this.cachedMounted = null;
    this.emit();
  }

  // -------------------------------------------------------------------------

  /** Admit as many queued tiles as the in-flight and mounted budgets allow. */
  private pump(force = false): boolean {
    let inflight = 0;
    for (const entry of this.admitted.values()) if (!entry.loaded) inflight += 1;

    let changed = force;
    for (const req of this.queue) {
      if (this.admitted.has(req.key)) continue;
      if (this.admitted.size >= this.maxMounted) break;
      const cached = this.everLoaded.has(req.key);
      if (!cached && inflight >= this.inflightLimit) break;
      this.admitted.set(req.key, { req, loaded: false, failed: false, seq: this.seq++ });
      if (!cached) inflight += 1;
      changed = true;
    }

    if (this.evict() || changed) {
      this.emit();
      return true;
    }
    return false;
  }

  /** LRU: when more tiles are mounted than the budget allows, drop the farthest ones. */
  private evict(): boolean {
    if (this.admitted.size <= this.maxMounted) return false;
    const order = [...this.admitted.entries()].sort(
      (a, b) => b[1].req.priority - a[1].req.priority || a[1].seq - b[1].seq,
    );
    let over = this.admitted.size - this.maxMounted;
    for (const [key] of order) {
      if (over <= 0) break;
      this.admitted.delete(key);
      over -= 1;
    }
    return true;
  }

  private emit(): void {
    this.cachedMounted = null;
    this.cachedByClient.clear();
    for (const listener of this.listeners) listener();
  }
}
