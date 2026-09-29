/**
 * Tool-mode routing (ARCHITECTURE §10, UI_SPEC §6) — the seam the tool strip, the keymap and the
 * canvas surface all talk to. Stage 1 (d) replaces Stage 0's stub and keeps every exported name.
 *
 * Two rules make this file worth its own module:
 *
 * 1. **Every tool is a pure state machine.** `onDown/onMove/onUp(state, point, ctx) → ToolResult`,
 *    no React, no DOM, no IPC — which is why `src/tools/*.test.ts` can drive a whole gesture in
 *    three calls and assert on the `AnnotSpec` that falls out.
 * 2. **This module stays small and eager.** The tool strip and `useCommands` import it on the
 *    critical path, so it must not pull in the tool modules, the overlay or the IPC layer: the
 *    modules register themselves from `src/tools/registry.ts`, which is dynamically imported by
 *    the annotation host (see `docs/STAGE1D_NOTES.md` §6 on the bundle budget).
 *
 * The controller owns no annotation state. It hands what a gesture produced to a **sink** that the
 * annotation host installs (`setSink`), and the sink is what calls `create_annotation` and friends.
 */
import type {
  Annot, AnnotId, AnnotPatch, AnnotSpec, DocGeneration, DocId, PageIndex, Point, Rect, Rgb,
} from "../ipc/types";
import type { ToolId } from "../store/appStore";

/** What a 도장 / 서명 places: a picked file or one of the built-in stamps (IPC_CONTRACT §7.1). */
export type StampImage =
  | { path: string }
  | { builtin: string }
  // v0.3 T2: a text stamp (내 도장 / 오늘 날짜); {{date}} / {{author}} expand at placement
  | { text: string; color: Rgb; shape?: "rect" | "round" | "none" };

/** A point in PDF user space on one page (points, y-up, unrotated — IPC_CONTRACT §3). */
export interface PagePoint {
  page: PageIndex;
  pt: Point;
  /** v0.3 A4: `PointerEvent.pointerType` ("mouse" | "pen" | "touch"); absent in tests = mouse */
  pointerType?: string;
  /** v0.3 A4: `PointerEvent.pressure`, 0..1 (a mouse reports 0.5 while pressed) */
  pressure?: number;
}

export interface ToolModifiers {
  shift: boolean;
  alt: boolean;
  meta: boolean;
  ctrl: boolean;
}

export interface ToolContext {
  docId: DocId;
  docGeneration: DocGeneration;
  /** current style from `annotStore` (colour, opacity, width, font size) */
  style: {
    color: Rgb;
    opacity: number;
    width: number;
    fontSize: number;
    fillColor: Rgb | null;
    heads: [boolean, boolean];
    align: "left" | "center" | "right";
    eraserSize: number;
  };
  modifiers: ToolModifiers;
  /** CSS px per PDF point — hit tolerances are pixel-sized, so they divide by this */
  scale: number;
  /** annotations known on a page (eraser, select); the host feeds it from `annotStore` */
  annots?(page: PageIndex): Annot[];
  /** currently selected ids, so a click inside an existing selection drags the whole set */
  selected?: AnnotId[];
  /** text-selection rectangles on a page, one per line run — the markup tools' quads */
  selectionRects?(page: PageIndex): Rect[];
  /** the image the 도장 / 서명 tool will place, chosen when the tool was armed */
  image?: StampImage | null;
}

/** What the tool wants drawn right now, before anything is committed (optimistic overlay). */
export interface ToolPreview {
  page: PageIndex;
  kind: "rect" | "ellipse" | "ink" | "line" | "marquee" | "quads" | "eraser" | "stamp" | "poly" | "callout";
  rect?: Rect;
  points?: number[];
  quads?: Rect[];
  /** line / arrow heads, [start, end] */
  heads?: [boolean, boolean];
  color?: Rgb;
  fillColor?: Rgb | null;
  opacity?: number;
  width?: number;
  /** ids the gesture is about to act on (eraser hover, marquee hit set) */
  ids?: AnnotId[];
  /** a built-in 도장's label, drawn inside the placement ghost (P1-12) */
  label?: string;
  /** v0.3 T1: the picked image of a 도장 / 서명 ghost (a `blob:` URL), drawn at 50 % */
  imageUrl?: string;
  /** v0.3 T2: a text stamp's border */
  shape?: "rect" | "round" | "none";
  /** v0.3 A2: polygon / polyline being drawn — closed ring or not; a measuring label */
  closed?: boolean;
  cloudy?: boolean;
  measureText?: string;
}

