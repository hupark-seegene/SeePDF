/**
 * The OCR sheet (UI_SPEC §10): range · languages · options · 예상 시간 → progress → result.
 * `runOcrJob` is stubbed; `ocrJob.test.ts` owns the orchestration.
 */
import { beforeEach, describe, expect, it, vi } from "vitest";
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { t } from "../i18n";
import { useJobStore } from "../store/jobStore";
import type { OcrJobResult, OcrRunOptions } from "./ocrJob";

const runOcrJob = vi.fn();

vi.mock("./ocrJob", async (importOriginal) => {
  const actual = await importOriginal<typeof import("./ocrJob")>();
  return { ...actual, runOcrJob: (o: OcrRunOptions) => runOcrJob(o) };
});

import { OcrDialog } from "./OcrDialog";
import { closeOcrDialog, openOcrDialog } from "./dialogState";
import { mock } from "../ipc/mock";
import { loadOcrCapabilities, resetOcrCapabilities } from "./engine";

/** `ocr_capabilities` as a machine without Apple Vision (Windows, macOS 12) or a Mac with it. */
function capabilities(vision: boolean) {
  resetOcrCapabilities();
  vi.spyOn(mock, "ocrCapabilities").mockResolvedValue({
    engines: vision ? ["tesseract", "vision"] : ["tesseract"], languages: ["kor", "eng"],
  });
}

const CONTEXT = { docId: "d1", docGeneration: 1, pageCount: 14, currentPage: 3 };

function result(over: Partial<OcrJobResult> = {}): OcrJobResult {
  return {
    jobId: -1, status: "done", applied: [0, 1], skipped: [], words: 42, confidence: 91,
    elapsedMs: 1200, messageKey: "ocr.done", ...over,
  };
}

beforeEach(() => {
  closeOcrDialog();
  useJobStore.setState({ jobs: [], active: null });
  runOcrJob.mockReset().mockResolvedValue(result());
  capabilities(false);
});

