/**
 * 편집 mode, Stage 9, against the mock adapter: the content below an edited paragraph follows it.
 * 완료 plans with a `push` dry run, then writes (content pushed down / pulled up, a toast with 실행
 * 취소); a push blocked by an obstacle asks — 글자 크기 줄여 맞추기 (N%) · 아래로 밀고 남은 부분은
 * 겹치기 · 계속 편집; while typing, a dashed line marks the original bottom once the text grows past
 * it; a commit that arrives mid-composition waits for the IME; Esc still cancels.
 *
 * Page 0 of the mock: paragraph A = two 11 pt lines at baselines 690 / 672 (leading 18, column
 * 72 … 366), paragraph B = two lines at 640 / 622 right below it, nothing else under them.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { mock } from "../ipc/mock";
import { onDocChanged } from "../ipc/events";
import type { ParagraphEditResult } from "../ipc/types";
import { makePageLayerContext, type PageLayerContext } from "../viewer";
import { useAppStore, type ToolId } from "../store/appStore";
import { useDocStore } from "../store/docStore";
import { useToastStore } from "../app/toastStore";
import { useDialogStore } from "../dialogs/dialogState";
import DialogHost from "../dialogs/DialogHost";
import { editLeaveGuard } from "../tools/commands";
import * as api from "../ipc/api";
import { useCommands } from "../app/useCommands";
import { ModeSwitcher } from "../app/ModeSwitcher";
import { useEditStore } from "./editStore";
import { EditLayer } from "./EditLayer";
import { editHost } from "./index";

let stop: (() => void) | null = null;
let offDoc: (() => void) | null = null;

async function setup(tool: ToolId = "editText") {
  const info = await useDocStore.getState().open("/tmp/sample.pdf");
  if (!info) throw new Error("mock open failed");
  // like App.tsx: the document store follows the engine's generation
  offDoc = onDocChanged((e) => useDocStore.getState().applyDocChanged(e));
  useAppStore.setState({ mode: "edit", tool });
  stop = editHost.start();
  const page = info.pages[0];
  const ctx = makePageLayerContext({
    docId: info.docId, docGeneration: info.docGeneration, page, rotation: 0, zoomPercent: 100,
    width: page.widthPt, height: page.heightPt,
  });
  const view = render(
    <>
      <EditLayer ctx={ctx} />
      <DialogHost />
    </>,
  );
  await waitFor(() => expect(useEditStore.getState().pages[0]).toBeDefined());
  const surface = view.container.querySelector(".edit-surface") as HTMLElement;
  return { info, ctx, surface, ...view };
}

function at(ctx: PageLayerContext, x: number, y: number) {
  const [cx, cy] = ctx.toDevice(x, y);
  return { clientX: cx, clientY: cy, pointerId: 1, button: 0 };
}

const objects = () => useEditStore.getState().pages[0]?.objects ?? [];
const textOf = (prefix: string) => objects().find((o) => o.text?.startsWith(prefix));
const toastKeys = () => useToastStore.getState().toasts.map((t) => t.messageKey);
const toastOf = (key: string) => useToastStore.getState().toasts.find((t) => t.messageKey === key);
const words = (n: number) => Array.from({ length: n }, (_, i) => `word${i}`).join(" ");
const writes = (spy: { mock: { calls: [{ edit: { dryRun?: boolean } }][] } }) =>
  spy.mock.calls.filter(([a]) => !a.edit.dryRun).length;

/** 문단 편집 on paragraph A. */
async function openA(ctx: PageLayerContext, surface: HTMLElement) {
  fireEvent.pointerDown(surface, at(ctx, 100, 692));
  const box = (await screen.findByLabelText("문단 편집")) as HTMLTextAreaElement;
  expect(box.value.startsWith("이 페이지는")).toBe(true);
  return box;
}

/** A full-width figure under paragraph B: an obstacle 7.36 pt below B's last line. */
async function addObstacle(docId: string) {
  const result = await mock.addImageObject({
    docId, page: 0, rect: { l: 40, r: 560, t: 612, b: 500 }, path: "/tmp/figure.png", keepAspect: false,
  });
  act(() => useEditStore.getState().setPage(0, result));
}

beforeEach(() => {
  useEditStore.getState().reset();
  useToastStore.setState({ toasts: [] });
  useDialogStore.getState().closeAll();
});

afterEach(() => {
  const top = useDialogStore.getState().stack.at(-1);
  if (top?.name === "choice") (top.props.resolve as (v: string) => void)("keepEditing");
  useEditStore.getState().closeSession();
  stop?.();
  stop = null;
  offDoc?.();
  offDoc = null;
  vi.restoreAllMocks();
});

