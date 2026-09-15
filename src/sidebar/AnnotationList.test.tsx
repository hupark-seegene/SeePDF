/**
 * 주석 sidebar tab (F-14: "the sidebar lists every annotation grouped by page and scrolls to one
 * on click").
 */
import { beforeEach, describe, expect, it } from "vitest";
import { fireEvent, render, screen } from "@testing-library/react";
import type { Annot, AnnotKind } from "../ipc/types";
import { useAnnotStore } from "../store/annotStore";
import { useDocStore } from "../store/docStore";
import { useViewStore } from "../store/viewStore";
import * as api from "../ipc/api";
import { AnnotationList, groupByPage } from "./AnnotationList";

function annot(over: Partial<Annot>): Annot {
  return {
    id: "a", page: 0, kind: "highlight", subtype: "Highlight",
    rect: { l: 0, b: 0, r: 10, t: 10 }, color: [255, 216, 77], fillColor: null, opacity: 0.4,
    borderWidth: 0, contents: "", author: null, created: null, modified: null,
    hidden: false, printed: true, locked: false, editable: "full", ...over,
  };
}

describe("AnnotationList", () => {
  beforeEach(() => {
    useAnnotStore.getState().reset();
  });

  it("groups by page in page order and merges the optimistic ghosts in", () => {
    const groups = groupByPage(
      { 2: [annot({ id: "p2" })], 0: [annot({ id: "p0" })] },
      [annot({ id: "ghost", page: 1, kind: "note" })],
      null,
    );
    expect(groups.map(([page]) => page)).toEqual([0, 1, 2]);
    expect(groups[1][1][0].id).toBe("ghost");
  });

  it("filters by kind", () => {
    const byPage = { 0: [annot({ id: "h" }), annot({ id: "n", kind: "note" })] };
    const filter: AnnotKind[] = ["note"];
    expect(groupByPage(byPage, [], filter)[0][1].map((a) => a.id)).toEqual(["n"]);
  });

  it("lists the document's annotations and navigates on click", async () => {
    const info = await useDocStore.getState().open("/tmp/sample.pdf");
    if (!info) throw new Error("open failed");
    const page1 = await api.listAnnotations({ docId: info.docId, page: 1 });
    useAnnotStore.getState().setPage(1, page1.annots, page1.docGeneration);
    useViewStore.getState().goToPage(0);

    render(<AnnotationList />);
    const row = screen.getByText("이 단락은 다시 검토할 것").closest(".annot-row");
    expect(row).not.toBeNull();
    fireEvent.click(row as Element);

    expect(useAnnotStore.getState().selected).toEqual([page1.annots[0].id]);
    expect(useViewStore.getState().scrollRequest?.page).toBe(1);
    // a 메모 opens its popover as well (UI_SPEC §12 "메모 열기")
    expect(useAnnotStore.getState().editing).toEqual({ page: 1, id: page1.annots[0].id });
  });

  it("shows the empty state when the document has no annotations", () => {
    render(<AnnotationList />);
    expect(screen.getByText("주석이 없습니다")).toBeInTheDocument();
  });
});
