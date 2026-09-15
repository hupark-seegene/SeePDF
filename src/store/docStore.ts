/**
 * The open document: `DocInfo`, outline, generation, dirty state, undo/redo.
 * Owner from Stage 1: (c) viewer. Stage 0 keeps it minimal but working against the mock.
 *
 * Invariant: `info.docGeneration` is the cache key for every tile URL and every page-object command.
 * `applyDocChanged` is the single place a new generation enters the frontend.
 */
import { create } from "zustand";
import * as api from "../ipc/api";
import type { DocChangedEvent, DocInfo, OutlineNode, PageIndex } from "../ipc/types";

export type DocStatus = "empty" | "opening" | "ready" | "error";

export interface DocState {
  docId: string | null;
  info: DocInfo | null;
  outline: OutlineNode[];
  status: DocStatus;
  error: { code: string; message: string } | null;
  /** bumped by `doc-changed` so views that cache per page can invalidate cheaply */
  changeNonce: number;
  changedPages: PageIndex[] | "all";

  open(path: string, password?: string): Promise<DocInfo | null>;
  /** Take a `DocInfo` the caller already has (merge, page ops, save) as the open document. */
  adopt(info: DocInfo): void;
  close(): Promise<void>;
  refresh(): Promise<void>;
  applyDocChanged(e: DocChangedEvent): void;
  undo(): Promise<void>;
  redo(): Promise<void>;
}

export const useDocStore = create<DocState>((set, get) => ({
  docId: null,
  info: null,
  outline: [],
  status: "empty",
  error: null,
  changeNonce: 0,
  changedPages: "all",

  async open(path, password) {
    set({ status: "opening", error: null });
    try {
      const info = await api.openDocument({ path, password });
      const outline = info.hasOutline ? await api.getOutline({ docId: info.docId }).catch(() => []) : [];
      set({ docId: info.docId, info, outline, status: "ready", changeNonce: get().changeNonce + 1 });
      return info;
    } catch (e) {
      const err = api.isSeePdfError(e) ? { code: e.code, message: e.message } : { code: "pdfium", message: String(e) };
      set({ status: "error", error: err });
      return null;
    }
  },

  adopt(info) {
    const outline = get().docId === info.docId ? get().outline : [];
    set({
      docId: info.docId,
      info,
      outline,
      status: "ready",
      error: null,
      changedPages: "all",
      changeNonce: get().changeNonce + 1,
    });
    if (info.hasOutline && outline.length === 0) {
      void api
        .getOutline({ docId: info.docId })
        .then((nodes) => {
          if (get().docId === info.docId) set({ outline: nodes });
        })
        .catch(() => undefined);
    }
  },

  async close() {
    const docId = get().docId;
    if (docId) await api.closeDocument({ docId }).catch(() => undefined);
    set({ docId: null, info: null, outline: [], status: "empty", error: null });
  },

  async refresh() {
    const docId = get().docId;
    if (!docId) return;
    const info = await api.getDocument({ docId }).catch(() => null);
    if (info) set({ info });
  },

  applyDocChanged(e) {
    const info = get().info;
    if (!info || info.docId !== e.docId) return;
    // Stage 2: `canUndo`/`canRedo` ride on the event (IPC_CONTRACT §8), so the title bar's
    // ↶ / ↷ stay honest without a `get_document` per edit (STAGE1D_NOTES §7.4). The labels
    // still come from `DocInfo`, and a structural change refreshes anyway.
    set({
      info: {
        ...info,
        docGeneration: e.docGeneration,
        dirty: e.dirty,
        canUndo: e.canUndo ?? info.canUndo,
        canRedo: e.canRedo ?? info.canRedo,
      },
      changedPages: e.changedPages,
      changeNonce: get().changeNonce + 1,
    });
    if (e.structure) void get().refresh();
  },

  async undo() {
    const docId = get().docId;
    if (!docId) return;
    const info = await api.undo({ docId }).catch(() => null);
    if (info) set({ info });
  },

  async redo() {
    const docId = get().docId;
    if (!docId) return;
    const info = await api.redo({ docId }).catch(() => null);
    if (info) set({ info });
  },
}));
