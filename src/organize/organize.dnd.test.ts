import { beforeEach, describe, expect, it, vi } from "vitest";
import { applyMove, caretToDestination, identityOrder, isNoOpMove, moveOpFor } from "./moveOp";
import { useDocStore } from "../store/docStore";
import { usePagesStore } from "../store/pagesStore";

vi.mock("../ipc/api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../ipc/api")>();
  return { ...actual, pageOps: vi.fn(actual.pageOps) };
});

const api = await import("../ipc/api");
const { runPageOps } = await import("../dialogs/flows");
const SAMPLE = "/Users/veri/Downloads/tracemonkey.pdf"; // 14 pages in the recents fixture

describe("organize.dnd", () => {
  beforeEach(() => {
    vi.mocked(api.pageOps).mockClear();
  });

  it("reproduces the pdfium ordered-list semantics: move([3,2] -> 1) gives A D C B", () => {
    // spikes/pages.md §1, the row the Rust side is tested against
    const order = identityOrder(14);
    expect(applyMove(order, [3, 2], 1).slice(0, 5)).toEqual([0, 2, 3, 1, 4]);
    expect(moveOpFor(order, [3, 2], 1)).toEqual({ kind: "move", pages: [2, 3], to: 1 });
  });

  it("converts a grid caret into FPDF_MovePages' destination index", () => {
    const order = identityOrder(10);
    // dragging pages 5 and 6 to the very front: nothing of the block lies before the caret
    expect(caretToDestination([5, 6], 0, order)).toBe(0);
    // dropping them after page 8: two moved pages already passed, so dest shifts down by two
    expect(caretToDestination([5, 6], 9, order)).toBe(7);
    expect(caretToDestination([0], 10, order)).toBe(9);
  });

  it("a drag produces exactly one move op and one page_ops call", async () => {
    await useDocStore.getState().open(SAMPLE);
    const order = identityOrder(useDocStore.getState().info!.pageCount);
    usePagesStore.getState().setSelected([2, 3]);

    const op = moveOpFor(order, usePagesStore.getState().selected, 6);
    expect(op).toEqual({ kind: "move", pages: [2, 3], to: 4 });

    await runPageOps([op!]);
    expect(api.pageOps).toHaveBeenCalledTimes(1);
    const arg = vi.mocked(api.pageOps).mock.calls[0][0];
    expect(arg.ops).toHaveLength(1);
    expect(arg.ops[0]).toEqual({ kind: "move", pages: [2, 3], to: 4 });
  });

  it("dropping a block where it already is changes nothing", () => {
    const order = identityOrder(8);
    expect(isNoOpMove(order, [2, 3], 2)).toBe(true);
    expect(isNoOpMove(order, [2, 3], 4)).toBe(true); // the caret just past the block
    expect(moveOpFor(order, [2, 3], 2)).toBeNull();
    expect(moveOpFor(order, [2, 3], 5)).not.toBeNull();
  });

  it("the optimistic order is painted immediately and dropped once the engine answers", async () => {
    await useDocStore.getState().open(SAMPLE);
    const order = identityOrder(14);
    usePagesStore.getState().setPendingOrder(applyMove(order, [0], 3));
    expect(usePagesStore.getState().pendingOrder?.slice(0, 3)).toEqual([1, 2, 0]);
    await runPageOps([{ kind: "move", pages: [0], to: 2 }]);
    expect(usePagesStore.getState().pendingOrder).toBeNull();
    expect(usePagesStore.getState().dropAt).toBeNull();
  });

  it("moving the whole document is refused rather than sent", () => {
    const order = identityOrder(3);
    expect(moveOpFor(order, [0, 1, 2], 0)).toBeNull();
    expect(moveOpFor(order, [], 1)).toBeNull();
  });
});
