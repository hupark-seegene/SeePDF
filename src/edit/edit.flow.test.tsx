/**
 * 편집 mode flows against the mock adapter (Stage 7): select + move + scale + delete, 문단 편집
 * with the font-substitution confirm, Esc cancel, 텍스트 추가, 이미지 추가. The Stage 9 flow of the
 * content below an edited paragraph lives in `flow.flow.test.tsx`.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import * as api from "../ipc/api";
import { mock } from "../ipc/mock";
import { makePageLayerContext, type PageLayerContext } from "../viewer";
import { useAppStore, type ToolId } from "../store/appStore";
import { useDocStore } from "../store/docStore";
import { useToastStore } from "../app/toastStore";
import { useDialogStore } from "../dialogs/dialogState";
import { useEditStore } from "./editStore";
import { EditLayer } from "./EditLayer";
import { editHost } from "./index";

vi.mock("../ipc/api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../ipc/api")>();
  return { ...actual, openFileDialog: vi.fn() };
});

let stop: (() => void) | null = null;

async function setup(tool: ToolId = "select") {
  const info = await useDocStore.getState().open("/tmp/sample.pdf");
  if (!info) throw new Error("mock open failed");
  useAppStore.setState({ mode: "edit", tool });
  stop = editHost.start();
  const page = info.pages[0];
  const ctx = makePageLayerContext({
    docId: info.docId, docGeneration: info.docGeneration, page, rotation: 0, zoomPercent: 100,
    width: page.widthPt, height: page.heightPt,
  });
  const view = render(<EditLayer ctx={ctx} />);
  await waitFor(() => expect(useEditStore.getState().pages[0]).toBeDefined());
  const surface = view.container.querySelector(".edit-surface") as HTMLElement;
  return { info, ctx, surface, ...view };
}

/** client coordinates of a page point (the surface sits at 0,0 in jsdom) */
function at(ctx: PageLayerContext, x: number, y: number) {
  const [cx, cy] = ctx.toDevice(x, y);
  return { clientX: cx, clientY: cy, pointerId: 1, button: 0 };
}

function objects() {
  return useEditStore.getState().pages[0]?.objects ?? [];
}

function toastKeys(): string[] {
  return useToastStore.getState().toasts.map((t) => t.messageKey);
}

function confirmTop() {
  const top = useDialogStore.getState().stack.at(-1);
  return top?.name === "confirm" ? (top.props.resolve as (v: boolean) => void) : null;
}

beforeEach(() => {
  useEditStore.getState().reset();
  useToastStore.setState({ toasts: [] });
  useDialogStore.getState().closeAll();
});

afterEach(() => {
  // an editor left open would be committed by stop(); answer any prompt so nothing hangs
  useEditStore.getState().closeSession();
  confirmTop()?.(false);
  stop?.();
  stop = null;
  vi.restoreAllMocks();
});

describe("편집 · 선택", () => {
  it("selects, moves with one transform on drop, scales from a corner and deletes", async () => {
    const { ctx, surface, container } = await setup();
    const transform = vi.spyOn(mock, "transformObject");
    const line = objects().find((o) => o.text?.startsWith("Lightweight"));
    expect(line).toBeDefined();

    fireEvent.pointerDown(surface, at(ctx, 100, 730));
    expect(useEditStore.getState().selection).toEqual({ page: 0, ids: [line!.objectId] });
    fireEvent.pointerMove(surface, { ...at(ctx, 100, 730), clientX: at(ctx, 100, 730).clientX + 20, clientY: at(ctx, 100, 730).clientY + 10 });
    expect(transform).not.toHaveBeenCalled();
    fireEvent.pointerUp(surface, at(ctx, 120, 720));
    await waitFor(() => expect(transform).toHaveBeenCalledTimes(1));
    expect(transform.mock.calls[0][0]).toMatchObject({ objectId: line!.objectId, translate: [20, -10] });
    await waitFor(() => expect(objects().find((o) => o.objectId === line!.objectId)?.rect.l).toBeCloseTo(92));
    // our own move keeps the selection (ids are unchanged)
    expect(useEditStore.getState().selection?.ids).toEqual([line!.objectId]);

    // corner handle → anchored scale + translate in one call
    const handles = container.querySelectorAll(".edit-handle");
    expect(handles).toHaveLength(4);
    const rt = container.querySelector('.edit-handle[data-corner="rt"]') as HTMLElement;
    const before = objects().find((o) => o.objectId === line!.objectId)!.rect;
    const corner = at(ctx, before.r, before.t);
    fireEvent.pointerDown(rt, corner);
    fireEvent.pointerMove(surface, { ...corner, clientX: corner.clientX + 30 });
    fireEvent.pointerUp(surface, corner);
    await waitFor(() => expect(transform).toHaveBeenCalledTimes(2));
    const scaleCall = transform.mock.calls[1][0];
    expect(scaleCall.scale?.[0]).toBeCloseTo((before.r - before.l + 30) / (before.r - before.l));
    expect(scaleCall.scale?.[1]).toBeCloseTo(1);

    // ⌫ deletes
    const count = objects().length;
    fireEvent.keyDown(window, { key: "Backspace" });
    await waitFor(() => expect(objects()).toHaveLength(count - 1));
    expect(useEditStore.getState().selection).toBeNull();
    expect(objects().some((o) => o.text?.startsWith("Lightweight"))).toBe(false);
  });

  it("a read-only object shows its badge and refuses move and delete", async () => {
    const { ctx, surface } = await setup();
    const transform = vi.spyOn(mock, "transformObject");
    const del = vi.spyOn(mock, "deleteObjects");
    fireEvent.pointerDown(surface, at(ctx, 100, 765));
    expect(useEditStore.getState().selection?.ids).toEqual([0]);
    expect(screen.getByText("읽기 전용")).toBeInTheDocument();
    fireEvent.pointerMove(surface, { ...at(ctx, 100, 765), clientX: 300 });
    fireEvent.pointerUp(surface, at(ctx, 100, 765));
    fireEvent.keyDown(window, { key: "Delete" });
    await act(async () => undefined);
    expect(transform).not.toHaveBeenCalled();
    expect(del).not.toHaveBeenCalled();
    expect(toastKeys()).toContain("edit.readOnly.xobject");
  });
});

