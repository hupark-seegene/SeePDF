/**
 * v0.3 pkg7-ocr in the two OCR dialogs:
 *
 *   U1  설정's 언어 / 해상도 (`Settings.ocrLanguages`, `ocrDpi`) seed the OCR sheet and 여러 파일 OCR
 *   O1  the language chips follow the engine: 日本語 / 中文 with Vision (or staged traineddata)
 *   O2  페이지 회전 자동 감지 reaches the job
 *   O3  Windows OCR is the native engine on Windows
 *
 * `runOcrJob` is stubbed for the sheet (as in `OcrDialog.test.tsx`); the batch dialog only needs to render.
 */
import { beforeEach, describe, expect, it, vi } from "vitest";
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { t } from "../i18n";
import { mock } from "../ipc/mock";
import type { OcrCapabilities, Settings } from "../ipc/types";
import { useAppStore } from "../store/appStore";
import { useJobStore } from "../store/jobStore";
import settingsFixture from "../test/ipc-samples/settings.json";
import type { OcrJobResult, OcrRunOptions } from "./ocrJob";

const runOcrJob = vi.fn();

vi.mock("./ocrJob", async (importOriginal) => {
  const actual = await importOriginal<typeof import("./ocrJob")>();
  return { ...actual, runOcrJob: (o: OcrRunOptions) => runOcrJob(o) };
});

import { OcrDialog } from "./OcrDialog";
import { closeOcrDialog, openOcrDialog } from "./dialogState";
import { loadOcrCapabilities, resetOcrCapabilities } from "./engine";
import { langsFor } from "./languages";
import DialogHost from "../dialogs/DialogHost";
import { openDialog, useDialogStore } from "../dialogs/dialogState";
import { resetBatch, useBatchOcr } from "./batch/flow";

const CONTEXT = { docId: "d1", docGeneration: 1, pageCount: 4, currentPage: 0 };

function capabilities(caps: OcrCapabilities) {
  resetOcrCapabilities();
  vi.spyOn(mock, "ocrCapabilities").mockResolvedValue(caps);
}

const TESSERACT_ONLY: OcrCapabilities = {
  engines: ["tesseract"], languages: ["kor", "eng"],
  engineLanguages: { tesseract: ["kor", "eng"], vision: [], windows: [] },
};

function settings(patch: Partial<Settings>) {
  useAppStore.setState({ settings: { ...(settingsFixture as unknown as Settings), ...patch } });
}

function result(): OcrJobResult {
  return {
    jobId: -1, status: "done", applied: [0], skipped: [], words: 4, confidence: 90, elapsedMs: 10,
    messageKey: "ocr.done",
  };
}

const chip = (key: string) => screen.queryByRole("button", { name: t(key) });

beforeEach(() => {
  vi.restoreAllMocks();
  closeOcrDialog();
  useDialogStore.getState().closeAll();
  useJobStore.setState({ jobs: [], active: null });
  runOcrJob.mockReset().mockResolvedValue(result());
  capabilities(TESSERACT_ONLY);
  settings({ ocrLanguages: ["kor", "eng"], ocrDpi: "auto" });
  resetBatch();
});

describe("U1 — 설정 seeds the OCR dialogs", () => {
  it("the OCR sheet opens with 설정's English only and 300 DPI", async () => {
    settings({ ocrLanguages: ["eng"], ocrDpi: 300 });
    render(<OcrDialog />);
    act(() => openOcrDialog(CONTEXT));
    expect(chip("ocr.language.ko")).toHaveAttribute("aria-pressed", "false");
    expect(chip("ocr.language.en")).toHaveAttribute("aria-pressed", "true");
    expect(screen.getByLabelText(t("ocr.option.dpi"))).toHaveValue("300");

    fireEvent.click(screen.getByRole("button", { name: t("ocr.start") }));
    await waitFor(() => expect(runOcrJob).toHaveBeenCalled());
    expect(runOcrJob.mock.calls[0][0]).toMatchObject({ langs: "eng", dpi: 300 });
  });

  it("without 설정 the sheet starts at 한국어 + English / 자동", async () => {
    useAppStore.setState({ settings: null });
    render(<OcrDialog />);
    act(() => openOcrDialog(CONTEXT));
    expect(chip("ocr.language.ko")).toHaveAttribute("aria-pressed", "true");
    expect(chip("ocr.language.en")).toHaveAttribute("aria-pressed", "true");
    fireEvent.click(screen.getByRole("button", { name: t("ocr.start") }));
    await waitFor(() => expect(runOcrJob).toHaveBeenCalled());
    expect(runOcrJob.mock.calls[0][0]).toMatchObject({ langs: "kor+eng", dpi: "auto" });
  });

  it("여러 파일 OCR opens with 설정's English only and 300 DPI", async () => {
    settings({ ocrLanguages: ["eng"], ocrDpi: 300 });
    render(<DialogHost />);
    act(() => openDialog("batchOcr"));
    await screen.findByRole("button", { name: t("ocr.start") });
    expect(chip("ocr.language.ko")).toHaveAttribute("aria-pressed", "false");
    expect(chip("ocr.language.en")).toHaveAttribute("aria-pressed", "true");
    expect(screen.getByLabelText(t("ocr.option.dpi"))).toHaveValue("300");
    expect(useBatchOcr.getState().options).toMatchObject({ langs: ["eng"], dpi: 300 });
  });
});

