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
 */
import { useEffect, useRef } from "react";
import type { DocGeneration, PageIndex } from "../ipc/types";
import { useDocStore } from "../store/docStore";
import { useViewStore, type PaneId } from "../store/viewStore";
import { Scroller } from "./Scroller";
import { TileManager } from "./TileManager";
import type { PageLayerRenderer } from "./PageShell";
import { useViewerCommands } from "./viewerCommands";
import "./viewer.css";
import { NeedsOcrBanner } from "../ocr/NeedsOcrBanner"; // v0.3 pkg7-ocr (O6)

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

  const pane = (id: PaneId) => {
    const active = !split || focusedPane === id;
    return (
      <Scroller
        key={id}
        paneId={id}
        focused={active}
        split={!!split}
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
    <>
      {/* v0.3 pkg7-ocr (O6): "이 문서에는 검색 가능한 텍스트가 없습니다 · OCR 실행…" */}
      <NeedsOcrBanner />
      <div className="viewer-panes" data-split={split?.orientation}>
        {pane("main")}
        {split && pane("second")}
      </div>
    </>
  );
}

export default Viewer;
