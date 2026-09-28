/**
 * Toast queue (UI_SPEC §10 "a completion toast with Finder에서 보기 / 폴더 열기").
 * The store lives in the entry chunk — anything may raise a toast — while `Toasts.tsx` renders it.
 */
import { create } from "zustand";
import type { TParams } from "../i18n";

export type ToastTone = "info" | "success" | "danger";

export interface ToastAction {
  labelKey: string;
  onSelect(): void;
}

export interface Toast {
  id: number;
  tone: ToastTone;
  messageKey: string;
  params?: TParams;
  /** developer detail behind 자세히 (error.details) — never shown directly */
  detail?: string;
  actions?: ToastAction[];
  timeoutMs: number;
}

interface ToastStore {
  toasts: Toast[];
  push(t: Omit<Toast, "id" | "timeoutMs"> & { timeoutMs?: number }): number;
  dismiss(id: number): void;
}

let nextId = 0;

export const useToastStore = create<ToastStore>((set, get) => ({
  toasts: [],
  push(t) {
    const id = ++nextId;
    // `toast()` always passes the key, often as `undefined`: a spread default would be overwritten by
    // it, and `setTimeout(dismiss, undefined)` removes the toast on its first frame
    const toast: Toast = { ...t, timeoutMs: t.timeoutMs ?? (t.tone === "danger" ? 8000 : 4500), id };
    set({ toasts: [...get().toasts, toast] });
    return id;
  },
  dismiss(id) {
    set({ toasts: get().toasts.filter((t) => t.id !== id) });
  },
}));

/** `toast("export.done", { name })` — the one call site everything else uses. */
export function toast(
  messageKey: string,
  params?: TParams,
  opts: { tone?: ToastTone; actions?: ToastAction[]; detail?: string; timeoutMs?: number } = {},
): number {
  return useToastStore.getState().push({
    messageKey,
    params,
    tone: opts.tone ?? "info",
    actions: opts.actions,
    detail: opts.detail,
    timeoutMs: opts.timeoutMs,
  });
}
