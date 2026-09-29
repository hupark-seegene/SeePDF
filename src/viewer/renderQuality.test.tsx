/**
 * U3 (v0.3): 렌더링 품질 takes effect — 고품질 renders the bitmaps 1.5× denser (capped) and skips the
 * low-resolution draft of a whole page; 설정 › 고급 캐시 비우기 and 기본값으로 되돌리기 work.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { Scroller } from "./Scroller";
import { resetPanes } from "./panes";
import { renderDpr, setMockAssetResolver } from "../ipc/protocol";
import { mock, mockAssetUrl, mockRenderCache } from "../ipc/mock";
import * as api from "../ipc/api";
import { useAppStore } from "../store/appStore";
import { useAnnotStore } from "../store/annotStore";
import { useViewStore } from "../store/viewStore";
import { useDialogStore } from "../dialogs/dialogState";
import { useToastStore } from "../app/toastStore";
import { SettingsDialog, resetPatch } from "../dialogs/SettingsDialog";
import type { DocInfo, PageGeom, Settings } from "../ipc/types";

function geom(index: number): PageGeom {
  return { index, widthPt: 400, heightPt: 600, rotation: 0, crop: { l: 0, b: 0, r: 400, t: 600 }, label: null };
}

const INFO: DocInfo = {
  docId: "d-q", path: "/tmp/q.pdf", name: "q.pdf", bytes: 1, pageCount: 3, pages: [geom(0), geom(1), geom(2)],
  docGeneration: 1, dirty: false, canUndo: false, canRedo: false, undoLabel: null, redoLabel: null, encrypted: false,
  permissions: { print: true, modify: true, extractText: true, annotate: true, fillForms: true, assemble: true, revision: "unprotected" },
  hasForm: false, xfa: false, hasOutline: false, meta: {}, pdfVersion: "1.7", tagged: false,
};

function withQuality(quality: Settings["renderQuality"]) {
  const base = useAppStore.getState().settings ?? ({} as Settings);
  useAppStore.setState({ settings: { ...base, renderQuality: quality } as Settings });
}

beforeEach(() => {
  useViewStore.setState({
    zoomPercent: 100, zoomMode: "custom", layout: "continuous", rotation: 0, currentPage: 0, scrollRequest: null,
    split: null, parked: null, focusedPane: "main",
  });
  // the page URLs, as the engine would receive them
  setMockAssetResolver((route, q) => `${route}?sk=${q.sk}&page=${q.page}`);
});

afterEach(() => {
  setMockAssetResolver(mockAssetUrl);
  resetPanes();
  vi.restoreAllMocks();
});

describe("렌더링 품질", () => {
  it("renderDpr oversamples 고품질 by 1.5, capped at 3, and leaves 균형 alone", () => {
    expect(renderDpr(1, "balanced")).toBe(1);
    expect(renderDpr(2, undefined)).toBe(2);
    expect(renderDpr(1, "high")).toBe(1.5);
    expect(renderDpr(2, "high")).toBe(3);
    expect(renderDpr(3, "high")).toBe(3);
  });

  it("고품질 changes the tile scale parameter and skips the draft pass; 균형 keeps today's URLs", async () => {
    // 120 %: the whole-page bitmap (sk 120) differs from the 1× draft (sk 100)
    useViewStore.setState({ zoomPercent: 120 });
    withQuality("balanced");
    const { container, unmount } = render(<Scroller info={INFO} />);
    const bitmap = () => container.querySelector<HTMLImageElement>(".page-shell[data-page='0'] .page-bitmap")!;
    const draft = () => container.querySelector<HTMLImageElement>(".page-shell[data-page='0'] .ph")!;
    await waitFor(() => expect(bitmap()).not.toBeNull());
    expect(bitmap().getAttribute("src")).toContain("sk=120");
    expect(draft().getAttribute("src")).toContain("sk=100");
    unmount();

    withQuality("high");
    const high = render(<Scroller info={INFO} />);
    const hb = () => high.container.querySelector<HTMLImageElement>(".page-shell[data-page='0'] .page-bitmap")!;
    await waitFor(() => expect(hb()?.getAttribute("src")).toContain("sk=180"));
    // one render, not two: the placeholder is the sharp bitmap itself
    expect(high.container.querySelector(".page-shell[data-page='0'] .ph")?.getAttribute("src")).toBe(hb().getAttribute("src"));
    // the page box is the same size: the denser bitmap is CSS-scaled into it
    const layer = high.container.querySelector<HTMLElement>(".page-shell[data-page='0'] .page-bitmaps")!;
    expect(layer.style.transform).toMatch(/scale\(0\.66/);
  });
});

describe("설정 › 고급", () => {
  beforeEach(async () => {
    await useAppStore.getState().bootstrap();
    useToastStore.setState({ toasts: [] });
    useDialogStore.getState().closeAll();
  });

  it("캐시 비우기 empties the engine's tile cache and says how much it freed", async () => {
    render(<SettingsDialog onClose={() => undefined} />);
    fireEvent.click(screen.getByRole("tab", { name: "고급" }));
    const clear = vi.spyOn(api, "clearRenderCache");
    fireEvent.click(screen.getByRole("button", { name: "캐시 비우기" }));
    await waitFor(() => expect(clear).toHaveBeenCalledTimes(1));
    expect(mockRenderCache.clears).toBe(1);
    await waitFor(() => expect(useToastStore.getState().toasts.at(-1)?.messageKey).toBe("settings.clearCache.done"));
    expect(useToastStore.getState().toasts.at(-1)?.params).toEqual({ size: "12 MB" });
  });

  it("캐시 크기 is written like every setting (the engine applies it at once)", async () => {
    render(<SettingsDialog onClose={() => undefined} />);
    fireEvent.click(screen.getByRole("tab", { name: "고급" }));
    const set = vi.spyOn(mock, "setSettings");
    fireEvent.change(screen.getByLabelText("캐시 크기"), { target: { value: "128" } });
    await waitFor(() => expect(set).toHaveBeenCalledWith({ patch: { tileCacheMb: 128 } }));
  });

  it("기본값으로 되돌리기 asks, then resets everything but the author, signatures and stamps", async () => {
    await useAppStore.getState().patchSettings({
      theme: "dark", renderQuality: "high", tileCacheMb: 256, author: "박현우", night: "sepia",
      toolDefaults: { highlight: { color: [1, 2, 3] } },
      signatures: [{ kind: "typed", id: "s1", text: "박현우", style: "pen", createdAt: "" }],
    });
    useAnnotStore.getState().loadToolDefaults({ highlight: { color: [1, 2, 3] } });
    render(<SettingsDialog onClose={() => undefined} />);
    fireEvent.click(screen.getByRole("tab", { name: "고급" }));
    fireEvent.click(screen.getByRole("button", { name: "기본값으로 되돌리기" }));
    const prompt = await waitFor(() => {
      const entry = useDialogStore.getState().stack.find((e) => e.name === "confirm");
      expect(entry).toBeDefined();
      return entry!;
    });
    expect(prompt.props.titleKey).toBe("settings.resetDefaults.title");
    await act(async () => (prompt.props.resolve as (v: boolean) => void)(true));
    await waitFor(() => expect(useToastStore.getState().toasts.at(-1)?.messageKey).toBe("settings.resetDefaults.done"));
    const s = useAppStore.getState().settings!;
    expect(s).toMatchObject({ theme: "system", renderQuality: "balanced", tileCacheMb: 64, night: "off", toolDefaults: {} });
    expect(s.author).toBe("박현우");
    expect(s.signatures).toHaveLength(1);
    expect(useAppStore.getState().theme).toBe("system");
    expect(useAnnotStore.getState().toolDefaults).toEqual({});
  });

  it("resetPatch never carries the kept keys", async () => {
    const defaults = await api.getDefaultSettings();
    const patch = resetPatch({ ...defaults, stamps: [] } as unknown as Settings) as Record<string, unknown>;
    expect("author" in patch || "signatures" in patch || "stamps" in patch).toBe(false);
    expect(patch.locale).toBe("ko");
  });
});
