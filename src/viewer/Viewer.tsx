/**
 * The canvas (UI_SPEC §5) — what `App.tsx` mounts where Stage 0 had `CanvasStub`.
 *
 * It is deliberately thin: the document comes from `docStore`, every view parameter from
 * `viewStore` (so the status bar, the keymap and the native menu all drive it without knowing
 * anything about the scroller), and the overlays of (d)/(e) arrive through the `layers` slot of
 * `PageShell`.
 *
 * **분할 보기 (P2).** With `viewStore.split` on, a second scroller of the same document opens beside
 * (좌우) or below (위아래) the first, with its own zoom, rotation, page and scroll (`PaneView`),
 * both fed by one `TileManager` (one in-flight budget, a tile both panes want is requested once)
 * and the engine's one tile cache. The annotation / form / edit layers — and so every tool
 * controller — are mounted in the **focused** pane only; the other one is a read-only view until
 * a click focuses it. The main pane is always the first child of the same element, so opening or
 * closing the split never remounts it (its scroll position and tile inventory survive).
 *
 * v0.3 (V3): the panes are divided by a draggable, keyboard-operable splitter (`role="separator"`,
 * 20–80 %, remembered per orientation in `viewStore.splitRatio`).
 */
import { useCallback, useEffect, useRef, type CSSProperties, type RefObject } from "react";
import type { DocGeneration, PageIndex } from "../ipc/types";
import { useDocStore } from "../store/docStore";
import { SPLIT_MAX, SPLIT_MIN, useViewStore, type PaneId, type SplitOrientation } from "../store/viewStore";
import { useT } from "../i18n/useT";
import { Scroller } from "./Scroller";
import { TileManager } from "./TileManager";
import type { PageLayerRenderer } from "./PageShell";
import { useViewerCommands } from "./viewerCommands";
import "./viewer.css";

export interface ViewerProps {
  /** (d)/(e): one call per mounted page, returns the annotation / form / pointer layers. */
  layers?: PageLayerRenderer;
  /** 필드 강조 표시 (IPC_CONTRACT §9 `hl=1`) — (d) drives it from `formStore`. */
  fieldHighlight?: boolean;
  /** `false` adds `forms=0`: PDFium skips the widgets while the 양식 overlay owns them (F-20). */
  renderFormWidgets?: boolean;
  /**
   * The engine has painted `page` at `docGeneration`. (d) settles its optimistic annotation
   * ghosts here — ARCHITECTURE §10, replacing `annot/RenderProbe.tsx`.
   */
  onPageRendered?(page: PageIndex, docGeneration: DocGeneration): void;
}

/** The document the viewer last showed (it outlives a remount of the viewer). */
let viewedDoc: string | null = null;

export function Viewer({ layers, fieldHighlight, renderFormWidgets, onPageRendered }: ViewerProps) {
  const info = useDocStore((s) => s.info);
  const split = useViewStore((s) => s.split);
  const focusedPane = useViewStore((s) => s.focusedPane);
  const ratio = useViewStore((s) => (s.split ? s.splitRatio[s.split.orientation] : 0.5));
  const panesRef = useRef<HTMLDivElement>(null);
  const tilesRef = useRef<TileManager | null>(null);
  if (!tilesRef.current) tilesRef.current = new TileManager();
  // Perf probe: `__seepdfOpenAt` → `__seepdfFirstPaint` (PageShell) is the "open → first page
  // painted" number of ARCHITECTURE §13, readable from the devtools console at any time.
  if (info && window.__seepdfOpenAt === undefined) window.__seepdfOpenAt = performance.now();
  useViewerCommands(!!info);

  // Another document: the second pane's page and zoom described the old one. (The same document
  // coming back — 페이지 mode unmounts the viewer — keeps the split.)
  const docId = info?.docId ?? null;
  useEffect(() => {
    if (docId !== null && viewedDoc !== null && docId !== viewedDoc) useViewStore.getState().closeSplit();
    if (docId !== null) viewedDoc = docId;
  }, [docId]);

  if (!info) return null;

  const pane = (id: PaneId, paneStyle?: CSSProperties) => {
    const active = !split || focusedPane === id;
    return (
      <Scroller
        key={id}
        paneId={id}
        focused={active}
        split={!!split}
        paneStyle={paneStyle}
        tiles={tilesRef.current ?? undefined}
        info={info}
        layers={active ? layers : undefined}
        fieldHighlight={active ? fieldHighlight : false}
        renderFormWidgets={active ? renderFormWidgets : true}
        onPageRendered={onPageRendered}
      />
    );
  };

  return (
    <div className="viewer-panes" ref={panesRef} data-split={split?.orientation}>
      {pane("main", split ? { flexGrow: ratio, flexShrink: 1, flexBasis: 0 } : undefined)}
      {split && <SplitDivider orientation={split.orientation} ratio={ratio} container={panesRef} />}
      {split && pane("second", { flexGrow: 1 - ratio, flexShrink: 1, flexBasis: 0 })}
    </div>
  );
}

