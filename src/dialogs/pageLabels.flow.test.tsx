/**
 * 페이지 레이블 (P2) against the mock adapter: rows → live preview → 적용 = one
 * `set_page_labels`; `DocInfo.pageLabels` then drives the status-bar page box (label or number);
 * 모두 제거 removes them; 문서 정보 opens the dialog; an encrypted document is read-only.
 */
import { beforeEach, describe, expect, it, vi } from "vitest";
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import DialogHost from "./DialogHost";
import { openDialog, useDialogStore } from "./dialogState";
import { mock } from "../ipc/mock";
import { useDocStore } from "../store/docStore";
import { useViewStore } from "../store/viewStore";
import { useToastStore } from "../app/toastStore";
import { StatusBar } from "../app/StatusBar";

const SAMPLE = "/Users/veri/Documents/SeePDF-샘플.pdf";

beforeEach(() => {
  useToastStore.setState({ toasts: [] });
  useDialogStore.getState().closeAll();
});

function previewLabels(): string[] {
  const list = screen.getByRole("list", { name: "미리 보기" });
  return within(list)
    .getAllByRole("listitem")
    .map((li) => li.querySelectorAll("span")[1]?.textContent ?? "");
}

describe("페이지 레이블", () => {
  it("builds ranges with a live preview, applies them in one call, and the page box speaks labels", async () => {
    await useDocStore.getState().open(SAMPLE);
    const spy = vi.spyOn(mock, "setPageLabels");
    render(
      <>
        <DialogHost />
        <StatusBar />
      </>,
    );
    openDialog("pageLabels");
    await screen.findByText("레이블이 없습니다. 페이지 번호가 그대로 표시됩니다.");
    const apply = screen.getByRole("button", { name: "적용" });
    expect(apply).toBeDisabled();
    expect(previewLabels()).toEqual(["1", "2", "3"]);

    fireEvent.click(screen.getByRole("button", { name: "범위 추가" }));
    expect(previewLabels()).toEqual(["i", "ii", "iii"]);
    fireEvent.click(screen.getByRole("button", { name: "범위 추가" }));
    // the second row starts on page 2; move it to 3 and give it a prefix
    const start2 = screen.getByLabelText("시작 페이지 2");
    expect(start2).toHaveValue("2");
    fireEvent.change(start2, { target: { value: "3" } });
    fireEvent.change(screen.getByLabelText("접두사 2"), { target: { value: "본-" } });
    expect(previewLabels()).toEqual(["i", "ii", "본-1"]);

    // a duplicate start is an inline error and blocks 적용
    fireEvent.change(start2, { target: { value: "1" } });
    // (both rows say so)
    expect(screen.getAllByRole("alert")[0]).toHaveTextContent("같은 페이지에서 시작하는 범위가 이미 있습니다");
    expect(apply).toBeDisabled();
    fireEvent.change(start2, { target: { value: "3" } });
    expect(apply).toBeEnabled();

    fireEvent.click(apply);
    await waitFor(() => expect(useDialogStore.getState().stack).toHaveLength(0));
    expect(spy).toHaveBeenCalledTimes(1);
    expect(spy.mock.calls[0][0].ranges).toEqual([
      { start: 0, style: "roman" },
      { start: 2, style: "decimal", prefix: "본-" },
    ]);
    const info = useDocStore.getState().info!;
    expect(info.pageLabels).toEqual(["i", "ii", "본-1"]);
    expect(info.pages.map((p) => p.label)).toEqual(["i", "ii", "본-1"]);
    expect(info.undoLabel).toBe("undo.pageLabels");
    expect(useToastStore.getState().toasts.at(-1)?.messageKey).toBe("pageLabels.applied");

    // the status bar: the label in the box, the physical position beside it
    const box = screen.getByRole("textbox", { name: "페이지 번호" });
    expect(box).toHaveValue("i");
    expect(screen.getByText("(1 / 3)")).toBeInTheDocument();
    fireEvent.change(box, { target: { value: "본-1" } });
    fireEvent.submit(box.closest("form")!);
    expect(useViewStore.getState().currentPage).toBe(2);
    await waitFor(() => expect(box).toHaveValue("본-1"));
    // a plain number still works
    fireEvent.change(box, { target: { value: "2" } });
    fireEvent.submit(box.closest("form")!);
    expect(useViewStore.getState().currentPage).toBe(1);

    // reopening shows the written ranges; 모두 제거 + 적용 removes them
    openDialog("pageLabels");
    await waitFor(() => expect(screen.getByLabelText("시작 페이지 2")).toHaveValue("3"));
    fireEvent.click(screen.getByRole("button", { name: "모두 제거" }));
    fireEvent.click(screen.getByRole("button", { name: "적용" }));
    await waitFor(() => expect(useDocStore.getState().info?.pageLabels).toBeUndefined());
    expect(useToastStore.getState().toasts.at(-1)?.messageKey).toBe("pageLabels.removed");

    // 실행 취소 twice: back to the labels, then to none
    await act(async () => {
      await useDocStore.getState().undo();
    });
    expect(useDocStore.getState().info?.pageLabels).toEqual(["i", "ii", "본-1"]);
    await act(async () => {
      await useDocStore.getState().undo();
    });
    expect(useDocStore.getState().info?.pageLabels).toBeUndefined();
    spy.mockRestore();
  });

  it("opens from 문서 정보 over it, and an encrypted document is read-only", async () => {
    await useDocStore.getState().open(SAMPLE);
    render(<DialogHost />);
    openDialog("docInfo");
    fireEvent.click(await screen.findByRole("button", { name: "페이지 레이블…" }));
    await screen.findByRole("button", { name: "범위 추가" });
    expect(useDialogStore.getState().stack.map((e) => e.name)).toEqual(["docInfo", "pageLabels"]);
    fireEvent.click(screen.getByRole("button", { name: "취소" }));
    await screen.findByRole("button", { name: "페이지 레이블…" });

    act(() => useDialogStore.getState().closeAll());
    const info = useDocStore.getState().info!;
    act(() => useDocStore.getState().adopt({ ...info, encrypted: true }));
    openDialog("pageLabels");
    expect(await screen.findByText("암호가 걸린 문서에서는 할 수 없습니다. 보안에서 암호를 먼저 제거하세요.")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "범위 추가" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "적용" })).toBeDisabled();
  });
});
