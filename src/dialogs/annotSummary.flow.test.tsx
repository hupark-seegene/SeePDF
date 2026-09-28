/**
 * P2 주석 목록 내보내기: the 주석 sidebar's 내보내기… opens 내보내기 on the 주석 목록 format; the
 * format radio picks the file type (CSV default), the save panel suggests `<name>-주석 목록.<ext>`,
 * and `export_annotation_summary` gets the range and the app language.
 */
import { beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import DialogHost from "./DialogHost";
import { openDialog, useDialogStore } from "./dialogState";
import { useDocStore } from "../store/docStore";
import { useToastStore } from "../app/toastStore";
import { mock } from "../ipc/mock";
import * as api from "../ipc/api";
import { AnnotationList } from "../sidebar/AnnotationList";

const SAMPLE = "/Users/veri/Documents/SeePDF-샘플.pdf"; // 3 pages, a highlight on 1, a note on 2

beforeEach(() => {
  useDialogStore.getState().closeAll();
  useToastStore.setState({ toasts: [] });
  vi.restoreAllMocks();
});

describe("dialogs.annotSummary.flow", () => {
  it("주석 목록 → 내보내기… opens the export dialog on the annotation summary", async () => {
    await useDocStore.getState().open(SAMPLE);
    render(<AnnotationList />);
    fireEvent.click(screen.getByRole("button", { name: "내보내기…" }));
    expect(useDialogStore.getState().stack.at(-1)).toMatchObject({ name: "export", props: { format: "annotations" } });
  });

  it("CSV by default, the whole document, a toast with Finder에서 보기", async () => {
    await useDocStore.getState().open(SAMPLE);
    const save = vi.spyOn(api, "saveFileDialog");
    const exportSummary = vi.spyOn(mock, "exportAnnotationSummary");
    render(<DialogHost />);
    openDialog("export", { format: "annotations" });
    expect(await screen.findByRole("option", { name: "주석 목록" })).toHaveAttribute("aria-selected", "true");
    expect(screen.getByRole("radio", { name: "CSV — Excel (.csv)" })).toBeChecked();
    fireEvent.click(screen.getByRole("button", { name: "내보내기" }));
    await waitFor(() => expect(exportSummary).toHaveBeenCalledTimes(1));
    expect(save.mock.calls[0][0]).toMatchObject({
      defaultPath: "SeePDF-샘플-주석 목록.csv",
      filters: [{ name: "CSV", extensions: ["csv"] }],
    });
    expect(exportSummary.mock.calls[0][0]).toMatchObject({
      path: "/Users/veri/Documents/SeePDF-샘플-주석 목록.csv",
      format: "csv",
      pages: undefined,
      locale: "ko",
    });
    await waitFor(() =>
      expect(useToastStore.getState().toasts.at(-1)).toMatchObject({ messageKey: "export.annotations.done", params: { count: 2 } }),
    );
    expect(useToastStore.getState().toasts.at(-1)?.actions?.length).toBe(1);
  });

  it("Markdown for one page; an empty page says so", async () => {
    await useDocStore.getState().open(SAMPLE);
    const exportSummary = vi.spyOn(mock, "exportAnnotationSummary");
    render(<DialogHost />);
    openDialog("export", { format: "annotations" });
    fireEvent.click(await screen.findByRole("radio", { name: "Markdown (.md)" }));
    fireEvent.click(screen.getByRole("radio", { name: "직접 입력" }));
    fireEvent.change(screen.getByLabelText("페이지 범위"), { target: { value: "3" } });
    fireEvent.click(screen.getByRole("button", { name: "내보내기" }));
    await waitFor(() => expect(exportSummary).toHaveBeenCalledTimes(1));
    expect(exportSummary.mock.calls[0][0]).toMatchObject({ format: "md", pages: [2] });
    expect(exportSummary.mock.calls[0][0].path.endsWith(".md")).toBe(true);
    await waitFor(() => expect(useToastStore.getState().toasts.at(-1)?.messageKey).toBe("export.annotations.empty"));
  });
});
