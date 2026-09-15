/**
 * One dispatcher for every command id: the keymap, the native menu (`menu:<id>`) and every toolbar
 * button call exactly this. Ids that belong to a Stage 1 module are logged as `todo` until their
 * owner lands — that list is the integration checklist in STAGE0_FRONTEND_NOTES.md.
 */
import { useCallback } from "react";
import * as api from "../ipc/api";
import { useAppStore, type Mode, type ToolId } from "../store/appStore";
import { useDocStore } from "../store/docStore";
import { useViewStore } from "../store/viewStore";
import { useJobStore } from "../store/jobStore";
import { usePagesStore } from "../store/pagesStore";
import { toolController } from "../tools/ToolController";

export type CommandId = string;

const MODE_OF: Record<string, Mode> = {
  "mode.read": "read",
  "mode.annotate": "annotate",
  "mode.edit": "edit",
  "mode.pages": "pages",
  "mode.form": "form",
};

export function useCommands(): (id: CommandId, opts?: { momentary?: boolean }) => void {
  return useCallback((id: CommandId, opts?: { momentary?: boolean }) => {
    const app = useAppStore.getState();
    const docs = useDocStore.getState();
    const view = useViewStore.getState();
    const jobs = useJobStore.getState();
    const pages = usePagesStore.getState();
    const info = docs.info;

    if (id in MODE_OF) return app.setMode(MODE_OF[id]);

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
        void api.openFileDialog().then((picked) => {
          if (picked?.length) void docs.open(picked[0]);
        });
        return;
      case "file.close":
        void docs.close();
        return;
      case "file.save": {
        if (!info) return;
        const labelKey = "status.saving";
        void api
          .saveDocument({ docId: info.docId }, (e) => jobs.apply("save", labelKey, e))
          .then(() => docs.refresh())
          .catch(() => undefined);
        return;
      }
      case "file.saveAs": {
        if (!info) return;
        void api.saveFileDialog({ defaultPath: info.name }).then((path) => {
          if (!path) return;
          return api
            .saveDocumentAs({ docId: info.docId, path }, (e) => jobs.apply("save", "status.saving", e))
            .then(() => docs.refresh());
        });
        return;
      }
      case "file.reveal":
        if (info?.path) void api.revealInFileManager({ path: info.path });
        return;
      case "file.copyPath":
        if (info?.path && typeof navigator !== "undefined" && navigator.clipboard) {
          void navigator.clipboard.writeText(info.path).catch(() => undefined);
        }
        return;

      // Edit -----------------------------------------------------------------
      case "edit.undo":
        void docs.undo();
        return;
      case "edit.redo":
        void docs.redo();
        return;
      case "edit.find":
        app.setSidebarTab("search");
        return;
      case "edit.selectAll":
        if (app.mode === "pages" && info) pages.selectAll(info.pageCount);
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
        view.rotate(-90);
        return;
      case "view.rotateRight":
        view.rotate(90);
        return;
      case "view.night":
        view.setNight(view.night === "off" ? "dark" : "off");
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

      // 페이지 mode ----------------------------------------------------------
      case "pages.rotateLeft":
      case "pages.rotateRight": {
        if (!info || pages.selected.length === 0) return;
        const delta = id === "pages.rotateLeft" ? 270 : 90;
        void api
          .pageOps({ docId: info.docId, ops: [{ kind: "rotate", pages: pages.selected, delta }] })
          .then(() => docs.refresh());
        return;
      }
      case "pages.delete": {
        if (!info || pages.selected.length === 0) return;
        void api
          .pageOps({ docId: info.docId, ops: [{ kind: "delete", pages: pages.selected }] })
          .then(() => {
            pages.clear();
            return docs.refresh();
          });
        return;
      }
      case "pages.selectAll":
        if (info) pages.selectAll(info.pageCount);
        return;

      default:
        if (import.meta.env.DEV) console.info(`[command] ${id} — not implemented in Stage 0`);
    }
  }, []);
}
