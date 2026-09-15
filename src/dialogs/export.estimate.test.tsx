import { describe, expect, it } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { ExportDialog } from "./ExportDialog";
import { useDocStore } from "../store/docStore";
import { usePagesStore } from "../store/pagesStore";

const SAMPLE = "/Users/veri/Documents/SeePDF-샘플.pdf"; // 3 pages

async function openSample() {
  await useDocStore.getState().open(SAMPLE);
  await waitFor(() => expect(useDocStore.getState().info).not.toBeNull());
}

describe("dialogs.export.estimate", () => {
  it("shows an estimated size for an image export and updates it with the DPI", async () => {
    await openSample();
    render(<ExportDialog onClose={() => {}} />);

    const before = await screen.findByText(/예상 크기/);
    const firstText = before.textContent ?? "";
    expect(firstText).toMatch(/KB|MB/);

    fireEvent.change(screen.getByLabelText("해상도 (DPI)"), { target: { value: "300" } });
    await waitFor(() => {
      expect(screen.getByText(/예상 크기/).textContent).not.toBe(firstText);
    });
  });

  it("estimates only the pages in the chosen range", async () => {
    await openSample();
    usePagesStore.getState().setSelected([1]);
    render(<ExportDialog onClose={() => {}} />);

    const all = (await screen.findByText(/예상 크기/)).textContent ?? "";
    fireEvent.click(screen.getByRole("radio", { name: "선택한 페이지" }));
    await waitFor(() => {
      expect(screen.getByText(/예상 크기/).textContent).not.toBe(all);
    });
  });

  it("has no estimate for text export and keeps 내보내기 enabled", async () => {
    await openSample();
    render(<ExportDialog onClose={() => {}} />);
    await screen.findByText(/예상 크기/);
    fireEvent.click(screen.getByRole("option", { name: /텍스트/ }));
    await waitFor(() => expect(screen.queryByText(/예상 크기/)).toBeNull());
    expect(screen.getByRole("button", { name: "내보내기" })).not.toBeDisabled();
  });
});
