/**
 * The canvas and 축소판 context menus of UI_SPEC §12.
 *
 * `src/viewer/**` and `src/sidebar/**` belong to module (c), so instead of adding handlers there we
 * read the page index off the DOM contract those modules already publish: `.page-shell[data-page]`
 * (frozen in STAGE1C_NOTES §1.2) and the `.thumb` button of the rail. `App.tsx` installs one
 * document-level `contextmenu` listener that calls in here.
 *
 * Items that belong to the annotation tools (붙여넣기 · 메모 추가 · 스냅샷) are deliberately absent:
 * module (d) owns them and can push them into `items` from the same seam.
 *
 * While a text selection exists the canvas menu opens with UI_SPEC §12 "Text selection": 복사 ·
 * 형광펜 · 밑줄 · 취소선 · 메모 추가 · 영역 표시로 표시 · 검색 (the actions live in the lazy
 * `textMenu.ts`; 복사 runs here, inside the click's user gesture).
 */
import { openContextMenu, type MenuEntry } from "./contextMenuStore";
import { openDialog } from "../dialogs/dialogState";
import { useAppStore } from "../store/appStore";
import { useDocStore } from "../store/docStore";
import { paneView, useViewStore, type PaneId } from "../store/viewStore";
import { usePagesStore } from "../store/pagesStore";
import { editLeaveGuard } from "../tools/commands";
import {
  copyToClipboard, currentSelectionText, isEmptySelection, makePageLayerContext, useSelectionStore,
} from "../viewer";
import { shortcutFor } from "../keys/keymap";
import { annotAt } from "../tools/hit";
import { GRAB_PX } from "../tools/select";
import { annotsOnPage, deleteAnnotations, openThread } from "../annot/actions";
import type { Annot, PageIndex, PageOp } from "../ipc/types";
import { permissionBlock } from "./permissions";

/** The page a context-menu event happened on, or `null` when it was not over a page. */
export function pageFromEvent(target: EventTarget | null): { page: PageIndex; source: "canvas" | "thumbnail" } | null {
  const el = target as HTMLElement | null;
  if (!el?.closest) return null;
  // a layer that renders its own menu (the annotation overlay) marks itself and we stay out of it
  if (el.closest("[data-context-menu]")) return null;
  const shell = el.closest<HTMLElement>(".page-shell");
  if (shell?.dataset.page !== undefined) return { page: Number(shell.dataset.page), source: "canvas" };
  const thumb = el.closest<HTMLElement>(".thumb");
  if (thumb) {
    if (thumb.dataset.page !== undefined) return { page: Number(thumb.dataset.page), source: "thumbnail" };
    // pre-Stage-2 fallback: the number chip, for any rail that has no `data-page`
    const n = Number(thumb.querySelector(".thumb-num")?.textContent);
    if (Number.isFinite(n) && n >= 1) return { page: n - 1, source: "thumbnail" };
  }
  return null;
}

function run(ops: PageOp[]): void {
  void import("../dialogs/flows").then((m) => m.runPageOps(ops));
}

type TextMenu = typeof import("./textMenu");

function textMenu(fn: (m: TextMenu) => unknown): void {
  void import("./textMenu").then(fn);
}

/**
 * The annotation under a canvas right-click, in the pane it happened in (분할 보기: each pane has
 * its own zoom and rotation). Replies are never on the page, so they are never under the pointer.
 */
function annotationUnder(page: PageIndex, x: number, y: number, target: EventTarget | null | undefined): Annot | null {
  const info = useDocStore.getState().info;
  const geom = info?.pages[page];
  const shell = (target as HTMLElement | null)?.closest?.<HTMLElement>(".page-shell");
  if (!info || !geom || !shell) return null;
  const pane = (shell.closest<HTMLElement>("[data-pane]")?.dataset.pane ?? "main") as PaneId;
  const view = paneView(useViewStore.getState(), pane);
  const box = shell.getBoundingClientRect();
  const ctx = makePageLayerContext({
    docId: info.docId,
    docGeneration: info.docGeneration,
    page: geom,
    rotation: view.rotation,
    zoomPercent: view.zoomPercent,
    width: box.width,
    height: box.height,
  });
  const [px, py] = ctx.toPage(x - box.left, y - box.top);
  return annotAt(annotsOnPage(page), px, py, GRAB_PX / Math.max(0.01, ctx.scale));
}