describe("편집 · 텍스트 수정", () => {
  it("edits a paragraph: asks before the Hangul font, retries the dry run with consent, then writes", async () => {
    const { ctx, surface } = await setup("editText");
    const edit = vi.spyOn(mock, "editParagraph");
    fireEvent.pointerDown(surface, at(ctx, 100, 642));
    const box = (await screen.findByLabelText("문단 편집")) as HTMLTextAreaElement;
    expect(box.value).toBe("The mock text layer carries real word boxes in PDF points. Search for PDF, 한국어 or mock to see streaming hits.");
    expect(document.querySelector(".edit-mask")).not.toBeNull();

    box.value = "새로 쓴 문단입니다. ".repeat(12).trim();
    fireEvent.click(screen.getByRole("button", { name: "완료" }));

    await waitFor(() => expect(confirmTop()).not.toBeNull());
    expect(edit).toHaveBeenCalledTimes(1);
    expect(edit.mock.calls[0][0]).toMatchObject({ allowFontSubstitution: false, edit: { flow: "push", dryRun: true } });
    act(() => confirmTop()!(true));

    await waitFor(() => expect(edit).toHaveBeenCalledTimes(3));
    expect(edit.mock.calls[1][0]).toMatchObject({ allowFontSubstitution: true, edit: { flow: "push", dryRun: true } });
    expect(edit.mock.calls[2][0]).toMatchObject({ allowFontSubstitution: true, edit: { flow: "push" } });
    expect(edit.mock.calls[2][0].edit.dryRun).toBeUndefined();
    await waitFor(() => expect(useEditStore.getState().session).toBeNull());
    // the last paragraph of the page: nothing below to move, so nothing to report
    expect(toastKeys().filter((k) => k.startsWith("edit.flow."))).toEqual([]);
    expect(objects().filter((o) => o.fontName === "SeePDF Hangul").length).toBeGreaterThan(2);
  });

  it("Esc cancels without writing; declining the font keeps the editor open", async () => {
    const { ctx, surface } = await setup("editText");
    const edit = vi.spyOn(mock, "editParagraph");
    fireEvent.pointerDown(surface, at(ctx, 100, 642));
    let box = (await screen.findByLabelText("문단 편집")) as HTMLTextAreaElement;
    box.value = "changed";
    fireEvent.keyDown(box, { key: "Escape" });
    expect(useEditStore.getState().session).toBeNull();
    expect(screen.queryByLabelText("문단 편집")).toBeNull();
    expect(edit).not.toHaveBeenCalled();

    fireEvent.pointerDown(surface, at(ctx, 100, 642));
    box = (await screen.findByLabelText("문단 편집")) as HTMLTextAreaElement;
    box.value = "가나다";
    fireEvent.keyDown(box, { key: "Enter", metaKey: true });
    await waitFor(() => expect(confirmTop()).not.toBeNull());
    act(() => confirmTop()!(false));
    await act(async () => undefined);
    expect(edit).toHaveBeenCalledTimes(1);
    expect(useEditStore.getState().session).not.toBeNull();
  });

  it("a probe that already needs the substitute font asks before the first write", async () => {
    const { ctx, surface } = await setup("editText");
    const real = mock.probeParagraph.bind(mock);
    vi.spyOn(mock, "probeParagraph").mockImplementation(async (a) => {
      const p = await real(a);
      return p && { ...p, strategy: "replaceFont", substituteFont: "SeePDF Hangul" };
    });
    const edit = vi.spyOn(mock, "editParagraph");
    fireEvent.pointerDown(surface, at(ctx, 100, 642));
    const box = (await screen.findByLabelText("문단 편집")) as HTMLTextAreaElement;
    box.value = "Short.";
    fireEvent.click(screen.getByRole("button", { name: "완료" }));
    await waitFor(() => expect(confirmTop()).not.toBeNull());
    expect(edit).not.toHaveBeenCalled();
    act(() => confirmTop()!(true));
    await waitFor(() => expect(edit).toHaveBeenCalledTimes(2));
    expect(edit.mock.calls[0][0]).toMatchObject({ allowFontSubstitution: true, edit: { dryRun: true } });
    expect(edit.mock.calls[1][0].allowFontSubstitution).toBe(true);
    await waitFor(() => expect(useEditStore.getState().session).toBeNull());
    expect(toastKeys().filter((k) => k.startsWith("edit.flow."))).toEqual([]);
  });

  // v0.3 pkg1 (R4): a run inside a group asks 그룹 해제 후 편집할까요? instead of only refusing
  it("a paragraph inside a group asks to ungroup: 취소 opens nothing, 그룹 해제 ungroups and opens the editor", async () => {
    const { ctx, surface } = await setup("editText");
    const ungroup = vi.spyOn(mock, "ungroupObject");
    fireEvent.pointerDown(surface, at(ctx, 100, 765));
    await waitFor(() => expect(confirmTop()).not.toBeNull());
    expect(useDialogStore.getState().stack.at(-1)!.props).toMatchObject({ titleKey: "edit.ungroup.title" });
    act(() => confirmTop()!(false));
    await act(async () => undefined);
    expect(ungroup).not.toHaveBeenCalled();
    expect(useEditStore.getState().session).toBeNull();

    fireEvent.pointerDown(surface, at(ctx, 100, 765));
    await waitFor(() => expect(confirmTop()).not.toBeNull());
    act(() => confirmTop()!(true));
    await waitFor(() => expect(useEditStore.getState().session?.kind).toBe("paragraph"));
    expect(ungroup).toHaveBeenCalledTimes(1);
    expect(ungroup.mock.calls[0][0]).toMatchObject({ page: 0, objectId: 0 });
    const info = await api.getDocument({ docId: useDocStore.getState().info!.docId });
    expect(info.undoLabel).toBe("undo.ungroup");
    expect(objects()[0].editable).toBe("full");
  });
});

