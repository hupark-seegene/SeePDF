/**
 * The lazy entry of 편집 mode (Stage 7). `annot/bridge.tsx` imports this chunk the first time the
 * mode is entered and puts `renderSurface` into every page's `surface` slot while 편집 is active.
 *
 * `start()` binds the store to the open document and owns the mode's keys — ⌫ / ⌦ delete, arrows
 * nudge (⇧ × 10), Esc closes the editor / deselects / returns to 선택 — but only while no text
 * field has focus. Undo / redo stay global.
 */
import type { PageLayerContext } from "../viewer";
import { useAppStore } from "../store/appStore";
import { useDocStore } from "../store/docStore";
import { setEditCommandHandler } from "../tools/commands";
import { toolController } from "../tools/ToolController";
import { useEditStore } from "./editStore";
import { cancelSession, commitSession, deleteSelection, moveObjects } from "./actions";
import { EditLayer } from "./EditLayer";
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
  const sel = store.selection;
  if (!sel || store.session) return;
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

function renderSurface(ctx: PageLayerContext) {
  return <EditLayer ctx={ctx} />;
}

function start(): () => void {
  const bind = () => useEditStore.getState().bind(useDocStore.getState().info?.docId ?? null);
  bind();
  const offDoc = useDocStore.subscribe((s, prev) => {
    if (s.info?.docId !== prev.info?.docId) bind();
  });
  window.addEventListener("keydown", onEditKeyDown, true);
  setEditCommandHandler((id) => {
    if (useAppStore.getState().mode !== "edit") return false;
    if (id === "edit.delete" && useEditStore.getState().selection && !useEditStore.getState().session) {
      void deleteSelection();
      return true;
    }
    return false;
  });
  return () => {
    window.removeEventListener("keydown", onEditKeyDown, true);
    setEditCommandHandler(null);
    offDoc();
    // leaving 편집 keeps what was typed
    if (useEditStore.getState().session) void commitSession();
    useEditStore.getState().clearSelection();
  };
}

export const editHost = { renderSurface, start };
export default editHost;
