/**
 * The properties panel's one interesting piece of logic: mapping a control to an `AnnotPatch`
 * (IPC_CONTRACT §7.1) — and deciding whether the change edits the **selection** or the **tool
 * default**.
 *
 * UI_SPEC §7 puts both behind the same controls: with nothing selected the swatches set the style
 * the next annotation is created with; with a selection they edit it. Getting that wrong is how a
 * properties panel ends up silently changing the next five annotations.
 *
 * Pure and React-free so `Inspector/patch.test.ts` can assert the payload directly.
 */
import type { AnnotKind, AnnotPatch, Rgb } from "../../ipc/types";
import type { ToolStyle } from "../../store/annotStore";

export type PropertyId =
  | "color"
  | "fillColor"
  | "opacity"
  | "width"
  | "fontSize"
  | "align"
  | "heads"
  | "contents"
  | "author"
  | "text"
  | "eraserSize"
  // v0.3 pkg4: 인쇄 (A10) and 선 스타일 (A6) exist on a selection only
  | "printed"
  | "dashed";

export type PropertyValue = Rgb | null | number | string | boolean | boolean[] | "left" | "center" | "right";

/** What `update_annotation` gets for one control. `null` = the control does not touch the file. */
export function patchFor(id: PropertyId, value: PropertyValue): AnnotPatch | null {
  switch (id) {
    case "color":
      return { color: value as Rgb };
    case "fillColor":
      return { fillColor: value as Rgb | null };
    case "opacity":
      return { opacity: value as number };
    case "width":
      return { borderWidth: value as number };
    case "fontSize":
      return { fontSize: value as number };
    case "contents":
      return { contents: value as string };
    case "author":
      return { author: value as string };
    case "text":
      // A free-text annotation keeps its string in both places: `/Contents` is what other viewers
      // read, `text` is what the engine renders (spikes/annotations §3.2).
      return { text: value as string, contents: value as string };
    // v0.3 A1: the text box's alignment and the line's heads are real patch fields now
    case "align":
      return { align: value as "left" | "center" | "right" };
    case "heads":
      return { heads: value as [boolean, boolean] };
    case "printed":
      return { printed: value as boolean };
    case "dashed":
      return { dashed: value as boolean };
    case "eraserSize":
      // a tool setting only: an annotation has no eraser size
      return null;
  }
}

/**
 * v0.3: whether a control means anything on an annotation of `kind` — 정렬 on a text box, 화살표
 * on a line, 선 스타일 on a stroked shape. A mixed selection patches only the ones it applies to
 * (one needless `update_annotation` would still be an undo step).
 */
export function appliesTo(id: PropertyId, kind: AnnotKind): boolean {
  switch (id) {
    case "align":
      return kind === "textbox" || kind === "callout";
    case "heads":
      return kind === "line" || kind === "arrow";
    case "dashed":
      return ["square", "circle", "ink", "line", "arrow", "polygon", "polyline"].includes(kind);
    case "fontSize":
    case "text":
      return kind === "textbox" || kind === "callout";
    default:
      return true;
  }
}

/** What the same control does to the tool defaults (`annotStore.style`). */
export function styleFor(id: PropertyId, value: PropertyValue): Partial<ToolStyle> | null {
  switch (id) {
    case "color":
      return { color: value as Rgb };
    case "fillColor":
      return { fillColor: value as Rgb | null };
    case "opacity":
      return { opacity: value as number };
    case "width":
      return { width: value as number };
    case "fontSize":
      return { fontSize: value as number };
    case "align":
      return { align: value as ToolStyle["align"] };
    case "heads":
      return { heads: value as [boolean, boolean] };
    case "eraserSize":
      return { eraserSize: value as number };
    case "contents":
    case "author":
    case "text":
    case "printed":
    case "dashed":
      return null;
  }
}

/** Sliders and colour drags coalesce; a click, a chip or a committed text field does not. */
export function isLive(id: PropertyId): boolean {
  return id === "opacity" || id === "width" || id === "fontSize" || id === "eraserSize";
}
