/**
 * v0.3 O6 — a scanned (image-only) document says so: the banner above the viewer with 'OCR 실행…',
 * dismissal remembered per path for the session, and the 검색 panel's 0-hit state with the same hint.
 * Against the mock adapter; `ocr_page_status` is the only thing faked.
 */
import { beforeEach, describe, expect, it, vi } from "vitest";
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { t } from "../i18n";
import { mock } from "../ipc/mock";
import { useDocStore } from "../store/docStore";
import { useViewStore } from "../store/viewStore";
import { useSearchStore } from "../viewer/search/SearchController";
import { SearchPanel } from "../sidebar/SearchPanel";
import { NeedsOcrBanner } from "./NeedsOcrBanner";
import { closeOcrDialog, useOcrDialogStore } from "./dialogState";
import { SAMPLE_PAGES, probeDocument, resetNeedsOcr, useNeedsOcr } from "./needsOcr";

const SCAN = "/Users/veri/Documents/스캔-공문.pdf";
const TEXT = "/Users/veri/Documents/SeePDF-샘플.pdf";

/** `ocr_page_status` for a scan (no page has text) or a born-digital file. */
function pagesHaveText(hasText: boolean) {
  return vi.spyOn(mock, "ocrPageStatus").mockImplementation(async (a) =>
    a.pages.map((page) => ({ page, hasText, charCount: hasText ? 900 : 0 })));
}

beforeEach(() => {
  vi.restoreAllMocks();
  resetNeedsOcr();
  closeOcrDialog();
  useViewStore.setState({ currentPage: 0, rotation: 0, scrollRequest: null });
  useSearchStore.setState({ query: "", hits: [], byPage: new Map(), total: 0, current: -1, running: false });
});

describe("ocr.needsOcr.banner", () => {
  it("an all-image document shows the banner, and 'OCR 실행…' opens the OCR sheet", async () => {
    const status = pagesHaveText(false);
    await useDocStore.getState().open(SCAN);
    render(<NeedsOcrBanner />);

    const banner = await screen.findByTestId("needs-ocr-banner", {}, { timeout: 4000 });
    expect(banner).toHaveTextContent(t("ocr.needsOcr.banner"));
    // Sampled once, off the critical path, and never more than the first SAMPLE_PAGES pages.
    const pages = status.mock.calls[0][0].pages;
    expect(pages[0]).toBe(0);
    expect(pages.length).toBeLessThanOrEqual(SAMPLE_PAGES);

    expect(useOcrDialogStore.getState().open).toBe(false);
    fireEvent.click(screen.getByRole("button", { name: t("ocr.needsOcr.run") }));
    expect(useOcrDialogStore.getState().open).toBe(true);
  });

  it("닫기 hides it for that document for the rest of the session", async () => {
    pagesHaveText(false);
    const info = await useDocStore.getState().open(SCAN);
    const { unmount } = render(<NeedsOcrBanner />);
    await screen.findByTestId("needs-ocr-banner", {}, { timeout: 4000 });
    fireEvent.click(screen.getByRole("button", { name: t("common.close") }));
    expect(screen.queryByTestId("needs-ocr-banner")).not.toBeInTheDocument();
    expect(useNeedsOcr.getState().dismissed).toEqual([info!.path]);

    // The same file again (reopened, the viewer remounted): still dismissed.
    unmount();
    await useDocStore.getState().open(SCAN);
    render(<NeedsOcrBanner />);
    await waitFor(() => expect(useNeedsOcr.getState().imageOnly).toBe(true), { timeout: 4000 });
    expect(screen.queryByTestId("needs-ocr-banner")).not.toBeInTheDocument();
  });

  it("a document with text shows nothing", async () => {
    const status = pagesHaveText(true);
    await useDocStore.getState().open(TEXT);
    render(<NeedsOcrBanner />);
    await waitFor(() => expect(status).toHaveBeenCalled(), { timeout: 4000 });
    await act(async () => { await Promise.resolve(); });
    expect(useNeedsOcr.getState().imageOnly).toBe(false);
    expect(screen.queryByTestId("needs-ocr-banner")).not.toBeInTheDocument();
  });

  it("goes away on its own once OCR has added a text layer", async () => {
    const status = pagesHaveText(false);
    const info = await useDocStore.getState().open(SCAN);
    render(<NeedsOcrBanner />);
    await screen.findByTestId("needs-ocr-banner", {}, { timeout: 4000 });

    status.mockImplementation(async (a) => a.pages.map((page) => ({ page, hasText: true, charCount: 400 })));
    act(() => useDocStore.getState().applyDocChanged({
      docId: info!.docId, docGeneration: info!.docGeneration + 1, changedPages: [0], structure: false,
      dirty: true, reason: "ocr", canUndo: true, canRedo: false,
    }));
    await waitFor(() => expect(screen.queryByTestId("needs-ocr-banner")).not.toBeInTheDocument());
  });
});

describe("ocr.needsOcr.search", () => {
  async function searchNothing() {
    render(<SearchPanel />);
    fireEvent.change(screen.getByRole("searchbox"), { target: { value: "존재하지않는검색어" } });
    await screen.findByText(t("sidebar.search.empty"), {}, { timeout: 4000 });
  }

  it("a 0-hit search on an image-only document shows the OCR hint", async () => {
    pagesHaveText(false);
    const info = await useDocStore.getState().open(SCAN);
    await probeDocument(info!.docId, info!.pageCount);
    await searchNothing();
    const hint = screen.getByTestId("needs-ocr-search");
    expect(hint).toHaveTextContent(t("ocr.needsOcr.searchHint"));
    fireEvent.click(screen.getByRole("button", { name: t("ocr.needsOcr.run") }));
    expect(useOcrDialogStore.getState().open).toBe(true);
  });

  it("a 0-hit search on a text document is just 'no results'", async () => {
    pagesHaveText(true);
    const info = await useDocStore.getState().open(TEXT);
    await probeDocument(info!.docId, info!.pageCount);
    await searchNothing();
    expect(screen.queryByTestId("needs-ocr-search")).not.toBeInTheDocument();
  });
});
