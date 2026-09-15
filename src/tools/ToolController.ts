/**
 * Tool-mode routing — the STUB Stage 1 (d) replaces (WORKPLAN: "the tool-mode contract used by the
 * tool strip, already stubbed in Stage 0"). The shell only ever calls `toolController.arm()` and
 * hands it pointer events from the canvas; nothing else in `src/app/**` knows about tools.
 *
 * Shape (ARCHITECTURE §10): every tool is a pure state machine
 * `onDown/onMove/onUp(state, pt) → { state, preview?, commit? }`, unit-testable without React.
 */
import type { AnnotSpec, DocGeneration, DocId, PageIndex, Point, Rgb } from "../ipc/types";
import type { ToolId } from "../store/appStore";

/** A point in PDF user space on one page (points, y-up, unrotated — IPC_CONTRACT §3). */
export interface PagePoint { page: PageIndex; pt: Point }

export interface ToolModifiers { shift: boolean; alt: boolean; meta: boolean; ctrl: boolean }

export interface ToolContext {
  docId: DocId;
  docGeneration: DocGeneration;
  /** current style from `annotStore` (colour, opacity, width, font size) */
  style: { color: Rgb; opacity: number; width: number; fontSize: number; fillColor: Rgb | null };
  modifiers: ToolModifiers;
}

/** What the tool wants drawn right now, before anything is committed (optimistic overlay). */
export interface ToolPreview {
  page: PageIndex;
  kind: "rect" | "ellipse" | "ink" | "line" | "marquee" | "quads";
  rect?: { l: number; b: number; r: number; t: number };
  points?: number[];
  quads?: { l: number; b: number; r: number; t: number }[];
}

export interface ToolResult<S = unknown> {
  state: S;
  preview?: ToolPreview | null;
  /** an annotation to create — the controller runs `api.createAnnotation` and clears the ghost */
  commit?: AnnotSpec;
  /** the tool wants to hand control back (Esc, or a one-shot tool that finished) */
  done?: boolean;
}

export interface ToolModule<S = unknown> {
  id: ToolId;
  /** CSS cursor while the tool is armed (UI_SPEC §6) */
  cursor: string;
  init(ctx: ToolContext): S;
  onDown(state: S, p: PagePoint, ctx: ToolContext): ToolResult<S>;
  onMove(state: S, p: PagePoint, ctx: ToolContext): ToolResult<S>;
  onUp(state: S, p: PagePoint, ctx: ToolContext): ToolResult<S>;
  onKey?(state: S, key: string, ctx: ToolContext): ToolResult<S>;
}

export interface ToolController {
  /** arm a tool (tap = latched, hold = momentary; UI_SPEC §6) */
  arm(tool: ToolId, momentary?: boolean): void;
  current(): ToolId;
  cursor(): string;
  register<S>(module: ToolModule<S>): void;
  /** the canvas forwards pointer events here; Stage 0 ignores them */
  pointerDown(p: PagePoint, ctx: ToolContext): void;
  pointerMove(p: PagePoint, ctx: ToolContext): void;
  pointerUp(p: PagePoint, ctx: ToolContext): void;
  preview(): ToolPreview | null;
}

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
  textbox: "crosshair",
  stamp: "crosshair",
  signature: "crosshair",
  editText: "text",
  addText: "crosshair",
  addImage: "crosshair",
  redact: "crosshair",
  fillForm: "default",
};

/** Stage 0 implementation: remembers the armed tool and its cursor, commits nothing. */
class StubToolController implements ToolController {
  private tool: ToolId = "select";
  private modules = new Map<ToolId, ToolModule<unknown>>();

  arm(tool: ToolId): void {
    this.tool = tool;
  }
  current(): ToolId {
    return this.tool;
  }
  cursor(): string {
    return this.modules.get(this.tool)?.cursor ?? CURSORS[this.tool] ?? "default";
  }
  register<S>(module: ToolModule<S>): void {
    this.modules.set(module.id, module as ToolModule<unknown>);
  }
  pointerDown(): void {}
  pointerMove(): void {}
  pointerUp(): void {}
  preview(): ToolPreview | null {
    return null;
  }
}

export const toolController: ToolController = new StubToolController();
