/**
 * 여러 파일 OCR (P1-7) end to end against the mock adapter: the dialog, the queue, the password
 * prompt, `open_document` → OCR → `save_document_as` → `close_document` per file, and 취소.
 *
 * The tesseract pool is a fake (as in `ocrJob.test.ts`): these tests are about orchestration.
 */
import { beforeEach, describe, expect, it, vi } from "vitest";
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import * as api from "../../ipc/api";
import { mock } from "../../ipc/mock";
import DialogHost from "../../dialogs/DialogHost";
import { openDialog, useDialogStore } from "../../dialogs/dialogState";
import { useJobStore } from "../../store/jobStore";
import { useToastStore } from "../../app/toastStore";
import { useBatchOcr, resetBatch, cancelBatch } from "./flow";
import { loadOcrCapabilities, resetOcrCapabilities } from "../engine";
import type { OcrPage } from "../../ipc/types";

/** `ocr_capabilities`: the suite runs as a machine without Apple Vision unless a test says so. */
function capabilities(vision: boolean) {
  resetOcrCapabilities();
  vi.spyOn(mock, "ocrCapabilities").mockResolvedValue({
    engines: vision ? ["tesseract", "vision"] : ["tesseract"], languages: ["kor", "eng"],
  });
}

vi.mock("../../ipc/api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../../ipc/api")>();
  return { ...actual, openFileDialog: vi.fn() };
});

/** Recognition is instant unless a test holds it for one file. */
let holdFor: RegExp | null = null;
let lastOpened = "";
const recognized: string[] = [];

vi.mock("../tesseractPool", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../tesseractPool")>();
  return {
    ...actual,
    TesseractPool: class {
      terminated = false;
      constructor(readonly options: Record<string, unknown>) {
        pools.push(this);
      }
      get maxWorkers() { return (this.options.workers as number) ?? 1; }
      async recognize(_image: unknown, req: { signal?: AbortSignal } = {}) {
        if (req.signal?.aborted) throw new actual.OcrCancelledError();
        const file = lastOpened;
        if (holdFor?.test(file)) {
          await new Promise((_, reject) =>
            req.signal?.addEventListener("abort", () => reject(new actual.OcrCancelledError()), { once: true }));
        }
        recognized.push(file);
        return tessPage();
      }
      terminate() { this.terminated = true; return Promise.resolve(); }
    },
  };
});

const pools: { terminated: boolean; options: Record<string, unknown> }[] = [];

function tessPage(text = "스캔 문서") {
  return {
    blocks: [{
      bbox: { x0: 0, y0: 0, x1: 2480, y1: 3508 },
      paragraphs: [{
        lines: [{
          text, confidence: 90,
          bbox: { x0: 100, y0: 100, x1: 400, y1: 160 },
          rowAttributes: { ascenders: 12, descenders: -6, rowHeight: 60 },
          words: text.split(" ").map((w, i) => ({
            text: w, confidence: 90, bbox: { x0: 100 + i * 150, y0: 100, x1: 220 + i * 150, y1: 160 },
          })),
        }],
      }],
    }],
    text,
  };
}

const PLAIN = "/scans/계약서.pdf";
const LOCKED = "/scans/encrypted-급여.pdf";
const BROKEN = "/scans/damaged-영수증.pdf";

beforeEach(() => {
  useDialogStore.getState().closeAll();
  useToastStore.setState({ toasts: [] });
  useJobStore.setState({ jobs: [], active: null });
  resetBatch();
  holdFor = null;
  lastOpened = "";
  recognized.length = 0;
  pools.length = 0;
  vi.mocked(api.openFileDialog).mockImplementation(async (o) => (o?.directory ? ["/out"] : [PLAIN, LOCKED, BROKEN]));
  vi.stubGlobal("fetch", vi.fn(async () => ({
    ok: true,
    status: 200,
    headers: { get: (k: string) => (k === "X-Image-Width" ? "2480" : k === "X-Image-Height" ? "3508" : null) },
    arrayBuffer: async () => new Uint8Array([1, 2, 3, 4]).buffer,
  }) as unknown as Response));
  const open = mock.openDocument.bind(mock);
  vi.spyOn(mock, "openDocument").mockImplementation(async (a) => {
    lastOpened = a.path;
    return open(a);
  });
  // The mock's sample text is on every page; these files are scans.
  vi.spyOn(mock, "ocrPageStatus").mockImplementation(async (a) =>
    a.pages.map((page) => ({ page, hasText: false, charCount: 0 })));
  capabilities(false);
});

