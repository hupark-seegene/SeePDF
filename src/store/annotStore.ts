/**
 * Annotations of the open document, per page, plus the tool style defaults.
 * Owner: Stage 1 (d) — see `docs/STAGE1D_NOTES.md`.
 *
 * Two lists live here and they mean different things:
 *
 * * `byPage` — what `list_annotations` returned for a page at `pageGeneration[page]`. These are
 *   already painted by the engine into the page bitmap, so the SVG overlay draws them as
 *   *transparent hit shapes* and only paints an outline when they are hovered or selected.
 * * `ghosts` — optimistic annotations: created locally, not yet in any bitmap. They are painted in
 *   full. A ghost is dropped when the page has been re-rendered at a generation that contains it
 *   (`pageRendered`), which is the `onload` of the new tile / placeholder (ARCHITECTURE §10).
 *
 * Nothing per-frame lives here: the in-flight ink points, the drag offset and the pointer state
 * are refs inside `src/tools/**` and `src/annot/**` (ARCHITECTURE §10).
 */
import { create } from "zustand";
import type { Annot, AnnotId, AnnotKind, DocGeneration, PageIndex, Rgb } from "../ipc/types";
import { BASE_STYLE, baseStyleFor, isStyledTool, styleFor } from "./toolStyles";

/** The 8 swatches of UI_SPEC §14.4, with the highlight alpha column. */
export const PALETTE: { key: string; rgb: Rgb; highlightAlpha: number }[] = [
  { key: "color.yellow", rgb: [255, 216, 77], highlightAlpha: 0.4 },
  { key: "color.green", rgb: [123, 214, 74], highlightAlpha: 0.38 },
  { key: "color.blue", rgb: [77, 184, 255], highlightAlpha: 0.38 },
  { key: "color.pink", rgb: [255, 127, 176], highlightAlpha: 0.38 },
  { key: "color.purple", rgb: [176, 123, 255], highlightAlpha: 0.35 },
  { key: "color.orange", rgb: [255, 154, 61], highlightAlpha: 0.38 },
  { key: "color.red", rgb: [245, 83, 61], highlightAlpha: 0.32 },
  { key: "color.black", rgb: [43, 47, 51], highlightAlpha: 1 },
];

/** The markup kinds take the swatch's alpha column; everything else defaults to 1.0. */
export const MARKUP_KINDS: AnnotKind[] = ["highlight", "underline", "strikeout", "squiggly"];

export interface ToolStyle {
  color: Rgb;
  opacity: number;
  width: number;
  fontSize: number;
  fillColor: Rgb | null;
  /** 선 / 화살표: [start head, end head] */
  heads: [boolean, boolean];
  align: "left" | "center" | "right";
  /** 지우개 크기, PDF points */
  eraserSize: number;
}

/** An optimistic annotation: drawn in full until the page bitmap contains it. */
export interface Ghost {
  annot: Annot;
  /** the generation that first contains it; `null` while `create_annotation` is still in flight */
  generation: DocGeneration | null;
}

/** What the inspector and the inline editors are pointed at. */
export interface AnnotTarget {
  page: PageIndex;
  id: AnnotId;
}

/** P2 threads: the thread popover's root, and whether its reply box takes the focus (답글). */
export interface ThreadTarget extends AnnotTarget {
  focusReply: boolean;
}

export interface AnnotState {
  byPage: Record<PageIndex, Annot[]>;
  /** the generation each page's list was produced at — a late reply for an older one is dropped */
  pageGeneration: Record<PageIndex, DocGeneration>;
  selected: AnnotId[];
  hovered: AnnotId | null;
  ghosts: Ghost[];
  /** the style the next annotation is drawn with: the armed tool's (P1-12) */
  style: ToolStyle;
  /** the tool `style` belongs to; a panel edit with nothing selected becomes its default */
  styleTool: string | null;
  /** per-tool overrides of the built-in styles, mirrored into `Settings.toolDefaults` */
  toolDefaults: Record<string, Partial<ToolStyle>>;
  /**
   * Per-page view nonce from `set_annotations_hidden` (P1-12): the viewer puts it in that page's
   * bitmap URLs, so hiding an annotation re-renders the page without a generation bump.
   */
  viewNonce: Record<PageIndex, number>;
  /** 주석 sidebar filter chips; `null` = every kind */
  filter: AnnotKind[] | null;
  /** the note popover / free-text editor target */
  editing: AnnotTarget | null;
  /**
   * P2 threads: the thread shown in a popover — a note's own popover (with `editing` on the same
   * id) or, for any other kind, the thread popover. Cleared with `editing` and when it goes away.
   */
  thread: ThreadTarget | null;
  /** bumped whenever a scan/list pass finishes, so the sidebar can memoise cheaply */
  listNonce: number;

