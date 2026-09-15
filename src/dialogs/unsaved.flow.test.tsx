import { describe, expect, it } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import DialogHost from "./DialogHost";
import { confirmUnsaved, closeDocumentFlow, saveFlow } from "./flows";
import { useDialogStore } from "./dialogState";
import { useDocStore } from "../store/docStore";

const SAMPLE = "/Users/veri/Documents/SeePDF-샘플.pdf";

async function openDirty() {
  await useDocStore.getState().open(SAMPLE);
  const info = useDocStore.getState().info!;
  useDocStore.setState({ info: { ...info, dirty: true } });
}

describe("dialogs.unsaved.flow", () => {
  it("does not prompt when there is nothing to lose", async () => {
    expect(await confirmUnsaved()).toBe(true);
    await useDocStore.getState().open(SAMPLE);
    expect(useDocStore.getState().info?.dirty).toBe(false);
    expect(await confirmUnsaved()).toBe(true);
    expect(useDialogStore.getState().stack).toHaveLength(0);
  });

  it("저장 안 함 discards and proceeds", async () => {
    await openDirty();
    render(<DialogHost />);
    const answer = confirmUnsaved();
    expect(await screen.findByText(/변경 사항을 저장하시겠습니까/)).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "저장 안 함" }));
    expect(await answer).toBe(true);
    expect(useDocStore.getState().info?.dirty).toBe(true); // discarded, not saved
    await waitFor(() => expect(useDialogStore.getState().stack).toHaveLength(0));
  });

  it("취소 stops the caller", async () => {
    await openDirty();
    render(<DialogHost />);
    const answer = confirmUnsaved();
    await screen.findByText(/변경 사항을 저장하시겠습니까/);
    fireEvent.click(screen.getByRole("button", { name: "취소" }));
    expect(await answer).toBe(false);
  });

  it("Esc is 취소", async () => {
    await openDirty();
    render(<DialogHost />);
    const answer = confirmUnsaved();
    const dialog = await screen.findByRole("dialog");
    fireEvent.keyDown(dialog, { key: "Escape" });
    expect(await answer).toBe(false);
  });

  it("저장 writes the document and then proceeds", async () => {
    await openDirty();
    render(<DialogHost />);
    const answer = confirmUnsaved();
    await screen.findByText(/변경 사항을 저장하시겠습니까/);
    fireEvent.click(screen.getByRole("button", { name: "저장" }));
    expect(await answer).toBe(true);
    await waitFor(() => expect(useDocStore.getState().info?.dirty).toBe(false));
  });

  it("closing a dirty document runs the same gate and can be cancelled", async () => {
    await openDirty();
    render(<DialogHost />);
    const closing = closeDocumentFlow();
    await screen.findByText(/변경 사항을 저장하시겠습니까/);
    fireEvent.click(screen.getByRole("button", { name: "취소" }));
    expect(await closing).toBe(false);
    expect(useDocStore.getState().info).not.toBeNull();
  });

  it("saving a document that has a path never opens the save panel", async () => {
    await openDirty();
    expect(await saveFlow()).toBe(true);
    expect(useDocStore.getState().info?.path).toBe(SAMPLE);
    expect(useDocStore.getState().info?.dirty).toBe(false);
  });
});
