/**
 * F-26 — the print-only DOM prints the **document**, not the app chrome.
 *
 * Before Stage 2b, `plugin:webview|print` printed the webview's DOM: toolbar, sidebar and the
 * canvas element on one sheet. These tests pin the two properties that fixed it — one image
 * per requested page off the `/page` route, and `window.print()` only after every one of them
 * has settled.
 */
import { act, cleanup, render } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { PrintRoot } from "./PrintRoot";
import { PRINT_DPI, scaleKeyForDpi, usePrintStore } from "./printStore";

afterEach(() => {
  cleanup();
  usePrintStore.getState().clear();
  vi.restoreAllMocks();
});

function start(pages: number[]) {
  usePrintStore.getState().start({
    docId: "d1",
    generation: 3,
    pages,
    rotation: 0,
    scaleKey: scaleKeyForDpi(PRINT_DPI),
  });
}

describe("print-only DOM", () => {
  it("renders nothing until a job starts", () => {
    const { container } = render(<PrintRoot />);
    expect(container.querySelector("#seepdf-print-root")).toBeNull();
  });

  it("renders one page image per requested page", () => {
    const { container } = render(<PrintRoot />);
    act(() => start([0, 2, 5]));
    expect(container.querySelectorAll("#seepdf-print-root img.print-page")).toHaveLength(3);
  });

  it("prints 150 DPI page images: sk = round(150 / 72 × 100) = 208", () => {
    expect(scaleKeyForDpi(PRINT_DPI)).toBe(208);
  });

  it("calls window.print() only once every page image has settled", async () => {
    const print = vi.spyOn(window, "print").mockImplementation(() => undefined);
    vi.spyOn(window, "requestAnimationFrame").mockImplementation((cb) => {
      cb(0);
      return 0;
    });
    const { container } = render(<PrintRoot />);
    act(() => start([0, 1]));

    const images = [...container.querySelectorAll<HTMLImageElement>("img.print-page")];
    expect(images).toHaveLength(2);

    act(() => {
      images[0].dispatchEvent(new Event("load"));
    });
    expect(print).not.toHaveBeenCalled();

    act(() => {
      images[1].dispatchEvent(new Event("load"));
    });
    expect(print).toHaveBeenCalledTimes(1);

    // The images stay mounted while the panel is open: `window.print()` returns before
    // WKWebView has laid the sheet out, and unmounting them first printed a blank page.
    expect(usePrintStore.getState().job).not.toBeNull();
    act(() => {
      window.dispatchEvent(new Event("afterprint"));
    });
    expect(usePrintStore.getState().job).toBeNull();
  });

  it("a page image that 410s still settles, so one bad page cannot hang the job", () => {
    const print = vi.spyOn(window, "print").mockImplementation(() => undefined);
    vi.spyOn(window, "requestAnimationFrame").mockImplementation((cb) => {
      cb(0);
      return 0;
    });
    const { container } = render(<PrintRoot />);
    act(() => start([7]));
    const img = container.querySelector<HTMLImageElement>("img.print-page")!;
    act(() => {
      img.dispatchEvent(new Event("error"));
    });
    expect(print).toHaveBeenCalledTimes(1);
  });
});
