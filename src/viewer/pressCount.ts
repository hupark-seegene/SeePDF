/**
 * How many presses in a row a `pointerdown` is: 1 a click, 2 a double-click, 3 a triple-click.
 *
 * WebKit (the macOS webview) reports the click count in a mouse `pointerdown`'s `detail`, like
 * `mousedown`. Chromium — WebView2, the Windows webview — always reports 0 there (the Pointer
 * Events spec keeps `detail` for `click`), so code that read `detail` never saw a double-click on
 * Windows: 선택's double-click to edit text (and to reach 그룹 해제) and the viewer's word / line
 * selection did nothing (v0.3.1 real-app QA). When `detail` is 0 the presses are counted here: a
 * press within the double-click time and a few pixels of the previous one continues the run.
 */
export interface Press {
  detail: number;
  button: number;
  clientX: number;
  clientY: number;
}

/** Windows' default double-click time (GetDoubleClickTime). */
export const DOUBLE_CLICK_MS = 500;
/** How far (px) the pointer may move between the presses of one double-click. */
export const DOUBLE_CLICK_SLOP_PX = 4;

/** One counter per surface: it remembers the previous press of that surface only. */
export function createPressCounter(now: () => number = () => performance.now()): (e: Press) => number {
  let last: { at: number; x: number; y: number; button: number; count: number } | null = null;
  return (e) => {
    const at = now();
    let count = e.detail;
    if (!(count > 0)) {
      const continues =
        last !== null &&
        e.button === last.button &&
        at - last.at <= DOUBLE_CLICK_MS &&
        Math.abs(e.clientX - last.x) <= DOUBLE_CLICK_SLOP_PX &&
        Math.abs(e.clientY - last.y) <= DOUBLE_CLICK_SLOP_PX;
      count = continues && last ? last.count + 1 : 1;
    }
    last = { at, x: e.clientX, y: e.clientY, button: e.button, count };
    return count;
  };
}
