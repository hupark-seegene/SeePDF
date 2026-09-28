/**
 * The lazy entry of 편집 mode (Stage 7). `annot/bridge.tsx` imports this chunk the first time the
 * mode is entered and puts `renderSurface` into every page's `surface` slot while 편집 is active.
 *
 * `start()` binds the store to the open document and owns the mode's keys — ⌫ / ⌦ delete (the
 * selected object, or the selected 영역 표시 mark), arrows nudge (⇧ × 10), Esc closes the editor /
 * deselects / returns to 선택 — but only while no text field has focus. Undo / redo stay global.
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
import { cancelSession, commitSession, deleteSelection, moveObjects } from "./actions";
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
    const store = useEditStore.getState();
    if (id === "edit.delete" && store.markSel !== null && !store.session) {
      store.removeMark(store.markSel);
      return true;
    }
    if (id === "edit.delete" && store.selection && !store.session) {
      void deleteSelection();
      return true;
    }
    return false;
  });
  setEditLeaveGuard(confirmLeave);
  const offMarks = watchMarks();
  const offChanged = onDocChanged(onDocChangedForMarks);
  return () => {
    window.removeEventListener("keydown", onEditKeyDown, true);
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
