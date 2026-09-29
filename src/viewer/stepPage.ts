/**
 * The page ◀ / ▶, ⌘↑ / ⌘↓ and Space go to — on its own, dependency-free, because the status bar
 * and the command dispatcher are on the critical path while the rest of `layout.ts` (and the
 * geometry it pulls in) belongs to the lazy viewer chunk.
 */
import type { PageIndex, ViewLayout } from "../ipc/types";

/**
 * The first page of the previous / next row, so 두 쪽 steps a whole spread. Clamped to the first
 * and the last row. 두 쪽 (표지 따로, v0.3) has the cover alone, so its rows start at 0, 1, 3, 5 ….
 */
export function stepPage(current: PageIndex, delta: 1 | -1, pageCount: number, mode: ViewLayout): PageIndex {
  if (pageCount <= 0) return 0;
  const last = pageCount - 1;
  const page = Math.max(0, Math.min(last, current));
  if (mode === "twoCover") {
    // rows: [0], [1, 2], [3, 4], … — a row starts at 0 or at an odd index
    const rowStart = (p: PageIndex) => (p === 0 ? 0 : p % 2 === 1 ? p : p - 1);
    const from = rowStart(page);
    const next = delta > 0 ? (from === 0 ? 1 : from + 2) : from <= 1 ? 0 : from - 2;
    return Math.max(0, Math.min(rowStart(last), next));
  }
  const span = mode === "two" ? 2 : 1;
  const rowStart = (p: PageIndex) => p - (p % span);
  const from = rowStart(page);
  return Math.max(0, Math.min(rowStart(last), from + delta * span));
}
