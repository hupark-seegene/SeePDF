/**
 * Public surface of the dialogs module (Stage 1 (e)).
 *
 * The **store and the ask-helpers** (`dialogState.ts`) are entry-safe: import them anywhere.
 * Everything else — the host, the sheets, the flows — is behind `import("../dialogs/…")` so it
 * never reaches the critical path (`scripts/check-bundle-size.mjs`). `DialogHost` is deliberately
 * NOT re-exported here: `App.tsx` reaches it with `lazy(() => import("./dialogs/DialogHost"))`.
 */
export {
  askExtract, askMultipleFiles, askPassword, askUnsaved, closeDialog, isDialogOpen, openDialog,
  useDialogStore, type DialogName, type MultipleFilesAnswer, type UnsavedAnswer,
} from "./dialogState";
export { formatPageRange, parsePageRange, resolveRange, type RangeChoice, type RangeMode } from "./pageRange";
