/**
 * 편집 mode state (Stage 7): the page objects of every page the layer has listed, the object
 * selection, and the one open text session (문단 편집 or 텍스트 추가).
 *
 * Object ids are valid only within one `docGeneration` (IPC_CONTRACT §7.4), so each page entry
 * carries the generation it was listed at, and a page re-listed at a newer generation that we did
 * not cause ourselves drops the selection on it.
 */
import { create } from "zustand";
import type { DocId, ObjectId, ObjectsResult, PageIndex, ParagraphAlign, ParagraphProbe, Point, Rgb } from "../ipc/types";

export type EditSession =
  | {
      kind: "paragraph";
      page: PageIndex;
      probe: ParagraphProbe;
      /** box width in points (the right-edge handle) */
      width: number;
      fontSizePt: number;
      color: Rgb;
      align: ParagraphAlign;
      /** the user already agreed to the bundled Hangul font for this session */
      consented: boolean;
    }
  | {
      kind: "addText";
      page: PageIndex;
      /** top-left of the new text, page points */
      at: Point;
      /** 0 = size to the typed text */
      width: number;
      fontSizePt: number;
      color: Rgb;
      align: ParagraphAlign;
    };

export interface EditSelection {
  page: PageIndex;
  ids: ObjectId[];
}

interface EditState {
  docId: DocId | null;
  pages: Record<PageIndex, ObjectsResult>;
  selection: EditSelection | null;
  session: EditSession | null;

  bind(docId: DocId | null): void;
  /** store a page list; `keepSelection` for results of our own transforms (ids unchanged) */
  setPage(page: PageIndex, result: ObjectsResult, keepSelection?: boolean): void;
  select(page: PageIndex, ids: ObjectId[]): void;
  clearSelection(): void;
  openSession(session: EditSession): void;
  patchSession(patch: Partial<EditSession>): void;
  closeSession(): void;
  reset(): void;
}

export const useEditStore = create<EditState>((set, get) => ({
  docId: null,
  pages: {},
  selection: null,
  session: null,

  bind(docId) {
    if (get().docId === docId) return;
    set({ docId, pages: {}, selection: null, session: null });
  },

  setPage(page, result, keepSelection = false) {
    const previous = get().pages[page];
    if (previous && previous.docGeneration > result.docGeneration) return; // a late, older reply
    const sel = get().selection;
    const moved = !previous || previous.docGeneration !== result.docGeneration;
    const dropSelection = sel?.page === page && moved && !keepSelection;
    set({
      pages: { ...get().pages, [page]: result },
      ...(dropSelection ? { selection: null } : null),
    });
  },

  select(page, ids) {
    set({ selection: ids.length ? { page, ids: [...new Set(ids)] } : null });
  },

  clearSelection() {
    if (get().selection) set({ selection: null });
  },

  openSession(session) {
    set({ session, selection: null });
  },

  patchSession(patch) {
    const s = get().session;
    if (!s) return;
    set({ session: { ...s, ...patch } as EditSession });
  },

  closeSession() {
    if (get().session) set({ session: null });
  },

  reset() {
    set({ docId: null, pages: {}, selection: null, session: null });
  },
}));
