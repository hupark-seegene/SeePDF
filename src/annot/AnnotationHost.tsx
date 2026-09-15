/**
 * The lazy entry point of module (d): it registers the tools, installs the sink that turns a
 * finished gesture into an IPC call, subscribes the store to `doc-changed`, owns the annotation
 * keyboard gestures, and renders the four `PageShell` slots.
 *
 * Nothing above it on the critical path imports this file — `src/annot/bridge.tsx` pulls it in
 * with a dynamic `import()` once a document is open, which is what keeps the entry chunk under
 * the 120 kB gz budget (STAGE1D_NOTES §6).
 */
import { useSyncExternalStore } from "react";
import * as api from "../ipc/api";
import type { AnnotId, AnnotSpec, PageIndex } from "../ipc/types";
import type { PageLayerContext, PageLayers } from "../viewer";
import { useSelectionStore } from "../viewer";
import { DRAWING_TOOLS, MARKUP_TOOLS, toolController, type ToolSink } from "../tools/ToolController";
import { registerTools } from "../tools/registry";
import { setAnnotCommandHandler } from "../tools/commands";
import { setStampImage, setStampPicker } from "../tools/stamp";
import { movePatch } from "../tools/hit";
import { useAnnotStore } from "../store/annotStore";
import { useAppStore } from "../store/appStore";
import { useDocStore } from "../store/docStore";
import { useViewStore } from "../store/viewStore";
import {
  annotsOnPage,
  createAnnotation,
  deleteAnnotations,
  duplicateAnnotations,
  flushPatches,
  patchAnnotation,
  specFromAnnot,
} from "./actions";
import { startAnnotSync } from "./sync";
import { AnnotOverlay } from "./AnnotOverlay";
import { RenderProbe } from "./RenderProbe";
import { ToolSurface } from "./ToolSurface";
import { NotePopover, TextBoxEditor, type TextDraft } from "./editors";
import { FormLayer } from "../forms/FormLayer";
import { startFormSync } from "../forms/formStore";
import { markupSpec, type MarkupKind } from "../tools/markup";
import { selectionRectsForPage, selectedPages } from "./selectionQuads";
import "./annot.css";

// ---------------------------------------------------------------------------
// Drafts: a 텍스트 상자 that is being typed but does not exist in the document yet.
// ---------------------------------------------------------------------------

let draft: TextDraft | null = null;
const draftListeners = new Set<() => void>();
let draftNonce = 0;

function setDraft(next: TextDraft | null): void {
  draft = next;
  draftNonce += 1;
  for (const l of draftListeners) l();
}

export function currentDraft(): TextDraft | null {
  return draft;
}

// ---------------------------------------------------------------------------
// The sink: gesture → IPC
// ---------------------------------------------------------------------------

const sink: ToolSink = {
  commit(page, spec: AnnotSpec) {
    void createAnnotation(page, spec, { edit: spec.kind === "note" });
  },
  erase(page, ids) {
    void deleteAnnotations(page, ids);
  },
  patch(page, edits, live) {
    for (const edit of edits) patchAnnotation(page, edit.id, edit.patch, live);
  },
  select(ids) {
    useAnnotStore.getState().select(ids);
  },
  edit(target) {
    if (target.id !== undefined) {
      const found = useAnnotStore.getState().find(target.id);
      if (found) useAnnotStore.getState().setEditing({ page: found.page, id: target.id });
      return;
    }
    if (target.rect) setDraft({ page: target.page, rect: target.rect });
  },
  done() {
    useAppStore.getState().setTool(toolController.current() === "eraser" ? "pen" : "select");
    toolController.arm(toolController.current() === "eraser" ? "pen" : "select");
  },
};

// ---------------------------------------------------------------------------
// Markup: the viewer owns the drag, so the commit hangs off a window pointer-up
// ---------------------------------------------------------------------------

function commitMarkup(): void {
  const tool = toolController.current() as MarkupKind;
  if (!MARKUP_TOOLS.includes(tool)) return;
  const style = useAnnotStore.getState().style;
  const ctx = {
    docId: useDocStore.getState().info?.docId ?? "",
    docGeneration: useDocStore.getState().info?.docGeneration ?? 0,
    style,
    modifiers: { shift: false, alt: false, meta: false, ctrl: false },
    scale: 1,
  };
  let made = false;
  for (const page of selectedPages()) {
    const spec = markupSpec(tool, selectionRectsForPage(page), ctx);
    if (!spec) continue;
    made = true;
    void createAnnotation(page, spec, { select: false });
  }
  // A markup annotation replaces the text selection it was made from; leaving it selected makes
  // the next drag extend the old range instead of starting a new one.
  if (made) useSelectionStore.getState().clear();
}

