import { convertFileSrc } from "@tauri-apps/api/core";

/**
 * Origin of the custom `seepdf` scheme for the running platform.
 * `convertFileSrc('', 'seepdf')` is implemented by Tauri's injected runtime and returns
 *   macOS / Linux / iOS : "seepdf://localhost/"
 *   Windows / Android   : "http://seepdf.localhost/"   ("https://…" when useHttpsScheme)
 * so we never sniff the OS ourselves.
 */
export function seepdfOrigin(): string {
  return convertFileSrc("", "seepdf").replace(/\/$/, "");
}

export type Query = Record<string, string | number | boolean | undefined>;

/** Build `seepdf://localhost/<route>?k=v…` (or the Windows form). */
export function seepdfUrl(route: string, query: Query = {}): string {
  const qs = Object.entries(query)
    .filter(([, v]) => v !== undefined)
    .map(([k, v]) => `${encodeURIComponent(k)}=${encodeURIComponent(String(v))}`)
    .join("&");
  const path = route.startsWith("/") ? route : `/${route}`;
  return `${seepdfOrigin()}${path}${qs ? `?${qs}` : ""}`;
}

/** URL for a rendered page (or one tile of it). Tiles are `tile`-pixel squares indexed by tx/ty. */
export function pageUrl(doc: string, page: number, scale: number, tile?: { tx: number; ty: number; tile: number }): string {
  return seepdfUrl("/page", { doc, page, scale, ...tile });
}