async function openBatchDialog(paths?: string[]) {
  if (paths) vi.mocked(api.openFileDialog).mockImplementation(async (o) => (o?.directory ? ["/out"] : paths));
  render(<DialogHost />);
  act(() => openDialog("batchOcr"));
  const start = await screen.findByRole("button", { name: "시작" });
  expect(start).toBeDisabled();
  fireEvent.click(screen.getByRole("button", { name: "파일 추가…" }));
  await waitFor(() => expect(screen.getAllByTestId("bocr-row").length).toBeGreaterThan(0));
  return start;
}

function row(name: string): HTMLElement {
  const found = screen.getAllByTestId("bocr-row").find((r) => r.textContent?.includes(name));
  if (!found) throw new Error(`no row for ${name}`);
  return found;
}

describe("batchOcr.flow", () => {
  it("3 files: one done, the encrypted one skipped when the password is dismissed, the damaged one failed", async () => {
    const save = vi.spyOn(mock, "saveDocumentAs");
    const close = vi.spyOn(mock, "closeDocument");
    const start = await openBatchDialog();
    expect(screen.getByText("파일 3개")).toBeInTheDocument();
    expect(start).toBeEnabled();
    fireEvent.click(start);

    // the encrypted file asks for its password over the dialog; dismissing it skips the file
    const cancelPrompt = await screen.findByText("이 문서는 암호로 보호되어 있습니다", {}, { timeout: 4000 });
    expect(cancelPrompt).toBeInTheDocument();
    expect(screen.getByText("encrypted-급여.pdf")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "취소" }));

    const summary = await screen.findByTestId("bocr-summary", {}, { timeout: 6000 });
    expect(summary.textContent).toBe("완료 1 · 건너뜀 1 · 실패 1");
    expect(row("계약서.pdf").dataset.status).toBe("done");
    expect(row("계약서.pdf").textContent).toContain("완료 · 계약서-ocr.pdf");
    expect(row("encrypted-급여.pdf").textContent).toContain("건너뜀 · 암호를 입력하지 않았습니다");
    expect(row("damaged-영수증.pdf").textContent).toContain("실패 · 파일을 열 수 없습니다");

    // the copy went beside the source, never over it; the file was closed afterwards
    expect(save).toHaveBeenCalledTimes(1);
    expect(save.mock.calls[0][0].path).toBe("/scans/계약서-ocr.pdf");
    const docId = save.mock.calls[0][0].docId;
    expect(close).toHaveBeenCalledWith({ docId });
    await expect(mock.getDocument({ docId })).rejects.toMatchObject({ code: "notFound" });
    // pages of one file ran on the one shared pool, terminated once at the end
    expect(recognized).toEqual([PLAIN, PLAIN, PLAIN]);
    expect(pools).toHaveLength(1);
    expect(pools[0].terminated).toBe(true);
    // the status bar job counted files and finished
    const job = useJobStore.getState().jobs.find((j) => j.kind === "batchOcr");
    expect(job).toMatchObject({ state: "done", done: 3, total: 3, labelKey: "batchOcr.running" });
    // …and is the only job: the per-file OCR runs are untracked (no status-bar entry each)
    expect(useJobStore.getState().jobs.map((j) => j.kind)).toEqual(["batchOcr"]);

    // 'Finder에서 보기' shows the output
    const reveal = vi.spyOn(mock, "revealInFileManager");
    fireEvent.click(screen.getByRole("button", { name: "Finder에서 보기" }));
    await waitFor(() => expect(reveal).toHaveBeenCalledWith({ path: "/scans/계약서-ocr.pdf" }));
  });

  it("an entered password opens the file; a taken name becomes (2); a chosen folder is used", async () => {
    vi.spyOn(mock, "pathExists").mockImplementation(async (a) => a.path === "/out/encrypted-급여-ocr.pdf");
    const save = vi.spyOn(mock, "saveDocumentAs");
    const start = await openBatchDialog([LOCKED]);
    fireEvent.click(screen.getByRole("radio", { name: "다른 폴더" }));
    await screen.findByText("/out");
    fireEvent.click(start);

    const input = await screen.findByLabelText("암호를 입력하세요", {}, { timeout: 4000 });
    fireEvent.change(input, { target: { value: "1234" } });
    fireEvent.click(screen.getByRole("button", { name: "확인" }));

    await screen.findByTestId("bocr-summary", {}, { timeout: 6000 });
    expect(save.mock.calls.map((c) => c[0].path)).toEqual(["/out/encrypted-급여-ocr (2).pdf"]);
    expect(row("encrypted-급여.pdf").dataset.status).toBe("done");
  });

  it("a file whose every page already has text is skipped and gets no copy", async () => {
    vi.mocked(mock.ocrPageStatus).mockImplementation(async (a) =>
      a.pages.map((page) => ({ page, hasText: true, charCount: 400 })));
    const save = vi.spyOn(mock, "saveDocumentAs");
    const start = await openBatchDialog([PLAIN]);
    fireEvent.click(start);
    await screen.findByTestId("bocr-summary", {}, { timeout: 4000 });
    expect(row("계약서.pdf").textContent).toContain("건너뜀 · 인식할 스캔 페이지가 없습니다");
    expect(save).not.toHaveBeenCalled();
  });

  it("취소 mid-queue stops the current file, closes it, and leaves the rest waiting", async () => {
    const second = "/scans/second-스캔.pdf";
    const third = "/scans/third-스캔.pdf";
    holdFor = /second/;
    const close = vi.spyOn(mock, "closeDocument");
    const save = vi.spyOn(mock, "saveDocumentAs");
    const start = await openBatchDialog([PLAIN, second, third]);
    fireEvent.click(start);

    await waitFor(() => expect(row("second-스캔.pdf").textContent).toContain("진행 중 0/3 페이지"), { timeout: 4000 });
    expect(row("계약서.pdf").dataset.status).toBe("done");
    const openedSecond = vi.mocked(mock.openDocument).mock.results.at(-1)?.value as Promise<{ docId: string }>;
    const { docId } = await openedSecond;

    const run = within(screen.getByRole("dialog"));
    fireEvent.click(run.getByRole("button", { name: "취소" }));

    await screen.findByTestId("bocr-summary", {}, { timeout: 4000 });
    expect(row("second-스캔.pdf").textContent).toContain("취소됨");
    expect(row("third-스캔.pdf").textContent).toContain("대기");
    expect(close).toHaveBeenCalledWith({ docId });
    await expect(mock.getDocument({ docId })).rejects.toMatchObject({ code: "notFound" });
    expect(save).toHaveBeenCalledTimes(1);
    expect(vi.mocked(mock.openDocument).mock.calls.map((c) => c[0].path)).not.toContain(third);
    expect(useJobStore.getState().jobs.find((j) => j.kind === "batchOcr")?.state).toBe("cancelled");

    // 시작 again resumes with what is left, and writes no second copy of the first file
    holdFor = null;
    fireEvent.click(screen.getByRole("button", { name: "시작" }));
    await waitFor(() => expect(row("third-스캔.pdf").dataset.status).toBe("done"), { timeout: 6000 });
    expect(save.mock.calls.map((c) => c[0].path)).toEqual([
      "/scans/계약서-ocr.pdf", "/scans/second-스캔-ocr.pdf", "/scans/third-스캔-ocr.pdf",
    ]);
  });

  it("the status bar's × cancels the batch, and a closed dialog reports with a toast", async () => {
    holdFor = /계약서/;
    const close = vi.spyOn(mock, "closeDocument");
    const start = await openBatchDialog([PLAIN]);
    fireEvent.click(start);
    await waitFor(() => expect(row("계약서.pdf").dataset.status).toBe("running"), { timeout: 4000 });

    // 닫기 hides the dialog; the batch keeps running
    fireEvent.click(screen.getByRole("button", { name: "닫기" }));
    expect(useDialogStore.getState().stack).toHaveLength(0);
    expect(useBatchOcr.getState().phase).toBe("running");

    const job = useJobStore.getState().active;
    expect(job?.kind).toBe("batchOcr");
    await act(async () => {
      await useJobStore.getState().cancel(job!.id);
    });
    await waitFor(() => expect(useBatchOcr.getState().phase).toBe("finished"));
    expect(close).toHaveBeenCalledTimes(1);
    expect(useToastStore.getState().toasts.map((t) => t.messageKey)).toEqual(["batchOcr.cancelled"]);
  });

  it("취소 while the password prompt is up takes the prompt down", async () => {
    const start = await openBatchDialog([LOCKED, PLAIN]);
    fireEvent.click(start);
    await screen.findByText("이 문서는 암호로 보호되어 있습니다", {}, { timeout: 4000 });
    await act(async () => {
      await cancelBatch();
    });
    expect(useDialogStore.getState().stack.map((e) => e.name)).toEqual(["batchOcr"]);
    expect(useBatchOcr.getState().items.map((i) => i.status)).toEqual(["cancelled", "queued"]);
  });
});

