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

/** `true` when the annotation module handled it; `false` means "not mine". */
export function runAnnotCommand(id: string, arg?: unknown): boolean {
  return handler ? handler(id, arg) : false;
}
