/**
 * What one properties-panel control does. Split out of `InspectorBody.tsx` so the rule is
 * testable on its own (and so the component file exports only components — React Fast Refresh
 * bails out of a module that mixes the two).
 */
import type { Annot } from "../../ipc/types";
import type { ToolStyle } from "../../store/annotStore";
import { patchAnnotation } from "../../annot/actions";
import { appliesTo, isLive, patchFor, styleFor, type PropertyId, type PropertyValue } from "./patch";

/**
 * With nothing selected the panel edits the **tool default**; with a selection it edits the
 * selection and leaves the default alone (that is what 이 스타일을 기본값으로 is for). Since v0.3
 * 정렬 and 화살표 are patch fields too (A1); only 지우개 크기 is a tool setting on its own. A mixed
 * selection patches only the annotations the control applies to (`appliesTo`).
 */
export function makeApply(selection: Annot[], setStyle: (patch: Partial<ToolStyle>) => void) {
  return (id: PropertyId, value: PropertyValue): void => {
    const patch = selection.length > 0 ? patchFor(id, value) : null;
    if (patch) {
      for (const annot of selection) {
        if (appliesTo(id, annot.kind)) patchAnnotation(annot.page, annot.id, patch, isLive(id));
      }
      return;
    }
    const style = styleFor(id, value);
    if (style) setStyle(style);
  };
}
