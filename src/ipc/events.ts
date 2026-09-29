/**
 * App-wide broadcasts (IPC_CONTRACT §8) plus the webview's drag-drop.
 *
 * Every helper returns a synchronous unsubscribe function, so it drops straight into
 * `useEffect(() => onDocChanged(fn), [])` — `listen()` resolves later and is unlistened on cleanup.
 * In mock mode the same helpers are wired to the mock adapter's in-process bus.
 */
import { emit, listen, type Event as TauriEvent, type UnlistenFn } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { useMock } from "./env";
import { appBus } from "./bus";
import type {
  DocChangedEvent, DocSavedEvent, EnginePressureEvent, OpenFileEvent, RecentsChangedEvent, TtsProgressEvent,
  EngineCrashedEvent, ThemeChangedEvent,
} from "./types";

export type Unsubscribe = () => void;

/**
 * Calls an unlisten function **at most once**, and never lets it reject.
 *
 * React 19 StrictMode mounts every effect twice, and Tauri's own `unlisten` deletes its row in
 * `window.__TAURI_INTERNALS__.listeners` — a second call then throws
 * `undefined is not an object (evaluating 'listeners[eventId].handlerId')` as an *unhandled
 * rejection* (STAGE1C_NOTES §7.3). Nulling the slot before calling makes the second call a
 * no-op, and the try/catch + `.catch` covers the case where the webview tore the listener down
 * for us (window close, reload).
 */
function once(fn: UnlistenFn | null): { call(): void } {
  let pending: UnlistenFn | null = fn;
  return {
    call() {
      const f = pending;
      pending = null;
      if (!f) return;
      try {
        const r = f() as unknown;
        if (r && typeof (r as Promise<void>).catch === "function") (r as Promise<void>).catch(() => undefined);
      } catch {
        /* already gone */
      }
    },
  };
}

function subscribe<T>(name: string, handler: (payload: T) => void, own = false): Unsubscribe {
  if (useMock()) return appBus.on(name, (p) => handler(p as T));
  let stop = once(null);
  let cancelled = false;
  const on = (e: TauriEvent<T>) => handler(e.payload);
  // `own`: only events sent to this webview (`emit_to(label, …)`), not every window's
  void (own ? getCurrentWebview().listen<T>(name, on) : listen<T>(name, on))
    .then((fn) => {
      stop = once(fn);
      if (cancelled) stop.call();
    })
    .catch(() => undefined);
  return () => {
    cancelled = true;
    stop.call();
  };
}

/**
 * A file was queued for **this window** (Finder / Explorer, argv, a drop, 새 창에서 열기): take it
 * with `takePendingOpens`, which hands each request to exactly one window, exactly once. The engine
 * sends it to one window only (v0.3 integration: a broadcast made every window replace its
 * document), and this listens to this webview's events only.
 */
export function onOpenFile(handler: (e: OpenFileEvent) => void): Unsubscribe {
  return subscribe("open-file", handler, true);
}

/** A document mutated: re-list what changed, invalidate tiles of `changedPages`. */
export function onDocChanged(handler: (e: DocChangedEvent) => void): Unsubscribe {
  return subscribe("doc-changed", handler);
}

export function onDocSaved(handler: (e: DocSavedEvent) => void): Unsubscribe {
  return subscribe("doc-saved", handler);
}

export function onRecentsChanged(handler: (e: RecentsChangedEvent) => void): Unsubscribe {
  return subscribe("recents-changed", handler);
}

/** Budgets halved — the viewer lowers MAX_MOUNTED_TILES until `level` returns to `normal`. */
export function onEnginePressure(handler: (e: EnginePressureEvent) => void): Unsubscribe {
  return subscribe("engine-pressure", handler);
}

// --- v0.3 pkg5-app-shell-release-diagnostics ---

/** H4: a panicking engine command closed these documents; their windows reopen them. */
export function onEngineCrashed(handler: (e: EngineCrashedEvent) => void): Unsubscribe {
  return subscribe("engine-crashed", handler);
}

/** U4: another window (or this one) changed the theme. */
export function onThemeChanged(handler: (e: ThemeChangedEvent) => void): Unsubscribe {
  return subscribe("theme-changed", handler);
}

/** U4: tell every window — this one included — that the theme changed. */
export function emitThemeChanged(e: ThemeChangedEvent): void {
  if (useMock()) {
    appBus.emit("theme-changed", e);
    return;
  }
  void emit("theme-changed", e).catch(() => undefined);
}

/** v0.3 (V4): read aloud started a sentence (`sentenceIndex`), or finished its queue (`null`). */
export function onTtsProgress(handler: (e: TtsProgressEvent) => void): Unsubscribe {
  return subscribe("tts-progress", handler);
}

