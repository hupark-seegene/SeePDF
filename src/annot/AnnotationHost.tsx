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
import { setDrawnSignature, setStampImage, setStampPicker, stampImage } from "../tools/stamp";
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
import { closeDialog, openDialog } from "../dialogs/dialogState";
import { startAnnotSync } from "./sync";
import { dragPatch, endDrag, startDragHide } from "./dragHide";
import { startToolDefaultsSync } from "./toolDefaults";
import { AnnotOverlay } from "./AnnotOverlay";
import { ToolSurface } from "./ToolSurface";
import { LinkLayer } from "./LinkLayer";
import { NotePopover, TextBoxEditor, ThreadPopover, type TextDraft } from "./editors";
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
    // Only the 선택 tool's move / resize reaches here: the drag hides the bitmap copy (P1-12).
    dragPatch(page, edits, live);
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
  // a widget with its own ⌫ / arrows (the 목차 editor's tree, P2)
  if (target?.closest?.("[data-own-keys]")) return;
  const hasSelection = useAnnotStore.getState().selected.length > 0;

  if (e.key === "Escape") {
    if (useAnnotStore.getState().editing || useAnnotStore.getState().thread || draft) {
      useAnnotStore.getState().setEditing(null);
      useAnnotStore.getState().setThread(null);
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
  const thread = useAnnotStore((s) => s.thread);
  const nonce = useDraftNonce();
  const annots = useAnnotStore((s) => s.byPage[ctx.index]);
  const ghosts = useAnnotStore((s) => s.ghosts);
  void nonce;

  const active = mode === "annotate" && (tool === "select" || DRAWING_TOOLS.includes(tool)) ? tool : null;
  const editTarget =
    editing && editing.page === ctx.index
      ? (annots ?? []).find((a) => a.id === editing.id) ?? ghosts.find((g) => g.annot.id === editing.id)?.annot
      : undefined;
  // P2 threads: 답글 on anything but a note (a note's thread lives in its own popover)
  const threadTarget =
    thread && thread.page === ctx.index && thread.id !== editTarget?.id
      ? (annots ?? []).find((a) => a.id === thread.id)
      : undefined;
  const pageDraft = draft && draft.page === ctx.index ? draft : null;

  return (
    <>
      {/* P2: links are live in 읽기 mode (hover outline, click follows) */}
      {mode === "read" && <LinkLayer ctx={ctx} />}
      {active && <ToolSurface ctx={ctx} tool={active} />}
      {editTarget?.kind === "note" && <NotePopover ctx={ctx} annot={editTarget} />}
      {threadTarget && threadTarget.kind !== "note" && <ThreadPopover ctx={ctx} annot={threadTarget} />}
      {editTarget?.kind === "textbox" && (
        <TextBoxEditor ctx={ctx} annot={editTarget} onClose={() => useAnnotStore.getState().setEditing(null)} />
      )}
      {pageDraft && <TextBoxEditor ctx={ctx} draft={pageDraft} onClose={() => setDraft(null)} />}
    </>
  );
}

function renderLayers(ctx: PageLayerContext): PageLayers {
  return {
    annotations: <Slots ctx={ctx} />,
    forms: <FormLayer ctx={ctx} />,
    surface: <SurfaceSlot ctx={ctx} />,
  };
}

// ---------------------------------------------------------------------------
// Start / stop
// ---------------------------------------------------------------------------

/**
 * 서명 만들기 (UI_SPEC §6): the sheet a user with no signature file needs. Drawing wins over
 * picking — `setDrawnSignature` clears any previously picked image — and 이미지 선택… inside
 * the sheet hands straight over to [`pickStamp`], so the image path is still one click away.
 * Cancelling either one disarms the tool rather than leaving a tool that cannot commit.
 */
function openSignatureSheet(): void {
  openDialog("signature", {
    onDrawn: (signature: { paths: number[][]; aspect: number }) => setDrawnSignature(signature),
    // 입력 / a saved typed signature (P1-9): a PNG on disk, placed like a picked image.
    onImage: (signature: { path: string; aspect: number }) =>
      setStampImage("signature", { path: signature.path }, signature.aspect),
    onChooseImage: () => {
      closeDialog("signature");
      void pickStamp("signature");
    },
  });
}

/** 도장 선택 (P1-12): the built-ins (결재 / 승인 / 기밀 first) or 이미지 선택…. */
function openStampPicker(): void {
  const current = stampImage("stamp");
  openDialog("stampPicker", {
    current: current && "builtin" in current ? current.builtin : null,
    onPick: (builtin: string) => setStampImage("stamp", { builtin }),
    onChooseImage: () => {
      closeDialog("stampPicker");
      void pickStamp("stamp");
    },
  });
}

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

/**
 * ⌘C / ⌘X / ⌘V for **annotations**, on macOS.
 *
 * The native Edit menu's Cut/Copy/Paste are predefined items — they are what makes ⌘C work in a
 * text field and over the canvas (`viewerCommands.ts`'s clipboard mirror), so they stay — and
 * macOS routes them through the responder chain into WKWebView, which raises these DOM events.
 * That is the seam `STAGE1D_NOTES` §7.8 was missing: the key never reaches a `keydown` handler,
 * but the clipboard event does. The annotation clipboard is in-process (an `AnnotSpec`), so
 * nothing is written to the system pasteboard.
 */
function onClipboardEvent(e: ClipboardEvent): void {
  if (useAppStore.getState().mode !== "annotate") return;
  const target = e.target as HTMLElement | null;
  const tag = target?.tagName?.toLowerCase();
  if (tag === "input" || tag === "textarea" || target?.isContentEditable) return;
  const handled =
    e.type === "copy"
      ? copySelection(false)
      : e.type === "cut"
        ? copySelection(true)
        : pasteClipboard();
  if (handled) e.preventDefault();
}

function start(): () => void {
  registerTools();
  toolController.setSink(sink);
  toolController.arm(useAppStore.getState().tool);
  setStampPicker((id) => (id === "signature" ? openSignatureSheet() : openStampPicker()));
  const offSync = startAnnotSync();
  const offDragHide = startDragHide();
  const offToolDefaults = startToolDefaultsSync();
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
  document.addEventListener("copy", onClipboardEvent, true);
  document.addEventListener("cut", onClipboardEvent, true);
  document.addEventListener("paste", onClipboardEvent, true);
  return () => {
    window.removeEventListener("keydown", onKeyDownCapture, true);
    window.removeEventListener("pointerup", onPointerUpWindow);
    document.removeEventListener("copy", onClipboardEvent, true);
    document.removeEventListener("cut", onClipboardEvent, true);
    document.removeEventListener("paste", onClipboardEvent, true);
    setAnnotCommandHandler(null);
    setStampPicker(null);
    toolController.setSink(null);
    offTool();
    offForms();
    offSync();
    offToolDefaults();
    // A drag in flight is restored (unhidden) and committed before the last flush.
    offDragHide();
    void endDrag().then(() => flushPatches());
  };
}

export const host = { renderLayers, start };
export default host;
