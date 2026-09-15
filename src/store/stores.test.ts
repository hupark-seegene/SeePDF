import { describe, expect, it, beforeEach } from "vitest";
import { useAppStore, DEFAULT_TOOL } from "./appStore";
import { useViewStore } from "./viewStore";
import { useDocStore } from "./docStore";
import { usePagesStore } from "./pagesStore";
import { useJobStore } from "./jobStore";
import { useAnnotStore, PALETTE } from "./annotStore";

beforeEach(() => {
  useViewStore.setState({ zoomPercent: 100, zoomMode: "custom", rotation: 0, layout: "continuous", currentPage: 0 });
  usePagesStore.setState({ selected: [], lastAnchor: null });
  useJobStore.setState({ jobs: [], active: null });
  useAppStore.setState({ mode: "read", tool: "select", momentaryFrom: null, sidebarOpen: true, inspectorOpen: false });
});

describe("viewStore", () => {
  it("steps zoom through the status-bar ladder and clamps", () => {
    const { zoomIn, zoomOut, setZoom } = useViewStore.getState();
    zoomIn();
    expect(useViewStore.getState().zoomPercent).toBe(125);
    zoomOut();
    expect(useViewStore.getState().zoomPercent).toBe(100);
    setZoom(9999);
    expect(useViewStore.getState().zoomPercent).toBe(400);
    setZoom(1);
    expect(useViewStore.getState().zoomPercent).toBe(25);
  });

  it("rotates in 90° steps in both directions", () => {
    const { rotate } = useViewStore.getState();
    rotate(-90);
    expect(useViewStore.getState().rotation).toBe(270);
    rotate(90);
    expect(useViewStore.getState().rotation).toBe(0);
  });

  it("records a scroll request when jumping to a page", () => {
    useViewStore.getState().goToPage(4);
    expect(useViewStore.getState().currentPage).toBe(4);
    expect(useViewStore.getState().scrollRequest?.page).toBe(4);
  });
});

describe("appStore", () => {
  it("switches mode, resets the tool and keeps the inspector closed in 읽기", () => {
    const { setMode } = useAppStore.getState();
    setMode("annotate");
    expect(useAppStore.getState().tool).toBe(DEFAULT_TOOL.annotate);
    expect(useAppStore.getState().inspectorOpen).toBe(true);
    setMode("read");
    expect(useAppStore.getState().inspectorOpen).toBe(false);
  });

  it("reverts a momentary tool on release", () => {
    const s = useAppStore.getState();
    s.setTool("pen");
    s.setTool("hand", true);
    expect(useAppStore.getState().tool).toBe("hand");
    useAppStore.getState().releaseMomentary();
    expect(useAppStore.getState().tool).toBe("pen");
  });

  it("clamps the sidebar width to the 180–420 px range", () => {
    useAppStore.getState().setSidebarWidth(20);
    expect(useAppStore.getState().sidebarWidth).toBe(180);
    useAppStore.getState().setSidebarWidth(900);
    expect(useAppStore.getState().sidebarWidth).toBe(420);
  });
});

describe("docStore", () => {
  it("opens through the mock and applies doc-changed without a round trip", async () => {
    const info = await useDocStore.getState().open("/Users/veri/Documents/SeePDF-샘플.pdf");
    expect(info?.pageCount).toBe(3);
    expect(useDocStore.getState().status).toBe("ready");
    expect(useDocStore.getState().outline.length).toBeGreaterThan(0);

    useDocStore.getState().applyDocChanged({
      docId: info!.docId,
      docGeneration: 9,
      changedPages: [0],
      structure: false,
      dirty: true,
      reason: "edit", canUndo: true, canRedo: false,
    });
    expect(useDocStore.getState().info?.docGeneration).toBe(9);
    expect(useDocStore.getState().info?.dirty).toBe(true);
    await useDocStore.getState().close();
    expect(useDocStore.getState().status).toBe("empty");
  });
});

describe("pagesStore", () => {
  it("toggles, ranges and selects all", () => {
    const s = usePagesStore.getState();
    s.toggle(2, false);
    expect(usePagesStore.getState().selected).toEqual([2]);
    usePagesStore.getState().toggle(4, true);
    expect(usePagesStore.getState().selected).toEqual([2, 4]);
    usePagesStore.getState().selectRange(6);
    expect(usePagesStore.getState().selected).toEqual([4, 5, 6]);
    usePagesStore.getState().selectAll(3);
    expect(usePagesStore.getState().selected).toEqual([0, 1, 2]);
  });
});

describe("jobStore", () => {
  it("folds JobEvents into one status-bar job", () => {
    const { apply } = useJobStore.getState();
    apply("export", "export.progress", { type: "started", jobId: 1, total: 4 });
    expect(useJobStore.getState().active?.total).toBe(4);
    apply("export", "export.progress", { type: "progress", jobId: 1, done: 2, total: 4 });
    expect(useJobStore.getState().active?.done).toBe(2);
    apply("export", "export.progress", { type: "done", jobId: 1, elapsedMs: 10 });
    expect(useJobStore.getState().active).toBeNull();
    expect(useJobStore.getState().jobs[0].state).toBe("done");
  });
});

describe("annotStore", () => {
  it("keeps the 8-swatch palette and per-tool style defaults", () => {
    expect(PALETTE).toHaveLength(8);
    expect(PALETTE[0].rgb).toEqual([255, 216, 77]);
    useAnnotStore.getState().setStyle({ width: 8 });
    expect(useAnnotStore.getState().style.width).toBe(8);
  });
});