  setPage(page: PageIndex, annots: Annot[], generation?: DocGeneration): void;
  upsert(annot: Annot): void;
  remove(page: PageIndex, ids: AnnotId[]): void;
  select(ids: AnnotId[]): void;
  toggleSelect(id: AnnotId, additive?: boolean): void;
  hover(id: AnnotId | null): void;
  setEditing(target: AnnotTarget | null): void;
  setThread(target: ThreadTarget | null): void;
  setFilter(filter: AnnotKind[] | null): void;

  addGhost(annot: Annot): void;
  settleGhost(id: AnnotId, generation: DocGeneration): void;
  clearGhosts(page?: PageIndex): void;
  clearGhostById(id: AnnotId): void;
  /** the page has been re-rendered at `generation`: every ghost it now contains can go */
  pageRendered(page: PageIndex, generation: DocGeneration): void;

  /** edit the current style; it is remembered as `styleTool`'s default */
  setStyle(patch: Partial<ToolStyle>): void;
  /** a tool was armed: its remembered style becomes the current one */
  activateTool(tool: string): void;
  /** `Settings.toolDefaults`, already validated (`toolStyles.readToolDefaults`) */
  loadToolDefaults(overrides: Record<string, Partial<ToolStyle>>): void;
  /** 이 스타일을 기본값으로 on a selection: set `tool`'s default without arming it */
  setToolDefault(tool: string, patch: Partial<ToolStyle>): void;
  /** 기본값으로 재설정: forget `tool`'s override */
  resetToolDefault(tool: string): void;
  setViewNonce(page: PageIndex, nonce: number | null): void;
  /** every annotation of the document, page order then document order */
  all(): Annot[];
  find(id: AnnotId): { page: PageIndex; annot: Annot } | null;
  reset(): void;
}

const INITIAL_STYLE: ToolStyle = BASE_STYLE;

