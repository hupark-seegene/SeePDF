/**
 * 편집 mode state (Stage 7): the page objects of every page the layer has listed, the object
 * selection, and the one open text session (문단 편집 or 텍스트 추가).
 *
 * Object ids are valid only within one `docGeneration` (IPC_CONTRACT §7.4), so each page entry
 * carries the generation it was listed at, and a page re-listed at a newer generation that we did
 * not cause ourselves drops the selection on it.
 */
import { create } from "zustand";
import type {
  DocId, ObjectId, ObjectsResult, PageIndex, ParagraphAlign, ParagraphProbe, Point, Rect, RedactPreview, Rgb,
} from "../ipc/types";

export type EditSession = (
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
      /**
       * Where to look for the paragraph again when the document changes under the editor: points
       * inside its first line and that line's text, from the page listing the probe ran on.
       */
      firstLine?: { at: Point[]; texts: string[] };
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
    }
) & {
  /** set by `openSession`: which editor this is — stable across patches (a re-found probe keeps the typed text) */
  serial?: number;
};

export interface EditSelection {
  page: PageIndex;
  ids: ObjectId[];
}

/** One pending 영역 표시 mark (F-22): a page rect in points, y-up. Nothing is written until 적용. */
export interface RedactMark {
  id: number;
  page: PageIndex;
  rect: Rect;
}

export type RedactPreviewState =
  | { status: "loading" }
  | { status: "ready"; result: RedactPreview }
  // v0.3 (pkg1): `refused` = the engine's dry run of the apply said `verifyFailed` — applying would
  // lose text outside the marks, so it would be refused too.
  | { status: "error"; refused?: boolean };

export const DEFAULT_REDACT_FILL: Rgb = [0, 0, 0];

interface EditState {
  docId: DocId | null;
  pages: Record<PageIndex, ObjectsResult>;
  selection: EditSelection | null;
  session: EditSession | null;
  /** 영역 표시: pending marks (every page), the selected mark, the per-page `redact_preview` */
  marks: RedactMark[];
  markSel: number | null;
  previews: Record<PageIndex, RedactPreviewState>;
  redactFill: Rgb;
  redactOverlay: string;

  bind(docId: DocId | null): void;
  /** store a page list; `keepSelection` for results of our own transforms (ids unchanged) */
  setPage(page: PageIndex, result: ObjectsResult, keepSelection?: boolean): void;
  select(page: PageIndex, ids: ObjectId[]): void;
  clearSelection(): void;
  openSession(session: EditSession): void;
  patchSession(patch: Partial<EditSession>): void;
  closeSession(): void;
  addMarks(page: PageIndex, rects: Rect[]): number[];
  removeMark(id: number): void;
  /** drop these marks; a page left with none loses its preview too */
  removeMarks(ids: number[]): void;
  /** drop the marks (and previews) of `pages`, or of every page */
  clearMarks(pages?: PageIndex[]): void;
  /** move these marks by `dy` points (Stage 9: the content under them moved with a paragraph edit) */
  moveMarks(ids: number[], dy: number): void;
  selectMark(id: number | null): void;
  setPreview(page: PageIndex, state: RedactPreviewState | null): void;
  setRedactOptions(patch: { fill?: Rgb; overlay?: string }): void;
  reset(): void;
}

let nextMark = 1;
let nextSession = 1;
const NO_MARKS = { marks: [] as RedactMark[], markSel: null, previews: {} };

export const useEditStore = create<EditState>((set, get) => ({
  docId: null,
  pages: {},
  selection: null,
  session: null,
  ...NO_MARKS,
  redactFill: DEFAULT_REDACT_FILL,
  redactOverlay: "",

  bind(docId) {
    if (get().docId === docId) return;
    set({ docId, pages: {}, selection: null, session: null, ...NO_MARKS });
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
    set({ selection: ids.length ? { page, ids: [...new Set(ids)] } : null, markSel: null });
  },

  clearSelection() {
    if (get().selection) set({ selection: null });
  },

  openSession(session) {
    set({ session: { ...session, serial: nextSession++ }, selection: null });
  },

  patchSession(patch) {
    const s = get().session;
    if (!s) return;
    set({ session: { ...s, ...patch } as EditSession });
  },

  closeSession() {
    if (get().session) set({ session: null });
  },

  addMarks(page, rects) {
    const added = rects
      .filter((r) => r.r - r.l > 0.5 && r.t - r.b > 0.5)
      .map((rect) => ({ id: nextMark++, page, rect: { ...rect } }));
    if (added.length) set({ marks: [...get().marks, ...added] });
    return added.map((m) => m.id);
  },

  removeMark(id) {
    const marks = get().marks.filter((m) => m.id !== id);
    if (marks.length === get().marks.length) return;
    set({ marks, ...(get().markSel === id ? { markSel: null } : null) });
  },

  removeMarks(ids) {
    const drop = new Set(ids);
    const { marks, previews, markSel } = get();
    const kept = marks.filter((m) => !drop.has(m.id));
    if (kept.length === marks.length) return;
    const nextPreviews = { ...previews };
    for (const m of marks) if (drop.has(m.id) && !kept.some((k) => k.page === m.page)) delete nextPreviews[m.page];
    set({ marks: kept, previews: nextPreviews, markSel: markSel !== null && drop.has(markSel) ? null : markSel });
  },

  clearMarks(pages) {
    const { marks, previews, markSel } = get();
    if (!pages) {
      if (marks.length || Object.keys(previews).length) set({ ...NO_MARKS });
      return;
    }
    const drop = new Set(pages);
    const kept = marks.filter((m) => !drop.has(m.page));
    const nextPreviews = { ...previews };
    for (const p of pages) delete nextPreviews[p];
    set({
      marks: kept,
      previews: nextPreviews,
      markSel: kept.some((m) => m.id === markSel) ? markSel : null,
    });
  },

  moveMarks(ids, dy) {
    const move = new Set(ids);
    if (!move.size || dy === 0) return;
    set({
      marks: get().marks.map((m) => (move.has(m.id) ? { ...m, rect: { ...m.rect, b: m.rect.b + dy, t: m.rect.t + dy } } : m)),
    });
  },

  selectMark(id) {
    if (get().markSel === id) return;
    set({ markSel: id, ...(id !== null ? { selection: null } : null) });
  },

  setPreview(page, state) {
    const previews = { ...get().previews };
    if (state) previews[page] = state;
    else delete previews[page];
    set({ previews });
  },

  setRedactOptions(patch) {
    set({
      ...(patch.fill ? { redactFill: patch.fill } : null),
      ...(patch.overlay !== undefined ? { redactOverlay: patch.overlay } : null),
    });
  },

  reset() {
    set({ docId: null, pages: {}, selection: null, session: null, ...NO_MARKS, redactFill: DEFAULT_REDACT_FILL, redactOverlay: "" });
  },
}));
