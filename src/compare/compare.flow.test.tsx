import { beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import * as api from "../ipc/api";
import DialogHost from "../dialogs/DialogHost";
import { openDialog, useDialogStore } from "../dialogs/dialogState";
import { useDocStore } from "../store/docStore";
import { useToastStore } from "../app/toastStore";
import { mock } from "../ipc/mock";
import CompareView from "./CompareView";
import { useCompareStore } from "./state";
import { useCompareRun } from "./flow";

// The mock's native picker always answers with the sample; compare needs a *different* file.
vi.mock("../ipc/api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../ipc/api")>();
  return { ...actual, openFileDialog: vi.fn() };
});

const SAMPLE = "/Users/veri/Documents/SeePDF-샘플.pdf"; // 3 pages
const OTHER = "/Users/veri/Downloads/160F-2019.pdf"; // 2 pages (recents fixture)

function CompareHost() {
  const on = useCompareStore((s) => s.session !== null);
  return on ? <CompareView /> : null;
}

beforeEach(async () => {
  useDialogStore.getState().closeAll();
  useToastStore.setState({ toasts: [] });
  useCompareStore.setState({ session: null });
  useCompareRun.setState({ path: null, ignoreCase: false, phase: "idle", done: 0, total: 0 });
  vi.mocked(api.openFileDialog).mockImplementation(async () => [OTHER]);
  await useDocStore.getState().open(SAMPLE);
});

async function pickAndCompare() {
  render(
    <>
      <CompareHost />
      <DialogHost />
    </>,
  );
  act(() => openDialog("compare"));
  const run = await screen.findByRole("button", { name: "비교" });
  expect(run).toBeDisabled();
  fireEvent.click(screen.getByRole("button", { name: "찾아보기…" }));
  await screen.findByText("160F-2019.pdf");
  expect(run).toBeEnabled();
  return run;
}

