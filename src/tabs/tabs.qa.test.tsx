/**
 * v0.3.0 real-app QA (문서 탭): with the whole app mounted, the canvas is keyed by document, so a tab
 * switch remounts the viewer and restarts the annotation sync. Neither may throw away what
 * `tabs/flow` just put back — the tab's 분할 보기 and its annotation selection / filter.
 */
import { beforeEach, describe, expect, it } from "vitest";
import { act, render, waitFor } from "@testing-library/react";
import App from "../App";
import { openPath } from "../dialogs/flows";
import { useAppStore } from "../store/appStore";
import { useAnnotStore } from "../store/annotStore";
import { useTabStore } from "../store/tabStore";
import { useViewStore } from "../store/viewStore";
import { activateTab } from "./flow";

const A = "/Users/veri/Documents/계약서 초안.pdf";
const B = "/Users/veri/Documents/SeePDF-샘플.pdf";

const panes = () => document.querySelectorAll(".canvas.viewer").length;
const tabOf = (docId: string) => useTabStore.getState().tabs.find((t) => t.docId === docId)!.id;

beforeEach(() => {
  useAppStore.setState({ os: "macos", mode: "read", tool: "select" });
  useViewStore.getState().closeSplit();
});

describe("tabs in the mounted app", () => {
  it("switching back to a tab keeps its split and its annotation selection", async () => {
    render(<App />);
    await waitFor(() => expect(useAppStore.getState().ready).toBe(true), { timeout: 3000 });
    const a = (await act(() => openPath(A)))!;
    await waitFor(() => expect(document.querySelectorAll(".page-shell").length).toBeGreaterThan(0), { timeout: 3000 });
    act(() => useViewStore.getState().openSplit("side"));
    await waitFor(() => expect(panes()).toBe(2), { timeout: 3000 });
    act(() => {
      useAnnotStore.getState().select(["annot-qa"]);
      useAnnotStore.getState().setFilter(["highlight"]);
    });

    const b = (await act(() => openPath(B)))!;
    await waitFor(() => expect(panes()).toBe(1), { timeout: 3000 });
    expect(useViewStore.getState().split).toBeNull();
    expect(useAnnotStore.getState().selected).toEqual([]);

    await act(async () => {
      expect(await activateTab(tabOf(a.docId))).toBe(true);
    });
    await waitFor(() => expect(panes()).toBe(2), { timeout: 3000 });
    // let the remounted canvas settle (its annotation sync restarts on mount)
    await act(() => new Promise((r) => setTimeout(r, 50)));
    expect(useViewStore.getState().split?.orientation).toBe("side");
    expect(panes()).toBe(2);
    expect(useAnnotStore.getState().selected).toEqual(["annot-qa"]);
    expect(useAnnotStore.getState().filter).toEqual(["highlight"]);

    // and the other way: the tab without a split comes back without one
    await act(async () => {
      expect(await activateTab(tabOf(b.docId))).toBe(true);
    });
    await waitFor(() => expect(panes()).toBe(1), { timeout: 3000 });
    expect(useAnnotStore.getState().selected).toEqual([]);
  });
});
