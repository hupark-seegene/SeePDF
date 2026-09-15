/**
 * Public surface of the organizer (Stage 1 (e)). `App.tsx` mounts it with
 * `lazy(() => import("./organize"))` so the grid never reaches the critical path.
 */
export { Organizer as default, Organizer } from "./Organizer";
export { applyMove, caretToDestination, identityOrder, isNoOpMove, moveOpFor } from "./moveOp";
export {
  boxFromPoints, boxesIntersect, clickSelect, marqueeSelect, normalizeSelection, pressSelect,
  rangeBetween, stepFocus, type Box, type ClickMods, type SelectionState,
} from "./selection";