describe("compare.flow", () => {
  it("비교 opens B beside A, runs the job, and shows the aligned rows with their marks", async () => {
    const compare = vi.spyOn(mock, "compareDocuments");
    const close = vi.spyOn(mock, "closeDocument");
    const run = await pickAndCompare();
    fireEvent.click(screen.getByRole("checkbox", { name: "대소문자 무시" }));
    fireEvent.click(run);

    const summary = await screen.findByTestId("compare-summary", {}, { timeout: 2000 });
    expect(compare).toHaveBeenCalledWith(
      { docA: "d1", docB: "d2", options: { ignoreCase: true, alignPages: true } },
      expect.any(Function),
    );
    expect(useDialogStore.getState().stack).toHaveLength(0);
    // A has 3 pages, B 2: pair 1 is revised, pair 2 has no B page
    expect(summary.textContent).toContain("변경된 페이지 2");
    expect(summary.textContent).toMatch(/삽입 \d+단어/);
    expect(summary.textContent).toMatch(/삭제 \d+단어/);
    const rows = document.querySelectorAll("[data-row]");
    expect(rows).toHaveLength(3);
    expect(document.querySelectorAll("[data-row][data-changed]")).toHaveLength(2);
    // the aligned third row: a page only A has
    expect(rows[2].textContent).toContain("삭제된 페이지");
    expect(rows[2]).toHaveAttribute("data-side", "deleted");
    expect(screen.getAllByTestId("cmp-mark-del").length).toBeGreaterThan(0);
    expect(screen.getAllByTestId("cmp-mark-ins").length).toBeGreaterThan(0);
    // the window's document is untouched
    expect(useDocStore.getState().info?.docId).toBe("d1");

    // 다음 변경 → the first changed row; 이전 변경 has nothing before it
    fireEvent.click(screen.getByRole("button", { name: "다음 변경" }));
    expect(document.querySelector("[data-row='1']")).toHaveAttribute("data-current");
    expect(screen.getByRole("button", { name: "이전 변경" })).toBeDisabled();
    fireEvent.click(screen.getByRole("button", { name: "다음 변경" }));
    expect(document.querySelector("[data-row='2']")).toHaveAttribute("data-current");
    expect(screen.getByRole("button", { name: "다음 변경" })).toBeDisabled();

    // 변경만 보기 hides the unchanged row
    fireEvent.click(screen.getByRole("checkbox", { name: "변경만 보기" }));
    expect(document.querySelectorAll("[data-row]")).toHaveLength(2);

    // 닫기 leaves compare mode and closes B only
    fireEvent.click(screen.getByRole("button", { name: "닫기" }));
    await waitFor(() => expect(close).toHaveBeenCalledWith({ docId: "d2" }));
    expect(close).not.toHaveBeenCalledWith({ docId: "d1" });
    expect(useCompareStore.getState().session).toBeNull();
    expect(screen.queryByTestId("compare-summary")).toBeNull();
    expect(useDocStore.getState().info?.docId).toBe("d1");
  });

  it("Esc closes compare mode", async () => {
    const close = vi.spyOn(mock, "closeDocument");
    fireEvent.click(await pickAndCompare());
    await screen.findByTestId("compare-summary", {}, { timeout: 2000 });
    fireEvent.keyDown(window, { key: "Escape" });
    await waitFor(() => expect(close).toHaveBeenCalledWith({ docId: "d2" }));
    expect(useCompareStore.getState().session).toBeNull();
  });

  it("취소 stops the job, closes B and keeps the dialog", async () => {
    const cancel = vi.spyOn(mock, "cancelJob");
    const close = vi.spyOn(mock, "closeDocument");
    fireEvent.click(await pickAndCompare());
    await screen.findByText(/비교 중/);
    const [jobCancel] = screen.getAllByRole("button", { name: "취소" });
    fireEvent.click(jobCancel);
    await waitFor(() => expect(cancel).toHaveBeenCalled());
    await waitFor(() => expect(close).toHaveBeenCalledWith({ docId: "d2" }));
    expect(useDialogStore.getState().stack.map((e) => e.name)).toEqual(["compare"]);
    expect(screen.getByRole("button", { name: "비교" })).toBeEnabled();
    expect(useCompareStore.getState().session).toBeNull();
  });

  it("a document closed mid-job ends quietly: `cancelled` (or an error coded cancelled) toasts nothing", async () => {
    for (const end of ["cancelled", "errorCancelled"] as const) {
      vi.spyOn(mock, "compareDocuments").mockImplementationOnce(async (_a, onEvent) => {
        setTimeout(() => {
          onEvent({ type: "started", jobId: 77, total: 3 });
          onEvent(
            end === "cancelled"
              ? { type: "cancelled", jobId: 77, done: 1 }
              : { type: "error", jobId: 77, error: { code: "cancelled", message: "document closed" } },
          );
        }, 5);
        return 77;
      });
      const close = vi.spyOn(mock, "closeDocument");
      fireEvent.click(await pickAndCompare());
      await waitFor(() => expect(close).toHaveBeenCalledWith({ docId: expect.any(String) }));
      await waitFor(() => expect(useCompareRun.getState().phase).toBe("idle"));
      expect(useToastStore.getState().toasts.filter((t) => t.tone === "danger")).toHaveLength(0);
      expect(useCompareStore.getState().session).toBeNull();
      cleanup();
      useDialogStore.getState().closeAll();
      useCompareRun.setState({ path: null, ignoreCase: false, phase: "idle", done: 0, total: 0 });
      vi.restoreAllMocks();
      vi.mocked(api.openFileDialog).mockImplementation(async () => [OTHER]);
    }
  });

  it("an inserted and a deleted page show as their own rows, labelled 삽입된 페이지 / 삭제된 페이지", async () => {
    const infoA = useDocStore.getState().info!;
    const infoB = await api.openDocument({ path: OTHER });
    const same = { changed: false, wordsA: 3, wordsB: 3, ops: [{ kind: "equal" as const, words: 3 }] };
    act(() =>
      useCompareStore.getState().enter({
        infoA,
        infoB,
        report: {
          docA: infoA.docId, docB: infoB.docId, changedPages: 2, inserted: 4, deleted: 5, elapsedMs: 1,
          pages: [
            { pageA: 0, pageB: 0, ...same },
            { pageA: null, pageB: 1, changed: true, wordsA: 0, wordsB: 4, ops: [{ kind: "insert", words: 4, textB: "a b c d" }] },
            { pageA: 1, pageB: null, changed: true, wordsA: 5, wordsB: 0, ops: [{ kind: "delete", words: 5, textA: "a b c d e" }] },
            { pageA: 2, pageB: null, ...same, changed: true },
          ],
        },
      }),
    );
    render(<CompareHost />);
    const rows = document.querySelectorAll("[data-row]");
    expect(rows).toHaveLength(4);
    expect(rows[0]).not.toHaveAttribute("data-side");
    expect(rows[1]).toHaveAttribute("data-side", "inserted");
    expect(within(rows[1] as HTMLElement).getAllByText("삽입된 페이지")).toHaveLength(2); // badge + the empty A side
    expect(rows[1].textContent).toContain("A 없음쪽 · B 2쪽");
    expect(rows[2]).toHaveAttribute("data-side", "deleted");
    expect(within(rows[2] as HTMLElement).getAllByText("삭제된 페이지")).toHaveLength(2);
    // 다음 변경 walks the null-sided rows like any other change
    fireEvent.click(screen.getByRole("button", { name: "다음 변경" }));
    expect(rows[1]).toHaveAttribute("data-current");
  });

  it("an encrypted B goes through the password prompt and the dialog comes back with its state", async () => {
    vi.mocked(api.openFileDialog).mockImplementation(async () => ["/Users/veri/Documents/encrypted-v2.pdf"]);
    render(
      <>
        <CompareHost />
        <DialogHost />
      </>,
    );
    act(() => openDialog("compare"));
    fireEvent.click(await screen.findByRole("button", { name: "찾아보기…" }));
    await screen.findByText("encrypted-v2.pdf");
    fireEvent.click(screen.getByRole("button", { name: "비교" }));

    const field = await screen.findByLabelText("암호를 입력하세요");
    fireEvent.change(field, { target: { value: "secret" } });
    fireEvent.click(screen.getByRole("button", { name: "확인" }));
    await screen.findByTestId("compare-summary", {}, { timeout: 2000 });
    expect(useCompareStore.getState().session?.infoB.name).toBe("encrypted-v2.pdf");
  });
});