describe("ocr.dialog", () => {
  it("renders nothing until openOcrDialog() is called", () => {
    const { container } = render(<OcrDialog />);
    expect(container).toBeEmptyDOMElement();
    act(() => openOcrDialog(CONTEXT));
    expect(screen.getByRole("dialog")).toBeInTheDocument();
    expect(screen.getByText(t("ocr.title"))).toBeInTheDocument();
  });

  it("offers 모든 페이지 / 현재 페이지 / 페이지 범위 with Korean + English preselected", () => {
    render(<OcrDialog />);
    act(() => openOcrDialog(CONTEXT));
    expect(screen.getByRole("radio", { name: t("ocr.range.all") })).toHaveAttribute("aria-checked", "true");
    expect(screen.getByRole("radio", { name: t("ocr.range.current") })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: t("ocr.language.ko") })).toHaveAttribute("aria-pressed", "true");
    expect(screen.getByRole("button", { name: t("ocr.language.en") })).toHaveAttribute("aria-pressed", "true");
    expect(screen.getByLabelText(t("ocr.option.skipText"))).toBeChecked();
  });

  it("shows ocr.range.invalid and blocks 시작 for a bad range", async () => {
    render(<OcrDialog />);
    act(() => openOcrDialog(CONTEXT));
    fireEvent.click(screen.getByRole("radio", { name: t("ocr.range.custom") }));
    fireEvent.change(screen.getByLabelText(t("ocr.range.custom")), { target: { value: "20" } });
    expect(screen.getByText(t("ocr.range.invalid"))).toBeInTheDocument();
    expect(screen.getByRole("button", { name: t("ocr.start") })).toBeDisabled();

    fireEvent.change(screen.getByLabelText(t("ocr.range.custom")), { target: { value: "1-3,5" } });
    expect(screen.queryByText(t("ocr.range.invalid"))).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: t("ocr.start") })).toBeEnabled();
  });

  it("starts the job with the chosen range, languages and options", async () => {
    render(<OcrDialog />);
    act(() => openOcrDialog(CONTEXT));
    fireEvent.click(screen.getByRole("radio", { name: t("ocr.range.current") }));
    fireEvent.click(screen.getByLabelText(t("ocr.option.skipText")));   // turn it off
    fireEvent.click(screen.getByRole("button", { name: t("ocr.start") }));

    await waitFor(() => expect(runOcrJob).toHaveBeenCalled());
    expect(runOcrJob.mock.calls[0][0]).toMatchObject({
      docId: "d1", pages: [3], langs: "kor+eng", layout: "column", skipPagesWithText: false,
    });
  });

  it("never sends `kor` on its own (spike §4.2)", async () => {
    render(<OcrDialog />);
    act(() => openOcrDialog(CONTEXT));
    fireEvent.click(screen.getByRole("button", { name: t("ocr.language.en") }));   // English off
    fireEvent.click(screen.getByRole("button", { name: t("ocr.start") }));
    await waitFor(() => expect(runOcrJob).toHaveBeenCalled());
    expect(runOcrJob.mock.calls[0][0].langs).toBe("kor+eng");
  });

  it("switches to a progress view with a live 취소, then shows the result", async () => {
    let finish: (r: OcrJobResult) => void = () => {};
    runOcrJob.mockImplementation((o: OcrRunOptions) => new Promise<OcrJobResult>((resolve) => {
      finish = resolve;
      // The job is what pushes progress into the store; imitate one page of 14.
      useJobStore.getState().start({ id: o.jobId!, kind: "ocr", labelKey: "status.ocr", total: 14 });
      useJobStore.getState().update(o.jobId!, { done: 1, total: 14 });
    }));

    render(<OcrDialog />);
    act(() => openOcrDialog(CONTEXT));
    fireEvent.click(screen.getByRole("button", { name: t("ocr.start") }));

    await screen.findByRole("progressbar");
    expect(screen.getByRole("progressbar")).toHaveAttribute("aria-valuenow", "7");
    expect(screen.getByText(t("ocr.progress", { current: 2, total: 14 }))).toBeInTheDocument();
    expect(screen.getByRole("button", { name: t("common.cancel") })).toBeEnabled();

    await act(async () => { finish(result({ applied: [0, 1, 2], words: 300, confidence: 88 })); });
    await screen.findByText(t("ocr.done"));
    expect(screen.getByText(new RegExp(t("ocr.doneCount", { count: 3 })))).toBeInTheDocument();
    expect(screen.getByRole("button", { name: t("common.undo") })).toBeInTheDocument();
  });

  it("cancels through the job store", async () => {
    const cancel = vi.fn();
    runOcrJob.mockImplementation((o: OcrRunOptions) => new Promise<OcrJobResult>(() => {
      useJobStore.getState().start({ id: o.jobId!, kind: "ocr", labelKey: "status.ocr", total: 3 });
    }));
    useJobStore.setState({ cancel: async (id) => { cancel(id); } });

    render(<OcrDialog />);
    act(() => openOcrDialog(CONTEXT));
    fireEvent.click(screen.getByRole("button", { name: t("ocr.start") }));
    await screen.findByRole("progressbar");
    fireEvent.click(screen.getByRole("button", { name: t("common.cancel") }));
    expect(cancel).toHaveBeenCalledWith(expect.any(Number));
  });

  it("reports a skip-everything run as ocr.noImagePages", async () => {
    runOcrJob.mockResolvedValue(result({ applied: [], skipped: [0, 1], messageKey: "ocr.noImagePages" }));
    render(<OcrDialog />);
    act(() => openOcrDialog(CONTEXT));
    fireEvent.click(screen.getByRole("button", { name: t("ocr.start") }));
    await screen.findByText(t("ocr.noImagePages"));
    expect(screen.getByText(t("ocr.skipped", { count: 2 }))).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: t("common.undo") })).not.toBeInTheDocument();
  });

  it("exposes the advanced layout option", async () => {
    render(<OcrDialog />);
    act(() => openOcrDialog(CONTEXT));
    expect(screen.queryByLabelText(t("ocr.layout"))).not.toBeInTheDocument();
    // the disclosure button carries a caret glyph before its label
    fireEvent.click(screen.getByRole("button", { name: new RegExp(t("settings.tab.advanced")) }));
    const select = screen.getByLabelText(t("ocr.layout"));
    expect(select).toHaveValue("column");
    fireEvent.change(select, { target: { value: "block" } });
    fireEvent.click(screen.getByRole("button", { name: t("ocr.start") }));
    await waitFor(() => expect(runOcrJob).toHaveBeenCalled());
    expect(runOcrJob.mock.calls[0][0].layout).toBe("block");
  });
});

