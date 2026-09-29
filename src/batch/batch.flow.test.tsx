/**
 * 여러 파일 처리 (v0.3 pkg8, X1) end to end against the mock adapter: the dialog, the queue, the
 * password prompt, `open_document` → action → `save_document_as` → `close_document` per file in
 * order, and 취소 after the current file.
 */
import { beforeEach, describe, expect, it, vi } from "vitest";
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import * as api from "../ipc/api";
import { mock } from "../ipc/mock";
import DialogHost from "../dialogs/DialogHost";
import { openDialog, useDialogStore } from "../dialogs/dialogState";
import { useJobStore } from "../store/jobStore";
import { useToastStore } from "../app/toastStore";
import { addFiles, cancelBatch, outputCandidate, resetBatch, setOptions, startBatch, useBatch } from "./flow";

vi.mock("../ipc/api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../ipc/api")>();
  return { ...actual, openFileDialog: vi.fn() };
});

const A = "/docs/보고서.pdf";
const B = "/docs/회의록.pdf";
const C = "/docs/계획.pdf";
const LOCKED = "/docs/encrypted-급여.pdf";

/** Every api call the batch makes, in order, as `verb:file`. */
const calls: string[] = [];
const pathOf = new Map<string, string>();

beforeEach(() => {
  useDialogStore.getState().closeAll();
  useToastStore.setState({ toasts: [] });
  useJobStore.setState({ jobs: [], active: null });
  resetBatch();
  calls.length = 0;
  pathOf.clear();
  const name = (docId: string) => (pathOf.get(docId) ?? docId).split("/").pop();
  const open = mock.openDocument.bind(mock);
  vi.spyOn(mock, "openDocument").mockImplementation(async (a) => {
    const info = await open(a);
    pathOf.set(info.docId, a.path);
    calls.push(`open:${name(info.docId)}`);
    return info;
  });
  const estimate = mock.compressEstimate.bind(mock);
  vi.spyOn(mock, "compressEstimate").mockImplementation(async (a, on) => {
    calls.push(`compress:${name(a.docId)}`);
    return estimate(a, on);
  });
  const apply = mock.compressApply.bind(mock);
  vi.spyOn(mock, "compressApply").mockImplementation(async (a) => {
    calls.push(`apply:${name(a.docId)}`);
    return apply(a);
  });
  const save = mock.saveDocumentAs.bind(mock);
  vi.spyOn(mock, "saveDocumentAs").mockImplementation(async (a, on) => {
    calls.push(`save:${name(a.docId)}`);
    return save(a, on);
  });
  const close = mock.closeDocument.bind(mock);
  vi.spyOn(mock, "closeDocument").mockImplementation(async (a) => {
    calls.push(`close:${name(a.docId)}`);
    return close(a);
  });
});

async function openBatch(paths: string[]) {
  vi.mocked(api.openFileDialog).mockImplementation(async (o) => (o?.directory ? ["/out"] : paths));
  render(<DialogHost />);
  act(() => openDialog("batch"));
  const start = await screen.findByRole("button", { name: "시작" });
  fireEvent.click(screen.getByRole("button", { name: "파일 추가…" }));
  await screen.findByText(`파일 ${paths.length}개`);
  return start;
}

function row(name: string): HTMLElement {
  return within(screen.getByText(name).closest("tr")!).getByText(/./, { selector: ".bocr-status" });
}

