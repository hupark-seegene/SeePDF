/**
 * Pure geometry for the tool state machines. Everything is in **PDF user space** (points, y-up,
 * unrotated — IPC_CONTRACT §3), so nothing in here knows about zoom, rotation or the DOM.
 */
import type { Point, Rect } from "../ipc/types";

export function normalizeRect(r: Rect): Rect {
  return {
    l: Math.min(r.l, r.r),
    r: Math.max(r.l, r.r),
    b: Math.min(r.b, r.t),
    t: Math.max(r.b, r.t),
  };
}

/** The drag rectangle of a two-point gesture. ⇧ = square, ⌥ = grow from the anchor's centre. */
export function rectFrom(a: Point, b: Point, opts: { square?: boolean; fromCentre?: boolean } = {}): Rect {
  let dx = b[0] - a[0];
  let dy = b[1] - a[1];
  if (opts.square) {
    const side = Math.max(Math.abs(dx), Math.abs(dy));
    dx = Math.sign(dx || 1) * side;
    dy = Math.sign(dy || 1) * side;
  }
  return opts.fromCentre
    ? normalizeRect({ l: a[0] - dx, b: a[1] - dy, r: a[0] + dx, t: a[1] + dy })
    : normalizeRect({ l: a[0], b: a[1], r: a[0] + dx, t: a[1] + dy });
}

export function rectIsEmpty(r: Rect, epsilon = 0.5): boolean {
  return r.r - r.l < epsilon && r.t - r.b < epsilon;
}

export function rectArea(r: Rect): number {
  return Math.max(0, r.r - r.l) * Math.max(0, r.t - r.b);
}

export function rectContains(r: Rect, x: number, y: number, pad = 0): boolean {
  return x >= r.l - pad && x <= r.r + pad && y >= r.b - pad && y <= r.t + pad;
}

export function rectsIntersect(a: Rect, b: Rect): boolean {
  return a.l <= b.r && a.r >= b.l && a.b <= b.t && a.t >= b.b;
}

export function rectUnion(a: Rect | null, b: Rect): Rect {
  if (!a) return { ...b };
  return { l: Math.min(a.l, b.l), b: Math.min(a.b, b.b), r: Math.max(a.r, b.r), t: Math.max(a.t, b.t) };
}

export function padRect(r: Rect, pad: number): Rect {
  return { l: r.l - pad, b: r.b - pad, r: r.r + pad, t: r.t + pad };
}

/** ⇧ on a line/arrow snaps the far end to the nearest multiple of `stepDeg` (UI_SPEC §6). */
export function snapAngle(a: Point, b: Point, stepDeg = 15): Point {
  const dx = b[0] - a[0];
  const dy = b[1] - a[1];
  const length = Math.hypot(dx, dy);
  if (length < 1e-6) return b;
  const step = (stepDeg * Math.PI) / 180;
  const angle = Math.round(Math.atan2(dy, dx) / step) * step;
  return [a[0] + Math.cos(angle) * length, a[1] + Math.sin(angle) * length];
}

/** Squared distance from a point to a segment — the eraser's and the line tool's hit test. */
export function distanceToSegment(px: number, py: number, x1: number, y1: number, x2: number, y2: number): number {
  const dx = x2 - x1;
  const dy = y2 - y1;
  const lengthSq = dx * dx + dy * dy;
  if (lengthSq < 1e-9) return Math.hypot(px - x1, py - y1);
  let t = ((px - x1) * dx + (py - y1) * dy) / lengthSq;
  t = Math.max(0, Math.min(1, t));
  return Math.hypot(px - (x1 + t * dx), py - (y1 + t * dy));
}

/** Distance from a point to a flat `[x0,y0,x1,y1,…]` polyline. `Infinity` for an empty path. */
export function distanceToPath(path: number[], x: number, y: number): number {
  if (path.length < 2) return Infinity;
  if (path.length === 2) return Math.hypot(x - path[0], y - path[1]);
  let best = Infinity;
  for (let i = 0; i + 3 < path.length; i += 2) {
    best = Math.min(best, distanceToSegment(x, y, path[i], path[i + 1], path[i + 2], path[i + 3]));
  }
  return best;
}

