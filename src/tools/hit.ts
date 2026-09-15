/**
 * Hit-testing and the geometry of a move / resize, in PDF user space. Pure — every tolerance
 * arrives as a number of points that the caller derived from the live zoom (`px / ctx.scale`), so
 * a handle is 8 CSS px at 25 % and at 400 % alike.
 */
import type { Annot, AnnotId, AnnotPatch, Rect } from "../ipc/types";
import { distanceToPath, mapPath, mapRect, normalizeRect, rectContains, rectsIntersect, translatePath, translateRect } from "./geometry";

/** The eight resize handles plus the body. `null` = the gesture missed the annotation. */
export type HandleId = "nw" | "n" | "ne" | "e" | "se" | "s" | "sw" | "w" | "body";

export const CORNER_HANDLES: HandleId[] = ["nw", "ne", "se", "sw"];
export const EDGE_HANDLES: HandleId[] = ["n", "e", "s", "w"];
export const ALL_HANDLES: HandleId[] = [...CORNER_HANDLES, ...EDGE_HANDLES];

/** Where a handle sits on a rectangle, in user space. */
export function handlePoint(rect: Rect, handle: HandleId): [number, number] {
  const cx = (rect.l + rect.r) / 2;
  const cy = (rect.b + rect.t) / 2;
  switch (handle) {
    case "nw": return [rect.l, rect.t];
    case "n": return [cx, rect.t];
    case "ne": return [rect.r, rect.t];
    case "e": return [rect.r, cy];
    case "se": return [rect.r, rect.b];
    case "s": return [cx, rect.b];
    case "sw": return [rect.l, rect.b];
    case "w": return [rect.l, cy];
    default: return [cx, cy];
  }
}

/** Markup and note annotations are moved, never resized (their AP follows the quads / the icon). */
export function isResizable(a: Annot): boolean {
  if (a.locked || a.editable !== "full") return false;
  return a.kind === "square" || a.kind === "circle" || a.kind === "textbox" || a.kind === "stamp" ||
    a.kind === "signature" || a.kind === "ink" || a.kind === "line" || a.kind === "arrow";
}

export function isMovable(a: Annot): boolean {
  return !a.locked && (a.editable === "full" || a.editable === "moveOnly");
}

/** Which handle of a selected annotation a point grabs, `null` when none does. */
export function handleAt(a: Annot, x: number, y: number, tolerance: number): HandleId | null {
  if (isResizable(a)) {
    for (const handle of ALL_HANDLES) {
      const [hx, hy] = handlePoint(a.rect, handle);
      if (Math.abs(x - hx) <= tolerance && Math.abs(y - hy) <= tolerance) return handle;
    }
  }
  return hitsAnnot(a, x, y, tolerance) ? "body" : null;
}

/** Does the point land on the annotation's own ink / quads / rectangle? */
export function hitsAnnot(a: Annot, x: number, y: number, tolerance: number): boolean {
  if (a.inkPaths?.length) {
    if (a.inkPaths.some((path) => distanceToPath(path, x, y) <= tolerance + a.borderWidth / 2)) return true;
    // a closed scribble is still grabbable from inside its bounds
    return rectContains(a.rect, x, y, 0) && a.kind !== "ink";
  }
  if (a.quads?.length) return a.quads.some((q) => rectContains(q, x, y, tolerance));
  return rectContains(a.rect, x, y, tolerance);
}

/** Topmost-first: the last annotation of the page paints last, so it wins the click. */
export function annotAt(annots: Annot[], x: number, y: number, tolerance: number): Annot | null {
  for (let i = annots.length - 1; i >= 0; i--) {
    if (annots[i].hidden) continue;
    if (hitsAnnot(annots[i], x, y, tolerance)) return annots[i];
  }
  return null;
}

/** Everything a marquee rectangle touches (UI_SPEC §6: touch, not full containment). */
export function annotsIn(annots: Annot[], marquee: Rect): AnnotId[] {
  const box = normalizeRect(marquee);
  return annots.filter((a) => !a.hidden && rectsIntersect(a.rect, box)).map((a) => a.id);
}

/** The patch that moves an annotation by (dx, dy) — rect, quads, ink and line points together. */
export function movePatch(a: Annot, dx: number, dy: number): AnnotPatch {
  const patch: AnnotPatch = { rect: translateRect(a.rect, dx, dy) };
  if (a.quads?.length) patch.rects = a.quads.map((q) => translateRect(q, dx, dy));
  if (a.inkPaths?.length) patch.paths = a.inkPaths.map((path) => translatePath(path, dx, dy));
  if (a.linePoints) {
    patch.p1 = [a.linePoints[0] + dx, a.linePoints[1] + dy];
    patch.p2 = [a.linePoints[2] + dx, a.linePoints[3] + dy];
  }
  return patch;
}

/** The rectangle a handle drag produces. `keepAspect` is ⇧; the rect never inverts. */
export function resizeRect(rect: Rect, handle: HandleId, x: number, y: number, keepAspect = false, min = 6): Rect {
  const base = normalizeRect(rect);
  let { l, b, r, t } = base;
  if (handle.includes("w")) l = Math.min(x, base.r - min);
  if (handle.includes("e")) r = Math.max(x, base.l + min);
  if (handle.includes("n")) t = Math.max(y, base.b + min);
  if (handle.includes("s")) b = Math.min(y, base.t - min);
  if (keepAspect && handle !== "body") {
    const ratio = (base.r - base.l) / Math.max(1e-6, base.t - base.b);
    const height = (r - l) / ratio;
    if (handle.includes("s")) b = t - height;
    else t = b + height;
  }
  return normalizeRect({ l, b, r, t });
}

/** The patch that resizes an annotation into `to`, carrying its quads and ink along. */
export function resizePatch(a: Annot, to: Rect): AnnotPatch {
  const patch: AnnotPatch = { rect: to };
  if (a.quads?.length) patch.rects = a.quads.map((q) => mapRect(q, a.rect, to));
  if (a.inkPaths?.length) patch.paths = a.inkPaths.map((path) => mapPath(path, a.rect, to));
  if (a.linePoints) {
    const [p1, p2] = [mapPointInto(a.linePoints[0], a.linePoints[1], a.rect, to), mapPointInto(a.linePoints[2], a.linePoints[3], a.rect, to)];
    patch.p1 = p1;
    patch.p2 = p2;
  }
  return patch;
}

function mapPointInto(x: number, y: number, from: Rect, to: Rect): [number, number] {
  const mapped = mapPath([x, y], from, to);
  return [mapped[0], mapped[1]];
}
