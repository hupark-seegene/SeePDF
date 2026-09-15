import { describe, expect, it } from "vitest";
import {
  boxFromPoints, boxesIntersect, clickSelect, marqueeSelect, normalizeSelection, pressSelect, rangeBetween, stepFocus,
} from "./selection";
import { usePagesStore } from "../store/pagesStore";

describe("organize.selection", () => {
  it("a plain click replaces the selection and moves the anchor", () => {
    const next = clickSelect({ selected: [1, 2, 3], anchor: 3 }, 7);
    expect(next).toEqual({ selected: [7], anchor: 7 });
  });

  it("⇧click ranges from the anchor in both directions and keeps the anchor", () => {
    const down = clickSelect({ selected: [2], anchor: 2 }, 5, { shift: true });
    expect(down).toEqual({ selected: [2, 3, 4, 5], anchor: 2 });
    const up = clickSelect(down, 0, { shift: true });
    expect(up).toEqual({ selected: [0, 1, 2], anchor: 2 });
  });

  it("⇧⌘click unions the new range with what is already selected", () => {
    const next = clickSelect({ selected: [8, 9], anchor: 1 }, 3, { shift: true, meta: true });
    expect(next.selected).toEqual([1, 2, 3, 8, 9]);
  });

  it("⌘click toggles one cell", () => {
    const added = clickSelect({ selected: [0, 4], anchor: 4 }, 2, { meta: true });
    expect(added).toEqual({ selected: [0, 2, 4], anchor: 2 });
    expect(clickSelect(added, 4, { meta: true }).selected).toEqual([0, 2]);
  });

  it("pressing inside an existing multi-selection keeps it, so the block can be dragged", () => {
    expect(pressSelect({ selected: [2, 3, 4], anchor: 4 }, 3).selected).toEqual([2, 3, 4]);
    expect(pressSelect({ selected: [2, 3, 4], anchor: 4 }, 9).selected).toEqual([9]);
  });

  it("a marquee replaces the selection, and unions while a modifier is held", () => {
    expect(marqueeSelect({ selected: [0], anchor: 0 }, [4, 5], false).selected).toEqual([4, 5]);
    expect(marqueeSelect({ selected: [0], anchor: 0 }, [4, 5], true).selected).toEqual([0, 4, 5]);
  });

  it("normalises to ascending unique and ranges are inclusive", () => {
    expect(normalizeSelection([5, 1, 5, 3])).toEqual([1, 3, 5]);
    expect(rangeBetween(4, 2)).toEqual([2, 3, 4]);
  });

  it("boxes intersect the way the marquee needs them to", () => {
    const rect = boxFromPoints(30, 40, 10, 10);
    expect(rect).toEqual({ x: 10, y: 10, w: 20, h: 30 });
    expect(boxesIntersect(rect, { x: 25, y: 35, w: 20, h: 20 })).toBe(true);
    expect(boxesIntersect(rect, { x: 60, y: 10, w: 10, h: 10 })).toBe(false);
  });

  it("roving focus clamps to the document", () => {
    expect(stepFocus(0, -4, 12)).toBe(0);
    expect(stepFocus(10, 4, 12)).toBe(11);
    expect(stepFocus(null, 3, 12)).toBe(3);
  });

  it("pagesStore.click drives the same arithmetic and remembers the focus", () => {
    const pages = usePagesStore.getState();
    pages.click(2);
    usePagesStore.getState().click(5, { shift: true });
    expect(usePagesStore.getState().selected).toEqual([2, 3, 4, 5]);
    expect(usePagesStore.getState().focus).toBe(5);
    usePagesStore.getState().click(3, { meta: true });
    expect(usePagesStore.getState().selected).toEqual([2, 4, 5]);
    usePagesStore.getState().selectAll(4);
    expect(usePagesStore.getState().selected).toEqual([0, 1, 2, 3]);
    usePagesStore.getState().reset();
    expect(usePagesStore.getState().selected).toEqual([]);
  });
});