export function boundsOfPaths(paths: number[][]): Rect {
  let box: Rect | null = null;
  for (const path of paths) {
    for (let i = 0; i + 1 < path.length; i += 2) {
      const p = { l: path[i], r: path[i], b: path[i + 1], t: path[i + 1] };
      box = rectUnion(box, p);
    }
  }
  return box ?? { l: 0, b: 0, r: 0, t: 0 };
}

export function boundsOfRects(rects: Rect[]): Rect {
  let box: Rect | null = null;
  for (const r of rects) box = rectUnion(box, normalizeRect(r));
  return box ?? { l: 0, b: 0, r: 0, t: 0 };
}

/**
 * Freehand smoothing, two cheap passes and no pressure (F-09, UI_SPEC §6):
 *
 * 1. Ramer–Douglas–Peucker with a tolerance in points, which is what keeps `/InkList` small — a
 *    2-second scribble is ~600 raw samples and ~60 after.
 * 2. A 3-tap moving average over the survivors, so the polyline reads as a curve at 400 %.
 *
 * The end points are never moved: a stroke must start and stop exactly where the pen did.
 */
export function smoothPath(path: number[], tolerance = 0.6): number[] {
  const points = simplifyPath(path, tolerance);
  const n = points.length / 2;
  if (n < 3) return points;
  const out = points.slice();
  for (let i = 1; i < n - 1; i++) {
    out[i * 2] = (points[(i - 1) * 2] + 2 * points[i * 2] + points[(i + 1) * 2]) / 4;
    out[i * 2 + 1] = (points[(i - 1) * 2 + 1] + 2 * points[i * 2 + 1] + points[(i + 1) * 2 + 1]) / 4;
  }
  return out;
}

export function simplifyPath(path: number[], tolerance: number): number[] {
  const n = Math.floor(path.length / 2);
  if (n < 3) return path.slice(0, n * 2);
  const keep = new Uint8Array(n);
  keep[0] = 1;
  keep[n - 1] = 1;
  const stack: [number, number][] = [[0, n - 1]];
  while (stack.length) {
    const [first, last] = stack.pop() as [number, number];
    let worst = -1;
    let worstIndex = -1;
    for (let i = first + 1; i < last; i++) {
      const d = distanceToSegment(
        path[i * 2],
        path[i * 2 + 1],
        path[first * 2],
        path[first * 2 + 1],
        path[last * 2],
        path[last * 2 + 1],
      );
      if (d > worst) {
        worst = d;
        worstIndex = i;
      }
    }
    if (worst > tolerance && worstIndex > 0) {
      keep[worstIndex] = 1;
      stack.push([first, worstIndex], [worstIndex, last]);
    }
  }
  const out: number[] = [];
  for (let i = 0; i < n; i++) {
    if (keep[i]) out.push(path[i * 2], path[i * 2 + 1]);
  }
  return out;
}

export function translatePath(path: number[], dx: number, dy: number): number[] {
  const out = path.slice();
  for (let i = 0; i + 1 < out.length; i += 2) {
    out[i] += dx;
    out[i + 1] += dy;
  }
  return out;
}

export function translateRect(r: Rect, dx: number, dy: number): Rect {
  return { l: r.l + dx, b: r.b + dy, r: r.r + dx, t: r.t + dy };
}

/** Map a path from `from` into `to` (the resize transform of an ink/line annotation). */
export function mapPath(path: number[], from: Rect, to: Rect): number[] {
  const sx = from.r - from.l < 1e-6 ? 1 : (to.r - to.l) / (from.r - from.l);
  const sy = from.t - from.b < 1e-6 ? 1 : (to.t - to.b) / (from.t - from.b);
  const out = path.slice();
  for (let i = 0; i + 1 < out.length; i += 2) {
    out[i] = to.l + (out[i] - from.l) * sx;
    out[i + 1] = to.b + (out[i + 1] - from.b) * sy;
  }
  return out;
}

export function mapRect(r: Rect, from: Rect, to: Rect): Rect {
  const sx = from.r - from.l < 1e-6 ? 1 : (to.r - to.l) / (from.r - from.l);
  const sy = from.t - from.b < 1e-6 ? 1 : (to.t - to.b) / (from.t - from.b);
  return {
    l: to.l + (r.l - from.l) * sx,
    r: to.l + (r.r - from.l) * sx,
    b: to.b + (r.b - from.b) * sy,
    t: to.b + (r.t - from.b) * sy,
  };
}