describe("ocr.dialog.engine", () => {
  it("has no 인식 엔진 row without Apple Vision, and runs Tesseract", async () => {
    render(<OcrDialog />);
    act(() => openOcrDialog(CONTEXT));
    // Let the capability answer land before asserting the row stays away.
    await act(async () => { await loadOcrCapabilities(); });
    expect(screen.queryByLabelText(t("ocr.engine"))).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: t("ocr.start") }));
    await waitFor(() => expect(runOcrJob).toHaveBeenCalled());
    expect(runOcrJob.mock.calls[0][0].engine).toBe("tesseract");
  });

  it("offers 자동 / Apple Vision / Tesseract where Vision exists; 자동 runs Vision", async () => {
    capabilities(true);
    render(<OcrDialog />);
    act(() => openOcrDialog(CONTEXT));
    const select = await screen.findByLabelText(t("ocr.engine"));
    expect(select).toHaveValue("auto");
    expect([...(select as HTMLSelectElement).options].map((o) => o.textContent)).toEqual([
      t("ocr.engine.auto"), t("ocr.engine.vision"), t("ocr.engine.tesseract"),
    ]);
    expect(screen.getByText(t("ocr.engine.autoHint"))).toBeInTheDocument();
    // Vision's estimate: 14 pages × 0.8 s, no worker speed-up.
    expect(screen.getByText(t("ocr.estimate", { seconds: 11 }))).toBeInTheDocument();
    // 레이아웃 is a Tesseract knob: not offered while Vision runs.
    fireEvent.click(screen.getByRole("button", { name: new RegExp(t("settings.tab.advanced")) }));
    expect(screen.queryByLabelText(t("ocr.layout"))).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: t("ocr.start") }));
    await waitFor(() => expect(runOcrJob).toHaveBeenCalled());
    expect(runOcrJob.mock.calls[0][0]).toMatchObject({ engine: "vision", langs: "kor+eng", pages: expect.any(Array) });
  });

  it("Tesseract picked explicitly runs Tesseract and brings 레이아웃 back", async () => {
    capabilities(true);
    render(<OcrDialog />);
    act(() => openOcrDialog(CONTEXT));
    fireEvent.change(await screen.findByLabelText(t("ocr.engine")), { target: { value: "tesseract" } });
    expect(screen.queryByText(t("ocr.engine.autoHint"))).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: new RegExp(t("settings.tab.advanced")) }));
    expect(screen.getByLabelText(t("ocr.layout"))).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: t("ocr.start") }));
    await waitFor(() => expect(runOcrJob).toHaveBeenCalled());
    expect(runOcrJob.mock.calls[0][0].engine).toBe("tesseract");
  });

  it("a 시작 faster than the capability answer still runs Vision under 자동", async () => {
    capabilities(true);
    render(<OcrDialog />);
    act(() => openOcrDialog(CONTEXT));
    fireEvent.click(screen.getByRole("button", { name: t("ocr.start") }));   // before the row appears
    await waitFor(() => expect(runOcrJob).toHaveBeenCalled());
    expect(runOcrJob.mock.calls[0][0].engine).toBe("vision");
  });
});
