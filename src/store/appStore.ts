/**
 * App-level shell state: OS, locale, theme, settings, recents, chrome visibility, mode and tool.
 * Owner: Stage 0, then (e) for settings/recents dialogs. Keep per-app (not per-document) state here —
 * the sidebar tab and width persist in `settings.json` (UI_SPEC §4).
 */
import { create } from "zustand";
import * as api from "../ipc/api";
import { detectOs, type OsName } from "../ipc/env";
import { resolveLocale, setLocale, type Locale } from "../i18n";
import type { AppInfo, RecentEntry, Settings, ThemePref } from "../ipc/types";
import { applyWindowTheme } from "../app/windowTheme";
import { emitThemeChanged } from "../ipc/events";

export type Mode = "read" | "annotate" | "edit" | "pages" | "form";
/** `attachments`: v0.3 pkg3 (S4) — shown only when the document has attachments or in 편집 mode. */
export type SidebarTab = "thumbnails" | "outline" | "annotations" | "search" | "attachments";
/** Tool ids — the same strings as the `tool.*` i18n keys and the keymap `tool.*` ids. */
export type ToolId =
  | "select" | "hand" | "snapshot"
  | "highlight" | "underline" | "strikeout" | "squiggly" | "note" | "pen" | "eraser"
  | "rectangle" | "ellipse" | "line" | "arrow" | "textbox" | "stamp" | "signature"
  | "editText" | "addText" | "addImage" | "redact" | "link"
  | "fillForm" | "highlightFields"
  // v0.3 pkg2-pages-structure-forms: 양식 ▸ 필드 만들기
  | "fieldText" | "fieldCheckbox" | "fieldRadio" | "fieldCombo" | "fieldSignature" | "formFlatten" | "formExport" | "formImport"
  // v0.3 pkg4-annotations-stamps-objects
  | "polygon" | "callout";

export interface AppState {
  os: OsName;
  locale: Locale;
  theme: ThemePref;
  settings: Settings | null;
  recents: RecentEntry[];
  ready: boolean;
  /** v0.3 pkg5 (H1): `app_info`, loaded once at bootstrap (Welcome's version, SeePDF 정보). */
  appInfo: AppInfo | null;

  sidebarOpen: boolean;
  sidebarTab: SidebarTab;
  sidebarWidth: number;
  inspectorOpen: boolean;
  /**
   * 읽기 모드 (P1-12, ⌃⌘R / F8): the toolbar, sidebars, inspector and status bar are hidden and
   * only the pages remain; Esc leaves it (`app/readingMode.ts`). Not the 읽기 *mode tab*.
   */
  readingMode: boolean;

  mode: Mode;
  tool: ToolId;
  /** a momentary (held) tool that reverts on key-up — UI_SPEC §6 "sticky tools" */
  momentaryFrom: ToolId | null;

  bootstrap(): Promise<void>;
  setLocale(locale: Locale): void;
  setTheme(theme: ThemePref): void;
  /** U4: another window changed the theme — follow it without writing settings again. */
  adoptTheme(theme: ThemePref): void;
  patchSettings(patch: Partial<Settings>): Promise<void>;
  refreshRecents(): Promise<void>;

  toggleSidebar(open?: boolean): void;
  setSidebarTab(tab: SidebarTab): void;
  setSidebarWidth(px: number): void;
  toggleInspector(open?: boolean): void;
  setReadingMode(on: boolean): void;

  setMode(mode: Mode): void;
  setTool(tool: ToolId, momentary?: boolean): void;
  releaseMomentary(): void;
}

export const DEFAULT_TOOL: Record<Mode, ToolId> = {
  read: "select",
  annotate: "highlight",
  edit: "select",
  pages: "select",
  form: "fillForm",
};

/** Themes are stamped on `<html>`; `system` leaves only `prefers-color-scheme` (UI_SPEC §14.3). */
export function applyTheme(theme: ThemePref): void {
  if (typeof document === "undefined") return;
  const root = document.documentElement;
  if (theme === "system") root.removeAttribute("data-theme");
  else root.setAttribute("data-theme", theme);
}

export const useAppStore = create<AppState>((set, get) => ({
  os: detectOs(),
  locale: resolveLocale(),
  theme: "system",
  settings: null,
  recents: [],
  ready: false,
  appInfo: null,

  sidebarOpen: true,
  sidebarTab: "thumbnails",
  sidebarWidth: 240,
  inspectorOpen: false,
  readingMode: false,

  mode: "read",
  tool: "select",
  momentaryFrom: null,

  async bootstrap() {
    const [settings, recents, appInfo] = await Promise.all([
      api.getSettings().catch(() => null),
      api.getRecent().catch(() => [] as RecentEntry[]),
      api.appInfo().catch(() => null),
    ]);
    const locale = resolveLocale(settings?.locale);
    setLocale(locale);
    applyTheme(settings?.theme ?? "system");
    // U4: the native caption bar / menus follow the app theme from the first frame
    void applyWindowTheme(settings?.theme ?? "system");
    if (typeof document !== "undefined") document.documentElement.dataset.os = get().os;
    set({ settings, recents, appInfo, locale, theme: settings?.theme ?? "system", ready: true });
  },

  setLocale(locale) {
    setLocale(locale);
    set({ locale });
    void get().patchSettings({ locale });
  },

  setTheme(theme) {
    applyTheme(theme);
    void applyWindowTheme(theme);
    set({ theme });
    void get().patchSettings({ theme });
    // U4: every other window follows (each one applies it to its own window chrome)
    emitThemeChanged({ theme });
  },

  adoptTheme(theme) {
    if (theme === get().theme) return;
    applyTheme(theme);
    void applyWindowTheme(theme);
    set((s) => ({ theme, settings: s.settings ? { ...s.settings, theme } : s.settings }));
  },

  async patchSettings(patch) {
    const settings = await api.setSettings({ patch }).catch(() => null);
    if (settings) set({ settings });
  },

  async refreshRecents() {
    set({ recents: await api.getRecent().catch(() => [] as RecentEntry[]) });
  },

  toggleSidebar(open) {
    set((s) => ({ sidebarOpen: open ?? !s.sidebarOpen }));
  },
  setSidebarTab(sidebarTab) {
    set({ sidebarTab, sidebarOpen: true });
  },
  setSidebarWidth(px) {
    set({ sidebarWidth: Math.max(180, Math.min(420, Math.round(px))) });
  },
  toggleInspector(open) {
    set((s) => ({ inspectorOpen: open ?? !s.inspectorOpen }));
  },
  setReadingMode(readingMode) {
    set({ readingMode });
  },

  setMode(mode) {
    set((s) => ({
      mode,
      tool: DEFAULT_TOOL[mode],
      momentaryFrom: null,
      // the inspector never opens in 읽기 (UI_SPEC §7)
      inspectorOpen: mode === "read" ? false : s.inspectorOpen || mode !== "pages",
    }));
  },

  setTool(tool, momentary = false) {
    set((s) => ({ tool, momentaryFrom: momentary ? s.momentaryFrom ?? s.tool : null }));
  },

  releaseMomentary() {
    const from = get().momentaryFrom;
    if (from) set({ tool: from, momentaryFrom: null });
  },
}));
