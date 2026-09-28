/**
 * 읽기 모드 and 전체 화면 (P1-12; UI_SPEC §13 View: 읽기 모드 ⌃⌘R / F8, 전체 화면 ⌃⌘F / F11).
 *
 * * **읽기 모드** hides every piece of chrome — title/tool bar, sidebar, 주석 tool strip, inspector,
 *   status bar — and leaves the pages (`App.tsx` reads `appStore.readingMode`). Esc leaves it.
 * * **전체 화면** is the window's own full screen (`Window.setFullscreen`). On macOS the native
 *   View menu's predefined item and the green button do the same thing, so the state is always
 *   asked of the window rather than remembered here.
 * * The two combine: 전체 화면 + 읽기 모드 is the distraction-free "presenting" state, and Esc
 *   leaves both at once.
 *
 * Kept out of `useCommands` so the dispatcher stays small; `@tauri-apps/api/window` is imported
 * lazily, and outside Tauri (the mock, vitest) the DOM Fullscreen API stands in.
 */
import { useEffect } from "react";
import { useMock } from "../ipc/env";
import { useAppStore } from "../store/appStore";
import { isDialogOpen } from "../dialogs/dialogState";
import { toast } from "./toastStore";

export async function isFullScreen(): Promise<boolean> {
  if (useMock()) return typeof document !== "undefined" && !!document.fullscreenElement;
  const { getCurrentWindow } = await import("@tauri-apps/api/window");
  return getCurrentWindow().isFullscreen();
}

export async function setFullScreen(on: boolean): Promise<void> {
  if (useMock()) {
    if (typeof document === "undefined") return;
    if (on && !document.fullscreenElement) await document.documentElement.requestFullscreen?.();
    if (!on && document.fullscreenElement) await document.exitFullscreen?.();
    return;
  }
  const { getCurrentWindow } = await import("@tauri-apps/api/window");
  await getCurrentWindow().setFullscreen(on);
}

/** ⌃⌘F / F11 / the menu. Never throws: a refused full screen just leaves the window as it is. */
export async function toggleFullScreen(): Promise<void> {
  try {
    await setFullScreen(!(await isFullScreen()));
  } catch {
    // no permission / not supported by this platform or webview
  }
}

/** ⌃⌘R / F8 / the menu. Only with a document open; 페이지 mode's grid is not a reading surface. */
export function toggleReadingMode(on?: boolean): void {
  const app = useAppStore.getState();
  const next = on ?? !app.readingMode;
  if (next === app.readingMode) return;
  if (next && app.mode === "pages") app.setMode("read");
  app.setReadingMode(next);
  if (next) toast("view.readingMode.hint");
}

/** Leave 읽기 모드, and 전체 화면 with it. */
export async function exitPresenting(): Promise<void> {
  toggleReadingMode(false);
  try {
    if (await isFullScreen()) await setFullScreen(false);
  } catch {
    // see toggleFullScreen
  }
}

/**
 * Esc while 읽기 모드 is on. A capture listener, so it wins over the tool keys; an open dialog
 * keeps its own Esc.
 */
export function useReadingModeKeys(): void {
  const on = useAppStore((s) => s.readingMode);
  useEffect(() => {
    if (!on) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape" || isDialogOpen()) return;
      e.preventDefault();
      e.stopImmediatePropagation();
      void exitPresenting();
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [on]);
}