describe("편집 · 문단 편집 · 아래 내용 따라가기", () => {
  it("a paragraph that grows: one push dry run, one push write, the content below moves down, 실행 취소", async () => {
    const { ctx, surface } = await setup();
    const edit = vi.spyOn(mock, "editParagraph");
    const undo = vi.spyOn(mock, "undo");
    const b0 = textOf("The mock text layer")!.rect;
    const generation = useDocStore.getState().info!.docGeneration;

    const box = await openA(ctx, surface);
    box.value = words(18); // three lines instead of two: 18 pt taller
    fireEvent.click(screen.getByRole("button", { name: "완료" }));

    await waitFor(() => expect(useEditStore.getState().session).toBeNull());
    expect(edit).toHaveBeenCalledTimes(2);
    expect(edit.mock.calls[0][0].edit).toMatchObject({ flow: "push", dryRun: true, text: words(18) });
    expect(edit.mock.calls[1][0].edit).toMatchObject({ flow: "push", text: words(18) });
    expect(edit.mock.calls[1][0].edit.dryRun).toBeUndefined();
    // the dry run wrote nothing: the write expected the same generation, and made one step
    expect(edit.mock.calls[1][0].expectGeneration).toBe(generation);
    expect(useDocStore.getState().info!.docGeneration).toBe(generation + 1);

    expect(textOf("The mock text layer")!.rect.t).toBeCloseTo(b0.t - 18);
    expect(textOf("Search for PDF")).toBeDefined();
    expect(toastKeys()).toEqual(["edit.flow.pushed"]);
    const pushed = toastOf("edit.flow.pushed")!;
    expect(pushed.params).toEqual({ pt: 18 });
    expect(pushed.actions?.[0].labelKey).toBe("common.undo");
    expect(useDialogStore.getState().stack).toHaveLength(0);

    act(() => pushed.actions![0].onSelect());
    await waitFor(() => expect(undo).toHaveBeenCalledTimes(1));
  });

  it("a paragraph that shrinks pulls the content below up", async () => {
    const { ctx, surface } = await setup();
    const edit = vi.spyOn(mock, "editParagraph");
    const b0 = textOf("The mock text layer")!.rect;
    const box = await openA(ctx, surface);
    box.value = words(5); // one line instead of two
    fireEvent.keyDown(box, { key: "Enter", metaKey: true });

    await waitFor(() => expect(useEditStore.getState().session).toBeNull());
    expect(edit).toHaveBeenCalledTimes(2);
    expect(writes(edit)).toBe(1);
    expect(textOf("The mock text layer")!.rect.t).toBeCloseTo(b0.t + 18);
    expect(toastKeys()).toEqual(["edit.flow.pulled"]);
    expect(toastOf("edit.flow.pulled")!.params).toEqual({ pt: 18 });
  });

  it("a growth the paragraph spacing absorbs is not blocked: no prompt, a plain push", async () => {
    const { info, ctx, surface } = await setup();
    await addObstacle(info.docId);
    const edit = vi.spyOn(mock, "editParagraph");
    const b0 = textOf("The mock text layer")!.rect;
    const box = await openA(ctx, surface);
    box.value = words(18); // one more line (18 pt): room 7.36 + the 20.78 pt gap above B hold it
    fireEvent.click(screen.getByRole("button", { name: "완료" }));

    await waitFor(() => expect(useEditStore.getState().session).toBeNull());
    expect(useDialogStore.getState().stack).toHaveLength(0);
    expect(edit.mock.calls.map(([a]) => [a.edit.flow, !!a.edit.dryRun])).toEqual([["push", true], ["push", false]]);
    const result = (await edit.mock.results[1].value) as ParagraphEditResult;
    expect(result).toMatchObject({ overflowPt: 0, shiftedPt: 7.36 });
    expect(result.blocked).toBeUndefined();
    expect(textOf("The mock text layer")!.rect.t).toBeCloseTo(b0.t - 7.36);
    // no overlap: the new paragraph ends above B's moved top
    expect(result.rect.b).toBeGreaterThan(textOf("The mock text layer")!.rect.t);
    expect(toastKeys()).toEqual(["edit.flow.pushed"]);
    expect(toastOf("edit.flow.pushed")!.params).toEqual({ pt: 7.4 });
  });

  it("blocked by an obstacle → 글자 크기 줄여 맞추기 (N%) writes with flow 'fit'", async () => {
    const { info, ctx, surface } = await setup();
    await addObstacle(info.docId);
    const edit = vi.spyOn(mock, "editParagraph");
    const b0 = textOf("The mock text layer")!.rect;
    const box = await openA(ctx, surface);
    box.value = words(25); // two more lines: 36 pt against 7.36 of room + the 20.78 pt gap
    fireEvent.click(screen.getByRole("button", { name: "완료" }));

    const fit = await screen.findByRole("button", { name: "글자 크기 줄여 맞추기 (70%)" });
    expect(screen.getByRole("dialog", { name: "문단이 들어갈 자리가 부족합니다" })).toBeInTheDocument();
    expect(screen.getByText("아래에 있는 다른 내용(그림·표·바닥글 등) 때문에 더 밀 수 없어 7.9pt가 겹칩니다.")).toBeInTheDocument();
    expect(fit).toBeEnabled();
    // a push dry run, then a fit dry run for the percentage; nothing written yet
    expect(edit.mock.calls.map(([a]) => [a.edit.flow, a.edit.dryRun])).toEqual([["push", true], ["fit", true]]);

    fireEvent.click(fit);
    await waitFor(() => expect(useEditStore.getState().session).toBeNull());
    expect(edit).toHaveBeenCalledTimes(3);
    expect(edit.mock.calls[2][0].edit).toMatchObject({ flow: "fit", text: words(25) });
    expect(edit.mock.calls[2][0].edit.dryRun).toBeUndefined();
    // nothing below moved; the paragraph got smaller instead
    expect(textOf("The mock text layer")!.rect).toEqual(b0);
    expect(textOf("word0")!.fontSizePt).toBeCloseTo(11 * 0.7);
    expect(toastKeys()).toEqual(["edit.flow.fitted"]);
    expect(toastOf("edit.flow.fitted")!.params).toEqual({ pct: 70 });
    expect(toastOf("edit.flow.fitted")!.actions?.[0].labelKey).toBe("common.undo");
  });

  it("blocked → 아래로 밀고 남은 부분은 겹치기 pushes as far as it can and warns about the rest", async () => {
    const { info, ctx, surface } = await setup();
    await addObstacle(info.docId);
    const edit = vi.spyOn(mock, "editParagraph");
    const b0 = textOf("The mock text layer")!.rect;
    const box = await openA(ctx, surface);
    box.value = words(25);
    fireEvent.click(screen.getByRole("button", { name: "완료" }));

    fireEvent.click(await screen.findByRole("button", { name: "아래로 밀고 남은 부분은 겹치기" }));
    await waitFor(() => expect(useEditStore.getState().session).toBeNull());
    expect(edit).toHaveBeenCalledTimes(3);
    expect(edit.mock.calls[2][0].edit).toMatchObject({ flow: "push", text: words(25) });
    expect(edit.mock.calls[2][0].edit.dryRun).toBeUndefined();
    const result = (await edit.mock.results[2].value) as ParagraphEditResult;
    expect(result.blocked).toBe("obstacle");
    // pushed by the room there was (7.36 pt), the rest overlaps
    expect(textOf("The mock text layer")!.rect.t).toBeCloseTo(b0.t - 7.36);
    expect(toastKeys()).toEqual(["edit.flow.pushedOverlap"]);
    expect(toastOf("edit.flow.pushedOverlap")!.params).toEqual({ pt: 7.4, over: 7.9 });
    // what the toast says is what is on the page: the new text reaches 7.86 pt into B's moved top
    const bTop = textOf("The mock text layer")!.rect.t;
    expect(bTop - result.rect.b).toBeGreaterThan(0);
  });

  it("blocked → 계속 편집 writes nothing and keeps the typed text; Esc then cancels", async () => {
    const { info, ctx, surface } = await setup();
    await addObstacle(info.docId);
    const edit = vi.spyOn(mock, "editParagraph");
    const generation = useDocStore.getState().info!.docGeneration;
    const box = await openA(ctx, surface);
    box.value = words(25);
    fireEvent.click(screen.getByRole("button", { name: "완료" }));

    fireEvent.click(await screen.findByRole("button", { name: "계속 편집" }));
    await waitFor(() => expect(useDialogStore.getState().stack).toHaveLength(0));
    await act(async () => new Promise((r) => setTimeout(r, 10)));
    expect(writes(edit)).toBe(0);
    expect(useDocStore.getState().info!.docGeneration).toBe(generation);
    expect(useEditStore.getState().session).not.toBeNull();
    const still = screen.getByLabelText("문단 편집") as HTMLTextAreaElement;
    expect(still).toBe(box);
    expect(still.value).toBe(words(25));
    expect(document.activeElement).toBe(still);
    expect(toastKeys()).toEqual([]);

    fireEvent.keyDown(still, { key: "Escape" });
    expect(useEditStore.getState().session).toBeNull();
    expect(writes(edit)).toBe(0);
  });

  it("blocked when even the smallest size overflows: the fit choice is disabled and says why", async () => {
    const { info, ctx, surface } = await setup();
    await addObstacle(info.docId);
    const box = await openA(ctx, surface);
    box.value = words(60);
    fireEvent.click(screen.getByRole("button", { name: "완료" }));

    const fit = await screen.findByRole("button", { name: "글자 크기 줄여 맞추기 (70%)" });
    expect(fit).toBeDisabled();
    expect(screen.getByText("글자를 70%까지 줄여도 원래 자리에 다 들어가지 않습니다.")).toBeInTheDocument();
    // Esc in the prompt = 계속 편집
    fireEvent.keyDown(screen.getByRole("dialog"), { key: "Escape" });
    await waitFor(() => expect(useDialogStore.getState().stack).toHaveLength(0));
    await act(async () => undefined);
    expect(useEditStore.getState().session).not.toBeNull();
  });
});

