/**
 * V2 (v0.3): the canvas context menus of UI_SPEC §12 against the mock — right-clicking an
 * annotation lists 편집 · 속성… · 메모 열기 · 복사 · 삭제 · 이 스타일을 기본값으로 (plus P2's 답글), and
 * 복사 then 붙여넣기 on an empty spot duplicates it there; 메모 추가 puts a note at the click's page
 * coordinates; 스냅샷 arms the V1 marquee.
 */
import { beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { act, render, waitFor } from "@testing-library/react";
import App from "../App";
import { mock } from "../ipc/mock";
import * as api from "../ipc/api";
import { useAppStore } from "../store/appStore";
import { useAnnotStore } from "../store/annotStore";
import { useDocStore } from "../store/docStore";
import { useViewStore } from "../store/viewStore";
import { useDialogStore } from "../dialogs/dialogState";
import { makePageLayerContext } from "../viewer";
import { warmLazyChunks } from "../test/warmLazy";
import { isSeparator, useContextMenuStore, type MenuItem } from "./contextMenuStore";
import { noteAtPoint, openPageContextMenu } from "./pageMenus";
import type { Point } from "../ipc/types";

const SAMPLE = "/Users/veri/Documents/SeePDF-샘플.pdf";
const HIGHLIGHT = "여기부터 확인";

beforeAll(() => warmLazyChunks());

beforeEach(() => {
  useAppStore.setState({ os: "macos", mode: "read", tool: "select" });
  useDialogStore.getState().closeAll();
  useViewStore.setState({ split: null, parked: null, focusedPane: "main", zoomPercent: 100, zoomMode: "custom", rotation: 0 });
  useDocStore.setState({ docId: null, info: null, status: "empty", error: null });
  useAnnotStore.getState().reset();
});

async function openSample() {
  render(<App />);
  await waitFor(() => expect(useAppStore.getState().ready).toBe(true), { timeout: 3000 });
  await act(async () => {
    await useDocStore.getState().open(SAMPLE);
  });
  await waitFor(() => expect(document.querySelector(".page-shell[data-page='0']")).not.toBeNull(), { timeout: 3000 });
  await waitFor(() => expect(useAnnotStore.getState().byPage[0]?.some((a) => a.contents === HIGHLIGHT)).toBe(true), {
    timeout: 4000,
  });
}

/** The client point over page point `pt` of page 0 (jsdom lays nothing out: the box is at 0, 0). */
function clientAt(pt: Point): { x: number; y: number; target: HTMLElement } {
  const info = useDocStore.getState().info!;
  const shell = document.querySelector<HTMLElement>(".page-shell[data-page='0']")!;
  const ctx = makePageLayerContext({
    docId: info.docId, docGeneration: info.docGeneration, page: info.pages[0], rotation: 0, zoomPercent: 100, width: 0, height: 0,
  });
  const [x, y] = ctx.toDevice(pt[0], pt[1]);
  return { x, y, target: shell };
}

function menuAt(pt: Point): MenuItem[] {
  const { x, y, target } = clientAt(pt);
  act(() => openPageContextMenu(0, "canvas", x, y, target));
  return (useContextMenuStore.getState().menu?.items ?? []).filter((i): i is MenuItem => !isSeparator(i));
}

const item = (items: MenuItem[], id: string) => items.find((i) => i.id === id)!;

describe("canvas context menus (UI_SPEC §12)", () => {
  it("an annotation's menu; 복사 then 붙여넣기 on an empty spot duplicates it there", async () => {
    await openSample();
    const highlight = useAnnotStore.getState().byPage[0].find((a) => a.contents === HIGHLIGHT)!;
    const items = menuAt([(highlight.rect.l + highlight.rect.r) / 2, (highlight.rect.b + highlight.rect.t) / 2]);
    const annotIds = ["editAnnot", "propsAnnot", "noteAnnot", "copyAnnot", "deleteAnnot", "defaultStyle"];
    expect(annotIds.every((id) => items.some((i) => i.id === id))).toBe(true);
    expect(annotIds.map((id) => item(items, id).labelKey)).toEqual([
      "canvasMenu.edit", "canvasMenu.properties", "canvasMenu.openNote", "menu.edit.copy", "common.delete", "prop.setDefault",
    ]);
    expect(items.some((i) => i.id === "replyAnnot")).toBe(true);
    // an annotation's menu has no empty-area items
    expect(items.some((i) => i.id === "pasteHere")).toBe(false);
    // nothing copied yet: an empty spot's 붙여넣기 is there but disabled
    expect(item(menuAt([300, 200]), "pasteHere").disabled).toBe(true);

    act(() => item(items, "copyAnnot").onSelect?.());
    const empty = menuAt([300, 200]);
    expect(item(empty, "pasteHere").disabled).toBe(false);
    const create = vi.spyOn(mock, "createAnnotation");
    act(() => item(empty, "pasteHere").onSelect?.());
    await waitFor(() => expect(create).toHaveBeenCalledTimes(1));
    const { page, spec } = create.mock.calls[0][0];
    expect(page).toBe(0);
    expect(spec).toMatchObject({ kind: "highlight", contents: HIGHLIGHT, color: highlight.color });
    // the copy's top-left is the clicked point; its size is the original's
    const rects = (spec as { rects: { l: number; b: number; r: number; t: number }[] }).rects;
    expect(rects[0].l).toBeCloseTo(300, 3);
    expect(rects[0].t).toBeCloseTo(200, 3);
    expect(rects[0].r - rects[0].l).toBeCloseTo(highlight.rect.r - highlight.rect.l, 3);
    await waitFor(() => expect(useAnnotStore.getState().byPage[0].filter((a) => a.contents === HIGHLIGHT)).toHaveLength(2));
  });

  it("속성… selects the annotation and opens the inspector in 주석; 이 스타일을 기본값으로 sets its tool's default", async () => {
    await openSample();
    const highlight = useAnnotStore.getState().byPage[0].find((a) => a.contents === HIGHLIGHT)!;
    const at: Point = [(highlight.rect.l + highlight.rect.r) / 2, (highlight.rect.b + highlight.rect.t) / 2];
    act(() => item(menuAt(at), "propsAnnot").onSelect?.());
    expect(useAppStore.getState()).toMatchObject({ mode: "annotate", inspectorOpen: true, tool: "select" });
    expect(useAnnotStore.getState().selected).toEqual([highlight.id]);

    act(() => item(menuAt(at), "defaultStyle").onSelect?.());
    expect(useAnnotStore.getState().toolDefaults.highlight).toMatchObject({ color: highlight.color, opacity: highlight.opacity });
  });

  it("메모 추가 puts a note at the click's page coordinates, opened for typing", async () => {
    await openSample();
    const items = menuAt([250, 400]);
    expect(item(items, "noteHere").labelKey).toBe("textMenu.addNote");
    const create = vi.spyOn(mock, "createAnnotation");
    act(() => item(items, "noteHere").onSelect?.());
    await waitFor(() => expect(create).toHaveBeenCalledTimes(1));
    const call = create.mock.calls[0][0];
    expect(call.page).toBe(0);
    expect(call.spec).toMatchObject({ kind: "note", contents: "" });
    const at = (call.spec as { at: Point }).at;
    expect(at[0]).toBeCloseTo(250, 3);
    expect(at[1]).toBeCloseTo(400, 3);
    expect(useAppStore.getState().mode).toBe("annotate");
    await waitFor(() => expect(useAnnotStore.getState().editing).not.toBeNull());
    // kept on the page near an edge
    const crop = useDocStore.getState().info!.pages[0].crop;
    expect(noteAtPoint([crop.r + 50, crop.b - 50], crop)).toEqual([crop.r - 22, crop.b + 22]);
  });

  it("스냅샷 arms the marquee tool", async () => {
    await openSample();
    const snapshot = item(menuAt([100, 100]), "snapshot");
    expect(snapshot.labelKey).toBe("tool.snapshot");
    act(() => snapshot.onSelect?.());
    expect(useAppStore.getState().tool).toBe("snapshot");
  });

  // v0.3 integration (pkg6 V2 × pkg4 A2): a callout is a text box with a leader — 편집 opens its
  // editor as double-clicking it does, and 이 스타일을 기본값으로 reaches the 설명선 tool with its fill
  it("편집 on a pkg4 callout opens its text editor; 이 스타일을 기본값으로 sets the 설명선 default", async () => {
    await openSample();
    const docId = useDocStore.getState().docId!;
    const rect = { l: 380, b: 120, r: 500, t: 170 };
    const made = await act(() =>
      api.createAnnotation({
        docId,
        page: 0,
        spec: {
          kind: "callout", rect, text: "확인 필요", fontSize: 12, color: [43, 47, 51], align: "center",
          fillColor: [255, 244, 200], callout: [330, 90, rect.l, 145],
        },
      }),
    );
    const callout = made.annot!;
    expect(callout.kind).toBe("callout");
    act(() => useAnnotStore.getState().setPage(0, made.list.annots, made.list.docGeneration));
    const at: Point = [(rect.l + rect.r) / 2, (rect.b + rect.t) / 2];
    // the editor may commit and close again at once in jsdom (focus): record what was opened
    const opened: unknown[] = [];
    const stop = useAnnotStore.subscribe((s) => s.editing && opened.push(s.editing));
    act(() => item(menuAt(at), "editAnnot").onSelect?.());
    stop();
    expect(useAppStore.getState().mode).toBe("annotate");
    expect(opened).toContainEqual({ page: 0, id: callout.id });

    act(() => item(menuAt(at), "defaultStyle").onSelect?.());
    expect(useAnnotStore.getState().toolDefaults.callout).toMatchObject({ color: [43, 47, 51], fillColor: [255, 244, 200] });
  });
});