export interface ToolResult<S = unknown> {
  state: S;
  /** `undefined` leaves the preview as it was; `null` clears it */
  preview?: ToolPreview | null;
  /** an annotation to create — the sink runs `create_annotation` and holds the ghost */
  commit?: { page: PageIndex; spec: AnnotSpec };
  /** annotations the gesture removed (지우개) */
  erase?: { page: PageIndex; ids: AnnotId[] };
  /** the gesture's selection result (선택 도구, marquee) */
  select?: { ids: AnnotId[]; additive?: boolean };
  /** changes to existing annotations (move / resize); `live` coalesces while the drag continues */
  patch?: { page: PageIndex; edits: { id: AnnotId; patch: AnnotPatch }[]; live?: boolean };
  /** open an inline editor at this rectangle (텍스트 상자) or annotation (메모) */
  edit?: { page: PageIndex; rect?: Rect; id?: AnnotId; callout?: number[] };
  /** v0.3 A4: several annotations from one gesture (a pen stroke whose pressure varies) */
  commits?: { page: PageIndex; spec: AnnotSpec }[];
  /** v0.3 A3 부분 지우개: what is left of each touched ink annotation (empty paths = delete it) */
  partialErase?: { page: PageIndex; edits: { id: AnnotId; paths: number[][] }[] };
  /** the tool wants to hand control back (Esc, or a one-shot tool that finished) */
  done?: boolean;
}

export interface ToolModule<S = unknown> {
  id: ToolId;
  /** CSS cursor while the tool is armed (UI_SPEC §6) */
  cursor: string;
  /** several ids can share one module (형광펜/밑줄/취소선/물결선, 선/화살표, 도장/서명) */
  init(ctx: ToolContext): S;
  onDown(state: S, p: PagePoint, ctx: ToolContext): ToolResult<S>;
  onMove(state: S, p: PagePoint, ctx: ToolContext): ToolResult<S>;
  onUp(state: S, p: PagePoint, ctx: ToolContext): ToolResult<S>;
  onKey?(state: S, key: string, ctx: ToolContext): ToolResult<S>;
  /** called when the tool is armed — the 도장 tool opens the file picker here */
  onArm?(): void;
  /**
   * `true` while the tool wants the pointer even between gestures (the eraser's cursor circle and
   * the stamp's ghost preview both track a hovering pointer).
   */
  hoverPreview?: boolean;
  /**
   * v0.3: the tool's state survives pointer-up — a gesture of several clicks (다각형). The tool
   * returns its empty state itself when it commits or cancels.
   */
  keepsState?: boolean;
}

/** What the annotation host plugs in so a finished gesture becomes an IPC call. */
export interface ToolSink {
  commit(page: PageIndex, spec: AnnotSpec): void;
  erase(page: PageIndex, ids: AnnotId[]): void;
  patch(page: PageIndex, edits: { id: AnnotId; patch: AnnotPatch }[], live: boolean): void;
  select(ids: AnnotId[], additive: boolean): void;
  edit(target: { page: PageIndex; rect?: Rect; id?: AnnotId; callout?: number[] }): void;
  /** v0.3 A3: the partial eraser's result — patch `paths`, delete what is empty (optional sink) */
  erasePartial?(page: PageIndex, edits: { id: AnnotId; paths: number[][] }[]): void;
  /** v0.3 A4: several annotations of one gesture, created as ONE undo step (without it: `commit` each) */
  commitMany?(page: PageIndex, specs: AnnotSpec[]): void;
  /** the tool finished and asked to go back to 선택 */
  done(): void;
}

export interface ToolController {
  /** arm a tool (tap = latched, hold = momentary; UI_SPEC §6) */
  arm(tool: ToolId, momentary?: boolean): void;
  current(): ToolId;
  cursor(): string;
  register<S>(module: ToolModule<S>): void;
  has(tool: ToolId): boolean;
  setSink(sink: ToolSink | null): void;
  /** the canvas forwards pointer events here */
  pointerDown(p: PagePoint, ctx: ToolContext): void;
  pointerMove(p: PagePoint, ctx: ToolContext): void;
  pointerUp(p: PagePoint, ctx: ToolContext): void;
  /** Esc, ⇧ press/release while drawing, … */
  key(key: string, ctx: ToolContext): void;
  /** drop the in-flight gesture without committing (pointercancel, tool change) */
  cancel(): void;
  preview(): ToolPreview | null;
  /** `true` while a gesture is in flight — the surface uses it to keep pointer capture */
  active(): boolean;
  /** v0.3: the armed tool's state between gestures (a multi-click tool's), `null` when idle */
  pendingState(): unknown;
  /** re-render hook for the overlay; returns an unsubscribe */
  subscribe(listener: () => void): () => void;
  /** monotonic counter, a `useSyncExternalStore` snapshot */
  version(): number;
}

/** UI_SPEC §6. The eraser and the stamp replace theirs with a drawn cursor in the overlay. */
const CURSORS: Partial<Record<ToolId, string>> = {
  select: "default",
  hand: "grab",
  snapshot: "crosshair",
  highlight: "text",
  underline: "text",
  strikeout: "text",
  squiggly: "text",
  note: "copy",
  pen: "crosshair",
  eraser: "crosshair",
  rectangle: "crosshair",
  ellipse: "crosshair",
  line: "crosshair",
  arrow: "crosshair",
  polygon: "crosshair",
  callout: "crosshair",
  textbox: "crosshair",
  stamp: "copy",
  signature: "copy",
  editText: "text",
  addText: "crosshair",
  addImage: "crosshair",
  redact: "crosshair",
  link: "crosshair",
  fillForm: "default",
};

