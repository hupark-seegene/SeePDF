/**
 * The page ◀ / ▶, ⌘↑ / ⌘↓ and Space go to — on its own, dependency-free, because the status bar
 * and the command dispatcher are on the critical path while the rest of `layout.ts` (and the
 * geometry it pulls in) belongs to the lazy viewer chunk.
 */
import type { PageIndex, ViewLayout } from "../ipc/types";

/**
 * The first page of the previous / next row, so 두 쪽 steps a whole spread. Clamped to the first
 * and the last row.
 */
export function stepPage(current: PageIndex, delta: 1 | -1, pageCount: number, mode: ViewLayout): PageIndex {
  if (pageCount <= 0) return 0;
  const span = mode === "two" ? 2 : 1;
  const last = pageCount - 1;
  const rowStart = (page: PageIndex) => page - (page % span);
  const from = rowStart(Math.max(0, Math.min(last, current)));
  return Math.max(0, Math.min(rowStart(last), from + delta * span));
}
