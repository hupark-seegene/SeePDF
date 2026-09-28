/**
 * P2 자르기 / 페이지 크기 변경 against the mock adapter: the dialogs open from the organizer rail,
 * margins typed or dragged become one `set_page_boxes { crop: { margins } }`, 여백 자동 감지 reads a
 * render, 원래대로 sends `crop: null`, and 페이지 크기 변경 sends one `resize_pages` — each one undo step
 * with a 실행 취소 toast.
 */
import { beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import DialogHost from "../dialogs/DialogHost";
import { openDialog, useDialogStore } from "../dialogs/dialogState";
import { useDocStore } from "../store/docStore";
import { usePagesStore } from "../store/pagesStore";
import { useAppStore } from "../store/appStore";
import { useToastStore } from "../app/toastStore";
import { encodeRawPage } from "../ipc/binary";
import { mock } from "../ipc/mock";
import { Organizer } from "./Organizer";

const SAMPLE = "/Users/veri/Documents/SeePDF-샘플.pdf"; // 3 A4 pages

beforeEach(() => {
  useDialogStore.getState().closeAll();
  useToastStore.setState({ toasts: [] });
  usePagesStore.getState().reset();
  vi.restoreAllMocks();
});

async function openCrop(pages?: number[]) {
  await useDocStore.getState().open(SAMPLE);
  render(<DialogHost />);
  openDialog("crop", pages ? { pages } : {});
  return screen.findByLabelText("위");
}

const lastToast = () => useToastStore.getState().toasts.at(-1);

describe("organize.crop.flow", () => {
  it("the rail opens 자르기 and 페이지 크기 변경 on the selection", async () => {
    await useDocStore.getState().open(SAMPLE);
    useAppStore.setState({ mode: "pages" });
    usePagesStore.getState().setSelected([1, 2]);
    render(<Organizer />);
    fireEvent.click(screen.getByRole("button", { name: "자르기…" }));
    expect(useDialogStore.getState().stack.at(-1)).toMatchObject({ name: "crop", props: { pages: [1, 2] } });
    fireEvent.click(screen.getByRole("button", { name: "페이지 크기 변경…" }));
    expect(useDialogStore.getState().stack.at(-1)).toMatchObject({ name: "resize", props: { pages: [1, 2] } });
  });

  it("typed margins crop the selected pages in one 페이지 자르기 step", async () => {
    const spy = vi.spyOn(mock, "setPageBoxes");
    const top = await openCrop([0, 1]);
    const apply = screen.getByRole("button", { name: "적용" });
    expect(apply).toBeDisabled(); // the whole page: nothing to crop
    expect(screen.getByRole("radio", { name: "선택한 페이지 (2쪽)" })).toBeChecked();

    fireEvent.change(top, { target: { value: "36" } });
    fireEvent.change(screen.getByLabelText("왼쪽"), { target: { value: "18" } });
    expect(screen.getByTestId("crop-result")).toHaveTextContent("남는 크기 204 × 284 mm");
    fireEvent.click(apply);
    await waitFor(() => expect(useDialogStore.getState().stack).toHaveLength(0));
    expect(spy).toHaveBeenCalledTimes(1);
    expect(spy.mock.calls[0][0]).toMatchObject({
      pages: [0, 1],
      crop: { margins: { top: 36, left: 18, right: 0, bottom: 0 } },
    });
    const info = useDocStore.getState().info!;
    expect(info.undoLabel).toBe("undo.pageCrop");
    expect(info.pages[0].widthPt).toBeCloseTo(595.28 - 18, 1);
    expect(info.pages[1].heightPt).toBeCloseTo(841.89 - 36, 1);
    expect(info.pages[2].widthPt).toBeCloseTo(595.28, 1);
    expect(lastToast()).toMatchObject({ messageKey: "crop.done", params: { count: 2 } });
    expect(lastToast()?.actions?.[0].labelKey).toBe("common.undo");
  });

  it("dragging a handle resizes the box; with no selection every page is the default", async () => {
    const spy = vi.spyOn(mock, "setPageBoxes");
    await openCrop();
    expect(screen.getByRole("radio", { name: "모든 페이지" })).toBeChecked();
    const se = document.querySelector('.crop-handle[data-handle="se"]') as HTMLElement;
    const scale = Math.min(380 / 595.28, 440 / 841.89);
    fireEvent.pointerDown(se, { button: 0, clientX: 595.28 * scale, clientY: 841.89 * scale });
    fireEvent.pointerMove(window, { clientX: 595.28 * scale - 50 * scale, clientY: 841.89 * scale - 100 * scale });
    fireEvent.pointerUp(window, {});
    expect(screen.getByLabelText("오른쪽")).toHaveValue(50);
    expect(screen.getByLabelText("아래")).toHaveValue(100);
    fireEvent.click(screen.getByRole("button", { name: "적용" }));
    await waitFor(() => expect(spy).toHaveBeenCalledTimes(1));
    expect(spy.mock.calls[0][0]).toMatchObject({ pages: "all", crop: { margins: { right: 50, bottom: 100 } } });
  });

  it("여백 자동 감지 finds the content of a render", async () => {
    const w = 600;
    const h = Math.round((w * 841.89) / 595.28);
    const px = new Uint8ClampedArray(w * h * 4).fill(255);
    for (let y = 100; y < h - 100; y++) for (let x = 60; x < w - 60; x++) px.set([0, 0, 0, 255], (y * w + x) * 4);
    const render = vi.spyOn(mock, "renderPageRaw").mockResolvedValue(encodeRawPage(w, h, px));
    await openCrop([0]);
    fireEvent.click(screen.getByRole("button", { name: "여백 자동 감지" }));
    await waitFor(() => expect(render).toHaveBeenCalledTimes(1));
    await waitFor(() => expect(Number((screen.getByLabelText("위") as HTMLInputElement).value)).toBeGreaterThan(80));
    const left = Number((screen.getByLabelText("왼쪽") as HTMLInputElement).value);
    const right = Number((screen.getByLabelText("오른쪽") as HTMLInputElement).value);
    // 60 px at ~1.008 px/pt, minus the 6 pt padding
    expect(left).toBeCloseTo(53.5, 0);
    expect(right).toBeCloseTo(left, 0);
    expect(screen.getByRole("button", { name: "적용" })).toBeEnabled();

    // a blank page: nothing found, a note instead
    render.mockResolvedValue(encodeRawPage(10, 14, new Uint8ClampedArray(10 * 14 * 4).fill(255)));
    fireEvent.click(screen.getByRole("button", { name: "여백 자동 감지" }));
    await waitFor(() => expect(lastToast()?.messageKey).toBe("crop.autoDetect.none"));
  });

  it("원래대로 resets the crop box to the media box and keeps the dialog open", async () => {
    await useDocStore.getState().open(SAMPLE);
    const docId = useDocStore.getState().info!.docId;
    const cropped = await mock.setPageBoxes({ docId, pages: [0], crop: { margins: { top: 100, right: 0, bottom: 0, left: 0 } } });
    useDocStore.getState().adopt(cropped);
    expect(cropped.pages[0].heightPt).toBeCloseTo(741.89, 1);
    const spy = vi.spyOn(mock, "setPageBoxes");
    render(<DialogHost />);
    openDialog("crop", { pages: [0] });
    fireEvent.click(await screen.findByRole("button", { name: "원래대로" }));
    await waitFor(() => expect(spy).toHaveBeenCalledTimes(1));
    expect(spy.mock.calls[0][0]).toMatchObject({ pages: [0], crop: null });
    await waitFor(() => expect(useDocStore.getState().info!.pages[0].heightPt).toBeCloseTo(841.89, 1));
    expect(lastToast()).toMatchObject({ messageKey: "crop.resetDone", params: { count: 1 } });
    expect(useDialogStore.getState().stack.at(-1)?.name).toBe("crop");
  });
});

describe("dialogs.resize.flow", () => {
  it("레터 + 확대/축소 on one page: one 페이지 크기 변경 step", async () => {
    const spy = vi.spyOn(mock, "resizePages");
    await useDocStore.getState().open(SAMPLE);
    render(<DialogHost />);
    openDialog("resize", { pages: [1] });
    fireEvent.click(await screen.findByRole("radio", { name: "레터 (216 × 279 mm)" }));
    expect(screen.getByText("현재 210 × 297 mm")).toBeInTheDocument();
    expect(screen.getByRole("radio", { name: "새 크기에 맞게 확대/축소" })).toBeChecked();
    fireEvent.click(screen.getByRole("button", { name: "적용" }));
    await waitFor(() => expect(useDialogStore.getState().stack).toHaveLength(0));
    expect(spy.mock.calls[0][0]).toMatchObject({ pages: [1], size: "Letter", mode: "scaleContent" });
    const info = useDocStore.getState().info!;
    expect(info.undoLabel).toBe("undo.pageResize");
    expect(info.pages[1].widthPt).toBe(612);
    expect(info.pages[0].widthPt).toBeCloseTo(595.28, 2);
    expect(lastToast()).toMatchObject({ messageKey: "resize.done", params: { count: 1 } });
  });

  it("a custom size is millimetres, validated, and can centre instead of scaling", async () => {
    const spy = vi.spyOn(mock, "resizePages");
    await useDocStore.getState().open(SAMPLE);
    usePagesStore.getState().setSelected([0, 2]);
    render(<DialogHost />);
    openDialog("resize");
    fireEvent.click(await screen.findByRole("radio", { name: "직접 입력" }));
    fireEvent.change(screen.getByLabelText("너비"), { target: { value: "0" } });
    expect(screen.getByRole("button", { name: "적용" })).toBeDisabled();
    expect(screen.getByText("크기를 확인해 주세요 (1–5080 mm)")).toBeInTheDocument();
    fireEvent.change(screen.getByLabelText("너비"), { target: { value: "100" } });
    fireEvent.change(screen.getByLabelText("높이"), { target: { value: "150" } });
    fireEvent.click(screen.getByRole("radio", { name: "원래 크기로 가운데 배치" }));
    expect(screen.getByRole("radio", { name: "선택한 페이지 (2쪽)" })).toBeChecked();
    fireEvent.click(screen.getByRole("button", { name: "적용" }));
    await waitFor(() => expect(spy).toHaveBeenCalledTimes(1));
    expect(spy.mock.calls[0][0]).toMatchObject({ pages: [0, 2], size: { w: 283.46, h: 425.2 }, mode: "centerContent" });
  });
});
