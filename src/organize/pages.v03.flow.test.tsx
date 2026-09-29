/**
 * v0.3 P2 / P3 against the mock:
 *  * an OS file dropped on the page grid at the gap after page 3 is inserted at index 3;
 *  * 위치 이동… → 1 moves the selection to the front as one `move` op (one undo step);
 *  * the status bar shows 변경됨 · 페이지 14 → 13 after a delete;
 *  * a page drop published by another window lands at the caret under the point.
 */
import { beforeEach, describe, expect, it, vi } from "vitest";
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { mock } from "../ipc/mock";
import { useDocStore } from "../store/docStore";
import { usePagesStore } from "../store/pagesStore";
import { useDialogStore } from "../dialogs/dialogState";
import DialogHost from "../dialogs/DialogHost";
import { StatusBar } from "../app/StatusBar";
import { Organizer } from "./Organizer";
import { dropFilesOnOrganizer } from "./fileDrop";
import { handlePagesDrop } from "./crossWindow";
import { runPageOps } from "../dialogs/flows";

const SAMPLE = "/Users/veri/Downloads/tracemonkey.pdf"; // 14 pages in the recents fixture

beforeEach(async () => {
  useDialogStore.getState().closeAll();
  usePagesStore.getState().reset();
  await useDocStore.getState().open(SAMPLE);
});

/** The grid point of caret `caret` (jsdom: the grid is 800 px wide, the content at 0,0). */
function caretPoint(caret: number): { x: number; y: number } {
  const GAP = 12;
  const thumb = usePagesStore.getState().thumbSize;
  const cols = Math.max(1, Math.floor((800 - GAP) / (thumb + GAP)));
  const rowH = Math.round(thumb * 1.32) + 26 + GAP;
  return { x: GAP + (caret % cols) * (thumb + GAP), y: GAP + Math.floor(caret / cols) * rowH + 10 };
}

describe("페이지 mode drops (P2)", () => {
  it("a PDF dropped at the gap after page 3 is inserted at index 3", async () => {
    const ops = vi.spyOn(mock, "pageOps");
    render(<Organizer />);
    const handled = await dropFilesOnOrganizer(["/Users/veri/Documents/부록.pdf"], caretPoint(3));
    expect(handled).toBe(true);
    expect(ops).toHaveBeenCalledTimes(1);
    expect(ops.mock.calls[0][0].ops).toEqual([{ kind: "insertFrom", at: 3, path: "/Users/veri/Documents/부록.pdf" }]);
  });

  it("dropped images become pages at the caret through create_from_images + import", async () => {
    const create = vi.spyOn(mock, "createFromImages");
    const imported = vi.spyOn(mock, "importPagesFromDoc");
    const closed = vi.spyOn(mock, "closeDocument");
    render(<Organizer />);
    await dropFilesOnOrganizer(["/tmp/a.png", "/tmp/b.jpg"], caretPoint(5));
    expect(create).toHaveBeenCalledTimes(1);
    expect(imported).toHaveBeenCalledTimes(1);
    const args = imported.mock.calls[0][0];
    expect(args).toMatchObject({ pages: [0, 1], dstDocId: useDocStore.getState().info!.docId, at: 5 });
    expect(closed).toHaveBeenCalledWith({ docId: args.srcDocId });
    await waitFor(() => expect(useDocStore.getState().info?.pageCount).toBe(16));
  });

  it("without the organizer on screen a drop is not taken", async () => {
    expect(await dropFilesOnOrganizer(["/x.pdf"], { x: 1, y: 1 })).toBe(false);
  });
});

describe("위치 이동… (P2)", () => {
  it("moveTo 1 puts the selection at the front in one undo step", async () => {
    const ops = vi.spyOn(mock, "pageOps");
    act(() => usePagesStore.getState().setSelected([4, 6]));
    act(() => useDialogStore.getState().open("moveTo", { pages: [4, 6] }));
    render(<DialogHost />);
    const field = await screen.findByRole("spinbutton", { name: "지정한 페이지 위치로" });
    fireEvent.change(field, { target: { value: "1" } });
    fireEvent.click(screen.getByRole("button", { name: "이동" }));
    await waitFor(() => expect(ops).toHaveBeenCalledTimes(1));
    expect(ops.mock.calls[0][0].ops).toEqual([{ kind: "move", pages: [4, 6], to: 0 }]);
    await waitFor(() => expect(usePagesStore.getState().selected).toEqual([0, 1]));
    await waitFor(() => expect(useDocStore.getState().info?.undoLabel).toBe("undo.pageMove"));
  });
});

describe("변경됨 chip (P2)", () => {
  it("appears after a delete and names the page counts", async () => {
    render(<StatusBar />);
    expect(screen.queryByText(/변경됨/)).toBeNull();
    await act(async () => {
      await runPageOps([{ kind: "delete", pages: [0] }]);
    });
    expect(await screen.findByText("변경됨 · 페이지 14 → 13")).toBeInTheDocument();
  });
});

describe("pages between windows (P3)", () => {
  it("a drop published by another window imports at the caret under the point", async () => {
    const source = await mock.openDocument({ path: "/Users/veri/Documents/보고서-2024.pdf" });
    const target = useDocStore.getState().info!;
    const imported = vi.spyOn(mock, "importPagesFromDoc");
    // a 축소판 of page 4 whose lower half is under the release point
    const thumb = document.createElement("button");
    thumb.className = "thumb";
    thumb.dataset.page = "4";
    thumb.getBoundingClientRect = () => ({ top: 0, bottom: 100, height: 100, left: 0, right: 80, width: 80, x: 0, y: 0 }) as DOMRect;
    document.body.appendChild(thumb);
    const ok = await handlePagesDrop(
      { srcDocId: source.docId, pages: [0, 2], from: "doc-2", screen: { x: 1040, y: 580 } },
      { origin: async () => ({ x: 1000, y: 500 }), elementAt: () => thumb },
    );
    expect(ok).toBe(true);
    expect(imported).toHaveBeenCalledWith({ srcDocId: source.docId, pages: [0, 2], dstDocId: target.docId, at: 5 });
    expect(useDocStore.getState().info?.pageCount).toBe(target.pageCount + 2);
    // the window a drag started in ignores its own event
    expect(
      await handlePagesDrop(
        { srcDocId: source.docId, pages: [0], from: "main", screen: { x: 1040, y: 580 } },
        { origin: async () => ({ x: 1000, y: 500 }), elementAt: () => thumb },
      ),
    ).toBe(false);
    thumb.remove();
  });
});
