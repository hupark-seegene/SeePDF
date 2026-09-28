/**
 * Closing the window is the same gate as closing the document (F-23): pending 편집 · 영역 표시
 * marks first (F-22 — they are not in the document, so they make it neither dirty nor saved), then
 * 저장 / 저장 안 함 / 취소.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { waitFor } from "@testing-library/react";
import { openPath, windowCloseGate } from "./flows";
import { useDialogStore, type UnsavedAnswer } from "./dialogState";
import { useDocStore } from "../store/docStore";
import { setEditLeaveGuard } from "../tools/commands";

const A = "/Users/veri/Documents/SeePDF-샘플.pdf";

async function answerUnsaved(answer: UnsavedAnswer): Promise<void> {
  await waitFor(() => expect(useDialogStore.getState().stack.at(-1)?.name).toBe("unsaved"));
  const entry = useDialogStore.getState().stack.at(-1)!;
  (entry.props.resolve as (v: UnsavedAnswer) => void)(answer);
}

describe("dialogs.flows.windowCloseGate", () => {
  beforeEach(() => {
    useDocStore.setState({ docId: null, info: null, outline: [], status: "empty", error: null });
    useDialogStore.getState().closeAll();
  });
  afterEach(() => setEditLeaveGuard(null));

  it("a clean document with nothing pending just closes", async () => {
    await openPath(A);
    expect(await windowCloseGate()).toBe("close");
  });

  it("pending 영역 표시 marks hold the close even when the document is clean", async () => {
    await openPath(A);
    const guard = vi.fn(() => Promise.resolve(false)); // 취소 on the F-22 prompt
    setEditLeaveGuard(guard);
    expect(await windowCloseGate()).toBe("cancel");
    expect(guard).toHaveBeenCalledOnce();

    guard.mockImplementation(() => Promise.resolve(true)); // 버리기
    expect(await windowCloseGate()).toBe("confirmed");
  });

  it("marks first, then the unsaved prompt", async () => {
    const info = (await openPath(A))!;
    useDocStore.setState({ info: { ...info, dirty: true } });
    const guard = vi.fn(() => Promise.resolve(true));
    setEditLeaveGuard(guard);
    const gate = windowCloseGate();
    await answerUnsaved("cancel");
    expect(await gate).toBe("cancel");
    expect(guard).toHaveBeenCalledOnce();
  });
});
