/**
 * 여러 파일에서 검색 (P2) end to end against the mock: the dialog, 파일 추가 / 폴더 추가, one job
 * over three files (one encrypted — its password prompt dismissed, so it is skipped), every file
 * opened beside the window's document and closed again, results grouped by file with counts, a
 * click that opens the file at the hit with the 검색 panel's highlights, and 취소 mid-way closing
 * the file being read.
 */
import { beforeEach, describe, expect, it, vi } from "vitest";
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import * as api from "../ipc/api";
import { mock } from "../ipc/mock";
import App from "../App";
import { openDialog, useDialogStore } from "../dialogs/dialogState";
import { useAppStore } from "../store/appStore";
import { useDocStore } from "../store/docStore";
import { useJobStore } from "../store/jobStore";
import { useToastStore } from "../app/toastStore";
import { useSearchStore } from "../viewer/search/SearchController";
import { resetMultiSearch, useMultiSearch } from "./flow";
import type { DocId } from "../ipc/types";

vi.mock("../ipc/api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../ipc/api")>();
  return { ...actual, openFileDialog: vi.fn() };
});

const REPORT = "/docs/보고서.pdf";
const LOCKED = "/docs/encrypted-급여.pdf";
const MINUTES = "/docs/회의록.pdf";

/** path → the docId `open_document` gave it, and every docId opened */
let openedAs = new Map<string, DocId>();

beforeEach(() => {
  useDialogStore.getState().closeAll();
  useJobStore.setState({ jobs: [], active: null });
  useToastStore.setState({ toasts: [] });
  useDocStore.setState({ docId: null, info: null, status: "empty", error: null });
  useSearchStore.getState().clear();
  useAppStore.setState({ os: "macos", mode: "read", tool: "select" });
  resetMultiSearch();
  openedAs = new Map();
  vi.mocked(api.openFileDialog).mockImplementation(async (o) => (o?.directory ? ["/archive"] : [REPORT, LOCKED, MINUTES]));
  const open = mock.openDocument.bind(mock);
  vi.spyOn(mock, "openDocument").mockImplementation(async (a) => {
    const info = await open(a);
    openedAs.set(a.path, info.docId);
    return info;
  });
});

async function openSearchDialog(query = "OCR") {
  render(<App />);
  await waitFor(() => expect(useAppStore.getState().ready).toBe(true), { timeout: 3000 });
  act(() => openDialog("multiSearch"));
  const start = await screen.findByRole("button", { name: "검색" }, { timeout: 5000 });
  expect(start).toBeDisabled();
  fireEvent.change(screen.getByLabelText("검색어"), { target: { value: query } });
  fireEvent.click(screen.getByRole("button", { name: "파일 추가…" }));
  await waitFor(() => expect(screen.getAllByTestId("msearch-file")).toHaveLength(3), { timeout: 3000 });
  expect(start).toBeEnabled();
  return start;
}

function fileRow(name: string): HTMLElement {
  const found = screen.getAllByTestId("msearch-file").find((r) => r.textContent?.includes(name));
  if (!found) throw new Error(`no row for ${name}`);
  return found;
}