/** The tools that draw on the page and therefore need the pointer-capture surface. */
export const DRAWING_TOOLS: ToolId[] = [
  "note",
  "pen",
  "eraser",
  "rectangle",
  "ellipse",
  "line",
  "arrow",
  "textbox",
  "stamp",
  "signature",
  // v0.3 pkg4
  "polygon",
  "callout",
];

/** The text-anchored tools: the viewer owns the drag, the tool only reads the selection on up. */
export const MARKUP_TOOLS: ToolId[] = ["highlight", "underline", "strikeout", "squiggly"];

class RealToolController implements ToolController {
  private tool: ToolId = "select";
  private momentaryFrom: ToolId | null = null;
  private modules = new Map<ToolId, ToolModule<unknown>>();
  private sink: ToolSink | null = null;
  private state: unknown = null;
  private started = false;
  private down = false;
  private currentPreview: ToolPreview | null = null;
  private listeners = new Set<() => void>();
  private nonce = 0;

  arm(tool: ToolId, momentary = false): void {
    if (this.tool === tool && !momentary) return;
    this.cancel();
    this.momentaryFrom = momentary ? (this.momentaryFrom ?? this.tool) : null;
    this.tool = tool;
    this.modules.get(tool)?.onArm?.();
    this.emit();
  }

  current(): ToolId {
    return this.tool;
  }

  cursor(): string {
    return this.modules.get(this.tool)?.cursor ?? CURSORS[this.tool] ?? "default";
  }

  register<S>(module: ToolModule<S>): void {
    this.modules.set(module.id, module as ToolModule<unknown>);
    if (module.id === this.tool) this.emit();
  }

  has(tool: ToolId): boolean {
    return this.modules.has(tool);
  }

  setSink(sink: ToolSink | null): void {
    this.sink = sink;
  }

  pointerDown(p: PagePoint, ctx: ToolContext): void {
    const module = this.modules.get(this.tool);
    if (!module) return;
    this.down = true;
    this.ensureState(module, ctx);
    this.apply(module.onDown(this.state, p, ctx));
  }

  pointerMove(p: PagePoint, ctx: ToolContext): void {
    const module = this.modules.get(this.tool);
    if (!module) return;
    if (!this.down && !module.hoverPreview) return;
    this.ensureState(module, ctx);
    this.apply(module.onMove(this.state, p, ctx));
  }

  pointerUp(p: PagePoint, ctx: ToolContext): void {
    const module = this.modules.get(this.tool);
    if (!module || !this.down) return;
    this.down = false;
    this.ensureState(module, ctx);
    this.apply(module.onUp(this.state, p, ctx));
    if (!module.keepsState) {
      this.started = false;
      this.state = null;
    }
  }

  key(key: string, ctx: ToolContext): void {
    const module = this.modules.get(this.tool);
    if (!module?.onKey) return;
    this.ensureState(module, ctx);
    this.apply(module.onKey(this.state, key, ctx));
  }

  cancel(): void {
    this.down = false;
    this.started = false;
    this.state = null;
    if (this.currentPreview) {
      this.currentPreview = null;
      this.emit();
    }
  }

  preview(): ToolPreview | null {
    return this.currentPreview;
  }

  active(): boolean {
    return this.down;
  }

  pendingState(): unknown {
    return this.started ? this.state : null;
  }

  subscribe(listener: () => void): () => void {
    this.listeners.add(listener);
    return () => {
      this.listeners.delete(listener);
    };
  }

  version(): number {
    return this.nonce;
  }

  private ensureState(module: ToolModule<unknown>, ctx: ToolContext): void {
    if (!this.started) {
      this.state = module.init(ctx);
      this.started = true;
    }
  }

  private apply(result: ToolResult<unknown>): void {
    this.state = result.state;
    const previewChanged = result.preview !== undefined && result.preview !== this.currentPreview;
    if (result.preview !== undefined) this.currentPreview = result.preview;

    const sink = this.sink;
    if (sink) {
      if (result.select) sink.select(result.select.ids, result.select.additive ?? false);
      if (result.erase && result.erase.ids.length) sink.erase(result.erase.page, result.erase.ids);
      if (result.patch?.edits.length) sink.patch(result.patch.page, result.patch.edits, result.patch.live ?? false);
      if (result.commit) sink.commit(result.commit.page, result.commit.spec);
      const commits = result.commits ?? [];
      const onePage = commits.length > 1 && commits.every((c) => c.page === commits[0].page);
      if (onePage && sink.commitMany) sink.commitMany(commits[0].page, commits.map((c) => c.spec));
      else for (const c of commits) sink.commit(c.page, c.spec);
      if (result.partialErase?.edits.length) sink.erasePartial?.(result.partialErase.page, result.partialErase.edits);
      if (result.edit) sink.edit(result.edit);
    }
    if (result.done) {
      this.cancel();
      if (sink) sink.done();
    }
    if (previewChanged || result.done) this.emit();
  }

  private emit(): void {
    this.nonce += 1;
    for (const listener of this.listeners) listener();
  }
}

export const toolController: ToolController = new RealToolController();
