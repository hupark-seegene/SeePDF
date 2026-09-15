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
 */
import { openContextMenu, type MenuEntry } from "./contextMenuStore";
import { openDialog } from "../dialogs/dialogState";
import { useAppStore } from "../store/appStore";
import { useDocStore } from "../store/docStore";
import { useViewStore } from "../store/viewStore";
import { usePagesStore } from "../store/pagesStore";
import type { PageIndex, PageOp } from "../ipc/types";

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

export function openPageContextMenu(page: PageIndex, source: "canvas" | "thumbnail", x: number, y: number): void {
  const info = useDocStore.getState().info;
  if (!info) return;
  const view = useViewStore.getState();
  const app = useAppStore.getState();

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
    { id: "sep2", separator: true },
    {
      id: "exportImage",
      labelKey: "pages.exportImage",
      onSelect: () => {
        usePagesStore.getState().setSelected([page]);
        openDialog("export");
      },
    },
    { id: "organize", labelKey: "pages.title", onSelect: () => app.setMode("pages") },
  ];

  openContextMenu({
    x,
    y,
    labelKey: source === "thumbnail" ? "sidebar.tab.thumbnails" : "a11y.canvas",
    items: source === "thumbnail" ? [...shared, ...editing] : [...shared, ...canvasOnly],
  });
}