describe("편집 · 문단 편집 · 모드 떠나기", () => {
  it("leaving 편집 commits the open editor first; 계속 편집 in the prompt keeps the mode", async () => {
    const { info, ctx, surface } = await setup();
    await addObstacle(info.docId);
    const edit = vi.spyOn(mock, "editParagraph");
    let box = await openA(ctx, surface);
    box.value = words(25);
    const blocked = editLeaveGuard()!;
    fireEvent.click(await screen.findByRole("button", { name: "계속 편집" }));
    await expect(blocked).resolves.toBe(false);
    expect(writes(edit)).toBe(0);
    expect((screen.getByLabelText("문단 편집") as HTMLTextAreaElement).value).toBe(words(25));

    box = screen.getByLabelText("문단 편집") as HTMLTextAreaElement;
    box.value = words(12); // still two lines: nothing to push
    await expect(editLeaveGuard()!).resolves.toBe(true);
    expect(writes(edit)).toBe(1);
    expect(useEditStore.getState().session).toBeNull();
  });
});

describe("편집 · 문단 편집 · 입력 중", () => {
  /** jsdom has no layout: the textarea reports the height it is told to. */
  function fakeHeight(box: HTMLTextAreaElement, px: number) {
    Object.defineProperty(box, "scrollHeight", { configurable: true, get: () => px });
    fireEvent.input(box);
  }

  it("the dashed original-bottom line and the hint appear only once the text passes the original box", async () => {
    const { ctx, surface } = await setup();
    const box = await openA(ctx, surface);
    const line = ctx.scale * 18;
    expect(screen.queryByTestId("edit-origin-line")).toBeNull();

    fakeHeight(box, 2 * line); // still two lines: inside the original box
    expect(screen.queryByTestId("edit-origin-line")).toBeNull();
    expect(screen.queryByText("완료하면 아래 내용이 밀립니다")).toBeNull();

    fakeHeight(box, 3 * line); // a third line: past the original bottom
    const dashed = screen.getByTestId("edit-origin-line");
    expect(parseFloat(dashed.style.top)).toBeCloseTo((698.58 - 669.36) * ctx.scale);
    expect(screen.getByText("완료하면 아래 내용이 밀립니다")).toBeInTheDocument();

    fakeHeight(box, 2 * line); // back inside
    expect(screen.queryByTestId("edit-origin-line")).toBeNull();
    expect(screen.queryByText("완료하면 아래 내용이 밀립니다")).toBeNull();

    // Esc still cancels, grown or not
    fakeHeight(box, 5 * line);
    fireEvent.keyDown(box, { key: "Escape" });
    expect(useEditStore.getState().session).toBeNull();
    expect(screen.queryByTestId("edit-origin-line")).toBeNull();
  });

  it("텍스트 추가 never shows the line or the hint and still writes with add_text_object", async () => {
    const { ctx, surface } = await setup("addText");
    const add = vi.spyOn(mock, "addTextObject");
    const edit = vi.spyOn(mock, "editParagraph");
    fireEvent.pointerDown(surface, at(ctx, 72, 500));
    const box = (await screen.findByLabelText("새 텍스트")) as HTMLTextAreaElement;
    box.value = "첫 줄\n둘째 줄\n셋째 줄\n넷째 줄";
    fakeHeight(box, 400);
    expect(screen.queryByTestId("edit-origin-line")).toBeNull();
    expect(screen.queryByText("완료하면 아래 내용이 밀립니다")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "완료" }));
    await waitFor(() => expect(add).toHaveBeenCalledTimes(1));
    expect(edit).not.toHaveBeenCalled();
    expect(useDialogStore.getState().stack).toHaveLength(0);
  });

  it("a commit mid-composition waits for the IME and writes the finished syllables", async () => {
    const { ctx, surface } = await setup();
    const edit = vi.spyOn(mock, "editParagraph");
    const box = await openA(ctx, surface);
    fireEvent.compositionStart(box);
    box.value = "가나다라ㅁ";
    // ⌘↵ while composing is the IME's, not ours
    fireEvent.keyDown(box, { key: "Enter", metaKey: true });
    await act(async () => undefined);
    expect(edit).not.toHaveBeenCalled();
    // a click outside lands before the browser ends the composition
    fireEvent.pointerDown(document.body, { pointerId: 1, button: 0 });
    await act(async () => undefined);
    expect(edit).not.toHaveBeenCalled();
    expect(useEditStore.getState().session).not.toBeNull();

    box.value = "가나다라마";
    fireEvent.compositionEnd(box);
    await waitFor(() => expect(useEditStore.getState().session).toBeNull());
    expect(edit.mock.calls[0][0].edit).toMatchObject({ text: "가나다라마", dryRun: true });
    expect(edit.mock.calls.at(-1)![0].edit.text).toBe("가나다라마");
    expect(writes(edit)).toBe(1);
  });
});

