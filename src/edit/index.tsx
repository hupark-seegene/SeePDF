/**
 * The lazy entry of 편집 mode (Stage 7). `annot/bridge.tsx` imports this chunk the first time the
 * mode is entered and puts `renderSurface` into every page's `surface` slot while 편집 is active.
 *
 * `start()` binds the store to the open document and owns the mode's keys — ⌫ / ⌦ delete (the
 * selected object, or the selected 영역 표시 mark), arrows nudge (⇧ × 10), ⌘C / ⌘V / ⌘D copy,
 * paste and duplicate objects (`duplicate_objects`), Esc closes the editor / deselects / returns
 * to 선택 — but only while no text field has focus. Undo / redo stay global.
 *
 * ⌘C / ⌘V arrive three ways and each claims the event only when it acts: a `keydown` (Windows,
 * tests), the DOM `copy` / `paste` events the native Edit menu's predefined items raise on macOS
 * (the key never reaches JS there — see `AnnotationHost.tsx`), and `edit.copy` / `edit.paste` /
 * `edit.duplicate` from the command bus (the menu's custom 복제 item).
 * It also runs the 영역 표시 lifecycle (`redact.ts`): previews while marks change, the leave guard,
 * and dropping the marks when the document moves under them or the mode is left.
 */
import type { PageLayerContext } from "../viewer";
import { useAppStore } from "../store/appStore";
import { useDocStore } from "../store/docStore";
import { onDocChanged } from "../ipc/events";
import { setEditCommandHandler, setEditLeaveGuard } from "../tools/commands";
import { toolController } from "../tools/ToolController";
import { useEditStore } from "./editStore";
import {
  cancelSession, clearObjectClipboard, commitSession, copySelection, deleteSelection, duplicateSelection,
  hasObjectClipboard, moveObjects, pasteObjects,
} from "./actions";
import { EditLayer } from "./EditLayer";
import { confirmLeave, dropMarks, onDocChangedForMarks, watchMarks } from "./redact";
import "./edit.css";

function typing(target: EventTarget | null): boolean {
  const el = target as HTMLElement | null;
  const tag = el?.tagName?.toLowerCase();
  return tag === "input" || tag === "textarea" || tag === "select" || !!el?.isContentEditable;
}

export function onEditKeyDown(e: KeyboardEvent): void {
  if (useAppStore.getState().mode !== "edit" || typing(e.target)) return;
  const store = useEditStore.getState();
  const claim = () => {
    e.preventDefault();
    e.stopPropagation();
  };
  if (e.key === "Escape") {
    if (store.session) {
      cancelSession();
      return claim();
    }
    if (store.markSel !== null) {
      store.selectMark(null);
      return claim();
    }
    if (store.selection) {
      store.clearSelection();
      return claim();
    }
    const tool = useAppStore.getState().tool;
    if (tool !== "select") {
      useAppStore.getState().setTool("select");
      toolController.arm("select");
      return claim();
    }
    return;
  }
  if (store.markSel !== null && !store.session && (e.key === "Backspace" || e.key === "Delete")) {
    claim();
    store.removeMark(store.markSel);
    return;
  }
  const letter = shortcutLetter(e);
  if (letter === "v" && !store.session && hasObjectClipboard()) {
    claim();
    void pasteObjects();
    return;
  }
  const sel = store.selection;
  if (!sel || store.session) return;
  if (letter === "c") {
    if (copySelection()) claim();
    return;
  }
  if (letter === "d") {
    claim();
    void duplicateSelection();
    return;
  }
  if (e.key === "Backspace" || e.key === "Delete") {
    claim();
    void deleteSelection();
    return;
  }
  const step = e.shiftKey ? 10 : 1;
  const delta: Record<string, [number, number]> = {
    ArrowLeft: [-step, 0],
    ArrowRight: [step, 0],
    ArrowUp: [0, step],
    ArrowDown: [0, -step],
  };
  const d = delta[e.key];
  if (d) {
    claim();
    void moveObjects(sel.page, sel.ids, d[0], d[1]);
  }
}

/**
 * The letter of a plain ⌘/Ctrl + letter chord, or "". With the Korean input source on, `e.key` is
 * the jamo ("ㅊ" for C), so a non-Latin key falls back to the physical key (`KeyC`).
 */
function shortcutLetter(e: KeyboardEvent): string {
  if (!(e.metaKey || e.ctrlKey) || e.altKey || e.shiftKey) return "";
  const key = e.key.toLowerCase();
  if (/^[a-z]$/.test(key)) return key;
  return e.code?.startsWith("Key") ? e.code.slice(3).toLowerCase() : "";
}

/** The macOS route of ⌘C / ⌘V (see the header). A field keeps its own clipboard. */
function onClipboardEvent(e: ClipboardEvent): void {
  if (useAppStore.getState().mode !== "edit" || typing(e.target)) return;
  if (useEditStore.getState().session) return;
  if (e.type === "copy") {
    if (copySelection()) e.preventDefault();
  } else if (e.type === "paste" && hasObjectClipboard()) {
    e.preventDefault();
    void pasteObjects();
  }
}

function renderSurface(ctx: PageLayerContext) {
  return <EditLayer ctx={ctx} />;
}

function start(): () => void {
  const bind = () => useEditStore.getState().bind(useDocStore.getState().info?.docId ?? null);
  bind();
  const offDoc = useDocStore.subscribe((s, prev) => {
    if (s.info?.docId !== prev.info?.docId) {
      bind();
      clearObjectClipboard();
    }
  });
  window.addEventListener("keydown", onEditKeyDown, true);
  setEditCommandHandler((id) => {
    if (useAppStore.getState().mode !== "edit") return false;
    const store = useEditStore.getState();
    if (id === "edit.delete" && store.markSel !== null && !store.session) {
      store.removeMark(store.markSel);
      return true;
    }
    if (id === "edit.delete" && store.selection && !store.session) {
      void deleteSelection();
      return true;
    }
    if (id === "edit.copy") return copySelection();
    if (id === "edit.duplicate" && store.selection && !store.session) {
      void duplicateSelection();
      return true;
    }
    if (id === "edit.paste" && hasObjectClipboard() && !store.session) {
      void pasteObjects();
      return true;
    }
    return false;
  });
  document.addEventListener("copy", onClipboardEvent, true);
  document.addEventListener("paste", onClipboardEvent, true);
  setEditLeaveGuard(confirmLeave);
  const offMarks = watchMarks();
  const offChanged = onDocChanged(onDocChangedForMarks);
  return () => {
    window.removeEventListener("keydown", onEditKeyDown, true);
    document.removeEventListener("copy", onClipboardEvent, true);
    document.removeEventListener("paste", onClipboardEvent, true);
    setEditCommandHandler(null);
    setEditLeaveGuard(null);
    offMarks();
    offChanged();
    offDoc();
    // pending marks never outlive the mode (the leave guard asked, where it could)
    dropMarks();
    // leaving 편집 keeps what was typed
    if (useEditStore.getState().session) void commitSession();
    useEditStore.getState().clearSelection();
  };
}

export const editHost = { renderSurface, start };
export default editHost;
