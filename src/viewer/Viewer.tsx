/**
 * The canvas (UI_SPEC §5) — what `App.tsx` mounts where Stage 0 had `CanvasStub`.
 *
 * It is deliberately thin: the document comes from `docStore`, every view parameter from
 * `viewStore` (so the status bar, the keymap and the native menu all drive it without knowing
 * anything about the scroller), and the overlays of (d)/(e) arrive through the `layers` slot of
 * `PageShell`.
 */
import { useDocStore } from "../store/docStore";
import { Scroller } from "./Scroller";
import type { PageLayerRenderer } from "./PageShell";
import { useViewerCommands } from "./viewerCommands";
import "./viewer.css";

export interface ViewerProps {
  /** (d)/(e): one call per mounted page, returns the annotation / form / pointer layers. */
  layers?: PageLayerRenderer;
}

export function Viewer({ layers }: ViewerProps) {
  const info = useDocStore((s) => s.info);
  // Perf probe: `__seepdfOpenAt` → `__seepdfFirstPaint` (PageShell) is the "open → first page
  // painted" number of ARCHITECTURE §13, readable from the devtools console at any time.
  if (info && window.__seepdfOpenAt === undefined) window.__seepdfOpenAt = performance.now();
  useViewerCommands(!!info);
  if (!info) return null;
  return <Scroller info={info} layers={layers} />;
}

export default Viewer;
