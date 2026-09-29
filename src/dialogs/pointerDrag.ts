/**
 * Pointer-event dragging for lists (v0.3 H7).
 *
 * HTML5 drag and drop (`draggable` / `dragover` / `drop`) never fires in the Windows webview while
 * Tauri's native file drag-drop handler is on — and that handler has to stay on, it is what gives
 * dropped files their real paths. So every in-app reorder (파일 합치기, 이미지로 PDF 만들기, 목차 편집)
 * is driven by pointer events instead, the way the page organizer already is: `pointerdown` arms,
 * a 4 px threshold starts the drag, `pointermove` moves an insertion indicator, `pointerup` commits,
 * Esc cancels. Buttons and fields inside a row keep their own clicks.
 */

const THRESHOLD_PX = 4;

export interface DragMove {
  clientX: number;
  clientY: number;
  /** from the pointerdown, in CSS pixels */
  dx: number;
  dy: number;
}

export interface DragHandlers {
  onStart?(): void;
  onMove(m: DragMove): void;
  onDrop(m: DragMove): void;
  onCancel(): void;
}

/** Controls (buttons, fields, links) inside a draggable row keep their own pointer handling. */
export function isInteractive(target: EventTarget | null): boolean {
  const el = target as HTMLElement | null;
  return Boolean(el?.closest?.("button, input, select, textarea, a, [contenteditable='true']"));
}

/**
 * Starts tracking a drag from a `pointerdown`. Nothing happens until the pointer moved
 * {@link THRESHOLD_PX}; a plain click is left alone (no `onStart`, no `onDrop`).
 */
export function startPointerDrag(
  e: { clientX: number; clientY: number; button?: number; pointerId?: number; target: EventTarget | null },
  handlers: DragHandlers,
): void {
  if ((e.button ?? 0) !== 0 || isInteractive(e.target)) return;
  const x0 = e.clientX;
  const y0 = e.clientY;
  let started = false;
  const at = (ev: PointerEvent | MouseEvent): DragMove => ({
    clientX: ev.clientX,
    clientY: ev.clientY,
    dx: ev.clientX - x0,
    dy: ev.clientY - y0,
  });
  const cleanup = () => {
    window.removeEventListener("pointermove", move);
    window.removeEventListener("pointerup", up);
    window.removeEventListener("pointercancel", cancel);
    window.removeEventListener("keydown", key, true);
  };
  const move = (ev: PointerEvent) => {
    const m = at(ev);
    if (!started) {
      if (Math.hypot(m.dx, m.dy) < THRESHOLD_PX) return;
      started = true;
      handlers.onStart?.();
    }
    ev.preventDefault?.();
    handlers.onMove(m);
  };
  const up = (ev: PointerEvent) => {
    cleanup();
    if (started) handlers.onDrop(at(ev));
  };
  const cancel = () => {
    cleanup();
    if (started) handlers.onCancel();
  };
  const key = (ev: KeyboardEvent) => {
    if (ev.key !== "Escape" || !started) return;
    ev.preventDefault();
    ev.stopPropagation();
    cancel();
  };
  window.addEventListener("pointermove", move);
  window.addEventListener("pointerup", up);
  window.addEventListener("pointercancel", cancel);
  window.addEventListener("keydown", key, true);
}

/**
 * The insertion index (0…n) for a pointer at `clientY` over the rows `list` holds (`selector`):
 * before the first row whose vertical middle is below the pointer.
 */
export function insertionIndex(list: HTMLElement | null, clientY: number, selector: string): number {
  if (!list) return 0;
  const rows = [...list.querySelectorAll<HTMLElement>(selector)];
  for (let i = 0; i < rows.length; i++) {
    const r = rows[i].getBoundingClientRect();
    if (clientY < r.top + r.height / 2) return i;
  }
  return rows.length;
}

/** `from` moved so that it lands at insertion index `to` (0…n) — or `null` when that is where it is. */
export function reorderTarget(from: number, to: number): number | null {
  if (to === from || to === from + 1) return null;
  return to > from ? to - 1 : to;
}

/** A copy of `items` with `from` moved to `index` (as {@link reorderTarget} computes it). */
export function moveItem<T>(items: T[], from: number, index: number): T[] {
  const next = [...items];
  const [item] = next.splice(from, 1);
  next.splice(index, 0, item);
  return next;
}
