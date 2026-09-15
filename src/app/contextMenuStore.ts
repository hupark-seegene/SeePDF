/**
 * Context-menu state (UI_SPEC §12). Tiny and entry-resident so any component — or the command
 * dispatcher — can open one; `ContextMenu.tsx` renders it.
 */
import { create } from "zustand";

export interface MenuItem {
  /** stable id, also the test hook */
  id: string;
  /** i18n key (never a literal string) */
  labelKey: string;
  /** raw text that is NOT translatable — file names and paths only (최근 항목 menu) */
  label?: string;
  shortcut?: string;
  danger?: boolean;
  disabled?: boolean;
  onSelect?(): void;
}

export type MenuEntry = MenuItem | { separator: true; id: string };

export interface MenuRequest {
  x: number;
  y: number;
  items: MenuEntry[];
  /** accessible name of the menu */
  labelKey?: string;
}

interface ContextMenuStore {
  menu: (MenuRequest & { key: number }) | null;
  open(req: MenuRequest): void;
  close(): void;
}

let key = 0;

export const useContextMenuStore = create<ContextMenuStore>((set) => ({
  menu: null,
  open(req) {
    set({ menu: { ...req, key: ++key } });
  },
  close() {
    set({ menu: null });
  },
}));

export function openContextMenu(req: MenuRequest): void {
  useContextMenuStore.getState().open(req);
}

export function closeContextMenu(): void {
  useContextMenuStore.getState().close();
}

export function isSeparator(entry: MenuEntry): entry is { separator: true; id: string } {
  return "separator" in entry;
}