export const useAnnotStore = create<AnnotState>((set, get) => ({
  byPage: {},
  pageGeneration: {},
  selected: [],
  hovered: null,
  ghosts: [],
  style: INITIAL_STYLE,
  styleTool: null,
  toolDefaults: {},
  viewNonce: {},
  filter: null,
  editing: null,
  thread: null,
  listNonce: 0,

  setPage(page, annots, generation) {
    const s = get();
    const known = s.pageGeneration[page];
    if (generation !== undefined && known !== undefined && generation < known) return; // stale reply
    const gone = (s.byPage[page] ?? []).filter((a) => !annots.some((n) => n.id === a.id)).map((a) => a.id);
    set({
      byPage: { ...s.byPage, [page]: annots },
      pageGeneration: generation === undefined ? s.pageGeneration : { ...s.pageGeneration, [page]: generation },
      selected: gone.length ? s.selected.filter((id) => !gone.includes(id)) : s.selected,
      editing: s.editing && gone.includes(s.editing.id) ? null : s.editing,
      thread: s.thread && gone.includes(s.thread.id) ? null : s.thread,
      listNonce: s.listNonce + 1,
    });
  },

  upsert(annot) {
    const list = get().byPage[annot.page] ?? [];
    const next = list.some((a) => a.id === annot.id)
      ? list.map((a) => (a.id === annot.id ? annot : a))
      : [...list, annot];
    set((s) => ({ byPage: { ...s.byPage, [annot.page]: next }, listNonce: s.listNonce + 1 }));
  },

  remove(page, ids) {
    const list = (get().byPage[page] ?? []).filter((a) => !ids.includes(a.id));
    set((s) => ({
      byPage: { ...s.byPage, [page]: list },
      selected: s.selected.filter((id) => !ids.includes(id)),
      ghosts: s.ghosts.filter((g) => !ids.includes(g.annot.id)),
      editing: s.editing && ids.includes(s.editing.id) ? null : s.editing,
      thread: s.thread && ids.includes(s.thread.id) ? null : s.thread,
      listNonce: s.listNonce + 1,
    }));
  },

  select(selected) {
    set({ selected });
  },

  toggleSelect(id, additive = false) {
    set((s) => {
      if (!additive) return { selected: s.selected.length === 1 && s.selected[0] === id ? s.selected : [id] };
      return { selected: s.selected.includes(id) ? s.selected.filter((x) => x !== id) : [...s.selected, id] };
    });
  },

  hover(hovered) {
    if (get().hovered !== hovered) set({ hovered });
  },

  setEditing(editing) {
    // the thread of the note being edited stays; any other thread popover closes
    set((s) => ({ editing, thread: editing && s.thread?.id === editing.id ? s.thread : null }));
  },

  setThread(thread) {
    set((s) => ({ thread, editing: thread && s.editing?.id === thread.id ? s.editing : null }));
  },

  setFilter(filter) {
    set({ filter });
  },

  addGhost(annot) {
    set((s) => ({ ghosts: [...s.ghosts.filter((g) => g.annot.id !== annot.id), { annot, generation: null }] }));
  },

  settleGhost(id, generation) {
    set((s) => ({ ghosts: s.ghosts.map((g) => (g.annot.id === id ? { ...g, generation } : g)) }));
  },

  clearGhosts(page) {
    set((s) => ({ ghosts: page === undefined ? [] : s.ghosts.filter((g) => g.annot.page !== page) }));
  },

  clearGhostById(id) {
    set((s) => ({ ghosts: s.ghosts.filter((g) => g.annot.id !== id) }));
  },

  pageRendered(page, generation) {
    const ghosts = get().ghosts;
    const next = ghosts.filter(
      (g) => !(g.annot.page === page && g.generation !== null && generation >= g.generation),
    );
    if (next.length !== ghosts.length) set({ ghosts: next });
  },

  setStyle(patch) {
    set((s) => ({
      style: { ...s.style, ...patch },
      toolDefaults: isStyledTool(s.styleTool)
        ? { ...s.toolDefaults, [s.styleTool]: { ...s.toolDefaults[s.styleTool], ...patch } }
        : s.toolDefaults,
    }));
  },

  activateTool(tool) {
    // 선택 / 손 / 도장 have no style of their own: keep the last one so the panel stays put.
    if (!isStyledTool(tool)) return;
    set((s) => ({ styleTool: tool, style: styleFor(tool, s.toolDefaults) }));
  },

  loadToolDefaults(overrides) {
    set((s) => ({
      toolDefaults: overrides,
      style: isStyledTool(s.styleTool) ? styleFor(s.styleTool, overrides) : s.style,
    }));
  },

  setToolDefault(tool, patch) {
    if (!isStyledTool(tool)) return;
    set((s) => {
      const toolDefaults = { ...s.toolDefaults, [tool]: { ...s.toolDefaults[tool], ...patch } };
      return { toolDefaults, style: s.styleTool === tool ? styleFor(tool, toolDefaults) : s.style };
    });
  },

  resetToolDefault(tool) {
    set((s) => {
      const toolDefaults = { ...s.toolDefaults };
      delete toolDefaults[tool];
      return { toolDefaults, style: s.styleTool === tool ? baseStyleFor(tool) : s.style };
    });
  },

  setViewNonce(page, nonce) {
    set((s) => {
      const viewNonce = { ...s.viewNonce };
      if (nonce === null) delete viewNonce[page];
      else viewNonce[page] = nonce;
      return { viewNonce };
    });
  },

  all() {
    const { byPage } = get();
    return Object.keys(byPage)
      .map(Number)
      .sort((a, b) => a - b)
      .flatMap((page) => byPage[page] ?? []);
  },

  find(id) {
    const { byPage, ghosts } = get();
    for (const key of Object.keys(byPage)) {
      const page = Number(key);
      const annot = (byPage[page] ?? []).find((a) => a.id === id);
      if (annot) return { page, annot };
    }
    const ghost = ghosts.find((g) => g.annot.id === id);
    return ghost ? { page: ghost.annot.page, annot: ghost.annot } : null;
  },

  reset() {
    set({
      byPage: {},
      pageGeneration: {},
      selected: [],
      hovered: null,
      ghosts: [],
      filter: null,
      editing: null,
      thread: null,
      listNonce: 0,
      viewNonce: {},
    });
  },
}));
