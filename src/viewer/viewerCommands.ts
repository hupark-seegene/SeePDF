/**
 * The keymap ids the **viewer** owns (UI_SPEC §13), bound in the capture phase so they resolve
 * before `useKeymap`'s window listener — which would otherwise route them to the Stage 0 "not
 * implemented" branch of `useCommands`:
 *
 *   ⌘A        전체 선택 → the visible page's text (in 페이지 mode the organizer keeps it)
 *   ⌘C        복사 → the selected text, with real line breaks
 *   ⌘G / ⇧⌘G  다음/이전 찾기 (F3 / Shift+F3 on Windows)
 *   Space     다음 페이지 on a tap (< 200 ms), 손 도구 while held — the one documented chord
 *             collision of the keymap (`sharesChordWith`), which only the pointer layer can settle
 *   Esc       clears the text selection (and still reaches `tool.none`)
 *
 * Everything else — page nav, zoom, rotate, layout, 찾기 — already flows through `useCommands`
 * into `viewStore`/`appStore`, which the scroller observes.
 */
import { useEffect } from "react";
import { matchesChord } from "../keys/keymap";
import { isEditingTarget } from "../keys/useKeymap";
import { useAppStore } from "../store/appStore";
import { useDocStore } from "../store/docStore";
import { useViewStore } from "../store/viewStore";
import { useSearchStore } from "./search/SearchController";
import { getTextLayer } from "./text/textLayers";
import { selectionText, useSelectionStore } from "./text/selection";

/** A tap is shorter than this; longer means the hand tool is being held (UI_SPEC §13). */
export const SPACE_HOLD_MS = 200;

function canvasHasFocus(): boolean {
  if (typeof document === "undefined") return false;
  const active = document.activeElement;
  if (!active || active === document.body) return true;
  return !!active.closest?.(".viewer");
}


/**
 * The clipboard mirror.
 *
 * macOS puts a native Edit menu in front of the webview (`app/menu.rs` uses tauri's predefined
 * `copy:` / `selectAll:` items), so ⌘C and ⌘A never reach a JS key handler — WebKit runs them
 * against the **DOM** selection, which our canvas does not have: the text layer is typed arrays,
 * not a node per character (ARCHITECTURE §5).
 *
 * So we keep one off-screen element holding exactly the selected string and point the DOM
 * selection at it. Native Copy, the Edit menu, the context menu and ⌘C all then copy the right
 * text, and `selectionchange` tells us when WebKit's Select All ran over the canvas so we can turn
 * it into a page selection.
 */
const MIRROR_ID = "seepdf-clipboard-mirror";

function ensureMirror(): HTMLElement {
  let el = document.getElementById(MIRROR_ID);
  if (!el) {
    el = document.createElement("div");
    el.id = MIRROR_ID;
    el.setAttribute("aria-hidden", "true");
    Object.assign(el.style, {
      position: "fixed",
      insetInlineStart: "-9999px",
      insetBlockStart: "0",
      width: "1px",
      height: "1px",
      overflow: "hidden",
      whiteSpace: "pre-wrap",
      userSelect: "text",
      webkitUserSelect: "text",
    } as Partial<CSSStyleDeclaration>);
    document.body.appendChild(el);
  }
  return el;
}

/** Put `text` in the mirror and make it the DOM selection (empty text clears both). */
export function syncClipboardMirror(text: string): void {
  if (typeof document === "undefined") return;
  const el = ensureMirror();
  const selection = document.getSelection();
  if (!text) {
    if (el.textContent) el.textContent = "";
    if (selection && selection.anchorNode && el.contains(selection.anchorNode)) selection.removeAllRanges();
    return;
  }
  if (el.textContent !== text) el.textContent = text;
  if (!selection) return;
  const range = document.createRange();
  range.selectNodeContents(el);
  selection.removeAllRanges();
  selection.addRange(range);
}

