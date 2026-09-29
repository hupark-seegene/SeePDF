/**
 * v0.3 pkg4-annotations-stamps-objects — 편집 mode against the mock: E1 이미지 바꾸기 (panel and
 * canvas menu), A6 비율 고정 and 맨 앞으로 / 맨 뒤로.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import * as api from "../ipc/api";
import { mock, resetMock } from "../ipc/mock";
import { useAppStore } from "../store/appStore";
import { useDocStore } from "../store/docStore";
import { useViewStore } from "../store/viewStore";
import { useToastStore } from "../app/toastStore";
import { useContextMenuStore } from "../app/contextMenuStore";
import { EditPanel } from "../app/Inspector/EditPanel";
import { useEditStore } from "./editStore";
import { editHost } from "./index";
import { lockedSize, loadPage } from "./actions";

let stop: (() => void) | null = null;

const objects = (page = 0) => useEditStore.getState().pages[page]?.objects ?? [];

async function setupWithImage() {
  const info = await useDocStore.getState().open("/tmp/sample.pdf");
  if (!info) throw new Error("mock open failed");
  useAppStore.setState({ mode: "edit", tool: "select" });
  useViewStore.setState({ currentPage: 0 });
  stop = editHost.start();
  // an image object to work on (the mock's image is 4:3 → 200 × 150 pt)
  const added = await api.addImageObject({ docId: info.docId, page: 0, rect: { l: 100, b: 100, r: 300, t: 250 }, path: "/pics/a.png", keepAspect: true });
  await loadPage(info.docId, 0, added.docGeneration);
  await waitFor(() => expect(objects().some((o) => o.type === "image")).toBe(true));
  const image = objects().find((o) => o.type === "image")!;
  return { info, image };
}

beforeEach(() => {
  resetMock();
  useEditStore.getState().reset();
  useToastStore.setState({ toasts: [] });
});

afterEach(() => {
  stop?.();
  stop = null;
  vi.restoreAllMocks();
});

describe("E1 — 이미지 바꾸기", () => {
  it("the panel button calls replace_image with the selected object id; undo restores the page", async () => {
    const { info, image } = await setupWithImage();
    const before = structuredClone(objects());
    act(() => useEditStore.getState().select(0, [image.objectId]));
    vi.spyOn(api, "openFileDialog").mockResolvedValue(["/pics/new-logo.png"]);
    const spy = vi.spyOn(mock, "replaceImage");
    render(<EditPanel />);
    fireEvent.click(screen.getByRole("button", { name: "이미지 바꾸기…" }));
    await waitFor(() => expect(spy).toHaveBeenCalledTimes(1));
    expect(spy.mock.calls[0][0]).toMatchObject({ docId: info.docId, page: 0, objectId: image.objectId, path: "/pics/new-logo.png" });
    const doc = await api.getDocument({ docId: info.docId });
    expect(doc.canUndo).toBe(true);
    expect(doc.undoLabel).toBe("undo.objectEdit");
    // position and size are kept
    expect(objects().find((o) => o.objectId === image.objectId)?.rect).toEqual(image.rect);
    await api.undo({ docId: info.docId });
    const restored = await api.listPageObjects({ docId: info.docId, page: 0 });
    expect(restored.objects).toEqual(before);
  });

  it("is offered only for exactly one image object", async () => {
    const { image } = await setupWithImage();
    const text = objects().find((o) => o.type === "text")!;
    act(() => useEditStore.getState().select(0, [text.objectId]));
    const view = render(<EditPanel />);
    expect(screen.queryByRole("button", { name: "이미지 바꾸기…" })).toBeNull();
    act(() => useEditStore.getState().select(0, [text.objectId, image.objectId]));
    view.rerender(<EditPanel />);
    expect(screen.queryByRole("button", { name: "이미지 바꾸기…" })).toBeNull();
  });

  it("the canvas menu offers it over an image object in 편집 mode", async () => {
    const { info, image } = await setupWithImage();
    const shell = document.createElement("div");
    shell.className = "page-shell";
    shell.dataset.page = "0";
    const geom = info.pages[0];
    shell.getBoundingClientRect = () => ({ left: 0, top: 0, width: geom.widthPt, height: geom.heightPt, right: geom.widthPt, bottom: geom.heightPt, x: 0, y: 0, toJSON: () => ({}) });
    document.body.appendChild(shell);
    const { openPageContextMenu } = await import("../app/pageMenus");
    // client point over the image centre (y flips: client y = height − pdf y at 100 %)
    const cx = (image.rect.l + image.rect.r) / 2;
    const cy = geom.heightPt - (image.rect.b + image.rect.t) / 2;
    openPageContextMenu(0, "canvas", cx, cy, shell);
    const items = useContextMenuStore.getState().menu?.items ?? [];
    expect(items.some((i) => "labelKey" in i && i.labelKey === "edit.replaceImage")).toBe(true);
    // not over the image: no entry
    openPageContextMenu(0, "canvas", 5, 5, shell);
    const again = useContextMenuStore.getState().menu?.items ?? [];
    expect(again.some((i) => "labelKey" in i && i.labelKey === "edit.replaceImage")).toBe(false);
    shell.remove();
  });
});

describe("A6 — 비율 고정, 맨 앞으로 / 맨 뒤로", () => {
  it("typing a width with 비율 고정 on (the default for an image) scales the height with it", async () => {
    const { image } = await setupWithImage();
    act(() => useEditStore.getState().select(0, [image.objectId]));
    const spy = vi.spyOn(mock, "transformObject");
    render(<EditPanel />);
    expect(screen.getByRole("checkbox", { name: "비율 고정" })).toBeChecked();
    const w = screen.getByLabelText("너비");
    fireEvent.change(w, { target: { value: "100" } });
    fireEvent.keyDown(w, { key: "Enter" });
    await waitFor(() => expect(spy).toHaveBeenCalled());
    const [sx, sy] = spy.mock.calls[0][0].scale!;
    expect(sx).toBeCloseTo(0.5, 3);
    expect(sy).toBeCloseTo(0.5, 3);
    expect(lockedSize({ l: 0, b: 0, r: 200, t: 150 }, { h: 30 })).toEqual({ w: 40, h: 30 });
  });

  it("맨 뒤로 moves the selection to index 0 — one undo step — and the selection follows", async () => {
    const { info, image } = await setupWithImage();
    act(() => useEditStore.getState().select(0, [image.objectId]));
    render(<EditPanel />);
    fireEvent.click(screen.getByRole("button", { name: "맨 뒤로" }));
    await waitFor(() => expect(objects()[0].type).toBe("image"));
    expect(useEditStore.getState().selection).toEqual({ page: 0, ids: [0] });
    const doc = await api.getDocument({ docId: info.docId });
    expect(doc.undoLabel).toBe("undo.objectArrange");
    fireEvent.click(screen.getByRole("button", { name: "맨 앞으로" }));
    await waitFor(() => expect(objects().at(-1)?.type).toBe("image"));
  });
});
