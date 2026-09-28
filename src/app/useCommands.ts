/**
 * One dispatcher for every command id: the keymap, the native menu (`menu:<id>`) and every toolbar
 * button call exactly this.
 *
 * Stage 1 (e) filled in the file / export / print / settings / 페이지 ids. Everything that needs
 * more than one command lives in `src/dialogs/flows.ts` and is reached with a dynamic import, so
 * the dispatcher itself stays in the entry chunk while the flows and dialogs do not.
 * Annotation, object and undo ids stay with (c)/(d).
 */
import { useCallback } from "react";
import * as api from "../ipc/api";
import { useAppStore, type Mode, type ToolId } from "../store/appStore";
import { useDocStore } from "../store/docStore";
import { useViewStore } from "../store/viewStore";
import { usePagesStore } from "../store/pagesStore";
import { openDialog } from "../dialogs/dialogState";
import { openContextMenu, type MenuEntry } from "./contextMenuStore";
import { toolController } from "../tools/ToolController";
import { clearTextSelection, findStep, selectAllOnCurrentPage } from "../viewer/viewerCommands";
import { editLeaveGuard, runAnnotCommand } from "../tools/commands";
import { toggleFullScreen, toggleReadingMode } from "./readingMode";
import type { PageOp } from "../ipc/types";

export type CommandId = string;

const MODE_OF: Record<string, Mode> = {
  "mode.read": "read",
  "mode.annotate": "annotate",
  "mode.edit": "edit",
  "mode.pages": "pages",
  "mode.form": "form",
};

/** The ids 주석 mode may claim before the shell's own handling (see `src/tools/commands.ts`). */
const ANNOT_IDS = new Set(["edit.delete", "edit.duplicate", "edit.copy", "edit.cut", "edit.paste"]);

/** `import("../dialogs/flows")` — kept in one place so every call site is obviously code-split. */
const flows = () => import("../dialogs/flows");

function runPages(ops: PageOp[]): void {
  void flows().then((m) => m.runPageOps(ops));
}

