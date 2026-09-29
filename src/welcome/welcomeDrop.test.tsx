/**
 * v0.3 H7: the Welcome drop zone highlights from the window-level drag state (`onFileDragState`),
 * not from its own HTML5 `dragover` — which never fires on Windows under Tauri's file drop.
 */
import { beforeEach, describe, expect, it } from "vitest";
import { act, render, screen } from "@testing-library/react";
import { Welcome } from "./Welcome";
import { useDocStore } from "../store/docStore";

beforeEach(() => {
  useDocStore.setState({ docId: null, info: null, outline: [], status: "empty", error: null });
});

describe("welcome drop highlight", () => {
  it("lights up while files are dragged over the window and clears on leave", async () => {
    const { container } = render(<Welcome />);
    await screen.findByText("SeePDF-샘플.pdf");
    const root = container.querySelector("[data-dragover]");
    expect(root).toBeNull();
    act(() => {
      window.dispatchEvent(new Event("dragenter"));
    });
    expect(container.querySelector("[data-dragover]")).not.toBeNull();
    act(() => {
      window.dispatchEvent(new Event("dragleave"));
    });
    expect(container.querySelector("[data-dragover]")).toBeNull();
  });
});
