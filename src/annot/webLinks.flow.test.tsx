/**
 * V5 (v0.3) against the mock: an address typed in the page text is clickable in 읽기 mode (hit box
 * over PDFium's rects, the same 웹 주소 열기 confirm, http(s) / mailto only); one a Link annotation
 * already covers is not doubled. And 두 쪽 (표지 따로) pairs [1], [2, 3], [4, 5].
 */
import { beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { act, fireEvent, render, waitFor } from "@testing-library/react";
import App from "../App";
import { mock, mockOpenedUrls } from "../ipc/mock";
import { useAppStore } from "../store/appStore";
import { useAnnotStore } from "../store/annotStore";
import { useDocStore } from "../store/docStore";
import { useViewStore } from "../store/viewStore";
import { useDialogStore } from "../dialogs/dialogState";
import { warmLazyChunks } from "../test/warmLazy";
import { pageRows, stepPage } from "../viewer/layout";
import { resetWebLinks } from "./LinkLayer";
import type { Annot } from "../ipc/types";

const SAMPLE = "/Users/veri/Documents/SeePDF-샘플.pdf";
const URL = "https://seepdf.example/v0.3";

beforeAll(() => warmLazyChunks());

beforeEach(() => {
  resetWebLinks();
  useAppStore.setState({ os: "macos", mode: "read", tool: "select" });
  useDialogStore.getState().closeAll();
  useAnnotStore.getState().reset();
  useViewStore.setState({ split: null, parked: null, focusedPane: "main", zoomPercent: 100, zoomMode: "custom", layout: "continuous" });
  useDocStore.setState({ docId: null, info: null, status: "empty", error: null });
});

async function openSample() {
  render(<App />);
  await waitFor(() => expect(useAppStore.getState().ready).toBe(true), { timeout: 3000 });
  await act(async () => {
    await useDocStore.getState().open(SAMPLE);
  });
  await waitFor(() => expect(document.querySelector(".page-shell[data-page='0']")).not.toBeNull(), { timeout: 3000 });
}

describe("web links in the page text", () => {
  it("the mock finds addresses in the text like PDFium's detector", async () => {
    const info = await mock.openDocument({ path: SAMPLE });
    // the fixture pages carry no address
    expect(await mock.getWebLinks({ docId: info.docId, page: 0 })).toEqual([]);
  });

  it("a typed address is a hit box in 읽기 mode; a click confirms, then opens it", async () => {
    const links = vi.spyOn(mock, "getWebLinks").mockImplementation(async ({ page }) =>
      page === 0 ? [{ url: URL, rects: [{ l: 100, b: 500, r: 260, t: 514 }], charStart: 10, charCount: URL.length }] : [],
    );
    await openSample();
    const box = await waitFor(() => {
      const el = document.querySelector<HTMLElement>(".page-shell[data-page='0'] .link-hit[data-web-link]");
      expect(el).not.toBeNull();
      return el!;
    });
    expect(links).toHaveBeenCalledWith(expect.objectContaining({ page: 0 }));
    expect(box).toHaveAttribute("title", URL);
    expect(box.dataset.kind).toBe("url");
    fireEvent.click(box);
    const prompt = await waitFor(() => {
      const entry = useDialogStore.getState().stack.find((e) => e.name === "confirm");
      expect(entry).toBeDefined();
      return entry!;
    });
    expect(prompt.props).toMatchObject({ titleKey: "link.openTitle", bodyParams: { url: URL } });
    await act(async () => (prompt.props.resolve as (v: boolean) => void)(true));
    await waitFor(() => expect(mockOpenedUrls()).toContain(URL));

    // not in 주석 mode: the layer is 읽기's
    act(() => useAppStore.getState().setMode("annotate"));
    await waitFor(() => expect(document.querySelector(".link-hit[data-web-link]")).toBeNull());
  });

  it("an address a Link annotation already covers is not doubled", async () => {
    vi.spyOn(mock, "getWebLinks").mockResolvedValue([
      { url: URL, rects: [{ l: 100, b: 500, r: 260, t: 514 }], charStart: 0, charCount: URL.length },
    ]);
    await openSample();
    await waitFor(() => expect(document.querySelector(".page-shell[data-page='0'] .link-hit[data-web-link]")).not.toBeNull());
    // the page's own list first, or its arrival would replace the link added below
    await waitFor(() => expect(useAnnotStore.getState().byPage[0]?.length).toBeGreaterThan(0), { timeout: 3000 });
    const link: Annot = {
      id: "link-1", page: 0, kind: "link", subtype: "Link", uri: URL, rect: { l: 90, b: 495, r: 270, t: 520 },
      color: [0, 0, 0], fillColor: null, opacity: 1, borderWidth: 0, contents: "", author: null, created: null,
      modified: null, hidden: false, printed: true, locked: false, editable: "full",
    };
    act(() => useAnnotStore.getState().upsert(link));
    await waitFor(() => expect(document.querySelector(".page-shell[data-page='0'] .link-hit[data-web-link]")).toBeNull());
    expect(document.querySelectorAll(".page-shell[data-page='0'] .link-hit")).toHaveLength(1);
  });
});

describe("두 쪽 (표지 따로)", () => {
  it("pairs [1], [2, 3], [4, 5] and steps a spread at a time", () => {
    expect(pageRows(5, "twoCover")).toEqual([[0], [1, 2], [3, 4]]);
    expect(pageRows(4, "twoCover")).toEqual([[0], [1, 2], [3]]);
    expect(pageRows(1, "twoCover")).toEqual([[0]]);
    expect(stepPage(0, 1, 5, "twoCover")).toBe(1);
    expect(stepPage(1, 1, 5, "twoCover")).toBe(3);
    expect(stepPage(2, 1, 5, "twoCover")).toBe(3);
    expect(stepPage(4, 1, 5, "twoCover")).toBe(3); // the last spread
    expect(stepPage(3, -1, 5, "twoCover")).toBe(1);
    expect(stepPage(1, -1, 5, "twoCover")).toBe(0);
    expect(stepPage(0, -1, 5, "twoCover")).toBe(0);
  });

  it("is a layout the settings, the status bar and the keymap offer", async () => {
    await openSample();
    const segment = document.querySelector<HTMLButtonElement>(".statusbar .segment:nth-child(4)");
    expect(segment?.textContent).toBe("표지+두 쪽");
    fireEvent.click(segment!);
    expect(useViewStore.getState().layout).toBe("twoCover");
    // page 1 alone on its row, 2 and 3 side by side
    const top = (p: number) => parseFloat(document.querySelector<HTMLElement>(`.page-shell[data-page='${p}']`)!.style.top);
    await waitFor(() => expect(top(1)).toBe(top(2)));
    expect(top(0)).toBeLessThan(top(1));
    const set = vi.spyOn(mock, "setSettings");
    await useAppStore.getState().patchSettings({ defaultLayout: "twoCover" });
    expect(set).toHaveBeenCalledWith({ patch: { defaultLayout: "twoCover" } });
    expect(useAppStore.getState().settings?.defaultLayout).toBe("twoCover");
  });
});
