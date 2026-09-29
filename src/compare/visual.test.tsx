/**
 * v0.3 pkg8 (X4) — the pixel diff's `visual` ops: collected per side by the model (never counted as
 * words), drawn as their own marks, and blinking until 바뀐 그림 깜빡이기 is turned off.
 */
import { afterEach, describe, expect, it } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import CompareView from "./CompareView";
import { buildRows } from "./model";
import { useCompareStore } from "./state";
import { useDocStore } from "../store/docStore";
import type { CompareReport, DocInfo, PageGeom } from "../ipc/types";

const A4: PageGeom = { index: 0, widthPt: 595, heightPt: 842, rotation: 0, crop: { l: 0, b: 0, r: 595, t: 842 }, label: null };

const REPORT: CompareReport = {
  docA: "d1",
  docB: "d2",
  changedPages: 1,
  inserted: 0,
  deleted: 0,
  elapsedMs: 5,
  pages: [
    {
      pageA: 0, pageB: 0, changed: true, wordsA: 4, wordsB: 4,
      ops: [
        { kind: "equal", words: 4 },
        {
          kind: "visual", words: 0,
          rectsA: [{ l: 150, b: 250, r: 450, t: 670 }, { l: 60, b: 60, r: 90, t: 90 }],
          rectsB: [{ l: 150, b: 250, r: 450, t: 670 }, { l: 60, b: 60, r: 90, t: 90 }],
        },
      ],
    },
    { pageA: 1, pageB: 1, changed: false, wordsA: 3, wordsB: 3, ops: [{ kind: "equal", words: 3 }] },
  ],
};

function info(docId: string): DocInfo {
  return {
    docId, name: `${docId}.pdf`, path: `/tmp/${docId}.pdf`, pageCount: 2, docGeneration: 1,
    pages: [A4, { ...A4, index: 1 }],
  } as unknown as DocInfo;
}

afterEach(() => {
  cleanup();
  useCompareStore.setState({ session: null });
});

describe("compare.visual", () => {
  it("collects visual rects per side without counting words", () => {
    const [row, same] = buildRows(REPORT);
    expect(row.visualA).toHaveLength(2);
    expect(row.visualB).toHaveLength(2);
    expect(row.rectsA).toEqual([]);
    expect([row.inserted, row.deleted]).toEqual([0, 0]);
    expect(same.visualA).toEqual([]);
  });

  it("draws the regions on both pages, labelled 그림 변경, blinking until turned off", () => {
    useDocStore.setState({ docId: "d1" });
    useCompareStore.setState({ session: { report: REPORT, infoA: info("d1"), infoB: info("d2") } });
    const { container } = render(<CompareView />);
    expect(screen.getAllByTestId("cmp-mark-visual")).toHaveLength(4);
    expect(screen.getByText("그림 변경")).toBeInTheDocument();
    const root = container.querySelector(".cmp-root")!;
    expect(root.hasAttribute("data-blink")).toBe(true);
    fireEvent.click(screen.getByRole("checkbox", { name: "바뀐 그림 깜빡이기" }));
    expect(root.hasAttribute("data-blink")).toBe(false);
    // The figure's region sits where the engine said: (150, 250)–(450, 670) on an A4 page.
    const mark = screen.getAllByTestId("cmp-mark-visual")[0];
    const scale = parseFloat(mark.style.width) / 300;
    expect(parseFloat(mark.style.left)).toBeCloseTo(150 * scale, 1);
    expect(parseFloat(mark.style.top)).toBeCloseTo((842 - 670) * scale, 1);
  });

  it("offers no blink switch when nothing changed visually", () => {
    const plain: CompareReport = { ...REPORT, pages: [REPORT.pages[1]], changedPages: 0 };
    useDocStore.setState({ docId: "d1" });
    useCompareStore.setState({ session: { report: plain, infoA: info("d1"), infoB: info("d2") } });
    render(<CompareView />);
    expect(screen.queryByRole("checkbox", { name: "바뀐 그림 깜빡이기" })).toBeNull();
    expect(screen.queryAllByTestId("cmp-mark-visual")).toHaveLength(0);
  });
});
