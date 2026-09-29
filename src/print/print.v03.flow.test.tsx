/**
 * v0.3 pkg8 — printing: 주석 (X7), fit / chunks (X8), 모아찍기 · 소책자 · 크기 · 흑백 (X2).
 *
 * The page URLs are read back through a test asset resolver that echoes the query, so the
 * `print=` flag the engine sees is what is asserted — not a component prop.
 */
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { PrintRoot } from "./PrintRoot";
import { PRINT_CHUNK, PRINT_DPI, scaleKeyForDpi, usePrintStore } from "./printStore";
import { setMockAssetResolver } from "../ipc/protocol";
import { mock, mockAssetUrl } from "../ipc/mock";
import { useDocStore } from "../store/docStore";
import { PrintDialog } from "../dialogs/PrintDialog";

const SAMPLE = "/Users/veri/Documents/SeePDF-샘플.pdf"; // 3 pages

beforeEach(() => {
  setMockAssetResolver((route, q) => {
    const qs = Object.entries(q)
      .filter(([, v]) => v !== undefined)
      .map(([k, v]) => `${k}=${String(v)}`)
      .join("&");
    return `mock:${route}?${qs}`;
  });
  vi.spyOn(window, "requestAnimationFrame").mockImplementation((cb) => {
    cb(0);
    return 0;
  });
});

afterEach(() => {
  setMockAssetResolver(mockAssetUrl);
  usePrintStore.getState().clear();
  vi.restoreAllMocks();
});

function images(container: HTMLElement) {
  return [...container.querySelectorAll<HTMLImageElement>("#seepdf-print-root img.print-page")];
}

function settleAll(container: HTMLElement) {
  for (const img of images(container)) act(() => void img.dispatchEvent(new Event("load")));
}

async function openSample() {
  await useDocStore.getState().open(SAMPLE);
  await waitFor(() => expect(useDocStore.getState().info).not.toBeNull());
}