export function useCommands(): (id: CommandId, opts?: { momentary?: boolean }) => void {
  return useCallback((id: CommandId, opts?: { momentary?: boolean }) => {
    const app = useAppStore.getState();
    const docs = useDocStore.getState();
    const view = useViewStore.getState();
    const pages = usePagesStore.getState();
    const info = docs.info;
    const target = pages.selected.length ? pages.selected : pages.focus !== null ? [pages.focus] : [];

    if (id in MODE_OF) {
      const next = MODE_OF[id];
      // 편집 mode may hold pending 영역 표시 marks: leaving asks first (F-22)
      const pending = app.mode === "edit" && next !== "edit" ? editLeaveGuard() : null;
      if (!pending) return app.setMode(next);
      void pending.then((ok) => ok && useAppStore.getState().setMode(next));
      return;
    }

    // (d): 주석 mode claims ⌫ / ⌘D / ⌘C / ⌘X / ⌘V while an annotation is selected. The bus answers
    // `false` when the annotation chunk is not loaded or nothing is selected, and the switch below
    // keeps doing whatever it did before (페이지 mode owns the same ids).
    if (ANNOT_IDS.has(id) && runAnnotCommand(id)) return;

    if (id.startsWith("tool.")) {
      const tool = id.slice("tool.".length) as ToolId;
      if (tool === ("none" as ToolId)) {
        app.setTool("select");
        toolController.arm("select");
        return;
      }
      app.setTool(tool, opts?.momentary);
      toolController.arm(tool, opts?.momentary);
      return;
    }

    switch (id) {
      // File -----------------------------------------------------------------
      case "file.open":
        void flows().then((m) => m.openFileFlow());
        return;
      case "file.openRecent":
        openRecentMenu();
        return;
      case "file.close":
        void flows().then((m) => m.closeDocumentFlow());
        return;
      case "file.save":
        void flows().then((m) => m.saveFlow());
        return;
      case "file.saveAs":
        void flows().then((m) => m.saveAsFlow());
        return;
      case "file.export":
        if (info) openDialog("export");
        return;
      case "file.print":
        if (info) openDialog("print");
        return;
      case "file.docInfo":
        if (info) openDialog("docInfo");
        return;
      case "tools.security":
        if (info) openDialog("security");
        return;
      case "tools.stamp":
        if (info) openDialog("stamp");
        return;
      case "tools.compress":
        if (info) openDialog("compress");
        return;
      case "tools.compare":
        if (info) openDialog("compare");
        return;
      // P2: 페이지 레이블… (페이지 mode rail, 문서 정보)
      case "pages.labels":
        if (info) openDialog("pageLabels");
        return;
      case "file.reveal":
        if (info?.path) void api.revealInFileManager({ path: info.path });
        return;
      case "file.copyPath":
        if (info?.path && typeof navigator !== "undefined" && navigator.clipboard) {
          void navigator.clipboard.writeText(info.path).catch(() => undefined);
        }
        return;

      // App ------------------------------------------------------------------
      case "app.settings":
        openDialog("settings");
        return;
      case "app.overflow":
        openOverflowMenu();
        return;
      case "tools.ocr":
        void import("../ocr").then((m) =>
          m.openOcrDialog({ selectedPages: pages.selected.length ? pages.selected : undefined }),
        );
        return;
      case "tools.batchOcr":
        openDialog("batchOcr");
        return;
      case "tools.merge":
        openDialog("merge");
        return;
      case "tools.split":
        if (info) openDialog("split");
        return;

      // Edit -----------------------------------------------------------------
      // A coalesced annotation patch must reach the engine before the snapshot is popped, or undo
      // would skip an edit the user has already seen (IPC_CONTRACT §7.8).
      case "edit.undo":
        void import("../annot/sync").then((m) => m.undoWithAnnots());
        return;
      case "edit.redo":
        void import("../annot/sync").then((m) => m.redoWithAnnots());
        return;
      case "edit.find":
        app.setSidebarTab("search");
        return;
      // The native Edit menu owns ⌘G / ⇧⌘G / ⌘A and emits `menu:<id>`, so these three are
      // dispatched here and nowhere else (the duplicate listener inside the viewer is gone,
      // STAGE1C_NOTES §7.2). The viewer exports the three primitives.
      case "edit.findNext":
        findStep(1);
        return;
      case "edit.findPrevious":
        findStep(-1);
        return;
      case "edit.deselect":
        clearTextSelection();
        return;
      case "edit.selectAll": {
        // An input / textarea / contenteditable selects its own value first: Select All is a
        // custom menu item now, so WebKit no longer does this for us.
        const el = typeof document === "undefined" ? null : (document.activeElement as HTMLElement | null);
        const tag = el?.tagName?.toLowerCase();
        if (tag === "input" || tag === "textarea") {
          (el as HTMLInputElement).select();
          return;
        }
        if (el?.isContentEditable) {
          const range = document.createRange();
          range.selectNodeContents(el);
          const selection = document.getSelection();
          selection?.removeAllRanges();
          selection?.addRange(range);
          return;
        }
        if (app.mode === "pages" && info) return pages.selectAll(info.pageCount);
        selectAllOnCurrentPage();
        return;
      }
      case "edit.delete":
        if (app.mode === "pages" && info && target.length) {
          pages.clear();
          runPages([{ kind: "delete", pages: target }]);
        }
        return;
      case "edit.duplicate":
        if (app.mode === "pages" && info && target.length) runPages([{ kind: "duplicate", pages: target }]);
        return;

      // View -----------------------------------------------------------------
      case "view.sidebar":
        app.toggleSidebar();
        return;
      case "view.inspector":
        app.toggleInspector();
        return;
      case "view.sidebar.thumbnails":
        app.setSidebarTab("thumbnails");
        return;
      case "view.sidebar.outline":
        app.setSidebarTab("outline");
        return;
      case "view.sidebar.annotations":
        app.setSidebarTab("annotations");
        return;
      case "view.sidebar.search":
        app.setSidebarTab("search");
        return;
      case "view.zoomIn":
        view.zoomIn();
        return;
      case "view.zoomOut":
        view.zoomOut();
        return;
      case "view.actualSize":
        view.setZoomMode("actual", 100);
        return;
      case "view.fitPage":
        view.setZoomMode("fit-page");
        return;
      case "view.fitWidth":
        view.setZoomMode("fit-width");
        return;
      case "view.layout.single":
        view.setLayout("single");
        return;
      case "view.layout.continuous":
        view.setLayout("continuous");
        return;
      case "view.layout.twoPage":
        view.setLayout("two");
        return;
      case "view.rotateLeft":
        if (app.mode === "pages" && target.length) return runPages([{ kind: "rotate", pages: target, delta: 270 }]);
        view.rotate(-90);
        return;
      case "view.rotateRight":
        if (app.mode === "pages" && target.length) return runPages([{ kind: "rotate", pages: target, delta: 90 }]);
        view.rotate(90);
        return;
      case "view.night":
        view.cycleNight();
        return;
      // 읽기 모드 / 전체 화면 (P1-12): the native View menu, ⌃⌘R / F8 and ⌃⌘F / F11
      case "view.readingMode":
        if (info) toggleReadingMode();
        return;
      case "view.fullScreen":
        void toggleFullScreen();
        return;

      // Navigation -----------------------------------------------------------
      case "go.nextPage":
        if (info) view.goToPage(Math.min(info.pageCount - 1, view.currentPage + 1));
        return;
      case "go.previousPage":
        view.goToPage(Math.max(0, view.currentPage - 1));
        return;
      case "go.firstPage":
        view.goToPage(0);
        return;
      case "go.lastPage":
        if (info) view.goToPage(info.pageCount - 1);
        return;
      case "go.goToPage": {
        const field = document.querySelector<HTMLInputElement>(".page-input");
        field?.focus();
        field?.select();
        return;
      }

      // 페이지 mode ----------------------------------------------------------
      case "pages.rotateLeft":
        if (target.length) runPages([{ kind: "rotate", pages: target, delta: 270 }]);
        return;
      case "pages.rotateRight":
        if (target.length) runPages([{ kind: "rotate", pages: target, delta: 90 }]);
        return;
      case "pages.delete":
        if (target.length) {
          pages.clear();
          runPages([{ kind: "delete", pages: target }]);
        }
        return;
      case "pages.duplicate":
        if (target.length) runPages([{ kind: "duplicate", pages: target }]);
        return;
      case "pages.reverse":
        if (info) runPages([{ kind: "reverse" }]);
        return;
      case "pages.extract":
        if (target.length) openDialog("extract", { pages: target });
        return;
      case "pages.insert":
        if (info) {
          const at = target.length ? Math.max(...target) + 1 : info.pageCount;
          runPages([{ kind: "insertBlank", at, size: "sameAs" }]);
        }
        return;
      case "pages.insertFrom":
        if (info) {
          const at = target.length ? Math.max(...target) + 1 : info.pageCount;
          void flows().then((m) => m.insertFromFileFlow(at));
        }
        return;
      case "pages.selectAll":
        if (info) pages.selectAll(info.pageCount);
        return;

      default:
        if (import.meta.env.DEV) console.info(`[command] ${id} — not implemented`);
    }
  }, []);
}