/**
 * Copy, synchronously.
 *
 * The order matters: `document.execCommand("copy")` must run **inside** the key event's user
 * gesture, so it goes first (WKWebView rejects `navigator.clipboard.writeText` on a custom scheme,
 * and awaiting that rejection consumes the gesture, which is exactly how the first version of this
 * lost every copy). The async API is only the fallback for environments without execCommand.
 */
export function copyToClipboard(text: string): boolean {
  if (!text || typeof document === "undefined") return false;
  const active = document.activeElement as HTMLElement | null;
  const area = document.createElement("textarea");
  area.value = text;
  area.setAttribute("aria-hidden", "true");
  area.style.position = "fixed";
  area.style.top = "-1000px";
  area.style.opacity = "0";
  document.body.appendChild(area);
  area.focus({ preventScroll: true });
  area.select();
  let ok = false;
  try {
    ok = document.execCommand("copy");
  } catch {
    ok = false;
  }
  area.remove();
  active?.focus?.({ preventScroll: true });
  if (!ok && navigator.clipboard?.writeText) {
    void navigator.clipboard.writeText(text).catch(() => undefined);
  }
  return ok;
}

/** 선택 해제 — the canvas text selection and its clipboard mirror. */
export function clearTextSelection(): void {
  useSelectionStore.getState().clear();
}

/** The text the clipboard would get right now (also used by the context menu later). */
export function currentSelectionText(): string {
  const info = useDocStore.getState().info;
  const selection = useSelectionStore.getState().selection;
  if (!info || !selection) return "";
  return selectionText(selection, info.docGeneration);
}

/** The visible page's text as the selection. Exported: `useCommands` owns `edit.selectAll`. */
export function selectAllOnCurrentPage(): boolean {
  const info = useDocStore.getState().info;
  if (!info) return false;
  const page = useViewStore.getState().currentPage;
  const layer = getTextLayer(info.docId, info.docGeneration, page);
  if (!layer || layer.charCount === 0) return false;
  useSelectionStore.getState().setSelection({
    docId: info.docId,
    anchor: { page, offset: 0 },
    focus: { page, offset: layer.charCount },
  });
  return true;
}

/** 다음/이전 찾기. Exported: `useCommands` owns `edit.findNext` / `edit.findPrevious`. */
export function findStep(direction: 1 | -1): boolean {
  const search = useSearchStore.getState();
  if (search.hits.length === 0) {
    useAppStore.getState().setSidebarTab("search");
    return false;
  }
  search.step(direction);
  return true;
}

/** When an Esc last cleared a text selection (`performance.now()`), 0 = never. */
let escapeClearedAt = 0;

/**
 * `true` once when the Esc being handled right now cleared a text selection — so `tool.none`
 * knows that Esc already did something and must not also close 분할 보기 (P2).
 */
export function escapeClearedSelection(): boolean {
  const recent = escapeClearedAt > 0 && performance.now() - escapeClearedAt < 250;
  escapeClearedAt = 0;
  return recent;
}

