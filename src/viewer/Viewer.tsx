/**
 * The canvas (UI_SPEC §5) — what `App.tsx` mounts where Stage 0 had `CanvasStub`.
 *
 * It is deliberately thin: the document comes from `docStore`, every view parameter from
 * `viewStore` (so the status bar, the keymap and the native menu all drive it without knowing
 * anything about the scroller), and the overlays of (d)/(e) arrive through the `layers` slot of
 * `PageShell`.
 */
import type { DocGeneration, PageIndex } from "../ipc/types";
import { useDocStore } from "../store/docStore";
import { Scroller } from "./Scroller";
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

export function Viewer({ layers, fieldHighlight, renderFormWidgets, onPageRendered }: ViewerProps) {
  const info = useDocStore((s) => s.info);
  // Perf probe: `__seepdfOpenAt` → `__seepdfFirstPaint` (PageShell) is the "open → first page
  // painted" number of ARCHITECTURE §13, readable from the devtools console at any time.
  if (info && window.__seepdfOpenAt === undefined) window.__seepdfOpenAt = performance.now();
  useViewerCommands(!!info);
  if (!info) return null;
  return (
    <Scroller
      info={info}
      layers={layers}
      fieldHighlight={fieldHighlight}
      renderFormWidgets={renderFormWidgets}
      onPageRendered={onPageRendered}
    />
  );
}

export default Viewer;
