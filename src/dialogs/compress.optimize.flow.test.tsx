/**
 * v0.3 pkg8 (X5) — 압축 ▸ 구조 최적화: the option reaches `compress_estimate`, and a smaller file
 * from the structural pass alone (no image above 300 DPI) can be applied.
 */
import { beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import DialogHost from "./DialogHost";
import { openDialog, useDialogStore } from "./dialogState";
import { useDocStore } from "../store/docStore";
import { useToastStore } from "../app/toastStore";
import { mock } from "../ipc/mock";
import { applyBlock, buildCompressOptions } from "./compress";

const SAMPLE = "/Users/veri/Documents/SeePDF-샘플.pdf";

beforeEach(() => {
  useDialogStore.getState().closeAll();
  useToastStore.setState({ toasts: [] });
});

describe("dialogs.compress.optimize", () => {
  it("builds the option and lets a structural-only saving apply", () => {
    expect(buildCompressOptions(150, null, true, true)).toEqual({ targetDpi: 150, optimize: true });
    expect(buildCompressOptions(150, [1], false)).toEqual({ targetDpi: 150, pages: [1] });
    const structuralOnly = { beforeBytes: 1000, afterBytes: 880, imagesDownsampled: 0 };
    expect(applyBlock(structuralOnly)).toBe("noImages");
    expect(applyBlock(structuralOnly, true)).toBeNull();
  });

  it("구조 최적화 at 300 DPI: estimate with optimize, 적용 on, the document gets smaller", async () => {
    const estimate = vi.spyOn(mock, "compressEstimate");
    await useDocStore.getState().open(SAMPLE);
    const before = useDocStore.getState().info!.bytes;
    render(<DialogHost />);
    openDialog("compress");
    const run = await screen.findByRole("button", { name: "예상" });
    fireEvent.click(screen.getByRole("radio", { name: /300 DPI/ }));
    fireEvent.click(screen.getByRole("checkbox", { name: "구조 최적화" }));
    fireEvent.click(run);
    await screen.findByText("현재 크기", {}, { timeout: 2000 });
    expect(estimate.mock.calls[0][0].options).toEqual({ targetDpi: 300, optimize: true });
    const primary = screen.getByRole("button", { name: "적용" });
    expect(primary).toBeEnabled();
    fireEvent.click(primary);
    await waitFor(() => expect(useDialogStore.getState().stack).toHaveLength(0));
    const info = useDocStore.getState().info!;
    expect(info.undoLabel).toBe("undo.compress");
    expect(info.bytes).toBeLessThan(before);
  });
});