/** Bound by `<Viewer>` while a document is open. */
export function useViewerCommands(enabled: boolean): void {
  useEffect(() => {
    if (!enabled || typeof window === "undefined") return;
    let spaceDownAt = 0;
    let spaceHeld = false;
    let holdTimer = 0;

    const claim = (e: KeyboardEvent) => {
      e.preventDefault();
      e.stopPropagation();
    };

    const onKeyDown = (e: KeyboardEvent) => {
      const os = useAppStore.getState().os;
      if (isEditingTarget(e.target)) return;

      if (matchesChord(e, "Escape", os)) {
        if (useSelectionStore.getState().selection) escapeClearedAt = performance.now();
        useSelectionStore.getState().clear();
        return; // Esc still reaches `tool.none`
      }

      if (!canvasHasFocus()) return;

      if (matchesChord(e, "Cmd+A", os) && useAppStore.getState().mode !== "pages") {
        if (selectAllOnCurrentPage()) claim(e);
        return;
      }

      if (matchesChord(e, "Cmd+C", os)) {
        const text = currentSelectionText();
        if (!text) return;
        claim(e);
        copyToClipboard(text);
        return;
      }

      const next = os === "windows" ? matchesChord(e, "F3", os) : matchesChord(e, "Cmd+G", os);
      const previous =
        os === "windows" ? matchesChord(e, "Shift+F3", os) : matchesChord(e, "Cmd+Shift+G", os);
      if (next || previous) {
        claim(e);
        findStep(previous ? -1 : 1);
        return;
      }

      if (matchesChord(e, "Space", os) || matchesChord(e, "Shift+Space", os)) {
        if (e.repeat) {
          claim(e);
          return;
        }
        claim(e);
        if (e.shiftKey) {
          const view = useViewStore.getState();
          view.goToPage(Math.max(0, view.currentPage - 1));
          return;
        }
        spaceDownAt = performance.now();
        spaceHeld = false;
        window.clearTimeout(holdTimer);
        holdTimer = window.setTimeout(() => {
          spaceHeld = true;
          useAppStore.getState().setTool("hand", true);
        }, SPACE_HOLD_MS);
      }
    };

    const onKeyUp = (e: KeyboardEvent) => {
      const os = useAppStore.getState().os;
      if (!matchesChord(e, "Space", os) && e.code !== "Space") return;
      if (!spaceDownAt) return;
      e.preventDefault();
      e.stopPropagation();
      window.clearTimeout(holdTimer);
      const held = spaceHeld || performance.now() - spaceDownAt >= SPACE_HOLD_MS;
      spaceDownAt = 0;
      spaceHeld = false;
      if (held) {
        useAppStore.getState().releaseMomentary();
        return;
      }
      const info = useDocStore.getState().info;
      const view = useViewStore.getState();
      if (info) view.goToPage(Math.min(info.pageCount - 1, view.currentPage + 1));
    };

    // Keep the mirror in step with our own selection (rAF-coalesced: a drag fires per pointermove).
    let mirrorFrame = 0;
    const scheduleMirror = () => {
      if (mirrorFrame) return;
      mirrorFrame = requestAnimationFrame(() => {
        mirrorFrame = 0;
        syncClipboardMirror(currentSelectionText());
      });
    };
    const unsubscribeSelection = useSelectionStore.subscribe((state, previous) => {
      if (state.selection !== previous.selection) scheduleMirror();
    });
    scheduleMirror();

    /** WebKit's own Select All (the native Edit menu item) landing on the canvas. */
    const onSelectionChange = () => {
      const selection = document.getSelection();
      if (!selection || selection.isCollapsed || selection.rangeCount === 0) return;
      const canvas = document.querySelector(".canvas.viewer");
      if (!canvas || !selection.containsNode(canvas, true)) return;
      if (selectAllOnCurrentPage()) syncClipboardMirror(currentSelectionText());
      else selection.removeAllRanges();
    };

    const onCopy = (e: ClipboardEvent) => {
      if (isEditingTarget(e.target)) return;
      const text = currentSelectionText();
      if (!text) return;
      e.clipboardData?.setData("text/plain", text);
      e.preventDefault();
    };

    // Stage 2: 다음 찾기 / 이전 찾기 / 선택 해제 / 전체 선택 arrive as `menu:<id>` and are
    // dispatched by `src/app/useCommands.ts`, which calls back into `findStep`,
    // `selectAllOnCurrentPage` and `clearTextSelection`. The duplicate listener that used to
    // live here is gone (STAGE1C_NOTES §7.2).

    window.addEventListener("keydown", onKeyDown, { capture: true });
    window.addEventListener("keyup", onKeyUp, { capture: true });
    document.addEventListener("copy", onCopy);
    document.addEventListener("selectionchange", onSelectionChange);
    return () => {
      window.removeEventListener("keydown", onKeyDown, { capture: true });
      window.removeEventListener("keyup", onKeyUp, { capture: true });
      document.removeEventListener("copy", onCopy);
      document.removeEventListener("selectionchange", onSelectionChange);
      unsubscribeSelection();
      if (mirrorFrame) cancelAnimationFrame(mirrorFrame);
      window.clearTimeout(holdTimer);
      syncClipboardMirror("");
    };
  }, [enabled]);
}
