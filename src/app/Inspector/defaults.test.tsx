/**
 * 도구별 기본 스타일 in the properties panel (P1-12): with nothing selected the panel edits the
 * armed tool's default (remembered per tool), 기본값으로 재설정 appears once there is an override,
 * and 이 스타일을 기본값으로 on a selection sets the default of the tool that drew it.
 */
import { beforeEach, describe, expect, it } from "vitest";
import { fireEvent, render, screen } from "@testing-library/react";
import InspectorBody from "./InspectorBody";
import type { Annot } from "../../ipc/types";
import { useAnnotStore } from "../../store/annotStore";
import { useAppStore } from "../../store/appStore";
import { baseStyleFor } from "../../store/toolStyles";

const SQUARE: Annot = {
  id: "sq", page: 0, kind: "square", subtype: "Square", rect: { l: 0, b: 0, r: 10, t: 10 }, color: [123, 214, 74],
  fillColor: null, opacity: 0.5, borderWidth: 4, contents: "", author: null, created: null, modified: null,
  hidden: false, printed: true, locked: false, editable: "full",
};

beforeEach(() => {
  useAnnotStore.getState().reset();
  useAnnotStore.setState({ toolDefaults: {}, styleTool: null, selected: [] });
  useAppStore.setState({ mode: "annotate", tool: "pen" });
  useAnnotStore.getState().activateTool("pen");
});

describe("properties panel — per-tool defaults", () => {
  it("a thickness change with nothing selected becomes 펜's default; 기본값으로 재설정 undoes it", () => {
    render(<InspectorBody />);
    expect(screen.queryByRole("button", { name: "기본값으로 재설정" })).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "8" }));
    expect(useAnnotStore.getState().toolDefaults).toEqual({ pen: { width: 8 } });
    fireEvent.click(screen.getByRole("button", { name: "기본값으로 재설정" }));
    expect(useAnnotStore.getState().toolDefaults).toEqual({});
    expect(useAnnotStore.getState().style).toEqual(baseStyleFor("pen"));
  });

  it("이 스타일을 기본값으로 on a selected 사각형 sets 사각형's default, not the armed tool's", () => {
    useAnnotStore.getState().setPage(0, [SQUARE], 1);
    useAnnotStore.getState().select(["sq"]);
    render(<InspectorBody />);
    fireEvent.click(screen.getByRole("button", { name: "이 스타일을 기본값으로" }));
    expect(useAnnotStore.getState().toolDefaults.rectangle).toMatchObject({ color: [123, 214, 74], opacity: 0.5, width: 4 });
    expect(useAnnotStore.getState().toolDefaults.pen).toBeUndefined();
  });

  it("offers 도장 변경… while 도장 is armed", () => {
    useAppStore.setState({ tool: "stamp" });
    render(<InspectorBody />);
    expect(screen.getByRole("button", { name: "도장 변경…" })).toBeInTheDocument();
  });
});
