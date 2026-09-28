/**
 * Which transport is live, and which OS we are painting for.
 *
 * `VITE_SEEPDF_MOCK=1 npm run dev` runs the whole frontend against `src/ipc/mock.ts` with no Rust at
 * all (IPC_CONTRACT §13). Outside Tauri the mock is forced on, because `invoke` does not exist there.
 */

export type OsName = "macos" | "windows" | "linux";

interface TauriInternals {
  convertFileSrc?: (path: string, protocol: string) => string;
  invoke?: unknown;
  metadata?: { currentWindow?: { label?: string } };
}

export function tauriInternals(): TauriInternals | undefined {
  return (globalThis as unknown as { __TAURI_INTERNALS__?: TauriInternals }).__TAURI_INTERNALS__;
}

export function isTauri(): boolean {
  return tauriInternals() !== undefined;
}

/** This webview's window label (`main`, `doc-2`, …); `main` outside Tauri. */
export function windowLabel(): string {
  return tauriInternals()?.metadata?.currentWindow?.label ?? "main";
}

/** True when the frontend must talk to the mock adapter instead of the engine. */
export function useMock(): boolean {
  const flag = import.meta.env?.VITE_SEEPDF_MOCK;
  if (flag === "1" || flag === "true") return true;
  return !isTauri();
}

/**
 * The OS the UI is laid out for: `data-os` on `<html>` drives the 78 px traffic-light gutter
 * (macOS) versus the 8 px gutter under the native caption bar (Windows) — UI_SPEC §1.
 */
export function detectOs(ua: string = typeof navigator === "undefined" ? "" : navigator.userAgent): OsName {
  if (/windows|win32|win64/i.test(ua)) return "windows";
  if (/mac os x|macintosh|iphone|ipad/i.test(ua)) return "macos";
  return "linux";
}
