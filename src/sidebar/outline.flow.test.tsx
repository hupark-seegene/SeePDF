/**
 * 목차 편집 (P2) against the mock adapter: 편집 → add from the current page, rename, outdent,
 * delete, drag → 완료 is ONE `set_outline` whose tree the store reads back; 실행 취소 restores the
 * original; 취소 writes nothing; an encrypted document cannot enter the editor.
 */
import { beforeEach, describe, expect, it, vi } from "vitest";
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { mock } from "../ipc/mock";
import { useDocStore } from "../store/docStore";
import { useViewStore } from "../store/viewStore";
import { useToastStore } from "../app/toastStore";
import { Outline } from "./Outline";

const SAMPLE = "/Users/veri/Documents/SeePDF-샘플.pdf";

beforeEach(() => {
  useToastStore.setState({ toasts: [] });
});

async function openEditor() {
  await useDocStore.getState().open(SAMPLE);
  render(<Outline />);
  fireEvent.click(await screen.findByRole("button", { name: "편집" }));
  const tree = await screen.findByRole("tree", { name: "편집" });
  return tree;
}

function row(tree: HTMLElement, title: string): HTMLElement {
  const item = within(tree)
    .getAllByRole("treeitem")
    .find((li) => li.querySelector(".outline-edit-title")?.textContent === title);
  if (!item) throw new Error(`no row ${title}`);
  return item;
}

/** jsdom has no layout: give every row a 24 px band, top to bottom, like the real tree. */
function layOutRows(tree: HTMLElement): void {
  within(tree)
    .getAllByRole("treeitem")
    .forEach((li, i) => {
      li.getBoundingClientRect = () =>
        ({ top: i * 24, bottom: i * 24 + 24, height: 24, left: 0, right: 200, width: 200, x: 0, y: i * 24 }) as DOMRect;
    });
}

function titlesOf(tree: HTMLElement): string[] {
  return within(tree)
    .getAllByRole("treeitem")
    .map((li) => li.querySelector(".outline-edit-title")?.textContent ?? li.querySelector("input")?.value ?? "");
}