describe("편집 · 추가", () => {
  it("텍스트 추가: click, type, 완료 → add_text_object at the click", async () => {
    const { ctx, surface } = await setup("addText");
    const add = vi.spyOn(mock, "addTextObject");
    const count = objects().length;
    fireEvent.pointerDown(surface, at(ctx, 72, 500));
    const box = (await screen.findByLabelText("새 텍스트")) as HTMLTextAreaElement;
    expect(box.value).toBe("");
    box.value = "안녕하세요";
    fireEvent.click(screen.getByRole("button", { name: "완료" }));
    await waitFor(() => expect(add).toHaveBeenCalledTimes(1));
    const args = add.mock.calls[0][0];
    expect(args).toMatchObject({ text: "안녕하세요", fontSizePt: 12, color: [0, 0, 0], align: "left" });
    expect(args.rect.l).toBeCloseTo(72);
    expect(args.rect.t).toBeCloseTo(500);
    await waitFor(() => expect(objects()).toHaveLength(count + 1));
  });

  it("이미지 추가: a click places a default box, a drag uses the box; cancel places nothing", async () => {
    const { ctx, surface } = await setup("addImage");
    const add = vi.spyOn(mock, "addImageObject");
    vi.mocked(api.openFileDialog).mockResolvedValue(["/Users/veri/Pictures/photo.png"]);
    fireEvent.pointerDown(surface, at(ctx, 100, 700));
    fireEvent.pointerUp(surface, at(ctx, 100, 700));
    await waitFor(() => expect(add).toHaveBeenCalledTimes(1));
    expect(add.mock.calls[0][0]).toMatchObject({ path: "/Users/veri/Pictures/photo.png", keepAspect: true, rect: { l: 100, t: 700, r: 340, b: 460 } });

    fireEvent.pointerDown(surface, at(ctx, 100, 400));
    fireEvent.pointerMove(surface, at(ctx, 300, 250));
    fireEvent.pointerUp(surface, at(ctx, 300, 250));
    await waitFor(() => expect(add).toHaveBeenCalledTimes(2));
    const rect = add.mock.calls[1][0].rect;
    expect(rect.l).toBeCloseTo(100);
    expect(rect.r).toBeCloseTo(300);
    expect(rect.b).toBeCloseTo(250);
    expect(rect.t).toBeCloseTo(400);
    await waitFor(() => expect(objects().filter((o) => o.type === "image")).toHaveLength(2));

    vi.mocked(api.openFileDialog).mockResolvedValue(null);
    fireEvent.pointerDown(surface, at(ctx, 100, 700));
    fireEvent.pointerUp(surface, at(ctx, 100, 700));
    await act(async () => undefined);
    expect(add).toHaveBeenCalledTimes(2);
  });
});
