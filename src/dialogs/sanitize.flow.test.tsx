/**
 * v0.3 pkg3 — 문서 정리 (S3) in the 보안 dialog and the 첨부 파일 panel (S4), against the mock.
 * The engine side is `src-tauri/tests/security.rs` (sanitize_*, attachments_*).
 */
import { beforeEach, describe, expect, it, vi } from "vitest";
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import DialogHost from "./DialogHost";
import { openDialog, useDialogStore } from "./dialogState";
import { useDocStore } from "../store/docStore";
import { useAppStore } from "../store/appStore";
import { useToastStore } from "../app/toastStore";
import { SidebarFrame } from "../app/SidebarFrame";
import { mock } from "../ipc/mock";

const RISKY = "/Users/veri/Documents/risky-배포본.pdf";
const PLAIN = "/Users/veri/Documents/SeePDF-샘플.pdf";

beforeEach(() => {
  useDocStore.setState({ docId: null, info: null, outline: [], status: "empty", error: null });
  useDialogStore.getState().closeAll();
  useToastStore.setState({ toasts: [] });
  useAppStore.setState({ mode: "read", sidebarOpen: true, sidebarTab: "thumbnails" });
});

describe("S3 — 문서 정리", () => {
  it("removes the checked categories as one undo step and shows what went", async () => {
    await useDocStore.getState().open(RISKY);
    const spy = vi.spyOn(mock, "sanitizeDocument");
    render(<DialogHost />);
    openDialog("security");
    const section = await screen.findByRole("region", { name: "문서 정리" });
    fireEvent.click(within(section).getByLabelText("숨겨진 레이어"));
    fireEvent.click(within(section).getByRole("button", { name: "정리" }));
    await waitFor(() => expect(spy).toHaveBeenCalledTimes(1));
    await within(section).findAllByText(/개 제거$/);
    expect(spy.mock.calls[0][0].options).toEqual({
      javascript: true, attachments: true, actions: true, metadata: true, hiddenLayers: false,
    });
    // counts next to each checked box; the unchecked one shows none
    const counts = within(section).getAllByText(/개 제거$/).map((e) => e.textContent);
    expect(counts).toEqual(["2개 제거", "2개 제거", "1개 제거", "2개 제거"]);
    const info = useDocStore.getState().info!;
    expect(info.undoLabel).toBe("undo.sanitize");
    expect(info.attachmentCount).toBeUndefined();
    expect(useToastStore.getState().toasts.map((t) => t.messageKey)).toContain("security.sanitize.done");

    // a second run with 숨겨진 레이어 checked removes only what is left: the layer
    const generation = info.docGeneration;
    fireEvent.click(within(section).getByLabelText("숨겨진 레이어"));
    fireEvent.click(within(section).getByRole("button", { name: "정리" }));
    await waitFor(() => expect(spy).toHaveBeenCalledTimes(2));
    await within(section).findByText("1개 제거");
    expect(useDocStore.getState().info!.docGeneration).toBe(generation + 1);
  });

  it("a document with nothing to remove says so", async () => {
    await useDocStore.getState().open(PLAIN);
    render(<DialogHost />);
    openDialog("security");
    const section = await screen.findByRole("region", { name: "문서 정리" });
    fireEvent.click(within(section).getByRole("button", { name: "정리" }));
    expect(await within(section).findByText("지울 항목이 없습니다")).toBeInTheDocument();
    expect(useDocStore.getState().info!.canUndo).toBe(false);
  });

  it("the password section explains an empty form; a restricted document cannot be sanitized", async () => {
    await useDocStore.getState().open("/Users/veri/Documents/restricted-보고서.pdf");
    render(<DialogHost />);
    openDialog("security");
    expect(await screen.findByText("문서 열기 암호나 권한 암호 중 하나 이상을 입력하세요")).toBeInTheDocument();
    const section = screen.getByRole("region", { name: "문서 정리" });
    expect(within(section).getByRole("button", { name: "정리" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "메타데이터 제거" })).toBeDisabled();
    // S5: a restricted open cannot hand out a copy without its restrictions
    expect(screen.getByText(/보안 설정을 바꿀 수 없습니다/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "암호 제거…" })).toBeDisabled();
  });

  it("메타데이터 제거 is available on an encrypted document (it stays encrypted)", async () => {
    await useDocStore.getState().open("/tmp/encrypted.pdf", "user");
    render(<DialogHost />);
    openDialog("security");
    const remove = await screen.findByRole("button", { name: "메타데이터 제거" });
    expect(remove).toBeEnabled();
  });
});

describe("S4 — 첨부 파일", () => {
  it("the tab appears only with attachments or in 편집 mode", async () => {
    await useDocStore.getState().open(PLAIN);
    render(<SidebarFrame />);
    expect(screen.queryByRole("button", { name: "첨부" })).toBeNull();
    act(() => useAppStore.setState({ mode: "edit" }));
    expect(screen.getByRole("button", { name: "첨부" })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "첨부" }));
    expect(await screen.findByText("첨부 파일이 없습니다")).toBeInTheDocument();
    // leaving 편집 with no attachments hides the tab and falls back to 축소판
    act(() => useAppStore.setState({ mode: "read" }));
    expect(screen.queryByRole("button", { name: "첨부" })).toBeNull();
  });

  it("lists, saves, adds and deletes (one undo step each)", async () => {
    await useDocStore.getState().open("/Users/veri/Documents/attach-견적.pdf");
    const save = vi.spyOn(mock, "saveAttachment");
    const add = vi.spyOn(mock, "addAttachment");
    const del = vi.spyOn(mock, "deleteAttachment");
    render(<SidebarFrame />);
    fireEvent.click(screen.getByRole("button", { name: "첨부" }));
    const list = await screen.findByRole("list", { name: "첨부" });
    expect(within(list).getByText("견적서.xlsx")).toBeInTheDocument();
    expect(within(list).getByText("47.1 KB")).toBeInTheDocument();

    fireEvent.click(within(list).getByRole("button", { name: "저장…" }));
    await waitFor(() =>
      expect(save).toHaveBeenCalledWith({
        docId: useDocStore.getState().docId, index: 0, path: "/Users/veri/Documents/견적서.xlsx",
      }),
    );
    expect(within(list).queryByRole("button", { name: "삭제" })).toBeNull();

    act(() => useAppStore.setState({ mode: "edit" }));
    fireEvent.click(await screen.findByRole("button", { name: /파일 첨부/ }));
    await waitFor(() => expect(add).toHaveBeenCalledTimes(1));
    await screen.findByText("SeePDF-샘플.pdf");
    expect(useDocStore.getState().info?.attachmentCount).toBe(2);
    expect(useDocStore.getState().info?.undoLabel).toBe("undo.attachmentAdd");

    await waitFor(() => expect(screen.getAllByRole("button", { name: "삭제" })[0]).toBeEnabled());
    fireEvent.click(screen.getAllByRole("button", { name: "삭제" })[0]);
    await waitFor(() => expect(del).toHaveBeenCalledTimes(1));
    await waitFor(() => expect(useDocStore.getState().info?.attachmentCount).toBe(1));
    expect(useDocStore.getState().info?.undoLabel).toBe("undo.attachmentDelete");
  });
});
