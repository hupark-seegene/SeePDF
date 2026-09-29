import { beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import DialogHost from "./DialogHost";
import { openDialog, useDialogStore } from "./dialogState";
import { useDocStore } from "../store/docStore";
import { useToastStore } from "../app/toastStore";
import { mock } from "../ipc/mock";
import { stampImageSource } from "./StampDialog";
import * as api from "../ipc/api";
import { setMockImageSize } from "../ipc/mock";

const SAMPLE = "/Users/veri/Documents/SeePDF-샘플.pdf"; // 3 pages

beforeEach(() => {
  useDialogStore.getState().closeAll();
  useToastStore.setState({ toasts: [] });
  vi.restoreAllMocks();
});

describe("dialogs.stamp.flow", () => {
  it("opens lazily with watermark defaults and applies one step labelled 워터마크", async () => {
    await useDocStore.getState().open(SAMPLE);
    const spy = vi.spyOn(mock, "addStamp");
    render(<DialogHost />);
    openDialog("stamp");

    const text = await screen.findByLabelText("텍스트", { selector: "textarea" });
    expect(text).toHaveValue("대외비");
    expect(screen.getByRole("radio", { name: "워터마크" })).toHaveAttribute("aria-checked", "true");
    expect(screen.getByRole("radio", { name: "가운데" })).toHaveAttribute("aria-checked", "true");
    expect(screen.getByLabelText("회전")).toHaveValue(45);

    fireEvent.click(screen.getByRole("button", { name: "적용" }));
    await waitFor(() => expect(useDialogStore.getState().stack).toHaveLength(0));
    expect(spy).toHaveBeenCalledTimes(1);
    expect(spy.mock.calls[0][0].spec).toMatchObject({
      role: "watermark", anchor: "mc", rotateDeg: 45, pages: "all",
      source: { kind: "text", text: "대외비" },
    });
    const info = useDocStore.getState().info;
    expect(info?.undoLabel).toBe("undo.watermark");
    expect(info?.canUndo).toBe(true);
    const done = useToastStore.getState().toasts.at(-1);
    expect(done?.messageKey).toBe("stamp.done.watermark");
    expect(done?.params).toEqual({ count: 3 });
    expect(done?.actions?.[0].labelKey).toBe("common.undo");
  });

  it("바닥글: bottom centre, no rotation, tokens preview per page and a custom range", async () => {
    await useDocStore.getState().open(SAMPLE);
    const spy = vi.spyOn(mock, "addStamp");
    render(<DialogHost />);
    openDialog("stamp");
    fireEvent.click(await screen.findByRole("radio", { name: "바닥글" }));

    expect(screen.getByRole("radio", { name: "아래 가운데" })).toHaveAttribute("aria-checked", "true");
    expect(screen.queryByLabelText("회전")).toBeNull();
    const text = screen.getByLabelText("텍스트", { selector: "textarea" });
    expect(text).toHaveValue("{{page}} / {{total}}");
    expect(screen.getByTestId("stamp-preview-mark")).toHaveTextContent("1 / 3");

    // token buttons append at the caret
    fireEvent.change(text, { target: { value: "" } });
    expect(screen.getByRole("button", { name: "적용" })).toBeDisabled();
    expect(screen.getByText("텍스트를 입력하세요")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "파일 이름" }));
    expect(text).toHaveValue("{{filename}}");
    expect(screen.getByTestId("stamp-preview-mark")).toHaveTextContent("SeePDF-샘플");

    fireEvent.click(screen.getByRole("radio", { name: "직접 입력" }));
    fireEvent.change(screen.getByLabelText("페이지 범위"), { target: { value: "2-3" } });
    fireEvent.click(screen.getByRole("button", { name: "적용" }));
    await waitFor(() => expect(spy).toHaveBeenCalledTimes(1));
    expect(spy.mock.calls[0][0].spec).toMatchObject({
      role: "footer", anchor: "bc", rotateDeg: 0, pages: [1, 2], source: { kind: "text", text: "{{filename}}" },
    });
    await waitFor(() => expect(useDocStore.getState().info?.undoLabel).toBe("undo.headerFooter"));
  });

  it("이미지: needs a picked file before 적용", async () => {
    await useDocStore.getState().open(SAMPLE);
    const spy = vi.spyOn(mock, "addStamp");
    render(<DialogHost />);
    openDialog("stamp");
    fireEvent.click(await screen.findByRole("radio", { name: "이미지" }));
    const apply = screen.getByRole("button", { name: "적용" });
    expect(apply).toBeDisabled();
    fireEvent.click(screen.getByRole("button", { name: "이미지 선택…" }));
    await waitFor(() => expect(apply).toBeEnabled());
    fireEvent.click(apply);
    await waitFor(() => expect(spy).toHaveBeenCalledTimes(1));
    expect(spy.mock.calls[0][0].spec.source).toMatchObject({ kind: "image", widthPt: 144 });
  });
});

