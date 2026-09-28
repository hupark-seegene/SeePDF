/**
 * 도구별 기본 스타일 (P1-12): every drawing tool starts from its own built-in style, overlaid
 * with what the user last chose for **that** tool (`Settings.toolDefaults[toolId]`). Pure, so
 * the resolution and the validation of a hand-edited settings file are unit-tested
 * (`toolStyles.test.ts`); `annotStore` holds the live copy and `annot/toolDefaults.ts` persists it.
 *
 * Only the fields the user changed are stored, so a later change to a built-in default still
 * reaches every field they never touched.
 */
import type { AnnotKind, Rgb } from "../ipc/types";
import type { ToolStyle } from "./annotStore";

const YELLOW: Rgb = [255, 216, 77];
const BLUE: Rgb = [77, 184, 255];
const RED: Rgb = [245, 83, 61];
const INK: Rgb = [43, 47, 51];

/** The fallback for every field (and the style of a tool with no entry below). */
export const BASE_STYLE: ToolStyle = {
  color: YELLOW,
  opacity: 0.4,
  width: 2,
  fontSize: 12,
  fillColor: null,
  heads: [false, true],
  align: "left",
  eraserSize: 12,
};

/** The tools whose style is remembered, with their built-in defaults. */
const BUILT_IN: Record<string, Partial<ToolStyle>> = {
  highlight: { color: YELLOW, opacity: 0.4 },
  underline: { color: BLUE, opacity: 1 },
  strikeout: { color: RED, opacity: 1 },
  squiggly: { color: RED, opacity: 1 },
  note: { color: YELLOW, opacity: 1 },
  pen: { color: INK, opacity: 1, width: 2 },
  eraser: { eraserSize: 12 },
  rectangle: { color: RED, opacity: 1, width: 2, fillColor: null },
  ellipse: { color: RED, opacity: 1, width: 2, fillColor: null },
  line: { color: RED, opacity: 1, width: 2, heads: [false, false] },
  arrow: { color: RED, opacity: 1, width: 2, heads: [false, true] },
  textbox: { color: INK, opacity: 1, fontSize: 12, fillColor: null, align: "left" },
};

export const STYLED_TOOLS: readonly string[] = Object.keys(BUILT_IN);

export function isStyledTool(tool: string | null | undefined): tool is string {
  return !!tool && tool in BUILT_IN;
}

export function baseStyleFor(tool: string): ToolStyle {
  return { ...BASE_STYLE, ...(BUILT_IN[tool] ?? {}) };
}

/** Built-in default + the user's override for `tool`. */
export function styleFor(tool: string, overrides: Record<string, Partial<ToolStyle>>): ToolStyle {
  return { ...baseStyleFor(tool), ...(overrides[tool] ?? {}) };
}

/** The tool an annotation of `kind` is drawn with — for 이 스타일을 기본값으로 on a selection. */
export function toolOfKind(kind: AnnotKind | undefined): string | null {
  switch (kind) {
    case "square":
      return "rectangle";
    case "circle":
      return "ellipse";
    case "ink":
      return "pen";
    case undefined:
      return null;
    default:
      return isStyledTool(kind) ? kind : null;
  }
}

function isRgb(v: unknown): v is Rgb {
  return Array.isArray(v) && v.length === 3 && v.every((c) => Number.isInteger(c) && c >= 0 && c <= 255);
}

function num(v: unknown, lo: number, hi: number): number | undefined {
  return typeof v === "number" && Number.isFinite(v) && v >= lo && v <= hi ? v : undefined;
}

/** One `toolDefaults` entry, keeping only well-formed fields (a hand-edited file stays harmless). */
export function parseToolStyle(value: unknown): Partial<ToolStyle> {
  if (!value || typeof value !== "object" || Array.isArray(value)) return {};
  const v = value as Record<string, unknown>;
  const out: Partial<ToolStyle> = {};
  if (isRgb(v.color)) out.color = [...v.color] as Rgb;
  if (v.fillColor === null || isRgb(v.fillColor)) out.fillColor = v.fillColor === null ? null : ([...(v.fillColor as Rgb)] as Rgb);
  const opacity = num(v.opacity, 0.05, 1);
  if (opacity !== undefined) out.opacity = opacity;
  const width = num(v.width, 0.25, 72);
  if (width !== undefined) out.width = width;
  const fontSize = num(v.fontSize, 4, 144);
  if (fontSize !== undefined) out.fontSize = fontSize;
  const eraserSize = num(v.eraserSize, 1, 144);
  if (eraserSize !== undefined) out.eraserSize = eraserSize;
  if (Array.isArray(v.heads) && v.heads.length === 2 && v.heads.every((h) => typeof h === "boolean")) {
    out.heads = [v.heads[0] as boolean, v.heads[1] as boolean];
  }
  if (v.align === "left" || v.align === "center" || v.align === "right") out.align = v.align;
  return out;
}

/** `Settings.toolDefaults` → the per-tool overrides, ignoring keys that are not styled tools. */
export function readToolDefaults(value: unknown): Record<string, Partial<ToolStyle>> {
  const out: Record<string, Partial<ToolStyle>> = {};
  if (!value || typeof value !== "object" || Array.isArray(value)) return out;
  for (const [tool, entry] of Object.entries(value as Record<string, unknown>)) {
    if (!isStyledTool(tool)) continue;
    const style = parseToolStyle(entry);
    if (Object.keys(style).length) out[tool] = style;
  }
  return out;
}

/**
 * The next `Settings.toolDefaults`: `overrides` for the styled tools (a reset tool loses its key),
 * every other key (the pre-Stage-2 `recentsCount`) left as it was.
 */
export function mergeToolDefaults(
  current: Record<string, unknown> | undefined,
  overrides: Record<string, Partial<ToolStyle>>,
): Record<string, unknown> {
  const next: Record<string, unknown> = { ...(current ?? {}) };
  for (const tool of STYLED_TOOLS) {
    if (overrides[tool] && Object.keys(overrides[tool]).length) next[tool] = overrides[tool];
    else delete next[tool];
  }
  return next;
}
