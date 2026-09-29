/**
 * App-wide broadcasts (IPC_CONTRACT §8) plus the webview's drag-drop.
 *
 * Every helper returns a synchronous unsubscribe function, so it drops straight into
 * `useEffect(() => onDocChanged(fn), [])` — `listen()` resolves later and is unlistened on cleanup.
 * In mock mode the same helpers are wired to the mock adapter's in-process bus.
 */
import { listen, type Event as TauriEvent, type UnlistenFn } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { useMock } from "./env";
import { appBus } from "./bus";
import type {
  DocChangedEvent, DocSavedEvent, EnginePressureEvent, OpenFileEvent, RecentsChangedEvent, TtsProgressEvent,
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

function subscribe<T>(name: string, handler: (payload: T) => void): Unsubscribe {
  if (useMock()) return appBus.on(name, (p) => handler(p as T));
  let stop = once(null);
  let cancelled = false;
  void listen<T>(name, (e: TauriEvent<T>) => handler(e.payload))
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

/** A file the OS handed us (Finder double-click, `open -a`, Dock drop, argv). */
export function onOpenFile(handler: (e: OpenFileEvent) => void): Unsubscribe {
  return subscribe("open-file", handler);
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

/** v0.3 (V4): read aloud started a sentence (`sentenceIndex`), or finished its queue (`null`). */
export function onTtsProgress(handler: (e: TtsProgressEvent) => void): Unsubscribe {
  return subscribe("tts-progress", handler);
}

/** Native macOS menu item -> the focused window. `id` matches a keymap id (`src/keys/keymap.ts`). */
export function onMenuCommand(handler: (id: string) => void, ids: readonly string[]): Unsubscribe {
  const offs = ids.map((id) => subscribe(`menu:${id.replace(/\./g, "/")}`, () => handler(id)));
  return () => offs.forEach((off) => off());
}

export interface DropPayload { paths: string[] }

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
      if (files.length) handler({ paths: files });
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
      if (e.payload.type === "drop") handler({ paths: e.payload.paths });
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
