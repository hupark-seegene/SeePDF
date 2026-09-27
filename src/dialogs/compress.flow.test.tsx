import { beforeEach, describe, expect, it, vi } from "vitest";
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import DialogHost from "./DialogHost";
import { closeDialog, openDialog, useDialogStore } from "./dialogState";
import { useDocStore } from "../store/docStore";
import { useToastStore } from "../app/toastStore";
import { mock } from "../ipc/mock";

const SAMPLE = "/Users/veri/Documents/SeePDF-샘플.pdf"; // 3 pages, 1 016 315 bytes

beforeEach(() => {
  useDialogStore.getState().closeAll();
  useToastStore.setState({ toasts: [] });
});

async function openCompress() {
  await useDocStore.getState().open(SAMPLE);
  render(<DialogHost />);
  openDialog("compress");
  return screen.findByRole("button", { name: "예상" });
}

describe("dialogs.compress.flow", () => {
  it("예상 → before/after table → 적용 replaces the document as one 압축 step", async () => {
    const estimate = vi.spyOn(mock, "compressEstimate");
    const apply = vi.spyOn(mock, "compressApply");
    const run = await openCompress();
    const primary = screen.getByRole("button", { name: "적용" });
    expect(primary).toBeDisabled();
    expect(screen.getByRole("radio", { name: /150 DPI/ })).toBeChecked();

    fireEvent.click(run);
    expect(await screen.findByText(/예상 크기 계산 중/)).toBeInTheDocument();
    expect(await screen.findByText("현재 크기", {}, { timeout: 2000 })).toBeInTheDocument();
    expect(estimate.mock.calls[0][0].options).toEqual({ targetDpi: 150 });
    expect(screen.getByTestId("compress-delta").textContent).toMatch(/^−\d/);
    expect(screen.queryByRole("alert")).toBeNull();
    expect(primary).toBeEnabled();

    fireEvent.click(primary);
    await waitFor(() => expect(useDialogStore.getState().stack).toHaveLength(0));
    expect(apply).toHaveBeenCalledWith({ docId: "d1", token: 1 });
    const info = useDocStore.getState().info;
    expect(info?.undoLabel).toBe("undo.compress");
    expect(info?.bytes).toBeLessThan(1_016_315);
    expect(useToastStore.getState().toasts.at(-1)?.messageKey).toBe("compress.done");
  });

  it("warns in amber when the result is not smaller, and a new estimate discards the old token", async () => {
    const discard = vi.spyOn(mock, "compressDiscard");
    const run = await openCompress();
    fireEvent.click(screen.getByRole("radio", { name: /300 DPI/ }));
    fireEvent.click(run);
    expect(await screen.findByRole("alert", {}, { timeout: 2000 })).toHaveTextContent("작아지지 않습니다");
    expect(screen.getByTestId("compress-delta").textContent).toMatch(/^\+/);
    expect(screen.getByTestId("compress-delta")).toHaveClass("compress-warn");

    fireEvent.click(screen.getByRole("button", { name: "다시 예상" }));
    await waitFor(() => expect(discard).toHaveBeenCalledWith({ docId: "d1", token: 1 }));
    await screen.findByText("현재 크기", {}, { timeout: 2000 });

    // changing the preset invalidates the shown estimate and releases its token
    fireEvent.click(screen.getByRole("radio", { name: /96 DPI/ }));
    await waitFor(() => expect(discard).toHaveBeenCalledWith({ docId: "d1", token: 2 }));
    expect(screen.queryByText("현재 크기")).toBeNull();
    expect(screen.getByRole("button", { name: "적용" })).toBeDisabled();
  });

  it("취소 stops a running estimate; closing the dialog discards a pending result", async () => {
    const cancel = vi.spyOn(mock, "cancelJob");
    const discard = vi.spyOn(mock, "compressDiscard");
    const run = await openCompress();
    fireEvent.click(run);
    await screen.findByText(/예상 크기 계산 중/);
    // two 취소 buttons exist: the job's (in the body) and the dialog's (in the footer)
    const [jobCancel, dialogCancel] = screen.getAllByRole("button", { name: "취소" });
    expect(dialogCancel.closest(".dlg-foot")).not.toBeNull();
    fireEvent.click(jobCancel);
    await waitFor(() => expect(cancel).toHaveBeenCalled());
    await waitFor(() => expect(screen.getByRole("button", { name: "예상" })).toBeInTheDocument());
    expect(useDialogStore.getState().stack).toHaveLength(1);

    fireEvent.click(screen.getByRole("button", { name: "예상" }));
    await screen.findByText("현재 크기", {}, { timeout: 2000 });
    act(() => closeDialog("compress"));
    await waitFor(() => expect(discard).toHaveBeenCalledWith({ docId: "d1", token: 1 }));
    expect(useDocStore.getState().info?.canUndo).toBe(false);
  });
});