/** Native macOS menu item -> the focused window. `id` matches a keymap id (`src/keys/keymap.ts`). */
export function onMenuCommand(handler: (id: string) => void, ids: readonly string[]): Unsubscribe {
  const offs = ids.map((id) => subscribe(`menu:${id.replace(/\./g, "/")}`, () => handler(id)));
  return () => offs.forEach((off) => off());
}

export interface DropPayload {
  paths: string[];
  /** v0.3: where the files were dropped, in CSS pixels of the window (the organizer inserts there) */
  position?: { x: number; y: number };
}

/**
 * PDFs dropped on the window. Tauri gives real filesystem paths; in the browser (mock) we only get
 * `File` objects, so the names are surfaced instead — enough to exercise the flow.
 */
export function onFileDrop(handler: (e: DropPayload) => void): Unsubscribe {
  if (useMock()) {
    const prevent = (ev: globalThis.DragEvent) => ev.preventDefault();
    const drop = (ev: globalThis.DragEvent) => {
      ev.preventDefault();
      const files = [...(ev.dataTransfer?.files ?? [])].map((f) => f.name);
      if (files.length) handler({ paths: files, position: { x: ev.clientX, y: ev.clientY } });
    };
    window.addEventListener("dragover", prevent);
    window.addEventListener("drop", drop);
    return () => {
      window.removeEventListener("dragover", prevent);
      window.removeEventListener("drop", drop);
    };
  }
  let stop = once(null);
  let cancelled = false;
  void getCurrentWebview()
    .onDragDropEvent((e) => {
      if (e.payload.type === "drop") {
        const scale = typeof window === "undefined" ? 1 : window.devicePixelRatio || 1;
        const p = e.payload.position;
        handler({ paths: e.payload.paths, position: p ? { x: p.x / scale, y: p.y / scale } : undefined });
      }
    })
    .then((fn) => {
      stop = once(fn);
      if (cancelled) stop.call();
    })
    .catch(() => undefined);
  return () => {
    cancelled = true;
    stop.call();
  };
}

// ---------------------------------------------------------------------------
// v0.3 pkg2-pages-structure-forms
// ---------------------------------------------------------------------------

export type FileDragState = "over" | "leave" | "drop";

/**
 * H7: files being dragged over the window. With Tauri's native drag-drop handler on (it must be:
 * it is what gives us real paths), the webview's HTML5 `dragover` never fires on Windows, so the
 * Welcome drop zone's highlight follows the native `enter` / `over` / `leave` / `drop` instead.
 * In mock mode the window's HTML5 events stand in.
 */
export function onFileDragState(handler: (state: FileDragState) => void): Unsubscribe {
  if (useMock()) {
    const over = () => handler("over");
    const leave = (ev: globalThis.DragEvent) => {
      // leaving for a child element is not leaving the window
      if (!ev.relatedTarget) handler("leave");
    };
    const drop = () => handler("drop");
    window.addEventListener("dragenter", over);
    window.addEventListener("dragover", over);
    window.addEventListener("dragleave", leave);
    window.addEventListener("drop", drop);
    return () => {
      window.removeEventListener("dragenter", over);
      window.removeEventListener("dragover", over);
      window.removeEventListener("dragleave", leave);
      window.removeEventListener("drop", drop);
    };
  }
  let stop = once(null);
  let cancelled = false;
  void getCurrentWebview()
    .onDragDropEvent((e) => {
      const type = e.payload.type;
      handler(type === "leave" ? "leave" : type === "drop" ? "drop" : "over");
    })
    .then((fn) => {
      stop = once(fn);
      if (cancelled) stop.call();
    })
    .catch(() => undefined);
  return () => {
    cancelled = true;
    stop.call();
  };
}

/**
 * P3: pages dragged out of one window's 축소판 / 페이지 grid and released outside it. Every
 * window hears it; the one whose client area contains `screen` (CSS pixels, screen space) drops
 * the pages there with `import_pages_from_doc`.
 */
export interface PagesDropEvent {
  srcDocId: string;
  pages: number[];
  /** the window the drag started in (it ignores its own event) */
  from: string;
  screen: { x: number; y: number };
}

const PAGES_DROP = "pages-drop";

export function onPagesDrop(handler: (e: PagesDropEvent) => void): Unsubscribe {
  return subscribe(PAGES_DROP, handler);
}

export async function emitPagesDrop(e: PagesDropEvent): Promise<void> {
  if (useMock()) {
    appBus.emit(PAGES_DROP, e);
    return;
  }
  const { emit } = await import("@tauri-apps/api/event");
  await emit(PAGES_DROP, e);
}
