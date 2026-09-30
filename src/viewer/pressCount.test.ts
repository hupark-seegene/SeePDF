import { describe, expect, it } from "vitest";
import { createPressCounter, DOUBLE_CLICK_MS } from "./pressCount";

const press = (x: number, y: number, detail = 0, button = 0) => ({ clientX: x, clientY: y, detail, button });

describe("pressCount (v0.3.1: WebView2 reports no click count on pointerdown)", () => {
  it("WebKit's own count is used as is", () => {
    const count = createPressCounter(() => 0);
    expect(count(press(10, 10, 1))).toBe(1);
    expect(count(press(10, 10, 2))).toBe(2);
    expect(count(press(10, 10, 3))).toBe(3);
  });

  it("detail 0 (Chromium): presses close in time and place make a double and a triple click", () => {
    let now = 1000;
    const count = createPressCounter(() => now);
    expect(count(press(100, 100))).toBe(1);
    now += 120;
    expect(count(press(102, 99))).toBe(2);
    now += 120;
    expect(count(press(101, 101))).toBe(3);
  });

  it("detail 0: a slow second press, a press elsewhere or with another button starts over", () => {
    let now = 0;
    const count = createPressCounter(() => now);
    expect(count(press(100, 100))).toBe(1);
    now += DOUBLE_CLICK_MS + 1;
    expect(count(press(100, 100))).toBe(1);
    now += 100;
    expect(count(press(120, 100))).toBe(1);
    now += 100;
    expect(count(press(120, 100, 0, 2))).toBe(1);
    now += 100;
    expect(count(press(120, 100))).toBe(1);
    now += 100;
    expect(count(press(120, 100))).toBe(2);
  });
});