/** ⇧⌘O — the recents list as a menu, plus 메뉴 지우기 (UI_SPEC §2). */
function openRecentMenu(): void {
  const { recents } = useAppStore.getState();
  const items: MenuEntry[] = recents.slice(0, 10).map((entry) => ({
    id: entry.path,
    labelKey: "welcome.recent",
    label: entry.name,
    onSelect: () => void import("../dialogs/flows").then((m) => m.openPath(entry.path)),
  }));
  if (items.length) items.push({ id: "sep", separator: true });
  items.push({
    id: "clear",
    labelKey: "menu.file.clearRecent",
    onSelect: () => void api.clearRecent().then(() => useAppStore.getState().refreshRecents()),
  });
  openContextMenu({ x: 96, y: 52, labelKey: "menu.file.openRecent", items });
}

/** ⋯ — 인쇄, 보안, 워터마크, 압축, 문서 비교, OCR, 여러 파일 OCR, 합치기, 분할, 문서 정보, 설정 (UI_SPEC §2). */
function openOverflowMenu(): void {
  const info = useDocStore.getState().info;
  openContextMenu({
    x: typeof window === "undefined" ? 0 : window.innerWidth - 16,
    y: 52,
    labelKey: "common.more",
    items: [
      { id: "print", labelKey: "menu.file.print", disabled: !info, onSelect: () => openDialog("print") },
      { id: "security", labelKey: "menu.tools.security", disabled: !info, onSelect: () => openDialog("security") },
      { id: "stamp", labelKey: "menu.tools.stamp", disabled: !info, onSelect: () => openDialog("stamp") },
      { id: "compress", labelKey: "menu.tools.compress", disabled: !info, onSelect: () => openDialog("compress") },
      { id: "compare", labelKey: "menu.tools.compare", disabled: !info, onSelect: () => openDialog("compare") },
      {
        id: "ocr",
        labelKey: "menu.tools.ocr",
        disabled: !info,
        onSelect: () => void import("../ocr").then((m) => m.openOcrDialog()),
      },
      { id: "batchOcr", labelKey: "menu.tools.batchOcr", onSelect: () => openDialog("batchOcr") },
      { id: "merge", labelKey: "menu.tools.merge", onSelect: () => openDialog("merge") },
      { id: "split", labelKey: "pages.split", disabled: !info, onSelect: () => openDialog("split") },
      { id: "sep", separator: true },
      { id: "docInfo", labelKey: "menu.file.docInfo", disabled: !info, onSelect: () => openDialog("docInfo") },
      { id: "settings", labelKey: "menu.settings", onSelect: () => openDialog("settings") },
    ],
  });
}
