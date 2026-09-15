/**
 * The only part of module (d) that is allowed on the critical path: ~40 lines that mount the
 * viewer and then swap real layers in once `./AnnotationHost` has been fetched.
 *
 * Why not `React.lazy`? Because the viewer must not remount. `lazy` + `Suspense` would render a
 * fallback first and a different element type afterwards, which throws away the scroller's scroll
 * position, its tile inventory and the text-layer cache the moment the chunk lands. Here the tree
 * shape never changes — only the `layers` callback does, and `PageShell` re-renders for free.
 */
import { useCallback, useEffect, useState, useSyncExternalStore } from "react";
import { Viewer, type PageLayerContext, type PageLayers } from "../viewer";
import { useAnnotStore } from "../store/annotStore";
import { useAppStore } from "../store/appStore";
import { useDocStore } from "../store/docStore";
import { useFormStore } from "../forms/formStore";

interface Host {
  renderLayers(ctx: PageLayerContext): PageLayers;
  start(): () => void;
}

let host: Host | null = null;
let loading = false;
const listeners = new Set<() => void>();

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

function snapshot(): Host | null {
  return host;
}

function load(): void {
  if (host || loading) return;
  loading = true;
  void import("./AnnotationHost")
    .then((m) => {
      host = m.host;
      for (const l of listeners) l();
    })
    .catch(() => {
      loading = false;
    });
}

/** The canvas with annotations, forms and tools on top. `App.tsx` mounts it via `CanvasStub`. */
export function AnnotatedCanvas() {
  const ready = useSyncExternalStore(subscribe, snapshot, snapshot);
  const [, force] = useState(0);

  useEffect(() => {
    load();
  }, []);

  useEffect(() => {
    if (!ready) return;
    const stop = ready.start();
    force((n) => n + 1);
    return stop;
  }, [ready]);

  const layers = useCallback(
    (ctx: PageLayerContext): PageLayers | null => (host ? host.renderLayers(ctx) : null),
    [ready],
  );

  // 필드 강조 표시: the engine tints the widgets in the bitmap (`hl=1`, IPC_CONTRACT §9) while
  // (d)'s overlay tint keeps an *empty* field discoverable at any zoom. Only in 양식 mode, so
  // no other mode pays a re-render for it.
  const formMode = useAppStore((s) => s.mode) === "form";
  const highlight = useFormStore((s) => s.highlight);
  const hasForm = useDocStore((s) => s.info?.hasForm ?? false);

  // F-20: while the HTML overlay is mounted it is the *only* renderer of a field. `forms=0`
  // makes the engine skip `FPDF_FFLDraw`, so PDFium's widget bitmap (value, wash, caption)
  // is not underneath the input a pixel or two off. Leaving 양식 mode unmounts the overlay
  // and the pages are re-requested with the widgets back on.
  const formOverlayMounted = formMode && hasForm;

  return (
    <Viewer
      layers={layers}
      fieldHighlight={formMode && highlight && hasForm}
      renderFormWidgets={!formOverlayMounted}
      onPageRendered={pageRendered}
    />
  );
}

/**
 * ARCHITECTURE §10: an optimistic ghost is dropped when the engine's own bitmap for that
 * generation lands. `PageShell` calls this from the bitmap `onload` — which is what
 * `annot/RenderProbe.tsx` used to approximate with a second `<img>` (STAGE1D_NOTES §7.1).
 */
function pageRendered(page: number, generation: number): void {
  useAnnotStore.getState().pageRendered(page, generation);
}

export default AnnotatedCanvas;