describe("dialogs.stamp.remove (Stage 8)", () => {
  const lastToast = () => useToastStore.getState().toasts.at(-1);

  it("워터마크 제거 removes the current role on the range; 모두 제거 every role; nothing left is not an error", async () => {
    const info = await useDocStore.getState().open(SAMPLE);
    // a watermark on every page and a footer on pages 2–3
    await mock.addStamp({
      docId: info!.docId,
      spec: { role: "watermark", source: { kind: "text", text: "대외비", fontSizePt: 60, color: [200, 30, 30] }, anchor: "mc", marginPt: 36, rotateDeg: 45, opacity: 0.25, pages: "all" },
    });
    await mock.addStamp({
      docId: info!.docId,
      spec: { role: "footer", source: { kind: "text", text: "{{page}}", fontSizePt: 10, color: [80, 80, 80] }, anchor: "bc", marginPt: 36, rotateDeg: 0, opacity: 1, pages: [1, 2] },
    });
    const remove = vi.spyOn(mock, "removeStamps");
    render(<DialogHost />);
    openDialog("stamp");

    fireEvent.click(await screen.findByRole("button", { name: "워터마크 제거" }));
    await waitFor(() => expect(remove).toHaveBeenCalledTimes(1));
    expect(remove.mock.calls[0][0]).toEqual({ docId: info!.docId, pages: "all", role: "watermark" });
    await waitFor(() => expect(lastToast()?.messageKey).toBe("stamp.remove.done"));
    expect(lastToast()?.params).toEqual({ count: 3 });
    expect(lastToast()?.actions?.[0].labelKey).toBe("common.undo");
    expect(useDocStore.getState().info?.undoLabel).toBe("undo.removeStamps");
    // the dialog stays open for a new watermark
    expect(useDialogStore.getState().stack.map((e) => e.name)).toEqual(["stamp"]);

    // again: nothing of that role is left — an info toast, not an error
    fireEvent.click(screen.getByRole("button", { name: "워터마크 제거" }));
    await waitFor(() => expect(lastToast()?.messageKey).toBe("stamp.remove.none"));
    expect(lastToast()?.tone).toBe("info");

    // 바닥글 → its own label; a range limits it; 모두 제거 omits the role
    fireEvent.click(screen.getByRole("radio", { name: "바닥글" }));
    expect(screen.getByRole("button", { name: "바닥글 제거" })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("radio", { name: "직접 입력" }));
    fireEvent.change(screen.getByLabelText("페이지 범위"), { target: { value: "2" } });
    fireEvent.click(screen.getByRole("button", { name: "모두 제거" }));
    await waitFor(() => expect(remove).toHaveBeenCalledTimes(3));
    expect(remove.mock.calls[2][0]).toEqual({ docId: info!.docId, pages: [1] });
    await waitFor(() => expect(lastToast()?.params).toEqual({ count: 1 }));
  });

  it("a failed removal toasts and keeps the dialog", async () => {
    await useDocStore.getState().open(SAMPLE);
    vi.spyOn(mock, "removeStamps").mockRejectedValue({ code: "pdfium", message: "boom" });
    render(<DialogHost />);
    openDialog("stamp");
    fireEvent.click(await screen.findByRole("button", { name: "워터마크 제거" }));
    await waitFor(() => expect(lastToast()?.messageKey).toBe("stamp.remove.failed"));
    expect(lastToast()?.tone).toBe("danger");
    expect(screen.getByRole("button", { name: "워터마크 제거" })).toBeEnabled();
  });
});