// ---------------------------------------------------------------------------------------------
// v0.3 pkg4-annotations-stamps-objects
// ---------------------------------------------------------------------------------------------

/**
 * A3 부분 지우개: what is left of a flat `[x0,y0,x1,y1,…]` polyline after a circle of radius `r`
 * at (cx, cy) erased what it covers — every piece outside the circle, split exactly where the
 * segments cross it. Pieces shorter than two points are dropped; `[]` = nothing left.
 */
export function splitPathByCircle(path: number[], cx: number, cy: number, r: number): number[][] {
  const n = Math.floor(path.length / 2);
  const inside = (x: number, y: number) => (x - cx) ** 2 + (y - cy) ** 2 < r * r;
  if (n === 0) return [];
  if (n === 1) return inside(path[0], path[1]) ? [] : [path.slice(0, 2)];
  const out: number[][] = [];
  let cur: number[] = [];
  const push = (x: number, y: number) => {
    const k = cur.length;
    if (k >= 2 && Math.abs(cur[k - 2] - x) < 1e-6 && Math.abs(cur[k - 1] - y) < 1e-6) return;
    cur.push(x, y);
  };
  const close = () => {
    if (cur.length >= 4) out.push(cur);
    cur = [];
  };
  for (let i = 0; i + 1 < n; i++) {
    const x0 = path[2 * i];
    const y0 = path[2 * i + 1];
    const dx = path[2 * i + 2] - x0;
    const dy = path[2 * i + 3] - y0;
    const fx = x0 - cx;
    const fy = y0 - cy;
    const a = dx * dx + dy * dy;
    const b = 2 * (fx * dx + fy * dy);
    const c = fx * fx + fy * fy - r * r;
    const cuts: number[] = [];
    const disc = b * b - 4 * a * c;
    if (a > 1e-12 && disc > 0) {
      const s = Math.sqrt(disc);
      for (const t of [(-b - s) / (2 * a), (-b + s) / (2 * a)]) if (t > 0 && t < 1) cuts.push(t);
    }
    const ts = [0, ...cuts, 1];
    for (let k = 0; k + 1 < ts.length; k++) {
      const ta = ts[k];
      const tb = ts[k + 1];
      const mid = (ta + tb) / 2;
      if (inside(x0 + dx * mid, y0 + dy * mid)) {
        close();
        continue;
      }
      push(x0 + dx * ta, y0 + dy * ta);
      push(x0 + dx * tb, y0 + dy * tb);
    }
  }
  close();
  return out;
}

/** A2: a polygon's vertices as a closed ring (hit-testing, drawing). */
export function polygonPath(vertices: number[], closed: boolean): number[] {
  return closed && vertices.length >= 4 ? [...vertices, vertices[0], vertices[1]] : vertices;
}

/** A2 measure: length of a polyline in points. */
export function pathLength(path: number[]): number {
  let sum = 0;
  for (let i = 2; i + 1 < path.length; i += 2) sum += Math.hypot(path[i] - path[i - 2], path[i + 1] - path[i - 1]);
  return sum;
}

/** A2 measure: area of a polygon in square points (shoelace, absolute). */
export function polygonArea(vertices: number[]): number {
  const n = Math.floor(vertices.length / 2);
  if (n < 3) return 0;
  let sum = 0;
  for (let i = 0; i < n; i++) {
    const j = (i + 1) % n;
    sum += vertices[2 * i] * vertices[2 * j + 1] - vertices[2 * j] * vertices[2 * i + 1];
  }
  return Math.abs(sum / 2);
}

/** A2: the label the engine draws on a measuring annotation (`lopdf_annots::measure_text`). */
export function measureLabel(value: number, unit: "mm" | "pt", square: boolean): string {
  const mm = 25.4 / 72;
  const v = unit === "mm" ? value * (square ? mm * mm : mm) : value;
  return `${v.toFixed(1)} ${unit}${square ? "²" : ""}`;
}