describe("O1 — languages per engine", () => {
  it("staged jpn traineddata brings the 日本語 chip for Tesseract, and langsFor joins in chip order", async () => {
    capabilities({
      engines: ["tesseract"], languages: ["kor", "eng"],
      engineLanguages: { tesseract: ["kor", "eng", "jpn"], vision: [], windows: [] },
    });
    render(<OcrDialog />);
    act(() => openOcrDialog(CONTEXT));
    await act(async () => { await loadOcrCapabilities(); });
    const ja = chip("ocr.language.ja");
    expect(ja).toBeInTheDocument();
    expect(chip("ocr.language.zh")).not.toBeInTheDocument();
    fireEvent.click(ja!);
    expect(ja).toHaveAttribute("aria-pressed", "true");
    fireEvent.click(screen.getByRole("button", { name: t("ocr.start") }));
    await waitFor(() => expect(runOcrJob).toHaveBeenCalled());
    expect(runOcrJob.mock.calls[0][0].langs).toBe("kor+eng+jpn");
    expect(langsFor(["jpn", "kor"])).toBe("kor+eng+jpn");
    expect(langsFor(["chi_sim"])).toBe("chi_sim");
    expect(langsFor([])).toBe("kor+eng");
  });

  it("Tesseract without staged data offers only 한국어 / English", async () => {
    render(<OcrDialog />);
    act(() => openOcrDialog(CONTEXT));
    await act(async () => { await loadOcrCapabilities(); });
    expect(chip("ocr.language.ja")).not.toBeInTheDocument();
    expect(chip("ocr.language.zh")).not.toBeInTheDocument();
  });

  it("Vision offers 日本語 and 中文; switching to Tesseract drops them from the run", async () => {
    capabilities({
      engines: ["tesseract", "vision"], languages: ["kor", "eng"],
      engineLanguages: { tesseract: ["kor", "eng"], vision: ["kor", "eng", "jpn", "chi_sim"], windows: [] },
    });
    render(<OcrDialog />);
    act(() => openOcrDialog(CONTEXT));
    await screen.findByLabelText(t("ocr.engine"));
    fireEvent.click(chip("ocr.language.zh")!);
    expect(chip("ocr.language.zh")).toHaveAttribute("aria-pressed", "true");

    fireEvent.change(screen.getByLabelText(t("ocr.engine")), { target: { value: "tesseract" } });
    expect(chip("ocr.language.zh")).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: t("ocr.start") }));
    await waitFor(() => expect(runOcrJob).toHaveBeenCalled());
    expect(runOcrJob.mock.calls[0][0]).toMatchObject({ engine: "tesseract", langs: "kor+eng" });
  });

  it("여러 파일 OCR shows the same engine chips", async () => {
    capabilities({
      engines: ["tesseract", "vision"], languages: ["kor", "eng"],
      engineLanguages: { tesseract: ["kor", "eng"], vision: ["kor", "eng", "jpn", "chi_sim"], windows: [] },
    });
    render(<DialogHost />);
    act(() => openDialog("batchOcr"));
    await screen.findByLabelText(t("ocr.engine"));
    expect(chip("ocr.language.ja")).toBeInTheDocument();
    fireEvent.click(chip("ocr.language.ja")!);
    expect(useBatchOcr.getState().options.langs).toEqual(["kor", "eng", "jpn"]);
  });
});

