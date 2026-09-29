/**
 * v0.3 pkg3-security-save-integrity — H8 (one window per file; a file changed on disk), S1 (saving
 * a signed document that needs a full rewrite) and U2 (백업 폴더 열기), against the mock. The engine
 * side is `src-tauri/tests/save.rs` and `security.rs`.
 */
import { beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import DialogHost from "./DialogHost";
import { openDialog, useDialogStore } from "./dialogState";
import { openPath, saveFlow } from "./flows";
import { useDocStore } from "../store/docStore";
import { useAppStore } from "../store/appStore";
import { useToastStore } from "../app/toastStore";
import { mock, mockChangeOnDisk, mockOpenInOtherWindow, mockSetIncrementalSave } from "../ipc/mock";
import * as api from "../ipc/api";

const A = "/Users/veri/Documents/SeePDF-샘플.pdf";

async function openDirty(path = A) {
  const info = (await openPath(path))!;
  await api.pageOps({ docId: info.docId, ops: [{ kind: "rotate", pages: [0], delta: 90 }] });
  await useDocStore.getState().refresh();
  return useDocStore.getState().info!;
}

beforeEach(() => {
  useDocStore.setState({ docId: null, info: null, outline: [], status: "empty", error: null });
  useDialogStore.getState().closeAll();
  useToastStore.setState({ toasts: [] });
});

describe("H8 — one window per file", () => {
  it("opening a path another window shows focuses it and does not call open_document", async () => {
    mockOpenInOtherWindow(A, "doc-3");
    const focus = vi.spyOn(mock, "focusDocumentWindow");
    const open = vi.spyOn(mock, "openDocument");
    expect(await openPath(A)).toBeNull();
    expect(focus).toHaveBeenCalledWith({ path: A });
    expect(open).not.toHaveBeenCalled();
    expect(useDocStore.getState().info).toBeNull();
  });

  it("a path no other window shows opens normally", async () => {
    const open = vi.spyOn(mock, "openDocument");
    expect(await openPath(A)).not.toBeNull();
    expect(open).toHaveBeenCalledTimes(1);
  });

  it("this window's own file is not 'elsewhere'", async () => {
    mockOpenInOtherWindow(A, "main");
    expect(await openPath(A)).not.toBeNull();
  });
});

describe("H8 — the file changed on disk", () => {
  async function prompt() {
    const info = await openDirty();
    mockChangeOnDisk(A);
    render(<DialogHost />);
    const saving = saveFlow();
    await screen.findByText("다른 프로그램에서 파일이 변경되었습니다");
    return { info, saving };
  }

  it("덮어쓰기 saves again with force", async () => {
    const save = vi.spyOn(mock, "saveDocument");
    const { info, saving } = await prompt();
    expect(screen.getByText(/연 뒤에 디스크에서 바뀌었습니다/)).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "덮어쓰기" }));
    expect(await saving).toBe(true);
    expect(save).toHaveBeenCalledTimes(2);
    expect(save.mock.calls[1][0]).toEqual({ docId: info.docId, force: true });
    expect(useDocStore.getState().info?.dirty).toBe(false);
  });

  it("다른 이름으로 저장 goes to Save As", async () => {
    const saveAs = vi.spyOn(mock, "saveDocumentAs");
    const { saving } = await prompt();
    fireEvent.click(screen.getByRole("button", { name: "다른 이름으로 저장…" }));
    expect(await saving).toBe(true);
    expect(saveAs).toHaveBeenLastCalledWith(
      expect.objectContaining({ path: "/Users/veri/Documents/SeePDF-샘플 (사본).pdf" }),
      expect.any(Function),
    );
  });

  it("다른 이름으로 저장… onto the same file saves (the save panel already asked to replace it)", async () => {
    const saveAs = vi.spyOn(mock, "saveDocumentAs");
    vi.spyOn(api, "saveFileDialog").mockResolvedValueOnce(A);
    const { saving } = await prompt();
    fireEvent.click(screen.getByRole("button", { name: "다른 이름으로 저장…" }));
    expect(await saving).toBe(true);
    expect(saveAs).toHaveBeenLastCalledWith(expect.objectContaining({ path: A }), expect.any(Function));
    expect(useToastStore.getState().toasts.map((t) => t.messageKey)).not.toContain("error.saveFailed");
    expect(useDocStore.getState().info?.dirty).toBe(false);
    // the Save As re-recorded the file: the next plain save goes straight through
    await api.pageOps({ docId: useDocStore.getState().info!.docId, ops: [{ kind: "rotate", pages: [0], delta: 90 }] });
    expect(await saveFlow()).toBe(true);
  });

  it("다시 불러오기 reopens the file from disk and drops this window's changes", async () => {
    const open = vi.spyOn(mock, "openDocument");
    const { info, saving } = await prompt();
    fireEvent.click(screen.getByRole("button", { name: "다시 불러오기 (내 변경 사항 버림)" }));
    await saving;
    expect(open).toHaveBeenLastCalledWith(expect.objectContaining({ path: A }));
    const now = useDocStore.getState().info!;
    expect(now.docId).not.toBe(info.docId);
    expect(now.dirty).toBe(false);
    await expect(mock.getDocument({ docId: info.docId })).rejects.toMatchObject({ code: "notFound" });
  });

  it("취소 saves nothing", async () => {
    const save = vi.spyOn(mock, "saveDocument");
    const { saving } = await prompt();
    fireEvent.click(screen.getByRole("button", { name: "취소" }));
    expect(await saving).toBe(false);
    expect(save).toHaveBeenCalledTimes(1);
    expect(useDocStore.getState().info?.dirty).toBe(true);
  });
});

describe("S1 — saving a signed document", () => {
  it("an incremental save asks nothing; a full rewrite asks once", async () => {
    // (the signature gate is not installed in this file: the edit goes straight through)
    const info = await openDirty("/Users/veri/Documents/signed-계약서.pdf");
    render(<DialogHost />);
    expect(await saveFlow()).toBe(true);
    expect(useDialogStore.getState().stack).toHaveLength(0);

    mockSetIncrementalSave(info.docId, false);
    const saving = saveFlow();
    await screen.findByText(/기존 디지털 서명이 무효화됩니다/);
    fireEvent.click(screen.getByRole("button", { name: "저장" }));
    expect(await saving).toBe(true);
    // asked once per document
    expect(await saveFlow()).toBe(true);
    await waitFor(() => expect(useDialogStore.getState().stack).toHaveLength(0));
  });
});

describe("U2 — 백업 폴더 열기", () => {
  it("reveals <app data>/backups", async () => {
    await useAppStore.getState().bootstrap();
    const reveal = vi.spyOn(mock, "revealInFileManager");
    render(<DialogHost />);
    openDialog("settings");
    fireEvent.click(await screen.findByRole("tab", { name: "고급" }).catch(() => screen.findByText("고급")));
    fireEvent.click(await screen.findByRole("button", { name: "백업 폴더 열기" }));
    await waitFor(() =>
      expect(reveal).toHaveBeenCalledWith({ path: "/Users/veri/Library/Application Support/SeePDF/backups" }),
    );
  });
});