describe("batchOcr.engine", () => {
  it("자동 on a Mac with Vision: every page through ocr_recognize_native, ONE ocr_apply per file, no pool", async () => {
    capabilities(true);
    const native = vi.spyOn(mock, "ocrRecognizeNative");
    const apply = vi.spyOn(mock, "ocrApply");
    const save = vi.spyOn(mock, "saveDocumentAs");
    const start = await openBatchDialog([PLAIN, "/scans/second-스캔.pdf"]);
    const select = await screen.findByLabelText("인식 엔진");
    expect(select).toHaveValue("auto");
    fireEvent.click(start);

    const summary = await screen.findByTestId("bocr-summary", {}, { timeout: 6000 });
    expect(summary.textContent).toBe("완료 2 · 건너뜀 0 · 실패 0");
    expect(native.mock.calls.map((c) => c[0].page)).toEqual([0, 1, 2, 0, 1, 2]);
    expect(native.mock.calls[0][0]).toMatchObject({ dpi: 300, languages: ["ko-KR", "en-US"] });
    // One undo snapshot per file, not per page: each file's pages in a single ocr_apply.
    expect(apply).toHaveBeenCalledTimes(2);
    for (const call of apply.mock.calls) {
      expect((call[0].pages as OcrPage[]).map((p) => p.page)).toEqual([0, 1, 2]);
    }
    expect(pools).toHaveLength(0);
    expect(recognized).toEqual([]);
    expect(save).toHaveBeenCalledTimes(2);
  });

  it("Tesseract chosen on a Mac with Vision: the shared pool, still one ocr_apply per file", async () => {
    capabilities(true);
    const native = vi.spyOn(mock, "ocrRecognizeNative");
    const apply = vi.spyOn(mock, "ocrApply");
    const start = await openBatchDialog([PLAIN]);
    fireEvent.change(await screen.findByLabelText("인식 엔진"), { target: { value: "tesseract" } });
    fireEvent.click(start);
    await screen.findByTestId("bocr-summary", {}, { timeout: 6000 });
    expect(native).not.toHaveBeenCalled();
    expect(recognized).toEqual([PLAIN, PLAIN, PLAIN]);
    expect(pools).toHaveLength(1);
    expect(apply).toHaveBeenCalledTimes(1);
    expect((apply.mock.calls[0][0].pages as OcrPage[]).map((p) => p.page)).toEqual([0, 1, 2]);
  });

  it("no 인식 엔진 row without Vision", async () => {
    await openBatchDialog([PLAIN]);
    await act(async () => { await loadOcrCapabilities(); });
    expect(screen.queryByLabelText("인식 엔진")).not.toBeInTheDocument();
  });
});
