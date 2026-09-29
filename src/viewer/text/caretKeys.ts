/**
 * The keys of 캐럿 탐색 (H9, `caret.ts`): mounted by the focused pane's scroller while F7 is on.
 * A capture-phase window listener, so it runs before the keymap (whose ← → ↑ ↓ page and whose
 * letters arm tools) and before the 주석 host's nudge — and claims only the keys it acts on.
 */
import { useEffect } from "react";
import type { AnnotSpec, DocInfo, PageIndex, Point } from "../../ipc/types";
import { useDocStore } from "../../store/docStore";
import { useAppStore } from "../../store/appStore";
import { useAnnotStore } from "../../store/annotStore";
import { useViewStore } from "../../store/viewStore";
import { styleFor } from "../../store/toolStyles";
import { isDialogOpen } from "../../dialogs/dialogState";
import { ensureTextLayer, getTextLayer } from "./textLayers";
import { isEmptySelection, useSelectionStore, type TextPoint } from "./selection";
import { caretRect, caretX, lineEdge, offsetOnLine, stepChar, stepLine, useCaretStore } from "./caret";

/** 메모's icon box (tools/note.ts `NOTE_SIZE_PT`, not imported: that module is the tools chunk's). */
const NOTE_PT = 22;

/**
 * Keys meant for something else: a field being typed in, a widget with its own arrows, or any
 * control outside the canvas (the sidebar's rails, a toolbar) — the caret takes keys only while
 * the canvas (or nothing in particular) has the focus.
 */
function elsewhere(target: EventTarget | null): boolean {
  const el = target as HTMLElement | null;
  if (!el || !el.tagName) return false; // window / document
  const tag = el.tagName.toLowerCase();
  if (tag === "input" || tag === "textarea" || tag === "select" || el.isContentEditable || el.closest?.("[data-own-keys]")) {
    return true;
  }
  return tag !== "body" && tag !== "html" && !el.closest?.(".canvas.viewer");
}

/** The physical letter, so the Korean input source (ㅜ for N) still works. */
function letter(e: KeyboardEvent): string {
  if (e.code?.startsWith("Key")) return e.code.slice(3).toLowerCase();
  return e.key.length === 1 ? e.key.toLowerCase() : "";
}

type Move = { page: PageIndex; offset: number; goalX: number | null } | null;

/** Where an arrow / Home / End takes the caret, crossing onto the next / previous page at an edge. */
function target(info: DocInfo, key: string): Move {
  const caret = useCaretStore.getState();
  const layer = getTextLayer(info.docId, info.docGeneration, caret.page);
  if (!layer) return null;
  const neighbour = (delta: 1 | -1) => {
    const page = caret.page + delta;
    if (page < 0 || page >= info.pageCount) return null;
    ensureTextLayer(info.docId, info.docGeneration, page);
    return { page, layer: getTextLayer(info.docId, info.docGeneration, page) };
  };
  switch (key) {
    case "ArrowRight":
    case "ArrowLeft": {
      const delta = key === "ArrowRight" ? 1 : -1;
      const next = stepChar(layer, caret.offset, delta);
      if (next !== null) return { page: caret.page, offset: next, goalX: null };
      const other = neighbour(delta);
      if (!other?.layer) return other ? { page: other.page, offset: 0, goalX: null } : null;
      return { page: other.page, offset: delta > 0 ? 0 : other.layer.charCount, goalX: null };
    }
    case "ArrowDown":
    case "ArrowUp": {
      const delta = key === "ArrowDown" ? 1 : -1;
      const goal = caret.goalX ?? caretX(layer, caret.offset);
      const next = stepLine(layer, caret.offset, delta, goal);
      if (next !== null) return { page: caret.page, offset: next, goalX: goal };
      const other = neighbour(delta);
      if (!other) return null;
      if (!other.layer || other.layer.lineCount === 0) return { page: other.page, offset: 0, goalX: goal };
      const line = delta > 0 ? 0 : other.layer.lineCount - 1;
      return { page: other.page, offset: offsetOnLine(other.layer, line, goal), goalX: goal };
    }
    case "Home":
    case "End":
      return { page: caret.page, offset: lineEdge(layer, caret.offset, key === "End"), goalX: null };
    default:
      return null;
  }
}