describe("편집 · 문단 편집 · Stage 9 verification round", () => {
  it("a change while the prompt is open (redo under it) writes nothing over the wrong objects; the editor stays open on the paragraph found again", async () => {
    const { info, ctx, surface } = await setup();
    await addObstacle(info.docId);
    // earlier on this page: delete the subtitle (object 1), then undo it — the delete waits on the redo stack
    const del = await mock.deleteObjects({
      docId: info.docId, page: 0, objectIds: [1], expectGeneration: useDocStore.getState().info!.docGeneration,
    });
    act(() => useEditStore.getState().setPage(0, del));
    await act(async () => void (await api.undo({ docId: info.docId })));
    await act(async () => useEditStore.getState().setPage(0, await mock.listPageObjects({ docId: info.docId, page: 0 })));
    const edit = vi.spyOn(mock, "editParagraph");
    const box = await openA(ctx, surface);
    const probe0 = (useEditStore.getState().session as { probe: { objectIds: number[] } }).probe;
    expect(probe0.objectIds).toEqual([2, 3]);
    box.value = words(25);
    fireEvent.click(screen.getByRole("button", { name: "완료" }));
    await screen.findByRole("button", { name: "아래로 밀고 남은 부분은 겹치기" });

    // the document moves under the prompt: A is now objects 1 and 2
    await act(async () => void (await api.redo({ docId: info.docId })));
    fireEvent.click(screen.getByRole("button", { name: "아래로 밀고 남은 부분은 겹치기" }));
    await waitFor(() => expect(toastKeys()).toContain("edit.flow.docChanged"));

    // every call was pinned to the probe's generation, so the write was refused …
    const generations = new Set(edit.mock.calls.map(([a]) => a.expectGeneration));
    expect(generations.size).toBe(1);
    const page = await mock.listPageObjects({ docId: info.docId, page: 0 });
    const texts = page.objects.map((o) => o.text ?? "");
    expect(texts.some((t) => t.startsWith("The mock text layer"))).toBe(true);
    expect(texts.some((t) => t.startsWith("word0"))).toBe(false);
    // … and the editor is still open, typed text kept, on the paragraph found again
    const session = useEditStore.getState().session as { probe: { objectIds: number[] } } | null;
    expect(session?.probe.objectIds).toEqual([1, 2]);
    const still = screen.getByLabelText("문단 편집") as HTMLTextAreaElement;
    expect(still.value).toBe(words(25));

    // 완료 again writes it where it is now
    fireEvent.click(screen.getByRole("button", { name: "완료" }));
    fireEvent.click(await screen.findByRole("button", { name: "아래로 밀고 남은 부분은 겹치기" }));
    await waitFor(() => expect(useEditStore.getState().session).toBeNull());
    const after = (await mock.listPageObjects({ docId: info.docId, page: 0 })).objects.map((o) => o.text ?? "");
    expect(after.some((t) => t.startsWith("The mock text layer"))).toBe(true);
    expect(after.some((t) => t.startsWith("이 페이지는"))).toBe(false);
    expect(after.some((t) => t.startsWith("word0"))).toBe(true);
  });

  it("nothing below the paragraph: the prompt and the toast say it runs past the page bottom, not that it overlaps", async () => {
    const { info, ctx, surface } = await setup();
    const added = await mock.addTextObject({
      docId: info.docId, page: 0, rect: { l: 72, t: 60, r: 400, b: 20 }, text: "Closing words near the bottom\nof the page, the last lines",
      fontSizePt: 12, color: [0, 0, 0], align: "left",
    });
    act(() => useEditStore.getState().setPage(0, added));
    const edit = vi.spyOn(mock, "editParagraph");
    fireEvent.pointerDown(surface, at(ctx, 90, 52));
    const box = (await screen.findByLabelText("문단 편집")) as HTMLTextAreaElement;
    box.value = words(40);
    fireEvent.click(screen.getByRole("button", { name: "완료" }));

    const dialog = await screen.findByRole("dialog", { name: "문단이 들어갈 자리가 부족합니다" });
    const plan = (await edit.mock.results[0].value) as ParagraphEditResult;
    expect(plan).toMatchObject({ blocked: "pageBottom", overflowPt: 0, movedObjects: 0 });
    expect(plan.pastBottomPt).toBeGreaterThan(0);
    const pt = Math.round(plan.pastBottomPt! * 10) / 10;
    expect(dialog).toHaveTextContent(`아래에는 다른 내용이 없지만, 문단이 페이지 아래 여백을 ${pt}pt 넘어갑니다.`);
    expect(screen.queryByRole("button", { name: "아래로 밀고 남은 부분은 겹치기" })).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "그대로 쓰기 (페이지 아래로 넘침)" }));
    await waitFor(() => expect(useEditStore.getState().session).toBeNull());
    expect(toastKeys()).toEqual(["edit.flow.pastBottom"]);
    expect(toastOf("edit.flow.pastBottom")!.params).toEqual({ pt });
  });

  it("a pending 영역 표시 mark over the content that moves goes with it; a mark elsewhere stays", async () => {
    const { ctx, surface } = await setup();
    const b0 = textOf("The mock text layer")!.rect;
    const sub = objects()[1].rect; // the line above paragraph A
    act(() => {
      useEditStore.getState().addMarks(0, [
        { l: b0.l - 1, b: b0.b - 1, r: b0.r + 1, t: b0.t + 1 },
        { l: sub.l, b: sub.b, r: sub.r, t: sub.t },
      ]);
    });
    const [onB, elsewhere] = useEditStore.getState().marks.map((m) => ({ ...m.rect }));
    const box = await openA(ctx, surface);
    box.value = words(18);
    fireEvent.click(screen.getByRole("button", { name: "완료" }));
    await waitFor(() => expect(useEditStore.getState().session).toBeNull());

    const b1 = textOf("The mock text layer")!.rect;
    expect(b1.t).toBeCloseTo(b0.t - 18);
    const [markB, markElsewhere] = useEditStore.getState().marks.map((m) => m.rect);
    expect(markB.t).toBeCloseTo(onB.t - 18);
    expect(markB.b <= b1.b && markB.t >= b1.t && markB.l <= b1.l && markB.r >= b1.r).toBe(true);
    expect(markElsewhere).toEqual(elsewhere);
  });
});