// ---------------------------------------------------------------------------
// Keyboard: ⌫ / ⌦ delete, arrows nudge, ⌥+arrow nudges by 10, Esc deselects
// ---------------------------------------------------------------------------

const NUDGE_PT = 1;
const NUDGE_BIG_PT = 10;

function selectionByPage(): Map<PageIndex, AnnotId[]> {
  const { selected } = useAnnotStore.getState();
  const map = new Map<PageIndex, AnnotId[]>();
  for (const id of selected) {
    const found = useAnnotStore.getState().find(id);
    if (!found) continue;
    map.set(found.page, [...(map.get(found.page) ?? []), id]);
  }
  return map;
}

function deleteSelection(): boolean {
  const map = selectionByPage();
  if (map.size === 0) return false;
  for (const [page, ids] of map) void deleteAnnotations(page, ids);
  useAnnotStore.getState().select([]);
  return true;
}

function nudge(dx: number, dy: number): boolean {
  const map = selectionByPage();
  if (map.size === 0) return false;
  for (const [page, ids] of map) {
    for (const a of annotsOnPage(page).filter((x) => ids.includes(x.id))) {
      patchAnnotation(page, a.id, movePatch(a, dx, dy), true);
    }
  }
  return true;
}

function duplicateSelection(): boolean {
  const map = selectionByPage();
  if (map.size === 0) return false;
  for (const [page, ids] of map) void duplicateAnnotations(page, ids);
  return true;
}

/** The annotation clipboard is in-process: an `AnnotSpec` survives a page change and a paste. */
let clipboard: { spec: AnnotSpec }[] = [];

function copySelection(cut: boolean): boolean {
  const map = selectionByPage();
  if (map.size === 0) return false;
  clipboard = [];
  for (const [page, ids] of map) {
    for (const a of annotsOnPage(page).filter((x) => ids.includes(x.id))) {
      const spec = specFromAnnot(a);
      if (spec) clipboard.push({ spec });
    }
  }
  if (cut) deleteSelection();
  return clipboard.length > 0;
}

function pasteClipboard(): boolean {
  if (clipboard.length === 0) return false;
  const page = currentPage();
  for (const item of clipboard) void createAnnotation(page, item.spec, { select: false });
  return true;
}

function currentPage(): PageIndex {
  const selected = useAnnotStore.getState().selected;
  if (selected.length) return useAnnotStore.getState().find(selected[0])?.page ?? 0;
  return useViewStore.getState().currentPage;
}

function onKeyDownCapture(e: KeyboardEvent): void {
  const app = useAppStore.getState();
  if (app.mode !== "annotate") return;
  const target = e.target as HTMLElement | null;
  const tag = target?.tagName?.toLowerCase();
  if (tag === "input" || tag === "textarea" || target?.isContentEditable) return;
  const hasSelection = useAnnotStore.getState().selected.length > 0;

  if (e.key === "Escape") {
    if (useAnnotStore.getState().editing || draft) {
      useAnnotStore.getState().setEditing(null);
      setDraft(null);
      e.preventDefault();
      e.stopPropagation();
      return;
    }
    if (hasSelection) {
      useAnnotStore.getState().select([]);
      e.preventDefault();
      e.stopPropagation();
    }
    return;
  }
  if (!hasSelection) return;

  if (e.key === "Backspace" || e.key === "Delete") {
    if (deleteSelection()) {
      e.preventDefault();
      e.stopPropagation();
    }
    return;
  }
  if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "d") {
    if (duplicateSelection()) {
      e.preventDefault();
      e.stopPropagation();
    }
    return;
  }
  const step = e.shiftKey ? NUDGE_BIG_PT : NUDGE_PT;
  const delta: Record<string, [number, number]> = {
    ArrowLeft: [-step, 0],
    ArrowRight: [step, 0],
    ArrowUp: [0, step],
    ArrowDown: [0, -step],
  };
  const move = delta[e.key];
  if (move && nudge(move[0], move[1])) {
    e.preventDefault();
    e.stopPropagation();
  }
}