/** 메모 at the caret: the note's top-left at the caret's x and its line's top, kept on the page. */
export function noteAtCaret(info: DocInfo): { page: PageIndex; at: Point } | null {
  const caret = useCaretStore.getState();
  const layer = getTextLayer(info.docId, info.docGeneration, caret.page);
  const crop = info.pages[caret.page]?.crop;
  if (!crop) return null;
  const bar = layer ? caretRect(layer, caret.offset) : null;
  const x = bar ? bar.l : crop.l + NOTE_PT;
  const y = bar ? bar.t : crop.t - NOTE_PT;
  const at: Point = [
    Math.min(Math.max(x, crop.l), crop.r - NOTE_PT),
    Math.min(Math.max(y, crop.b + NOTE_PT), crop.t),
  ];
  return { page: caret.page, at };
}

async function addNote(info: DocInfo): Promise<void> {
  const spot = noteAtCaret(info);
  if (!spot) return;
  const app = useAppStore.getState();
  // a note is a 주석: 읽기 switches, as 답글 does (UI_SPEC §12)
  if (app.mode === "read") app.setMode("annotate");
  const color = styleFor("note", useAnnotStore.getState().toolDefaults).color;
  const spec: AnnotSpec = { kind: "note", at: spot.at, color, contents: "" };
  const { createAnnotation } = await import("../../annot/actions");
  await createAnnotation(spot.page, spec, { edit: true });
}

function markup(kind: "highlight" | "underline" | "strikeout"): void {
  void import("../../app/textMenu").then((m) => m.markupSelection(kind));
}

export function onCaretKey(e: KeyboardEvent): void {
  const caret = useCaretStore.getState();
  if (!caret.on || e.defaultPrevented || isDialogOpen() || elsewhere(e.target)) return;
  if (e.metaKey || e.ctrlKey || e.altKey) return; // ⌘C and friends keep working on the selection
  const info = useDocStore.getState().info;
  if (!info || caret.docId !== info.docId) return;

  const key = letter(e);
  if (!e.shiftKey && (key === "n" || key === "h" || key === "u" || key === "k")) {
    if (key === "n") {
      void addNote(info);
    } else {
      const selection = useSelectionStore.getState().selection;
      if (!selection || selection.docId !== info.docId || isEmptySelection(selection)) return;
      markup(key === "h" ? "highlight" : key === "u" ? "underline" : "strikeout");
    }
    e.preventDefault();
    e.stopImmediatePropagation();
    return;
  }

  const move = target(info, e.key);
  if (!move) {
    if (e.key.startsWith("Arrow") || e.key === "Home" || e.key === "End") {
      // at the document's edge: swallow it, so the keymap does not page away under the caret
      e.preventDefault();
      e.stopImmediatePropagation();
    }
    return;
  }
  e.preventDefault();
  e.stopImmediatePropagation();

  const from: TextPoint = { page: caret.page, offset: caret.offset };
  const to: TextPoint = { page: move.page, offset: move.offset };
  const selection = useSelectionStore.getState();
  if (e.shiftKey) {
    const existing = selection.selection;
    const continues =
      existing && existing.docId === info.docId && existing.focus.page === from.page && existing.focus.offset === from.offset;
    selection.setSelection({ docId: info.docId, anchor: continues ? existing.anchor : from, focus: to });
  } else if (selection.selection) {
    selection.clear();
  }
  useCaretStore.getState().moveTo(move.page, move.offset, move.goalX);
  if (move.page !== caret.page) useViewStore.getState().goToPage(move.page);
}

/** Mounts the caret keys while `active` (the focused pane, with 캐럿 탐색 on). */
export function useCaretKeys(active: boolean): void {
  useEffect(() => {
    if (!active) return;
    window.addEventListener("keydown", onCaretKey, true);
    return () => window.removeEventListener("keydown", onCaretKey, true);
  }, [active]);
}

/** F7: on at the selection's end (or the top of the page in view), or off. */
export function toggleCaret(): boolean {
  const caret = useCaretStore.getState();
  if (caret.on) {
    caret.set(false);
    return false;
  }
  const info = useDocStore.getState().info;
  if (!info) return false;
  const selection = useSelectionStore.getState().selection;
  const at =
    selection && selection.docId === info.docId
      ? selection.focus
      : { page: useViewStore.getState().currentPage, offset: 0 };
  ensureTextLayer(info.docId, info.docGeneration, at.page);
  caret.set(true, { docId: info.docId, page: at.page, offset: at.offset });
  return true;
}

