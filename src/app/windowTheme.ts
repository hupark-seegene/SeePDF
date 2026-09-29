/**
 * U4 (v0.3 pkg5): the app theme reaches the native window chrome — the Windows caption bar, and
 * on macOS the native menus, alerts and scrollbars — through `getCurrentWindow().setTheme()`.
 * `system` passes `null`, which hands the choice back to the operating system.
 *
 * `@tauri-apps/api/window` is loaded on first use, so none of it lands in the entry chunk. Outside
 * Tauri (the mock adapter, vitest) there is no window to theme; tests install their own setter with
 * `setWindowThemeBackend`.
 */
import { useMock } from "../ipc/env";
import type { ThemePref } from "../ipc/types";

export type WindowThemeSetter = (theme: "light" | "dark" | null) => Promise<void>;

let override: WindowThemeSetter | null = null;

/** Tests only; `null` restores the real window. */
export function setWindowThemeBackend(setter: WindowThemeSetter | null): void {
  override = setter;
}

export async function applyWindowTheme(theme: ThemePref): Promise<void> {
  const value = theme === "system" ? null : theme;
  try {
    if (override) return await override(value);
    if (useMock()) return;
    const { getCurrentWindow } = await import("@tauri-apps/api/window");
    await getCurrentWindow().setTheme(value);
  } catch {
    // a window that refuses (an old WebView2, a window being destroyed) keeps its chrome
  }
}
