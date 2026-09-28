/**
 * P2 annotation threads end to end against the mock: 답글 from the 주석 tab's context menu opens the
 * note's popover with the reply box focused, a reply is sent as Settings' 작성자 and lands under
 * its parent (count badge, fold), a reply is edited and deleted in place, 답글 on a highlight
 * opens the thread popover, and deleting a parent with replies asks first.
 */
import { beforeEach, describe, expect, it, vi } from "vitest";
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import App from "../App";
import { mock } from "../ipc/mock";
import { useAppStore } from "../store/appStore";
import { useAnnotStore } from "../store/annotStore";
import { useDocStore } from "../store/docStore";
import { useDialogStore } from "../dialogs/dialogState";
import { useViewStore } from "../store/viewStore";
import { useToastStore } from "../app/toastStore";

const SAMPLE = "/Users/veri/Documents/SeePDF-샘플.pdf";
const NOTE = "이 단락은 다시 검토할 것";
const HIGHLIGHT = "여기부터 확인";

beforeEach(() => {
  useAppStore.setState({ os: "macos", mode: "annotate", tool: "select", sidebarOpen: true, sidebarTab: "annotations" });
  useDialogStore.getState().closeAll();
  useViewStore.setState({ split: null, parked: null, focusedPane: "main" });
  // every test opens its own "d1": nothing of the previous test's document may survive
  useDocStore.setState({ docId: null, info: null, status: "empty", error: null });
  useAnnotStore.getState().reset();
  useToastStore.setState({ toasts: [] });
});

async function openSample() {
  render(<App />);
  await waitFor(() => expect(useAppStore.getState().ready).toBe(true), { timeout: 3000 });
  await act(async () => {
    await useDocStore.getState().open(SAMPLE);
  });
  act(() => useAppStore.setState({ mode: "annotate", sidebarOpen: true, sidebarTab: "annotations" }));
  await screen.findByText(NOTE, { selector: ".annot-row .excerpt" }, { timeout: 4000 });
}

function row(text: string): HTMLElement {
  const el = screen.getByText(text, { selector: ".annot-row .excerpt" }).closest(".annot-thread-item");
  if (!el) throw new Error(`no thread item for ${text}`);
  return el as HTMLElement;
}

async function menuItem(name: string) {
  return screen.findByRole("menuitem", { name }, { timeout: 4000 });
}

