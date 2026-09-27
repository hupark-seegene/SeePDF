import { beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import DialogHost from "./DialogHost";
import { openDialog, useDialogStore } from "./dialogState";
import { useDocStore } from "../store/docStore";
import { useToastStore } from "../app/toastStore";
import { mock } from "../ipc/mock";

const SAMPLE = "/Users/veri/Documents/SeePDF-샘플.pdf"; // 3 pages

beforeEach(() => {
  useDialogStore.getState().closeAll();
  useToastStore.setState({ toasts: [] });
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