describe("편집 · 문단 편집 · 다시 쓸 수 없는 페이지", () => {
  it("an edit the engine refuses because the page holds an inline image or a gradient says so — and the typed text stays in the editor", async () => {
    const { ctx, surface } = await setup();
    const edit = vi.spyOn(mock, "editParagraph").mockRejectedValue({
      code: "unsupported",
      message: "rewriting this page would drop content PDFium cannot write back",
      detail: "unwritableContent",
    });
    const box = await openA(ctx, surface);
    box.value = words(18);
    fireEvent.click(screen.getByRole("button", { name: "완료" }));
    await waitFor(() => expect(toastKeys()).toEqual(["edit.reason.unwritableContent"]));
    // push refused, then overlap refused too: nothing written, the editor still open with the text
    expect(edit.mock.calls.map(([a]) => `${a.edit.flow}${a.edit.dryRun ? " dry" : ""}`)).toEqual(["push dry", "overlap dry"]);
    expect(useEditStore.getState().session).not.toBeNull();
    expect((screen.getByLabelText("문단 편집") as HTMLTextAreaElement).value).toBe(words(18));
  });

  it("only the push is unwritable (what follows sits in a stream that cannot be rewritten): the prompt offers writing over it instead of dropping the text", async () => {
    const { ctx, surface } = await setup();
    const real = mock.editParagraph.bind(mock);
    const edit = vi.spyOn(mock, "editParagraph").mockImplementation(async (a) => {
      if ((a.edit.flow ?? "push") === "push") {
        throw { code: "unsupported", message: "rewriting this page would drop content", detail: "unwritableContent" };
      }
      return real(a);
    });
    const box = await openA(ctx, surface);
    box.value = words(40);
    fireEvent.click(screen.getByRole("button", { name: "완료" }));
    const dialog = await screen.findByRole("dialog", { name: "문단이 들어갈 자리가 부족합니다" });
    expect(dialog).toHaveTextContent("이 페이지에서는 아래 내용을 옮길 수 없어");
    expect(screen.queryByRole("button", { name: "아래로 밀고 남은 부분은 겹치기" })).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "겹쳐서 그대로 쓰기" }));
    await waitFor(() => expect(useEditStore.getState().session).toBeNull());
    const last = edit.mock.calls.at(-1)![0].edit;
    expect(last).toMatchObject({ flow: "overlap", text: words(40) });
    expect(last.dryRun).toBeUndefined();
    expect(objects().some((o) => o.text?.startsWith("word0"))).toBe(true);
    expect(toastKeys()).toEqual(["edit.flow.overlaps"]);
  });
});

