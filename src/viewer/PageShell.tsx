/**
 * One mounted page — and the **frozen layer contract** every other frontend module builds on
 * (WORKPLAN (c) → (d)/(e), STAGE0_FRONTEND_NOTES §3).
 *
 * Layer order, bottom to top (ARCHITECTURE §10):
 *
 *   1. `.ph`            low-resolution whole-page placeholder — always mounted, so no white gap
 *   2. `.page-bitmap`   the whole-page bitmap (s ≤ 2) **or** the 512 px tile mosaic
 *   3. `.page-marks`    selection and search rectangles (the viewer's own)
 *   4. `.page-annots`   SVG annotation overlay      — slot `layers.annotations`, owner (d)
 *   5. `.page-forms`    HTML form inputs            — slot `layers.forms`, owner (d)
 *   6. `.page-surface`  pointer-capture surface     — slot `layers.surface`, owner (d)
 *
 * Only layer 1–2 are inside `.page-bitmaps`, which is the only element night mode filters and the
 * only element a zoom gesture CSS-scales. Everything above it is laid out at the live scale, so
 * handles and text never blur.
 *
 * Slot content is rendered inside a `<g transform="matrix(…)">` (annotations) or an absolutely
 * positioned box covering the page (forms, surface); in both cases the coordinates a slot works
 * in are **PDF user space** — use `ctx.toDevice` / `ctx.rectToBox` to place HTML, and nothing at
 * all for SVG.
 */
import { memo, type ReactNode } from "react";
import type { DocGeneration, DocId, Mat6, PageGeom, PageIndex, Point, Rect, Rotation } from "../ipc/types";
import { applyMat, deviceToPage, pageToDevice, rectToBox, type Box } from "./geometry";
import type { TileRequest } from "./TileManager";
import type { NightMode } from "../store/viewStore";

/** Everything a layer needs to place itself on one page. Frozen for (d) and (e). */
export interface PageLayerContext {
  docId: DocId;
  docGeneration: DocGeneration;
  index: PageIndex;
  page: PageGeom;
  /** view rotation (⌘L/⌘R), *not* the page's intrinsic `/Rotate` */
  rotation: Rotation;
  zoomPercent: number;
  /** CSS px per PDF point — `zoomPercent / 100`, display density excluded */
  scale: number;
  /** CSS size of the page box */
  width: number;
  height: number;
  /** PDF user space → CSS px inside the page box (IPC_CONTRACT §3) */
  matrix: Mat6;
  /** CSS px inside the page box → PDF user space */
  inverse: Mat6;
  toDevice(x: number, y: number): Point;
  toPage(x: number, y: number): Point;
  rectToBox(rect: Rect): Box;
  /** for a slot that mounts its own `<svg>`: user-space viewBox plus the y-flip */
  svg: { viewBox: string; transform: string };
}

export interface PageLayers {
  /** below the selection rectangles (rare: a page-wide tint, a redaction preview) */
  under?: ReactNode;
  annotations?: ReactNode;
  forms?: ReactNode;
  surface?: ReactNode;
}

/** (d)/(e) pass one of these to `<Viewer layers={…}>`; it is called once per mounted page. */
export type PageLayerRenderer = (ctx: PageLayerContext) => PageLayers | null | undefined;

export function makePageLayerContext(a: {
  docId: DocId;
  docGeneration: DocGeneration;
  page: PageGeom;
  rotation: Rotation;
  zoomPercent: number;
  width: number;
  height: number;
}): PageLayerContext {
  const scale = a.zoomPercent / 100;
  const matrix = pageToDevice(a.page, a.rotation, scale);
  const inverse = deviceToPage(matrix);
  const { l, b, r, t } = a.page.crop;
  return {
    docId: a.docId,
    docGeneration: a.docGeneration,
    index: a.page.index,
    page: a.page,
    rotation: a.rotation,
    zoomPercent: a.zoomPercent,
    scale,
    width: a.width,
    height: a.height,
    matrix,
    inverse,
    toDevice: (x, y) => applyMat(matrix, x, y),
    toPage: (x, y) => applyMat(inverse, x, y),
    rectToBox: (rect) => rectToBox(rect, matrix),
    svg: { viewBox: `${l} ${-t} ${r - l} ${t - b}`, transform: "matrix(1 0 0 -1 0 0)" },
  };
}

export interface PageShellProps {
  ctx: PageLayerContext;
  /** position of the page box inside the scroll content, CSS px */
  left: number;
  top: number;
  /** the bitmap layer is laid out at the render scale and CSS-scaled by this while a zoom settles */
  bitmapScale: number;
  bitmapWidth: number;
  bitmapHeight: number;
  placeholderUrl: string;
  /** whole-page bitmap, or `null` when the page is tiled */
  bitmapUrl: string | null;
  tiles: TileRequest[];
  night: NightMode;
  current: boolean;
  label: string;
  marks?: ReactNode;
  layers?: PageLayers | null;
  onTileLoad(key: string): void;
  onTileError(key: string): void;
}

declare global {
  interface Window {
    __seepdfFirstPaint?: number;
    __seepdfOpenAt?: number;
  }
}

function markLoaded(e: { currentTarget: HTMLImageElement }): void {
  // No React state: a tile fading in must not re-render the page (ARCHITECTURE §10).
  e.currentTarget.dataset.loaded = "1";
  if (window.__seepdfFirstPaint === undefined) window.__seepdfFirstPaint = performance.now();
}

export const PageShell = memo(function PageShell(props: PageShellProps) {
  const { ctx, tiles, layers } = props;
  return (
    <div
      className="page-shell"
      data-page={ctx.index}
      data-current={props.current || undefined}
      aria-label={props.label}
      style={{ left: props.left, top: props.top, width: ctx.width, height: ctx.height }}
    >
      <div
        className="page-bitmaps"
        data-night={props.night !== "off" ? props.night : undefined}
        style={{
          width: props.bitmapWidth,
          height: props.bitmapHeight,
          transform: props.bitmapScale === 1 ? undefined : `scale(${props.bitmapScale})`,
        }}
      >
        <img className="ph" src={props.placeholderUrl} alt="" draggable={false} onLoad={markLoaded} />
        {props.bitmapUrl && (
          <img
            className="page-bitmap"
            src={props.bitmapUrl}
            alt=""
            draggable={false}
            onLoad={markLoaded}
            onError={markLoaded}
          />
        )}
        {tiles.map((tile) => (
          <img
            key={tile.key}
            className="page-tile"
            src={tile.url}
            alt=""
            draggable={false}
            style={{ left: tile.box.x, top: tile.box.y, width: tile.box.w, height: tile.box.h }}
            onLoad={(e) => {
              markLoaded(e);
              props.onTileLoad(tile.key);
            }}
            onError={() => props.onTileError(tile.key)}
          />
        ))}
      </div>

      {layers?.under}
      {props.marks && <div className="page-marks">{props.marks}</div>}

      {layers?.annotations && (
        <svg
          className="page-annots"
          viewBox={`0 0 ${ctx.width} ${ctx.height}`}
          width={ctx.width}
          height={ctx.height}
          preserveAspectRatio="none"
        >
          <g transform={`matrix(${ctx.matrix.join(" ")})`}>{layers.annotations}</g>
        </svg>
      )}
      {layers?.forms && <div className="page-forms">{layers.forms}</div>}
      {layers?.surface && <div className="page-surface">{layers.surface}</div>}
    </div>
  );
});