describe("multiSearch.flow", () => {
  it("3 files: the encrypted one skipped, hits grouped by file, every file closed; a hit opens its file there", async () => {
    const close = vi.spyOn(mock, "closeDocument");
    const start = await openSearchDialog();
    fireEvent.click(start);

    // the encrypted file asks for its password over the dialog; dismissing it skips the file
    expect(await screen.findByText("이 문서는 암호로 보호되어 있습니다", {}, { timeout: 4000 })).toBeInTheDocument();
    fireEvent.click(within(screen.getByRole("dialog")).getByRole("button", { name: "취소" }));

    const summary = await screen.findByTestId("msearch-summary", {}, { timeout: 6000 });
    const files = useMultiSearch.getState().files;
    const report = files.find((f) => f.path === REPORT)!;
    const minutes = files.find((f) => f.path === MINUTES)!;
    expect(report.status).toBe("done");
    expect(minutes.status).toBe("done");
    expect(report.hits.length).toBeGreaterThan(0);
    // pages in order, each hit with its ±40-character context
    expect(report.hits.map((h) => h.page)).toEqual([...report.hits.map((h) => h.page)].sort((a, b) => a - b));
    expect(report.hits.every((h) => h.context.includes("OCR") && h.contextMatch[1] === 3)).toBe(true);
    const total = report.hits.length + minutes.hits.length;
    expect(summary.textContent).toBe(`결과 ${total}개 · 파일 2개 · 건너뜀 1 · 실패 0`);
    expect(fileRow("보고서.pdf").textContent).toContain(`결과 ${report.hits.length}개`);
    expect(fileRow("encrypted-급여.pdf").textContent).toContain("건너뜀 · 암호를 입력하지 않았습니다");
    expect(within(fileRow("회의록.pdf")).getAllByTestId("msearch-hit")).toHaveLength(minutes.hits.length);

    // both searched files were opened beside the window's (empty) document and closed again
    for (const path of [REPORT, MINUTES]) {
      const docId = openedAs.get(path)!;
      expect(close).toHaveBeenCalledWith({ docId });
      await expect(mock.getDocument({ docId })).rejects.toMatchObject({ code: "notFound" });
    }
    expect(useDocStore.getState().info).toBeNull();
    // one job in the status bar, counted in files
    expect(useJobStore.getState().jobs.find((j) => j.kind === "multiSearch")).toMatchObject({
      state: "done", done: 3, total: 3, labelKey: "multiSearch.running",
    });

    // a hit of 회의록 opens 회의록 through the normal open path, as the 검색 panel's current hit
    const hits = within(fileRow("회의록.pdf")).getAllByTestId("msearch-hit");
    const target = minutes.hits.length - 1;
    fireEvent.click(hits[target]);
    await waitFor(() => expect(useDocStore.getState().info?.path).toBe(MINUTES), { timeout: 3000 });
    await waitFor(() => expect(useDialogStore.getState().stack.some((e) => e.name === "multiSearch")).toBe(false), { timeout: 3000 });
    const search = useSearchStore.getState();
    expect(search.docId).toBe(useDocStore.getState().info!.docId);
    expect(search.query).toBe("OCR");
    expect(search.hits).toHaveLength(minutes.hits.length);
    expect(search.hits[search.current]).toMatchObject({
      page: minutes.hits[target].page, charStart: minutes.hits[target].charStart,
    });
    // the page highlights are the search panel's own marks, the clicked one current
    await waitFor(() => expect(document.querySelectorAll(".mark.hit").length).toBeGreaterThan(0), { timeout: 3000 });
  });

  it("취소 mid-way stops the run and closes the file being read", async () => {
    const close = vi.spyOn(mock, "closeDocument");
    const cancelJob = vi.spyOn(mock, "cancelJob");
    const search = mock.searchStart.bind(mock);
    // 회의록 never finishes on its own
    vi.spyOn(mock, "searchStart").mockImplementation(async (a, onEvent) => {
      if (openedAs.get(MINUTES) === a.docId) {
        onEvent({ type: "page", page: 0, hits: [], scanned: 1, total: 0 });
        return 4242;
      }
      return search(a, onEvent);
    });
    // no password prompt this time: the list is 보고서 then 회의록
    vi.mocked(api.openFileDialog).mockImplementation(async () => [REPORT, MINUTES, "/docs/예산안.pdf"]);
    const start = await openSearchDialog();
    fireEvent.click(start);

    await waitFor(() => expect(fileRow("회의록.pdf").dataset.status).toBe("searching"), { timeout: 4000 });
    const reading = openedAs.get(MINUTES)!;
    expect(fileRow("보고서.pdf").dataset.status).toBe("done");
    fireEvent.click(within(screen.getByRole("dialog")).getByRole("button", { name: "취소" }));

    await screen.findByTestId("msearch-summary", {}, { timeout: 4000 });
    expect(close).toHaveBeenCalledWith({ docId: reading });
    await expect(mock.getDocument({ docId: reading })).rejects.toMatchObject({ code: "notFound" });
    expect(cancelJob).toHaveBeenCalledWith({ jobId: 4242 });
    expect(fileRow("회의록.pdf").dataset.status).toBe("cancelled");
    expect(fileRow("예산안.pdf").dataset.status).toBe("queued");
    expect(openedAs.has("/docs/예산안.pdf")).toBe(false);
    expect(useJobStore.getState().jobs.find((j) => j.kind === "multiSearch")?.state).toBe("cancelled");
  });

  it("폴더 추가 lists every PDF below the folder, once", async () => {
    render(<App />);
    await waitFor(() => expect(useAppStore.getState().ready).toBe(true), { timeout: 3000 });
    act(() => openDialog("multiSearch"));
    fireEvent.click(await screen.findByRole("button", { name: "폴더 추가…" }, { timeout: 5000 }));
    await waitFor(() => expect(screen.getAllByTestId("msearch-file")).toHaveLength(3), { timeout: 3000 });
    fireEvent.click(screen.getByRole("button", { name: "폴더 추가…" }));
    await new Promise((r) => setTimeout(r, 30));
    expect(useMultiSearch.getState().files.map((f) => f.path)).toEqual([
      "/archive/2023/예산안.pdf", "/archive/보고서-2024.pdf", "/archive/회의록.pdf",
    ]);
  });
});
