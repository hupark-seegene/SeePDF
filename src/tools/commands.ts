/**
 * The command seam between `src/app/useCommands.ts` (eager, critical path) and the annotation
 * host (lazy). `useCommands` asks the bus first; the bus answers `false` when the annotation
 * module is not loaded or the command does not apply right now, and the dispatcher falls through
 * to whatever it did before.
 *
 * Deliberately 30 lines with no imports: everything it routes to lives in the lazy chunk.
 */
export type AnnotCommandId =
  | "edit.delete"
  | "edit.duplicate"
  | "edit.copy"
  | "edit.cut"
  | "edit.paste"
  | "edit.selectAll"
  | "annot.nudge";

export interface AnnotCommandHandler {
  (id: AnnotCommandId | string, arg?: unknown): boolean;
}

let handler: AnnotCommandHandler | null = null;

export function setAnnotCommandHandler(fn: AnnotCommandHandler | null): void {
  handler = fn;
}

/** The 편집 mode host (Stage 7, lazy `src/edit/`) claims the same ids while 편집 is active. */
let editHandler: AnnotCommandHandler | null = null;

export function setEditCommandHandler(fn: AnnotCommandHandler | null): void {
  editHandler = fn;
}

/** `true` when the annotation (or 편집) module handled it; `false` means "not mine". */
export function runAnnotCommand(id: string, arg?: unknown): boolean {
  if (handler?.(id, arg)) return true;
  return editHandler ? editHandler(id, arg) : false;
}
