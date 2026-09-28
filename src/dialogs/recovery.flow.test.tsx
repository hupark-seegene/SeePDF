import { beforeEach, describe, expect, it, vi } from "vitest";
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import DialogHost from "./DialogHost";
import { openDialog, useDialogStore } from "./dialogState";
import { useDocStore } from "../store/docStore";
import { useAppStore } from "../store/appStore";
import { useToastStore } from "../app/toastStore";
import { autosave, recoveredEntry } from "../app/autosave";
import { mock, seedRecovery } from "../ipc/mock";
import * as api from "../ipc/api";
import type { RecoveryEntry } from "../ipc/types";

const ENTRIES: RecoveryEntry[] = [
  {
    id: "11111111-1111-4111-8111-111111111111", originalPath: "/Users/veri/Documents/계약서 초안.pdf",
    name: "계약서 초안.pdf", savedAt: "2026-09-27T10:15:00Z", bytes: 2_456_789, pages: 12,
    recoveryPath: "/Users/veri/Library/Application Support/SeePDF/recovery/11111111-1111-4111-8111-111111111111.pdf",
  },
  {
    id: "22222222-2222-4222-8222-222222222222", originalPath: null, name: "제목 없음", savedAt: "2026-09-26T08:00:00Z",
    bytes: 1024, pages: 1,
    recoveryPath: "/Users/veri/Library/Application Support/SeePDF/recovery/22222222-2222-4222-8222-222222222222.pdf",
  },
];

beforeEach(() => {
  useDialogStore.getState().closeAll();
  useToastStore.setState({ toasts: [] });
  useDocStore.setState({ docId: null, info: null, status: "empty", error: null });
  autosave.reset();
  seedRecovery(ENTRIES);
});

async function showDialog() {
  render(<DialogHost />);
  act(() => openDialog("recovery", { entries: ENTRIES }));
  return screen.findByRole("dialog", { name: "문서 복구" });
}

describe("dialogs.recovery.flow", () => {
  it("lists name, time, pages and size; 삭제 discards one row", async () => {
    const discard = vi.spyOn(mock, "discardRecovery");
    const dlg = await showDialog();
    const rows = within(dlg).getAllByRole("row").slice(1);
    expect(rows).toHaveLength(2);
    expect(rows[0]).toHaveTextContent("계약서 초안.pdf");
    expect(rows[0]).toHaveTextContent("12");
    expect(rows[0]).toHaveTextContent("2.3 MB");
    expect(rows[0]).toHaveTextContent("2026");

    fireEvent.click(screen.getByRole("button", { name: "제목 없음 삭제" }));
    await waitFor(() => expect(discard).toHaveBeenCalledWith({ id: ENTRIES[1].id }));
    await waitFor(() => expect(within(dlg).getAllByRole("row")).toHaveLength(2));
    expect(await api.listRecovery()).toHaveLength(1);
  });

  it("모두 삭제 discards everything and closes; 나중에 keeps the copies", async () => {
    await showDialog();
    fireEvent.click(screen.getByRole("button", { name: "나중에" }));
    await waitFor(() => expect(useDialogStore.getState().stack).toHaveLength(0));
    expect(await api.listRecovery()).toHaveLength(2);

    act(() => openDialog("recovery", { entries: ENTRIES }));
    fireEvent.click(await screen.findByRole("button", { name: "모두 삭제" }));
    await waitFor(() => expect(useDialogStore.getState().stack).toHaveLength(0));
    expect(await api.listRecovery()).toHaveLength(0);
  });

  it("열기 opens the copy, warns, keeps it out of 최근 항목, and 저장 goes to 다른 이름으로 저장", async () => {
    const updateRecent = vi.spyOn(mock, "updateRecent");
    const save = vi.spyOn(mock, "saveDocument");
    const saveAs = vi.spyOn(mock, "saveDocumentAs");
    const discard = vi.spyOn(mock, "discardRecovery");
    await showDialog();
    fireEvent.click(screen.getByRole("button", { name: "계약서 초안.pdf 열기" }));

    await waitFor(() => expect(useDocStore.getState().info?.path).toBe(ENTRIES[0].recoveryPath));
    const info = useDocStore.getState().info!;
    expect(useDialogStore.getState().stack).toHaveLength(0);
    await waitFor(() =>
      expect(useToastStore.getState().toasts.map((t) => t.messageKey)).toContain("recovery.opened"),
    );
    expect(recoveredEntry(info.docId)?.id).toBe(ENTRIES[0].id);
    expect(updateRecent).not.toHaveBeenCalled();

    const flows = await import("./flows");
    expect(await flows.saveFlow()).toBe(true);
    expect(save).not.toHaveBeenCalled();
    expect(saveAs).toHaveBeenCalledWith({ docId: info.docId, path: expect.any(String) }, expect.any(Function));
    expect(discard).toHaveBeenCalledWith({ id: ENTRIES[0].id });
    expect(recoveredEntry(info.docId)).toBeUndefined();
    expect(updateRecent).toHaveBeenCalled();
  });
});

describe("dialogs.settings.autosave", () => {
  it("자동 저장 간격 persists 끄기 / 30초 / 1분 / 5분", async () => {
    await useAppStore.getState().bootstrap();
    render(<DialogHost />);
    act(() => openDialog("settings"));
    const oneMin = await screen.findByRole("radio", { name: "1분" });
    expect(oneMin).toHaveAttribute("aria-checked", "true");
    fireEvent.click(screen.getByRole("radio", { name: "5분" }));
    await waitFor(() => expect(useAppStore.getState().settings?.autosaveSec).toBe(300));
    fireEvent.click(screen.getByRole("radio", { name: "끄기" }));
    await waitFor(() => expect(useAppStore.getState().settings?.autosaveSec).toBe(0));
    expect(screen.getByRole("radio", { name: "끄기" })).toHaveAttribute("aria-checked", "true");
  });
});
