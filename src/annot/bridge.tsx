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

  return <Viewer layers={layers} />;
}

export default AnnotatedCanvas;
