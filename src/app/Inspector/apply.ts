/**
 * What one properties-panel control does. Split out of `InspectorBody.tsx` so the rule is
 * testable on its own (and so the component file exports only components — React Fast Refresh
 * bails out of a module that mixes the two).
 */
import type { Annot } from "../../ipc/types";
import type { ToolStyle } from "../../store/annotStore";
import { patchAnnotation } from "../../annot/actions";
import { isLive, patchFor, styleFor, type PropertyId, type PropertyValue } from "./patch";

/**
 * With nothing selected the panel edits the **tool default**; with a selection it edits the
 * selection and leaves the default alone (that is what 이 스타일을 기본값으로 is for). The controls
 * `AnnotPatch` has no field for (정렬, 화살표, 지우개 크기) always fall back to the default, because
 * patching them would be a silent no-op.
 */
export function makeApply(selection: Annot[], setStyle: (patch: Partial<ToolStyle>) => void) {
  return (id: PropertyId, value: PropertyValue): void => {
    const patch = selection.length > 0 ? patchFor(id, value) : null;
    if (patch) {
      for (const annot of selection) patchAnnotation(annot.page, annot.id, patch, isLive(id));
      return;
    }
    const style = styleFor(id, value);
    if (style) setStyle(style);
  };
}