/** The share of `container` a pointer at (x, y) leaves to the first pane. */
export function ratioAt(orientation: SplitOrientation, box: DOMRect, x: number, y: number): number {
  const size = orientation === "side" ? box.width : box.height;
  if (!(size > 0)) return 0.5;
  return orientation === "side" ? (x - box.left) / size : (y - box.top) / size;
}

/** Arrow keys move the divider by this much; Home / End go to the limits. */
const KEY_STEP = 0.05;

function SplitDivider({
  orientation,
  ratio,
  container,
}: {
  orientation: SplitOrientation;
  ratio: number;
  container: RefObject<HTMLDivElement | null>;
}) {
  const t = useT();
  const dragging = useRef(false);
  const set = useCallback((r: number) => useViewStore.getState().setSplitRatio(r, orientation), [orientation]);

  const onPointerDown = (e: React.PointerEvent<HTMLDivElement>) => {
    if (e.button !== 0) return;
    e.preventDefault();
    e.stopPropagation();
    dragging.current = true;
    e.currentTarget.setPointerCapture?.(e.pointerId);
  };
  const onPointerMove = (e: React.PointerEvent<HTMLDivElement>) => {
    const box = container.current?.getBoundingClientRect();
    if (!dragging.current || !box) return;
    set(ratioAt(orientation, box, e.clientX, e.clientY));
  };
  const onPointerUp = (e: React.PointerEvent<HTMLDivElement>) => {
    dragging.current = false;
    if (e.currentTarget.hasPointerCapture?.(e.pointerId)) e.currentTarget.releasePointerCapture(e.pointerId);
  };
  const onKeyDown = (e: React.KeyboardEvent<HTMLDivElement>) => {
    const back = orientation === "side" ? "ArrowLeft" : "ArrowUp";
    const on = orientation === "side" ? "ArrowRight" : "ArrowDown";
    let next: number | null = null;
    if (e.key === back) next = ratio - KEY_STEP;
    else if (e.key === on) next = ratio + KEY_STEP;
    else if (e.key === "Home") next = SPLIT_MIN;
    else if (e.key === "End") next = SPLIT_MAX;
    if (next === null) return;
    e.preventDefault();
    e.stopPropagation();
    set(next);
  };

  return (
    <div
      className="split-divider"
      role="separator"
      tabIndex={0}
      data-orientation={orientation}
      aria-orientation={orientation === "side" ? "vertical" : "horizontal"}
      aria-label={t("view.split.divider")}
      aria-valuemin={Math.round(SPLIT_MIN * 100)}
      aria-valuemax={Math.round(SPLIT_MAX * 100)}
      aria-valuenow={Math.round(ratio * 100)}
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={onPointerUp}
      onPointerCancel={onPointerUp}
      onKeyDown={onKeyDown}
      onDoubleClick={() => set(0.5)}
    />
  );
}

export default Viewer;
