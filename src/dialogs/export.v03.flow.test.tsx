/**
 * v0.3 pkg8 — 내보내기 additions: every new format / option reaches the right command.
 * X2 N-up PDF, X3 TIFF / 이미지 추출 / 이어 붙이기 / 레이아웃 유지, X6 Word·한글·HTML·Markdown.
 */
import { beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import * as api from "../ipc/api";
import { mock } from "../ipc/mock";
import { ExportDialog } from "./ExportDialog";
import { useDocStore } from "../store/docStore";
import { useToastStore } from "../app/toastStore";

vi.mock("../ipc/api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../ipc/api")>();
  return { ...actual, openFileDialog: vi.fn(), saveFileDialog: vi.fn() };
});

const SAMPLE = "/Users/veri/Documents/SeePDF-샘플.pdf"; // 3 pages

beforeEach(async () => {
  useToastStore.setState({ toasts: [] });
  vi.mocked(api.openFileDialog).mockResolvedValue(["/out"]);
  vi.mocked(api.saveFileDialog).mockImplementation(async (o) => `/out/${o?.defaultPath ?? "x"}`);
  await useDocStore.getState().open(SAMPLE);
  await waitFor(() => expect(useDocStore.getState().info).not.toBeNull());
});

function exportAs(label: RegExp | string) {
  const onClose = vi.fn();
  render(<ExportDialog onClose={onClose} />);
  fireEvent.click(screen.getByRole("option", { name: label }));
  return onClose;
}

async function go(onClose: ReturnType<typeof vi.fn>) {
  fireEvent.click(screen.getByRole("button", { name: "내보내기" }));
  await waitFor(() => expect(onClose).toHaveBeenCalled());
}

describe("export v0.3", () => {
  it("TIFF writes one multi-page file at the chosen DPI", async () => {
    const tiff = vi.spyOn(mock, "exportTiff");
    const close = exportAs(/TIFF/);
    fireEvent.change(screen.getByLabelText("해상도 (DPI)"), { target: { value: "200" } });
    await go(close);
    expect(tiff.mock.calls[0][0]).toEqual({ docId: "d1", pages: [0, 1, 2], dpi: 200, outPath: "/out/SeePDF-샘플.tiff" });
  });

  it("PNG + 하나의 이미지로 이어 붙이기 calls the stitched export and says when the DPI was lowered", async () => {
    const stitched = vi.spyOn(mock, "exportStitchedImage");
    const images = vi.spyOn(mock, "exportImages");
    const close = exportAs("PNG 이미지");
    fireEvent.click(screen.getByRole("checkbox", { name: "하나의 이미지로 이어 붙이기" }));
    fireEvent.change(screen.getByLabelText("해상도 (DPI)"), { target: { value: "600" } });
    await go(close);
    expect(stitched.mock.calls[0][0]).toMatchObject({ format: "png", dpi: 600, outPath: "/out/SeePDF-샘플.png" });
    expect(images).not.toHaveBeenCalled();
  });

  it("이미지 추출 saves the embedded images into the chosen folder", async () => {
    const embedded = vi.spyOn(mock, "exportEmbeddedImages");
    const close = exportAs("이미지 추출");
    await go(close);
    expect(embedded.mock.calls[0][0]).toEqual({ docId: "d1", pages: [0, 1, 2], outDir: "/out", baseName: "SeePDF-샘플" });
    await waitFor(() => expect(useToastStore.getState().toasts.at(-1)?.messageKey).toBe("export.embedded.done"));
  });

  it("텍스트 + 레이아웃 유지 passes preserveLayout", async () => {
    const text = vi.spyOn(mock, "exportText");
    const close = exportAs(/텍스트/);
    fireEvent.click(screen.getByRole("checkbox", { name: "레이아웃 유지 시도" }));
    await go(close);
    expect(text.mock.calls[0][0]).toMatchObject({ preserveLayout: true, outPath: "/out/SeePDF-샘플.txt" });
  });

  it("Word / 한글 / HTML / Markdown export the text flow and warn that formatting is lost", async () => {
    const flow = vi.spyOn(mock, "exportTextFlow");
    for (const [label, format, ext] of [
      [/Word/, "docx", "docx"],
      [/한글 문서/, "hwpx", "hwpx"],
      [/웹 페이지/, "html", "html"],
      [/Markdown \(\.md\)/, "md", "md"],
    ] as const) {
      const close = exportAs(label);
      expect(screen.getByTestId("export-lossy")).toHaveTextContent("서식 일부 유실");
      await go(close);
      expect(flow.mock.calls.at(-1)![0]).toEqual({ docId: "d1", pages: [0, 1, 2], format, outPath: `/out/SeePDF-샘플.${ext}` });
      document.body.innerHTML = "";
    }
  });

  it("모아찍기 PDF writes an n-up (or booklet) file", async () => {
    const nup = vi.spyOn(mock, "makeNup");
    let close = exportAs(/N-up/);
    fireEvent.change(screen.getByLabelText("모아찍기"), { target: { value: "4" } });
    fireEvent.change(screen.getByLabelText("순서"), { target: { value: "down" } });
    await go(close);
    expect(nup.mock.calls[0][0]).toMatchObject({
      docId: "d1", outPath: "/out/SeePDF-샘플-4up.pdf",
      options: { perSheet: 4, order: "down", booklet: false, paper: "a4" },
    });
    document.body.innerHTML = "";

    close = exportAs(/N-up/);
    fireEvent.click(screen.getByRole("checkbox", { name: "소책자 (중철)" }));
    await go(close);
    expect(nup.mock.calls[1][0]).toMatchObject({ options: { perSheet: 2, booklet: true } });
  });
});
