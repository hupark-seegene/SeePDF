import { beforeEach, describe, expect, it } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { Thumbnails } from "./Thumbnails";
import { Outline } from "./Outline";
import { SearchPanel } from "./SearchPanel";
import { useDocStore } from "../store/docStore";
import { useViewStore } from "../store/viewStore";
import { useSearchStore } from "../viewer/search/SearchController";

const SAMPLE = "/Users/veri/Documents/SeePDF-샘플.pdf";

async function openSample() {
  const info = await useDocStore.getState().open(SAMPLE);
  expect(info).not.toBeNull();
  return info!;
}

beforeEach(() => {
  useViewStore.setState({ currentPage: 0, rotation: 0, scrollRequest: null });
  useSearchStore.setState({ query: "", hits: [], byPage: new Map(), total: 0, current: -1, running: false });
});

describe("sidebar", () => {
  it("thumbnails: one row per page, the current one ringed, click navigates", async () => {
    const info = await openSample();
    render(<Thumbnails />);

    const options = await screen.findAllByRole("option");
    expect(options).toHaveLength(info.pageCount);
    expect(options[0]).toHaveAttribute("aria-selected", "true");
    expect(options[0].querySelector("img")?.getAttribute("src")).toMatch(/^data:image\/png/);

    fireEvent.click(screen.getByRole("option", { name: "2쪽 축소판" }));
    expect(useViewStore.getState().currentPage).toBe(1);
    expect(useViewStore.getState().scrollRequest?.page).toBe(1);

    await waitFor(() =>
      expect(screen.getByRole("option", { name: "2쪽 축소판" })).toHaveAttribute("aria-selected", "true"),
    );
  });

  it("outline: renders the tree, collapses a branch and navigates on click", async () => {
    await openSample();
    render(<Outline />);

    expect(await screen.findByText("1. 시작하기")).toBeInTheDocument();
    expect(screen.getByText("1.1 문서 열기")).toBeInTheDocument();
    expect(screen.getByText("3. 양식과 OCR")).toBeInTheDocument();
    const before = screen.getAllByRole("treeitem").length;
    expect(before).toBeGreaterThan(1);

    // the first node with children has a twisty; collapsing hides its subtree
    const twisty = document.querySelector("button.outline-twisty") as HTMLButtonElement;
    expect(twisty).toBeTruthy();
    fireEvent.click(twisty);
    expect(screen.getAllByRole("treeitem").length).toBeLessThan(before);
    fireEvent.click(twisty);
    expect(screen.getAllByRole("treeitem").length).toBe(before);

    const target = screen
      .getAllByRole("button")
      .find((b) => b.classList.contains("outline-item") && !b.hasAttribute("disabled"))!;
    fireEvent.click(target);
    expect(useViewStore.getState().scrollRequest).not.toBeNull();
  });

  it("outline: empty documents show the empty state", () => {
    useDocStore.setState({ outline: [] });
    render(<Outline />);
    expect(screen.getByText("이 문서에는 목차가 없습니다")).toBeInTheDocument();
  });

  it("search: streams results, shows the count and navigates hits", async () => {
    const info = await openSample();
    render(<SearchPanel />);

    const field = screen.getByRole("searchbox");
    expect(field).toHaveFocus();
    fireEvent.change(field, { target: { value: "PDF" } });

    await waitFor(
      () => {
        const s = useSearchStore.getState();
        expect(s.running).toBe(false);
        expect(s.scanned).toBe(info.pageCount);
        expect(s.total).toBeGreaterThan(0);
      },
      { timeout: 3000 },
    );

    const total = useSearchStore.getState().total;
    expect(screen.getByText(`결과 ${total}개`)).toBeInTheDocument();
    expect(useSearchStore.getState().current).toBe(0);

    const results = screen.getAllByRole("button", { name: /PDF/ });
    expect(results.length).toBeGreaterThan(0);
    fireEvent.click(results[results.length - 1]);
    expect(useSearchStore.getState().current).toBe(total - 1);

    // Enter steps to the next hit (wrapping), Shift+Enter steps back
    fireEvent.keyDown(field, { key: "Enter" });
    expect(useSearchStore.getState().current).toBe(0);
    fireEvent.keyDown(field, { key: "Enter", shiftKey: true });
    expect(useSearchStore.getState().current).toBe(total - 1);

    // Esc clears
    fireEvent.keyDown(field, { key: "Escape" });
    await waitFor(() => expect(useSearchStore.getState().hits).toEqual([]));
  });

  it("search: toggling 대소문자 구분 re-runs the query", async () => {
    const info = await openSample();
    render(<SearchPanel />);
    fireEvent.change(screen.getByRole("searchbox"), { target: { value: "pdf" } });
    await waitFor(
      () => {
        const s = useSearchStore.getState();
        expect(s.running).toBe(false);
        expect(s.scanned).toBe(info.pageCount);
        expect(s.total).toBeGreaterThan(0);
      },
      { timeout: 3000 },
    );
    const insensitive = useSearchStore.getState().total;

    fireEvent.click(screen.getByLabelText("대소문자 구분"));
    await waitFor(
      () => {
        const s = useSearchStore.getState();
        expect(s.matchCase).toBe(true);
        expect(s.running).toBe(false);
        expect(s.scanned).toBe(info.pageCount);
        expect(s.total).toBeLessThan(insensitive);
      },
      { timeout: 3000 },
    );
  });
});
