/**
 * 지우개 (F-09). Two modes (`toolOptions.eraserMode`, v0.3 A3):
 *
 * * **전체** (default) — removes **whole** annotations it touches: an ink stroke by its path,
 *   anything else by its rect. Dragging accumulates the ids and deletes them as they are hit.
 * * **부분** — cuts ink strokes where the eraser circle crosses them (`splitPathByCircle`) and
 *   keeps the rest: nothing is sent while scrubbing (the overlay shows the circle and what it
 *   touched); on pointer-up each touched ink annotation gets **one** `update_annotation {paths}`
 *   with its remaining pieces — or is deleted when nothing is left. Other kinds are left alone in
 *   this mode.
 *
 * The circle the cursor draws is `style.eraserSize` in points (UI_SPEC §7 "지우개 크기").
 */
import type { Annot, AnnotId, PageIndex } from "../ipc/types";
import type { PagePoint, ToolContext, ToolModule, ToolResult } from "./ToolController";
import { distanceToPath, polygonPath, rectContains, splitPathByCircle } from "./geometry";
import { toolOptions } from "./toolOptions";

export interface EraserState {
  page: PageIndex | null;
  hit: AnnotId[];
  /** 부분 mode: the remaining strokes of every ink annotation touched so far */
  cut: Record<AnnotId, number[][]>;
}

const EMPTY: EraserState = { page: null, hit: [], cut: {} };

function erasable(a: Annot): boolean {
  return !a.locked && a.editable !== "readOnly";
}

/** Which annotations a `radius`-wide dab at (x, y) touches. Ink by path, everything else by rect. */
export function eraserHits(annots: Annot[], x: number, y: number, radius: number): AnnotId[] {
  const out: AnnotId[] = [];
  for (const a of annots) {
    if (!erasable(a)) continue;
    const reach = radius + a.borderWidth / 2;
    if (a.inkPaths?.length) {
      if (a.inkPaths.some((path) => distanceToPath(path, x, y) <= reach)) out.push(a.id);
      continue;
    }
    // v0.3: a real /Line and a polygon are their strokes too
    if (a.linePoints && (a.kind === "line" || a.kind === "arrow")) {
      if (distanceToPath(a.linePoints, x, y) <= reach) out.push(a.id);
      continue;
    }
    if (a.vertices?.length) {
      if (distanceToPath(polygonPath(a.vertices, a.kind === "polygon"), x, y) <= reach) out.push(a.id);
      continue;
    }
    if (a.quads?.length) {
      if (a.quads.some((q) => rectContains(q, x, y, radius))) out.push(a.id);
      continue;
    }
    if (rectContains(a.rect, x, y, radius)) out.push(a.id);
  }
  return out;
}

/**
 * A3: one dab of the partial eraser over `annots` — the ink annotations it cut, with what is left
 * of them (`cut` carries earlier dabs of the same scrub).
 */
export function partialDab(
  annots: Annot[],
  cut: Record<AnnotId, number[][]>,
  x: number,
  y: number,
  radius: number,
): Record<AnnotId, number[][]> {
  let next = cut;
  for (const a of annots) {
    if (a.kind !== "ink" || !erasable(a) || !a.inkPaths?.length) continue;
    const paths = cut[a.id] ?? a.inkPaths;
    const r = radius + a.borderWidth / 2;
    if (!paths.some((p) => distanceToPath(p, x, y) < r)) continue;
    const pieces = paths.flatMap((p) => splitPathByCircle(p, x, y, r));
    if (next === cut) next = { ...cut };
    next[a.id] = pieces;
  }
  return next;
}

function partial(ctx: ToolContext): boolean {
  void ctx;
  return toolOptions().eraserMode === "partial";
}

export const eraserTool: ToolModule<EraserState> = {
  id: "eraser",
  cursor: "none",
  hoverPreview: true,
  init: () => ({ ...EMPTY, hit: [], cut: {} }),

  onDown(_state, p, ctx): ToolResult<EraserState> {
    const radius = ctx.style.eraserSize / 2;
    if (partial(ctx)) {
      const cut = partialDab(ctx.annots?.(p.page) ?? [], {}, p.pt[0], p.pt[1], radius);
      return {
        state: { page: p.page, hit: [], cut },
        preview: { page: p.page, kind: "eraser", rect: dab(p, radius), ids: Object.keys(cut) },
      };
    }
    const hit = eraserHits(ctx.annots?.(p.page) ?? [], p.pt[0], p.pt[1], radius);
    return {
      state: { page: p.page, hit, cut: {} },
      preview: { page: p.page, kind: "eraser", rect: dab(p, radius), ids: hit },
      erase: hit.length ? { page: p.page, ids: hit } : undefined,
    };
  },

  onMove(state, p, ctx): ToolResult<EraserState> {
    const radius = ctx.style.eraserSize / 2;
    const preview = { page: p.page, kind: "eraser" as const, rect: dab(p, radius) };
    if (state.page === null || state.page !== p.page) return { state, preview };
    if (partial(ctx)) {
      const cut = partialDab(ctx.annots?.(p.page) ?? [], state.cut, p.pt[0], p.pt[1], radius);
      return { state: { ...state, cut }, preview: { ...preview, ids: Object.keys(cut) } };
    }
    const hit = eraserHits(ctx.annots?.(p.page) ?? [], p.pt[0], p.pt[1], radius).filter(
      (id) => !state.hit.includes(id),
    );
    if (hit.length === 0) return { state, preview: { ...preview, ids: state.hit } };
    const next = { ...state, hit: [...state.hit, ...hit] };
    return { state: next, preview: { ...preview, ids: next.hit }, erase: { page: p.page, ids: hit } };
  },

  onUp(state): ToolResult<EraserState> {
    const edits = Object.entries(state.cut).map(([id, paths]) => ({ id, paths }));
    return {
      state: { ...EMPTY, hit: [], cut: {} },
      preview: null,
      partialErase: state.page !== null && edits.length ? { page: state.page, edits } : undefined,
    };
  },

  onKey(state, key): ToolResult<EraserState> {
    // Esc reverts to 펜, not to 선택 (UI_SPEC §6).
    if (key === "Escape") return { state: { ...EMPTY, hit: [], cut: {} }, preview: null, done: true };
    return { state };
  },
};

function dab(p: PagePoint, radius: number) {
  return { l: p.pt[0] - radius, r: p.pt[0] + radius, b: p.pt[1] - radius, t: p.pt[1] + radius };
}