describe("편집 · 문단 편집 · 검증 2차", () => {
  /** A grows by a line (B pushed down), then B's editor opens where B is now. */
  async function pushThenOpenB(ctx: PageLayerContext, surface: HTMLElement) {
    const a = await openA(ctx, surface);
    a.value = words(18);
    fireEvent.click(screen.getByRole("button", { name: "완료" }));
    await waitFor(() => expect(useEditStore.getState().session).toBeNull());
    const b = textOf("The mock text layer")!.rect;
    fireEvent.pointerDown(surface, at(ctx, 100, (b.t + b.b) / 2));
    return (await screen.findByLabelText("문단 편집")) as HTMLTextAreaElement;
  }

  it("the document changes under the editor and the paragraph moved: it is found again elsewhere and the same editor keeps the typed text", async () => {
    const { info, ctx, surface } = await setup();
    const box = await pushThenOpenB(ctx, surface);
    const before = (useEditStore.getState().session as { probe: { rect: { t: number } } }).probe.rect;
    const typed = `${box.value} One more sentence typed here.`;
    box.value = typed;
    // ⌘Z under the open editor undoes A's push: B moves back up (the generation moves on)
    await act(async () => void (await api.undo({ docId: info.docId })));
    const edit = vi.spyOn(mock, "editParagraph");
    fireEvent.click(screen.getByRole("button", { name: "완료" }));
    await waitFor(() => expect(toastKeys()).toContain("edit.flow.docChanged"));
    const found = (useEditStore.getState().session as { probe: { rect: { t: number } } }).probe.rect;
    expect(found.t).toBeGreaterThan(before.t + 10);
    const now = screen.getByLabelText("문단 편집") as HTMLTextAreaElement;
    expect(now).toBe(box);
    expect(now.value).toBe(typed);
    // 완료 again writes it where the paragraph is now
    fireEvent.click(screen.getByRole("button", { name: "완료" }));
    await waitFor(() => expect(useEditStore.getState().session).toBeNull());
    expect(writes(edit)).toBe(1);
    expect(objects().some((o) => o.text?.includes("typed here."))).toBe(true);
  });

  it("a right-aligned paragraph with a short first line is found again after an unrelated change, from its first line", async () => {
    const { info, ctx, surface } = await setup();
    for (const [l, t, text] of [[300, 420, "Seoul office"], [150, 405.6, "123 Teheran-ro, Gangnam-gu, Seoul 06236"]] as const) {
      const r = await mock.addTextObject({ docId: info.docId, page: 0, rect: { l, t, r: 460, b: t - 14 }, text, fontSizePt: 12, color: [0, 0, 0], align: "right" });
      act(() => useEditStore.getState().setPage(0, r));
    }
    fireEvent.pointerDown(surface, at(ctx, 320, 413));
    const box = (await screen.findByLabelText("문단 편집")) as HTMLTextAreaElement;
    const typed = `${box.value} (4th floor)`;
    box.value = typed;
    // an unrelated change on another page moves the generation on
    await act(async () => void (await mock.addImageObject({ docId: info.docId, page: 1, rect: { l: 40, r: 100, t: 100, b: 40 }, path: "/tmp/x.png", keepAspect: false })));
    fireEvent.click(screen.getByRole("button", { name: "완료" }));
    await waitFor(() => expect(toastKeys()).toContain("edit.flow.docChanged"));
    expect(toastKeys()).not.toContain("edit.flow.paragraphGone");
    expect((screen.getByLabelText("문단 편집") as HTMLTextAreaElement).value).toBe(typed);
  });

  it("when the paragraph is really gone, the editor stays open with what was typed", async () => {
    const { info, ctx, surface } = await setup();
    const box = await openA(ctx, surface);
    box.value = words(18);
    const probe = (useEditStore.getState().session as { probe: { objectIds: number[] } }).probe;
    // A is deleted under the editor
    await act(async () => {
      const r = await mock.deleteObjects({ docId: info.docId, page: 0, objectIds: probe.objectIds, expectGeneration: useDocStore.getState().info!.docGeneration });
      useEditStore.getState().setPage(0, r);
    });
    fireEvent.click(screen.getByRole("button", { name: "완료" }));
    await waitFor(() => expect(toastKeys()).toContain("edit.flow.paragraphGone"));
    expect(useEditStore.getState().session).not.toBeNull();
    expect((screen.getByLabelText("문단 편집") as HTMLTextAreaElement).value).toBe(words(18));
  });

  it("a pending 영역 표시 mark drawn with a margin around the last moved line moves with it (its centre is in the moved band)", async () => {
    const { ctx, surface } = await setup();
    const lines = objects().filter((o) => o.text && o.rect.t < 650 && o.rect.b > 600);
    const last = lines.reduce((lo, o) => (o.rect.b < lo.rect.b ? o : lo)).rect;
    act(() => useEditStore.getState().addMarks(0, [{ l: last.l - 1, b: last.b - 6, r: last.r + 1, t: last.t + 1 }]));
    const mark0 = { ...useEditStore.getState().marks[0].rect };
    const box = await openA(ctx, surface);
    box.value = words(18);
    fireEvent.click(screen.getByRole("button", { name: "완료" }));
    await waitFor(() => expect(useEditStore.getState().session).toBeNull());
    expect(toastKeys()).not.toContain("redact.marksDropped");
    const moved = useEditStore.getState().marks[0]?.rect;
    expect(moved?.t).toBeCloseTo(mark0.t - 18);
    expect(moved?.b).toBeCloseTo(mark0.b - 18);
  });

  it("emptying a paragraph deletes it and pulls what follows up into its place, as one edit", async () => {
    const { ctx, surface } = await setup();
    const del = vi.spyOn(mock, "deleteObjects");
    const edit = vi.spyOn(mock, "editParagraph");
    const a0 = textOf("이 페이지는")!.rect;
    const box = await openA(ctx, surface);
    box.value = "";
    fireEvent.click(screen.getByRole("button", { name: "완료" }));
    await waitFor(() => expect(useEditStore.getState().session).toBeNull());
    expect(del).not.toHaveBeenCalled();
    expect(edit.mock.calls.map(([a]) => [a.edit.text, a.edit.flow, a.edit.dryRun ?? false])).toEqual([["", "push", true], ["", "push", false]]);
    expect(textOf("이 페이지는")).toBeUndefined();
    expect(textOf("The mock text layer")!.rect.t).toBeCloseTo(a0.t);
    expect(toastKeys()).toEqual(["edit.flow.pulled"]);
  });
});

describe("편집 · 문단 편집 · 모드 탭", () => {
  function Tabs() {
    const run = useCommands();
    return <ModeSwitcher run={run} />;
  }

  it("clicking a mode tab while the editor is open commits it and then switches (the leave guard waits for the commit the click-outside started)", async () => {
    const { ctx, surface } = await setup();
    render(<Tabs />);
    const box = await openA(ctx, surface);
    box.value = words(18);
    const real = mock.editParagraph.bind(mock);
    // engine latency: a dry run and a write are two round trips
    vi.spyOn(mock, "editParagraph").mockImplementation(async (a) => {
      await new Promise((r) => setTimeout(r, 30));
      return real(a);
    });
    const tab = screen.getByRole("tab", { name: "주석" });
    fireEvent.pointerDown(tab, { pointerId: 1, button: 0 }); // the editor's click-outside commits
    await act(async () => new Promise((r) => setTimeout(r, 40)));
    fireEvent.click(tab); // arrives while the commit is still running
    await waitFor(() => expect(useEditStore.getState().session).toBeNull());
    await waitFor(() => expect(useAppStore.getState().mode).toBe("annotate"));
    expect(toastKeys()).toEqual(["edit.flow.pushed"]);
  });
});