describe("print v0.3", () => {
  it("mounts at most one chunk of page images and prints the chunks one after another", () => {
    const print = vi.spyOn(window, "print").mockImplementation(() => undefined);
    const { container } = render(<PrintRoot />);
    const total = PRINT_CHUNK * 2 + 7;
    act(() =>
      usePrintStore.getState().start({
        docId: "d1", generation: 1, pages: Array.from({ length: total }, (_, i) => i),
        rotation: 0, scaleKey: scaleKeyForDpi(PRINT_DPI),
      }),
    );
    // Chunk 1: pages 0..49, a progress line on screen.
    expect(images(container)).toHaveLength(PRINT_CHUNK);
    expect(screen.getByRole("status").textContent).toContain(`/ ${total}쪽`);
    settleAll(container);
    expect(print).toHaveBeenCalledTimes(1);
    act(() => void window.dispatchEvent(new Event("afterprint")));

    // The panel closed: nothing mounted until the user asks for the next chunk.
    expect(images(container)).toHaveLength(0);
    expect(print).toHaveBeenCalledTimes(1);
    fireEvent.click(screen.getByRole("button", { name: "다음 묶음 인쇄 (2/3)" }));

    // Chunk 2: the next 50, never more than one chunk mounted.
    expect(images(container)).toHaveLength(PRINT_CHUNK);
    expect(images(container)[0].src).toContain(`page=${PRINT_CHUNK}&`);
    settleAll(container);
    expect(print).toHaveBeenCalledTimes(2);
    act(() => void window.dispatchEvent(new Event("afterprint")));
    fireEvent.click(screen.getByRole("button", { name: "다음 묶음 인쇄 (3/3)" }));

    // Chunk 3: the remaining 7, then the job is over.
    expect(images(container)).toHaveLength(7);
    settleAll(container);
    expect(print).toHaveBeenCalledTimes(3);
    act(() => void window.dispatchEvent(new Event("afterprint")));
    expect(usePrintStore.getState().job).toBeNull();
  });

  it("a cancelled print panel does not open the next chunk's panel; 중지 ends the job", async () => {
    const print = vi.spyOn(window, "print").mockImplementation(() => undefined);
    const close = vi.spyOn(mock, "closeDocument");
    const { container } = render(<PrintRoot />);
    act(() =>
      usePrintStore.getState().start({
        docId: "d1", generation: 1, pages: Array.from({ length: PRINT_CHUNK * 10 }, (_, i) => i),
        rotation: 0, scaleKey: 208, tempDocId: "d1",
      }),
    );
    settleAll(container);
    expect(print).toHaveBeenCalledTimes(1);
    // The user presses 취소 in the print panel: WebKit fires afterprint all the same.
    act(() => void window.dispatchEvent(new Event("afterprint")));
    expect(print).toHaveBeenCalledTimes(1);
    expect(usePrintStore.getState().chunk).toBe(0);
    expect(usePrintStore.getState().waiting).toBe(true);
    expect(images(container)).toHaveLength(0);
    fireEvent.click(screen.getByRole("button", { name: "중지" }));
    expect(usePrintStore.getState().job).toBeNull();
    expect(print).toHaveBeenCalledTimes(1);
    // The temporary n-up document is closed with the job.
    await waitFor(() => expect(close).toHaveBeenCalledWith({ docId: "d1" }));
  });

  it("취소 on the progress line stops a chunked job while its pages are still loading", () => {
    const print = vi.spyOn(window, "print").mockImplementation(() => undefined);
    const { container } = render(<PrintRoot />);
    act(() =>
      usePrintStore.getState().start({
        docId: "d1", generation: 1, pages: Array.from({ length: PRINT_CHUNK * 10 }, (_, i) => i),
        rotation: 0, scaleKey: 208,
      }),
    );
    expect(images(container)).toHaveLength(PRINT_CHUNK);
    fireEvent.click(screen.getByRole("button", { name: "취소" }));
    expect(usePrintStore.getState().job).toBeNull();
    expect(images(container)).toHaveLength(0);
    expect(print).not.toHaveBeenCalled();
  });

  it("marks landscape pages so the sheet turns them, and keeps fit / grayscale on the root", () => {
    vi.spyOn(window, "print").mockImplementation(() => undefined);
    const { container } = render(<PrintRoot />);
    act(() =>
      usePrintStore.getState().start({
        docId: "d1", generation: 1, pages: [0, 1], rotation: 0, scaleKey: 208,
        sizes: [[612, 792], [842, 595]], grayscale: true,
      }),
    );
    const sheets = [...container.querySelectorAll<HTMLElement>(".print-sheet")];
    expect(sheets.map((s) => s.dataset.orient)).toEqual(["portrait", "landscape"]);
    expect(sheets.every((s) => s.dataset.fit === "fit")).toBe(true);
    expect(container.querySelector("#seepdf-print-root")?.getAttribute("data-gray")).toBe("1");
  });

  it("실제 크기 sizes each page in inches", () => {
    vi.spyOn(window, "print").mockImplementation(() => undefined);
    const { container } = render(<PrintRoot />);
    act(() =>
      usePrintStore.getState().start({
        docId: "d1", generation: 1, pages: [0], rotation: 0, scaleKey: 208,
        sizes: [[612, 792]], fit: "actual",
      }),
    );
    const img = images(container)[0];
    expect(img.style.width).toBe("8.5in");
    expect(img.style.height).toBe("11in");
  });

  it("문서만 puts print=none on every print image URL", async () => {
    vi.spyOn(window, "print").mockImplementation(() => undefined);
    await openSample();
    const { container } = render(
      <>
        <PrintDialog onClose={() => {}} />
        <PrintRoot />
      </>,
    );
    fireEvent.change(screen.getByLabelText("주석"), { target: { value: "none" } });
    fireEvent.click(screen.getByRole("button", { name: "인쇄" }));
    await waitFor(() => expect(images(container)).toHaveLength(3));
    for (const img of images(container)) expect(img.src).toContain("print=none");
  });

  it("the default prints every page with print=all (annotation Print flags honoured)", async () => {
    vi.spyOn(window, "print").mockImplementation(() => undefined);
    await openSample();
    const { container } = render(
      <>
        <PrintDialog onClose={() => {}} />
        <PrintRoot />
      </>,
    );
    fireEvent.click(screen.getByRole("button", { name: "인쇄" }));
    await waitFor(() => expect(images(container)).toHaveLength(3));
    for (const img of images(container)) expect(img.src).toContain("print=all");
  });

  it("모아찍기 and 흑백 reach the print root through an n-up document", async () => {
    vi.spyOn(window, "print").mockImplementation(() => undefined);
    const makeNup = vi.spyOn(mock, "makeNup");
    await openSample();
    const source = useDocStore.getState().info!.docId;
    const { container } = render(
      <>
        <PrintDialog onClose={() => {}} />
        <PrintRoot />
      </>,
    );
    fireEvent.change(screen.getByLabelText("모아찍기"), { target: { value: "2" } });
    fireEvent.change(screen.getByLabelText("순서"), { target: { value: "down" } });
    fireEvent.change(screen.getByLabelText("주석"), { target: { value: "stamps" } });
    fireEvent.click(screen.getByRole("checkbox", { name: "흑백" }));
    fireEvent.click(screen.getByRole("button", { name: "인쇄" }));

    await waitFor(() => expect(usePrintStore.getState().job).not.toBeNull());
    expect(makeNup).toHaveBeenCalledWith(
      expect.objectContaining({
        docId: source,
        options: expect.objectContaining({ perSheet: 2, order: "down", booklet: false, annots: "stamps" }),
      }),
    );
    const job = usePrintStore.getState().job!;
    expect(job.docId).not.toBe(source);
    expect(job.tempDocId).toBe(job.docId);
    expect(job.pages).toEqual([0, 1]); // 3 pages at 2-up
    expect(job.grayscale).toBe(true);
    await waitFor(() => expect(images(container)).toHaveLength(2));
    expect(container.querySelector("#seepdf-print-root")?.getAttribute("data-gray")).toBe("1");
    // The window's document is unchanged.
    expect(useDocStore.getState().info!.docId).toBe(source);
  });

  it("소책자 asks the engine for a booklet and the handler path gets the n-up file", async () => {
    const makeNup = vi.spyOn(mock, "makeNup");
    const prepare = vi.spyOn(mock, "printPrepare");
    await openSample();
    render(<PrintDialog onClose={() => {}} />);
    fireEvent.change(screen.getByLabelText("인쇄 방법"), { target: { value: "handler" } });
    fireEvent.click(screen.getByRole("checkbox", { name: "소책자 (중철)" }));
    // 크기 and 흑백 are print-panel options: not offered for the PDF app.
    expect(screen.queryByLabelText("크기")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "인쇄" }));
    await waitFor(() => expect(makeNup).toHaveBeenCalled());
    expect(makeNup.mock.calls[0][0].options).toMatchObject({ booklet: true });
    expect(prepare).not.toHaveBeenCalled();
  });

  it("the handler path passes the 주석 option to print_prepare", async () => {
    const prepare = vi.spyOn(mock, "printPrepare");
    await openSample();
    render(<PrintDialog onClose={() => {}} />);
    fireEvent.change(screen.getByLabelText("인쇄 방법"), { target: { value: "handler" } });
    fireEvent.change(screen.getByLabelText("주석"), { target: { value: "none" } });
    fireEvent.click(screen.getByRole("button", { name: "인쇄" }));
    await waitFor(() => expect(prepare).toHaveBeenCalled());
    expect(prepare.mock.calls[0][0]).toMatchObject({ annots: "none" });
  });
});
