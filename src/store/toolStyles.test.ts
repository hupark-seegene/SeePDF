/**
 * 도구별 기본 스타일 (P1-12): each tool starts from its own built-in style plus what the user last
 * chose for that tool; the choice is written back to `Settings.toolDefaults` (only the changed
 * fields, legacy keys untouched), and 기본값으로 재설정 forgets it.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import * as api from "../ipc/api";
import { useAnnotStore } from "./annotStore";
import { useAppStore } from "./appStore";
import { baseStyleFor, mergeToolDefaults, parseToolStyle, readToolDefaults, styleFor, toolOfKind } from "./toolStyles";
import { TOOL_DEFAULTS_SAVE_MS, startToolDefaultsSync } from "../annot/toolDefaults";

function resetStores() {
  useAnnotStore.setState({ styleTool: null, toolDefaults: {}, style: baseStyleFor("highlight") });
  useAppStore.setState({ tool: "select", mode: "annotate" });
}

describe("toolStyles (pure)", () => {
  it("gives each tool its own built-in style", () => {
    expect(baseStyleFor("highlight")).toMatchObject({ color: [255, 216, 77], opacity: 0.4 });
    expect(baseStyleFor("pen")).toMatchObject({ color: [43, 47, 51], opacity: 1, width: 2 });
    expect(baseStyleFor("rectangle")).toMatchObject({ opacity: 1, fillColor: null });
    expect(styleFor("pen", { pen: { width: 8 } })).toMatchObject({ width: 8, opacity: 1 });
  });

  it("validates a hand-edited entry field by field", () => {
    expect(parseToolStyle({ color: [1, 2, 3], opacity: 7, width: "x", heads: [true, false], align: "center" })).toEqual({
      color: [1, 2, 3],
      heads: [true, false],
      align: "center",
    });
    expect(parseToolStyle([1, 2])).toEqual({});
    expect(readToolDefaults({ pen: { width: 4 }, recentsCount: 20, bogus: { width: 2 } })).toEqual({ pen: { width: 4 } });
  });

  it("merges back only the styled tools and drops a reset one", () => {
    const next = mergeToolDefaults({ recentsCount: 20, pen: { width: 4 }, ellipse: { width: 1 } }, { pen: { width: 8 } });
    expect(next).toEqual({ recentsCount: 20, pen: { width: 8 } });
  });

  it("maps annotation kinds to the tool that draws them", () => {
    expect(toolOfKind("square")).toBe("rectangle");
    expect(toolOfKind("circle")).toBe("ellipse");
    expect(toolOfKind("ink")).toBe("pen");
    expect(toolOfKind("highlight")).toBe("highlight");
    expect(toolOfKind("stamp")).toBeNull();
  });
});

describe("annotStore per-tool styles", () => {
  beforeEach(resetStores);

  it("switches style with the tool and remembers an edit for that tool only", () => {
    const s = useAnnotStore.getState();
    s.activateTool("pen");
    expect(useAnnotStore.getState().style.color).toEqual([43, 47, 51]);
    useAnnotStore.getState().setStyle({ width: 8 });
    useAnnotStore.getState().activateTool("rectangle");
    expect(useAnnotStore.getState().style.width).toBe(2);
    useAnnotStore.getState().activateTool("select"); // no style of its own: nothing changes
    expect(useAnnotStore.getState().styleTool).toBe("rectangle");
    useAnnotStore.getState().activateTool("pen");
    expect(useAnnotStore.getState().style.width).toBe(8);
    expect(useAnnotStore.getState().toolDefaults).toEqual({ pen: { width: 8 } });
  });

  it("sets a default from a selection without arming the tool, and resets it", () => {
    useAnnotStore.getState().activateTool("pen");
    useAnnotStore.getState().setToolDefault("rectangle", { color: [1, 2, 3] });
    expect(useAnnotStore.getState().style.color).toEqual([43, 47, 51]);
    useAnnotStore.getState().activateTool("rectangle");
    expect(useAnnotStore.getState().style.color).toEqual([1, 2, 3]);
    useAnnotStore.getState().resetToolDefault("rectangle");
    expect(useAnnotStore.getState().style).toEqual(baseStyleFor("rectangle"));
    expect(useAnnotStore.getState().toolDefaults).toEqual({});
  });
});

describe("toolDefaults ↔ Settings", () => {
  beforeEach(async () => {
    resetStores();
    await useAppStore.getState().bootstrap();
    await useAppStore.getState().patchSettings({ toolDefaults: { recentsCount: 20, pen: { width: 12 } } });
  });
  afterEach(() => vi.useRealTimers());

  it("loads the saved defaults, follows the armed tool, and writes edits back debounced", async () => {
    const stop = startToolDefaultsSync();
    useAppStore.getState().setTool("pen");
    expect(useAnnotStore.getState().style.width).toBe(12);

    vi.useFakeTimers();
    const spy = vi.spyOn(api, "setSettings");
    useAnnotStore.getState().setStyle({ opacity: 0.5 });
    useAnnotStore.getState().setStyle({ opacity: 0.6 });
    expect(spy).not.toHaveBeenCalled();
    await vi.advanceTimersByTimeAsync(TOOL_DEFAULTS_SAVE_MS + 10);
    expect(spy).toHaveBeenCalledTimes(1);
    expect(spy.mock.calls[0][0].patch.toolDefaults).toEqual({ recentsCount: 20, pen: { width: 12, opacity: 0.6 } });

    useAnnotStore.getState().resetToolDefault("pen");
    await vi.advanceTimersByTimeAsync(TOOL_DEFAULTS_SAVE_MS + 10);
    expect(spy.mock.calls[1][0].patch.toolDefaults).toEqual({ recentsCount: 20 });
    stop();
    spy.mockRestore();
  });
});