describe("dialogs.stamp.preview (Stage 8)", () => {
  it("is the first page of the range at its visual size: crop box, /Rotate 90 swaps the sides", async () => {
    const info = await useDocStore.getState().open(SAMPLE);
    // page 2 is a landscape crop of a rotated page: crop 500 × 300 at /Rotate 90 → 300 wide, 500 tall
    const pages = info!.pages.map((p, i) =>
      i === 1 ? { ...p, rotation: 90 as const, crop: { l: 50, b: 40, r: 550, t: 340 } } : p,
    );
    useDocStore.setState({ info: { ...info!, pages } });
    render(<DialogHost />);
    openDialog("stamp");
    const page = await screen.findByTestId("stamp-preview-page");
    const portrait = parseFloat(page.style.height) / parseFloat(page.style.width);
    expect(portrait).toBeCloseTo(info!.pages[0].heightPt / info!.pages[0].widthPt, 1);
    // the thumbnail of that page sits under the stamp
    expect(page.querySelector("img.stamp-thumb")).not.toBeNull();

    fireEvent.click(screen.getByRole("radio", { name: "직접 입력" }));
    fireEvent.change(screen.getByLabelText("페이지 범위"), { target: { value: "2-3" } });
    await waitFor(() => expect(parseFloat(page.style.height)).toBe(Math.round((500 / 300) * 168)));
    expect(page.style.width).toBe("168px");
  });

  it("an image stamp shows the picked file (image_preview, v0.3 T1) at its real aspect", async () => {
    await useDocStore.getState().open(SAMPLE);
    vi.spyOn(api, "openFileDialog").mockResolvedValue(["/Users/veri/Pictures/logo.png"]);
    setMockImageSize("/Users/veri/Pictures/logo.png", 400, 100);
    vi.spyOn(api, "previewUrl").mockReturnValue("blob:seepdf/logo");
    render(<DialogHost />);
    openDialog("stamp");
    fireEvent.click(await screen.findByRole("radio", { name: "이미지" }));
    // nothing picked yet: 적용 is off and the dialog says why
    expect(screen.getByText("이미지를 선택하세요")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "이미지 선택…" }));
    const img = (await screen.findByTestId("stamp-preview-image")) as HTMLImageElement;
    expect(img.src).toBe("blob:seepdf/logo");
    expect(screen.queryByText("이미지를 선택하세요")).toBeNull();
    const box = img.parentElement as HTMLElement;
    // 144 pt wide at 400 × 100 → 36 pt tall, not the 0.6 placeholder
    expect(parseFloat(box.style.height) / parseFloat(box.style.width)).toBeCloseTo(0.25, 2);
  });

  it("keeps the placeholder when the image cannot be read", async () => {
    await useDocStore.getState().open(SAMPLE);
    vi.spyOn(api, "openFileDialog").mockResolvedValue(["/Users/veri/Pictures/broken.png"]);
    vi.spyOn(stampImageSource, "preview").mockRejectedValue(new Error("not an image"));
    render(<DialogHost />);
    openDialog("stamp");
    fireEvent.click(await screen.findByRole("radio", { name: "이미지" }));
    fireEvent.click(screen.getByRole("button", { name: "이미지 선택…" }));
    await screen.findByText("이미지를 읽을 수 없습니다");
    expect(screen.queryByTestId("stamp-preview-image")).toBeNull();
    const box = document.querySelector(".stamp-image") as HTMLElement;
    expect(parseFloat(box.style.height) / parseFloat(box.style.width)).toBeCloseTo(0.6, 2);
  });

  it("뒤에 배치 and 배경색 reach add_stamp (v0.3 T4)", async () => {
    await useDocStore.getState().open(SAMPLE);
    const spy = vi.spyOn(mock, "addStamp");
    render(<DialogHost />);
    openDialog("stamp");
    fireEvent.click(await screen.findByRole("checkbox", { name: /뒤에 배치/ }));
    fireEvent.click(screen.getByRole("button", { name: "적용" }));
    await waitFor(() => expect(spy).toHaveBeenCalledTimes(1));
    expect(spy.mock.calls[0][0].spec).toMatchObject({ behind: true, source: { kind: "text" } });

    openDialog("stamp");
    fireEvent.click(await screen.findByRole("radio", { name: "배경색" }));
    expect(screen.getByTestId("stamp-preview-background")).toBeInTheDocument();
    expect(screen.queryByRole("checkbox", { name: /뒤에 배치/ })).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "적용" }));
    await waitFor(() => expect(spy).toHaveBeenCalledTimes(2));
    const spec = spy.mock.calls[1][0].spec;
    expect(spec.source).toEqual({ kind: "background", color: [255, 248, 220] });
    expect(spec.behind).toBeUndefined();
  });
});
