/**
 * v0.3 P4 / P5 against the mock: 문서 분할 ▸ 책갈피로 sends `{ byOutline: { level } }`; the link
 * panel's 테두리 표시 writes a border (`update_link { border }`), and a link made from a text
 * selection carries its quads.
 */
import { beforeEach, describe, expect, it, vi } from "vitest";
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import * as api from "../ipc/api";
import { mock } from "../ipc/mock";
import { useDocStore } from "../store/docStore";
import { useAnnotStore } from "../store/annotStore";
import { SplitDialog } from "./SplitDialog";
import { LinkPanel } from "../edit/LinkPanel";
import { createLink } from "../edit/linkActions";

const SAMPLE = "/Users/veri/Documents/SeePDF-샘플.pdf";

beforeEach(async () => {
  await useDocStore.getState().open(SAMPLE);
});

describe("문서 분할 ▸ 책갈피로 (P4)", () => {
  it("splits by the chosen bookmark level", async () => {
    await waitFor(() => expect(useDocStore.getState().outline.length).toBeGreaterThan(0));
    const split = vi.spyOn(mock, "splitDocument");
    render(<SplitDialog onClose={() => undefined} />);
    fireEvent.click(screen.getByRole("radio", { name: "책갈피로" }));
    fireEvent.change(screen.getByRole("combobox", { name: "책갈피 수준" }), { target: { value: "2" } });
    fireEvent.click(screen.getByRole("button", { name: "적용" }));
    await waitFor(() => expect(split).toHaveBeenCalledTimes(1));
    expect(split.mock.calls[0][0].mode).toEqual({ byOutline: { level: 2 } });
  });

  it("is disabled for a document without bookmarks", async () => {
    act(() => useDocStore.setState({ outline: [] }));
    render(<SplitDialog onClose={() => undefined} />);
    expect(screen.getByRole("radio", { name: "책갈피로" })).toBeDisabled();
    expect(screen.getByText("이 문서에는 책갈피가 없습니다")).toBeInTheDocument();
  });
});

describe("링크 테두리와 여러 줄 링크 (P5)", () => {
  it("테두리 표시 writes a visible border; off again writes width 0", async () => {
    const info = useDocStore.getState().info!;
    const quads = [
      { l: 100, b: 700, r: 300, t: 712 },
      { l: 72, b: 686, r: 180, t: 698 },
    ];
    const create = vi.spyOn(api, "createLink");
    const made = await createLink(0, quads[0], { page: 1 }, quads);
    expect(made).toBe(true);
    expect(create.mock.calls[0][0].quads).toEqual(quads);
    const link = useAnnotStore.getState().byPage[0]!.find((a) => a.kind === "link")!;
    expect(link.quads).toHaveLength(2);
    expect(link.rect).toEqual({ l: 72, b: 686, r: 300, t: 712 });

    const update = vi.spyOn(mock, "updateLink");
    render(<LinkPanel link={{ page: 0, id: link.id }} />);
    fireEvent.click(screen.getByRole("checkbox", { name: "테두리 표시" }));
    await waitFor(() => expect(update).toHaveBeenCalledTimes(1));
    expect(update.mock.calls[0][0]).toMatchObject({ docId: info.docId, page: 0, id: link.id, border: { width: 1, color: [0, 102, 204] } });
    await waitFor(() => expect(screen.getByRole("checkbox", { name: "테두리 표시" })).toBeChecked());
    fireEvent.click(screen.getByRole("checkbox", { name: "테두리 표시" }));
    await waitFor(() => expect(update).toHaveBeenCalledTimes(2));
    expect(update.mock.calls[1][0].border?.width).toBe(0);
  });
});