describe("batch.flow", () => {
  it("names outputs <name>-<suffix>.pdf beside the source or in a folder", () => {
    expect(outputCandidate("/a/보고서.pdf", null, "-compressed")).toBe("/a/보고서-compressed.pdf");
    expect(outputCandidate("/a/보고서.PDF", "/out/", "-flat", 2)).toBe("/out/보고서-flat (2).PDF");
    expect(outputCandidate("C:\\a\\x.pdf", null, "-stamped")).toBe("C:\\a\\x-stamped.pdf");
  });

  it("a 3-file queue with compress calls open/compress/save/close per file, in order", async () => {
    const start = await openBatch([A, B, C]);
    // 압축 is the default action, 화면용 150 DPI the default preset.
    expect(start).toBeEnabled();
    fireEvent.click(start);
    const summary = await screen.findByTestId("batch-summary", {}, { timeout: 8000 });
    expect(summary.textContent).toBe("완료 3 · 건너뜀 0 · 실패 0");
    expect(calls).toEqual([
      "open:보고서.pdf", "compress:보고서.pdf", "apply:보고서.pdf", "save:보고서.pdf", "close:보고서.pdf",
      "open:회의록.pdf", "compress:회의록.pdf", "apply:회의록.pdf", "save:회의록.pdf", "close:회의록.pdf",
      "open:계획.pdf", "compress:계획.pdf", "apply:계획.pdf", "save:계획.pdf", "close:계획.pdf",
    ]);
    expect(vi.mocked(mock.saveDocumentAs).mock.calls.map((c) => c[0].path)).toEqual([
      "/docs/보고서-compressed.pdf", "/docs/회의록-compressed.pdf", "/docs/계획-compressed.pdf",
    ]);
    expect(row("보고서.pdf").textContent).toContain("완료 · 보고서-compressed.pdf");
  });

  it("취소 mid-way stops after the current file: nothing after it is opened", async () => {
    const start = await openBatch([A, B, C]);
    fireEvent.click(start);
    // Wait until the second file is being compressed, then cancel.
    await waitFor(() => expect(calls).toContain("compress:회의록.pdf"), { timeout: 6000 });
    await act(async () => {
      await cancelBatch();
    });
    expect(calls).not.toContain("open:계획.pdf");
    expect(calls).toContain("close:회의록.pdf");
    expect(useBatch.getState().items.map((i) => i.status)).toEqual(["done", "cancelled", "queued"]);
    expect(useJobStore.getState().jobs.find((j) => j.kind === "batch")?.state).toBe("cancelled");
  });

  it("a password-protected file is prompted for and can be skipped", async () => {
    const start = await openBatch([LOCKED, A]);
    fireEvent.click(start);
    await screen.findByText("이 문서는 암호로 보호되어 있습니다", {}, { timeout: 4000 });
    fireEvent.click(screen.getByRole("button", { name: "취소" }));
    const summary = await screen.findByTestId("batch-summary", {}, { timeout: 6000 });
    expect(summary.textContent).toBe("완료 1 · 건너뜀 1 · 실패 0");
    expect(row("encrypted-급여.pdf").textContent).toContain("건너뜀 · 암호를 입력하지 않았습니다");
    expect(calls.filter((c) => c.startsWith("open:"))).toEqual(["open:보고서.pdf"]);
  });

  it("a password entered at the prompt opens the file and the action runs", async () => {
    const start = await openBatch([LOCKED]);
    fireEvent.click(start);
    const input = await screen.findByLabelText("암호를 입력하세요", {}, { timeout: 4000 });
    fireEvent.change(input, { target: { value: "1234" } });
    fireEvent.click(screen.getByRole("button", { name: "확인" }));
    await screen.findByTestId("batch-summary", {}, { timeout: 6000 });
    expect(vi.mocked(mock.saveDocumentAs).mock.calls.map((c) => c[0].path)).toEqual(["/docs/encrypted-급여-compressed.pdf"]);
  });

  it("워터마크 · Bates: the 워터마크 form sets the spec, every file is stamped, Bates continues", async () => {
    const stamp = vi.spyOn(mock, "addStamp");
    const start = await openBatch([A, B]);
    fireEvent.change(screen.getByLabelText("작업"), { target: { value: "stamp" } });
    expect(start).toBeDisabled();
    fireEvent.click(screen.getByRole("button", { name: "설정…" }));
    // The 워터마크 dialog opens over the batch dialog, without the range / remove sections.
    fireEvent.click(await screen.findByRole("button", { name: "Bates 번호 매기기" }));
    expect(screen.queryByText("기존 항목 제거")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "적용" }));
    // Back on the batch dialog (a fresh mount), now startable.
    await screen.findByTestId("batch-stamp-summary");
    const again = screen.getByRole("button", { name: "시작" });
    expect(again).toBeEnabled();
    fireEvent.click(again);
    await screen.findByTestId("batch-summary", {}, { timeout: 8000 });
    expect(stamp).toHaveBeenCalledTimes(2);
    const [first, second] = stamp.mock.calls.map((c) => c[0].spec);
    expect(first.pages).toBe("all");
    // the sample has 3 pages: the second file starts where the first ended
    expect(second.batesStart).toBe((first.batesStart ?? 1) + 3);
    expect(vi.mocked(mock.saveDocumentAs).mock.calls.map((c) => c[0].path)).toEqual([
      "/docs/보고서-stamped.pdf", "/docs/회의록-stamped.pdf",
    ]);
  });

  it("암호 설정 and 평면화 write their own output, with no save", async () => {
    const setPassword = vi.spyOn(mock, "setPassword");
    const flatten = vi.spyOn(mock, "exportFlattened");
    let start = await openBatch([A]);
    fireEvent.change(screen.getByLabelText("작업"), { target: { value: "password" } });
    expect(start).toBeDisabled();
    fireEvent.change(screen.getByLabelText("열기 암호"), { target: { value: "s3cret" } });
    fireEvent.click(start);
    await screen.findByTestId("batch-summary", {}, { timeout: 6000 });
    expect(setPassword.mock.calls[0][0]).toMatchObject({ outPath: "/docs/보고서-protected.pdf", userPassword: "s3cret" });

    fireEvent.change(screen.getByLabelText("작업"), { target: { value: "flatten" } });
    start = screen.getByRole("button", { name: "시작" });
    fireEvent.click(screen.getByRole("button", { name: "목록 비우기" }));
    fireEvent.click(screen.getByRole("button", { name: "파일 추가…" }));
    await screen.findByText("파일 1개");
    fireEvent.click(start);
    await screen.findByTestId("batch-summary", {}, { timeout: 6000 });
    expect(flatten.mock.calls[0][0]).toMatchObject({ outPath: "/docs/보고서-flat.pdf", annotations: true, forms: true });
    expect(mock.saveDocumentAs).not.toHaveBeenCalled();
  });

  it("이미지로 내보내기 exports every page into the chosen folder", async () => {
    const images = vi.spyOn(mock, "exportImages");
    const start = await openBatch([A]);
    fireEvent.change(screen.getByLabelText("작업"), { target: { value: "images" } });
    fireEvent.click(screen.getByRole("radio", { name: "다른 폴더" }));
    await screen.findByText("/out");
    fireEvent.click(start);
    await screen.findByTestId("batch-summary", {}, { timeout: 6000 });
    expect(images.mock.calls[0][0]).toMatchObject({ outDir: "/out", baseName: "보고서", format: "png", pages: [] });
  });

  // v0.3 integration (X1 × Windows paths): `C:` alone is the drive's current directory, not its root.
  it.each([
    ["C:\\scan.pdf", "C:\\", "C:\\scan-001.png"],
    ["D:\\x.pdf", "D:\\", "D:\\x-001.png"],
    ["/x.pdf", "/", "/x-001.png"],
  ])("이미지로 내보내기 beside %s writes into the drive root, not the current directory", async (file, dir, first) => {
    const images = vi.spyOn(mock, "exportImages");
    const probe = vi.spyOn(mock, "pathExists");
    addFiles([file]);
    setOptions({ action: "images" });
    await startBatch();
    expect(probe.mock.calls[0][0]).toEqual({ path: first });
    expect(images.mock.calls[0][0]).toMatchObject({ outDir: dir, baseName: first.slice(dir.length, -"-001.png".length) });
  });
});
