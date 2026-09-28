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
/**
 * How many loaded tile keys are remembered as "in the webview cache". Least recently loaded keys
 * go first; the webview's own cache is not much bigger, and an unbounded set grew for the life of
 * the window (every edit bumps the generation, so old keys can never match again).
 */
export const MAX_CACHED_KEYS = 2048;

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

  private generation = 0;
  private fling = false;
  private seq = 0;
  private pressure: "normal" | "high" = "normal";

  /** everything the viewport wants, priority-sorted */
  private queue: TileRequest[] = [];
  /** what is in the DOM, in admission order */
  private admitted = new Map<string, Admitted>();
  /** keys the webview has already cached — a re-mount paints from cache, so it is not a request */
  private everLoaded = new Set<string>();
  private listeners = new Set<() => void>();
  private cachedMounted: TileRequest[] | null = null;

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
  setDesired(generation: number, tiles: TileRequest[], opts: { fling?: boolean } = {}): void {
    const generationChanged = generation !== this.generation;
    this.generation = generation;
    this.fling = opts.fling ?? false;

    this.queue = [...tiles].sort((a, b) => a.priority - b.priority || a.page - b.page || a.ty - b.ty || a.tx - b.tx);
    const wanted = new Set(this.queue.map((t) => t.key));
    // A new render generation (document generation, scale key, rotation, night) never asks for an
    // old key again: forget them.
    if (generationChanged) {
      for (const key of [...this.everLoaded]) if (!wanted.has(key)) this.everLoaded.delete(key);
    }

    // Cancellation: a tile nobody wants any more is unmounted at once. On a generation bump the
    // engine has already dropped its render, so even a loaded tile would repaint stale pixels.
    let removed = false;
    for (const [key, entry] of [...this.admitted]) {
      if (!wanted.has(key) || (generationChanged && !entry.loaded)) {
        this.admitted.delete(key);
        removed = true;
      }
    }

    this.pump(removed);
  }

  /** The `<img>`s the viewer should render, in admission (priority) order. */
  mounted(): TileRequest[] {
    if (!this.cachedMounted) {
      this.cachedMounted = [...this.admitted.values()].sort((a, b) => a.seq - b.seq).map((e) => e.req);
    }
    return this.cachedMounted;
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
    this.rememberLoaded(key);
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

  stats(): {
    generation: number;
    inflight: number;
    mounted: number;
    queued: number;
    limit: number;
    cachedKeys: number;
  } {
    let inflight = 0;
    for (const entry of this.admitted.values()) if (!entry.loaded) inflight += 1;
    return {
      generation: this.generation,
      inflight,
      mounted: this.admitted.size,
      queued: this.queue.length,
      limit: this.inflightLimit,
      cachedKeys: this.everLoaded.size,
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

  /** LRU by insertion order: a re-load moves the key to the young end. */
  private rememberLoaded(key: string): void {
    this.everLoaded.delete(key);
    this.everLoaded.add(key);
    while (this.everLoaded.size > MAX_CACHED_KEYS) {
      const oldest = this.everLoaded.values().next().value;
      if (oldest === undefined) break;
      this.everLoaded.delete(oldest);
    }
  }

  /** Admit as many queued tiles as the in-flight and mounted budgets allow. */
  private pump(force = false): void {
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

    if (this.evict() || changed) this.emit();
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
    for (const listener of this.listeners) listener();
  }
}
