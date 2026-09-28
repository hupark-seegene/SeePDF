import { useEffect, useState } from "react";
import { devicePixelRatio } from "./geometry";

/**
 * `window.devicePixelRatio` as React state. Dragging the window between a Retina display and a 1×
 * monitor (or changing the Windows display scale) changes it without any other state changing, so
 * a render-time read would keep laying out and requesting bitmaps at the old density. A
 * `(resolution: Ndppx)` query stops matching exactly when the ratio moves off N; it is re-armed at
 * the new ratio after every change.
 */
export function useDevicePixelRatio(): number {
  const [dpr, setDpr] = useState(devicePixelRatio);
  useEffect(() => {
    if (typeof window === "undefined" || typeof window.matchMedia !== "function") return;
    const query = window.matchMedia(`(resolution: ${dpr}dppx)`);
    const onChange = () => setDpr(devicePixelRatio());
    if (typeof query.addEventListener === "function") {
      query.addEventListener("change", onChange);
      return () => query.removeEventListener("change", onChange);
    }
    query.addListener?.(onChange);
    return () => query.removeListener?.(onChange);
  }, [dpr]);
  return dpr;
}