export function openPageContextMenu(
  page: PageIndex,
  source: "canvas" | "thumbnail",
  x: number,
  y: number,
  target?: EventTarget | null,
): void {
  const info = useDocStore.getState().info;
  if (!info) return;
  const view = useViewStore.getState();
  const app = useAppStore.getState();

  // v0.3 pkg3 (S5): entries the document's permissions forbid are disabled, with the reason
  const annotBlock = permissionBlock("mode.annotate", info);
  const pagesBlock = permissionBlock("mode.pages", info);
  const redactBlock = permissionBlock("tools.redact", info);

  // UI_SPEC §12 Annotation (P2 threads): 답글 opens the thread with the reply box focused
  const annot = source === "canvas" ? annotationUnder(page, x, y, target) : null;
  const annotItems: MenuEntry[] = annot
    ? [
        {
          id: "replyAnnot",
          labelKey: "annot.thread.reply",
          disabled: !!annotBlock,
          hintKey: annotBlock ?? undefined,
          onSelect: () => {
            if (useAppStore.getState().mode === "read") useAppStore.getState().setMode("annotate");
            openThread(page, annot.id, true);
          },
        },
        {
          id: "deleteAnnot",
          labelKey: "common.delete",
          danger: true,
          disabled: !!annotBlock,
          hintKey: annotBlock ?? undefined,
          onSelect: () => void deleteAnnotations(page, [annot.id]),
        },
        { id: "sepAnnot", separator: true },
      ]
    : [];

  const shared: MenuEntry[] = [
    { id: "goTo", labelKey: "pages.goTo", onSelect: () => view.goToPage(page) },
    { id: "sep1", separator: true },
    { id: "rotateLeft", labelKey: "pages.rotateLeft", onSelect: () => run([{ kind: "rotate", pages: [page], delta: 270 }]) },
    { id: "rotateRight", labelKey: "pages.rotateRight", onSelect: () => run([{ kind: "rotate", pages: [page], delta: 90 }]) },
  ];

  const editing: MenuEntry[] = [
    { id: "duplicate", labelKey: "pages.duplicate", onSelect: () => run([{ kind: "duplicate", pages: [page] }]) },
    {
      id: "extract",
      labelKey: "pages.extract",
      onSelect: () => openDialog("extract", { pages: [page] }),
    },
    {
      id: "insertAfter",
      labelKey: "pages.insertAfter",
      onSelect: () => void import("../dialogs/flows").then((m) => m.insertFromFileFlow(page + 1)),
    },
    { id: "crop", labelKey: "pages.crop", onSelect: () => openDialog("crop", { pages: [page] }) },
    { id: "resize", labelKey: "pages.resize", onSelect: () => openDialog("resize", { pages: [page] }) },
    { id: "sep2", separator: true },
    {
      id: "exportImage",
      labelKey: "pages.exportImage",
      onSelect: () => {
        usePagesStore.getState().setSelected([page]);
        openDialog("export");
      },
    },
    {
      id: "delete",
      labelKey: "pages.delete",
      danger: true,
      disabled: info.pageCount <= 1,
      onSelect: () => run([{ kind: "delete", pages: [page] }]),
    },
  ];

  const canvasOnly: MenuEntry[] = [
    {
      id: "readPage",
      labelKey: "menu.view.readAloud",
      onSelect: () => void import("../tts/speak").then((m) => m.readPageAloud(page)),
    },
    { id: "sep2", separator: true },
    {
      id: "exportImage",
      labelKey: "pages.exportImage",
      onSelect: () => {
        usePagesStore.getState().setSelected([page]);
        openDialog("export");
      },
    },
    {
      id: "organize",
      labelKey: "pages.title",
      disabled: !!pagesBlock,
      hintKey: pagesBlock ?? undefined,
      onSelect: () => {
        const pending = app.mode === "edit" ? editLeaveGuard() : null;
        if (!pending) return app.setMode("pages");
        void pending.then((ok) => ok && useAppStore.getState().setMode("pages"));
      },
    },
  ];

  // UI_SPEC §12 text selection: 복사 · 형광펜 · 밑줄 · 취소선 · 메모 추가 · 영역 표시로 표시 · 검색
  const selection = useSelectionStore.getState().selection;
  const hasText = source === "canvas" && selection?.docId === info.docId && !isEmptySelection(selection);
  const selectedText = hasText ? currentSelectionText() : "";
  const textItems: MenuEntry[] = hasText
    ? [
        {
          id: "copyText",
          labelKey: "menu.edit.copy",
          shortcut: shortcutFor("edit.copy", app.os),
          disabled: !selectedText,
          onSelect: () => void copyToClipboard(selectedText),
        },
        { id: "sepText1", separator: true },
        { id: "highlightSelection", labelKey: "tool.highlight", disabled: !!annotBlock, hintKey: annotBlock ?? undefined, onSelect: () => textMenu((m) => m.markupSelection("highlight")) },
        { id: "underlineSelection", labelKey: "tool.underline", disabled: !!annotBlock, hintKey: annotBlock ?? undefined, onSelect: () => textMenu((m) => m.markupSelection("underline")) },
        { id: "strikeoutSelection", labelKey: "tool.strikeout", disabled: !!annotBlock, hintKey: annotBlock ?? undefined, onSelect: () => textMenu((m) => m.markupSelection("strikeout")) },
        { id: "noteSelection", labelKey: "textMenu.addNote", disabled: !!annotBlock, hintKey: annotBlock ?? undefined, onSelect: () => textMenu((m) => m.noteOnSelection()) },
        {
          id: "redactSelection",
          labelKey: "redact.markSelection",
          disabled: !!redactBlock,
          hintKey: redactBlock ?? undefined,
          onSelect: () => void import("../edit/redact").then((m) => m.markTextSelection()),
        },
        { id: "sepText2", separator: true },
        {
          id: "searchSelection",
          labelKey: "textMenu.search",
          disabled: !selectedText.trim(),
          onSelect: () => textMenu((m) => m.searchSelectedText(selectedText)),
        },
        {
          id: "readSelection",
          labelKey: "tts.readSelection",
          disabled: !selectedText.trim(),
          onSelect: () => void import("../tts/speak").then((m) => m.speakText(selectedText, "selection")),
        },
        { id: "sep0", separator: true },
      ]
    : [];

  openContextMenu({
    x,
    y,
    labelKey: source === "thumbnail" ? "sidebar.tab.thumbnails" : "a11y.canvas",
    items: source === "thumbnail" ? [...shared, ...editing] : [...annotItems, ...textItems, ...shared, ...canvasOnly],
  });
}