describe("annotation threads", () => {
  it("답글 → reply as the author → grouped under the note; edit and delete a reply", async () => {
    const reply = vi.spyOn(mock, "replyAnnotation");
    await openSample();

    // 답글 from the row's context menu: the note's popover opens with the reply box focused
    fireEvent.contextMenu(within(row(NOTE)).getByRole("button", { name: /메모/ }));
    fireEvent.click(await menuItem("답글"));
    const popover = await screen.findByTestId("note-popover", {}, { timeout: 4000 });
    const box = within(popover).getByRole("textbox", { name: "답글" });
    await waitFor(() => expect(box).toHaveFocus(), { timeout: 3000 });

    fireEvent.change(box, { target: { value: "확인했습니다. 3쪽도 봐 주세요" } });
    fireEvent.click(within(popover).getByRole("button", { name: "답글 달기" }));
    await waitFor(() => expect(reply).toHaveBeenCalledTimes(1), { timeout: 3000 });
    const parentId = useAnnotStore.getState().byPage[1].find((a) => a.contents === NOTE)!.id;
    expect(reply.mock.calls[0][0]).toMatchObject({ page: 1, parentId, contents: "확인했습니다. 3쪽도 봐 주세요", author: "박현우" });

    // the reply is listed in the popover and under its parent in the 주석 tab, not on its own
    await waitFor(() => expect(within(popover).getAllByTestId("annot-reply")).toHaveLength(1), { timeout: 3000 });
    await waitFor(() => expect((box as HTMLTextAreaElement).value).toBe(""), { timeout: 3000 });
    const thread = row(NOTE);
    const badge = within(thread).getByRole("button", { name: "답글 1개 접기" });
    expect(within(thread).getByTestId("annot-reply-row")).toHaveTextContent("확인했습니다. 3쪽도 봐 주세요");
    expect(within(thread).getByTestId("annot-reply-row")).toHaveTextContent("박현우");
    expect(screen.getAllByTestId("annot-thread")).toHaveLength(2); // the highlight + the note thread
    fireEvent.click(badge);
    expect(within(thread).queryByTestId("annot-reply-row")).toBeNull();
    fireEvent.click(within(thread).getByRole("button", { name: "답글 1개 펼치기" }));
    expect(within(thread).getByTestId("annot-reply-row")).toBeInTheDocument();

    // a second reply with ⌘↵, then edit the first in place
    fireEvent.change(box, { target: { value: "반영했습니다" } });
    fireEvent.keyDown(box, { key: "Enter", metaKey: true });
    await waitFor(() => expect(within(popover).getAllByTestId("annot-reply")).toHaveLength(2), { timeout: 3000 });
    const [first] = within(popover).getAllByTestId("annot-reply");
    fireEvent.click(within(first).getByRole("button", { name: "답글 편집" }));
    const editor = within(first).getByRole("textbox", { name: "답글 편집" });
    fireEvent.change(editor, { target: { value: "확인했습니다 (수정)" } });
    fireEvent.blur(editor);
    await waitFor(() =>
      expect(useAnnotStore.getState().byPage[1].some((a) => a.contents === "확인했습니다 (수정)")).toBe(true));

    // deleting one reply (no replies of its own) asks nothing
    const confirm = vi.spyOn(useDialogStore.getState(), "open");
    fireEvent.click(within(within(popover).getAllByTestId("annot-reply")[1]).getByRole("button", { name: "답글 삭제" }));
    await waitFor(() => expect(within(popover).getAllByTestId("annot-reply")).toHaveLength(1), { timeout: 3000 });
    expect(confirm).not.toHaveBeenCalledWith("confirm", expect.anything());
  });

  it("답글 on a highlight opens the thread popover; deleting a parent with replies asks first", async () => {
    await openSample();
    act(() => useViewStore.getState().goToPage(0));
    fireEvent.contextMenu(within(row(HIGHLIGHT)).getByRole("button", { name: /형광펜/ }));
    fireEvent.click(await menuItem("답글"));
    const popover = await screen.findByTestId("thread-popover", {}, { timeout: 4000 });
    expect(popover).toHaveTextContent(HIGHLIGHT);
    const box = within(popover).getByRole("textbox", { name: "답글" });
    fireEvent.change(box, { target: { value: "근거 자료 첨부 부탁드립니다" } });
    fireEvent.click(within(popover).getByRole("button", { name: "답글 달기" }));
    await waitFor(() => expect(within(popover).getAllByTestId("annot-reply")).toHaveLength(1), { timeout: 3000 });

    // the replies are never hit on the canvas
    const { annotsOnPage } = await import("./actions");
    expect(annotsOnPage(0).map((a) => a.contents)).toEqual([HIGHLIGHT]);

    // 삭제 on the parent: 주석과 답글 삭제 — 취소 keeps both
    const del = vi.spyOn(mock, "deleteAnnotations");
    fireEvent.click(within(row(HIGHLIGHT)).getByRole("button", { name: "삭제" }));
    expect(await screen.findByText("주석과 답글 삭제", {}, { timeout: 4000 })).toBeInTheDocument();
    expect(screen.getByText("이 주석에 달린 답글 1개도 함께 삭제됩니다. 삭제할까요?")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "취소" }));
    await waitFor(() => expect(screen.queryByText("주석과 답글 삭제")).toBeNull(), { timeout: 3000 });
    expect(del).not.toHaveBeenCalled();
    expect(useAnnotStore.getState().byPage[0]).toHaveLength(2);

    // …삭제 removes the thread in one call (the engine takes the replies with the parent)
    fireEvent.click(within(row(HIGHLIGHT)).getByRole("button", { name: "삭제" }));
    const dialog = await screen.findByRole("dialog", {}, { timeout: 4000 });
    fireEvent.click(within(dialog).getByRole("button", { name: "삭제" }));
    await waitFor(() => expect(del).toHaveBeenCalledTimes(1), { timeout: 3000 });
    expect(del.mock.calls[0][0].ids).toHaveLength(1);
    await waitFor(() => expect(useAnnotStore.getState().byPage[0]).toEqual([]), { timeout: 3000 });
    expect(screen.queryByText(HIGHLIGHT, { selector: ".annot-row .excerpt" })).toBeNull();
  });

  it("an encrypted document refuses replies with a toast", async () => {
    render(<App />);
    await waitFor(() => expect(useAppStore.getState().ready).toBe(true), { timeout: 3000 });
    await act(async () => {
      await useDocStore.getState().open("/Users/veri/Documents/encrypted-급여.pdf", "1234");
    });
    const info = useDocStore.getState().info!;
    const made = await mock.createAnnotation({
      docId: info.docId, page: 0, spec: { kind: "note", at: [100, 700], color: [255, 216, 77], contents: "메모" },
    });
    useAnnotStore.getState().setPage(0, made.list.annots, made.list.docGeneration);
    const { replyToAnnotation } = await import("./actions");
    // what the engine answers for an encrypted document (`cargo test --test thread reply_errors`)
    vi.spyOn(mock, "replyAnnotation").mockRejectedValueOnce({ code: "unsupported", message: "encrypted" });
    expect(await replyToAnnotation(0, made.annot!.id, "답글")).toBeNull();
    expect(await screen.findByText(/암호가 걸린 문서에는 답글을 저장할 수 없습니다/)).toBeInTheDocument();
  });
});