describe("목차 편집", () => {
  it("add / rename / outdent / delete / drag, then 완료 writes the whole tree once and 실행 취소 restores it", async () => {
    const tree = await openEditor();
    const spy = vi.spyOn(mock, "setOutline");
    expect(titlesOf(tree)).toEqual(["1. 시작하기", "1.1 문서 열기", "1.2 화면 구성", "2. 주석 도구", "2.1 형광펜", "3. 양식과 OCR"]);

    // 현재 페이지 추가 after the selected row, straight into renaming
    act(() => useViewStore.setState({ currentPage: 2 }));
    fireEvent.click(row(tree, "3. 양식과 OCR").querySelector(".outline-edit-row")!);
    fireEvent.click(screen.getByRole("button", { name: "현재 페이지 추가" }));
    const input = await screen.findByRole("textbox", { name: "항목 이름" });
    expect(input).toHaveValue("새 항목");
    fireEvent.change(input, { target: { value: "4. 부록" } });
    fireEvent.keyDown(input, { key: "Enter" });
    expect(titlesOf(tree).at(-1)).toBe("4. 부록");

    // ⇧Tab: 1.2 leaves its parent and lands right after it
    fireEvent.click(row(tree, "1.2 화면 구성").querySelector(".outline-edit-row")!);
    fireEvent.keyDown(tree, { key: "Tab", shiftKey: true });
    // Delete removes 2. and its child
    fireEvent.click(row(tree, "2. 주석 도구").querySelector(".outline-edit-row")!);
    fireEvent.keyDown(tree, { key: "Delete" });
    expect(titlesOf(tree)).toEqual(["1. 시작하기", "1.1 문서 열기", "1.2 화면 구성", "3. 양식과 OCR", "4. 부록"]);

    // drag 4. 부록 onto the bottom quarter of 1. 시작하기 ("after") — v0.3 H7: pointer events, since
    // HTML5 drag and drop never fires in the Windows webview
    layOutRows(tree);
    const from = row(tree, "4. 부록").getBoundingClientRect();
    const onto = row(tree, "1. 시작하기").getBoundingClientRect();
    fireEvent.pointerDown(row(tree, "4. 부록"), { button: 0, clientX: 40, clientY: from.top + 12 });
    fireEvent.pointerMove(window, { clientX: 40, clientY: onto.top + 20 });
    fireEvent.pointerUp(window, { clientX: 40, clientY: onto.top + 20 });
    expect(titlesOf(tree)).toEqual(["1. 시작하기", "1.1 문서 열기", "4. 부록", "1.2 화면 구성", "3. 양식과 OCR"]);

    fireEvent.click(screen.getByRole("button", { name: "완료" }));
    await waitFor(() => expect(spy).toHaveBeenCalledTimes(1));
    const nodes = spy.mock.calls[0][0].nodes;
    expect(nodes.map((n) => n.title)).toEqual(["1. 시작하기", "4. 부록", "1.2 화면 구성", "3. 양식과 OCR"]);
    expect(nodes[0].children.map((n) => n.title)).toEqual(["1.1 문서 열기"]);
    expect(nodes[0].open).toBe(true);
    expect(nodes[1]).toEqual({ title: "4. 부록", page: 2, children: [] });

    // the editor closes and the sidebar shows what the engine read back
    await screen.findByRole("tree", { name: "목차" });
    await waitFor(() => expect(useDocStore.getState().outline.map((n) => n.title)).toEqual(nodes.map((n) => n.title)));
    expect(useDocStore.getState().info?.undoLabel).toBe("undo.outlineEdit");
    const saved = useToastStore.getState().toasts.find((t) => t.messageKey === "outline.saved");
    expect(saved?.actions?.[0].labelKey).toBe("common.undo");

    // 실행 취소 → the original outline again
    await act(async () => {
      await useDocStore.getState().undo();
    });
    expect(useDocStore.getState().outline.map((n) => n.title)).toEqual(["1. 시작하기", "2. 주석 도구", "3. 양식과 OCR"]);
    spy.mockRestore();
  });

  it("취소 writes nothing; 목적지를 현재 보기로 re-points the selection", async () => {
    const tree = await openEditor();
    const spy = vi.spyOn(mock, "setOutline");
    fireEvent.click(row(tree, "1.1 문서 열기").querySelector(".outline-edit-row")!);
    act(() => useViewStore.setState({ currentPage: 1 }));
    fireEvent.click(screen.getByRole("button", { name: "목적지를 현재 보기로" }));
    expect(row(tree, "1.1 문서 열기").querySelector(".outline-page")?.textContent).toBe("2");
    fireEvent.click(screen.getByRole("button", { name: "취소" }));
    await screen.findByRole("tree", { name: "목차" });
    expect(spy).not.toHaveBeenCalled();
    spy.mockRestore();
  });

  it("Enter renames, Esc restores; 완료 with no change closes without writing", async () => {
    const tree = await openEditor();
    const spy = vi.spyOn(mock, "setOutline");
    fireEvent.click(row(tree, "2.1 형광펜").querySelector(".outline-edit-row")!);
    fireEvent.keyDown(tree, { key: "Enter" });
    const input = await screen.findByRole("textbox", { name: "항목 이름" });
    fireEvent.change(input, { target: { value: "버릴 이름" } });
    fireEvent.keyDown(input, { key: "Escape" });
    expect(titlesOf(tree)).toContain("2.1 형광펜");
    fireEvent.click(screen.getByRole("button", { name: "완료" }));
    await screen.findByRole("tree", { name: "목차" });
    expect(spy).not.toHaveBeenCalled();
    spy.mockRestore();
  });

  it("an encrypted document cannot enter the editor, and the engine refuses one anyway", async () => {
    await useDocStore.getState().open(SAMPLE);
    const info = useDocStore.getState().info!;
    act(() => useDocStore.getState().adopt({ ...info, encrypted: true }));
    render(<Outline />);
    expect(screen.getByRole("button", { name: "편집" })).toBeDisabled();
    await expect(mock.setOutline({ docId: "nope", nodes: [] })).rejects.toMatchObject({ code: "notFound" });
  });

  it("closed nodes start collapsed and page numbers show the document's labels", async () => {
    await useDocStore.getState().open(SAMPLE);
    const info = useDocStore.getState().info!;
    await mock.setOutline({
      docId: info.docId,
      nodes: [
        { title: "앞부분", page: 0, open: false, children: [{ title: "숨은 항목", page: 1, children: [] }] },
        { title: "본문", page: 2, children: [] },
      ],
    });
    await mock.setPageLabels({ docId: info.docId, ranges: [{ start: 0, style: "roman" }, { start: 2, style: "decimal" }] });
    await act(async () => {
      await useDocStore.getState().refresh();
    });
    render(<Outline />);
    const tree = await screen.findByRole("tree", { name: "목차" });
    expect(within(tree).queryByText("숨은 항목")).toBeNull();
    expect(within(tree).getByText("앞부분").closest("button")?.textContent).toContain("i");
    expect(within(tree).getByText("본문").closest("button")?.querySelector(".outline-page")?.textContent).toBe("1");
  });
});
