import { describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import DialogHost from "./DialogHost";
import { openDialog, useDialogStore } from "./dialogState";
import { useDocStore } from "../store/docStore";
import { mock } from "../ipc/mock";

const SAMPLE = "/Users/veri/Documents/SeePDF-샘플.pdf";

describe("dialogs.docInfo.flow", () => {
  it("적용 sends trimmed values, empties as blank strings (remove), and refreshes the store", async () => {
    await useDocStore.getState().open(SAMPLE);
    const spy = vi.spyOn(mock, "setMetadata");
    render(<DialogHost />);
    openDialog("docInfo");
    const title = await screen.findByDisplayValue("SeePDF 샘플 문서");
    const apply = screen.getByRole("button", { name: "적용" });
    expect(apply).toBeDisabled();

    fireEvent.change(title, { target: { value: "  새 제목  " } });
    fireEvent.change(screen.getByDisplayValue("SeePDF"), { target: { value: "   " } });
    expect(apply).toBeEnabled();
    fireEvent.click(apply);

    await waitFor(() => expect(useDialogStore.getState().stack).toHaveLength(0));
    expect(spy).toHaveBeenCalledTimes(1);
    const meta = spy.mock.calls[0][0].meta;
    expect(meta).toEqual({ title: "새 제목", author: "", subject: "Stage 0 mock fixture", keywords: "" });
    expect(useDocStore.getState().info?.meta.title).toBe("새 제목");
    expect(useDocStore.getState().info?.meta.author).toBeUndefined();
    spy.mockRestore();
  });

  it("보안 opens lazily and 암호 설정 needs a matching password", async () => {
    await useDocStore.getState().open(SAMPLE);
    const spy = vi.spyOn(mock, "setPassword");
    render(<DialogHost />);
    openDialog("security");
    const primary = await screen.findByRole("button", { name: "암호 설정…" });
    expect(primary).toBeDisabled();
    const open = screen.getByLabelText("문서 열기 암호");
    const [confirm] = screen.getAllByLabelText("암호 확인");
    fireEvent.change(open, { target: { value: "1234" } });
    fireEvent.change(confirm, { target: { value: "123" } });
    expect(screen.getByRole("alert")).toHaveTextContent("암호가 일치하지 않습니다");
    expect(primary).toBeDisabled();
    fireEvent.change(confirm, { target: { value: "1234" } });
    expect(primary).toBeEnabled();
    // Unchecking a permission with no separate 권한 암호 would let anyone who can open the
    // file lift the restriction, so the dialog refuses until one is entered.
    fireEvent.click(screen.getByLabelText("내용 복사 허용"));
    expect(screen.getByRole("alert")).toHaveTextContent("권한 암호");
    expect(primary).toBeDisabled();
    const owner = screen.getByLabelText("권한 암호");
    const [, ownerConfirm] = screen.getAllByLabelText("암호 확인");
    fireEvent.change(owner, { target: { value: "owner" } });
    fireEvent.change(ownerConfirm, { target: { value: "owner" } });
    expect(screen.queryByRole("alert")).toBeNull();
    fireEvent.click(primary);
    await waitFor(() => expect(spy).toHaveBeenCalledTimes(1));
    expect(spy.mock.calls[0][0]).toMatchObject({
      userPassword: "1234",
      ownerPassword: "owner",
      permissions: { print: true, extractText: false, modify: true, annotate: true },
    });
    await waitFor(() => expect(useDialogStore.getState().stack).toHaveLength(0));
    spy.mockRestore();
  });
});

describe("dialogs.docInfo.encryption", () => {
  it("shows AES-256 (R6) for a revision-6 document", async () => {
    await useDocStore.getState().open(SAMPLE);
    const info = useDocStore.getState().info!;
    useDocStore.getState().adopt({ ...info, encrypted: true, permissions: { ...info.permissions, revision: "r6" } });
    render(<DialogHost />);
    openDialog("docInfo");
    expect(await screen.findByText("AES-256 (R6)")).toBeInTheDocument();
  });
});