describe("O2 / O3 — options and engines", () => {
  it("페이지 회전 자동 감지 is off by default and reaches the job when ticked", async () => {
    render(<OcrDialog />);
    act(() => openOcrDialog(CONTEXT));
    const box = screen.getByLabelText(t("ocr.option.autoRotate"));
    expect(box).not.toBeChecked();
    fireEvent.click(box);
    fireEvent.click(screen.getByRole("button", { name: t("ocr.start") }));
    await waitFor(() => expect(runOcrJob).toHaveBeenCalled());
    expect(runOcrJob.mock.calls[0][0].autoRotate).toBe(true);
  });

  it("the result says how many pages were turned upright", async () => {
    runOcrJob.mockResolvedValue({ ...result(), applied: [0, 1], rotated: [1] });
    render(<OcrDialog />);
    act(() => openOcrDialog(CONTEXT));
    fireEvent.click(screen.getByRole("button", { name: t("ocr.start") }));
    await screen.findByText(t("ocr.rotatedCount", { count: 1 }));
  });

  it("on Windows the native engine is Windows OCR, and 자동 runs it", async () => {
    capabilities({
      engines: ["tesseract", "windows"], languages: ["kor", "eng"],
      engineLanguages: { tesseract: ["kor", "eng"], vision: [], windows: ["kor", "eng"] },
    });
    render(<OcrDialog />);
    act(() => openOcrDialog(CONTEXT));
    const select = await screen.findByLabelText(t("ocr.engine"));
    expect([...(select as HTMLSelectElement).options].map((o) => o.textContent)).toEqual([
      t("ocr.engine.auto"), t("ocr.engine.windows"), t("ocr.engine.tesseract"),
    ]);
    expect(screen.getByText(t("ocr.engine.autoHintWindows"))).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: t("ocr.start") }));
    await waitFor(() => expect(runOcrJob).toHaveBeenCalled());
    expect(runOcrJob.mock.calls[0][0].engine).toBe("windows");
  });
});

describe("O3 (verification round 1) — 자동 does not trade Korean for the native engine", () => {
  const WINDOWS_EN_ONLY: OcrCapabilities = {
    engines: ["tesseract", "windows"], languages: ["kor", "eng"],
    engineLanguages: { tesseract: ["kor", "eng"], vision: [], windows: ["eng"] },
  };

  it("Windows with only the en-US recogniser: 설정's 한국어 + English runs Tesseract kor+eng", async () => {
    capabilities(WINDOWS_EN_ONLY);
    render(<OcrDialog />);
    act(() => openOcrDialog(CONTEXT));
    await screen.findByLabelText(t("ocr.engine"));
    expect(chip("ocr.language.ko")).toHaveAttribute("aria-pressed", "true");
    expect(chip("ocr.language.en")).toHaveAttribute("aria-pressed", "true");
    expect(screen.getByText(t("ocr.engine.autoFallbackWindows"))).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: t("ocr.start") }));
    await waitFor(() => expect(runOcrJob).toHaveBeenCalled());
    expect(runOcrJob.mock.calls[0][0]).toMatchObject({ engine: "tesseract", langs: "kor+eng" });
  });

  it("English alone goes to Windows OCR, and the 한국어 chip stays on offer", async () => {
    capabilities(WINDOWS_EN_ONLY);
    render(<OcrDialog />);
    act(() => openOcrDialog(CONTEXT));
    await screen.findByLabelText(t("ocr.engine"));
    fireEvent.click(chip("ocr.language.ko")!);
    expect(chip("ocr.language.ko")).toHaveAttribute("aria-pressed", "false");
    expect(screen.getByText(t("ocr.engine.autoHintWindows"))).toBeInTheDocument();
    fireEvent.click(chip("ocr.language.ko")!);
    expect(chip("ocr.language.ko")).toHaveAttribute("aria-pressed", "true");
    fireEvent.click(chip("ocr.language.ko")!);
    fireEvent.click(screen.getByRole("button", { name: t("ocr.start") }));
    await waitFor(() => expect(runOcrJob).toHaveBeenCalled());
    expect(runOcrJob.mock.calls[0][0]).toMatchObject({ engine: "windows", langs: "eng" });
  });

  it("an explicit Windows OCR choice is honoured (and offers only what it reads)", async () => {
    capabilities(WINDOWS_EN_ONLY);
    render(<OcrDialog />);
    act(() => openOcrDialog(CONTEXT));
    fireEvent.change(await screen.findByLabelText(t("ocr.engine")), { target: { value: "vision" } });
    expect(chip("ocr.language.ko")).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: t("ocr.start") }));
    await waitFor(() => expect(runOcrJob).toHaveBeenCalled());
    expect(runOcrJob.mock.calls[0][0]).toMatchObject({ engine: "windows", langs: "eng" });
  });

  it("여러 파일 OCR keeps the 한국어 chip and says 자동 will use Tesseract", async () => {
    capabilities(WINDOWS_EN_ONLY);
    render(<DialogHost />);
    act(() => openDialog("batchOcr"));
    await screen.findByLabelText(t("ocr.engine"));
    expect(chip("ocr.language.ko")).toHaveAttribute("aria-pressed", "true");
    expect(screen.getByText(t("ocr.engine.autoFallbackWindows"))).toBeInTheDocument();
  });
});
