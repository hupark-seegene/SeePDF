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
import type { AnnotPatch, Rgb } from "../../ipc/types";
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
  | "eraserSize";

export type PropertyValue = Rgb | null | number | string | boolean[] | "left" | "center" | "right";

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
    case "align":
    case "heads":
    case "eraserSize":
      // Stored in the tool style only: `AnnotPatch` has no field for them, so changing them on a
      // selected annotation would silently do nothing — the UI hides them instead.
      return null;
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
      return null;
  }
}

/** Sliders and colour drags coalesce; a click, a chip or a committed text field does not. */
export function isLive(id: PropertyId): boolean {
  return id === "opacity" || id === "width" || id === "fontSize" || id === "eraserSize";
}
