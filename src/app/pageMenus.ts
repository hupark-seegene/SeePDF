/**
 * The canvas and 축소판 context menus of UI_SPEC §12.
 *
 * `src/viewer/**` and `src/sidebar/**` belong to module (c), so instead of adding handlers there we
 * read the page index off the DOM contract those modules already publish: `.page-shell[data-page]`
 * (frozen in STAGE1C_NOTES §1.2) and the `.thumb` button of the rail. `App.tsx` installs one
 * document-level `contextmenu` listener that calls in here.
 *
 * v0.3 (V2): the annotation menu is UI_SPEC §12's — 편집 · 속성… · 메모 열기 · 답글 · 복사 · 삭제 ·
 * 이 스타일을 기본값으로 — and the empty page area adds 붙여넣기 (the annotation clipboard at the
 * click point, or the 편집 object clipboard onto the page) · 메모 추가 (a note at the click point) ·
 * 스냅샷 (arms the V1 marquee). The annotation clipboard lives in the lazy annotation host, reached
 * through the `runAnnotCommand` bus (`annot.copy` / `annot.canPaste` / `annot.pasteAt`).
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
import { runAnnotCommand } from "../tools/commands";
import { toolController } from "../tools/ToolController";
import { useAnnotStore, type ToolStyle } from "../store/annotStore";
import { styleFor, toolOfKind } from "../store/toolStyles";
import type { Annot, PageIndex, PageOp, Point } from "../ipc/types";
import { permissionBlock } from "./permissions";
// v0.3 pkg4-annotations-stamps-objects (E1): 이미지 바꾸기 on an image object in 편집 mode
import { useEditStore } from "../edit/editStore";

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
 * A canvas right-click in PDF user space, in the pane it happened in (분할 보기: each pane has its
 * own zoom and rotation), with the page's CSS px per point.
 */
function pagePointUnder(
  page: PageIndex,
  x: number,
  y: number,
  target: EventTarget | null | undefined,
): { at: Point; scale: number } | null {
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
  return { at: ctx.toPage(x - box.left, y - box.top), scale: ctx.scale };
}

/**
 * The annotation under a canvas right-click. Replies are never on the page, so they are never
 * under the pointer.
 */
function annotationUnder(page: PageIndex, x: number, y: number, target: EventTarget | null | undefined): Annot | null {
  const hit = pagePointUnder(page, x, y, target);
  if (!hit) return null;
  return annotAt(annotsOnPage(page), hit.at[0], hit.at[1], GRAB_PX / Math.max(0.01, hit.scale));
}

/** 메모's icon box (tools/note.ts `NOTE_SIZE_PT`; that module is the lazy tools chunk's). */
const NOTE_PT = 22;

/** The note placed by 메모 추가: its top-left at the click, kept on the page. */
export function noteAtPoint(at: Point, crop: { l: number; b: number; r: number; t: number }): Point {
  return [Math.min(Math.max(at[0], crop.l), crop.r - NOTE_PT), Math.min(Math.max(at[1], crop.b + NOTE_PT), crop.t)];
}

/** Annotation items act in 주석: 읽기 / 양식 switch; 편집 asks about its pending marks first. */
function inAnnotate(then: () => void): void {
  const app = useAppStore.getState();
  if (app.mode === "annotate") return then();
  const pending = app.mode === "edit" ? editLeaveGuard() : null;
  if (!pending) {
    app.setMode("annotate");
    return then();
  }
  void pending.then((ok) => {
    if (!ok) return;
    useAppStore.getState().setMode("annotate");
    then();
  });
}

/** 이 스타일을 기본값으로: the annotation's look becomes its tool's default (P1-12). */
export function styleOfAnnot(a: Annot): Partial<ToolStyle> {
  // v0.3 integration (pkg4 A2): the polygon / polyline stroke and the polygon / callout fill too
  const stroked = ["ink", "square", "circle", "line", "arrow", "polygon", "polyline"].includes(a.kind);
  const filled = ["square", "circle", "textbox", "polygon", "callout"].includes(a.kind);
  return {
    color: a.color,
    opacity: a.opacity,
    ...(stroked && a.borderWidth > 0 ? { width: a.borderWidth } : {}),
    ...(filled ? { fillColor: a.fillColor } : {}),
    ...(a.fontSize ? { fontSize: a.fontSize } : {}),
  };
}