function onPointerUpWindow(): void {
  if (useAppStore.getState().mode !== "annotate") return;
  commitMarkup();
}

// ---------------------------------------------------------------------------
// Layers
// ---------------------------------------------------------------------------

function Slots({ ctx }: { ctx: PageLayerContext }) {
  return <AnnotOverlay ctx={ctx} />;
}

/** A one-value external store for the draft, so the surface re-renders when a draft opens. */
function useDraftNonce(): number {
  return useSyncExternalStore(
    (l) => {
      draftListeners.add(l);
      return () => {
        draftListeners.delete(l);
      };
    },
    () => draftNonce,
    () => draftNonce,
  );
}

function SurfaceSlot({ ctx }: { ctx: PageLayerContext }) {
  const mode = useAppStore((s) => s.mode);
  const tool = useAppStore((s) => s.tool);
  const editing = useAnnotStore((s) => s.editing);
  const nonce = useDraftNonce();
  const annots = useAnnotStore((s) => s.byPage[ctx.index]);
  const ghosts = useAnnotStore((s) => s.ghosts);
  void nonce;

  const active = mode === "annotate" && (tool === "select" || DRAWING_TOOLS.includes(tool)) ? tool : null;
  const editTarget =
    editing && editing.page === ctx.index
      ? (annots ?? []).find((a) => a.id === editing.id) ?? ghosts.find((g) => g.annot.id === editing.id)?.annot
      : undefined;
  const pageDraft = draft && draft.page === ctx.index ? draft : null;

  return (
    <>
      {active && <ToolSurface ctx={ctx} tool={active} />}
      {editTarget?.kind === "note" && <NotePopover ctx={ctx} annot={editTarget} />}
      {editTarget?.kind === "textbox" && (
        <TextBoxEditor ctx={ctx} annot={editTarget} onClose={() => useAnnotStore.getState().setEditing(null)} />
      )}
      {pageDraft && <TextBoxEditor ctx={ctx} draft={pageDraft} onClose={() => setDraft(null)} />}
    </>
  );
}

function renderLayers(ctx: PageLayerContext): PageLayers {
  return {
    under: <RenderProbe ctx={ctx} />,
    annotations: <Slots ctx={ctx} />,
    forms: <FormLayer ctx={ctx} />,
    surface: <SurfaceSlot ctx={ctx} />,
  };
}

// ---------------------------------------------------------------------------
// Start / stop
// ---------------------------------------------------------------------------

async function pickStamp(id: "stamp" | "signature"): Promise<void> {
  const picked = await api
    .openFileDialog({
      multiple: false,
      filters: [{ name: "PNG / JPEG", extensions: ["png", "jpg", "jpeg"] }],
    })
    .catch(() => null);
  if (picked?.length) {
    setStampImage(id, { path: picked[0] });
  } else {
    useAppStore.getState().setTool("select");
    toolController.arm("select");
  }
}

function start(): () => void {
  registerTools();
  toolController.setSink(sink);
  toolController.arm(useAppStore.getState().tool);
  setStampPicker((id) => void pickStamp(id));
  const offSync = startAnnotSync();
  const offForms = startFormSync();
  setAnnotCommandHandler((id) => {
    if (useAppStore.getState().mode !== "annotate") return false;
    switch (id) {
      case "edit.delete":
        return deleteSelection();
      case "edit.duplicate":
        return duplicateSelection();
      case "edit.copy":
        return copySelection(false);
      case "edit.cut":
        return copySelection(true);
      case "edit.paste":
        return pasteClipboard();
      default:
        return false;
    }
  });
  const offTool = useAppStore.subscribe((s) => {
    if (s.tool !== toolController.current()) toolController.arm(s.tool);
  });
  window.addEventListener("keydown", onKeyDownCapture, true);
  window.addEventListener("pointerup", onPointerUpWindow);
  return () => {
    window.removeEventListener("keydown", onKeyDownCapture, true);
    window.removeEventListener("pointerup", onPointerUpWindow);
    setAnnotCommandHandler(null);
    setStampPicker(null);
    toolController.setSink(null);
    offTool();
    offForms();
    offSync();
    void flushPatches();
  };
}

export const host = { renderLayers, start };
export default host;