/**
 * The 편집 object clipboard lives in the lazy edit chunk; once 편집 has been entered the module is
 * kept here, so the menu can tell (synchronously) whether there is anything to paste. Before that
 * the clipboard cannot hold anything — copying needs the module.
 */
let editActions: typeof import("../edit/actions") | null = null;
useAppStore.subscribe((s) => {
  if (s.mode === "edit" && !editActions) {
    void import("../edit/actions").then((m) => {
      editActions = m;
    });
  }
});

function canPasteObjects(): boolean {
  return useAppStore.getState().mode === "edit" && !!editActions?.hasObjectClipboard();
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
  const editBlock = permissionBlock("mode.edit", info);
  const snapshotBlock = permissionBlock("tool.snapshot", info);

  // UI_SPEC §12 Annotation: 편집 · 속성… · 메모 열기 · 답글 (P2) · 복사 · 삭제 · 이 스타일을 기본값으로
  const annot = source === "canvas" ? annotationUnder(page, x, y, target) : null;
  const annotTool = annot ? toolOfKind(annot.kind) : null;
  const annotItems: MenuEntry[] = annot
    ? [
        {
          id: "editAnnot",
          labelKey: "canvasMenu.edit",
          disabled: annot.locked || annot.editable === "readOnly" || !!annotBlock,
          hintKey: annotBlock ?? undefined,
          onSelect: () =>
            inAnnotate(() => {
              const store = useAnnotStore.getState();
              store.select([annot.id]);
              // a note opens its popover and a text box (v0.3 integration: or a pkg4 callout, which
              // double-click edits the same way) its editor; anything else gets its handles
              if (annot.kind === "note" || annot.kind === "textbox" || annot.kind === "callout") {
                store.setEditing({ page, id: annot.id });
              } else {
                useAppStore.getState().setTool("select");
                toolController.arm("select");
              }
            }),
        },
        {
          id: "propsAnnot",
          labelKey: "canvasMenu.properties",
          disabled: !!annotBlock,
          hintKey: annotBlock ?? undefined,
          onSelect: () =>
            inAnnotate(() => {
              useAnnotStore.getState().select([annot.id]);
              useAppStore.getState().setTool("select");
              toolController.arm("select");
              useAppStore.getState().toggleInspector(true);
            }),
        },
        {
          id: "noteAnnot",
          labelKey: "canvasMenu.openNote",
          disabled: !!annotBlock,
          hintKey: annotBlock ?? undefined,
          onSelect: () =>
            inAnnotate(() => {
              if (annot.kind === "note") useAnnotStore.getState().setEditing({ page, id: annot.id });
              else openThread(page, annot.id, false);
            }),
        },
        {
          id: "replyAnnot",
          labelKey: "annot.thread.reply",
          // v0.3 pkg3 (S5): 답글 switches to 주석, which the document may forbid
          disabled: !!annotBlock,
          hintKey: annotBlock ?? undefined,
          onSelect: () => inAnnotate(() => openThread(page, annot.id, true)),
        },
        { id: "sepAnnot0", separator: true },
        {
          id: "copyAnnot",
          labelKey: "menu.edit.copy",
          onSelect: () => void runAnnotCommand("annot.copy", { page, ids: [annot.id] }),
        },
        {
          id: "deleteAnnot",
          labelKey: "common.delete",
          danger: true,
          disabled: !!annotBlock,
          hintKey: annotBlock ?? undefined,
          onSelect: () => void deleteAnnotations(page, [annot.id]),
        },
        { id: "sepAnnot1", separator: true },
        {
          id: "defaultStyle",
          labelKey: "prop.setDefault",
          disabled: !annotTool,
          onSelect: () => {
            if (annotTool) useAnnotStore.getState().setToolDefault(annotTool, styleOfAnnot(annot));
          },
        },
        { id: "sepAnnot", separator: true },
      ]
    : [];

  // UI_SPEC §12 empty page area: 붙여넣기 · 메모 추가 (at the click point)
  const point = source === "canvas" && !annot ? pagePointUnder(page, x, y, target) : null;
  const canPasteAnnots = !!point && runAnnotCommand("annot.canPaste");
  const canPaste = canPasteAnnots || (!!point && canPasteObjects());
  const pasteBlock = canPasteAnnots ? annotBlock : canPaste ? editBlock : null;
  const emptyItems: MenuEntry[] = point
    ? [
        {
          id: "pasteHere",
          labelKey: "menu.edit.paste",
          shortcut: shortcutFor("edit.paste", app.os),
          // v0.3 pkg3 (S5): annotations need 주석, page objects need 편집
          disabled: !canPaste || !!pasteBlock,
          hintKey: pasteBlock ?? undefined,
          onSelect: () => {
            if (canPasteAnnots) runAnnotCommand("annot.pasteAt", { page, at: point.at });
            else void editActions?.pasteObjects(page);
          },
        },
        {
          id: "noteHere",
          labelKey: "textMenu.addNote",
          disabled: !!annotBlock,
          hintKey: annotBlock ?? undefined,
          onSelect: () =>
            inAnnotate(() => {
              const crop = useDocStore.getState().info?.pages[page]?.crop;
              if (!crop) return;
              const color = styleFor("note", useAnnotStore.getState().toolDefaults).color;
              void import("../annot/actions").then((m) =>
                m.createAnnotation(page, { kind: "note", at: noteAtPoint(point.at, crop), color, contents: "" }, { edit: true }),
              );
            }),
        },
        { id: "sepEmpty", separator: true },
      ]
    : [];

  // v0.3 E1: 편집 mode, an image object under the pointer → 이미지 바꾸기…
  const image = source === "canvas" && app.mode === "edit" ? imageObjectUnder(page, x, y, target) : null;
  const imageItems: MenuEntry[] = image !== null
    ? [
        {
          id: "replaceImage",
          labelKey: "edit.replaceImage",
          // v0.3 integration (E1 × S5): 편집 changes the page, like the 편집 mode it lives in
          disabled: !!editBlock,
          hintKey: editBlock ?? undefined,
          onSelect: () => void import("../edit/actions").then((m) => m.replaceSelectedImage({ page, objectId: image })),
        },
        { id: "sepImage", separator: true },
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
      id: "snapshot",
      labelKey: "tool.snapshot",
      shortcut: shortcutFor("tool.snapshot", app.os),
      // v0.3 integration (S5 × V1): a document that forbids copying forbids snapshots
      disabled: !!snapshotBlock,
      hintKey: snapshotBlock ?? undefined,
      onSelect: () => {
        useAppStore.getState().setTool("snapshot");
        toolController.arm("snapshot");
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
    items:
      source === "thumbnail"
        ? [...shared, ...editing]
        : [...imageItems, ...annotItems, ...textItems, ...emptyItems, ...shared, ...canvasOnly],
  });
}

/** v0.3 E1: the topmost editable image object of 편집 mode under a canvas right-click, or `null`. */
function imageObjectUnder(page: PageIndex, x: number, y: number, target: EventTarget | null | undefined): number | null {
  const info = useDocStore.getState().info;
  const geom = info?.pages[page];
  const shell = (target as HTMLElement | null)?.closest?.<HTMLElement>(".page-shell");
  const objects = useEditStore.getState().pages[page]?.objects;
  if (!info || !geom || !shell || !objects?.length) return null;
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
  for (let i = objects.length - 1; i >= 0; i--) {
    const o = objects[i];
    if (o.type !== "image" || o.editable === "readOnly") continue;
    if (px >= o.rect.l && px <= o.rect.r && py >= o.rect.b && py <= o.rect.t) return o.objectId;
  }
  return null;
}
